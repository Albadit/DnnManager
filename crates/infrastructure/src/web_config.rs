//! A site's web.config: the SiteSqlServer connection (`WebConfigService.ReadDatabaseConnection`, `ConnectionStrings`).
//! A site's files are untrusted input - the site's app pool can change them - so a DTD is refused (as `SiteXml.Load`
//! does) and a `configSource` is only followed inside the site's folder, never through a link.

use std::path::{Path, PathBuf};

/// What kind of database a connection opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    SqlServer,
    /// A file attached with `AttachDbFilename` (LocalDB or DNN's shipped SQL Express User Instance).
    LocalDbFile,
}

/// The parts of a SiteSqlServer connection DNN Manager uses. The password is kept to ask the server, never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseConnection {
    pub kind: DatabaseKind,
    pub server: String,
    pub database: String,
    pub windows_authentication: bool,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for DatabaseConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConnection")
            .field("kind", &self.kind)
            .field("server", &self.server)
            .field("database", &self.database)
            .field("windows_authentication", &self.windows_authentication)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// `(localdb)\…` - LocalDB's own instances.
pub fn is_local_db(server: &str) -> bool {
    server.trim().to_ascii_lowercase().starts_with("(localdb)")
}

/// Parses a SQL Server connection string the way SqlConnectionStringBuilder reads the keys DNN uses.
pub fn parse_connection_string(text: &str) -> Option<DatabaseConnection> {
    let mut server = String::new();
    let mut database = String::new();
    let mut attach = String::new();
    let mut integrated = false;
    let mut user = String::new();
    let mut password = String::new();
    for (key, value) in split_pairs(text)? {
        match key.to_ascii_lowercase().as_str() {
            "data source" | "server" | "address" | "addr" | "network address" => server = value,
            "initial catalog" | "database" => database = value,
            "attachdbfilename" | "extended properties" | "initial file name" => attach = value,
            "integrated security" | "trusted_connection" => {
                integrated = matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "sspi")
            }
            "user id" | "uid" | "user" => user = value,
            "password" | "pwd" => password = value,
            _ => {}
        }
    }
    if !attach.trim().is_empty() {
        let file = attach.replace("|DataDirectory|", "").replace("|datadirectory|", "");
        let file = file.trim_start_matches(['\\', '/']);
        let name = Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Some(DatabaseConnection {
            kind: DatabaseKind::LocalDbFile,
            server,
            database: name,
            windows_authentication: true,
            user: String::new(),
            password: String::new(),
        });
    }
    if server.trim().is_empty() {
        return None;
    }
    Some(DatabaseConnection {
        kind: DatabaseKind::SqlServer,
        server,
        database,
        windows_authentication: integrated,
        user: if integrated { String::new() } else { user },
        password: if integrated { String::new() } else { password },
    })
}

/// `key=value;key='va;lue';key="x"` → pairs; None for a string that doesn't parse (an unclosed quote).
fn split_pairs(text: &str) -> Option<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ';' || c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Some(pairs);
        }
        let mut key = String::new();
        for c in chars.by_ref() {
            if c == '=' {
                break;
            }
            key.push(c);
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        match chars.peek().copied() {
            Some(quote @ ('\'' | '"')) => {
                chars.next();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == quote {
                        // A doubled quote is the quote itself.
                        if chars.peek() == Some(&quote) {
                            chars.next();
                            value.push(quote);
                            continue;
                        }
                        closed = true;
                        break;
                    }
                    value.push(c);
                }
                if !closed {
                    return None;
                }
            }
            _ => {
                while let Some(&c) = chars.peek() {
                    if c == ';' {
                        break;
                    }
                    value.push(c);
                    chars.next();
                }
                value = value.trim_end().to_string();
            }
        }
        pairs.push((key.trim().to_string(), value));
    }
}

/// Parses a site's XML file, refusing a DTD (entities could read other files or blow up memory).
pub fn load_site_xml(text: &str) -> Result<roxmltree::Document<'_>, String> {
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") {
        return Err("the file has a DTD, which DNN Manager doesn't read".into());
    }
    roxmltree::Document::parse(text).map_err(|e| e.to_string())
}

/// Reads the SiteSqlServer connection of `web_config`; the error says why there is none, naming the file.
pub fn read_database_connection(web_config: &Path) -> Result<DatabaseConnection, String> {
    if !web_config.is_file() {
        return Err(format!("web.config not found: {}", web_config.display()));
    }
    let text = std::fs::read_to_string(web_config).map_err(|e| e.to_string())?;
    let doc = load_site_xml(&text)?;
    let Some(section) = doc.descendants().find(|n| n.has_tag_name("connectionStrings")) else {
        return Err(format!("SiteSqlServer is missing or empty in {}.", web_config.display()));
    };

    let (raw, source) = match section.attribute("configSource").filter(|s| !s.trim().is_empty()) {
        Some(config_source) => {
            let dir = web_config.parent().unwrap_or(Path::new(""));
            let external = normalize(&dir.join(config_source));
            if !is_inside(&external, dir) || has_link(dir, &external) || !external.is_file() {
                return Err(format!("SiteSqlServer is missing or empty in {}.", external.display()));
            }
            let text = std::fs::read_to_string(&external).map_err(|e| e.to_string())?;
            let doc = load_site_xml(&text)?;
            let raw = site_sql_server(doc.root_element());
            (raw, external)
        }
        None => (site_sql_server(section), web_config.to_path_buf()),
    };
    let raw = raw.filter(|r| !r.trim().is_empty());
    let Some(raw) = raw else {
        return Err(format!("SiteSqlServer is missing or empty in {}.", source.display()));
    };
    parse_connection_string(&raw)
        .ok_or_else(|| format!("The SiteSqlServer connection string in {} can't be read.", source.display()))
}

fn site_sql_server(section: roxmltree::Node<'_, '_>) -> Option<String> {
    section
        .children()
        .filter(|n| n.has_tag_name("add"))
        .find(|n| n.attribute("name").is_some_and(|v| v.eq_ignore_ascii_case("SiteSqlServer")))
        .and_then(|n| n.attribute("connectionString"))
        .map(str::to_string)
}

/// Removes `.` and `..` without touching the disk (`Path.GetFullPath`).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `path` is `dir` or below it, compared by whole components and ignoring case (`SafePath.IsSameOrInside`).
pub fn is_inside(path: &Path, dir: &Path) -> bool {
    let path = normalize(path);
    let dir = normalize(dir);
    let mut a = path.components();
    dir.components().all(|d| {
        a.next().is_some_and(|p| p.as_os_str().to_string_lossy().eq_ignore_ascii_case(&d.as_os_str().to_string_lossy()))
    })
}

/// A folder between `dir` (exclusive) and `path` (inclusive) is a link or junction (`SafePath.HasLink`).
pub fn has_link(dir: &Path, path: &Path) -> bool {
    let dir = normalize(dir);
    let mut current = normalize(path);
    while current != dir && current.starts_with(&dir) {
        if std::fs::symlink_metadata(&current).is_ok_and(|m| is_reparse_point(&m)) {
            return true;
        }
        if !current.pop() {
            break;
        }
    }
    false
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sql_login_connections() {
        let c = parse_connection_string(
            "Server=localhost,1433;Database=dnn10;uid=sa;pwd='p;a''ss';TrustServerCertificate=True",
        )
        .unwrap();
        assert_eq!(
            (c.kind, c.server.as_str(), c.database.as_str()),
            (DatabaseKind::SqlServer, "localhost,1433", "dnn10")
        );
        assert!(!c.windows_authentication);
        assert_eq!((c.user.as_str(), c.password.as_str()), ("sa", "p;a'ss"));
        assert!(!format!("{c:?}").contains("p;a"), "Debug never shows the password");
    }

    #[test]
    fn parses_windows_authentication_and_attached_files() {
        let c =
            parse_connection_string("Data Source=.\\SQLEXPRESS;Initial Catalog=x;Integrated Security=SSPI").unwrap();
        assert!(c.windows_authentication && c.user.is_empty());
        let file = parse_connection_string(
            r"Data Source=.\SQLExpress;Integrated Security=True;User Instance=True;AttachDBFilename=|DataDirectory|Database.mdf;",
        )
        .unwrap();
        assert_eq!((file.kind, file.database.as_str()), (DatabaseKind::LocalDbFile, "Database.mdf"));
        assert!(!is_local_db(&file.server));
        assert!(is_local_db(r"(LocalDB)\MSSQLLocalDB"));
    }

    #[test]
    fn rejects_unreadable_connection_strings() {
        assert!(parse_connection_string("Database=x").is_none(), "no server");
        assert!(parse_connection_string("Server='unclosed").is_none());
    }

    #[test]
    fn reads_site_sql_server_from_web_config() {
        let dir = tempfile::tempdir().unwrap();
        let web_config = dir.path().join("web.config");
        std::fs::write(
            &web_config,
            r#"<configuration><connectionStrings>
                <add name="Other" connectionString="Server=a;Database=b" />
                <add name="SiteSqlServer" connectionString="Server=localhost;Database=dnn10;uid=sa;pwd=x" providerName="System.Data.SqlClient" />
            </connectionStrings></configuration>"#,
        )
        .unwrap();
        assert_eq!(read_database_connection(&web_config).unwrap().database, "dnn10");
    }

    #[test]
    fn follows_config_source_only_inside_the_site() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join("App_Data")).unwrap();
        std::fs::write(
            site.join("App_Data").join("cs.config"),
            r#"<connectionStrings><add name="SiteSqlServer" connectionString="Server=s;Database=inside" /></connectionStrings>"#,
        )
        .unwrap();
        std::fs::write(
            site.join("web.config"),
            r#"<configuration><connectionStrings configSource="App_Data\cs.config" /></configuration>"#,
        )
        .unwrap();
        assert_eq!(read_database_connection(&site.join("web.config")).unwrap().database, "inside");

        std::fs::write(dir.path().join("outside.config"), r#"<connectionStrings><add name="SiteSqlServer" connectionString="Server=s;Database=out" /></connectionStrings>"#)
            .unwrap();
        std::fs::write(
            site.join("web.config"),
            r#"<configuration><connectionStrings configSource="..\outside.config" /></configuration>"#,
        )
        .unwrap();
        assert!(read_database_connection(&site.join("web.config")).unwrap_err().contains("missing or empty"));
    }

    #[test]
    fn refuses_a_dtd() {
        let dir = tempfile::tempdir().unwrap();
        let web_config = dir.path().join("web.config");
        std::fs::write(
            &web_config,
            r#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///c:/windows/win.ini">]><configuration/>"#,
        )
        .unwrap();
        assert!(read_database_connection(&web_config).unwrap_err().contains("DTD"));
    }

    #[test]
    fn inside_compares_whole_components_ignoring_case() {
        assert!(is_inside(Path::new(r"C:\DNN\site\x"), Path::new(r"c:\dnn\SITE")));
        assert!(!is_inside(Path::new(r"C:\DNN\site2"), Path::new(r"C:\DNN\site")));
        assert!(!is_inside(Path::new(r"C:\DNN\site\..\other"), Path::new(r"C:\DNN\site")));
    }
}
