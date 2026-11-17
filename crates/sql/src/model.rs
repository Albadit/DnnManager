//! The records the SQL layer works with (`Application/Abstractions/Databases.cs`, `SiteSqlConnection.cs`,
//! `DockerStatus.cs`, `Configuration/AppOptions.cs`'s `DockerOptions` and `Domain/Models.cs`'s `DatabaseConfig`).
//! A password is only held in memory, to sign in or write web.config: no `Debug` here prints one.

use std::fmt;

/// Where a site's database lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    /// The local SQL Server container (Docker) from the settings - signed in to as its user (sa by default).
    Container,
    /// Any SQL Server or SQL Server Express instance, with Windows or SQL Server authentication.
    SqlServer,
    /// SQL Server Express LocalDB attaching the site's own `App_Data\Database.mdf` - DNN's "SQL Server Express File".
    LocalDbFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlAuthentication {
    /// Integrated security: the site signs in as its app pool's Windows identity.
    Windows,
    /// A SQL Server login with a user name and password.
    Sql,
}

/// A site's database: where it is and how the site signs in to it. `database` is the .mdf file in App_Data for
/// [`DatabaseKind::LocalDbFile`].
#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseConnection {
    pub kind: DatabaseKind,
    pub server: String,
    pub database: String,
    pub authentication: SqlAuthentication,
    pub user: String,
    pub password: String,
}

impl DatabaseConnection {
    /// LocalDB's automatic instance - the one every Windows user has.
    pub const LOCAL_DB_SERVER: &str = r"(LocalDB)\MSSQLLocalDB";
    /// The empty database file DNN's install package ships in App_Data.
    pub const LOCAL_DB_FILE_NAME: &str = "Database.mdf";

    pub fn new(kind: DatabaseKind, server: &str, database: &str, authentication: SqlAuthentication) -> Self {
        Self {
            kind,
            server: server.into(),
            database: database.into(),
            authentication,
            user: String::new(),
            password: String::new(),
        }
    }

    pub fn with_login(mut self, user: &str, password: &str) -> Self {
        self.user = user.into();
        self.password = password.into();
        self
    }

    pub fn uses_windows_authentication(&self) -> bool {
        self.kind == DatabaseKind::LocalDbFile || self.authentication == SqlAuthentication::Windows
    }

    /// "[db] on server (who)" - for messages; never the password.
    pub fn describe(&self) -> String {
        let who = if self.uses_windows_authentication() {
            "Windows authentication".to_string()
        } else {
            format!("user '{}'", self.user)
        };
        if self.kind == DatabaseKind::LocalDbFile {
            format!("App_Data\\{} with {} ({who})", self.database, self.server)
        } else {
            format!("[{}] on {} ({who})", self.database, self.server)
        }
    }

    /// The kind of connection in words, as the overview and the settings name it.
    pub fn kind_text(&self) -> &'static str {
        match self.kind {
            DatabaseKind::Container => "Local SQL container (Docker)",
            DatabaseKind::LocalDbFile => "SQL Server Express LocalDB (file)",
            DatabaseKind::SqlServer => "SQL Server",
        }
    }

    pub fn authentication_text(&self) -> &'static str {
        if self.uses_windows_authentication() { "Windows authentication" } else { "SQL Server authentication" }
    }
}

impl fmt::Display for DatabaseConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

impl fmt::Debug for DatabaseConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Passed,
    Warning,
    Failed,
}

/// One check of Test connection: what was checked ("Server reachable") and what was found - or why it failed and
/// what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseCheck {
    pub name: String,
    pub outcome: CheckOutcome,
    pub detail: String,
}

/// The checks Test connection ran, in order; it stops at the first one the rest depend on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseCheckReport {
    pub checks: Vec<DatabaseCheck>,
}

impl DatabaseCheckReport {
    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.outcome != CheckOutcome::Failed)
    }

    /// The first failed check as "Name: detail", or None when nothing failed.
    pub fn problem(&self) -> Option<String> {
        self.checks.iter().find(|c| c.outcome == CheckOutcome::Failed).map(|c| format!("{}: {}", c.name, c.detail))
    }
}

/// What a connection test is for, so it checks what that needs (`DatabaseCheckOptions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseCheckOptions {
    /// A new DNN install goes into the database: it must not hold a DNN site already.
    pub for_new_install: bool,
    /// With Windows authentication, the login the site will sign in as (`IIS APPPOOL\<pool>`); None to skip.
    pub site_login: Option<String>,
    /// The oldest SQL Server the DNN version supports (14 = SQL Server 2017 for DNN 10).
    pub minimum_major_version: i32,
    /// Only the server (Settings → Database server): no particular database.
    pub server_only: bool,
}

impl DatabaseCheckOptions {
    pub fn new(for_new_install: bool) -> Self {
        Self { for_new_install, site_login: None, minimum_major_version: 14, server_only: false }
    }
}

/// A DNN host account (superuser).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnHostAccount {
    pub user_id: i32,
    pub user_name: String,
    pub email: String,
}

/// A server, database and login as a site's web.config names them - an empty user is Windows authentication as
/// whoever runs DNN Manager.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SiteSqlConnection {
    pub server: String,
    pub database: String,
    pub user: String,
    pub password: String,
}

impl SiteSqlConnection {
    pub fn new(server: &str, database: &str, user: &str, password: &str) -> Self {
        Self { server: server.into(), database: database.into(), user: user.into(), password: password.into() }
    }
}

impl fmt::Debug for SiteSqlConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiteSqlConnection")
            .field("server", &self.server)
            .field("database", &self.database)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// The shared SQL Server container's settings (`DockerOptions`).
#[derive(Clone, PartialEq, Eq)]
pub struct DockerOptions {
    pub container_name: String,
    /// The host DNN Manager connects to SQL Server on - this machine (localhost) for the container's published port.
    pub container_ip: String,
    pub volume_name: String,
    /// The login DNN Manager signs in to the container with - sa by default.
    pub sql_user: String,
    /// The password of `sql_user`, and the sa password the container is created with.
    pub sa_password: String,
    pub default_port: u16,
    pub collation: String,
    pub mssql_pid: String,
}

impl Default for DockerOptions {
    fn default() -> Self {
        Self {
            container_name: "dnn-sqlserver".into(),
            container_ip: "localhost".into(),
            volume_name: "dnn_sqlserver_data".into(),
            sql_user: "sa".into(),
            sa_password: String::new(),
            default_port: 1433,
            collation: "Latin1_General_CI_AS".into(),
            mssql_pid: "Developer".into(),
        }
    }
}

impl DockerOptions {
    /// These options with another sa password - the one a data volume already has.
    pub fn with_sa_password(&self, password: &str) -> Self {
        Self { sa_password: password.into(), ..self.clone() }
    }
}

impl fmt::Debug for DockerOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DockerOptions")
            .field("container_name", &self.container_name)
            .field("container_ip", &self.container_ip)
            .field("volume_name", &self.volume_name)
            .field("sql_user", &self.sql_user)
            .field("default_port", &self.default_port)
            .field("collation", &self.collation)
            .field("mssql_pid", &self.mssql_pid)
            .finish_non_exhaustive()
    }
}

/// A project's database in the container (`DatabaseConfig`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub server: String,
    pub database_name: String,
    pub collation: String,
    pub port: u16,
    pub backup_directory: String,
}

/// The kind of SQL Server new projects get their database on (`DatabaseServerOptions`) - what decides whether DNN
/// Manager may sign in to a site's server as the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseServerOptions {
    /// "container", "sqlServer" or "localDbFile".
    pub kind: String,
    pub server: String,
}

impl DatabaseServerOptions {
    pub fn is_container(&self) -> bool {
        self.kind.eq_ignore_ascii_case("container")
    }

    pub fn is_local_db_file(&self) -> bool {
        self.kind.eq_ignore_ascii_case("localDbFile")
    }
}

/// Docker Desktop, its engine, and the state of the container (`DockerStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockerStatus {
    /// Docker Desktop (or at least the docker CLI) is on this PC.
    pub desktop_installed: bool,
    pub client_version: Option<String>,
    pub engine_running: bool,
    pub engine_version: Option<String>,
    /// Docker's state of the container ("running", "exited"…); None when it doesn't exist or the engine is down.
    pub container_state: Option<String>,
    /// Docker's description, e.g. "Up 2 hours (healthy)".
    pub container_status: Option<String>,
}

impl DockerStatus {
    /// Set up docker-compose can run: Docker with its engine up.
    pub fn can_set_up(&self) -> bool {
        self.engine_running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_never_prints_its_password() {
        let c = DatabaseConnection::new(DatabaseKind::SqlServer, "db", "shop", SqlAuthentication::Sql)
            .with_login("u", "Secret-123");
        assert!(!format!("{c} {c:?}").contains("Secret-123"));
        assert_eq!(c.describe(), "[shop] on db (user 'u')");
        let site = SiteSqlConnection::new("db", "shop", "u", "Secret-123");
        assert!(!format!("{site:?}").contains("Secret-123"));
        let docker = DockerOptions { sa_password: "Secret-123".into(), ..Default::default() };
        assert!(!format!("{docker:?}").contains("Secret-123"));
    }

    #[test]
    fn a_report_passes_without_a_failed_check() {
        let check = |outcome| DatabaseCheck { name: "Database".into(), outcome, detail: "x".into() };
        assert!(!DatabaseCheckReport::default().passed());
        let report = DatabaseCheckReport { checks: vec![check(CheckOutcome::Passed), check(CheckOutcome::Warning)] };
        assert!(report.passed());
        assert_eq!(report.problem(), None);
        let failed = DatabaseCheckReport { checks: vec![check(CheckOutcome::Passed), check(CheckOutcome::Failed)] };
        assert_eq!(failed.problem().as_deref(), Some("Database: x"));
    }
}
