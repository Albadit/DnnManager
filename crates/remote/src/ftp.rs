//! FTP and FTPS (`suppaftp`, explicit TLS through the system's own TLS: Schannel on Windows, Secure Transport on macOS,
//! OpenSSL - built in - on Linux). FTPS checks the server's certificate and name against the system's trusted roots at
//! every connection; plain FTP sends everything as it is - the use case asks before it is ever used.
//!
//! Not rustls: many FTPS servers - Azure App Service's among them - offer only TLS 1.2 with CBC cipher suites
//! (ECDHE-RSA-AES256-SHA384), which rustls deliberately leaves out, so the handshake ends before it starts.
//!
//! A server lists with MLSD (exact times) where it has it, else LIST; a file's facts come from its folder's listing
//! the same way, so a comparison and the check before a transfer agree. An upload is stored under a temporary name
//! and renamed over the old file once it is whole. FTP can't set a file's time; the records keep what the server
//! lists.

use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::time::{Duration, UNIX_EPOCH};

use dnnm_application::operation::CancelToken;
use dnnm_application::source_control::model::{Environment, FileMeta, Protocol};
use dnnm_application::source_control::sync::{RemoteError, RemoteFs};
use suppaftp::list::{File, ListParser};
use suppaftp::types::FileType;
use suppaftp::{FtpError, NativeTlsConnector, NativeTlsFtpStream, Status};

use super::SecretReader;

const CONNECT_WITHIN: Duration = Duration::from_secs(20);
const CHUNK: usize = 64 * 1024;

/// The TLS settings: the system's, at TLS 1.2 or newer - the certificate and its name checked, never skipped.
fn tls_connector() -> Result<NativeTlsConnector, String> {
    suppaftp::native_tls::TlsConnector::builder()
        .min_protocol_version(Some(suppaftp::native_tls::Protocol::Tlsv12))
        .build()
        .map(NativeTlsConnector::from)
        .map_err(|e| e.to_string())
}

fn remote_error(e: FtpError) -> RemoteError {
    match &e {
        FtpError::ConnectionError(_) => RemoteError::Connection(e.to_string()),
        FtpError::UnexpectedResponse(r) if matches!(r.status, Status::NotLoggedIn | Status::StoringNeedAccount) => {
            RemoteError::Denied(e.to_string())
        }
        FtpError::UnexpectedResponse(r) if matches!(r.status, Status::FileUnavailable) && e.to_string().to_lowercase().contains("permission") => {
            RemoteError::Denied(e.to_string())
        }
        FtpError::UnexpectedResponse(r) if matches!(r.status, Status::NotAvailable | Status::TransferAborted | Status::Closing) => {
            RemoteError::Connection(e.to_string())
        }
        _ => RemoteError::Other(e.to_string()),
    }
}

/// Signs in to an FTP or FTPS server.
pub(crate) fn connect(env: &Environment, secret: &SecretReader) -> Result<Box<dyn RemoteFs>, RemoteError> {
    let ftp = sign_in(env, secret)?;
    Ok(Box::new(FtpFs { ftp: Some(ftp), mlsd: true, env: env.clone(), secret: secret.clone() }))
}

/// How often a listing whose connection was lost is tried again on a new one.
const RELIST: usize = 3;

fn sign_in(env: &Environment, secret: &SecretReader) -> Result<NativeTlsFtpStream, RemoteError> {
    let address = (env.host.as_str(), env.port)
        .to_socket_addrs()
        .map_err(|e| RemoteError::Connection(format!("{} can't be found: {e}", env.host)))?
        .next()
        .ok_or_else(|| RemoteError::Connection(format!("{} can't be found", env.host)))?;
    let mut ftp = NativeTlsFtpStream::connect_timeout(address, CONNECT_WITHIN).map_err(remote_error)?;
    if env.protocol == Protocol::Ftps {
        let connector = tls_connector().map_err(RemoteError::Other)?;
        ftp = ftp.into_secure(connector, &env.host).map_err(|e| {
            RemoteError::Other(format!(
                "FTPS couldn't be started: the server's certificate isn't one this computer trusts or isn't for {}, or the server has no FTPS ({e}). A self-signed certificate can't be checked: use SFTP, or a certificate from a known authority.",
                env.host
            ))
        })?;
    }
    let password = (secret)(&env.secret_name()).unwrap_or_default();
    ftp.login(env.user.as_str(), password.as_str()).map_err(|e| match e {
        FtpError::UnexpectedResponse(_) => RemoteError::Denied(format!("{} couldn't sign in - check the user name and the password", env.user)),
        other => remote_error(other),
    })?;
    ftp.transfer_type(FileType::Binary).map_err(remote_error)?;
    ftp.set_passive_nat_workaround(true);
    Ok(ftp)
}

struct FtpFs {
    ftp: Option<NativeTlsFtpStream>,
    /// The server answers MLSD; once it didn't, LIST is used.
    mlsd: bool,
    /// To sign in again: some servers (Azure App Service's) drop the control connection after a few hundred
    /// commands, which a big site's comparison - a listing per folder - goes past.
    env: Environment,
    secret: SecretReader,
}

/// A listed entry: name, folder or not, size, time.
struct Listed {
    name: String,
    folder: bool,
    meta: FileMeta,
}

fn listed(file: &File) -> Option<Listed> {
    let name = file.name().to_string();
    if name == "." || name == ".." || file.is_symlink() {
        return None;
    }
    let mtime = file.modified().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    Some(Listed { name, folder: file.is_directory(), meta: FileMeta::new(file.size() as u64, mtime) })
}

/// The folder and the name of a server path.
fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

impl FtpFs {
    fn ftp(&mut self) -> Result<&mut NativeTlsFtpStream, RemoteError> {
        self.ftp.as_mut().ok_or_else(|| RemoteError::Connection("the connection is closed".into()))
    }

    /// A folder's entries; None when there is no such folder. A listing whose connection is lost is tried again on a
    /// new one - listing changes nothing, so it is safe to repeat.
    fn read_folder(&mut self, folder: &str) -> Result<Option<Vec<Listed>>, RemoteError> {
        let mut tries = 0;
        loop {
            match self.read_folder_once(folder) {
                Err(RemoteError::Connection(e)) if tries < RELIST => {
                    tries += 1;
                    log::info!("FTP: the connection was lost listing {folder} ({e}) - signing in again ({tries})");
                    self.ftp = None;
                    self.ftp = Some(sign_in(&self.env, &self.secret)?);
                }
                other => return other,
            }
        }
    }

    fn read_folder_once(&mut self, folder: &str) -> Result<Option<Vec<Listed>>, RemoteError> {
        let folder = if folder.is_empty() { "/" } else { folder };
            if self.mlsd {
            match self.ftp()?.mlsd(Some(folder)) {
                Ok(lines) => return Ok(Some(lines.iter().filter_map(|l| ListParser::parse_mlsd(l).ok()).filter_map(|f| listed(&f)).collect())),
                Err(FtpError::UnexpectedResponse(r)) if matches!(r.status, Status::FileUnavailable) => return Ok(None),
                // Not understood: LIST from now on.
                Err(FtpError::UnexpectedResponse(r))
                    if matches!(r.status, Status::CommandNotImplemented | Status::BadCommand | Status::NotImplemented | Status::BadSequence) =>
                {
                    self.mlsd = false
                }
                Err(e) => return Err(remote_error(e)),
            }
        }
        match self.ftp()?.list(Some(folder)) {
            Ok(lines) => Ok(Some(lines.iter().filter_map(|l| File::try_from(l.as_str()).ok()).filter_map(|f| listed(&f)).collect())),
            Err(FtpError::UnexpectedResponse(r)) if matches!(r.status, Status::FileUnavailable) => Ok(None),
            Err(e) => Err(remote_error(e)),
        }
    }

    fn make_folders(&mut self, folder: &str) -> Result<(), RemoteError> {
        let mut path = String::new();
        for part in folder.split('/').filter(|p| !p.is_empty()) {
            path.push('/');
            path.push_str(part);
            let (parent, name) = split(&path);
            let parent = parent.to_string();
            let exists = self
                .read_folder(&parent)?
                .is_some_and(|entries| entries.iter().any(|e| e.folder && e.name == name));
            if !exists {
                self.ftp()?.mkdir(&path).map_err(remote_error)?;
            }
        }
        Ok(())
    }
}

impl RemoteFs for FtpFs {
    fn list(&mut self, folder: &str, cancel: &CancelToken) -> Result<Vec<(String, FileMeta)>, RemoteError> {
        let root = folder.trim_end_matches('/').to_string();
        let mut found = Vec::new();
        let mut pending = vec![String::new()];
        while let Some(relative) = pending.pop() {
            if cancel.is_cancelled() {
                return Err(RemoteError::Cancelled);
            }
            let dir = if relative.is_empty() { root.clone() } else { format!("{root}/{relative}") };
            let Some(entries) = self.read_folder(&dir)? else {
                // Not made yet on the server: nothing there.
                if relative.is_empty() {
                    return Ok(found);
                }
                continue;
            };
            for entry in entries {
                let path = if relative.is_empty() { entry.name } else { format!("{relative}/{}", entry.name) };
                if entry.folder {
                    pending.push(path);
                } else {
                    found.push((path, entry.meta));
                }
            }
        }
        Ok(found)
    }

    fn stat(&mut self, path: &str) -> Result<Option<FileMeta>, RemoteError> {
        let (folder, name) = split(path);
        let (folder, name) = (folder.to_string(), name.to_string());
        Ok(self.read_folder(&folder)?.and_then(|entries| entries.into_iter().find(|e| !e.folder && e.name == name)).map(|e| e.meta))
    }

    fn download(&mut self, path: &str, to: &mut dyn Write, progress: &mut dyn FnMut(u64), cancel: &CancelToken) -> Result<u64, RemoteError> {
        let mut stream = self.ftp()?.retr_as_stream(path).map_err(remote_error)?;
        let mut buffer = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            if cancel.is_cancelled() {
                // The data connection is dropped; the control one is closed too, as the server may still be sending.
                drop(stream);
                self.ftp = None;
                return Err(RemoteError::Cancelled);
            }
            let read = stream.read(&mut buffer).map_err(|e| RemoteError::Connection(e.to_string()))?;
            if read == 0 {
                break;
            }
            to.write_all(&buffer[..read]).map_err(|e| RemoteError::Other(format!("writing here: {e}")))?;
            done += read as u64;
            progress(done);
        }
        stream.finish().map_err(remote_error)?;
        Ok(done)
    }

    fn upload(&mut self, from: &mut dyn Read, path: &str, _mtime: i64, progress: &mut dyn FnMut(u64), cancel: &CancelToken) -> Result<(), RemoteError> {
        let (folder, name) = split(path);
        let partial = if folder.ends_with('/') { format!("{folder}.{name}.dnnm-part") } else { format!("{folder}/.{name}.dnnm-part") };
        if !folder.is_empty() {
            self.make_folders(folder)?;
        }
        let written: Result<(), RemoteError> = (|| {
            let mut stream = self.ftp()?.put_with_stream(&partial).map_err(remote_error)?;
            let mut buffer = vec![0u8; CHUNK];
            let mut done = 0u64;
            loop {
                if cancel.is_cancelled() {
                    drop(stream);
                    self.ftp = None;
                    return Err(RemoteError::Cancelled);
                }
                let read = from.read(&mut buffer).map_err(|e| RemoteError::Other(format!("reading here: {e}")))?;
                if read == 0 {
                    break;
                }
                stream.write_all(&buffer[..read]).map_err(|e| RemoteError::Connection(e.to_string()))?;
                done += read as u64;
                progress(done);
            }
            stream.finish().map_err(remote_error)?;
            // Over the old file; a server that won't rename onto an existing file has it removed first.
            if self.ftp()?.rename(partial.as_str(), path).is_err() {
                let _ = self.ftp()?.rm(path);
                self.ftp()?.rename(partial.as_str(), path).map_err(remote_error)?;
            }
            Ok(())
        })();
        if written.is_err()
            && let Ok(ftp) = self.ftp()
        {
            let _ = ftp.rm(&partial);
        }
        written
    }

    fn remove(&mut self, path: &str) -> Result<(), RemoteError> {
        if self.stat(path)?.is_none() {
            return Ok(());
        }
        self.ftp()?.rm(path).map_err(remote_error)
    }
}

impl Drop for FtpFs {
    fn drop(&mut self) {
        if let Some(mut ftp) = self.ftp.take() {
            let _ = ftp.quit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listings_in_either_form_are_read() {
        let mlsd = ListParser::parse_mlsd("type=file;size=1234;modify=20261123140200; web.config").unwrap();
        let entry = listed(&mlsd).unwrap();
        assert_eq!((entry.name.as_str(), entry.folder, entry.meta.size), ("web.config", false, 1234));
        assert_eq!(entry.meta.mtime, 1_795_442_520, "2026-11-23 14:02:00 UTC");
        let dir = ListParser::parse_mlsd("type=dir;modify=20261123140200; Portals").unwrap();
        assert!(listed(&dir).unwrap().folder);
        let list = File::try_from("-rw-r--r-- 1 user group 42 Nov 23 2025 robots.txt").unwrap();
        assert_eq!(listed(&list).unwrap().meta.size, 42);
        assert!(listed(&ListParser::parse_mlsd("type=cdir; .").unwrap()).is_none());
    }

    #[test]
    fn the_tls_settings_build() {
        assert!(tls_connector().is_ok());
    }
}
