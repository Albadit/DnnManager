//! New project (`src/DnnManager.Presentation/Pages/SetupPage.xaml(.cs)`): a new project folder from a DNN download
//! or an imported site .zip. A name whose folder already exists is refused - its files are never set up over. A DNN
//! download gets its IIS website (host name, port), its database (named like the project, filled in from Settings →
//! Database server and changeable here for this project only - asked about as it is typed) and, with automatic
//! setup, DNN installed with the account and website given here. **Create project** waits until everything is right, and
//! says next to it what is still needed.
//!
//! The page doesn't run anything itself: it emits [`SetupPageEvent::CreateProject`] or
//! [`SetupPageEvent::ImportProject`] for the app to run as an operation, and the app calls [`SetupPage::created`] when
//! it succeeded. What it asks of the system goes through [`NewProjectLookups`], off the UI thread; what the window knows
//! (the settings, IIS's sites) it is given with [`SetupPage::set_settings`] and [`SetupPage::set_sites`]. Its draft is
//! kept in the WPF app's `forms` state rows (passwords never), restored once when the page is made.

use dnnm_ui::px;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, MouseButton, MouseDownEvent, PathPromptOptions, SharedString, Subscription, Task, Window, div, prelude::*};

use dnnm_application::iis::IisSiteRuntime;
use dnnm_application::operation::CancelToken;
use dnnm_application::records::InstallMode;
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::setup_project::{
    BackupProject, DatabaseConnection, DatabaseKind, DnnAccount, NewProjectLookups, PageSecrets, SetupProjectRequest,
    site_using,
};
use dnnm_domain::{DnnRelease, DnnReleaseList, validate_project_name};
use dnnm_ui::dialog::{self, option, radio, select};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, ButtonKind, Pad, Tip as _};

use crate::host_password::{PasswordField, PasswordFieldChanged};

/// The cultures DNN's installer offers, and the site templates its package ships (`DnnAccountRules`).
pub const LANGUAGES: [&str; 6] = dnnm_dnn::install::LANGUAGES;
pub const TEMPLATES: [&str; 2] = dnnm_dnn::install::TEMPLATES;

/// A database that couldn't be asked is asked again this often (`AskAgainAfter`).
pub const ASK_AGAIN_AFTER: Duration = Duration::from_secs(10);
/// How long the database fields stay still before the server is asked (`_dbAsk.Interval`).
pub const ASK_AFTER: Duration = Duration::from_millis(600);
/// How long the form stays still before its draft is saved.
const SAVE_DRAFT_AFTER: Duration = Duration::from_secs(1);

/// What the page asks the app to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupPageEvent {
    /// Create project: run `dnnm_wiring::setup_project::setup_project` with it, titled `Create '<name>'`.
    CreateProject(Box<SetupProjectRequest>),
    /// Import project: run `dnnm_wiring::import_project::import_project` with it, titled `Import '<name>'`.
    ImportProject(ImportRequest),
}

/// An existing site to import (`ImportProjectRequest`), as the page has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub project_name: String,
    pub zip_path: PathBuf,
    pub backup_file_path: PathBuf,
    pub host_name: String,
    pub port: Option<u16>,
}

// ─── The rules, as pure functions ────────────────────────────────────────────

/// "Deutsch (Deutschland) (de-DE)" - the language's own name and its code (`Languages.Name`).
pub fn language_name(culture: &str) -> String {
    let native = match culture {
        "en-US" => "English (United States)",
        "de-DE" => "Deutsch (Deutschland)",
        "es-ES" => "Español (España)",
        "fr-FR" => "Français (France)",
        "it-IT" => "Italiano (Italia)",
        "nl-NL" => "Nederlands (Nederland)",
        _ => return culture.to_string(),
    };
    format!("{native} ({culture})")
}

/// `https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases` → `dnnsoftware/Dnn.Platform` (`RepositoryLabel`).
pub fn repository_label(api: &str) -> String {
    let Some(rest) = api.strip_prefix("https://").or_else(|| api.strip_prefix("http://")) else {
        return api.to_string();
    };
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or_default();
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 && parts[0] == "repos" { format!("{}/{}", parts[1], parts[2]) } else { api.to_string() }
}

/// A version to install; `release` None for "the latest", used when the list couldn't be loaded (`VersionOption`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionOption {
    pub release: Option<DnnRelease>,
    pub label: String,
    pub ready: bool,
}

/// The version list and its hint for what GitHub answered, and which is selected (`LoadVersions`).
pub fn versions_for(
    source_label: &str,
    answer: &Result<DnnReleaseList, String>,
    is_kept: &dyn Fn(&DnnRelease) -> bool,
    keeps_packages: bool,
) -> (Vec<VersionOption>, String, Option<usize>) {
    let list = match answer {
        Err(error) => {
            // Still usable: the latest release is looked up when the project is set up.
            let mut hint = format!(
                "Could not load the versions ({error}) - the latest release is used. GitHub is asked again when you come back to this page."
            );
            if !keeps_packages {
                hint.push_str(" To set up projects without internet, turn on Settings → DNN releases → Keep downloaded DNN install packages while online.");
            }
            let latest = VersionOption { release: None, label: "Latest release".into(), ready: true };
            return (vec![latest], hint, Some(0));
        }
        Ok(list) => list,
    };
    if list.releases.is_empty() {
        return (Vec::new(), format!("{source_label} has no release with a DNN install package."), None);
    }
    // A pre-release can be newer than the latest release - it's listed, but the latest release stays the default.
    let latest = list.releases.iter().position(|r| !r.prerelease).unwrap_or(0);
    let options = list
        .releases
        .iter()
        .enumerate()
        .map(|(i, r)| VersionOption {
            release: Some(r.clone()),
            label: format!(
                "{}{}{}{}",
                r.version,
                if i == latest { "  (latest)" } else { "" },
                if r.prerelease { "  (pre-release)" } else { "" },
                if is_kept(r) { "  - kept, no download" } else { "" }
            ),
            ready: true,
        })
        .collect();
    let hint = match &list.saved_at {
        Some(saved_at) => format!(
            "GitHub can't be reached - the releases of {source_label} as of {}. Without internet only a version marked \"kept, no download\" can be set up. GitHub is asked again when you come back to this page.",
            crate::upgrade_dnn::saved_at_shown(saved_at)
        ),
        None => format!(
            "{} releases of {source_label}, highest version first - the latest release is selected.",
            list.releases.len()
        ),
    };
    (options, hint, Some(latest))
}

/// The port typed, when it is one (`ChosenPort`).
pub fn chosen_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

/// What's wrong with the host name and port, or None (`IisProblem`). `sites`: the IIS sites the window shows - None
/// until it has read them.
pub fn iis_problem(host: &str, port: &str, sites: Option<&[(String, IisSiteRuntime)]>, name: &str) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return Some("Enter the host name the site answers on.".into());
    }
    if host.chars().any(|c| !(c.is_alphanumeric() || c == '.' || c == '-')) {
        return Some("The host name can only have letters, digits, '.' and '-'.".into());
    }
    let Some(port) = chosen_port(port) else {
        return Some("The port must be a number between 1 and 65535.".into());
    };
    // From the sites the window shows - IIS isn't asked at every key. The setup asks IIS itself again.
    let sites = sites?;
    let other = site_using(sites.iter().map(|(n, s)| (n.as_str(), s)), host, port, Some(name))?;
    Some(format!(
        "http://{} is already the address of the IIS site '{other}' - choose another host name or port.",
        alias_for(host, port)
    ))
}

/// The host name, with the port when it isn't 80 (`DnnSiteAddress.AliasFor`).
pub fn alias_for(host: &str, port: u16) -> String {
    if port == 80 { host.to_string() } else { format!("{host}:{port}") }
}

/// A host name of the user's own - not one under the host name suffix (`IsCustomDomain`).
pub fn is_custom_domain(host: &str, suffix: &str) -> bool {
    let lower = host.to_lowercase();
    let suffix = suffix.to_lowercase();
    !host.is_empty() && !lower.ends_with(&format!(".{suffix}")) && lower != suffix
}

/// The IIS card's hint: the problem, or where the site answers (`UpdateState`). `name` and `host` as typed.
pub fn iis_hint(name: &str, host: &str, port: &str, problem: Option<&str>, suffix: &str) -> (String, bool) {
    // An empty host name isn't an error yet - it follows the project's name. A bad port is shown at once.
    let port_value = chosen_port(port);
    let shown = if !host.trim().is_empty() {
        problem.map(str::to_string)
    } else if port_value.is_none() {
        Some("The port must be a number between 1 and 65535.".to_string())
    } else {
        None
    };
    if let Some(problem) = shown {
        return (problem, true);
    }
    let host = host.trim();
    let project = if name.is_empty() { "<project>" } else { name };
    let address = alias_for(if host.is_empty() { "<host>" } else { host }, port_value.unwrap_or(80));
    let mut hint = format!(
        "The IIS website and its app pool are named '{project}'; the site answers at http://{address} - DNN Manager adds the host name to this PC's hosts file, so it opens without internet too."
    );
    if is_custom_domain(host, suffix) {
        hint.push_str(" A custom domain then opens this site on this PC, not what the internet has at that name.");
    }
    (hint, false)
}

/// The folder of the IIS site named `name` when it serves another folder than `folder` - from the sites the window
/// shows; None when there is none, or the window hasn't read them (`SiteServingAnotherFolder`).
pub fn site_serving_another_folder(
    sites: Option<&[(String, IisSiteRuntime)]>,
    name: &str,
    folder: &Path,
) -> Option<String> {
    let (_, site) = sites?.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?;
    let path = site.physical_path.to_string_lossy();
    let trimmed = |p: &str| p.trim_end_matches('\\').to_lowercase();
    if trimmed(&path) == trimmed(&folder.to_string_lossy()) {
        None
    } else if path.is_empty() {
        Some("another folder".into())
    } else {
        Some(path.into_owned())
    }
}

/// The name's error line: why it can't be used, or None (`UpdateState`).
pub fn name_error(name: &str, exists: bool, site_elsewhere: Option<&str>) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if let Err(e) = validate_project_name(name) {
        return Some(e);
    }
    if exists {
        return Some(format!("A project named '{name}' already exists. Choose another name."));
    }
    site_elsewhere.map(|other| format!("IIS already has a site named '{name}', serving {other} - choose another name."))
}

/// What's wrong with the import fields, or None (`ImportProblem`).
pub fn import_problem(zip: &str, backup: &str, is_file: &dyn Fn(&Path) -> bool) -> Option<String> {
    let zip = zip.trim();
    if zip.is_empty() {
        return Some("Choose the site .zip.".into());
    }
    if !is_file(Path::new(zip)) {
        return Some(format!("File not found: {zip}"));
    }
    let backup = backup.trim();
    if backup.is_empty() {
        return Some("Choose the database .bacpac.".into());
    }
    if !is_file(Path::new(backup)) {
        return Some(format!("File not found: {backup}"));
    }
    (!is_backup_file(Path::new(backup))).then(|| "The database backup must be a .bacpac or .bak file.".into())
}

/// A `.bacpac`, or a `.bak` that isn't a copied file such as `web.config.bak` (`LocalSqlContainer.IsBackupFile`).
pub fn is_backup_file(path: &Path) -> bool {
    let ext = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_lowercase());
    match ext(path).as_deref() {
        Some("bacpac") => true,
        Some("bak") => {
            const COPIED: [&str; 12] =
                ["config", "json", "xml", "txt", "resources", "resx", "js", "css", "aspx", "ascx", "cs", "dll"];
            let inner = path.file_stem().map(Path::new).and_then(ext);
            !inner.is_some_and(|e| COPIED.contains(&e.as_str()))
        }
        _ => false,
    }
}

/// The Database card's connection type (`ContainerType`, `SqlServerType`, `LocalDbType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbType {
    Container,
    SqlServer,
    LocalDb,
}

impl DbType {
    /// As the settings' `sqlServer.type` names it.
    pub fn from_setting(text: &str) -> Self {
        if text.eq_ignore_ascii_case("container") {
            DbType::Container
        } else if text.eq_ignore_ascii_case("localDbFile") {
            DbType::LocalDb
        } else {
            DbType::SqlServer
        }
    }
}

/// The Database card as typed.
#[derive(Clone, PartialEq, Eq)]
pub struct DbFields {
    pub db_type: DbType,
    pub server: String,
    pub name: String,
    /// SQL Server authentication chosen (SQL Server type only).
    pub sql_auth: bool,
    pub user: String,
    pub password: String,
}

impl DbFields {
    /// The container, or SQL Server with SQL Server authentication: a user name and password (`DbSqlLogin`).
    pub fn sql_login(&self) -> bool {
        self.db_type == DbType::Container || (self.db_type == DbType::SqlServer && self.sql_auth)
    }

    /// What's wrong with the database fields, or None when they can be tested (`DatabaseProblem`).
    pub fn problem(&self) -> Option<String> {
        if self.server.trim().is_empty() {
            return Some(
                if self.db_type == DbType::LocalDb { "Enter the LocalDB instance." } else { "Enter the server." }
                    .into(),
            );
        }
        if self.db_type != DbType::LocalDb {
            let name = self.name.trim();
            if name.is_empty() {
                return Some("Enter the database name.".into());
            }
            if name.chars().count() > 128 || name.contains(']') {
                return Some("The database name can have at most 128 characters, and no ']'.".into());
            }
        }
        if self.sql_login() && self.user.trim().is_empty() {
            return Some("Enter the username.".into());
        }
        if self.sql_login() && self.password.is_empty() {
            return Some("Enter the password.".into());
        }
        None
    }

    /// The new project's database, as the card has it (`ChosenDatabase`).
    pub fn connection(&self) -> DatabaseConnection {
        let (server, name, user) = (self.server.trim(), self.name.trim(), self.user.trim());
        match self.db_type {
            DbType::Container => {
                DatabaseConnection::sql_login(DatabaseKind::Container, server, name, user, &self.password)
            }
            DbType::LocalDb => {
                DatabaseConnection::windows(DatabaseKind::LocalDbFile, server, DatabaseConnection::LOCAL_DB_FILE_NAME)
            }
            DbType::SqlServer if self.sql_auth => {
                DatabaseConnection::sql_login(DatabaseKind::SqlServer, server, name, user, &self.password)
            }
            DbType::SqlServer => DatabaseConnection::windows(DatabaseKind::SqlServer, server, name),
        }
    }
}

/// What the server said about the database (`DbAnswer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbAnswer {
    None,
    Asking,
    Free,
    Exists,
    /// It couldn't be asked - SqlClient's words.
    NoAnswer(String),
}

/// Whether the database lets the project be created: a LocalDB file is the site's own; another is free - or, for a
/// manual install (which may go into a database that is there), exists (`DatabaseFree`).
pub fn database_free(db_type: DbType, answer: &DbAnswer, automatic: bool) -> bool {
    db_type == DbType::LocalDb || *answer == DbAnswer::Free || (*answer == DbAnswer::Exists && !automatic)
}

/// The Database card's error line, its tooltip and its hint (`ShowDatabase`). `started`: a name is typed.
pub fn database_texts(
    fields: &DbFields,
    answer: &DbAnswer,
    automatic: bool,
    started: bool,
    edited: bool,
) -> (String, Option<String>, String) {
    let problem = fields.problem();
    let server = fields.server.trim();
    let database = format!("[{}] on {server}", fields.name.trim());
    // An empty name isn't an error yet - the database name follows it.
    let error = match (&problem, answer) {
        (Some(p), _) => {
            if started || edited {
                p.clone()
            } else {
                String::new()
            }
        }
        (None, DbAnswer::Exists) if automatic => {
            format!("Database {database} already exists - choose another name, or remove that database first.")
        }
        (None, DbAnswer::NoAnswer(detail)) if detail.to_lowercase().contains("login failed") => {
            format!("SQL Server at {server} refuses the login.")
        }
        (None, DbAnswer::NoAnswer(_)) => format!("Can't reach SQL Server at {server}."),
        _ => String::new(),
    };
    // One line here; SqlClient's own words on hover (and asked again every few seconds).
    let tooltip = match (answer, &problem) {
        (DbAnswer::NoAnswer(detail), None) => {
            Some(format!("{detail}\n\nAsked again every {} seconds.", ASK_AGAIN_AFTER.as_secs()))
        }
        _ => None,
    };
    let said = database_said(fields, answer, automatic);
    let file = if fields.db_type == DbType::LocalDb {
        format!("The site's own App_Data\\{}. ", DatabaseConnection::LOCAL_DB_FILE_NAME)
    } else {
        String::new()
    };
    let hint = format!("{said}{file}{}", if edited { "For this project only." } else { "" }).trim().to_string();
    (error, tooltip, hint)
}

/// What the server answered about the database, for the hint - empty when there is nothing to say (the fields aren't
/// right yet, or the answer is an error line). The page shows the hint as text while it says this, otherwise as a tip.
pub fn database_said(fields: &DbFields, answer: &DbAnswer, automatic: bool) -> String {
    if fields.problem().is_some() {
        return String::new();
    }
    let database = format!("[{}] on {}", fields.name.trim(), fields.server.trim());
    match answer {
        DbAnswer::Asking => format!("Checking whether database {database} is free… "),
        DbAnswer::Free => format!("Database {database} is free. "),
        DbAnswer::Exists if !automatic => {
            format!("Database {database} already exists - the setup asks whether to drop it or install into it. ")
        }
        _ => String::new(),
    }
}

/// The account card's hint (`AccountHint`).
pub fn account_hint(language: &str) -> String {
    let mut hint = "The host (superuser) account signs in without being asked to change its password. Defaults in Settings → Projects → DNN defaults.".to_string();
    if language != "en-US" {
        hint.push_str(&format!(
            " {}: DNN downloads its language pack while installing (needs internet).",
            language_name(language)
        ));
    }
    hint
}

/// The name's hint (`NameHint`).
pub fn name_hint(importing: bool, container_server: &str) -> String {
    let tail = if importing {
        format!(", on the local SQL container ({container_server}).")
    } else {
        " (see Database below).".to_string()
    };
    format!(
        "Letters, digits, '-', '_' or '.' - the name becomes the folder, the IIS site, the hostname and the database{tail}"
    )
}

/// What the page is missing, in the order of the cards: each true when that part is right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Readiness {
    pub name: bool,
    pub iis: bool,
    pub importing: bool,
    pub import: bool,
    pub version: bool,
    pub database: bool,
    pub account: bool,
}

impl Readiness {
    /// Create project (or Import project) can be clicked.
    pub fn can_run(&self) -> bool {
        self.name
            && self.iis
            && if self.importing { self.import } else { self.version && self.database && self.account }
    }

    /// "Still needed: …" next to the button; empty when it can be clicked.
    pub fn still_needed(&self) -> String {
        if self.can_run() {
            return String::new();
        }
        let mut needed = Vec::new();
        if !self.name {
            needed.push("a project name");
        }
        if !self.iis {
            needed.push("a host name and port");
        }
        if self.importing && !self.import {
            needed.push("the site's .zip and its database backup");
        }
        if !self.importing && !self.version {
            needed.push("a DNN version");
        }
        if !self.importing && !self.database {
            needed.push("a database");
        }
        if !self.importing && !self.account {
            needed.push("the host account");
        }
        if needed.is_empty() { String::new() } else { format!("Still needed: {}.", needed.join(", ")) }
    }
}

/// The DNN defaults of the settings (`DnnDefaultsSettings`, `AppOptions.DefaultHostEmail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnDefaults {
    pub automatic: bool,
    pub host_username: String,
    pub host_email: String,
    pub website_name: String,
    pub language: String,
    pub template: String,
}

impl DnnDefaults {
    pub fn of(settings: &Settings) -> Self {
        let get = |key: &str, default: &str| -> String {
            settings.get(&format!("projects.dnnDefaults.{key}")).unwrap_or(default).to_string()
        };
        let suffix = suffix_of(settings);
        let email = get("hostEmail", "admin@admin.com");
        Self {
            automatic: get("installMode", "automatic").eq_ignore_ascii_case("automatic"),
            host_username: get("hostUsername", "host"),
            host_email: if email.is_empty() { format!("host@{suffix}") } else { email },
            website_name: get("websiteName", "My Website"),
            language: get("language", "en-US"),
            template: get("template", TEMPLATES[0]),
        }
    }
}

fn suffix_of(settings: &Settings) -> String {
    settings.get("projects.hostnameSuffix").unwrap_or("dnndev.me").trim().to_string()
}

fn site_port_of(settings: &Settings) -> u16 {
    settings.get("projects.sitePort").and_then(|p| p.trim().parse().ok()).unwrap_or(80)
}

/// The container's SQL Server address, `host,port` (`AppOptions.ServerFor(Docker.DefaultPort)`).
pub fn container_server(settings: &Settings) -> String {
    let host = settings.get("sqlServer.host").map(str::trim).filter(|h| !h.is_empty()).unwrap_or("localhost");
    let port = settings.get("sqlServer.port").and_then(|p| p.trim().parse::<u16>().ok()).unwrap_or(1433);
    format!("{host},{port}")
}

/// The Database card for `db_type` with the settings' server and login for it (`FillDatabase`). The name is left as
/// it is.
pub fn database_defaults(settings: &Settings, secrets: &PageSecrets, db_type: DbType, name: &str) -> DbFields {
    let saved_type = DbType::from_setting(settings.get("sqlServer.type").unwrap_or("container"));
    let server = settings.get("sqlServer.server").unwrap_or_default().trim().to_string();
    let user_name = settings.get("sqlServer.userName").unwrap_or_default().trim().to_string();
    let sql_auth_saved = settings.get("sqlServer.authentication").is_some_and(|a| a.eq_ignore_ascii_case("sql"));
    let mut fields = DbFields {
        db_type,
        server: String::new(),
        name: name.to_string(),
        sql_auth: false,
        user: String::new(),
        password: String::new(),
    };
    match db_type {
        DbType::Container => {
            fields.server = container_server(settings);
            fields.user = if user_name.is_empty() { "sa".into() } else { user_name };
            fields.password = secrets.sa_password.clone();
        }
        DbType::LocalDb => {
            fields.server = if saved_type == DbType::LocalDb && !server.is_empty() {
                server
            } else {
                DatabaseConnection::LOCAL_DB_SERVER.into()
            };
        }
        DbType::SqlServer => {
            let saved = saved_type == DbType::SqlServer;
            fields.server = if saved && !server.is_empty() { server } else { r".\SQLEXPRESS".into() };
            fields.sql_auth = saved && sql_auth_saved;
            fields.user = if saved { user_name } else { String::new() };
            fields.password = if fields.sql_auth {
                secrets.database_server_password.clone().unwrap_or_default()
            } else {
                String::new()
            };
        }
    }
    fields
}

// ─── The view ────────────────────────────────────────────────────────────────

/// The page's text fields, by their WPF names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Field {
    Name,
    HostName,
    Port,
    Zip,
    Backup,
    HostUser,
    HostEmail,
    WebsiteName,
    DbServer,
    DbName,
    DbUser,
}

impl Field {
    const ALL: [Field; 11] = [
        Field::Name,
        Field::HostName,
        Field::Port,
        Field::Zip,
        Field::Backup,
        Field::HostUser,
        Field::HostEmail,
        Field::WebsiteName,
        Field::DbServer,
        Field::DbName,
        Field::DbUser,
    ];

    fn key(self) -> &'static str {
        match self {
            Field::Name => "NameBox",
            Field::HostName => "HostNameBox",
            Field::Port => "PortBox",
            Field::Zip => "ZipBox",
            Field::Backup => "BackupBox",
            Field::HostUser => "HostUserBox",
            Field::HostEmail => "HostEmailBox",
            Field::WebsiteName => "WebsiteNameBox",
            Field::DbServer => "DbServerBox",
            Field::DbName => "DbNameBox",
            Field::DbUser => "DbUserBox",
        }
    }
}

/// The page's lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    BackupProject,
    ProjectBackup,
    Source,
    Version,
    Language,
    Template,
}

pub struct SetupPage {
    lookups: Arc<dyn NewProjectLookups>,
    settings: Settings,
    secrets: PageSecrets,
    sites: Option<Vec<(String, IisSiteRuntime)>>,
    focus: FocusHandle,
    inputs: BTreeMap<Field, Entity<TextInput>>,
    /// What the page itself last put in each field - a change to anything else is the user's.
    filled: BTreeMap<Field, String>,
    host_password: Entity<PasswordField>,
    db_password: Entity<PasswordField>,
    filled_db_password: String,
    _subscriptions: Vec<Subscription>,

    importing: bool,
    automatic: bool,
    // Fields that follow the project's name until they are edited.
    host_edited: bool,
    website_edited: bool,
    db_type: DbType,
    db_sql_auth: bool,
    db_edited: bool,
    db_name_edited: bool,

    open: Option<List>,
    backup_projects: Vec<BackupProject>,
    backup_project: Option<usize>,
    project_backup: Option<usize>,
    sources: Vec<String>,
    source: Option<usize>,
    versions: Vec<VersionOption>,
    version: Option<usize>,
    version_hint: String,
    /// The hint only explains the list (an info tip beside Version) - false while it says what GitHub answered.
    version_hint_explains: bool,
    /// Which source's versions the list shows now - an answer for another source that arrives late is dropped.
    versions_for: Option<String>,
    /// GitHub's answers by source, asked again for one that failed (`DnnReleaseCatalog`).
    catalog: BTreeMap<String, Result<DnnReleaseList, String>>,
    language: usize,
    template: usize,
    /// A restored draft's version, selected once the versions are there - unless one is picked first.
    pending_version: Option<String>,

    db_asked: Option<DatabaseConnection>,
    db_answer: DbAnswer,
    db_ask: Option<(CancelToken, Task<()>)>,
    shown: bool,
    draft_save: Option<Task<()>>,
    _draft_load: Option<Task<()>>,
    /// The saved draft has been read (and put back): until then nothing is saved over it.
    draft_loaded: bool,
}

impl EventEmitter<SetupPageEvent> for SetupPage {}

impl SetupPage {
    /// The page, filled in from `settings` (and `lookups`' passwords), its draft put back once it is read.
    pub fn new(
        lookups: Arc<dyn NewProjectLookups>,
        settings: Settings,
        sites: Option<Vec<(String, IisSiteRuntime)>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let secrets = lookups.secrets(&settings);
        let mut inputs = BTreeMap::new();
        let mut subscriptions = Vec::new();
        for field in Field::ALL {
            let input = cx.new(|cx| TextInput::new("", cx));
            subscriptions.push(
                cx.subscribe(&input, move |page: &mut Self, _, e: &TextChanged, cx| page.text_changed(field, &e.0, cx)),
            );
            inputs.insert(field, input);
        }
        let host_password = cx.new(PasswordField::new);
        let db_password = cx.new(PasswordField::new);
        subscriptions.push(
            cx.subscribe(&host_password, |page: &mut Self, _, _: &PasswordFieldChanged, cx| page.update_state(cx)),
        );
        subscriptions.push(cx.subscribe(&db_password, |page: &mut Self, input, _: &PasswordFieldChanged, cx| {
            if input.read(cx).password(cx) != page.filled_db_password {
                page.db_edited = true;
            }
            page.update_state(cx)
        }));
        let sources = lookups.release_sources();
        let mut page = Self {
            lookups,
            settings,
            secrets,
            sites,
            focus: cx.focus_handle(),
            inputs,
            filled: BTreeMap::new(),
            host_password,
            db_password,
            filled_db_password: String::new(),
            _subscriptions: subscriptions,
            importing: false,
            automatic: true,
            host_edited: false,
            website_edited: false,
            db_type: DbType::Container,
            db_sql_auth: false,
            db_edited: false,
            db_name_edited: false,
            open: None,
            backup_projects: Vec::new(),
            backup_project: None,
            project_backup: None,
            source: (!sources.is_empty()).then_some(0),
            sources,
            versions: Vec::new(),
            version: None,
            version_hint: "The repository's DNN releases, highest version first - the latest is selected.".into(),
            version_hint_explains: true,
            versions_for: None,
            catalog: BTreeMap::new(),
            language: 0,
            template: 0,
            pending_version: None,
            db_asked: None,
            db_answer: DbAnswer::None,
            db_ask: None,
            shown: true,
            draft_save: None,
            _draft_load: None,
            draft_loaded: false,
        };
        page.load_defaults(cx);
        page.load_backup_projects(cx);
        page.load_versions(cx);
        page.load_draft(cx);
        page
    }

    // ─── What the app tells the page ──────────────────────────────────────

    /// The settings were saved: the database follows them - unless it was changed here (`OnOptionsChanged`).
    pub fn set_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        self.secrets = self.lookups.secrets(&settings);
        self.settings = settings;
        if !self.db_edited {
            self.load_database_defaults(cx);
        }
        self.update_state(cx);
    }

    /// IIS's sites as the window has read them - a site added meanwhile may have the host name typed here.
    pub fn set_sites(&mut self, sites: Option<Vec<(String, IisSiteRuntime)>>, cx: &mut Context<Self>) {
        self.sites = sites;
        self.update_state(cx);
    }

    /// The page is shown again (`Refresh`, `Loaded`): the backups and the database asked anew, the versions from the
    /// list kept - GitHub again if it failed.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        self.shown = true;
        self.load_backup_projects(cx);
        self.forget_database_answer();
        self.catalog.retain(|_, answer| answer.is_ok());
        self.load_versions(cx);
        self.update_state(cx);
    }

    /// The page is left (`Unloaded`): the database isn't asked about meanwhile.
    pub fn hidden(&mut self, cx: &mut Context<Self>) {
        self.shown = false;
        self.forget_database_answer();
        cx.notify();
    }

    /// The project was created or imported: the name cleared, so the page is ready for the next one (`Created`).
    pub fn created(&mut self, cx: &mut Context<Self>) {
        self.set_field(Field::Name, "", cx);
        self.set_field(Field::Zip, "", cx);
        self.set_field(Field::Backup, "", cx);
        self.host_edited = false;
        self.website_edited = !DnnDefaults::of(&self.settings).website_name.is_empty();
        // The next project's database is named like it again - on the server chosen here, which stays.
        self.db_name_edited = false;
        self.follow_name(cx);
        self.update_state(cx);
    }

    // ─── Filling in ───────────────────────────────────────────────────────

    fn text(&self, field: Field, cx: &App) -> String {
        self.inputs[&field].read(cx).text().to_string()
    }

    fn name(&self, cx: &App) -> String {
        self.text(Field::Name, cx).trim().to_string()
    }

    /// Puts `text` in the field as the page's own - not taken for an edit.
    fn set_field(&mut self, field: Field, text: &str, cx: &mut Context<Self>) {
        self.filled.insert(field, text.to_string());
        if self.inputs[&field].read(cx).text().as_ref() != text {
            let text = text.to_string();
            self.inputs[&field].update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    fn set_db_password(&mut self, password: &str, cx: &mut Context<Self>) {
        self.filled_db_password = password.to_string();
        self.db_password.update(cx, |input, cx| input.set_password(password, cx));
    }

    fn text_changed(&mut self, field: Field, text: &str, cx: &mut Context<Self>) {
        let own = self.filled.get(&field).is_some_and(|f| f == text);
        match field {
            Field::Name => self.follow_name(cx),
            Field::HostName if !own => self.host_edited = !text.trim().is_empty(),
            Field::WebsiteName if !own => self.website_edited = !text.trim().is_empty(),
            Field::DbName if !own => {
                self.db_name_edited = !text.trim().is_empty();
                self.db_edited = true;
            }
            Field::DbServer | Field::DbUser if !own => self.db_edited = true,
            _ => {}
        }
        self.update_state(cx);
    }

    /// The settings' DNN defaults and database server, and the site address for no name yet (`LoadDefaults`).
    fn load_defaults(&mut self, cx: &mut Context<Self>) {
        let dnn = DnnDefaults::of(&self.settings);
        self.automatic = dnn.automatic;
        self.set_field(Field::HostUser, &dnn.host_username, cx);
        let host_password = self.secrets.default_host_password.clone().unwrap_or_default();
        self.host_password.update(cx, |input, cx| input.set_password(&host_password, cx));
        self.set_field(Field::HostEmail, &dnn.host_email, cx);
        self.set_field(Field::WebsiteName, &dnn.website_name, cx);
        self.website_edited = !dnn.website_name.is_empty();
        self.language = LANGUAGES.iter().position(|l| *l == dnn.language).unwrap_or(0);
        self.template = TEMPLATES.iter().position(|t| *t == dnn.template).unwrap_or(0);
        self.set_field(Field::Port, &site_port_of(&self.settings).to_string(), cx);
        self.host_edited = false;
        self.load_database_defaults(cx);
        self.follow_name(cx);
    }

    /// The host name, website name and database name are the project's name until they are typed in (`FollowName`).
    fn follow_name(&mut self, cx: &mut Context<Self>) {
        let name = self.name(cx);
        if !self.host_edited {
            let host = if name.is_empty() { String::new() } else { format!("{name}.{}", suffix_of(&self.settings)) };
            self.set_field(Field::HostName, &host, cx);
        }
        if !self.website_edited {
            self.set_field(Field::WebsiteName, &name, cx);
        }
        if !self.db_name_edited {
            self.set_field(Field::DbName, &name, cx);
        }
    }

    /// The database as Settings → Database server has it (`LoadDatabaseDefaults`).
    fn load_database_defaults(&mut self, cx: &mut Context<Self>) {
        let db_type = DbType::from_setting(self.settings.get("sqlServer.type").unwrap_or("container"));
        self.fill_database(db_type, cx);
        self.db_edited = false;
    }

    /// Shows `db_type` with the settings' server and login for it (`FillDatabase`).
    fn fill_database(&mut self, db_type: DbType, cx: &mut Context<Self>) {
        let name = self.text(Field::DbName, cx);
        let fields = database_defaults(&self.settings, &self.secrets, db_type, &name);
        self.db_type = db_type;
        self.set_field(Field::DbServer, &fields.server, cx);
        if db_type != DbType::LocalDb {
            self.db_sql_auth = fields.sql_auth;
            self.set_field(Field::DbUser, &fields.user, cx);
            self.set_db_password(&fields.password, cx);
        }
    }

    fn choose_db_type(&mut self, db_type: DbType, cx: &mut Context<Self>) {
        if db_type == self.db_type {
            return;
        }
        // Another type: its server and login as the settings have them.
        self.fill_database(db_type, cx);
        self.db_edited = true;
        self.update_state(cx);
    }

    fn choose_db_auth(&mut self, sql: bool, cx: &mut Context<Self>) {
        self.db_sql_auth = sql;
        self.db_edited = true;
        self.update_state(cx);
    }

    fn use_the_settings(&mut self, cx: &mut Context<Self>) {
        self.load_database_defaults(cx);
        self.db_name_edited = false;
        self.follow_name(cx);
        self.update_state(cx);
    }

    fn generate_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.lookups.generate_password();
        self.host_password.update(cx, |field, cx| {
            field.set_password(&password, cx);
            field.reveal(window, cx);
        });
    }

    fn db_fields(&self, cx: &App) -> DbFields {
        DbFields {
            db_type: self.db_type,
            server: self.text(Field::DbServer, cx),
            name: self.text(Field::DbName, cx),
            sql_auth: self.db_sql_auth,
            user: self.text(Field::DbUser, cx),
            password: self.db_password.read(cx).password(cx),
        }
    }

    fn account(&self, cx: &App) -> DnnAccount {
        let website = self.text(Field::WebsiteName, cx).trim().to_string();
        DnnAccount {
            user_name: self.text(Field::HostUser, cx).trim().to_string(),
            password: self.host_password.read(cx).password(cx),
            email: self.text(Field::HostEmail, cx).trim().to_string(),
            website_name: if website.is_empty() { self.name(cx) } else { website },
            language: LANGUAGES[self.language].to_string(),
            template: TEMPLATES[self.template].to_string(),
        }
    }

    // ─── The lists ────────────────────────────────────────────────────────

    /// The projects with a complete backup, off the UI thread (`LoadBackupProjects`).
    fn load_backup_projects(&mut self, cx: &mut Context<Self>) {
        let lookups = Arc::clone(&self.lookups);
        cx.spawn(async move |this, cx| {
            let projects = cx.background_executor().spawn(async move { lookups.backup_projects() }).await;
            let _ = this.update(cx, |page, cx| {
                let selected = page.backup_project.and_then(|i| page.backup_projects.get(i)).map(|p| p.project.clone());
                page.backup_project = selected.and_then(|s| projects.iter().position(|p| p.project == s));
                if page.backup_project.is_none() {
                    page.project_backup = None;
                }
                page.backup_projects = projects;
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_backup_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.backup_project = Some(index);
        let count = self.backup_projects[index].backups.len();
        // Newest first.
        self.project_backup = None;
        if count > 0 {
            self.choose_project_backup(0, cx);
        }
        cx.notify();
    }

    fn choose_project_backup(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(project) = self.backup_project.and_then(|i| self.backup_projects.get(i)).cloned() else { return };
        let Some(pick) = project.backups.get(index) else { return };
        self.project_backup = Some(index);
        self.set_field(Field::Zip, &pick.site_zip.to_string_lossy(), cx);
        self.set_field(Field::Backup, &pick.database.to_string_lossy(), cx);
        // A copy of an existing project needs a name of its own.
        let suggested = format!("{}_copy", project.project);
        if self.name(cx).is_empty()
            && validate_project_name(&suggested).is_ok()
            && !self.lookups.project_exists(&suggested)
        {
            self.set_field(Field::Name, &suggested, cx);
        }
        self.update_state(cx);
    }

    /// Fills the version list for the chosen repository - from the list asked before; asked of GitHub again when that
    /// couldn't reach it (`LoadVersions`).
    fn load_versions(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.source.and_then(|i| self.sources.get(i)).cloned() else { return };
        self.versions_for = Some(api.clone());
        if let Some(answer) = self.catalog.get(&api).cloned() {
            self.show_versions(&api, &answer, cx);
            return;
        }
        let label = repository_label(&api);
        self.versions = vec![VersionOption { release: None, label: "Loading versions…".into(), ready: false }];
        self.version = Some(0);
        self.version_hint = format!("Asking GitHub for the releases of {label}…");
        self.version_hint_explains = false;
        self.update_state(cx);
        let lookups = Arc::clone(&self.lookups);
        cx.spawn(async move |this, cx| {
            let asked = api.clone();
            let answer =
                cx.background_executor().spawn(async move { lookups.list_releases(&asked, &CancelToken::new()) }).await;
            let _ = this.update(cx, |page, cx| {
                page.catalog.insert(api.clone(), answer.clone());
                // Another repository was picked meanwhile.
                if page.versions_for.as_deref() == Some(api.as_str()) {
                    page.show_versions(&api, &answer, cx);
                }
            });
        })
        .detach();
    }

    fn show_versions(&mut self, api: &str, answer: &Result<DnnReleaseList, String>, cx: &mut Context<Self>) {
        let lookups = Arc::clone(&self.lookups);
        let (options, hint, selected) =
            versions_for(&repository_label(api), answer, &|r| lookups.is_kept(r), lookups.keeps_packages());
        let chosen = self.version.and_then(|i| self.versions.get(i)).filter(|v| v.ready).map(|v| v.label.clone());
        self.versions = options;
        self.version_hint = hint;
        // A list from GitHub just now: the hint explains it. Otherwise it says what went wrong, or that it's offline.
        self.version_hint_explains = matches!(answer, Ok(list) if list.saved_at.is_none() && !list.releases.is_empty());
        // The draft's version - or the one chosen before the list was refreshed - when it is still there.
        let wanted = self.pending_version.take().or(chosen);
        self.version = wanted.and_then(|w| self.versions.iter().position(|v| v.label == w)).or(selected);
        self.update_state(cx);
    }

    fn version_ready(&self) -> bool {
        self.source.is_some() && self.version.and_then(|i| self.versions.get(i)).is_some_and(|v| v.ready)
    }

    // ─── Is the database free? ────────────────────────────────────────────

    /// Asks about `connection` a moment from now, unless that is what was asked already (`FollowDatabase`).
    fn follow_database(&mut self, connection: DatabaseConnection, cx: &mut Context<Self>) {
        if self.db_asked.as_ref() == Some(&connection) {
            return;
        }
        self.forget_database_answer();
        self.db_asked = Some(connection.clone());
        self.db_answer = DbAnswer::Asking;
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let lookups = Arc::clone(&self.lookups);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ASK_AFTER).await;
            loop {
                if token.is_cancelled() {
                    return;
                }
                let (asked, ask_token, l) = (connection.clone(), token.clone(), Arc::clone(&lookups));
                let exists = cx.background_executor().spawn(async move { l.database_exists(&asked, &ask_token) }).await;
                // Another connection typed meanwhile - this answer is about the old one.
                if token.is_cancelled() {
                    return;
                }
                let answer = match exists {
                    Ok(true) => DbAnswer::Exists,
                    Ok(false) => DbAnswer::Free,
                    Err(detail) => DbAnswer::NoAnswer(detail),
                };
                let again = matches!(answer, DbAnswer::NoAnswer(_));
                if this
                    .update(cx, |page, cx| {
                        page.db_answer = answer;
                        page.update_state(cx);
                    })
                    .is_err()
                {
                    return;
                }
                if !again {
                    return;
                }
                // The container may be starting - asked again while nothing changes (a change cancels this wait).
                cx.background_executor().timer(ASK_AGAIN_AFTER).await;
            }
        });
        self.db_ask = Some((cancel, task));
    }

    fn forget_database_answer(&mut self) {
        if let Some((cancel, _task)) = self.db_ask.take() {
            cancel.cancel();
        }
        self.db_asked = None;
        self.db_answer = DbAnswer::None;
    }

    // ─── State ────────────────────────────────────────────────────────────

    fn readiness(&self, cx: &App) -> Readiness {
        let name = self.name(cx);
        let folder = self.lookups.project_directory(&name);
        let exists = validate_project_name(&name).is_ok() && self.lookups.project_exists(&name);
        let elsewhere = if validate_project_name(&name).is_ok() && !exists {
            site_serving_another_folder(self.sites.as_deref(), &name, &folder)
        } else {
            None
        };
        let fields = self.db_fields(cx);
        let account_ok = !self.automatic || self.lookups.account_problems(&self.account(cx)).is_empty();
        Readiness {
            name: validate_project_name(&name).is_ok() && !exists && elsewhere.is_none(),
            iis: iis_problem(
                &self.text(Field::HostName, cx),
                &self.text(Field::Port, cx),
                self.sites.as_deref(),
                &name,
            )
            .is_none(),
            importing: self.importing,
            import: import_problem(&self.text(Field::Zip, cx), &self.text(Field::Backup, cx), &|p| p.is_file())
                .is_none(),
            version: self.version_ready(),
            database: fields.problem().is_none() && database_free(self.db_type, &self.db_answer, self.automatic),
            account: account_ok,
        }
    }

    /// Asks about the database when its fields can be tested, and saves the draft a moment later (`UpdateState`).
    fn update_state(&mut self, cx: &mut Context<Self>) {
        let fields = self.db_fields(cx);
        if self.shown && !self.importing && fields.problem().is_none() && self.db_type != DbType::LocalDb {
            self.follow_database(fields.connection(), cx);
        } else {
            self.forget_database_answer();
        }
        self.schedule_draft_save(cx);
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let name = self.name(cx);
        if validate_project_name(&name).is_err() || self.lookups.project_exists(&name) {
            return;
        }
        let ready = self.readiness(cx);
        if !ready.can_run() {
            return;
        }
        let host = self.text(Field::HostName, cx).trim().to_string();
        let port = chosen_port(&self.text(Field::Port, cx));
        if self.importing {
            cx.emit(SetupPageEvent::ImportProject(ImportRequest {
                project_name: name,
                zip_path: PathBuf::from(self.text(Field::Zip, cx).trim()),
                backup_file_path: PathBuf::from(self.text(Field::Backup, cx).trim()),
                host_name: host,
                port,
            }));
            return;
        }
        let (Some(api), Some(version)) = (
            self.source.and_then(|i| self.sources.get(i)).cloned(),
            self.version.and_then(|i| self.versions.get(i)).filter(|v| v.ready).cloned(),
        ) else {
            return;
        };
        let account = self.account(cx);
        cx.emit(SetupPageEvent::CreateProject(Box::new(SetupProjectRequest {
            project_name: name,
            release_api_url: api,
            version: version.release.map(|r| r.tag_name),
            host_name: Some(host),
            port,
            install_mode: if self.automatic { InstallMode::Automatic } else { InstallMode::Manual },
            account: self.automatic.then_some(account),
            database: Some(self.db_fields(cx).connection()),
        })));
    }

    fn browse(&mut self, zip: bool, cx: &mut Context<Self>) {
        let title = if zip { "Site .zip" } else { "Database .bacpac" };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(title.into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |page, cx| {
                let text = path.to_string_lossy().into_owned();
                let field = if zip { Field::Zip } else { Field::Backup };
                page.inputs[&field].update(cx, |input, cx| input.set_text(text, cx));
                if zip {
                    let suggested = path.file_stem().map(|s| s.to_string_lossy().replace(' ', "_")).unwrap_or_default();
                    if page.name(cx).is_empty() && validate_project_name(&suggested).is_ok() {
                        page.inputs[&Field::Name].update(cx, |input, cx| input.set_text(suggested, cx));
                    }
                }
            });
        })
        .detach();
    }

    // ─── The draft (`FormDraft`) ──────────────────────────────────────────

    /// What is typed and chosen, by the WPF field names - never a password.
    pub fn capture_draft(&self, cx: &App) -> BTreeMap<String, String> {
        let mut draft = BTreeMap::new();
        let flag = |on: bool| if on { "true" } else { "false" }.to_string();
        for field in Field::ALL {
            draft.insert(field.key().to_string(), self.text(field, cx));
        }
        draft.insert("FromScratch".into(), flag(!self.importing));
        draft.insert("FromZip".into(), flag(self.importing));
        if let Some(project) = self.backup_project.and_then(|i| self.backup_projects.get(i)) {
            draft.insert("BackupProjectCombo".into(), project.project.clone());
            if let Some(backup) = self.project_backup.and_then(|i| project.backups.get(i)) {
                draft.insert("ProjectBackupCombo".into(), backup.label.clone());
            }
        }
        if let Some(api) = self.source.and_then(|i| self.sources.get(i)) {
            draft.insert("SourceCombo".into(), repository_label(api));
        }
        if let Some(version) = self.version.and_then(|i| self.versions.get(i)) {
            draft.insert("VersionCombo".into(), version.label.clone());
        }
        draft.insert("AutomaticInstall".into(), flag(self.automatic));
        draft.insert("ManualInstall".into(), flag(!self.automatic));
        draft.insert("LanguageCombo".into(), LANGUAGES[self.language].to_string());
        draft.insert("TemplateCombo".into(), TEMPLATES[self.template].to_string());
        draft.insert("DbContainer".into(), flag(self.db_type == DbType::Container));
        draft.insert("DbSqlServer".into(), flag(self.db_type == DbType::SqlServer));
        draft.insert("DbLocalDb".into(), flag(self.db_type == DbType::LocalDb));
        draft.insert("DbWindowsAuth".into(), flag(self.db_type == DbType::SqlServer && !self.db_sql_auth));
        draft.insert("DbSqlAuth".into(), flag(self.db_type == DbType::SqlServer && self.db_sql_auth));
        draft
    }

    /// Puts a draft back: the choices first and the text last, so text the page fills in from a choice ends up as it
    /// was typed (`FormDraft.Restore`). What is put back counts as typed.
    pub fn restore_draft(&mut self, draft: &BTreeMap<String, String>, cx: &mut Context<Self>) {
        let on = |key: &str| draft.get(key).map(|v| v == "true");
        if let Some(true) = on("FromZip") {
            self.importing = true;
        } else if let Some(true) = on("FromScratch") {
            self.importing = false;
        }
        if let Some(manual) = on("ManualInstall") {
            self.automatic = !manual;
        }
        for (key, db_type) in
            [("DbContainer", DbType::Container), ("DbSqlServer", DbType::SqlServer), ("DbLocalDb", DbType::LocalDb)]
        {
            if on(key) == Some(true) {
                self.choose_db_type(db_type, cx);
            }
        }
        if on("DbSqlAuth") == Some(true) {
            self.choose_db_auth(true, cx);
        } else if on("DbWindowsAuth") == Some(true) {
            self.choose_db_auth(false, cx);
        }
        if let Some(index) =
            draft.get("BackupProjectCombo").and_then(|p| self.backup_projects.iter().position(|b| &b.project == p))
        {
            self.choose_backup_project(index, cx);
            if let Some(i) = draft
                .get("ProjectBackupCombo")
                .and_then(|l| self.backup_projects[index].backups.iter().position(|b| &b.label == l))
            {
                self.choose_project_backup(i, cx);
            }
        }
        if let Some(i) =
            draft.get("SourceCombo").and_then(|l| self.sources.iter().position(|s| repository_label(s) == *l))
            && Some(i) != self.source
        {
            self.source = Some(i);
            self.load_versions(cx);
        }
        if let Some(label) = draft.get("VersionCombo") {
            match self.versions.iter().position(|v| v.ready && &v.label == label) {
                Some(i) => self.version = Some(i),
                // Not there yet (loaded in the background): selected when it comes.
                None => self.pending_version = Some(label.clone()),
            }
        }
        if let Some(i) = draft.get("LanguageCombo").and_then(|l| LANGUAGES.iter().position(|x| x == l)) {
            self.language = i;
        }
        if let Some(i) = draft.get("TemplateCombo").and_then(|t| TEMPLATES.iter().position(|x| x == t)) {
            self.template = i;
        }
        // Restored text counts as typed: what follows the name stays as it was.
        let typed = |key: &str| draft.get(key).is_some_and(|t| !t.trim().is_empty());
        self.host_edited = typed("HostNameBox");
        self.website_edited = typed("WebsiteNameBox");
        self.db_name_edited = typed("DbNameBox");
        for field in Field::ALL {
            if let Some(text) = draft.get(field.key())
                && *self.inputs[&field].read(cx).text() != *text
            {
                let text = text.clone();
                self.inputs[&field].update(cx, |input, cx| input.set_text(text, cx));
            }
        }
        self.update_state(cx);
    }

    fn load_draft(&mut self, cx: &mut Context<Self>) {
        let lookups = Arc::clone(&self.lookups);
        self._draft_load = Some(cx.spawn(async move |this, cx| {
            let draft = cx.background_executor().spawn(async move { lookups.load_draft() }).await;
            let _ = this.update(cx, |page, cx| {
                page.draft_loaded = true;
                if !draft.is_empty() {
                    page.restore_draft(&draft, cx);
                }
            });
        }));
    }

    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        if !self.draft_loaded {
            return;
        }
        let lookups = Arc::clone(&self.lookups);
        // A later change replaces this one: the draft is saved once the form stands still.
        self.draft_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DRAFT_AFTER).await;
            let Ok(draft) = this.update(cx, |page, cx| page.capture_draft(cx)) else { return };
            cx.background_executor().spawn(async move { lookups.save_draft(&draft) }).await;
        }));
    }
}

impl Focusable for SetupPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ─── Drawing ─────────────────────────────────────────────────────────────────

/// A card's frame (`Card`): the card colours, padding 18 × 14; its parts `gap` apart. A flex column: every text in it
/// is as wide as the card (`w_full`), so its wrapped height is measured as it is drawn.
fn card_frame(gap: f32, cx: &App) -> Div {
    let theme = Theme::get(cx);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(gap))
        .pad(px(14.0), px(18.0))
        .rounded(theme.metrics.card_radius)
        .border_1()
        .border_color(theme.colors.card_border)
        .bg(theme.colors.card_bg)
}

/// What a title explains, shown in an info tip beside it: the tip's element id and its text.
type Tip = Option<(&'static str, SharedString)>;

/// `title` with an info tip beside it when there is one.
fn tipped(title: Div, tip: Tip, cx: &App) -> Div {
    match tip {
        Some((id, text)) => ui::with_tip(title, id, text, cx),
        None => title,
    }
}

/// A card with its title (and the tip beside it), `body` 10 under it.
fn card(title: &'static str, tip: Tip, body: impl IntoElement, cx: &App) -> Div {
    card_frame(10.0, cx).child(tipped(card_title(title, cx), tip, cx)).child(body)
}

/// `CardTitle`: heading size, semibold.
fn card_title(title: &'static str, cx: &App) -> Div {
    let theme = Theme::get(cx);
    div().text_size(theme.text.heading).font_weight(FontWeight::SEMIBOLD).child(title)
}

/// Parts one under the other, `gap` apart, as wide as their parent.
fn stack(gap: f32) -> Div {
    div().w_full().flex().flex_col().gap(px(gap))
}

/// A field: its label (`FieldLabel`, muted) 4 above its box.
fn field(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    stack(4.0).child(field_label(label, None, cx)).child(control)
}

/// `FieldLabel`: muted - with the info tip beside it when there is one.
fn field_label(text: impl Into<SharedString>, tip: Tip, cx: &App) -> Div {
    let label = div().text_color(Theme::get(cx).colors.text_muted).child(text.into());
    match tip {
        Some(_) => tipped(label, tip, cx),
        None => label.w_full(),
    }
}

/// `Hint`: muted, caption size.
fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = Theme::get(cx);
    // As wide as the card: measured narrower, its wrapped height would leave a gap under it.
    div().w_full().text_size(theme.text.caption).text_color(theme.colors.text_muted).child(text.into())
}

/// `ErrorLine`: red; nothing when empty.
fn error_line(text: &str, cx: &App) -> Option<Div> {
    (!text.is_empty()).then(|| div().w_full().text_color(Theme::get(cx).colors.error_text).child(text.to_string()))
}

fn two_columns(left: impl IntoElement, right: impl IntoElement, right_width: Option<f32>) -> Div {
    let right = div().flex().flex_col().child(div().w_full().child(right));
    let right = match right_width {
        Some(w) => right.w(px(w)).flex_none(),
        None => right.flex_1().min_w_0(),
    };
    // Each side as wide as its column - GPUI doesn't stretch a flex item's children, and a label would wrap per letter.
    div()
        .w_full()
        .flex()
        .gap(px(12.0))
        .child(div().flex().flex_col().flex_1().min_w_0().child(div().w_full().child(left)))
        .child(right)
}

impl SetupPage {
    /// A list (`ComboBox`): its value, the options under it while open; a click on one chooses it.
    fn list(
        &self,
        list: List,
        id: &'static str,
        value: String,
        enabled: bool,
        options: Vec<(String, bool)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open == Some(list) && enabled;
        let items: Vec<AnyElement> = options
            .into_iter()
            .enumerate()
            .map(|(i, (label, chosen))| {
                // On the press: the list is an overlay outside the box, and a press outside the box closes the list
                // before a click could arrive.
                option((id, i), label, chosen, cx)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |page, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        page.open = None;
                        page.choose(list, i, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .id(id)
            .when(!enabled, |d| d.opacity(0.5))
            .when(enabled, |d| {
                d.on_click(cx.listener(move |page, _: &ClickEvent, _, cx| {
                    page.open = if page.open == Some(list) { None } else { Some(list) };
                    cx.notify();
                }))
            })
            .on_mouse_down_out(cx.listener(move |page, _, _, cx| {
                if page.open == Some(list) {
                    page.open = None;
                    cx.notify();
                }
            }))
            .child(select(SharedString::from(format!("{id}-box")), value, open, items, cx))
            .into_any_element()
    }

    fn choose(&mut self, list: List, index: usize, cx: &mut Context<Self>) {
        match list {
            List::BackupProject => self.choose_backup_project(index, cx),
            List::ProjectBackup => self.choose_project_backup(index, cx),
            List::Source => {
                self.source = Some(index);
                self.pending_version = None;
                self.load_versions(cx);
            }
            List::Version => {
                self.version = Some(index);
                self.pending_version = None;
            }
            List::Language => self.language = index,
            List::Template => self.template = index,
        }
        self.update_state(cx);
    }

    fn text_field(&self, field: Field, window: &Window, cx: &App) -> Div {
        dialog::text_field(&self.inputs[&field], window, cx)
    }
}

impl Render for SetupPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let name = self.name(cx);
        let suffix = suffix_of(&self.settings);
        let importing = self.importing;
        let fresh = !importing;
        let automatic = self.automatic;

        // The name.
        let folder = self.lookups.project_directory(&name);
        let valid = validate_project_name(&name).is_ok();
        let exists = valid && self.lookups.project_exists(&name);
        let elsewhere =
            if valid && !exists { site_serving_another_folder(self.sites.as_deref(), &name, &folder) } else { None };
        let name_error = name_error(&name, exists, elsewhere.as_deref()).unwrap_or_default();

        // IIS.
        let host = self.text(Field::HostName, cx);
        let port = self.text(Field::Port, cx);
        let problem = iis_problem(&host, &port, self.sites.as_deref(), &name);
        let (iis_hint, iis_bad) = iis_hint(&name, &host, &port, problem.as_deref(), &suffix);

        // Import.
        let zip = self.text(Field::Zip, cx);
        let backup = self.text(Field::Backup, cx);
        let import_problem = import_problem(&zip, &backup, &|p| p.is_file());
        // Nothing chosen yet isn't an error - the disabled button says enough.
        let import_started = !zip.trim().is_empty() || !backup.trim().is_empty();
        let import_error = if import_started { import_problem.clone().unwrap_or_default() } else { String::new() };

        // The account.
        let account = self.account(cx);
        let account_problems = if fresh && automatic { self.lookups.account_problems(&account) } else { Vec::new() };
        // Only once something is typed - an empty form isn't an error yet.
        let account_started = !account.password.is_empty() || !name.is_empty();
        let account_error = if account_started { account_problems.join(" ") } else { String::new() };

        // The database.
        let fields = self.db_fields(cx);
        let (db_error, db_tooltip, db_hint) =
            database_texts(&fields, &self.db_answer, automatic, !name.is_empty(), self.db_edited);

        let ready = self.readiness(cx);
        let can_run = ready.can_run();
        let still_needed = ready.still_needed();

        let page = div()
            .id("setup-page")
            .size_full()
            .overflow_y_scroll()
            .track_focus(&self.focus)
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            .font_family(theme.font.clone());
        // The page's title with what it does in the tip beside it, the cards 16 under it and 14 apart.
        let header = ui::with_tip_sized(
            div().text_size(theme.text.display).font_weight(FontWeight::SEMIBOLD).child("New project"),
            theme.text.display,
            "new-project-tip",
            "Creates a project from a fresh DNN download - installed for you, so the first visit shows your new site - or from a .zip of an existing site (with its .bacpac / .bak), with its IIS website, hostname and database.",
            cx,
        );
        let mut cards = stack(14.0);

        // Project: the name's rules in the tip beside its label, why it can't be used under its box.
        cards = cards.child(card(
            "Project",
            None,
            stack(4.0)
                .child(field_label(
                    "Project name",
                    Some(("project-name-tip", name_hint(importing, &container_server(&self.settings)).into())),
                    cx,
                ))
                .child(self.text_field(Field::Name, window, cx))
                .children(error_line(&name_error, cx)),
            cx,
        ));

        // IIS - right under the name it follows, for a new site and an imported one alike. Where the site answers in
        // the tip beside the title; what's wrong with the host name or port in red under the fields.
        let iis_tip = (!iis_bad).then(|| ("iis-tip", SharedString::from(iis_hint.clone())));
        cards = cards.child(card(
            "IIS",
            iis_tip,
            stack(4.0)
                .child(two_columns(
                    field("Host name", self.text_field(Field::HostName, window, cx), cx),
                    field("Port", self.text_field(Field::Port, window, cx), cx),
                    Some(120.0),
                ))
                .when(iis_bad, |d| d.child(hint(iis_hint, cx).text_color(c.error_text))),
            cx,
        ));

        // Start from: the radio buttons keep 4 above and under them.
        cards = cards.child(card(
            "Start from",
            None,
            stack(0.0)
                .child(radio("from-scratch", !importing, "A new site - download a DNN release", cx).on_click(
                    cx.listener(|page, _: &ClickEvent, _, cx| {
                        page.importing = false;
                        page.update_state(cx);
                    }),
                ))
                .child(
                    radio("from-zip", importing, "An existing site - import a .zip of its files and its .bacpac", cx)
                        .on_click(cx.listener(|page, _: &ClickEvent, _, cx| {
                            page.importing = true;
                            page.update_state(cx);
                        })),
                ),
            cx,
        ));

        // Import: the fields 8 apart, what each takes in the tip beside its label.
        if importing {
            let mut import = stack(8.0);
            // Only when a project has a backup to start from.
            if !self.backup_projects.is_empty() {
                let projects: Vec<(String, bool)> = self
                    .backup_projects
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.project.clone(), Some(i) == self.backup_project))
                    .collect();
                let chosen = self.backup_project.and_then(|i| self.backup_projects.get(i));
                let backups: Vec<(String, bool)> = chosen
                    .map(|p| {
                        p.backups
                            .iter()
                            .enumerate()
                            .map(|(i, b)| (b.label.clone(), Some(i) == self.project_backup))
                            .collect()
                    })
                    .unwrap_or_default();
                let project_value = chosen.map(|p| p.project.clone()).unwrap_or_default();
                let backup_value = chosen
                    .and_then(|p| self.project_backup.and_then(|i| p.backups.get(i)))
                    .map(|b| b.label.clone())
                    .unwrap_or_default();
                let backups_enabled = !backups.is_empty();
                import = import.child(
                    stack(4.0)
                        .child(field_label(
                            "From a project backup",
                            Some((
                                "project-backup-tip",
                                "A project and one of its backups (Documents\\DnnManager\\backups) - or choose the files anywhere on this PC below.".into(),
                            )),
                            cx,
                        ))
                        .child(two_columns(
                            self.list(List::BackupProject, "backup-project", project_value, true, projects, cx),
                            self.list(List::ProjectBackup, "project-backup", backup_value, backups_enabled, backups, cx),
                            None,
                        )),
                );
            }
            let browse = |id: &'static str, zip: bool, cx: &mut Context<Self>| {
                ui::button(id, "Browse…", ButtonKind::Secondary, true, cx)
                    .tip("Browse")
                    .on_click(cx.listener(move |page, _: &ClickEvent, _, cx| page.browse(zip, cx)))
            };
            import = import
                .child(
                    stack(4.0)
                        .child(field_label(
                            "Site .zip",
                            Some((
                                "site-zip-tip",
                                "The site's files - web.config at the top of the zip or in one folder inside it."
                                    .into(),
                            )),
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(self.text_field(Field::Zip, window, cx).flex_1())
                                .child(browse("browse-zip", true, cx)),
                        ),
                )
                .child(
                    stack(4.0)
                        .child(field_label(
                            "Database .bacpac",
                            Some((
                                "database-backup-tip",
                                "The site's database - restored into the project's database. A .bak works too.".into(),
                            )),
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(self.text_field(Field::Backup, window, cx).flex_1())
                                .child(browse("browse-backup", false, cx)),
                        )
                        .children(error_line(&import_error, cx)),
                );
            cards = cards.child(card("Import", None, import, cx));
        }

        if fresh {
            // DNN package.
            let sources: Vec<(String, bool)> =
                self.sources.iter().enumerate().map(|(i, s)| (repository_label(s), Some(i) == self.source)).collect();
            let source_value =
                self.source.and_then(|i| self.sources.get(i)).map(|s| repository_label(s)).unwrap_or_default();
            let versions: Vec<(String, bool)> =
                self.versions.iter().enumerate().map(|(i, v)| (v.label.clone(), Some(i) == self.version)).collect();
            let version_value =
                self.version.and_then(|i| self.versions.get(i)).map(|v| v.label.clone()).unwrap_or_default();
            let versions_enabled = self.versions.first().is_some_and(|v| v.ready);
            // The list explained in the tip beside Version; what GitHub answered (asking, offline, failed) under it.
            let version_tip =
                self.version_hint_explains.then(|| ("version-tip", SharedString::from(self.version_hint.clone())));
            let version_status = (!self.version_hint_explains).then(|| hint(self.version_hint.clone(), cx));
            cards = cards.child(card(
                "DNN package",
                None,
                stack(4.0)
                    .child(two_columns(
                        field("Repository", self.list(List::Source, "source", source_value, true, sources, cx), cx),
                        stack(4.0).child(field_label("Version", version_tip, cx)).child(self.list(
                            List::Version,
                            "version",
                            version_value,
                            versions_enabled,
                            versions,
                            cx,
                        )),
                        None,
                    ))
                    .children(version_status),
                cx,
            ));

            // DNN installation: each choice with what it does in the tip right after its text; the choices stacked as
            // Start from's (the radio buttons keep 4 above and under them).
            let install_choice = |id: &'static str,
                                  tip_id: &'static str,
                                  chosen: bool,
                                  title: &'static str,
                                  text: &'static str,
                                  cx: &mut Context<Self>| {
                radio(id, chosen, title, cx)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(ui::info_tip(tip_id, text, cx))
                    .on_click(cx.listener(move |page, _: &ClickEvent, _, cx| {
                        page.automatic = id == "automatic-install";
                        page.update_state(cx);
                    }))
            };
            cards = cards.child(card(
                "DNN installation",
                None,
                stack(0.0)
                    .child(install_choice(
                        "automatic-install",
                        "automatic-install-tip",
                        automatic,
                        "Automatic setup (recommended)",
                        "DNN Manager installs DNN with the account and website below - the first visit shows your new site, not DNN's installation wizard.",
                        cx,
                    ))
                    .child(install_choice(
                        "manual-install",
                        "manual-install-tip",
                        !automatic,
                        "Manual DNN setup",
                        "DNN's own installation wizard opens on the first visit, for you to fill in.",
                        cx,
                    )),
                cx,
            ));
        }

        if fresh && automatic {
            // DNN account and website: the rows of fields 8 apart.
            let languages: Vec<(String, bool)> =
                LANGUAGES.iter().enumerate().map(|(i, l)| (language_name(l), i == self.language)).collect();
            let templates: Vec<(String, bool)> =
                TEMPLATES.iter().enumerate().map(|(i, t)| (t.to_string(), i == self.template)).collect();
            let generate = ui::link("generate-password", "Generate", cx)
                .text_size(theme.text.caption)
                .tip("Generate password")
                .on_click(cx.listener(|page, _: &ClickEvent, window, cx| page.generate_password(window, cx)));
            let language =
                self.list(List::Language, "language", language_name(LANGUAGES[self.language]), true, languages, cx);
            let template =
                self.list(List::Template, "template", TEMPLATES[self.template].to_string(), true, templates, cx);
            let account_fields = stack(8.0)
                .child(two_columns(
                    field("Host username", self.text_field(Field::HostUser, window, cx), cx),
                    stack(4.0)
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(div().text_color(c.text_muted).child("Host password"))
                                .child(generate),
                        )
                        .child(self.host_password.clone()),
                    None,
                ))
                .child(two_columns(
                    field("Host e-mail", self.text_field(Field::HostEmail, window, cx), cx),
                    field("Website name", self.text_field(Field::WebsiteName, window, cx), cx),
                    None,
                ))
                .child(two_columns(field("Language", language, cx), field("Site template", template, cx), None));
            cards = cards.child(card(
                "DNN account and website",
                Some(("account-tip", account_hint(LANGUAGES[self.language]).into())),
                stack(4.0).child(account_fields).children(error_line(&account_error, cx)),
                cx,
            ));
        }

        if fresh {
            // Database - filled in from Settings → Database server, changed here for this project only.
            let db_type = self.db_type;
            let sql_login = fields.sql_login();
            let type_radio = |id: &'static str, t: DbType, text: &'static str, cx: &mut Context<Self>| {
                radio(id, db_type == t, text, cx)
                    .on_click(cx.listener(move |page, _: &ClickEvent, _, cx| page.choose_db_type(t, cx)))
            };
            // What the server answered stays under the fields; the hint that only explains (the LocalDB file, "for this
            // project only") is the tip beside the title.
            let db_status = !database_said(&fields, &self.db_answer, automatic).is_empty();
            let db_tip =
                (!db_status && !db_hint.is_empty()).then(|| ("database-tip", SharedString::from(db_hint.clone())));
            let title = div()
                .w_full()
                .flex()
                .justify_between()
                .items_start()
                .child(tipped(card_title("Database", cx), db_tip, cx))
                .when(self.db_edited || self.db_name_edited, |d| {
                    d.child(
                        ui::link("use-the-settings", "Use the settings", cx)
                            .text_size(theme.text.caption)
                            .tip("Use the settings")
                            .on_click(cx.listener(|page, _: &ClickEvent, _, cx| page.use_the_settings(cx))),
                    )
                });
            // The connection type: its label, the radio buttons 8 under it (they keep 4 above and under them).
            let connection_type = stack(4.0).child(field_label("Connection type", None, cx)).child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(20.0))
                    .child(type_radio("db-container", DbType::Container, "Local SQL container (Docker)", cx))
                    .child(type_radio("db-sqlserver", DbType::SqlServer, "SQL Server / SQL Server Express", cx))
                    .child(type_radio("db-localdb", DbType::LocalDb, "LocalDB file", cx)),
            );
            let server_label = if db_type == DbType::LocalDb { "LocalDB instance" } else { "Server" };
            let server = field(server_label, self.text_field(Field::DbServer, window, cx), cx);
            let server_row = if db_type == DbType::LocalDb {
                two_columns(server, div(), None)
            } else {
                two_columns(server, field("Database name", self.text_field(Field::DbName, window, cx), cx), None)
            };
            let login = sql_login.then(|| {
                two_columns(
                    field("Username", self.text_field(Field::DbUser, window, cx), cx),
                    field("Password", self.db_password.clone(), cx),
                    None,
                )
            });
            // The server's row, then SQL Server's authentication 14 under it and the login 12 under that (less the radio
            // buttons' own 4) - or, for the container, the login 8 under the server's row.
            let connection = if db_type == DbType::SqlServer {
                let sql_auth = self.db_sql_auth;
                let auth = div()
                    .flex()
                    .gap(px(20.0))
                    .child(
                        radio("db-windows-auth", !sql_auth, "Windows authentication", cx)
                            .on_click(cx.listener(|page, _: &ClickEvent, _, cx| page.choose_db_auth(false, cx))),
                    )
                    .child(
                        radio("db-sql-auth", sql_auth, "SQL Server authentication", cx)
                            .on_click(cx.listener(|page, _: &ClickEvent, _, cx| page.choose_db_auth(true, cx))),
                    );
                stack(10.0).child(server_row).child(stack(8.0).child(auth).children(login))
            } else {
                stack(8.0).child(server_row).children(login)
            };
            let mut notes = Vec::new();
            if let Some(line) = error_line(&db_error, cx) {
                notes.push(match db_tooltip {
                    Some(tip) => div().id("database-error").tip(tip).child(line).into_any_element(),
                    None => line.into_any_element(),
                });
            }
            if db_status {
                notes.push(hint(db_hint, cx).into_any_element());
            }
            // The error and the hint 4 under the fields.
            let body = stack(4.0)
                .child(stack(8.0).child(connection_type).child(connection))
                .when(!notes.is_empty(), |d| d.child(stack(4.0).children(notes)));
            cards = cards.child(card_frame(10.0, cx).child(title).child(body));
        }

        // Create project, and why it can't be clicked yet: a disabled button says nothing.
        let run_label = if importing { "Import project" } else { "Create project" };
        cards = cards.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    ui::button("run", run_label, ButtonKind::Primary, can_run, cx)
                        .tip(
                            "Create the website with these choices - the panel at the bottom shows each step, Cancel undoes it",
                        )
                        .when(can_run, |b| b.on_click(cx.listener(|page, _: &ClickEvent, _, cx| page.run(cx)))),
                )
                .when(!still_needed.is_empty(), |d| d.child(div().max_w(px(520.0)).child(hint(still_needed, cx)))),
        );

        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .max_w(px(860.0))
            .gap(px(16.0))
            .pad(px(22.0), px(28.0))
            .child(header)
            .child(cards);

        // Centred at most 860 wide: a row that gives the column a definite width, so its text wraps (auto margins
        // don't).
        page.child(div().flex().justify_center().w_full().child(column))
    }
}

#[cfg(test)]
mod tests;
