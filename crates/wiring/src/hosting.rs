//! What Clone, Host existing, Import and Set up docker-compose act on: the traits of
//! `dnnm_application::use_cases::hosting` implemented with the SQL, DNN, Windows and IIS crates - the local container
//! through sqlcmd (`SqlServerService`), SqlPackage, BACKUP DATABASE on another server, the site copier and zip, and the
//! parts of `WebConfigs/WebConfigService.cs` these need that no other crate has: reading SiteSqlServer with the
//! clone's messages, removing the URL Rewrite section, switching HTTPS redirect rules off, and the loopback address.
//!
//! web.config is changed in place: only the bytes of what changes are rewritten, through a file next to it that then
//! takes its place - never half a web.config. The C# rewrites the whole document with XDocument.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dnnm_application::iis::IisManager;
use dnnm_application::operation::{CancelToken, Operation, Reporter};
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::hosting::{
    DatabaseCopies, HostingDeps, HostingSettings, HttpsRedirectRules, LocalSql, Machine, ProjectFiles, SiteDatabases,
    SiteSql, SiteWebConfig,
};
use dnnm_application::use_cases::sql_container::SqlContainer;
use dnnm_domain::DatabaseConfig;
use dnnm_infrastructure::web_config::{self, has_link, is_inside, load_site_xml, normalize};
use dnnm_sql::container::SqlServerService;
use dnnm_sql::provisioner::DatabaseProvisioner;
use dnnm_sql::tester::{SqlClientTester, SqlConnectionTester};
use dnnm_sql::{
    DatabaseConnection, DatabaseKind, DockerOptions, SiteSqlConnection, SqlAuthentication, SqlServerAddress,
};

use crate::Services;

/// A local server either answers at once or isn't running.
const CONNECT_TIMEOUT_SECONDS: u64 = 5;

/// The settings as these use cases read them (`AppOptions`).
pub fn hosting_settings(settings: &Settings, docker: &DockerOptions) -> HostingSettings {
    HostingSettings {
        base_directory: PathBuf::from(settings.base_directory().trim()),
        backups_directory: dnnm_infrastructure::paths::app_data_directory().join("backups"),
        hostname_suffix: settings.get("projects.hostnameSuffix").unwrap_or("dnndev.me").trim().trim_matches('.').into(),
        site_port: settings.get("projects.sitePort").and_then(|p| p.trim().parse().ok()).unwrap_or(80),
        container_ip: docker.container_ip.clone(),
        container_port: docker.default_port,
        sql_user: docker.sql_user.clone(),
        collation: docker.collation.clone(),
    }
}

/// Clone's, Host existing's and Import's dependencies over the real system. Err when the settings' sa password can't
/// be decrypted.
pub fn hosting_deps(services: &Services) -> Result<HostingDeps, String> {
    let docker = crate::remove_project::docker_options(services)?;
    let web_config: Arc<dyn SiteWebConfig> = Arc::new(WebConfigFile);
    Ok(HostingDeps {
        settings: hosting_settings(&services.settings, &docker),
        iis: Arc::clone(&services.iis),
        sql: Arc::new(ContainerSql { service: SqlServerService::new(docker.clone()), docker: docker.clone() }),
        copies: Arc::new(Copies { docker }),
        web_config,
        sites: Arc::new(IisSiteDatabases { iis: Arc::clone(&services.iis) }),
        files: Arc::new(Files),
        machine: Arc::new(Pc { features: iis_features(&services.settings) }),
    })
}

/// `iis.requiredFeatures` of the settings - each item's `name` and `label` row - or the defaults.
fn iis_features(settings: &Settings) -> Vec<dnnm_system::prereq::IisFeature> {
    let key = "iis.requiredFeatures";
    let count: usize = settings.rows().get(key).and_then(|c| c.parse().ok()).unwrap_or(0);
    let listed: Vec<_> = (0..count)
        .filter_map(|i| {
            let name = settings.rows().get(&format!("{key}[{i}].name"))?.clone();
            let label = settings.rows().get(&format!("{key}[{i}].label")).cloned().unwrap_or_else(|| name.clone());
            Some(dnnm_system::prereq::IisFeature { name, label })
        })
        .collect();
    if settings.rows().contains_key(key) { listed } else { dnnm_system::prereq::default_features() }
}

fn to_site(c: &SiteSql) -> SiteSqlConnection {
    SiteSqlConnection::new(&c.server, &c.database, &c.user, &c.password)
}

fn to_sql_config(db: &DatabaseConfig) -> dnnm_sql::DatabaseConfig {
    dnnm_sql::DatabaseConfig {
        server: db.server.clone(),
        database_name: db.database_name.clone(),
        collation: db.collation.clone(),
        port: db.port,
        backup_directory: db.backup_directory.display().to_string(),
    }
}

// ─── The local container ─────────────────────────────────────────────────────

/// `ISqlServerService` (sqlcmd in the container) and `LocalSqlContainer`, signed in to as the settings' user.
struct ContainerSql {
    service: SqlServerService,
    docker: DockerOptions,
}

impl ContainerSql {
    fn server(&self) -> String {
        format!("{},{}", self.docker.container_ip, self.docker.default_port)
    }

    fn connection(&self, database: &str) -> DatabaseConnection {
        DatabaseConnection::new(DatabaseKind::Container, &self.server(), database, SqlAuthentication::Sql)
            .with_login(&self.docker.sql_user, &self.docker.sa_password)
    }
}

impl LocalSql for ContainerSql {
    fn test_login(&self, cancel: &CancelToken) -> Result<String, String> {
        let master = SiteSqlConnection::new(&self.server(), "master", &self.docker.sql_user, &self.docker.sa_password);
        SqlClientTester.test(&master, CONNECT_TIMEOUT_SECONDS, Some(cancel))
    }
    fn is_local_container(&self, server: &str, port: u16) -> bool {
        SqlServerAddress::parse(server).is_container(&self.docker.container_ip, port)
    }
    fn database_exists(&self, database: &str, cancel: &CancelToken) -> Result<bool, String> {
        self.service.database_exists(database, cancel)
    }
    fn create_database(&self, database: &DatabaseConfig, cancel: &CancelToken) -> Result<(), String> {
        self.service.create_database(&to_sql_config(database), cancel)
    }
    fn drop_database(&self, database: &str, cancel: &CancelToken) -> Result<(), String> {
        self.service.drop_database(database, cancel)
    }
    fn rename_database(&self, database: &str, new_name: &str, cancel: &CancelToken) -> Result<(), String> {
        DatabaseProvisioner.rename_database(&self.connection(database), new_name, Some(cancel))
    }
    fn backup_local(&self, database: &str, file_name: &str, cancel: &CancelToken) -> Result<PathBuf, String> {
        self.service.backup_database_local(database, file_name, cancel)
    }
    fn restore_local(&self, database: &DatabaseConfig, file: &Path, cancel: &CancelToken) -> Result<(), String> {
        self.service.restore_database_local(&to_sql_config(database), file, cancel)
    }
    fn remap_portal_aliases(
        &self,
        database: &str,
        hostname_suffix: &str,
        alias: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<dnnm_application::use_cases::portal_aliases::Localized>, String> {
        self.service.remap_portal_aliases(database, hostname_suffix, alias, cancel)
    }
    fn disable_ssl(&self, database: &str, cancel: &CancelToken) -> Result<i64, String> {
        self.service.disable_ssl(database, cancel)
    }
    fn grant_site_login(&self, database: &str, login: &str, cancel: &CancelToken) -> Result<String, String> {
        let password = crate::setup_project::new_sql_password();
        self.service.grant_site_login(database, login, &password, cancel)?;
        Ok(password)
    }
    fn drop_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String> {
        self.service.drop_login(login, cancel)
    }
}

/// SqlPackage and BACKUP DATABASE on another server.
struct Copies {
    docker: DockerOptions,
}

impl DatabaseCopies for Copies {
    fn ensure_sqlpackage(&self, report: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String> {
        dnnm_sql::sqlpackage::ensure_available(report, cancel)
    }
    fn export_bacpac(
        &self,
        source: &SiteSql,
        file: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        dnnm_sql::sqlpackage::export(&to_site(source), file, report, cancel)
    }
    fn import_bacpac(
        &self,
        server: &str,
        database: &str,
        file: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let d = &self.docker;
        dnnm_sql::sqlpackage::import(server, &d.sql_user, &d.sa_password, database, file, report, cancel)
    }
    fn backup_remote(
        &self,
        source: &SiteSql,
        server_path: &str,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<String, String> {
        dnnm_sql::remote_backup::backup(&to_site(source), server_path, report, cancel)
    }
}

/// `SiteDatabases`: what the IIS sites' web.config files name.
struct IisSiteDatabases {
    iis: Arc<dyn IisManager>,
}

impl IisSiteDatabases {
    /// Every IIS site but `site` with its web.config and the database it names - not a LocalDB file.
    fn other_sites(&self, site: &str) -> Vec<(String, PathBuf, web_config::DatabaseConnection)> {
        let Ok(sites) = self.iis.site_runtimes() else { return Vec::new() };
        sites
            .into_iter()
            .filter(|(name, runtime)| !name.eq_ignore_ascii_case(site) && !runtime.physical_path.as_os_str().is_empty())
            .filter_map(|(name, runtime)| {
                let file = runtime.physical_path.join("web.config");
                let connection = web_config::read_database_connection(&file).ok()?;
                (connection.kind != web_config::DatabaseKind::LocalDbFile).then_some((name, file, connection))
            })
            .collect()
    }
}

impl SiteDatabases for IisSiteDatabases {
    fn other_site_using(&self, site: &str, server: &str, database: &str) -> Option<String> {
        if database.is_empty() {
            return None;
        }
        let address = SqlServerAddress::parse(server);
        self.other_sites(site)
            .into_iter()
            .find(|(_, _, c)| {
                c.database.eq_ignore_ascii_case(database) && address.same_server_as(&SqlServerAddress::parse(&c.server))
            })
            .map(|(name, _, _)| name)
    }
    fn other_site_signing_in_as(&self, site: &str, server: &str, login: &str) -> Option<String> {
        if login.is_empty() {
            return None;
        }
        let address = SqlServerAddress::parse(server);
        self.other_sites(site)
            .into_iter()
            .find(|(_, _, c)| {
                c.user.eq_ignore_ascii_case(login) && address.same_server_as(&SqlServerAddress::parse(&c.server))
            })
            .map(|(name, _, _)| name)
    }
    fn reaching_as_localhost(&self, server: &str) -> Vec<(String, PathBuf)> {
        let address = SqlServerAddress::parse(server);
        self.other_sites("")
            .into_iter()
            .filter(|(_, _, c)| {
                let named = SqlServerAddress::parse(&c.server);
                named.port.is_some() && named.host.eq_ignore_ascii_case("localhost") && address.same_server_as(&named)
            })
            .map(|(name, file, _)| (name, file))
            .collect()
    }
}

/// The site copier, the zip, the .gitignore and the private temporary folder.
struct Files;

impl ProjectFiles for Files {
    fn copy_site(
        &self,
        source: &Path,
        target: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        // The C#'s in-place progress line has no counterpart in the port's Reporter.
        dnnm_system::files::copier::copy(source, target, report, &mut |_| {}, cancel)
    }
    fn extract_zip(
        &self,
        zip: &Path,
        target: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        dnnm_system::files::copier::extract_zip(zip, target, report, &mut |_| {}, cancel).map_err(|e| e.to_string())
    }
    fn ensure_gitignore(&self, directory: &Path) -> Result<(), String> {
        crate::setup_project::ensure_gitignore(directory)
    }
    fn private_temp(&self) -> Result<PathBuf, String> {
        dnnm_sql::dirs::temp()
    }
}

/// IIS's Windows features, the site's first answer and the clock.
struct Pc {
    features: Vec<dnnm_system::prereq::IisFeature>,
}

impl Machine for Pc {
    fn ensure_iis_features(&self, op: &mut Operation) -> Result<(), String> {
        dnnm_system::prereq::ensure_iis_features(&self.features, op)
    }
    fn check_http(&self, url: &str, timeout_seconds: u64, cancel: &CancelToken) -> Result<u16, String> {
        dnnm_dnn::connectivity::check(url, timeout_seconds, cancel)
    }
    fn stamp(&self) -> String {
        chrono::Local::now().format("%Y%m%d_%H%M%S").to_string()
    }
}

// ─── Docker ──────────────────────────────────────────────────────────────────

/// `docker compose up` and the sa login for Set up docker-compose.
pub(crate) struct Compose {
    pub docker: DockerOptions,
    pub started: Instant,
}

impl SqlContainer for Compose {
    fn up(&self, sa_password: &str, report: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String> {
        dnnm_sql::docker::up(&self.docker.with_sa_password(sa_password), report, cancel)
    }
    fn test_sa(&self, server: &str, password: &str, cancel: &CancelToken) -> Result<String, String> {
        SqlClientTester.test(
            &SiteSqlConnection::new(server, "master", "sa", password),
            CONNECT_TIMEOUT_SECONDS,
            Some(cancel),
        )
    }
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn wait(&self, how_long: Duration, cancel: &CancelToken) -> Result<(), String> {
        let until = Instant::now() + how_long;
        while Instant::now() < until {
            cancel.check()?;
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

// ─── web.config ──────────────────────────────────────────────────────────────

/// The comment that marks a rule DNN Manager switched off (`WebConfigService.DisabledRuleComment`).
pub const DISABLED_RULE_COMMENT: &str = " Disabled by DNN Manager for local development (no HTTPS locally). Re-enable \
                                         (remove enabled=\"false\") before deploying to production. ";

/// `IWebConfigService`'s parts these use cases need.
pub(crate) struct WebConfigFile;

impl SiteWebConfig for WebConfigFile {
    fn read_site_sql_server(&self, web_config: &Path) -> Result<SiteSql, String> {
        read_site_sql_server(web_config)
    }
    fn write_site_sql_server(&self, web_config: &Path, connection: &SiteSql) -> Result<(), String> {
        let site = DatabaseConnection::new(
            DatabaseKind::SqlServer,
            &connection.server,
            &connection.database,
            SqlAuthentication::Sql,
        )
        .with_login(&connection.user, &connection.password);
        crate::change_database_connection::write_site_sql_server(
            web_config,
            &dnnm_sql::connection_strings::for_site(&site),
        )
    }
    fn remove_rewrite_rules(&self, web_config: &Path) -> Result<(), String> {
        remove_rewrite_rules(web_config)
    }
    fn disable_https_redirect_rules(&self, web_config: &Path) -> Result<HttpsRedirectRules, String> {
        disable_https_redirect_rules(web_config)
    }
    fn use_loopback_address(&self, web_config: &Path) -> Result<bool, String> {
        use_loopback_address(web_config)
    }
}

/// The text without a byte order mark, for the parser - and the mark's length, which the parser's offsets leave out.
fn xml(text: &str) -> (&str, usize) {
    let body = text.trim_start_matches('\u{feff}');
    (body, text.len() - body.len())
}

fn read(file: &Path) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|e| e.to_string())
}

/// Writes `text` through a file next to `file` that then takes its place (`WebConfigService.Save`).
fn save(file: &Path, text: &str) -> Result<(), String> {
    let temp = PathBuf::from(format!("{}.dnnmanager.tmp", file.display()));
    let written = std::fs::write(&temp, text.as_bytes()).and_then(|()| std::fs::rename(&temp, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(|e| e.to_string())
}

/// Applies `edits` (byte range in the parsed text → new text), last first so the earlier ranges stay right.
fn apply(text: &str, bom: usize, mut edits: Vec<(std::ops::Range<usize>, String)>) -> String {
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = text.to_string();
    for (range, piece) in edits {
        out.replace_range(range.start + bom..range.end + bom, &piece);
    }
    out
}

/// The raw SiteSqlServer connection string and the file it is in - web.config, or the file its `configSource` names
/// inside the site (`FindConnectionStringElement`). None for the string when there is no such entry.
fn site_sql_server_raw(web_config: &Path) -> Result<(Option<String>, PathBuf), String> {
    let text = read(web_config)?;
    let doc = load_site_xml(xml(&text).0)?;
    let Some(section) = doc.descendants().find(|n| n.has_tag_name("connectionStrings")) else {
        return Ok((None, web_config.to_path_buf()));
    };
    let find = |parent: roxmltree::Node| {
        parent
            .children()
            .filter(|n| n.has_tag_name("add"))
            .find(|n| n.attribute("name").is_some_and(|v| v.eq_ignore_ascii_case("SiteSqlServer")))
            .map(|n| n.attribute("connectionString").unwrap_or_default().to_string())
    };
    match section.attribute("configSource").filter(|s| !s.trim().is_empty()) {
        Some(source) => {
            let dir = web_config.parent().unwrap_or(Path::new(""));
            let external = normalize(&dir.join(source));
            if !is_inside(&external, dir) || has_link(dir, &external) || !external.is_file() {
                return Ok((None, external));
            }
            let text = read(&external)?;
            let doc = load_site_xml(xml(&text).0)?;
            Ok((find(doc.root_element()), external))
        }
        None => Ok((find(section), web_config.to_path_buf())),
    }
}

/// `key=value` pairs of a connection string, quotes honoured; None when a quote isn't closed.
fn pairs(text: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ';' || c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Some(out);
        }
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if let Some(quote @ ('\'' | '"')) = chars.peek().copied() {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == quote {
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
        } else {
            while let Some(&c) = chars.peek() {
                if c == ';' {
                    break;
                }
                value.push(c);
                chars.next();
            }
            value = value.trim_end().to_string();
        }
        out.push((key.trim().to_string(), value));
    }
}

/// Which of SqlConnectionStringBuilder's keys `key` is, by its synonyms.
fn canonical(key: &str) -> &'static str {
    match key.to_ascii_lowercase().as_str() {
        "data source" | "server" | "address" | "addr" | "network address" => "data source",
        "initial catalog" | "database" => "initial catalog",
        "attachdbfilename" | "extended properties" | "initial file name" => "attachdbfilename",
        "user id" | "uid" | "user" => "user id",
        "password" | "pwd" => "password",
        _ => "",
    }
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

/// SiteSqlServer with its server and database (`WebConfigService.ReadSiteSqlServer`); the error says why there is
/// none, never the connection string itself (it can hold a password).
pub fn read_site_sql_server(web_config: &Path) -> Result<SiteSql, String> {
    if !web_config.is_file() {
        return Err(format!("web.config not found: {}", web_config.display()));
    }
    let (raw, source) = site_sql_server_raw(web_config)?;
    let source = source.display();
    let Some(raw) = raw else {
        return Err(format!("connectionStrings/add[@name='SiteSqlServer'] not found in {source}."));
    };
    if raw.trim().is_empty() {
        return Err(format!("SiteSqlServer connectionString attribute is empty in {source}."));
    }
    let Some(pairs) = pairs(&raw) else {
        return Err(format!(
            "Failed to parse the SiteSqlServer connection string in {source}: a quoted value isn't closed."
        ));
    };
    let value =
        |key: &str| pairs.iter().rev().find(|(k, _)| canonical(k) == key).map(|(_, v)| v.clone()).unwrap_or_default();
    let (server, database, attach) = (value("data source"), value("initial catalog"), value("attachdbfilename"));
    if server.trim().is_empty() || database.trim().is_empty() {
        if !attach.trim().is_empty() {
            return Err(format!(
                "This site uses LocalDB (AttachDBFilename='{attach}') and has no Initial Catalog. There's no SQL Server \
                 database to back up - connect the project to a real DNN database first, then retry the clone. (from {source})"
            ));
        }
        return Err(format!(
            "SiteSqlServer is missing Data Source or Initial Catalog. DataSource='{server}', InitialCatalog='{database}' \
             (from {source})."
        ));
    }
    Ok(SiteSql { server, database, user: value("user id"), password: value("password") })
}

/// SiteSqlServer's `localhost,<port>` as `127.0.0.1,<port>`, the rest of it as it is (`UseLoopbackAddress`).
pub fn use_loopback_address(web_config: &Path) -> Result<bool, String> {
    if !web_config.is_file() {
        return Err(format!("web.config not found: {}", web_config.display()));
    }
    let (raw, _) = site_sql_server_raw(web_config)?;
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else { return Ok(false) };
    let Some(mut pairs) = pairs(&raw) else { return Err("The SiteSqlServer connection string can't be read.".into()) };
    let Some(at) = pairs.iter().rposition(|(k, _)| canonical(k) == "data source") else { return Ok(false) };
    let reached = dnnm_sql::connection_strings::reachable(&pairs[at].1);
    if reached == pairs[at].1 {
        return Ok(false);
    }
    pairs[at].1 = reached;
    let text = pairs.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect::<Vec<_>>().join(";");
    crate::change_database_connection::write_site_sql_server(web_config, &text)?;
    Ok(true)
}

/// Removes system.webServer/rewrite (`RemoveRewriteRules`); nothing to do when there is none.
pub fn remove_rewrite_rules(web_config: &Path) -> Result<(), String> {
    if !web_config.is_file() {
        return Err(format!("web.config not found: {}", web_config.display()));
    }
    let text = read(web_config)?;
    let (body, bom) = xml(&text);
    let doc = load_site_xml(body)?;
    let edits: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("system.webServer"))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rewrite")).collect::<Vec<_>>())
        .map(|n| (n.range(), String::new()))
        .collect();
    if edits.is_empty() {
        return Ok(());
    }
    save(web_config, &apply(&text, bom, edits))
}

/// Every enabled rule that redirects to https:// switched off, with DNN Manager's comment before it
/// (`DisableHttpsRedirectRules`) - and the rules an earlier run switched off. None for no web.config.
pub fn disable_https_redirect_rules(web_config: &Path) -> Result<HttpsRedirectRules, String> {
    if !web_config.is_file() {
        return Ok(HttpsRedirectRules::default());
    }
    let text = read(web_config)?;
    let (body, bom) = xml(&text);
    let doc = load_site_xml(body)?;
    let rules = doc
        .descendants()
        .filter(|n| n.has_tag_name("system.webServer"))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rewrite")))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rules")))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rule")))
        .filter(|rule| is_https_redirect(*rule));

    let mut found = HttpsRedirectRules::default();
    let mut edits = Vec::new();
    for rule in rules {
        let name = rule.attribute("name").unwrap_or("(unnamed)").to_string();
        let enabled = rule.attributes().find(|a| a.name() == "enabled" && a.namespace().is_none());
        if enabled.as_ref().is_some_and(|a| a.value().eq_ignore_ascii_case("false")) {
            // Only ours: a rule that is off in the site's own config isn't DNN Manager's to report.
            if has_disabled_comment(rule) {
                found.already_off.push(name);
            }
            continue;
        }
        match enabled {
            Some(a) => edits.push((a.range_value(), "false".to_string())),
            // After its last attribute, as XLinq adds one.
            None => {
                let at = rule.attributes().map(|a| a.range().end).max().unwrap_or(rule.range().start + "<rule".len());
                edits.push((at..at, " enabled=\"false\"".to_string()));
            }
        }
        // The rule stays on its own line, indented as it was - from the file's own text: the parser's has its line
        // breaks normalised to LF.
        let indent = rule
            .prev_sibling()
            .filter(|n| n.is_text() && n.text().is_some_and(|t| t.trim().is_empty()))
            .map(|n| &body[n.range()])
            .unwrap_or_default();
        let start = rule.range().start;
        edits.push((start..start, format!("<!--{DISABLED_RULE_COMMENT}-->{indent}")));
        found.switched_off.push(name);
    }
    if !found.switched_off.is_empty() {
        save(web_config, &apply(&text, bom, edits))?;
    }
    Ok(found)
}

/// A rule that redirects to an https:// address (typically "HTTP to HTTPS redirect").
fn is_https_redirect(rule: roxmltree::Node) -> bool {
    rule.children().find(|c| c.has_tag_name("action")).is_some_and(|action| {
        action.attribute("type").is_some_and(|t| t.eq_ignore_ascii_case("Redirect"))
            && action.attribute("url").is_some_and(|u| u.trim_start().to_lowercase().starts_with("https://"))
    })
}

/// The nearest node before the rule that isn't white space is DNN Manager's comment.
fn has_disabled_comment(rule: roxmltree::Node) -> bool {
    let mut node = rule.prev_sibling();
    while let Some(n) = node {
        if n.is_text() && n.text().is_some_and(|t| t.trim().is_empty()) {
            node = n.prev_sibling();
            continue;
        }
        return n.is_comment() && n.text().is_some_and(|t| t.contains("Disabled by DNN Manager"));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const REWRITE: &str = "\u{feff}<?xml version=\"1.0\"?>\r\n<configuration>\r\n  <system.webServer>\r\n    <rewrite>\r\n      <rules>\r\n        <rule name=\"Force HTTPS\" stopProcessing=\"true\">\r\n          <action type=\"Redirect\" url=\"https://{HTTP_HOST}/{R:1}\" />\r\n        </rule>\r\n        <rule name=\"Old\" enabled=\"true\">\r\n          <action type=\"redirect\" url=\" HTTPS://x\" />\r\n        </rule>\r\n        <rule name=\"Theirs\" enabled=\"false\">\r\n          <action type=\"Redirect\" url=\"https://y\" />\r\n        </rule>\r\n        <rule name=\"Rewrite\">\r\n          <action type=\"Rewrite\" url=\"https://z\" />\r\n        </rule>\r\n      </rules>\r\n    </rewrite>\r\n  </system.webServer>\r\n</configuration>\r\n";

    fn site(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("web.config");
        std::fs::write(&file, text).unwrap();
        (dir, file)
    }

    #[test]
    fn https_redirects_are_switched_off_once_and_said_again_after() {
        let (_dir, file) = site(REWRITE);
        let found = disable_https_redirect_rules(&file).unwrap();
        assert_eq!(found.switched_off, vec!["Force HTTPS", "Old"]);
        assert!(found.already_off.is_empty());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with('\u{feff}'), "the byte order mark stays");
        assert!(text.contains(&format!(
            "<!--{DISABLED_RULE_COMMENT}-->\r\n        <rule name=\"Force HTTPS\" stopProcessing=\"true\" enabled=\"false\">"
        )));
        assert!(text.contains("<rule name=\"Old\" enabled=\"false\">"));
        assert!(text.contains("<rule name=\"Theirs\" enabled=\"false\">\r\n"), "a rule the site switched off stays");
        let again = disable_https_redirect_rules(&file).unwrap();
        assert!(again.switched_off.is_empty());
        assert_eq!(again.already_off, vec!["Force HTTPS", "Old"]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "nothing to change: not written");
    }

    #[test]
    fn no_web_config_has_no_redirects_and_a_dtd_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            disable_https_redirect_rules(&dir.path().join("web.config")).unwrap(),
            HttpsRedirectRules::default()
        );
        let (_dir, file) = site("<!DOCTYPE x [<!ENTITY e SYSTEM \"file:///c:/x\">]><configuration/>");
        assert!(disable_https_redirect_rules(&file).is_err());
    }

    #[test]
    fn the_rewrite_section_is_removed_and_the_rest_kept() {
        let (_dir, file) = site(REWRITE);
        remove_rewrite_rules(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text,
            "\u{feff}<?xml version=\"1.0\"?>\r\n<configuration>\r\n  <system.webServer>\r\n    \r\n  </system.webServer>\r\n</configuration>\r\n"
        );
        remove_rewrite_rules(&file).unwrap();
        assert!(
            remove_rewrite_rules(&file.with_file_name("none.config")).unwrap_err().starts_with("web.config not found")
        );
    }

    fn with_connection(connection: &str) -> String {
        format!(
            "<configuration>\r\n  <connectionStrings>\r\n    <add name=\"SiteSqlServer\" connectionString=\"{connection}\" providerName=\"System.Data.SqlClient\" />\r\n  </connectionStrings>\r\n  <appSettings>\r\n    <add key=\"SiteSqlServer\" value=\"{connection}\" />\r\n  </appSettings>\r\n</configuration>"
        )
    }

    #[test]
    fn site_sql_server_is_read_with_the_clones_messages() {
        let (_dir, file) = site(&with_connection("Server=sql.example.com;Database=shop;uid=dnn;pwd='p;w'"));
        assert_eq!(read_site_sql_server(&file).unwrap(), SiteSql::new("sql.example.com", "shop", "dnn", "p;w"));
        let (_dir, file) = site(&with_connection(
            r"Data Source=.\SQLExpress;Integrated Security=True;User Instance=True;AttachDBFilename=|DataDirectory|Database.mdf;",
        ));
        let e = read_site_sql_server(&file).unwrap_err();
        assert!(
            e.starts_with(
                "This site uses LocalDB (AttachDBFilename='|DataDirectory|Database.mdf') and has no Initial Catalog."
            ),
            "{e}"
        );
        let (_dir, file) = site("<configuration><connectionStrings/></configuration>");
        assert!(
            read_site_sql_server(&file)
                .unwrap_err()
                .starts_with("connectionStrings/add[@name='SiteSqlServer'] not found in ")
        );
    }

    #[test]
    fn localhost_with_a_port_becomes_the_loopback_address_in_both_places() {
        let (_dir, file) =
            site(&with_connection("Data Source=localhost,1433;Initial Catalog=shop;User ID=dnn_shop;Password=a b"));
        assert!(use_loopback_address(&file).unwrap());
        let text = std::fs::read_to_string(&file).unwrap();
        let expected = "Data Source=127.0.0.1,1433;Initial Catalog=shop;User ID=dnn_shop;Password=&quot;a b&quot;";
        assert_eq!(text.matches(expected).count(), 2, "{text}");
        assert!(!use_loopback_address(&file).unwrap());
    }

    #[test]
    fn a_clone_writes_its_own_login_for_the_site() {
        let (_dir, file) =
            site(&with_connection("Data Source=sql.example.com;Initial Catalog=shop;Integrated Security=True"));
        WebConfigFile
            .write_site_sql_server(&file, &SiteSql::new("localhost,1433", "shop_copy", "dnn_shop_copy", "Pw-1"))
            .unwrap();
        assert_eq!(
            read_site_sql_server(&file).unwrap(),
            SiteSql::new("127.0.0.1,1433", "shop_copy", "dnn_shop_copy", "Pw-1")
        );
    }
}
