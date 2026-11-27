//! Source Control's servers as FileZilla keeps its Site Manager: `sitemanager.xml`, or the file FileZilla's
//! File → Export… writes. [`read`] takes the sites of either (folders included) as servers; [`write`] makes a file
//! FileZilla imports. What only DNN Manager has - the mappings, the ignore and protect rules, the backup switch - goes
//! in a `<DnnManager>` element of each `<Server>`, which FileZilla leaves alone.
//!
//! Protocols: FileZilla's 0 ("use explicit FTP over TLS if available") and 4 (require it) are FTPS here - DNN
//! Manager never falls back to plain FTP by itself; 6 is plain FTP, 1 SFTP. Implicit FTPS (3) and the cloud
//! protocols are left out, each with its reason.
//!
//! Passwords: `<Pass encoding="base64">` is how FileZilla writes them - readable by anyone with the file. One
//! protected by FileZilla's master password (`encoding="crypt"`) can't be read: the site comes without it. A
//! trusted host key is never taken from a file - the first connection asks again.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use dnnm_application::source_control::model::{Auth, Environment, EnvironmentKind, Mapping, Protocol};
use dnnm_application::source_control::paths::clean;

/// A site read from a file: the server (its `id` and `project` empty - the caller gives them), its password
/// when the file had one that could be read, and what was noticed about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub env: Environment,
    pub password: Option<String>,
    /// E.g. "its password is protected by FileZilla's master password".
    pub note: Option<String>,
}

/// What [`read`] found: the sites it can use, and those it left out with why ("Shop (S3): …").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sites {
    pub sites: Vec<Site>,
    pub skipped: Vec<String>,
}

// FileZilla's ServerProtocol and LogonType numbers.
const FZ_FTP: i64 = 0;
const FZ_SFTP: i64 = 1;
const FZ_FTPS_IMPLICIT: i64 = 3;
const FZ_FTPES: i64 = 4;
const FZ_INSECURE_FTP: i64 = 6;
const FZ_LOGON_NORMAL: i64 = 1;
const FZ_LOGON_KEY: i64 = 5;

/// The sites in a FileZilla file. An error when it isn't one.
pub fn read(xml: &str) -> Result<Sites, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("The file isn't valid XML ({e})."))?;
    let root = doc.root_element();
    if !root.has_tag_name("FileZilla3") {
        return Err("The file isn't a FileZilla Site Manager file (it has no <FileZilla3> element).".into());
    }
    let mut found = Sites::default();
    // Every <Server> under <Servers>, at any depth: the Site Manager's folders nest them.
    for servers in root.children().filter(|n| n.has_tag_name("Servers")) {
        for server in servers.descendants().filter(|n| n.has_tag_name("Server")) {
            match site(server) {
                Ok(site) => found.sites.push(site),
                Err(why) => found.skipped.push(why),
            }
        }
    }
    Ok(found)
}

fn text<'a>(server: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    server.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).map(str::trim)
}

fn number(server: roxmltree::Node, tag: &str) -> Option<i64> {
    text(server, tag).and_then(|t| t.parse().ok())
}

fn site(server: roxmltree::Node) -> Result<Site, String> {
    let host = text(server, "Host").unwrap_or_default().to_string();
    let name = text(server, "Name").filter(|n| !n.is_empty()).map_or_else(|| host.clone(), str::to_string);
    let shown = if name.is_empty() { "A site without a name".to_string() } else { name.clone() };
    if host.is_empty() {
        return Err(format!("{shown}: it has no host."));
    }
    let protocol = match number(server, "Protocol").unwrap_or(FZ_FTP) {
        FZ_FTP | FZ_FTPES => Protocol::Ftps,
        FZ_SFTP => Protocol::Sftp,
        FZ_INSECURE_FTP => Protocol::Ftp,
        FZ_FTPS_IMPLICIT => return Err(format!("{shown}: implicit FTP over TLS isn't supported - use explicit TLS or SFTP.")),
        _ => return Err(format!("{shown}: its protocol isn't FTP, FTPS or SFTP.")),
    };
    let port = number(server, "Port")
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0)
        .unwrap_or_else(|| protocol.default_port());
    let logon = number(server, "Logontype").unwrap_or(FZ_LOGON_NORMAL);
    let auth = match text(server, "Keyfile").filter(|k| !k.is_empty()) {
        Some(key) if logon == FZ_LOGON_KEY && protocol == Protocol::Sftp => Auth::KeyFile(key.to_string()),
        _ => Auth::Password,
    };
    let (password, note) = password(server);
    let extra = server.children().find(|n| n.has_tag_name("DnnManager"));
    let lines = |tag: &str| -> Vec<String> {
        extra
            .and_then(|e| text(e, tag))
            .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    };
    // As the server dialog checks them: a mapping stays in the project and goes to a full server path, and no rule
    // climbs out of the project.
    let mappings = lines("Mappings")
        .into_iter()
        .filter_map(|l| {
            let (local, remote) = l.split_once("=>")?;
            let (local, remote) = (clean(local.trim())?, remote.trim());
            remote.starts_with('/').then(|| Mapping { local, remote: remote.into() })
        })
        .collect();
    let rules = |tag: &str| -> Vec<String> { lines(tag).into_iter().filter(|r| !r.contains("..")).collect() };
    let backup_remote = extra.and_then(|e| text(e, "BackupServerCopy")).is_none_or(|b| b != "0");
    let env = Environment {
        id: String::new(),
        project: String::new(),
        name,
        kind: EnvironmentKind::Production,
        protocol,
        host,
        port,
        user: text(server, "User").unwrap_or_default().to_string(),
        auth,
        remote_root: text(server, "RemoteDir").and_then(remote_dir).unwrap_or_else(|| "/".into()),
        mappings,
        ignore: rules("Ignore"),
        protected: rules("Protect"),
        backup_remote,
        host_key: None,
    };
    Ok(Site { env, password, note })
}

fn password(server: roxmltree::Node) -> (Option<String>, Option<String>) {
    let Some(pass) = server.children().find(|n| n.has_tag_name("Pass")) else { return (None, None) };
    let value = pass.text().unwrap_or_default().trim();
    if value.is_empty() {
        return (None, None);
    }
    match pass.attribute("encoding") {
        Some("base64") => match BASE64.decode(value).ok().and_then(|b| String::from_utf8(b).ok()) {
            Some(plain) => (Some(plain), None),
            None => (None, Some("its password couldn't be read".into())),
        },
        Some("crypt") => (None, Some("its password is protected by FileZilla's master password - type it in the server".into())),
        // FileZilla before 3.x kept it as it is.
        None => (Some(value.to_string()), None),
        Some(_) => (None, Some("its password is stored in a way DNN Manager can't read".into())),
    }
}

/// FileZilla's remote path - `1 0 4 site 7 wwwroot`: the server type, the prefix's length and the prefix, then each
/// folder as its length and its name (a name may hold spaces) - as `/site/wwwroot`. None when it can't be read.
pub fn remote_dir(encoded: &str) -> Option<String> {
    let chars: Vec<char> = encoded.trim().chars().collect();
    let mut at = 0;
    let next_number = |at: &mut usize| -> Option<usize> {
        let start = *at;
        while *at < chars.len() && chars[*at].is_ascii_digit() {
            *at += 1;
        }
        let n = chars[start..*at].iter().collect::<String>().parse().ok()?;
        if *at < chars.len() {
            *at += 1; // the space after it
        }
        Some(n)
    };
    let _server_type = next_number(&mut at)?;
    let mut parts = Vec::new();
    let mut first = true;
    while at < chars.len() {
        let len = next_number(&mut at)?;
        if at + len > chars.len() {
            return None;
        }
        let part: String = chars[at..at + len].iter().collect();
        at += len;
        // The space after it - an empty one (the prefix) has only the space after its length.
        if len > 0 && at < chars.len() {
            at += 1;
        }
        // The first one is the prefix (empty on a Unix path).
        if !(first || part.is_empty()) {
            parts.push(part);
        }
        first = false;
    }
    Some(format!("/{}", parts.join("/")))
}

/// `/site/wwwroot` as FileZilla writes a Unix path: `1 0 4 site 7 wwwroot`.
pub fn encode_remote_dir(path: &str) -> String {
    let mut out = String::from("1 0");
    for part in path.split('/').filter(|p| !p.is_empty()) {
        out.push_str(&format!(" {} {part}", part.chars().count()));
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A FileZilla file with these servers, each with its password when one is given.
pub fn write(servers: &[(Environment, Option<String>)]) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<FileZilla3 version=\"3.66.0\">\r\n  <Servers>\r\n");
    for (env, password) in servers {
        let protocol = match env.protocol {
            Protocol::Sftp => FZ_SFTP,
            Protocol::Ftps => FZ_FTPES,
            Protocol::Ftp => FZ_INSECURE_FTP,
        };
        let (logon, key) = match &env.auth {
            Auth::KeyFile(key) => (FZ_LOGON_KEY, Some(key)),
            Auth::Password => (FZ_LOGON_NORMAL, None),
        };
        xml.push_str("    <Server>\r\n");
        for (tag, value) in [
            ("Host", env.host.clone()),
            ("Port", env.port.to_string()),
            ("Protocol", protocol.to_string()),
            ("Type", "0".into()),
            ("User", env.user.clone()),
        ] {
            xml.push_str(&format!("      <{tag}>{}</{tag}>\r\n", escape(&value)));
        }
        if let Some(password) = password.as_deref().filter(|p| !p.is_empty()) {
            xml.push_str(&format!("      <Pass encoding=\"base64\">{}</Pass>\r\n", BASE64.encode(password)));
        }
        if let Some(key) = key {
            xml.push_str(&format!("      <Keyfile>{}</Keyfile>\r\n", escape(key)));
        }
        xml.push_str(&format!("      <Logontype>{logon}</Logontype>\r\n"));
        xml.push_str(&format!("      <Name>{}</Name>\r\n", escape(&env.name)));
        xml.push_str(&format!("      <RemoteDir>{}</RemoteDir>\r\n", escape(&encode_remote_dir(&env.remote_root))));
        xml.push_str("      <DnnManager>\r\n");
        let mappings: Vec<String> = env.mappings.iter().map(|m| format!("{} => {}", m.local, m.remote)).collect();
        for (tag, items) in [("Mappings", &mappings), ("Ignore", &env.ignore), ("Protect", &env.protected)] {
            if !items.is_empty() {
                xml.push_str(&format!("        <{tag}>{}</{tag}>\r\n", escape(&items.join("\n"))));
            }
        }
        xml.push_str(&format!("        <BackupServerCopy>{}</BackupServerCopy>\r\n", u8::from(env.backup_remote)));
        xml.push_str("      </DnnManager>\r\n    </Server>\r\n");
    }
    xml.push_str("  </Servers>\r\n</FileZilla3>\r\n");
    xml
}

/// Whether two servers are the same one: protocol, host (any case), port and user.
pub fn same_server(a: &Environment, b: &Environment) -> bool {
    a.protocol == b.protocol && a.host.eq_ignore_ascii_case(&b.host) && a.port == b.port && a.user == b.user
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, protocol: Protocol) -> Environment {
        Environment {
            id: "x".into(),
            project: String::new(),
            name: name.into(),
            kind: EnvironmentKind::Production,
            protocol,
            host: "shop.example.com".into(),
            port: protocol.default_port(),
            user: "deploy".into(),
            auth: Auth::Password,
            remote_root: "/site/wwwroot".into(),
            mappings: vec![Mapping { local: "DesktopModules/Shop".into(), remote: "/site/wwwroot/DesktopModules/Shop".into() }],
            ignore: vec!["*.pdb".into()],
            protected: vec!["robots.txt".into()],
            backup_remote: false,
            host_key: Some("SHA256:abc".into()),
        }
    }

    #[test]
    fn remote_dirs_read_and_write_as_filezilla_does() {
        assert_eq!(remote_dir("1 0 4 site 7 wwwroot").as_deref(), Some("/site/wwwroot"));
        assert_eq!(remote_dir("1 0 7 My Shop 3 www").as_deref(), Some("/My Shop/www"));
        assert_eq!(remote_dir("1 0").as_deref(), Some("/"));
        assert_eq!(remote_dir("1 0 9 short").as_deref(), None);
        assert_eq!(encode_remote_dir("/site/wwwroot"), "1 0 4 site 7 wwwroot");
        assert_eq!(encode_remote_dir("/"), "1 0");
        assert_eq!(remote_dir(&encode_remote_dir("/My Shop/www")).as_deref(), Some("/My Shop/www"));
    }

    #[test]
    fn written_servers_read_back_without_the_trusted_key() {
        let servers = vec![
            (server("Shop & Co <live>", Protocol::Sftp), Some("s3cret-é".to_string())),
            (server("Plain", Protocol::Ftp), None),
            (server("Tls", Protocol::Ftps), None),
        ];
        let read = read(&write(&servers)).unwrap();
        assert!(read.skipped.is_empty());
        assert_eq!(read.sites.len(), 3);
        let first = &read.sites[0];
        assert_eq!(first.password.as_deref(), Some("s3cret-é"));
        let expected = Environment { id: String::new(), host_key: None, ..servers[0].0.clone() };
        assert_eq!(first.env, expected);
        assert_eq!(read.sites[1].env.protocol, Protocol::Ftp);
        assert_eq!(read.sites[1].password, None);
        assert_eq!(read.sites[2].env.protocol, Protocol::Ftps);
    }

    #[test]
    fn filezillas_own_export_with_folders_and_every_kind_of_password() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FileZilla3 version="3.67.0" platform="windows">
  <Servers>
    <Server>
      <Host>ftp.example.com</Host><Port>21</Port><Protocol>0</Protocol><Type>0</Type>
      <User>web</User><Pass encoding="base64">cGFzcw==</Pass><Logontype>1</Logontype>
      <Name>Example</Name><RemoteDir>1 0 4 site 7 wwwroot</RemoteDir>
    </Server>
    <Folder expanded="1">Clients
      <Server>
        <Host>sftp.client.nl</Host><Port>2222</Port><Protocol>1</Protocol><User>me</User>
        <Keyfile>C:\keys\id_ed25519</Keyfile><Logontype>5</Logontype><Name>Client</Name>
      </Server>
      <Server>
        <Host>locked.example.com</Host><Protocol>4</Protocol><User>a</User>
        <Pass encoding="crypt" pubkey="abc">xyz</Pass><Logontype>1</Logontype><Name>Locked</Name>
      </Server>
      <Server><Host>old.example.com</Host><Protocol>3</Protocol><Name>Implicit</Name></Server>
      <Server><Host>bucket</Host><Protocol>7</Protocol><Name>S3</Name></Server>
    </Folder>
  </Servers>
</FileZilla3>"#;
        let read = read(xml).unwrap();
        assert_eq!(read.sites.len(), 3);
        let example = &read.sites[0];
        assert_eq!((example.env.protocol, example.env.port, example.password.as_deref()), (Protocol::Ftps, 21, Some("pass")));
        assert_eq!(example.env.remote_root, "/site/wwwroot");
        assert!(example.env.backup_remote, "on unless the file says otherwise");
        let client = &read.sites[1];
        assert_eq!(client.env.auth, Auth::KeyFile(r"C:\keys\id_ed25519".into()));
        assert_eq!((client.env.port, client.env.remote_root.as_str()), (2222, "/"));
        let locked = &read.sites[2];
        assert_eq!(locked.password, None);
        assert!(locked.note.as_deref().unwrap().contains("master password"));
        assert_eq!(locked.env.port, 21, "the protocol's default when none is given");
        assert_eq!(read.skipped.len(), 2);
        assert!(read.skipped[0].starts_with("Implicit: implicit FTP over TLS"));
        assert!(read.skipped[1].starts_with("S3:"));
    }

    #[test]
    fn a_file_that_isnt_filezillas_is_refused() {
        assert!(read("<settings/>").unwrap_err().contains("FileZilla"));
        assert!(read("not xml").unwrap_err().contains("XML"));
    }

    #[test]
    fn the_same_server_is_protocol_host_port_and_user() {
        let a = server("A", Protocol::Sftp);
        let b = Environment { name: "B".into(), host: "SHOP.example.com".into(), ..a.clone() };
        assert!(same_server(&a, &b));
        assert!(!same_server(&a, &Environment { user: "other".into(), ..a.clone() }));
        assert!(!same_server(&a, &Environment { protocol: Protocol::Ftps, ..a.clone() }));
    }
}
