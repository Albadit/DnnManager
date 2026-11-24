//! What Source Control works with: a project's environments (a server, its folder, the mappings, ignore and protect
//! rules), a file's facts on either side, what was there at the last sync, and what changed.

/// What an environment is for - Production protects its files the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnvironmentKind {
    Development,
    Test,
    Acceptance,
    Production,
}

impl EnvironmentKind {
    pub const ALL: [EnvironmentKind; 4] = [Self::Development, Self::Test, Self::Acceptance, Self::Production];

    pub fn key(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Acceptance => "acceptance",
            Self::Production => "production",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Development => "Development",
            Self::Test => "Test",
            Self::Acceptance => "Acceptance",
            Self::Production => "Production",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(key))
    }
}

/// How the server is reached. SFTP and FTPS are encrypted; plain FTP sends the password and the files as they are,
/// so it is used only after the user said so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Sftp,
    /// FTP with explicit TLS (`AUTH TLS`), its certificate checked against the system's trusted roots.
    Ftps,
    Ftp,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [Protocol::Sftp, Protocol::Ftps, Protocol::Ftp];

    pub fn key(self) -> &'static str {
        match self {
            Protocol::Sftp => "sftp",
            Protocol::Ftps => "ftps",
            Protocol::Ftp => "ftp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Protocol::Sftp => "SFTP (SSH)",
            Protocol::Ftps => "FTPS (FTP over TLS)",
            Protocol::Ftp => "FTP - unencrypted",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key().eq_ignore_ascii_case(key))
    }

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp => 22,
            Protocol::Ftps | Protocol::Ftp => 21,
        }
    }
}

/// What `Environment::host_key` holds for FTPS: its certificate is checked against the system's roots each time.
pub const TLS_CHECKED: &str = "tls:system-roots";
/// What it holds for plain FTP once the user accepted that it isn't encrypted.
pub const PLAIN_FTP_ACCEPTED: &str = "ftp:unencrypted-accepted";

/// How DNN Manager signs in to the server. The password (or the key's passphrase) is in the OS's store, never here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Auth {
    Password,
    /// An OpenSSH private key file.
    KeyFile(String),
}

/// A part of the project and where it goes on the server: `local` is a folder in the project (`/`-separated, ""
/// for all of it), `remote` a full folder on the server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mapping {
    pub local: String,
    pub remote: String,
}

/// A server the project is deployed to over SFTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// Unique among all environments; also its password's name in the OS's store.
    pub id: String,
    /// The project (IIS site) it belongs to.
    pub project: String,
    pub name: String,
    pub kind: EnvironmentKind,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// A key file only with SFTP.
    pub auth: Auth,
    /// The site's folder on the server - where the whole project goes when there are no mappings.
    pub remote_root: String,
    /// Only these parts of the project, each to its own folder (a module, a theme); empty: all of it to
    /// `remote_root`.
    pub mappings: Vec<Mapping>,
    /// Paths never compared or sent ([`super::paths::IgnoreRules`]), on top of the defaults.
    pub ignore: Vec<String>,
    /// Paths replaced or deleted on the server only after a question naming them ([`super::paths::IgnoreRules`]
    /// patterns). On Production the defaults apply too.
    pub protected: Vec<String>,
    /// A copy of each server file in the backups folder before it is replaced or deleted.
    pub backup_remote: bool,
    /// The server's host key (SHA-256 fingerprint) trusted at the first connection; a different one is refused.
    pub host_key: Option<String>,
}

impl Environment {
    /// The mappings in use: the given ones, or the whole project to `remote_root`.
    pub fn effective_mappings(&self) -> Vec<Mapping> {
        if self.mappings.is_empty() {
            vec![Mapping { local: String::new(), remote: self.remote_root.clone() }]
        } else {
            self.mappings.clone()
        }
    }

    /// "Production - sftp://deploy@shop.example.com:22".
    pub fn label(&self) -> String {
        format!("{} - {}://{}@{}:{}", self.name, self.protocol.key(), self.user, self.host, self.port)
    }

    /// Where what is kept between runs - the records of the last sync, the history, the server's last listing - is
    /// kept: per server and project, as a server isn't tied to one project (`<id>@<project>`). The trusted key and
    /// the secret are the server's (its `id`).
    pub fn state_key(&self) -> String {
        if self.project.is_empty() { self.id.clone() } else { format!("{}@{}", self.id, self.project.to_lowercase()) }
    }

    /// The name its secret (password or passphrase) has in the OS's store.
    pub fn secret_name(&self) -> String {
        format!("source-control/{}", self.id)
    }
}

/// A file's facts on one side: size, modification time (Unix seconds) and, when it was read, its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub mtime: i64,
    pub hash: Option<String>,
}

impl FileMeta {
    pub fn new(size: u64, mtime: i64) -> Self {
        Self { size, mtime, hash: None }
    }

    pub fn with_hash(mut self, hash: impl Into<String>) -> Self {
        self.hash = Some(hash.into());
        self
    }

    /// Whether this is the file `then` was: the same hash when both have one, else the same size and time (within 2
    /// seconds - FAT and some servers keep times in 2-second steps).
    pub fn same_as(&self, then: &FileMeta) -> bool {
        if let (Some(a), Some(b)) = (&self.hash, &then.hash) {
            return a == b;
        }
        self.size == then.size && (self.mtime - then.mtime).abs() <= 2
    }
}

/// Both sides as they were right after the last sync of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRecord {
    pub local: FileMeta,
    pub remote: FileMeta,
    /// When (Unix seconds).
    pub synced: i64,
}

/// What differs between the project and the server for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    /// Both sides changed since the last sync - never overwritten without a choice.
    Conflict,
    /// Changed here, not on the server: to upload.
    Modified,
    /// Only here, never synced: to upload.
    Added,
    /// Deleted here, unchanged on the server since the last sync: the server's copy may be deleted - only on
    /// request, after a question.
    Deleted,
    /// Changed on the server, not here: to download.
    Incoming,
    /// Only on the server, never synced: to download. A missing local file is never a reason to delete it.
    RemoteOnly,
    /// Deleted on the server, unchanged here since the last sync.
    DeletedOnServer,
    Unchanged,
}

impl Change {
    /// Its letter in the list (VS Code's style).
    pub fn letter(self) -> &'static str {
        match self {
            Change::Conflict => "C",
            Change::Modified => "M",
            Change::Added => "A",
            Change::Deleted => "D",
            Change::Incoming => "I",
            Change::RemoteOnly => "R",
            Change::DeletedOnServer => "S",
            Change::Unchanged => "",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Change::Conflict => "Conflict",
            Change::Modified => "Modified",
            Change::Added => "Added",
            Change::Deleted => "Deleted",
            Change::Incoming => "Changed on the server",
            Change::RemoteOnly => "Only on the server",
            Change::DeletedOnServer => "Deleted on the server",
            Change::Unchanged => "Unchanged",
        }
    }

    /// What it means, for its tooltip.
    pub fn description(self) -> &'static str {
        match self {
            Change::Conflict => "Changed here and on the server since the last sync - compare, then keep one side",
            Change::Modified => "Changed here since the last sync - upload it to the server",
            Change::Added => "New here - upload it to the server",
            Change::Deleted => "Deleted here - the server still has it; delete it there only if you mean to",
            Change::Incoming => "Changed on the server since the last sync - download it",
            Change::RemoteOnly => "Only on the server - download it, or leave it",
            Change::DeletedOnServer => "Deleted on the server - upload it again, or leave it",
            Change::Unchanged => "The same here and on the server",
        }
    }

    /// Whether uploading it changes the server.
    pub fn uploads(self) -> bool {
        matches!(self, Change::Modified | Change::Added | Change::DeletedOnServer)
    }

    /// Whether downloading it changes the project.
    pub fn downloads(self) -> bool {
        matches!(self, Change::Incoming | Change::RemoteOnly)
    }
}

/// One file's state: its path in the project (`/`-separated), both sides, and what differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub local: Option<FileMeta>,
    pub remote: Option<FileMeta>,
    pub change: Change,
}

/// One line of the synchronization history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLine {
    /// Unix seconds.
    pub at: i64,
    /// "Upload", "Download", "Delete on server", "Revert".
    pub action: String,
    pub path: String,
    /// "Done", or why not.
    pub outcome: String,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_file_is_the_same_by_hash_else_by_size_and_time() {
        let a = FileMeta::new(10, 1000);
        assert!(a.same_as(&FileMeta::new(10, 1002)), "within 2 seconds");
        assert!(!a.same_as(&FileMeta::new(10, 1003)));
        assert!(!a.same_as(&FileMeta::new(11, 1000)));
        assert!(a.clone().with_hash("x").same_as(&FileMeta::new(99, 0).with_hash("x")), "the hash decides");
        assert!(!a.clone().with_hash("x").same_as(&a.clone().with_hash("y")));
    }

    #[test]
    fn without_mappings_the_whole_project_goes_to_the_root() {
        let mut e = environment();
        assert_eq!(e.effective_mappings(), vec![Mapping { local: String::new(), remote: "/var/www/shop".into() }]);
        e.mappings = vec![Mapping { local: "DesktopModules/Shop".into(), remote: "/srv/modules/Shop".into() }];
        assert_eq!(e.effective_mappings().len(), 1);
        assert_eq!(e.secret_name(), "source-control/shop-prod");
        assert_eq!(e.label(), "Production - sftp://deploy@shop.example:22");
        assert_eq!(Protocol::from_key("FTPS"), Some(Protocol::Ftps));
        assert_eq!(Protocol::Ftp.default_port(), 21);
        assert_eq!(EnvironmentKind::from_key("PRODUCTION"), Some(EnvironmentKind::Production));
    }

    pub(crate) fn environment() -> Environment {
        Environment {
            id: "shop-prod".into(),
            project: "shop".into(),
            name: "Production".into(),
            kind: EnvironmentKind::Production,
            protocol: Protocol::Sftp,
            host: "shop.example".into(),
            port: 22,
            user: "deploy".into(),
            auth: Auth::Password,
            remote_root: "/var/www/shop".into(),
            mappings: Vec::new(),
            ignore: Vec::new(),
            protected: Vec::new(),
            backup_remote: true,
            host_key: None,
        }
    }
}
