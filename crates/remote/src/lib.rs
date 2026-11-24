//! Source Control's servers, behind `dnnm_application::source_control::sync::{Remote, RemoteFs}`: SFTP
//! ([`sftp`]) and FTP / FTPS ([`ftp`]), each in pure Rust - the same on Windows, macOS and Linux.
//!
//! Each connection is checked before anything is sent: an SFTP server's host key against the fingerprint the user
//! trusted, an FTPS server's certificate against the system's roots; plain FTP is used only once the user accepted
//! that it isn't encrypted (the use case asks). Files go a chunk at a time both ways, and an upload is written under
//! a temporary name and renamed over the old file at the end.

use std::sync::Arc;

use dnnm_application::source_control::model::{Environment, PLAIN_FTP_ACCEPTED, Protocol, TLS_CHECKED};
use dnnm_application::source_control::sync::{Remote, RemoteError, RemoteFs};

mod ftp;
mod sftp;

/// Reads an environment's secret (its password, or its key file's passphrase) by name from the OS's store.
pub type SecretReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The servers of every protocol, signed in with the secrets `secret` reads.
pub struct Servers {
    secret: SecretReader,
    sftp: sftp::Sftp,
}

impl Servers {
    pub fn new(secret: SecretReader) -> Self {
        Self { sftp: sftp::Sftp::new(secret.clone()), secret }
    }
}

impl Remote for Servers {
    fn host_key(&self, env: &Environment) -> Result<String, String> {
        match env.protocol {
            Protocol::Sftp => self.sftp.host_key(env),
            Protocol::Ftps => Ok(TLS_CHECKED.into()),
            Protocol::Ftp => Ok(PLAIN_FTP_ACCEPTED.into()),
        }
    }

    fn connect(&self, env: &Environment) -> Result<Box<dyn RemoteFs>, RemoteError> {
        if env.host_key.is_none() {
            return Err(RemoteError::Other("the server isn't trusted yet".into()));
        }
        match env.protocol {
            Protocol::Sftp => self.sftp.connect(env),
            Protocol::Ftps | Protocol::Ftp => ftp::connect(env, &self.secret),
        }
    }
}
