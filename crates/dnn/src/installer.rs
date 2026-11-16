//! DNN's own unattended install - `GET /Install/Install.aspx?mode=install` with an install template that names the
//! host account and the website - and upgrade (`mode=upgrade`), and what DNN's installation wizard does around them
//! (`src/DnnManager.Infrastructure/Dnn/DnnInstaller.cs`, `IDnnInstaller`). The first request has DNN write new machine
//! keys and redirect to itself; the second runs the whole install and streams its progress, ending in "Installation
//! Complete" (DNN then deletes its installer).
//!
//! What needs SQL Server goes through `InstallerSql`, implemented where SQL is (another crate); a LocalDB file
//! database is that implementation's to open through DNN Manager's own instance (`LocalDbFiles.WithDatabaseAsync`).

use std::path::Path;
use std::sync::LazyLock;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use dnnm_application::operation::{CANCELLED, CancelToken, Reporter};
use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind, load_site_xml, read_database_connection};
use regex::Regex;
use url::Url;

use crate::install::{
    DnnAccount, DnnSiteAddress, MAX_PASSWORD_LENGTH, MAX_SITE_PATH_LENGTH, MIN_PASSWORD_LENGTH, account_problems,
};
use crate::install_output::{DnnInstallOutput, DnnInstallStep, html_decode};
use crate::install_template;
use crate::log_files::{files_ending, log_time, modified, read_shared, script_problems, short, starts_with_date};
use crate::packages::Progress;
use crate::safe_zip::{has_link, is_hard_linked};
use crate::site_http::{get, local_agent, location};

// Measured: 35-60 s for the whole install, gaps between progress lines of at most ~11 s.
const INSTALL_LIMIT: Duration = Duration::from_secs(30 * 60);
const QUIET_WARNING: Duration = Duration::from_secs(2 * 60);
const QUIET_LIMIT: Duration = Duration::from_secs(10 * 60);
// The first page after the install runs the modules' upgrades: 12-45 s measured.
const WARM_UP_LIMIT: Duration = Duration::from_secs(3 * 60);
const MAX_REDIRECTS: usize = 6;

/// Which of the database's `Version` rows counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionOrder {
    /// The highest version (`ORDER BY Major DESC, Minor DESC, Build DESC`).
    Highest,
    /// The one written last (`ORDER BY VersionId DESC`) - right after an install.
    LastWritten,
}

/// The DNN version a database is at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbVersion {
    /// The database has no DNN tables (`DnnTables.QualifierAsync` found no `Version` table).
    NoDnnTables,
    /// `Major.Minor.Build` of its `Version` table - None when the table is empty.
    At(Option<String>),
}

/// How setting a host password ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPasswordOutcome {
    Changed,
    NoDnnTables,
    /// No host (`IsSuperUser = 1`) of that name.
    NotAHost,
    /// `aspnet_Membership_SetPassword` returned non-zero: no such user in the membership application.
    NoMembershipUser,
}

/// The SQL the installer needs - values as parameters, table names with DNN's object qualifier. Err: the database
/// couldn't be reached or the query failed (its message; DNN Manager keeps passwords out of it).
pub trait InstallerSql: Send + Sync {
    /// For a LocalDB file database: raises the database's compatibility level to the server's (`model`'s) when it is
    /// lower - DNN's shipped `Database.mdf` is at SQL Server 2008's.
    fn raise_compatibility_level(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String>;

    fn version(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        order: VersionOrder,
        cancel: &CancelToken,
    ) -> Result<DbVersion, String>;

    /// `SELECT COUNT(*) FROM PortalAlias WHERE PortalID = 0 AND HTTPAlias = @alias`.
    fn portal_alias_count(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        alias: &str,
        cancel: &CancelToken,
    ) -> Result<i64, String>;

    /// After an install, as DNN's wizard leaves it: `UPDATE Users SET UpdatePassword = 0 WHERE IsSuperUser = 1 AND
    /// Username = @user` (the rows it changed) and `UPDATE Tabs SET IsSecure = 0 WHERE PortalID = 0 AND IsSecure = 1`.
    fn finish_host(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        user: &str,
        cancel: &CancelToken,
    ) -> Result<i64, String>;

    /// In one transaction: checks `user` is a host, `aspnet_Membership_SetPassword` (format 1, hashed) with `hash` and
    /// `salt`, `aspnet_Membership_UnlockUser`, and `UpdatePassword = 0`.
    #[allow(clippy::too_many_arguments)]
    fn set_host_password(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        application_name: &str,
        user: &str,
        hash: &str,
        salt: &str,
        cancel: &CancelToken,
    ) -> Result<HostPasswordOutcome, String>;

    /// Opens the connection (not pooled) and closes it again - whether it works from here.
    fn try_connect(&self, database: &DatabaseConnection) -> Result<(), String>;
}

/// How DNN's membership provider hashes a password (`MembershipPasswords`, ported elsewhere).
pub trait PasswordHasher {
    fn is_supported(&self, algorithm: &str) -> bool;
    fn new_salt(&self) -> String;
    fn hash(&self, password: &str, salt: &str, algorithm: &str) -> String;
}

/// `IDnnInstaller`.
pub struct Installer<S: InstallerSql> {
    sql: S,
}

/// Passwords to keep out of every message - as they are and as XML, HTML or a URL would carry them.
pub struct Secrets(Vec<String>);

impl Secrets {
    pub fn new(secrets: &[&str]) -> Self {
        let mut forms: Vec<String> = secrets
            .iter()
            .filter(|s| s.chars().count() >= 4)
            .flat_map(|s| [s.to_string(), xml_escape(s), html_encode(s), uri_escape(s)])
            .collect();
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        forms.dedup();
        Self(forms)
    }

    pub fn hide(&self, text: &str) -> String {
        self.0.iter().fold(text.to_string(), |text, form| text.replace(form.as_str(), "***"))
    }
}

/// `SecurityElement.Escape`.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `WebUtility.HtmlEncode`.
fn html_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '&' => "&amp;".into(),
            '\'' => "&#39;".into(),
            c if ('\u{a0}'..='\u{ff}').contains(&c) => format!("&#{};", c as u32),
            c => c.to_string(),
        })
        .collect()
}

/// `Uri.EscapeDataString`.
fn uri_escape(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or_default().trim().to_string()
}

fn starts_with(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(prefix))
}

fn after(text: &str, prefix: &str) -> String {
    text[prefix.len()..].trim().trim_start_matches(':').trim().to_string()
}

fn try_delete(file: &Path) {
    if file.is_file() {
        let _ = std::fs::remove_file(file);
    }
}

/// The DNN version of the site's files: bin\DotNetNuke.dll's assembly version, e.g. "10.3.3"; None when it can't be read.
fn dnn_version_of(site_directory: &Path) -> Option<String> {
    let info = crate::metadata::read(&site_directory.join("bin").join("DotNetNuke.dll"))?;
    Some(info.version.to_string_n(3))
}

/// The appSettings of a web.config, by key (case ignored).
fn app_settings(web_config: &Path) -> Result<Vec<(String, String)>, String> {
    if !web_config.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(web_config).map_err(|e| e.to_string())?;
    let doc = load_site_xml(text.trim_start_matches('\u{feff}'))?;
    let root = doc.root_element();
    Ok(root
        .children()
        .filter(|n| n.has_tag_name("appSettings"))
        .take(1)
        .flat_map(|s| s.children().filter(|n| n.has_tag_name("add")))
        .filter_map(|add| {
            Some((add.attribute("key")?.to_string(), add.attribute("value").unwrap_or_default().to_string()))
        })
        .collect())
}

/// The extension packages DNN installs with itself - one progress line each.
fn count_packages(site_directory: &Path) -> usize {
    fn walk(folder: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
        entries
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(t) if t.is_file() && e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) => 1,
                _ => 0,
            })
            .sum()
    }
    walk(&site_directory.join("Install"))
}

/// ASP.NET's error page's title - "Could not load file or assembly 'AngleSharp'…" - or None when it says nothing.
pub fn error_of(html: &str) -> Option<String> {
    static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title>\s*(.*?)\s*</title>").expect("valid"));
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid"));
    let text = TITLE
        .captures(html)
        .map(|t| html_decode(&SPACES.replace_all(&t[1], " ")).trim().to_string())
        .unwrap_or_default();
    (!text.is_empty() && !text.eq_ignore_ascii_case("Runtime Error")).then(|| short(&text))
}

/// What a request to the site, run on a thread of its own, tells as it goes.
enum Event {
    Head {
        status: u16,
        reason: String,
        location: Option<Url>,
    },
    Body(Vec<u8>),
    End,
    /// Not reached (`HttpRequestException`).
    Unreachable(String),
    /// Broke off while reading (`IOException`).
    BrokeOff(String),
}

/// Starts GET `path` on the site on a thread of its own, so Cancel, the time limit and the quiet watchdog are seen while
/// DNN takes its time; the thread ends with the request (its own time limit is the install's).
fn start_request(site: &DnnSiteAddress, path: &str, total: Duration) -> mpsc::Receiver<Event> {
    let (send, receive) = mpsc::channel();
    let (port, alias, path) = (site.port, site.alias.clone(), path.to_string());
    let base = Url::parse(&site.url()).ok();
    std::thread::spawn(move || {
        let agent = local_agent(Duration::from_secs(30), Some(total));
        let response = match get(&agent, port, &alias, &path) {
            Ok(response) => response,
            Err(e) => {
                let _ = send.send(Event::Unreachable(e));
                return;
            }
        };
        let status = response.status();
        let target = base.as_ref().and_then(|b| location(&response, b));
        let head = Event::Head {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or_default().to_string(),
            location: target,
        };
        if send.send(head).is_err() {
            return;
        }
        let mut reader = response.into_body().into_reader();
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(0) => {
                    let _ = send.send(Event::End);
                    return;
                }
                Ok(read) => {
                    if send.send(Event::Body(buffer[..read].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = send.send(Event::BrokeOff(e.to_string()));
                    return;
                }
            }
        }
    });
    receive
}

/// UTF-8 decoded as it streams in - a character split between two chunks waits for the rest.
#[derive(Default)]
struct Decoder(Vec<u8>);

impl Decoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.0.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.0) {
                Ok(text) => {
                    out.push_str(text);
                    self.0.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.0[..valid]).expect("valid prefix"));
                    match e.error_len() {
                        Some(bad) => {
                            out.push('\u{fffd}');
                            self.0.drain(..valid + bad);
                        }
                        None => {
                            self.0.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }
}

/// How the page run ended before it was read whole.
enum Stop {
    Cancelled,
    Stuck,
    TooLong,
}

/// The site's `membership` settings from web.config - what decides how DNN stores and checks passwords.
struct Membership {
    password_format: String,
    hash_algorithm: Option<String>,
    application_name: String,
    min_length: usize,
    min_non_alphanumeric: usize,
    strength_pattern: Option<String>,
}

impl Membership {
    fn read(web_config: &Path) -> Result<Option<Self>, String> {
        if !web_config.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(web_config).map_err(|e| e.to_string())?;
        let doc = load_site_xml(text.trim_start_matches('\u{feff}'))?;
        fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
            node.children().find(|n| n.has_tag_name(name))
        }
        let Some(membership) = child(doc.root_element(), "system.web").and_then(|w| child(w, "membership")) else {
            return Ok(None);
        };
        let name = membership.attribute("defaultProvider");
        let Some(provider) = child(membership, "providers").and_then(|p| {
            p.children()
                .filter(|n| n.has_tag_name("add"))
                .find(|n| name.is_none_or(|name| n.attribute("name").is_some_and(|a| a.eq_ignore_ascii_case(name))))
        }) else {
            return Ok(None);
        };
        let non_empty = |v: Option<&str>| v.filter(|v| !v.is_empty()).map(str::to_string);
        Ok(Some(Self {
            password_format: provider.attribute("passwordFormat").unwrap_or("Hashed").to_string(),
            hash_algorithm: non_empty(membership.attribute("hashAlgorithmType")),
            application_name: provider.attribute("applicationName").unwrap_or("DotNetNuke").to_string(),
            min_length: provider
                .attribute("minRequiredPasswordLength")
                .and_then(|v| v.parse().ok())
                .unwrap_or(MIN_PASSWORD_LENGTH),
            min_non_alphanumeric: provider
                .attribute("minRequiredNonalphanumericCharacters")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            strength_pattern: non_empty(provider.attribute("passwordStrengthRegularExpression")),
        }))
    }

    /// What the site's membership rules have against `password`; None when it may be used.
    fn problem(&self, password: &str) -> Option<String> {
        let length = password.encode_utf16().count();
        if length < self.min_length {
            return Some(format!("The site wants passwords of at least {} characters.", self.min_length));
        }
        if length > MAX_PASSWORD_LENGTH {
            return Some(format!("The password can have at most {MAX_PASSWORD_LENGTH} characters."));
        }
        if password.chars().filter(|c| !c.is_alphanumeric()).count() < self.min_non_alphanumeric {
            return Some(format!(
                "The site wants at least {} characters that aren't letters or digits.",
                self.min_non_alphanumeric
            ));
        }
        // .NET's pattern read with Rust's regex: one it can't read counts as not met.
        if let Some(pattern) = &self.strength_pattern
            && !Regex::new(pattern).is_ok_and(|r| r.is_match(password))
        {
            return Some(
                "The password doesn't meet the site's password rules (passwordStrengthRegularExpression).".into(),
            );
        }
        None
    }
}

impl<S: InstallerSql> Installer<S> {
    pub fn new(sql: S) -> Self {
        Self { sql }
    }

    /// Whether the freshly extracted DNN in `site_directory` can be installed unattended: its installer is there, it
    /// isn't installed yet, and its folder isn't too deep. Checked before anything else is set up.
    pub fn check_package(&self, site_directory: &Path) -> Result<(), String> {
        let length = site_directory.as_os_str().to_string_lossy().encode_utf16().count();
        if length > MAX_SITE_PATH_LENGTH {
            return Err(format!(
                "The site's folder is too deep for DNN's installer ({length} characters, at most {MAX_SITE_PATH_LENGTH}) - some of its extensions would fail to install. Use a shorter projects folder (Settings → Projects), e.g. C:\\DNN."
            ));
        }
        let install = site_directory.join("Install");
        if !install.join("Install.aspx").is_file() {
            return Err(
                "The package has no Install\\Install.aspx - it isn't a DNN install package, or it was installed before."
                    .into(),
            );
        }
        if !install.join(install_template::SHIPPED_FILE_NAME).is_file() {
            return Err(format!(
                "The package has no Install\\{} - DNN's install template.",
                install_template::SHIPPED_FILE_NAME
            ));
        }
        if site_directory.join("installBlocker.lock").is_file() {
            return Err("An installation of this site is running, or one was interrupted (installBlocker.lock).".into());
        }
        let settings = app_settings(&site_directory.join("web.config"))?;
        if settings
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("InstallVersion") || k.eq_ignore_ascii_case("InstallationDate"))
        {
            return Err(
                "This DNN has been installed (or started installing) before - an automatic install needs a fresh package."
                    .into(),
            );
        }
        Ok(())
    }

    /// For a LocalDB file database: DNN's install package ships `App_Data\Database.mdf` at SQL Server 2008's
    /// compatibility level, which DNN 10's modules can't upgrade in - it is raised to the server's level.
    pub fn prepare_local_db_file(
        &self,
        site_directory: &Path,
        connection: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        self.sql.raise_compatibility_level(site_directory, connection, cancel).map_err(|e| {
            if cancel.is_cancelled() {
                CANCELLED.to_string()
            } else {
                format!("Could not prepare App_Data\\{}: {}", connection.database, first_line(&e))
            }
        })
    }

    /// Installs DNN: writes the install template right before asking the site to install itself, follows DNN's
    /// progress and removes the template again whatever happens. Ok only when DNN reports a complete install without
    /// errors.
    pub fn install(
        &self,
        site: &DnnSiteAddress,
        account: &DnnAccount,
        database: &DatabaseConnection,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let problems = account_problems(account);
        if !problems.is_empty() {
            return Err(problems.join(" "));
        }
        let secrets = Secrets::new(&[&account.password, &database.password]);
        let result = (|| {
            // Written right before the request: while DNN isn't installed its installer asks nobody for a password.
            let template = install_template::write(&site.directory, account, &site.alias)?;
            keep_to_own_app_pool(&template, &site.directory);
            run(
                site,
                "/Install/Install.aspx?mode=install",
                "installation",
                DnnInstallOutput::outcome,
                count_packages(&site.directory),
                &secrets,
                reporter,
                progress,
                cancel,
                None,
                &self.sql,
            )
        })();
        // It holds the host password - and with it any visitor could run the installer again.
        install_template::delete(&site.directory);
        result.map_err(|e| if e == CANCELLED { e } else { secrets.hide(&first_line(&e)) })
    }

    /// DNN's own unattended upgrade after a newer DNN's files were put in: runs its database scripts and extension
    /// upgrades. Ok only when DNN reports a complete upgrade without errors. `raw_output_file`: where to keep everything
    /// DNN answered, as it answered it (passwords left out).
    pub fn upgrade(
        &self,
        site: &DnnSiteAddress,
        database_password: &str,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
        raw_output_file: Option<&Path>,
    ) -> Result<(), String> {
        let secrets = Secrets::new(&[database_password]);
        // An installation or upgrade cut off earlier leaves its lock - DNN would refuse to start.
        try_delete(&site.directory.join("installBlocker.lock"));
        run(
            site,
            "/Install/Install.aspx?mode=upgrade",
            "upgrade",
            DnnInstallOutput::upgrade_outcome,
            count_packages(&site.directory),
            &secrets,
            reporter,
            progress,
            cancel,
            raw_output_file,
            &self.sql,
        )
        .map_err(|e| if e == CANCELLED { e } else { secrets.hide(&first_line(&e)) })
    }

    /// The DNN version the site's database is at - Err when it isn't the version of the site's files (an upgrade that
    /// didn't run or didn't finish).
    pub fn check_version(
        &self,
        site_directory: &Path,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<String, String> {
        let secrets = Secrets::new(&[&database.password]);
        let files = dnn_version_of(site_directory);
        let installed = match self.sql.version(site_directory, database, VersionOrder::Highest, cancel) {
            Ok(DbVersion::NoDnnTables) => return Err("The database has no DNN tables.".into()),
            Ok(DbVersion::At(installed)) => installed,
            Err(_) if cancel.is_cancelled() => return Err(CANCELLED.into()),
            Err(e) => {
                return Err(format!(
                    "Could not read the site's DNN version from its database: {}",
                    secrets.hide(&first_line(&e))
                ));
            }
        };
        if let Some(files) = &files
            && installed.as_deref() != Some(files.as_str())
        {
            return Err(format!(
                "The database is at DNN {}, the site's files at {files}.",
                installed.as_deref().unwrap_or("(nothing)")
            ));
        }
        Ok(installed.or(files).unwrap_or_default())
    }

    /// After a successful install: checks the database holds the installed site, and leaves it as DNN's wizard does -
    /// the host signs in without being made to change the password. DNN caches users: restart the site afterwards.
    pub fn complete(
        &self,
        site: &DnnSiteAddress,
        account: &DnnAccount,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let secrets = Secrets::new(&[&account.password, &database.password]);
        let expected = dnn_version_of(&site.directory);
        let checked = (|| -> Result<Result<(), String>, String> {
            let installed = match self.sql.version(&site.directory, database, VersionOrder::LastWritten, cancel)? {
                DbVersion::NoDnnTables => {
                    return Ok(Err("The database has no DNN tables - the installation didn't reach it.".into()));
                }
                DbVersion::At(installed) => installed,
            };
            if let Some(expected) = &expected
                && installed.as_deref() != Some(expected.as_str())
            {
                return Ok(Err(format!(
                    "The database is at DNN {}, the site's files at {expected}.",
                    installed.as_deref().unwrap_or("(nothing)")
                )));
            }
            if self.sql.portal_alias_count(&site.directory, database, &site.alias, cancel)? == 0 {
                return Ok(Err(format!("DNN didn't create the portal alias {}.", site.alias)));
            }
            // Install.aspx makes the host - the site's administrator since DNN 9.3 - change its password at the first
            // sign-in; the wizard doesn't. Same for the pages it marks secure (the wizard's SSL step undoes that).
            if self.sql.finish_host(&site.directory, database, &account.user_name, cancel)? == 0 {
                return Ok(Err(format!("DNN didn't create the host account '{}'.", account.user_name)));
            }
            Ok(Ok(()))
        })();
        match checked {
            Ok(result) => result,
            Err(_) if cancel.is_cancelled() => Err(CANCELLED.into()),
            Err(e) => Err(format!("Could not check the installed site's database: {}", secrets.hide(&first_line(&e)))),
        }
    }

    /// The first visit after the install - DNN finishes upgrading its modules then - and what it shows: the home page,
    /// not the installer, and no errors in DNN's logs since `install_started`.
    pub fn warm_up(
        &self,
        site: &DnnSiteAddress,
        install_started: DateTime<Utc>,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        progress("Opening the site - DNN finishes its modules on this first visit…");
        let opened_at = Local::now();
        let deadline = Instant::now() + WARM_UP_LIMIT;
        let mut path = "/".to_string();
        let mut hop = 0;
        loop {
            if hop >= 10 {
                return Err("The site keeps redirecting.".into());
            }
            let events = start_request(site, &path, WARM_UP_LIMIT);
            let (status, target, html) = match collect(&events, cancel, deadline) {
                Ok(answer) => answer,
                Err(Collected::Stop(Stop::Cancelled)) => return Err(CANCELLED.into()),
                Err(Collected::Stop(_)) => return Err("The site didn't answer within 3 minutes.".into()),
                Err(Collected::Unreachable(e) | Collected::BrokeOff(e)) => {
                    return Err(format!("Could not reach the site at {}: {}", site.url(), first_line(&e)));
                }
            };
            if (300..400).contains(&status) {
                let Some(target) = target else {
                    return Err(format!("The site answered HTTP {status} without saying where to."));
                };
                let lower_path = target.path().to_ascii_lowercase();
                if lower_path.starts_with("/install") {
                    return Err("The site still sends visitors to DNN's installer.".into());
                }
                if lower_path.contains("errorpage")
                    || target.query().is_some_and(|q| q.to_ascii_lowercase().contains("error="))
                {
                    return Err("The site shows DNN's error page - see its logs in Portals\\_default\\Logs.".into());
                }
                path = format!("{}{}", target.path(), target.query().map(|q| format!("?{q}")).unwrap_or_default());
                hop += 1;
                continue;
            }
            // 503: DNN is still installing or upgrading something (its installBlocker.lock), or the app restarts after
            // it changed web.config - it answers once it is done.
            if status == 503 {
                progress("Opening the site - DNN is still finishing (HTTP 503), waiting…");
                let until = Instant::now() + Duration::from_secs(5);
                while Instant::now() < until {
                    if cancel.is_cancelled() {
                        return Err(CANCELLED.into());
                    }
                    if Instant::now() >= deadline {
                        return Err("The site didn't answer within 3 minutes.".into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
            if status != 200 {
                return Err(format!("The site answered HTTP {status} - see its logs in Portals\\_default\\Logs."));
            }
            if html.to_lowercase().contains("installwizard") {
                return Err("The site still shows DNN's installation wizard.".into());
            }
            break;
        }
        for problem in log_problems(&site.directory, install_started, opened_at) {
            reporter.warn(&problem);
        }
        Ok(())
    }

    /// Removes what still holds a password in plain text: the install template and - after an install - the copies of
    /// web.config DNN made while installing (they have the connection string).
    pub fn clean_up(&self, site_directory: &Path, installed: bool) {
        install_template::delete(site_directory);
        try_delete(&site_directory.join("installBlocker.lock"));
        if !installed {
            return;
        }
        // DNN deletes its installer pages after an install - in case it didn't, nobody should run them again.
        for page in ["Install.aspx", "InstallWizard.aspx", "UpgradeWizard.aspx"] {
            for file in [page.to_string(), format!("{page}.cs"), format!("{page}.designer.cs")] {
                try_delete(&site_directory.join("Install").join(file));
            }
        }
        // Copies of web.config from the install - with the connection string, password included.
        let config = site_directory.join("Config");
        if let Ok(entries) = std::fs::read_dir(&config) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                // A link is removed itself, never followed (`remove_dir_all` doesn't follow one).
                if name.starts_with("backup_") && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }

    /// Sets the password of host account `user_name` the way DNN's membership provider stores it (only for a site that
    /// keeps hashed passwords). DNN caches users: restart the site afterwards.
    pub fn change_host_password(
        &self,
        site_directory: &Path,
        database: &DatabaseConnection,
        user_name: &str,
        new_password: &str,
        hasher: &dyn PasswordHasher,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let Some(membership) = Membership::read(&site_directory.join("web.config"))? else {
            return Err("The site's web.config has no membership provider to go by.".into());
        };
        if !membership.password_format.eq_ignore_ascii_case("Hashed") {
            return Err(format!(
                "This site keeps passwords '{}', not hashed - change the password in DNN (Persona Bar → Users).",
                membership.password_format
            ));
        }
        let Some(algorithm) = membership.hash_algorithm.as_deref().filter(|a| hasher.is_supported(a)) else {
            return Err("The site's web.config doesn't say how passwords are hashed - change the password in DNN (Persona Bar → Users).".into());
        };
        if let Some(problem) = membership.problem(new_password) {
            return Err(problem);
        }
        let secrets = Secrets::new(&[new_password, &database.password]);
        let salt = hasher.new_salt();
        let hash = hasher.hash(new_password, &salt, algorithm);
        let app = &membership.application_name;
        match self.sql.set_host_password(site_directory, database, app, user_name, &hash, &salt, cancel) {
            Ok(HostPasswordOutcome::Changed) => Ok(()),
            Ok(HostPasswordOutcome::NoDnnTables) => Err("The site's database has no DNN tables.".into()),
            Ok(HostPasswordOutcome::NotAHost) => Err(format!("'{user_name}' isn't a host account of this site.")),
            Ok(HostPasswordOutcome::NoMembershipUser) => {
                Err(format!("DNN's membership has no user '{user_name}' in application '{app}'."))
            }
            Err(_) if cancel.is_cancelled() => Err(CANCELLED.into()),
            Err(e) => Err(format!("Could not change the password: {}", secrets.hide(&first_line(&e)))),
        }
    }
}

enum Collected {
    Stop(Stop),
    Unreachable(String),
    BrokeOff(String),
}

/// A whole answer from `start_request`: status, where it redirects to, and its body.
fn collect(
    events: &mpsc::Receiver<Event>,
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<(u16, Option<Url>, String), Collected> {
    let mut head = None;
    let mut decoder = Decoder::default();
    let mut body = String::new();
    loop {
        if cancel.is_cancelled() {
            return Err(Collected::Stop(Stop::Cancelled));
        }
        if Instant::now() >= deadline {
            return Err(Collected::Stop(Stop::TooLong));
        }
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Head { status, location, .. }) => head = Some((status, location)),
            Ok(Event::Body(bytes)) => body.push_str(&decoder.push(&bytes)),
            Ok(Event::End) => {
                let (status, location) = head.unwrap_or((0, None));
                return Ok((status, location, body));
            }
            Ok(Event::Unreachable(e)) => return Err(Collected::Unreachable(e)),
            Ok(Event::BrokeOff(e)) => return Err(Collected::BrokeOff(e)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(Collected::BrokeOff("the connection closed".into())),
        }
    }
}

/// Runs DNN's installer page `path` (`mode=install` or `mode=upgrade`) and follows what it streams; `outcome` decides
/// from it how it went. `what`: "installation", "upgrade".
#[allow(clippy::too_many_arguments)]
fn run(
    site: &DnnSiteAddress,
    path: &str,
    what: &str,
    outcome: fn(&DnnInstallOutput) -> Result<(), String>,
    packages: usize,
    secrets: &Secrets,
    reporter: &mut dyn Reporter,
    progress: Progress,
    cancel: &CancelToken,
    raw_output_file: Option<&Path>,
    sql: &dyn InstallerSql,
) -> Result<(), String> {
    let mut raw = String::new();
    let result = run_page(site, path, what, outcome, packages, secrets, reporter, progress, cancel, &mut raw, sql);
    // Everything DNN answered, as it answered it (passwords left out) - for whoever needs to see where it stopped.
    if let Some(file) = raw_output_file {
        let _ = std::fs::write(file, secrets.hide(&raw));
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_page(
    site: &DnnSiteAddress,
    path: &str,
    what: &str,
    outcome: fn(&DnnInstallOutput) -> Result<(), String>,
    packages: usize,
    secrets: &Secrets,
    reporter: &mut dyn Reporter,
    progress: Progress,
    cancel: &CancelToken,
    raw: &mut String,
    sql: &dyn InstallerSql,
) -> Result<(), String> {
    let started = Instant::now();
    let started_utc = crate::log_files::now();
    let mut output = DnnInstallOutput::new();
    let mut installed = 0usize;
    let mut warned = false;
    let mut path = path.to_string();
    let mut report = |step: DnnInstallStep, reporter: &mut dyn Reporter, progress: &mut dyn FnMut(&str)| {
        let text = secrets.hide(&step.text);
        if step.succeeded == Some(false) {
            reporter.fail(&format!("DNN: {text}"));
        } else if starts_with(&text, "Installing Package File") {
            installed += 1;
            let of =
                if packages > 0 { format!("({} of {packages})…", installed.min(packages)) } else { "…".into() };
            progress(&format!("Running DNN {what}: installing {} {of}", after(&text, "Installing Package File")));
        } else if starts_with(&text, "Creating Site Alias") {
            reporter.info(&format!("Creating portal alias {}…", after(&text, "Creating Site Alias")));
        } else if starts_with(&text, "Creating Site") {
            reporter.info(&format!("Creating portal '{}'…", after(&text, "Creating Site")));
        } else if starts_with(&text, "Successfully Installed Site") {
            reporter.success("Portal created.");
        } else if starts_with(&text, "Installing DNN") {
            reporter.info("Running DNN installation…");
        } else if starts_with(&text, "Upgrade Complete") {
            reporter.success("DNN finished its upgrade.");
        } else if !starts_with(&text, "Installation Complete") {
            progress(&format!("Running DNN {what}: {text}…"));
        }
    };

    let mut request = 1;
    'requests: loop {
        if request > MAX_REDIRECTS {
            return Err("DNN's installer kept redirecting to itself.".into());
        }
        request += 1;
        let events = start_request(site, &path, INSTALL_LIMIT);
        let mut last_heard = Instant::now();
        let mut decoder = Decoder::default();
        let mut status = 0;
        let mut status_reason = String::new();
        let mut body = String::new();
        loop {
            // DNN writes a line per step; a long silence means it hangs (e.g. on a locked file).
            let stop = if cancel.is_cancelled() {
                Some(Stop::Cancelled)
            } else if started.elapsed() > INSTALL_LIMIT {
                Some(Stop::TooLong)
            } else if last_heard.elapsed() > QUIET_LIMIT {
                Some(Stop::Stuck)
            } else {
                None
            };
            match stop {
                Some(Stop::Cancelled) => return Err(CANCELLED.into()),
                Some(Stop::Stuck) => {
                    return Err(format!("DNN stopped reporting progress for 10 minutes - its {what} is stuck."));
                }
                Some(Stop::TooLong) => return Err(format!("DNN's {what} took longer than 30 minutes.")),
                None => {}
            }
            if last_heard.elapsed() > QUIET_WARNING && !warned {
                warned = true;
                reporter.warn(&format!("DNN hasn't reported progress for 2 minutes - still waiting for its {what}."));
            }
            let event = match events.recv_timeout(Duration::from_millis(500)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => Event::BrokeOff("the connection closed".into()),
            };
            last_heard = Instant::now();
            match event {
                Event::Head { status: code, reason, location } => {
                    status = code;
                    status_reason = reason.clone();
                    raw.push_str(&format!(
                        "<!-- GET {path} → HTTP {code} {reason} {} -->\r\n",
                        location.as_ref().map(|l| l.to_string()).unwrap_or_default()
                    ));
                    if (300..400).contains(&code) {
                        let lower = location.as_ref().map(|l| l.path().to_ascii_lowercase()).unwrap_or_default();
                        if lower == "/install/install.aspx" {
                            // DNN wrote new machine keys, restarts, and asks to be called again.
                            progress(&format!("Running DNN {what}: DNN prepares its configuration…"));
                            let target = location.expect("checked");
                            path = format!(
                                "{}{}",
                                target.path(),
                                target.query().map(|q| format!("?{q}")).unwrap_or_default()
                            );
                            continue 'requests;
                        }
                        if lower == "/install/upgradewizard.aspx" {
                            return Err("DNN wants its upgrade wizard instead (a host account signs in to it) - it doesn't upgrade unattended here.".into());
                        }
                        let to = location
                            .map(|l| format!("{}{}", l.path(), l.query().map(|q| format!("?{q}")).unwrap_or_default()))
                            .unwrap_or_else(|| "nowhere".into());
                        return Err(format!(
                            "DNN's installer sent the request to {to} - often a sign that the site may not write its own web.config."
                        ));
                    }
                }
                Event::Body(bytes) => {
                    let text = decoder.push(&bytes);
                    if status == 200 {
                        for step in output.add(&text) {
                            report(step, reporter, progress);
                        }
                    } else {
                        body.push_str(&text);
                    }
                }
                Event::End => {
                    if status != 200 {
                        // ASP.NET's error page says what went wrong.
                        raw.push_str(&body);
                        return Err(format!(
                            "DNN's installer answered HTTP {status} {status_reason}{}",
                            match error_of(&body) {
                                Some(error) => format!(": {error}"),
                                None => " - see the site's logs in Portals\\_default\\Logs.".into(),
                            }
                        ));
                    }
                    for step in output.finish() {
                        report(step, reporter, progress);
                    }
                    raw.push_str(output.body());
                    break 'requests;
                }
                Event::Unreachable(e) => {
                    return Err(format!("Could not reach the site at {}: {}", site.url(), first_line(&e)));
                }
                Event::BrokeOff(e) => {
                    raw.push_str(output.body());
                    return Err(format!("The connection to the site broke off during the {what}: {}", first_line(&e)));
                }
            }
        }
    }

    let result = outcome(&output);
    // DNN says only that it couldn't connect: what SQL Server answers its connection string, and what DNN logged.
    if result.is_err() && output.body().to_lowercase().contains("could not connect to database") {
        diagnose_connection(&site.directory, started_utc, secrets, reporter, sql);
    }
    result.map_err(|e| secrets.hide(&e))
}

/// When DNN couldn't connect to its database: the site's own connection string tried from here, and the errors DNN
/// logged since `since`, in its log4net or (DNN 10.4 on) Serilog files.
fn diagnose_connection(
    site_directory: &Path,
    since: DateTime<Utc>,
    secrets: &Secrets,
    reporter: &mut dyn Reporter,
    sql: &dyn InstallerSql,
) {
    if let Ok(site) = read_database_connection(&site_directory.join("web.config"))
        && site.kind != DatabaseKind::LocalDbFile
    {
        let who = if site.windows_authentication {
            "with Windows authentication".to_string()
        } else {
            format!("as {}", site.user)
        };
        match sql.try_connect(&site) {
                Ok(()) => reporter.warn(&format!(
                    "The site's connection string works from DNN Manager ([{}] on {}, {who}) - so it is the site's own process that can't reach it: its app pool's identity, or the address as it resolves there.",
                    site.database, site.server
                )),
                Err(e) => reporter.warn(&format!(
                    "The site's connection string ([{}] on {}, {who}) fails from DNN Manager too: {}",
                    site.database,
                    site.server,
                    secrets.hide(&first_line(&e))
                )),
            }
    }

    // What DNN logged meanwhile: log4net's [ERROR]/[FATAL], Serilog's [ERR]/[FTL].
    let logs = site_directory.join("Portals").join("_default").join("Logs");
    let since = since - chrono::Duration::minutes(1);
    let mut found = 0;
    for file in files_ending(&logs, "", "").into_iter().filter(|f| modified(f).is_some_and(|m| m >= since)) {
        let lines = read_shared(&file);
        for (i, line) in lines.iter().enumerate() {
            if !["[ERROR]", "[FATAL]", "[ERR]", "[FTL]"].iter().any(|m| line.contains(m)) {
                continue;
            }
            // The exception is on the lines after it, up to the next entry: its type and message say what failed.
            let detail = lines[i + 1..]
                .iter()
                .take_while(|l| !starts_with_date(l))
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with("at "))
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ");
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            reporter.warn(&format!(
                "DNN logged: {}{} (Portals\\_default\\Logs\\{name})",
                secrets.hide(&short(line)),
                if detail.is_empty() { String::new() } else { format!(" → {}", secrets.hide(&detail)) }
            ));
            found += 1;
            if found == 3 {
                return;
            }
        }
    }
}

/// What DNN logged as an error during the install or the first visit after it. Before its tables exist DNN always logs
/// a few errors about them - only what came after the install counts there.
fn log_problems(site_directory: &Path, install_started: DateTime<Utc>, opened_at: DateTime<Local>) -> Vec<String> {
    let mut problems = Vec::new();
    let logs = site_directory.join("Portals").join("_default").join("Logs");
    for file in files_ending(&logs, "InstallerLog", ".resources") {
        if let Some(error) = read_shared(&file).into_iter().find(|l| l.contains("[ERROR]")) {
            problems.push(format!(
                "DNN's installer logged an error: {} (Portals\\_default\\Logs\\{})",
                short(&error),
                file_name(&file)
            ));
        }
    }
    for file in files_ending(&logs, "", ".log.resources") {
        let first = read_shared(&file)
            .into_iter()
            .filter(|l| l.contains("[ERROR]") || l.contains("[FATAL]"))
            .find(|l| log_time(l).is_some_and(|at| at >= opened_at));
        if let Some(line) = first {
            problems.push(format!(
                "DNN logged an error on the first visit: {} (Portals\\_default\\Logs\\{})",
                short(&line),
                file_name(&file)
            ));
        }
    }
    // A database script that ran cleanly leaves an empty log behind.
    for (file, first) in script_problems(site_directory, install_started) {
        problems.push(format!(
            "A DNN database script reported a problem: {first} (Providers\\DataProviders\\SqlDataProvider\\{file})"
        ));
    }
    problems.truncate(5);
    problems
}

fn file_name(file: &Path) -> String {
    file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The install template holds the host password in plain text while DNN installs. The site's folder lets IIS_IUSRS read
/// it - every app pool on this computer: the template gets the rights of administrators, SYSTEM and this site's own app
/// pool only (the one IIS AppPool account that may change the site's folder). A site run as another account keeps the
/// folder's rights - its installer must read the template.
///
/// The C# sets the ACL through .NET's FileSecurity; the port reads and sets it with System32's icacls.exe (argument
/// list, no shell): the folder's own (not inherited) entries for `IIS APPPOOL\…` with Modify or Full control.
fn keep_to_own_app_pool(template: &Path, site_directory: &Path) {
    let Some(pool) = own_app_pool(site_directory) else { return };
    // Rights set on a link, or on a file with other names, would be set on another file: not done then.
    if has_link(site_directory, template, None) || is_hard_linked(template) {
        return;
    }
    let file = template.to_string_lossy().into_owned();
    let pool_grant = format!("{pool}:(F)");
    let _ = dnnm_infrastructure::process::run(
        dnnm_infrastructure::process::system32("icacls.exe"),
        &[&file, "/inheritance:r", "/grant:r", "*S-1-5-18:(F)", "*S-1-5-32-544:(F)", &pool_grant],
        Duration::from_secs(30),
    );
}

/// The one `IIS APPPOOL\…` account with its own Modify (or Full) entry on the folder; None when there isn't exactly one.
fn own_app_pool(site_directory: &Path) -> Option<String> {
    let folder = site_directory.to_string_lossy().into_owned();
    let output = dnnm_infrastructure::process::run(
        dnnm_infrastructure::process::system32("icacls.exe"),
        &[&folder],
        Duration::from_secs(30),
    )
    .ok()
    .filter(|o| o.success())?;
    own_app_pools(&output.stdout)
        .into_iter()
        .fold(None, |found: Option<Vec<String>>, pool| {
            let mut all = found.unwrap_or_default();
            if !all.iter().any(|p| p.eq_ignore_ascii_case(&pool)) {
                all.push(pool);
            }
            Some(all)
        })
        .filter(|all| all.len() == 1)
        .map(|mut all| all.remove(0))
}

/// The `IIS APPPOOL\…` accounts icacls lists with an own (not `(I)`nherited) Modify or Full control entry.
fn own_app_pools(icacls: &str) -> Vec<String> {
    let mut pools = Vec::new();
    for line in icacls.lines() {
        let Some(at) = line.to_ascii_uppercase().find("IIS APPPOOL\\") else { continue };
        let entry = &line[at..];
        let Some(colon) = entry.find(":(") else { continue };
        let (account, rights) = (&entry[..colon], &entry[colon + 1..]);
        if rights.contains("(I)") || !(rights.contains("(M)") || rights.contains("(F)")) {
            continue;
        }
        if !rights.contains("(DENY)") {
            pools.push(account.trim().to_string());
        }
    }
    pools
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoSql;

    impl InstallerSql for NoSql {
        fn raise_compatibility_level(&self, _: &Path, _: &DatabaseConnection, _: &CancelToken) -> Result<(), String> {
            Err("A network-related error\nmore".into())
        }
        fn version(
            &self,
            _: &Path,
            _: &DatabaseConnection,
            _: VersionOrder,
            _: &CancelToken,
        ) -> Result<DbVersion, String> {
            Ok(DbVersion::At(Some("10.2.5".into())))
        }
        fn portal_alias_count(
            &self,
            _: &Path,
            _: &DatabaseConnection,
            _: &str,
            _: &CancelToken,
        ) -> Result<i64, String> {
            Ok(0)
        }
        fn finish_host(&self, _: &Path, _: &DatabaseConnection, _: &str, _: &CancelToken) -> Result<i64, String> {
            Ok(1)
        }
        fn set_host_password(
            &self,
            _: &Path,
            _: &DatabaseConnection,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &CancelToken,
        ) -> Result<HostPasswordOutcome, String> {
            Err("Login failed for user 'sa' with password s3cret!pw".into())
        }
        fn try_connect(&self, _: &DatabaseConnection) -> Result<(), String> {
            Ok(())
        }
    }

    struct Sha;

    impl PasswordHasher for Sha {
        fn is_supported(&self, algorithm: &str) -> bool {
            algorithm == "SHA256"
        }
        fn new_salt(&self) -> String {
            "salt".into()
        }
        fn hash(&self, password: &str, salt: &str, _: &str) -> String {
            format!("{salt}{password}")
        }
    }

    fn database() -> DatabaseConnection {
        DatabaseConnection {
            kind: DatabaseKind::SqlServer,
            server: "localhost".into(),
            database: "dnn10".into(),
            windows_authentication: false,
            user: "sa".into(),
            password: "s3cret!pw".into(),
        }
    }

    #[test]
    fn secrets_are_hidden_in_every_form() {
        let secrets = Secrets::new(&["p&ss<wörd", "abc"]);
        assert_eq!(secrets.hide("x p&ss<wörd y"), "x *** y");
        assert_eq!(secrets.hide("p&amp;ss&lt;w&#246;rd"), "***");
        assert_eq!(secrets.hide("p%26ss%3Cw%C3%B6rd"), "***");
        assert_eq!(secrets.hide("abc"), "abc", "Shorter than 4 characters: not looked for.");
    }

    #[test]
    fn aspnets_error_page_title_says_what_went_wrong() {
        assert_eq!(
            error_of(
                "<html><title>Could not load file or assembly 'AngleSharp' or one\r\n of its dependencies.</title>"
            )
            .as_deref(),
            Some("Could not load file or assembly 'AngleSharp' or one of its dependencies.")
        );
        assert_eq!(error_of("<title>Runtime Error</title>"), None);
        assert_eq!(error_of("no title"), None);
    }

    #[test]
    fn a_package_that_isnt_fresh_is_refused() {
        let installer = Installer::new(NoSql);
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("s");
        std::fs::create_dir_all(site.join("Install")).unwrap();
        assert!(installer.check_package(&site).unwrap_err().contains("Install\\Install.aspx"));
        std::fs::write(site.join("Install").join("Install.aspx"), "").unwrap();
        assert!(installer.check_package(&site).unwrap_err().contains("DNN's install template"));
        std::fs::write(site.join("Install").join(install_template::SHIPPED_FILE_NAME), "").unwrap();
        installer.check_package(&site).unwrap();
        std::fs::write(
            site.join("web.config"),
            "<configuration><appSettings><add key=\"InstallVersion\" value=\"10\" /></appSettings></configuration>",
        )
        .unwrap();
        assert!(installer.check_package(&site).unwrap_err().contains("installed (or started installing) before"));
        std::fs::write(site.join("installBlocker.lock"), "").unwrap();
        assert!(installer.check_package(&site).unwrap_err().contains("installBlocker.lock"));
        let deep = dir.path().join("x".repeat(120));
        assert!(
            installer
                .check_package(&deep)
                .unwrap_err()
                .starts_with("The site's folder is too deep for DNN's installer")
        );
    }

    #[test]
    fn sql_failures_are_said_without_passwords() {
        let installer = Installer::new(NoSql);
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        assert_eq!(
            installer.prepare_local_db_file(dir.path(), &database(), &cancel).unwrap_err(),
            "Could not prepare App_Data\\dnn10: A network-related error"
        );
        std::fs::write(
            dir.path().join("web.config"),
            "<configuration><system.web><membership defaultProvider=\"AspNetSqlMembershipProvider\" hashAlgorithmType=\"SHA256\"><providers>\
             <add name=\"AspNetSqlMembershipProvider\" passwordFormat=\"Hashed\" minRequiredPasswordLength=\"7\" minRequiredNonalphanumericCharacters=\"1\" />\
             </providers></membership></system.web></configuration>",
        )
        .unwrap();
        assert_eq!(
            installer.change_host_password(dir.path(), &database(), "host", "abcdefgh", &Sha, &cancel).unwrap_err(),
            "The site wants at least 1 characters that aren't letters or digits."
        );
        assert_eq!(
            installer.change_host_password(dir.path(), &database(), "host", "abcdefg!", &Sha, &cancel).unwrap_err(),
            "Could not change the password: Login failed for user 'sa' with password ***"
        );
        // The files can't be read here (no bin\DotNetNuke.dll): the database's version counts.
        assert_eq!(installer.check_version(dir.path(), &database(), &cancel).unwrap(), "10.2.5");
        let site = DnnSiteAddress { directory: dir.path().into(), alias: "shop.dnndev.me".into(), port: 80 };
        let account = DnnAccount {
            user_name: "host".into(),
            password: "abcdefg".into(),
            email: "h@x.io".into(),
            website_name: "S".into(),
            language: "en-US".into(),
            template: "Default Website".into(),
        };
        assert_eq!(
            installer.complete(&site, &account, &database(), &cancel).unwrap_err(),
            "DNN didn't create the portal alias shop.dnndev.me."
        );
    }

    #[test]
    fn the_own_app_pool_is_read_from_icacls() {
        let output = "C:\\DNN\\shop BUILTIN\\Administrators:(OI)(CI)(F)\r\n\
                      \x20         IIS APPPOOL\\shop:(OI)(CI)(M)\r\n\
                      \x20         IIS APPPOOL\\other:(I)(OI)(CI)(M)\r\n\
                      \x20         IIS APPPOOL\\reader:(OI)(CI)(RX)\r\n\
                      \x20         BUILTIN\\IIS_IUSRS:(I)(OI)(CI)(RX)\r\n\r\nSuccessfully processed 1 files; Failed processing 0 files";
        assert_eq!(own_app_pools(output), ["IIS APPPOOL\\shop"]);
    }

    #[test]
    fn streamed_utf8_is_decoded_across_chunks() {
        let mut decoder = Decoder::default();
        let bytes = "ö-€".as_bytes();
        let mut text = decoder.push(&bytes[..1]);
        text += &decoder.push(&bytes[1..4]);
        text += &decoder.push(&bytes[4..]);
        assert_eq!(text, "ö-€");
        assert_eq!(Decoder::default().push(&[b'a', 0xff, b'b']), "a\u{fffd}b");
    }

    #[test]
    fn the_install_runs_against_a_local_stand_in_for_dnn() {
        // DNN's installer on this PC: first the redirect to itself, then the streamed page.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let mut hosts = Vec::new();
            for (i, stream) in listener.incoming().take(2).enumerate() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line.to_ascii_lowercase().starts_with("host:") {
                        hosts.push(line.trim().to_ascii_lowercase());
                    }
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let answer = if i == 0 {
                    "HTTP/1.1 302 Found\r\nLocation: /Install/Install.aspx?mode=upgrade&x=1\r\nContent-Length: 0\r\n\r\n".to_string()
                } else {
                    let body = "<h1>Upgrade Status Report</h1>00:00:01.000 - Upgrading DNN<br>00:00:59.000 - Upgrade Complete<br>";
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                };
                stream.write_all(answer.as_bytes()).unwrap();
            }
            hosts
        });
        let dir = tempfile::tempdir().unwrap();
        let site = DnnSiteAddress { directory: dir.path().into(), alias: "shop.dnndev.me".into(), port };
        std::fs::write(dir.path().join("installBlocker.lock"), "").unwrap();
        let raw = dir.path().join("raw.html");
        let mut lines = Vec::<String>::new();
        struct R<'a>(&'a mut Vec<String>);
        impl Reporter for R<'_> {
            fn step(&mut self, t: &str) {
                self.0.push(t.into());
            }
            fn info(&mut self, t: &str) {
                self.0.push(t.into());
            }
            fn success(&mut self, t: &str) {
                self.0.push(format!("ok {t}"));
            }
            fn warn(&mut self, t: &str) {
                self.0.push(format!("warn {t}"));
            }
            fn fail(&mut self, t: &str) {
                self.0.push(format!("! {t}"));
            }
        }
        let mut progress = Vec::new();
        Installer::new(NoSql)
            .upgrade(
                &site,
                "dbpass",
                &mut R(&mut lines),
                &mut |p| progress.push(p.to_string()),
                &CancelToken::new(),
                Some(&raw),
            )
            .unwrap();
        assert_eq!(lines, ["ok DNN finished its upgrade."]);
        assert!(progress.contains(&"Running DNN upgrade: DNN prepares its configuration…".to_string()), "{progress:?}");
        assert!(!dir.path().join("installBlocker.lock").exists(), "The lock left by a cut-off upgrade is removed.");
        assert!(std::fs::read_to_string(&raw).unwrap().contains("Upgrade Complete"));
        assert_eq!(server.join().unwrap(), ["host: shop.dnndev.me", "host: shop.dnndev.me"]);
    }
}
