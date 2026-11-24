//! SFTP (`russh`, `russh-sftp` - pure Rust, the same on Windows, macOS and Linux).
//!
//! - The server's host key is checked at every connection against the one the user trusted (its SHA-256
//!   fingerprint); a different one ends the connection before a password or key is sent.
//! - Sign-in by password, or by an OpenSSH key file (its passphrase, if any, is the environment's secret).
//! - Files go a chunk at a time both ways - never whole into memory. An upload is written beside its file under a
//!   temporary name and renamed over it at the end (`posix-rename@openssh.com` where the server has it), so one that
//!   breaks off leaves the old file as it was.
//! - Each connection has its own Tokio runtime (one worker), so the synchronous use cases can call it from their
//!   operation's thread.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dnnm_application::operation::CancelToken;
use dnnm_application::source_control::model::{Auth, Environment, FileMeta};
use dnnm_application::source_control::sync::{RemoteError, RemoteFs};
use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;

/// How long a server has to answer the connection and the sign-in.
const CONNECT_WITHIN: Duration = Duration::from_secs(20);
const CHUNK: usize = 64 * 1024;
/// Folders listed at once.
const LIST_AT_ONCE: usize = 8;

/// Reads an environment's secret (its password, or its key file's passphrase) by name from the OS's store.
use super::SecretReader;

/// SFTP servers, signed in with the secrets `secret` reads.
pub(crate) struct Sftp {
    secret: SecretReader,
}

impl Sftp {
    pub(crate) fn new(secret: SecretReader) -> Self {
        Self { secret }
    }
}

/// The server key's fingerprint as OpenSSH writes it: `SHA256:…`.
fn fingerprint(key: &PublicKeyOrCertificate) -> String {
    match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256).to_string(),
        PublicKeyOrCertificate::Certificate(cert) => cert.public_key().fingerprint(HashAlg::Sha256).to_string(),
    }
}

/// Checks the server's key: notes what it is, and accepts only the expected one (None: accepts nothing - the
/// connection only reads the key).
struct Checker {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let print = fingerprint(key);
        let trusted = self.expected.as_deref() == Some(print.as_str());
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(print);
        }
        Ok(trusted)
    }
}

fn runtime() -> Result<Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start the network: {e}"))
}

fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(120)),
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    })
}

impl Sftp {
    pub(crate) fn host_key(&self, env: &Environment) -> Result<String, String> {
        let rt = runtime()?;
        let seen = Arc::new(Mutex::new(None));
        let checker = Checker { expected: None, seen: seen.clone() };
        let address = (env.host.clone(), env.port);
        // The key is refused on purpose: only read, nothing sent.
        let _ = rt.block_on(async { tokio::time::timeout(CONNECT_WITHIN, client::connect(config(), address, checker)).await });
        let key = seen.lock().ok().and_then(|s| s.clone());
        key.ok_or_else(|| "the server didn't answer as an SSH server".to_string())
    }

    pub(crate) fn connect(&self, env: &Environment) -> Result<Box<dyn RemoteFs>, RemoteError> {
        let Some(expected) = env.host_key.clone() else {
            return Err(RemoteError::Other("the server's key isn't trusted yet".into()));
        };
        let rt = runtime().map_err(RemoteError::Other)?;
        let seen = Arc::new(Mutex::new(None));
        let checker = Checker { expected: Some(expected.clone()), seen: seen.clone() };
        let secret = (self.secret)(&env.secret_name());
        let (user, auth, address) = (env.user.clone(), env.auth.clone(), (env.host.clone(), env.port));
        let connected: Result<(Handle<Checker>, SftpSession), RemoteError> = rt.block_on(async {
            let session = tokio::time::timeout(CONNECT_WITHIN, client::connect(config(), address, checker))
                .await
                .map_err(|_| RemoteError::Connection("the server didn't answer in time".into()))?;
            let mut session = match session {
                Ok(s) => s,
                Err(e) => {
                    let now = seen.lock().ok().and_then(|s| s.clone());
                    return Err(match now {
                        Some(print) if print != expected => RemoteError::Other(format!(
                            "the server's key ({print}) isn't the one trusted ({expected}) - it may have been replaced, or someone is in between. Check with the server's administrator; to trust the new key, edit the environment and save it again."
                        )),
                        _ => RemoteError::Connection(e.to_string()),
                    });
                }
            };
            let result = match &auth {
                Auth::Password => {
                    let password = secret.clone().unwrap_or_default();
                    session.authenticate_password(user.clone(), password).await
                }
                Auth::KeyFile(path) => {
                    let key = russh::keys::load_secret_key(path, secret.as_deref())
                        .map_err(|e| RemoteError::Other(format!("the key file {path} can't be read: {e}")))?;
                    let hash = session.best_supported_rsa_hash().await.ok().flatten().flatten();
                    session.authenticate_publickey(user.clone(), PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await
                }
            }
            .map_err(|e| RemoteError::Connection(e.to_string()))?;
            if !result.success() {
                return Err(RemoteError::Denied(format!(
                    "{user} couldn't sign in - check the user name and the {}",
                    if matches!(auth, Auth::Password) { "password" } else { "key file and its passphrase" }
                )));
            }
            let channel = session.channel_open_session().await.map_err(|e| RemoteError::Connection(e.to_string()))?;
            channel.request_subsystem(true, "sftp").await.map_err(|e| RemoteError::Connection(e.to_string()))?;
            let sftp = SftpSession::new(channel.into_stream())
                .await
                .map_err(|e| RemoteError::Other(format!("the server has no SFTP: {e}")))?;
            Ok((session, sftp))
        });
        let (session, sftp) = connected?;
        Ok(Box::new(SftpFs { rt, sftp, _session: session }))
    }
}

/// A signed-in SFTP connection.
struct SftpFs {
    rt: Runtime,
    sftp: SftpSession,
    _session: Handle<Checker>,
}

fn status(e: &SftpError) -> Option<StatusCode> {
    match e {
        SftpError::Status(s) => Some(s.status_code),
        _ => None,
    }
}

fn remote_error(e: SftpError) -> RemoteError {
    match status(&e) {
        Some(StatusCode::PermissionDenied) => RemoteError::Denied(e.to_string()),
        Some(StatusCode::NoConnection | StatusCode::ConnectionLost) => RemoteError::Connection(e.to_string()),
        Some(_) => RemoteError::Other(e.to_string()),
        None => match e {
            SftpError::IO(_) | SftpError::Timeout => RemoteError::Connection(e.to_string()),
            other => RemoteError::Other(other.to_string()),
        },
    }
}

fn meta(attrs: &FileAttributes) -> FileMeta {
    FileMeta::new(attrs.size.unwrap_or(0), attrs.mtime.map_or(0, i64::from))
}

/// The folder and the name of a server path.
fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

impl SftpFs {
    /// Makes `folder` and those above it that aren't there.
    async fn make_folders(&self, folder: &str) -> Result<(), RemoteError> {
        let mut path = String::new();
        for part in folder.split('/') {
            if part.is_empty() {
                if path.is_empty() {
                    path.push('/');
                }
                continue;
            }
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(part);
            match self.sftp.metadata(path.clone()).await {
                Ok(attrs) if attrs.is_dir() => {}
                Ok(_) => return Err(RemoteError::Other(format!("{path} is a file, not a folder"))),
                Err(_) => self.sftp.create_dir(path.clone()).await.map_err(remote_error)?,
            }
        }
        Ok(())
    }
}

impl RemoteFs for SftpFs {
    fn list(&mut self, folder: &str, cancel: &CancelToken) -> Result<Vec<(String, FileMeta)>, RemoteError> {
        let root = folder.trim_end_matches('/').to_string();
        let sftp = &self.sftp;
        self.rt.block_on(async {
            let mut found = Vec::new();
            let mut pending = vec![String::new()];
            while !pending.is_empty() {
                if cancel.is_cancelled() {
                    return Err(RemoteError::Cancelled);
                }
                // Several folders at once over the one session: a deep tree doesn't wait a round trip a folder.
                let batch: Vec<String> = pending.drain(..pending.len().min(LIST_AT_ONCE)).collect();
                let reads = batch.iter().map(|relative| {
                    let dir = if relative.is_empty() { root.clone() } else { format!("{root}/{relative}") };
                    sftp.read_dir(if dir.is_empty() { "/".to_string() } else { dir })
                });
                for (relative, read) in batch.iter().zip(futures::future::join_all(reads).await) {
                    let entries = match read {
                        Ok(entries) => entries,
                        // Not made yet on the server: nothing there.
                        Err(e) if relative.is_empty() && status(&e) == Some(StatusCode::NoSuchFile) => return Ok(found),
                        Err(e) => return Err(remote_error(e)),
                    };
                    for entry in entries {
                        let name = entry.file_name();
                        let path = if relative.is_empty() { name.to_string() } else { format!("{relative}/{name}") };
                        let attrs = entry.metadata();
                        // Links are left alone, as on the local side.
                        if attrs.is_symlink() {
                            continue;
                        }
                        if attrs.is_dir() {
                            pending.push(path);
                        } else if attrs.is_regular() {
                            found.push((path, meta(&attrs)));
                        }
                    }
                }
            }
            Ok(found)
        })
    }

    fn stat(&mut self, path: &str) -> Result<Option<FileMeta>, RemoteError> {
        self.rt.block_on(async {
            match self.sftp.metadata(path.to_string()).await {
                Ok(attrs) if attrs.is_dir() => Err(RemoteError::Other(format!("{path} is a folder on the server"))),
                Ok(attrs) => Ok(Some(meta(&attrs))),
                Err(e) if status(&e) == Some(StatusCode::NoSuchFile) => Ok(None),
                Err(e) => Err(remote_error(e)),
            }
        })
    }

    fn download(
        &mut self,
        path: &str,
        to: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
        cancel: &CancelToken,
    ) -> Result<u64, RemoteError> {
        self.rt.block_on(async {
            let mut file = self.sftp.open(path.to_string()).await.map_err(remote_error)?;
            let mut buffer = vec![0u8; CHUNK];
            let mut done = 0u64;
            loop {
                if cancel.is_cancelled() {
                    return Err(RemoteError::Cancelled);
                }
                let read = file.read(&mut buffer).await.map_err(|e| RemoteError::Connection(e.to_string()))?;
                if read == 0 {
                    break;
                }
                to.write_all(&buffer[..read]).map_err(|e| RemoteError::Other(format!("writing here: {e}")))?;
                done += read as u64;
                progress(done);
            }
            Ok(done)
        })
    }

    fn upload(
        &mut self,
        from: &mut dyn Read,
        path: &str,
        mtime: i64,
        progress: &mut dyn FnMut(u64),
        cancel: &CancelToken,
    ) -> Result<(), RemoteError> {
        let (folder, name) = split(path);
        let partial = if folder.ends_with('/') { format!("{folder}.{name}.dnnm-part") } else { format!("{folder}/.{name}.dnnm-part") };
        self.rt.block_on(async {
            if !folder.is_empty() {
                self.make_folders(folder).await?;
            }
            let written: Result<(), RemoteError> = async {
                let mut file = self
                    .sftp
                    .open_with_flags(partial.clone(), OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE)
                    .await
                    .map_err(remote_error)?;
                let mut buffer = vec![0u8; CHUNK];
                let mut done = 0u64;
                loop {
                    if cancel.is_cancelled() {
                        return Err(RemoteError::Cancelled);
                    }
                    let read = from.read(&mut buffer).map_err(|e| RemoteError::Other(format!("reading here: {e}")))?;
                    if read == 0 {
                        break;
                    }
                    file.write_all(&buffer[..read]).await.map_err(|e| RemoteError::Connection(e.to_string()))?;
                    done += read as u64;
                    progress(done);
                }
                file.shutdown().await.map_err(|e| RemoteError::Connection(e.to_string()))?;
                if mtime > 0 {
                    let time = u32::try_from(mtime).unwrap_or(u32::MAX);
                    let attrs = FileAttributes { atime: Some(time), mtime: Some(time), ..FileAttributes::empty() };
                    // Not every server lets the time be set; the record keeps what the server says.
                    let _ = self.sftp.set_metadata(partial.clone(), attrs).await;
                }
                // Over the old file in one step where the server can; else the old one goes first.
                if !self.sftp.posix_rename(partial.clone(), path.to_string()).await.map_err(remote_error)? {
                    if self.sftp.try_exists(path.to_string()).await.unwrap_or(false) {
                        self.sftp.remove_file(path.to_string()).await.map_err(remote_error)?;
                    }
                    self.sftp.rename(partial.clone(), path.to_string()).await.map_err(remote_error)?;
                }
                Ok(())
            }
            .await;
            if written.is_err() {
                let _ = self.sftp.remove_file(partial.clone()).await;
            }
            written
        })
    }

    fn remove(&mut self, path: &str) -> Result<(), RemoteError> {
        self.rt.block_on(async {
            match self.sftp.remove_file(path.to_string()).await {
                Ok(()) => Ok(()),
                Err(e) if status(&e) == Some(StatusCode::NoSuchFile) => Ok(()),
                Err(e) => Err(remote_error(e)),
            }
        })
    }
}

impl Drop for SftpFs {
    fn drop(&mut self) {
        let _ = self.rt.block_on(self.sftp.close());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_path_splits_into_its_folder_and_name() {
        assert_eq!(split("/var/www/web.config"), ("/var/www", "web.config"));
        assert_eq!(split("/web.config"), ("/", "web.config"));
        assert_eq!(split("web.config"), ("", "web.config"));
    }

    #[test]
    fn an_untrusted_environment_is_never_signed_in() {
        let sftp = Sftp::new(Arc::new(|_| None));
        let mut env = dnnm_application::source_control::model::Environment {
            id: "x".into(),
            project: "x".into(),
            name: "Test".into(),
            kind: dnnm_application::source_control::model::EnvironmentKind::Test,
            protocol: dnnm_application::source_control::model::Protocol::Sftp,
            host: "127.0.0.1".into(),
            port: 1,
            user: "u".into(),
            auth: Auth::Password,
            remote_root: "/".into(),
            mappings: Vec::new(),
            ignore: Vec::new(),
            protected: Vec::new(),
            backup_remote: false,
            host_key: None,
        };
        assert!(matches!(sftp.connect(&env), Err(RemoteError::Other(e)) if e.contains("isn't trusted yet")));
        env.host_key = Some("SHA256:x".into());
        // Nothing listens on port 1: a connection error, not a hang.
        assert!(matches!(sftp.connect(&env), Err(RemoteError::Connection(_))));
    }
}
