//! How every connection is made (`Sql/ConnectionStrings.cs`): the settings of DNN Manager's own connections - the one
//! place the encryption is decided - and the `SiteSqlServer` connection string DNN reads from web.config, written as
//! SqlConnectionStringBuilder writes it so a password with `;`, `=` or quotes in it can't break it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::address::SqlServerAddress;
use crate::model::{DatabaseConnection, DatabaseKind, SqlAuthentication};

/// SqlClient's `Encrypt`: `Mandatory` is TLS for the whole session, `Optional` only for the login (LocalDB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encrypt {
    Mandatory,
    Optional,
}

/// One of DNN Manager's own connections (`ConnectionStrings.For`'s SqlConnectionStringBuilder).
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionSettings {
    /// As the connection string names it; [`reachable`] is applied when connecting.
    pub server: String,
    /// None: the login's default database.
    pub database: Option<String>,
    /// Empty: Windows authentication, as whoever runs DNN Manager.
    pub user: String,
    pub password: String,
    pub connect_timeout: Duration,
    pub encrypt: Encrypt,
    pub trust_server_certificate: bool,
    /// A LocalDB file database attached by its full path (`AttachDbFilename`).
    pub attach_file: Option<PathBuf>,
}

impl std::fmt::Debug for ConnectionSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionSettings")
            .field("server", &self.server)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("connect_timeout", &self.connect_timeout)
            .field("encrypt", &self.encrypt)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .field("attach_file", &self.attach_file)
            .finish_non_exhaustive()
    }
}

impl ConnectionSettings {
    pub fn integrated_security(&self) -> bool {
        self.user.is_empty()
    }

    /// The connection string SqlClient-based programs (SqlPackage) take - written in SqlConnectionStringBuilder's order
    /// and quoting. It holds the password: never logged.
    pub fn connection_string(&self) -> String {
        let mut pairs = vec![
            ("Data Source", reachable(&self.server)),
            ("Encrypt", if self.encrypt == Encrypt::Mandatory { "True" } else { "False" }.to_string()),
            ("Trust Server Certificate", if self.trust_server_certificate { "True" } else { "False" }.to_string()),
            ("Connect Timeout", self.connect_timeout.as_secs().to_string()),
        ];
        if let Some(database) = self.database.as_ref().filter(|d| !d.is_empty()) {
            pairs.push(("Initial Catalog", database.clone()));
        }
        if self.integrated_security() {
            pairs.push(("Integrated Security", "True".into()));
        } else {
            pairs.push(("User ID", self.user.clone()));
            pairs.push(("Password", self.password.clone()));
        }
        if let Some(file) = &self.attach_file {
            pairs.push(("AttachDbFilename", file.display().to_string()));
        }
        join(&pairs)
    }
}

/// Every connection DNN Manager itself makes, to `database` (None: the login's default) on `server` - as `user`, or
/// with Windows authentication as whoever runs DNN Manager when it is empty. Azure SQL requires TLS, so it is always
/// on - but for LocalDB, which is only ever on this machine; a server on this PC has a self-signed certificate, taken
/// as it is; one elsewhere must show a certificate Windows trusts.
pub fn for_server(
    server: &str,
    database: Option<&str>,
    user: &str,
    password: Option<&str>,
    timeout_seconds: u64,
) -> ConnectionSettings {
    ConnectionSettings {
        server: server.to_string(),
        database: database.filter(|d| !d.is_empty()).map(str::to_string),
        user: user.to_string(),
        password: if user.is_empty() { String::new() } else { password.unwrap_or_default().to_string() },
        connect_timeout: Duration::from_secs(timeout_seconds),
        encrypt: if is_local_db(server) { Encrypt::Optional } else { Encrypt::Mandatory },
        trust_server_certificate: trust_server_certificate(server),
        attach_file: None,
    }
}

/// DNN Manager's own connection to `database` (the connection's database when None) on the connection's server. A
/// LocalDB file database is attached by its full path under `site_directory`.
pub fn for_app(
    connection: &DatabaseConnection,
    database: Option<&str>,
    timeout_seconds: u64,
    site_directory: Option<&Path>,
) -> ConnectionSettings {
    let attach = connection.kind == DatabaseKind::LocalDbFile && database.is_none() && site_directory.is_some();
    let user = if connection.uses_windows_authentication() { "" } else { connection.user.as_str() };
    let database = if attach { None } else { Some(database.unwrap_or(&connection.database)) };
    let mut settings = for_server(&connection.server, database, user, Some(&connection.password), timeout_seconds);
    if attach {
        settings.attach_file = site_directory.map(|d| d.join("App_Data").join(&connection.database));
    }
    settings
}

/// The `SiteSqlServer` connection string DNN reads from web.config. DNN runs on .NET Framework's
/// System.Data.SqlClient, so only keywords both clients know are used (no Encrypt / Trust Server Certificate).
pub fn for_site(connection: &DatabaseConnection) -> String {
    // localhost,<port> as 127.0.0.1: the site's SqlClient reaches localhost by this computer's name and network
    // address, where the SQL container - published on this PC's loopback only - doesn't listen.
    let mut pairs = vec![("Data Source", reachable(&connection.server))];
    if connection.kind == DatabaseKind::LocalDbFile {
        // DNN's "SQL Server Express File", but without User Instance - LocalDB refuses that.
        pairs.push(("AttachDbFilename", format!("|DataDirectory|{}", connection.database)));
        pairs.push(("Integrated Security", "True".into()));
        // The site's first connection may have to create the LocalDB instance of its identity - that takes a while.
        pairs.push(("Connect Timeout", "60".into()));
        return join(&pairs);
    }
    pairs.push(("Initial Catalog", connection.database.clone()));
    if connection.uses_windows_authentication() {
        pairs.push(("Integrated Security", "True".into()));
    } else {
        pairs.push(("User ID", connection.user.clone()));
        pairs.push(("Password", connection.password.clone()));
    }
    join(&pairs)
}

/// Reads web.config's `SiteSqlServer` into a connection: integrated security or a SQL login, and `AttachDbFilename`
/// for a LocalDB file. Whether a server is the local container is the caller's to decide.
pub fn parse(connection_string: &str) -> Option<DatabaseConnection> {
    let c = dnnm_infrastructure::web_config::parse_connection_string(connection_string)?;
    let kind = match c.kind {
        dnnm_infrastructure::web_config::DatabaseKind::LocalDbFile => DatabaseKind::LocalDbFile,
        dnnm_infrastructure::web_config::DatabaseKind::SqlServer => DatabaseKind::SqlServer,
    };
    let authentication = if c.windows_authentication { SqlAuthentication::Windows } else { SqlAuthentication::Sql };
    Some(DatabaseConnection {
        kind,
        server: c.server,
        database: c.database,
        authentication,
        user: c.user,
        password: c.password,
    })
}

/// `server` as it is connected to: `localhost` with a port as `127.0.0.1`. A server published on this PC's loopback
/// only (the SQL container) isn't reached as localhost: a client may wait at ::1 when it listens on IPv4 only. A named
/// instance without a port (`localhost\SQLEXPRESS`) is left as it is.
pub fn reachable(server: &str) -> String {
    let trimmed = server.trim();
    let protocol = if trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case("tcp:") { &trimmed[..4] } else { "" };
    let rest = &trimmed[protocol.len()..];
    if let Some(comma) = rest.find(',')
        && comma > 0
        && rest[..comma].trim().eq_ignore_ascii_case("localhost")
        && let Ok(port) = rest[comma + 1..].trim().parse::<i32>()
    {
        return format!("{protocol}127.0.0.1,{port}");
    }
    server.to_string()
}

/// True for a LocalDB instance, e.g. `(LocalDB)\MSSQLLocalDB`.
pub fn is_local_db(server: &str) -> bool {
    let s = server.trim_start();
    s.len() >= 9 && s.is_char_boundary(9) && s[..9].eq_ignore_ascii_case("(localdb)")
}

/// Whether `server`'s certificate is taken without checking it: only on this computer, whose certificates are
/// self-signed and where nobody sits in between. A server elsewhere must show a certificate Windows trusts.
pub fn trust_server_certificate(server: &str) -> bool {
    SqlServerAddress::is_on_this_machine(server)
}

/// `message` - with what to do when it is about a certificate Windows doesn't trust.
pub fn explained(message: &str) -> String {
    if message.to_lowercase().contains("certificate") {
        format!(
            "{message} DNN Manager checks the certificate of a SQL Server on another computer. Use the name its certificate \
             is made out to, or - for a server of your own with a self-signed certificate - import that certificate into \
             this computer's Trusted Root Certification Authorities (certlm.msc)."
        )
    } else {
        message.to_string()
    }
}

/// `key=value;…` as DbConnectionStringBuilder writes it: a value with `"`, `'`, `=`, `;`, white space or a control
/// character in it quoted - in `'…'` when it has a `"` but no `'`, otherwise in `"…"` with `"` doubled.
fn join(pairs: &[(&str, String)]) -> String {
    pairs.iter().map(|(key, value)| format!("{key}={}", quote(value))).collect::<Vec<_>>().join(";")
}

fn quote(value: &str) -> String {
    let plain = value.chars().all(|c| !matches!(c, '"' | '\'' | '=' | ';') && !c.is_whitespace() && !c.is_control());
    if plain {
        value.to_string()
    } else if value.contains('"') && !value.contains('\'') {
        format!("'{value}'")
    } else {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_server_must_show_a_certificate_windows_trusts() {
        let b = for_server("sql.example.com", Some("shop"), "dnn", Some("p;w='d"), 30);
        assert_eq!(b.encrypt, Encrypt::Mandatory);
        assert!(!b.trust_server_certificate);
        assert_eq!(b.password, "p;w='d");
        assert!(!b.integrated_security());
        assert_eq!(b.connect_timeout, Duration::from_secs(30));
        assert!(!format!("{b:?}").contains("p;w='d"));
    }

    #[test]
    fn this_pc_is_encrypted_with_its_own_certificate() {
        let b = for_server("localhost,1433", Some("master"), "sa", Some("x"), 15);
        assert_eq!(b.encrypt, Encrypt::Mandatory);
        assert!(b.trust_server_certificate);
    }

    #[test]
    fn local_db_and_windows_authentication() {
        let b = for_server(r"(LocalDB)\MSSQLLocalDB", None, "", None, 60);
        assert_eq!(b.encrypt, Encrypt::Optional);
        assert!(b.integrated_security());
        assert_eq!(b.database, None);
    }

    #[test]
    fn localhost_with_a_port_is_reached_as_127_0_0_1() {
        for (server, reached) in [
            ("localhost,1433", "127.0.0.1,1433"),
            ("LOCALHOST, 1433", "127.0.0.1,1433"),
            ("tcp:localhost,14330", "tcp:127.0.0.1,14330"),
            (r"localhost\SQLEXPRESS", r"localhost\SQLEXPRESS"),
            ("localhost", "localhost"),
            ("sql.example.com,1433", "sql.example.com,1433"),
            (r"(LocalDB)\MSSQLLocalDB", r"(LocalDB)\MSSQLLocalDB"),
        ] {
            assert_eq!(reachable(server), reached, "{server}");
        }
    }

    #[test]
    fn connection_strings_quote_a_password_with_special_characters() {
        let connection =
            DatabaseConnection::new(DatabaseKind::SqlServer, "localhost,1433", "shop", SqlAuthentication::Sql)
                .with_login("shopuser", "a;b=c'd\"e{f}");
        let site = for_site(&connection);
        assert_eq!(
            site,
            "Data Source=127.0.0.1,1433;Initial Catalog=shop;User ID=shopuser;Password=\"a;b=c'd\"\"e{f}\""
        );
        assert!(
            !site.to_lowercase().contains("trust server certificate"),
            "DNN's System.Data.SqlClient doesn't know it."
        );
        let round_trip = parse(&site).unwrap();
        assert_eq!(round_trip, DatabaseConnection { server: "127.0.0.1,1433".into(), ..connection });
    }

    #[test]
    fn windows_authentication_and_local_db_file() {
        let windows = for_site(&DatabaseConnection::new(
            DatabaseKind::SqlServer,
            r".\SQLEXPRESS",
            "shop",
            SqlAuthentication::Windows,
        ));
        assert_eq!(windows, r"Data Source=.\SQLEXPRESS;Initial Catalog=shop;Integrated Security=True");
        let file = for_site(&DatabaseConnection::new(
            DatabaseKind::LocalDbFile,
            DatabaseConnection::LOCAL_DB_SERVER,
            "Database.mdf",
            SqlAuthentication::Windows,
        ));
        assert!(file.contains("AttachDbFilename=|DataDirectory|Database.mdf"));
        assert!(!file.to_lowercase().contains("user instance"), "LocalDB refuses User Instance.");
        assert_eq!(parse(&file).unwrap().kind, DatabaseKind::LocalDbFile);
    }

    #[test]
    fn a_site_reaches_the_container_at_127_0_0_1() {
        let written = for_site(
            &DatabaseConnection::new(DatabaseKind::Container, "localhost,1433", "shop", SqlAuthentication::Sql)
                .with_login("dnn_shop", "p"),
        );
        assert!(written.contains("Data Source=127.0.0.1,1433"));
    }

    #[test]
    fn quoting_follows_db_connection_string_builder() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote("has \"double\""), "'has \"double\"'");
        assert_eq!(quote("both \"'"), "\"both \"\"'\"");
        assert_eq!(quote("a b"), "\"a b\"");
    }

    #[test]
    fn the_app_attaches_a_local_db_file_by_its_full_path() {
        let c = DatabaseConnection::new(
            DatabaseKind::LocalDbFile,
            r"(LocalDB)\MSSQLLocalDB",
            "Database.mdf",
            SqlAuthentication::Windows,
        );
        let s = for_app(&c, None, 60, Some(Path::new(r"C:\sites\shop")));
        assert_eq!(s.attach_file, Some(PathBuf::from(r"C:\sites\shop\App_Data\Database.mdf")));
        assert_eq!(s.database, None);
        assert!(s.integrated_security());
        let master = for_app(&c, Some("master"), 60, Some(Path::new(r"C:\sites\shop")));
        assert_eq!((master.attach_file, master.database.as_deref()), (None, Some("master")));
    }

    #[test]
    fn a_certificate_problem_says_what_to_do() {
        assert!(
            explained("The certificate chain was issued by an authority that is not trusted.").contains("certlm.msc")
        );
        assert_eq!(explained("Login failed for user 'sa'."), "Login failed for user 'sa'.");
    }
}
