//! The records a site's database connection is chosen and tested with, as the dialogs and use cases see them
//! (`Application/Abstractions/Databases.cs`: `DatabaseConnection`, `DatabaseCheck`, `DatabaseCheckReport`). The SQL
//! crate has records of its own for the same things; `dnnm-wiring` turns one into the other. A password is only held
//! in memory, to sign in or to write web.config: nothing here prints one.

use std::fmt;

/// Where a site's database lives (`DatabaseKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    /// The local SQL Server container (Docker) from the settings - signed in to as its user (sa by default).
    Container,
    /// Any SQL Server or SQL Server Express instance, with Windows or SQL Server authentication.
    SqlServer,
    /// SQL Server Express LocalDB attaching the site's own `App_Data\Database.mdf`.
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

    /// Whether `other` reaches the same database the same way (`DatabaseConnectionDialog.Same`). A user name and
    /// password only count with SQL Server authentication - with Windows authentication they aren't used.
    pub fn same_as(&self, other: &DatabaseConnection) -> bool {
        self.kind == other.kind
            && eq_ignore_case(&self.server, &other.server)
            && eq_ignore_case(&self.database, &other.database)
            && self.uses_windows_authentication() == other.uses_windows_authentication()
            && (self.uses_windows_authentication() || (self.user == other.user && self.password == other.password))
    }
}

/// `StringComparison.OrdinalIgnoreCase` for names: case folded character by character.
fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

impl fmt::Debug for DatabaseConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// How one check of Test connection went.
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

/// The checks Test connection ran, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseCheckReport {
    pub checks: Vec<DatabaseCheck>,
}

impl DatabaseCheckReport {
    /// Something was checked and nothing failed.
    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.outcome != CheckOutcome::Failed)
    }
}

/// What the settings say a connection form starts from (`AppOptions`' `Docker` and `DatabaseServer`, and the
/// Credential Manager's login password) - read by the app, held in memory only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConnectionDefaults {
    /// The local container's server as connections name it: `localhost,1433` (`AppOptions.ServerFor`).
    pub container_server: String,
    /// The container's login (sa unless the settings name another).
    pub container_user: String,
    pub container_password: String,
    /// Settings → Database server: "container", "sqlServer" or "localDbFile".
    pub server_type: String,
    pub server: String,
    pub server_uses_sql_authentication: bool,
    pub server_user: String,
    /// The SQL Server login's password from the Credential Manager; empty with Windows authentication.
    pub server_password: String,
}

impl ConnectionDefaults {
    /// Settings → Database server is a SQL Server (not the container, not a LocalDB file).
    pub fn server_is_sql_server(&self) -> bool {
        !self.server_type.eq_ignore_ascii_case("container") && !self.server_type.eq_ignore_ascii_case("localDbFile")
    }

    /// Database `database` on the local container, as its user (`LocalSqlContainer.Connection`).
    pub fn container_connection(&self, database: &str) -> DatabaseConnection {
        DatabaseConnection::new(DatabaseKind::Container, &self.container_server, database, SqlAuthentication::Sql)
            .with_login(&self.container_user, &self.container_password)
    }
}

impl fmt::Debug for ConnectionDefaults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionDefaults")
            .field("container_server", &self.container_server)
            .field("container_user", &self.container_user)
            .field("server_type", &self.server_type)
            .field("server", &self.server)
            .field("server_uses_sql_authentication", &self.server_uses_sql_authentication)
            .field("server_user", &self.server_user)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql(user: &str, password: &str) -> DatabaseConnection {
        DatabaseConnection::new(DatabaseKind::SqlServer, "DB", "Shop", SqlAuthentication::Sql)
            .with_login(user, password)
    }

    #[test]
    fn describes_without_the_password() {
        let c = sql("u", "Secret-123");
        assert_eq!(c.describe(), "[Shop] on DB (user 'u')");
        assert!(!format!("{c:?}").contains("Secret-123"));
        let file = DatabaseConnection::new(
            DatabaseKind::LocalDbFile,
            r"(LocalDB)\MSSQLLocalDB",
            "Database.mdf",
            SqlAuthentication::Sql,
        );
        assert_eq!(file.describe(), r"App_Data\Database.mdf with (LocalDB)\MSSQLLocalDB (Windows authentication)");
        let defaults = ConnectionDefaults { container_password: "Secret-123".into(), ..Default::default() };
        assert!(!format!("{defaults:?}").contains("Secret-123"));
    }

    #[test]
    fn same_ignores_case_and_an_unused_login() {
        assert!(sql("u", "p").same_as(&DatabaseConnection {
            server: "db".into(),
            database: "shop".into(),
            ..sql("u", "p")
        }));
        assert!(!sql("u", "p").same_as(&sql("u", "q")));
        let windows = DatabaseConnection::new(DatabaseKind::SqlServer, "db", "shop", SqlAuthentication::Windows);
        assert!(windows.same_as(&windows.clone().with_login("x", "y")));
        assert!(!windows.same_as(&DatabaseConnection { kind: DatabaseKind::Container, ..windows.clone() }));
    }

    #[test]
    fn a_report_passes_without_a_failed_check() {
        let check = |outcome| DatabaseCheck { name: "Database".into(), outcome, detail: "x".into() };
        assert!(!DatabaseCheckReport::default().passed());
        assert!(
            DatabaseCheckReport { checks: vec![check(CheckOutcome::Passed), check(CheckOutcome::Warning)] }.passed()
        );
        assert!(!DatabaseCheckReport { checks: vec![check(CheckOutcome::Failed)] }.passed());
    }

    #[test]
    fn settings_say_whether_the_server_is_a_sql_server() {
        let mut d = ConnectionDefaults { server_type: "container".into(), ..Default::default() };
        assert!(!d.server_is_sql_server());
        d.server_type = "sqlServer".into();
        assert!(d.server_is_sql_server());
        d.server_type = "LocalDbFile".into();
        assert!(!d.server_is_sql_server());
    }
}
