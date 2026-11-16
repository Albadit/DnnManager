//! Change connection joined to SQL Server and web.config (`ChangeDatabaseConnectionUseCase`, `DatabaseProvisioner`'s
//! checks and grant, `WebConfigService.WriteDatabaseConnection`), and what its dialog needs from the settings
//! (`LocalSqlContainer.DatabaseOf`, `AppOptions`' Docker and DatabaseServer). Every function here blocks: the app runs
//! it off the UI thread.

use std::path::{Path, PathBuf};

use dnnm_application::operation::{CancelToken, Operation};
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::change_database_connection::{self as use_case, ChangeConnectionRequest};
use dnnm_application::use_cases::database_connection::{
    CheckOutcome, ConnectionDefaults, DatabaseCheck, DatabaseCheckReport, DatabaseConnection, DatabaseKind,
    SqlAuthentication,
};
use dnnm_infrastructure::web_config::{self, has_link, is_inside, load_site_xml, normalize};
use dnnm_sql::provisioner::DatabaseProvisioner;

use crate::Services;

/// The button on Details → Database, and its tooltip (`ProjectView.xaml`).
pub const CHANGE_CONNECTION_BUTTON: &str = "Change connection…";
pub const CHANGE_CONNECTION_TOOLTIP: &str = "Point web.config at another server, database or sign-in";

/// The operation's title (`ProjectEdits.ChangeDatabase`).
pub fn change_connection_title(site: &str) -> String {
    format!("Change the database connection of '{site}'")
}

/// The toast when it is done.
pub fn change_connection_done(site: &str, connection: &DatabaseConnection) -> String {
    format!("'{site}' uses {}.", connection.describe())
}

/// Change connection: the app's entry point, run as an operation.
pub fn change_database_connection(
    services: &Services,
    request: ChangeConnectionRequest,
    op: &mut Operation,
) -> Result<(), String> {
    use_case::change_database_connection(services.iis.as_ref(), &SqlConnectionChanger, &request, op)
}

/// The use case's databases and web.config: `dnnm-sql`'s provisioner and [`write_site_sql_server`].
struct SqlConnectionChanger;

impl use_case::ConnectionChanger for SqlConnectionChanger {
    fn check(
        &self,
        connection: &DatabaseConnection,
        site_login: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<DatabaseCheckReport, String> {
        check(connection, site_login, Some(cancel))
    }

    fn write_connection(&self, web_config: &Path, connection: &DatabaseConnection) -> Result<(), String> {
        write_site_sql_server(web_config, &dnnm_sql::connection_strings::for_site(&to_sql(connection)))
    }

    fn grant_site_access(
        &self,
        connection: &DatabaseConnection,
        account: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        DatabaseProvisioner.grant_site_access(&to_sql(connection), account, Some(cancel))
    }
}

/// Test connection, as the dialog runs it (`DatabaseProvisioner.CheckAsync`, not for a new install): `site_login`,
/// the Windows login the site signs in as, is checked too. Blocks for up to a minute (LocalDB may start first).
pub fn test_connection(connection: &DatabaseConnection, site_login: Option<&str>) -> DatabaseCheckReport {
    check(connection, site_login, None).unwrap_or_else(|e| DatabaseCheckReport {
        checks: vec![DatabaseCheck { name: "Test".into(), outcome: CheckOutcome::Failed, detail: e }],
    })
}

fn check(
    connection: &DatabaseConnection,
    site_login: Option<&str>,
    cancel: Option<&CancelToken>,
) -> Result<DatabaseCheckReport, String> {
    let mut options = dnnm_sql::DatabaseCheckOptions::new(false);
    options.site_login = site_login.map(str::to_string);
    let report = DatabaseProvisioner.check(&to_sql(connection), &options, cancel)?;
    Ok(DatabaseCheckReport {
        checks: report
            .checks
            .into_iter()
            .map(|c| DatabaseCheck {
                name: c.name,
                outcome: match c.outcome {
                    dnnm_sql::CheckOutcome::Passed => CheckOutcome::Passed,
                    dnnm_sql::CheckOutcome::Warning => CheckOutcome::Warning,
                    dnnm_sql::CheckOutcome::Failed => CheckOutcome::Failed,
                },
                detail: c.detail,
            })
            .collect(),
    })
}

/// The application's connection as the SQL crate's.
pub(crate) fn to_sql(c: &DatabaseConnection) -> dnnm_sql::DatabaseConnection {
    dnnm_sql::DatabaseConnection {
        kind: match c.kind {
            DatabaseKind::Container => dnnm_sql::DatabaseKind::Container,
            DatabaseKind::SqlServer => dnnm_sql::DatabaseKind::SqlServer,
            DatabaseKind::LocalDbFile => dnnm_sql::DatabaseKind::LocalDbFile,
        },
        server: c.server.clone(),
        database: c.database.clone(),
        authentication: match c.authentication {
            SqlAuthentication::Windows => dnnm_sql::SqlAuthentication::Windows,
            SqlAuthentication::Sql => dnnm_sql::SqlAuthentication::Sql,
        },
        user: c.user.clone(),
        password: c.password.clone(),
    }
}

/// What the dialog starts from: the container and Settings → Database server, with the sa password (decrypted
/// when the settings keep it encrypted) and the SQL Server login's password from the Credential Manager.
pub fn connection_defaults(settings: &Settings) -> ConnectionDefaults {
    let text = |key: &str, default: &str| settings.get(key).unwrap_or(default).trim().to_string();
    let user_name = text("sqlServer.userName", "");
    let sa = settings.get(dnnm_system::secrets::SA_PASSWORD_KEY).unwrap_or_default();
    let sa = if dnnm_system::secrets::is_protected(sa) {
        dnnm_system::secrets::unprotect(sa).unwrap_or_default()
    } else {
        sa.to_string()
    };
    let sql_authentication = text("sqlServer.authentication", "windows").eq_ignore_ascii_case("sql");
    ConnectionDefaults {
        container_server: format!("{},{}", text("sqlServer.host", "localhost"), text("sqlServer.port", "1433")),
        container_user: if user_name.is_empty() { "sa".into() } else { user_name.clone() },
        container_password: sa,
        server_type: text("sqlServer.type", "container"),
        server: text("sqlServer.server", r".\SQLEXPRESS"),
        server_uses_sql_authentication: sql_authentication,
        server_user: user_name,
        server_password: if sql_authentication {
            dnnm_system::secrets::CredentialStore
                .read(dnnm_system::secrets::names::DATABASE_SERVER_PASSWORD)
                .unwrap_or_default()
        } else {
            String::new()
        },
    }
}

/// The database the site in `directory` uses, as its web.config has it - the local container when that is where it
/// points; None when web.config has no database of its own yet, DNN's shipped "SQL Server Express File"
/// (`LocalSqlContainer.DatabaseOf`).
pub fn database_of(settings: &Settings, directory: &Path) -> Option<DatabaseConnection> {
    let read = web_config::read_database_connection(&directory.join("web.config")).ok()?;
    database_from(&read, settings)
}

pub(crate) fn database_from(read: &web_config::DatabaseConnection, settings: &Settings) -> Option<DatabaseConnection> {
    let local_file = read.kind == web_config::DatabaseKind::LocalDbFile;
    if local_file && !web_config::is_local_db(&read.server) {
        return None;
    }
    let authentication = if read.windows_authentication { SqlAuthentication::Windows } else { SqlAuthentication::Sql };
    let host = settings.get("sqlServer.host").unwrap_or("localhost").trim().to_string();
    let port = settings.get("sqlServer.port").and_then(|p| p.trim().parse().ok()).unwrap_or(1433);
    let kind = if local_file {
        DatabaseKind::LocalDbFile
    } else if !read.windows_authentication && dnnm_sql::SqlServerAddress::parse(&read.server).is_container(&host, port)
    {
        DatabaseKind::Container
    } else {
        DatabaseKind::SqlServer
    };
    Some(
        DatabaseConnection::new(kind, &read.server, &read.database, authentication)
            .with_login(&read.user, &read.password),
    )
}

/// Writes `connection_string` as web.config's `connectionStrings/add[@name='SiteSqlServer']` - in the file its
/// `configSource` names, when it names one inside the site - and as `appSettings/add[@key='SiteSqlServer']` when there
/// is one, which DNN's upgrade wizard reads (`WebConfigService.WriteConnectionString`). Only the attribute's value
/// changes; the rest of the file stays as it is, byte for byte.
pub fn write_site_sql_server(web_config: &Path, connection_string: &str) -> Result<(), String> {
    if !web_config.is_file() {
        return Err(format!("web.config not found: {}", web_config.display()));
    }
    let (file, edit) = find_entry(web_config, "connectionStrings", "name", "connectionString")?;
    let Some(edit) = edit else {
        return Err(format!("connectionStrings/add[@name='SiteSqlServer'] not found in {}.", file.display()));
    };
    save_with(&file, edit, connection_string)?;
    if let (file, Some(edit)) = find_entry(web_config, "appSettings", "key", "value")? {
        save_with(&file, edit, connection_string)?;
    }
    Ok(())
}

/// Where a value goes: the file's text, and the byte range to replace or the place to insert the attribute.
struct Edit {
    text: String,
    at: std::ops::Range<usize>,
    quote: char,
    /// The attribute isn't there yet: ` name="value"` is inserted at `at.start`.
    insert: Option<&'static str>,
}

/// `section`'s `add` element whose `key` attribute is SiteSqlServer, in web.config or the file its `configSource`
/// names (`FindAndLoadSection`): the file, and the edit of its `attribute` - None when there is no such element.
fn find_entry(
    web_config: &Path,
    section: &str,
    key: &str,
    attribute: &'static str,
) -> Result<(PathBuf, Option<Edit>), String> {
    let text = read(web_config)?;
    let doc = load_site_xml(xml(&text))?;
    let Some(node) = doc.descendants().find(|n| n.has_tag_name(section)) else {
        return Ok((web_config.to_path_buf(), None));
    };
    if let Some(source) = node.attribute("configSource").filter(|s| !s.trim().is_empty()) {
        // ASP.NET only takes a file in the site's own folder (or below) - nothing else is read or written, nor
        // through a link or junction (the site's app pool can make one).
        let dir = web_config.parent().unwrap_or(Path::new(""));
        let external = normalize(&dir.join(source));
        if !is_inside(&external, dir) || has_link(dir, &external) || !external.is_file() {
            return Ok((external, None));
        }
        let text = read(&external)?;
        let doc = load_site_xml(xml(&text))?;
        let edit = entry_edit(&text, doc.root_element(), key, attribute);
        return Ok((external, edit));
    }
    let edit = entry_edit(&text, node, key, attribute);
    Ok((web_config.to_path_buf(), edit))
}

fn entry_edit(text: &str, parent: roxmltree::Node, key: &str, attribute: &'static str) -> Option<Edit> {
    // The parser saw the text without its byte order mark.
    let bom = text.len() - xml(text).len();
    let add = parent
        .children()
        .filter(|n| n.has_tag_name("add"))
        .find(|n| n.attribute(key).is_some_and(|v| v.eq_ignore_ascii_case("SiteSqlServer")))?;
    let edit = match add.attributes().find(|a| a.name() == attribute && a.namespace().is_none()) {
        Some(a) => {
            let at = a.range_value();
            let at = at.start + bom..at.end + bom;
            let quote = text[at.end..].chars().next().unwrap_or('"');
            Edit { text: text.to_string(), at, quote, insert: None }
        }
        // After `<add`: the element's name is all that comes before its attributes.
        None => {
            let start = bom + add.range().start + "<add".len();
            Edit { text: text.to_string(), at: start..start, quote: '"', insert: Some(attribute) }
        }
    };
    Some(edit)
}

fn read(file: &Path) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|e| e.to_string())
}

/// The text without a byte order mark, for the XML parser; the mark stays in the file.
fn xml(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

/// Writes the file with the value set, through a temporary file next to it that replaces it (`Save`): never half a
/// web.config, and never through a link the file may be.
fn save_with(file: &Path, edit: Edit, value: &str) -> Result<(), String> {
    let escaped = escape_attribute(value, edit.quote);
    let piece = match edit.insert {
        Some(name) => format!(" {name}={0}{escaped}{0}", edit.quote),
        None => escaped,
    };
    let mut text = edit.text;
    text.replace_range(edit.at, &piece);
    let temp = PathBuf::from(format!("{}.dnnmanager.tmp", file.display()));
    let written = std::fs::write(&temp, text.as_bytes()).and_then(|()| std::fs::rename(&temp, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(|e| e.to_string())
}

/// `value` inside an attribute quoted with `quote`, as XmlWriter escapes it.
fn escape_attribute(value: &str, quote: char) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote == '"' => out.push_str("&quot;"),
            '\'' if quote == '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<configuration>\r\n  <connectionStrings>\r\n    \
        <add name=\"SiteSqlServer\" connectionString=\"Data Source=.\\SQLExpress;Integrated Security=True;User Instance=True;AttachDBFilename=|DataDirectory|Database.mdf;\" providerName=\"System.Data.SqlClient\" />\r\n  \
        </connectionStrings>\r\n  <appSettings>\r\n    <add key='SiteSqlServer' value='old' />\r\n    <add key=\"InstallTemplate\" value=\"DotNetNuke.install.config\" />\r\n  </appSettings>\r\n</configuration>\r\n";

    fn settings() -> Settings {
        Settings::from_rows([("sqlServer.host".to_string(), "localhost".to_string())])
    }

    #[test]
    fn writes_both_entries_and_leaves_the_rest_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("web.config");
        std::fs::write(&file, CONFIG).unwrap();
        let connection =
            DatabaseConnection::new(DatabaseKind::SqlServer, "localhost,1433", "shop", SqlAuthentication::Sql)
                .with_login("sa", "p\"a&ss'<x>");
        let value = dnnm_sql::connection_strings::for_site(&to_sql(&connection));
        write_site_sql_server(&file, &value).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with('\u{feff}') && written.contains("\r\n  <appSettings>\r\n"));
        assert!(written.contains("providerName=\"System.Data.SqlClient\" />"));
        assert!(written.contains("value=\"DotNetNuke.install.config\""));
        let read = web_config::read_database_connection(&file).unwrap();
        assert_eq!((read.server.as_str(), read.database.as_str()), ("127.0.0.1,1433", "shop"));
        assert_eq!((read.user.as_str(), read.password.as_str()), ("sa", "p\"a&ss'<x>"));
        let doc = load_site_xml(&written).unwrap();
        let app = doc.descendants().find(|n| n.attribute("key") == Some("SiteSqlServer")).unwrap();
        assert_eq!(app.attribute("value"), Some(value.as_str()));
        assert!(!dir.path().join("web.config.dnnmanager.tmp").exists());
    }

    #[test]
    fn follows_config_source_only_inside_the_site() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir(&site).unwrap();
        let file = site.join("web.config");
        std::fs::write(&file, r#"<configuration><connectionStrings configSource="cs.config" /></configuration>"#)
            .unwrap();
        std::fs::write(
            site.join("cs.config"),
            r#"<connectionStrings><add name="SiteSqlServer" /></connectionStrings>"#,
        )
        .unwrap();
        write_site_sql_server(&file, "Data Source=x;Initial Catalog=y;Integrated Security=True").unwrap();
        assert_eq!(web_config::read_database_connection(&file).unwrap().database, "y");

        std::fs::write(&file, r#"<configuration><connectionStrings configSource="..\out.config" /></configuration>"#)
            .unwrap();
        std::fs::write(
            dir.path().join("out.config"),
            r#"<connectionStrings><add name="SiteSqlServer" connectionString="a" /></connectionStrings>"#,
        )
        .unwrap();
        let error = write_site_sql_server(&file, "b").unwrap_err();
        assert!(error.starts_with("connectionStrings/add[@name='SiteSqlServer'] not found in "), "{error}");
        assert!(std::fs::read_to_string(dir.path().join("out.config")).unwrap().contains("\"a\""));
        assert!(
            write_site_sql_server(&site.join("missing.config"), "b").unwrap_err().starts_with("web.config not found: ")
        );
    }

    #[test]
    fn the_container_and_dnns_shipped_file_are_told_apart() {
        let parse = |s: &str| web_config::parse_connection_string(s).unwrap();
        let container = database_from(&parse("Server=localhost,1433;Database=shop;uid=sa;pwd=x"), &settings()).unwrap();
        assert_eq!((container.kind, container.user.as_str()), (DatabaseKind::Container, "sa"));
        let other = database_from(&parse("Server=db,1433;Database=shop;uid=sa;pwd=x"), &settings()).unwrap();
        assert_eq!(other.kind, DatabaseKind::SqlServer);
        let windows =
            database_from(&parse("Server=localhost,1433;Database=shop;Integrated Security=True"), &settings());
        assert_eq!(windows.unwrap().kind, DatabaseKind::SqlServer);
        let shipped = parse(
            r"Data Source=.\SQLExpress;Integrated Security=True;User Instance=True;AttachDBFilename=|DataDirectory|Database.mdf;",
        );
        assert!(database_from(&shipped, &settings()).is_none());
        let file = parse(
            r"Data Source=(LocalDB)\MSSQLLocalDB;AttachDbFilename=|DataDirectory|Site.mdf;Integrated Security=True",
        );
        let file = database_from(&file, &settings()).unwrap();
        assert_eq!((file.kind, file.database.as_str()), (DatabaseKind::LocalDbFile, "Site.mdf"));
    }

    #[test]
    fn defaults_follow_the_settings() {
        let defaults = connection_defaults(&Settings::from_rows([
            ("sqlServer.host".to_string(), "localhost".to_string()),
            ("sqlServer.port".to_string(), "1434".to_string()),
            ("sqlServer.saPassword".to_string(), "Secret-123".to_string()),
        ]));
        assert_eq!(defaults.container_server, "localhost,1434");
        assert_eq!((defaults.container_user.as_str(), defaults.container_password.as_str()), ("sa", "Secret-123"));
        assert!(!defaults.server_uses_sql_authentication && defaults.server_password.is_empty());
        assert_eq!(change_connection_title("shop"), "Change the database connection of 'shop'");
    }
}
