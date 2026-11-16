//! The Settings page's form, apart from its view (`Pages/SettingsPage.xaml.cs`): what each field holds, filled from
//! the saved settings (`ShowSettings`, `ShowServer`), copied back into them (`ApplyForm`), and one snapshot string that
//! tells whether anything differs from what is saved (`FormSnapshot`, `ServerSnapshot`) - a box ticked and unticked
//! again, or a connection type tried and left, leaves nothing to save.
//!
//! Passwords are held here only while the page is open; the type has no `Debug`, so none is ever printed.

use std::collections::BTreeMap;

use dnnm_application::settings::Settings;
use dnnm_sql::{DatabaseConnection, DatabaseKind, DockerOptions, SqlAuthentication};

use crate::setting_rules::{
    self as rules, Category, flag, normalize_path, password_problem, release_sources, set_flag, value,
};

/// The text fields (not the passwords), by what they edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    BaseDirectory,
    HostnameSuffix,
    SitePort,
    HostUsername,
    HostEmail,
    WebsiteName,
    PingPath,
    WarmUpPath,
    ContainerHost,
    ContainerPort,
    ContainerUser,
    ServerName,
    ServerUser,
    ContainerName,
    VolumeName,
    Edition,
    Collation,
}

impl Field {
    pub const ALL: [Field; 17] = [
        Field::BaseDirectory,
        Field::HostnameSuffix,
        Field::SitePort,
        Field::HostUsername,
        Field::HostEmail,
        Field::WebsiteName,
        Field::PingPath,
        Field::WarmUpPath,
        Field::ContainerHost,
        Field::ContainerPort,
        Field::ContainerUser,
        Field::ServerName,
        Field::ServerUser,
        Field::ContainerName,
        Field::VolumeName,
        Field::Edition,
        Field::Collation,
    ];
}

/// The password fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Secret {
    /// The default host password - in the Credential Manager.
    Host,
    /// The container login's password - the settings' `sqlServer.saPassword`, kept encrypted.
    Sa,
    /// The SQL Server login's password - in the Credential Manager.
    Server,
}

pub const CONTAINER: &str = "container";
pub const SQL_SERVER: &str = "sqlServer";
pub const LOCAL_DB_FILE: &str = "localDbFile";

/// The server a connection type starts from when none is saved for it (`DefaultServer`).
pub fn default_server(server_type: &str) -> &'static str {
    if server_type == LOCAL_DB_FILE { DatabaseConnection::LOCAL_DB_SERVER } else { r".\SQLEXPRESS" }
}

#[derive(Clone, PartialEq, Default)]
pub struct Form {
    // General
    pub keep_running: bool,
    pub check_updates: bool,
    pub ui_scale: u32,
    pub font_size: f64,
    pub animations: bool,
    pub shell: String,
    pub terminal_font: String,
    pub terminal_font_size: u32,
    // Projects
    pub base_directory: String,
    pub hostname_suffix: String,
    pub site_port: String,
    pub install_automatic: bool,
    pub host_username: String,
    pub host_password: String,
    pub host_email: String,
    pub website_name: String,
    pub language: String,
    pub template: String,
    pub ping_minutes: u32,
    pub ping_path: String,
    pub warm_up_path: String,
    pub keep_days: u32,
    // DNN releases
    pub release_sources: Vec<String>,
    pub keep_packages: bool,
    // Database server
    pub server_type: String,
    pub container_host: String,
    pub container_port: String,
    pub container_user: String,
    pub sa_password: String,
    pub server_name: String,
    pub sql_auth: bool,
    pub server_user: String,
    pub server_password: String,
    /// The server typed for SQL Server and for LocalDB - switching to one and back gives each its own again.
    pub server_text: BTreeMap<String, String>,
    pub ssms_remember: bool,
    // Docker container
    pub container_name: String,
    pub volume_name: String,
    pub edition: String,
    pub collation: String,
}

fn number<T: std::str::FromStr>(settings: &Settings, key: &str, fallback: T) -> T {
    value(settings, key).trim().parse().unwrap_or(fallback)
}

impl Form {
    /// The form for `settings` and the passwords that go with them (`ShowSettings`); `sa_password` decrypted.
    pub fn load(settings: &Settings, host_password: &str, server_password: &str, sa_password: &str) -> Form {
        let v = |key: &str| value(settings, key);
        let mut form = Form {
            keep_running: flag(settings, "window.keepRunningWhenClosed"),
            check_updates: flag(settings, "updates.checkAtStart"),
            ui_scale: number(settings, "appearance.uiScale", 100),
            font_size: number(settings, "appearance.fontSize", 13.0),
            animations: flag(settings, "appearance.animations"),
            shell: v("terminal.defaultShell"),
            terminal_font: v("terminal.fontFamily"),
            terminal_font_size: number(settings, "terminal.fontSize", 13),
            base_directory: v("projects.baseDirectory"),
            hostname_suffix: v("projects.hostnameSuffix"),
            site_port: v("projects.sitePort"),
            install_automatic: v("projects.dnnDefaults.installMode").eq_ignore_ascii_case("automatic"),
            host_username: v("projects.dnnDefaults.hostUsername"),
            host_password: host_password.to_string(),
            host_email: v("projects.dnnDefaults.hostEmail"),
            website_name: v("projects.dnnDefaults.websiteName"),
            language: v("projects.dnnDefaults.language"),
            template: v("projects.dnnDefaults.template"),
            ping_minutes: number(settings, "projects.keepWarm.pingMinutes", 5),
            ping_path: v("projects.keepWarm.pingPath"),
            warm_up_path: v("projects.keepWarm.warmUpPath"),
            keep_days: number(settings, "backups.keepDays", 0),
            release_sources: release_sources(settings),
            keep_packages: flag(settings, "projects.keepDnnPackages"),
            ssms_remember: flag(settings, "ssms.rememberPassword"),
            container_name: v("docker.containerName"),
            volume_name: v("docker.volumeName"),
            edition: v("docker.edition"),
            collation: v("docker.collation"),
            ..Form::default()
        };
        form.show_server(settings, server_password, sa_password);
        form
    }

    /// The database server's fields as `settings` have them - on loading, and after saving (`ShowServer`).
    pub fn show_server(&mut self, settings: &Settings, server_password: &str, sa_password: &str) {
        let v = |key: &str| value(settings, key);
        let saved_type = v("sqlServer.type");
        let server_type = [CONTAINER, SQL_SERVER, LOCAL_DB_FILE]
            .into_iter()
            .find(|t| t.eq_ignore_ascii_case(&saved_type))
            .unwrap_or(CONTAINER);
        let server = v("sqlServer.server");
        let user = v("sqlServer.userName");
        self.server_type = server_type.to_string();
        self.container_host = v("sqlServer.host");
        self.container_port = v("sqlServer.port");
        self.container_user = if user.trim().is_empty() { "sa".into() } else { user.trim().to_string() };
        self.sa_password = sa_password.to_string();
        // The saved server belongs to the type it fits; the other type starts from its default.
        let saved_for =
            if server.trim().to_ascii_lowercase().starts_with("(localdb)") { LOCAL_DB_FILE } else { SQL_SERVER };
        self.server_text = BTreeMap::from([(saved_for.to_string(), server.clone())]);
        self.server_name = if server_type == CONTAINER {
            server
        } else {
            self.server_text.get(server_type).cloned().unwrap_or_else(|| default_server(server_type).to_string())
        };
        self.sql_auth = v("sqlServer.authentication").eq_ignore_ascii_case("sql");
        self.server_user = user;
        self.server_password = server_password.to_string();
    }

    /// Another connection type chosen: SQL Server and LocalDB each keep the server typed for them (`ServerType_Checked`).
    pub fn set_server_type(&mut self, server_type: &str) {
        if self.server_type == server_type {
            return;
        }
        if self.server_type != CONTAINER {
            self.server_text.insert(self.server_type.clone(), self.server_name.clone());
        }
        if server_type != CONTAINER {
            self.server_name =
                self.server_text.get(server_type).cloned().unwrap_or_else(|| default_server(server_type).to_string());
        }
        self.server_type = server_type.to_string();
    }

    pub fn text(&self, field: Field) -> &str {
        match field {
            Field::BaseDirectory => &self.base_directory,
            Field::HostnameSuffix => &self.hostname_suffix,
            Field::SitePort => &self.site_port,
            Field::HostUsername => &self.host_username,
            Field::HostEmail => &self.host_email,
            Field::WebsiteName => &self.website_name,
            Field::PingPath => &self.ping_path,
            Field::WarmUpPath => &self.warm_up_path,
            Field::ContainerHost => &self.container_host,
            Field::ContainerPort => &self.container_port,
            Field::ContainerUser => &self.container_user,
            Field::ServerName => &self.server_name,
            Field::ServerUser => &self.server_user,
            Field::ContainerName => &self.container_name,
            Field::VolumeName => &self.volume_name,
            Field::Edition => &self.edition,
            Field::Collation => &self.collation,
        }
    }

    pub fn set_text(&mut self, field: Field, text: String) {
        let slot = match field {
            Field::BaseDirectory => &mut self.base_directory,
            Field::HostnameSuffix => &mut self.hostname_suffix,
            Field::SitePort => &mut self.site_port,
            Field::HostUsername => &mut self.host_username,
            Field::HostEmail => &mut self.host_email,
            Field::WebsiteName => &mut self.website_name,
            Field::PingPath => &mut self.ping_path,
            Field::WarmUpPath => &mut self.warm_up_path,
            Field::ContainerHost => &mut self.container_host,
            Field::ContainerPort => &mut self.container_port,
            Field::ContainerUser => &mut self.container_user,
            Field::ServerName => &mut self.server_name,
            Field::ServerUser => &mut self.server_user,
            Field::ContainerName => &mut self.container_name,
            Field::VolumeName => &mut self.volume_name,
            Field::Edition => &mut self.edition,
            Field::Collation => &mut self.collation,
        };
        *slot = text;
    }

    pub fn password(&self, secret: Secret) -> &str {
        match secret {
            Secret::Host => &self.host_password,
            Secret::Sa => &self.sa_password,
            Secret::Server => &self.server_password,
        }
    }

    pub fn set_password(&mut self, secret: Secret, password: String) {
        match secret {
            Secret::Host => self.host_password = password,
            Secret::Sa => self.sa_password = password,
            Secret::Server => self.server_password = password,
        }
    }

    fn trimmed_sources(&self) -> Vec<String> {
        self.release_sources.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    }

    /// The chosen connection type and only its fields (`ServerSnapshot`).
    fn server_snapshot(&self) -> String {
        let t = self.server_type.as_str();
        let parts: Vec<String> = match t {
            SQL_SERVER => vec![
                t.into(),
                self.server_name.trim().into(),
                self.sql_auth.to_string(),
                if self.sql_auth { self.server_user.trim().into() } else { String::new() },
                if self.sql_auth { self.server_password.clone() } else { String::new() },
            ],
            LOCAL_DB_FILE => vec![t.into(), self.server_name.trim().into()],
            _ => vec![
                t.into(),
                self.container_host.trim().into(),
                self.container_port.trim().into(),
                self.container_user.trim().into(),
                self.sa_password.clone(),
            ],
        };
        parts.join("\u{1e}")
    }

    /// Every value as it would be saved, in one string - to tell whether anything differs (`FormSnapshot`).
    pub fn snapshot(&self) -> String {
        [
            self.base_directory.trim().to_string(),
            self.site_port.trim().to_string(),
            self.hostname_suffix.trim().trim_matches('.').to_string(),
            self.trimmed_sources().join("\n"),
            self.keep_packages.to_string(),
            self.server_snapshot(),
            self.container_name.trim().to_string(),
            self.volume_name.trim().to_string(),
            self.edition.trim().to_string(),
            self.collation.trim().to_string(),
            self.ssms_remember.to_string(),
            self.ui_scale.to_string(),
            self.font_size.to_string(),
            self.animations.to_string(),
            self.keep_running.to_string(),
            self.check_updates.to_string(),
            self.shell.clone(),
            self.terminal_font.clone(),
            self.terminal_font_size.to_string(),
            self.install_automatic.to_string(),
            self.host_username.trim().to_string(),
            self.host_password.clone(),
            self.host_email.trim().to_string(),
            self.website_name.trim().to_string(),
            self.language.clone(),
            self.template.clone(),
            self.ping_minutes.to_string(),
            normalize_path(&self.ping_path),
            normalize_path(&self.warm_up_path),
            self.keep_days.to_string(),
        ]
        .join("\u{1f}")
    }

    /// Copies the form into `settings` - only the chosen connection type's fields; the others keep what is saved
    /// (`ApplyForm`). The sa password goes in as it is typed: the caller encrypts it. An error is the first value
    /// that isn't a number where one is needed (or a host password DNN would refuse), with its category.
    pub fn apply(&self, settings: &mut Settings) -> Result<(), (String, Category)> {
        let Ok(site_port) = self.site_port.trim().parse::<i32>() else {
            return Err(("Site port must be a number between 1 and 65535.".into(), Category::Projects));
        };
        let container = self.server_type == CONTAINER;
        let sql_port = if container {
            match self.container_port.trim().parse::<i32>() {
                Ok(port) => Some(port),
                Err(_) => return Err(("Port must be a number between 1 and 65535.".into(), Category::Sql)),
            }
        } else {
            None
        };
        if !self.host_password.is_empty()
            && let Some(problem) = password_problem(&self.host_password)
        {
            return Err((format!("Host password: {problem}"), Category::Projects));
        }

        let s = settings;
        s.set("projects.baseDirectory", self.base_directory.trim());
        s.set("projects.sitePort", site_port.to_string());
        s.set("projects.hostnameSuffix", self.hostname_suffix.trim().trim_matches('.'));
        s.set_list("projects.dnnReleaseSources", &self.trimmed_sources());
        set_flag(s, "projects.keepDnnPackages", self.keep_packages);
        s.set("projects.keepWarm.pingMinutes", self.ping_minutes.to_string());
        s.set("projects.keepWarm.pingPath", normalize_path(&self.ping_path));
        s.set("projects.keepWarm.warmUpPath", normalize_path(&self.warm_up_path));

        s.set("sqlServer.type", self.server_type.clone());
        if let Some(port) = sql_port {
            s.set("sqlServer.host", self.container_host.trim());
            s.set("sqlServer.port", port.to_string());
            // The container's login is the same setting as the SQL Server login; empty is sa.
            s.set("sqlServer.userName", self.container_user.trim());
            s.set("sqlServer.saPassword", self.sa_password.clone());
        } else {
            s.set("sqlServer.server", self.server_name.trim());
        }
        if self.server_type == SQL_SERVER {
            s.set("sqlServer.authentication", if self.sql_auth { "sql" } else { "windows" });
            if self.sql_auth {
                s.set("sqlServer.userName", self.server_user.trim());
            }
        }

        s.set("docker.containerName", self.container_name.trim());
        s.set("docker.volumeName", self.volume_name.trim());
        s.set("docker.edition", self.edition.trim());
        s.set("docker.collation", self.collation.trim());

        s.set("projects.dnnDefaults.installMode", if self.install_automatic { "automatic" } else { "manual" });
        s.set("projects.dnnDefaults.hostUsername", self.host_username.trim());
        s.set("projects.dnnDefaults.hostEmail", self.host_email.trim());
        s.set("projects.dnnDefaults.websiteName", self.website_name.trim());
        s.set("projects.dnnDefaults.language", self.language.clone());
        s.set("projects.dnnDefaults.template", self.template.clone());

        set_flag(s, "ssms.rememberPassword", self.ssms_remember);
        s.set("appearance.uiScale", self.ui_scale.to_string());
        s.set("appearance.fontSize", self.font_size.to_string());
        set_flag(s, "appearance.animations", self.animations);
        set_flag(s, "window.keepRunningWhenClosed", self.keep_running);
        set_flag(s, "updates.checkAtStart", self.check_updates);
        s.set("terminal.defaultShell", self.shell.clone());
        s.set("terminal.fontFamily", self.terminal_font.clone());
        s.set("terminal.fontSize", self.terminal_font_size.to_string());
        s.set("backups.keepDays", self.keep_days.to_string());
        Ok(())
    }

    /// "Sites answer at http://<project>.dnndev.me" - the URL preview under Site address.
    pub fn url_preview(&self) -> String {
        let suffix = self.hostname_suffix.trim().trim_matches('.');
        let port = self.site_port.trim().parse::<i32>().ok().filter(|p| (1..=65535).contains(p)).unwrap_or(80);
        let port = if port == 80 { String::new() } else { format!(":{port}") };
        format!("Sites answer at http://<project>.{suffix}{port}")
    }

    /// DNN defaults' hint (`ShowDefaultsHint`).
    pub fn defaults_hint(&self) -> String {
        let suffix = self.hostname_suffix.trim().trim_matches('.');
        let language = if self.language.is_empty() { "en-US" } else { self.language.as_str() };
        let extra = if language == "en-US" {
            String::new()
        } else {
            format!(" {} needs internet while installing.", rules::language_name(language))
        };
        format!("Empty e-mail: host@{suffix}. Empty password: New project asks.{extra}")
    }

    /// The hint under the server field (`ShowServerFields`).
    pub fn server_hint(&self) -> String {
        if self.server_type == SQL_SERVER {
            r"The instance, e.g. .\SQLEXPRESS, localhost or localhost,1433.".into()
        } else {
            format!(
                "{} is the instance every Windows user has. Each site runs its database in the LocalDB of its app pool identity.",
                DatabaseConnection::LOCAL_DB_SERVER
            )
        }
    }

    pub fn auth_hint(&self) -> &'static str {
        if self.sql_auth {
            "The login must be able to create a database, or own it when it exists. Its password is kept in the Windows Credential Manager of your account, not with the other settings."
        } else {
            r"Each site signs in as its app pool's identity (IIS APPPOOL\<project>) - DNN Manager creates that login with your Windows account and makes it the database's owner."
        }
    }
}

/// The settings sign in to SQL Server with a login whose password is in the Credential Manager (`UsesLogin`).
pub fn uses_login(settings: &Settings) -> bool {
    value(settings, "sqlServer.type").eq_ignore_ascii_case(SQL_SERVER)
        && value(settings, "sqlServer.authentication").eq_ignore_ascii_case("sql")
}

/// The container's login: `sqlServer.userName`, or sa.
pub fn container_user(settings: &Settings) -> String {
    let user = value(settings, "sqlServer.userName");
    if user.trim().is_empty() { "sa".into() } else { user.trim().to_string() }
}

/// The server new projects get their database on, as the saved settings say (`AppOptions.DatabaseOnServer("")`) -
/// `sa_password` decrypted, `server_password` from the Credential Manager.
pub fn database_on_server(settings: &Settings, sa_password: &str, server_password: &str) -> DatabaseConnection {
    let kind = value(settings, "sqlServer.type");
    let server = value(settings, "sqlServer.server").trim().to_string();
    if kind.eq_ignore_ascii_case(CONTAINER) {
        let address = format!("{},{}", value(settings, "sqlServer.host"), value(settings, "sqlServer.port"));
        DatabaseConnection::new(DatabaseKind::Container, &address, "", SqlAuthentication::Sql)
            .with_login(&container_user(settings), sa_password)
    } else if kind.eq_ignore_ascii_case(LOCAL_DB_FILE) {
        DatabaseConnection::new(
            DatabaseKind::LocalDbFile,
            &server,
            DatabaseConnection::LOCAL_DB_FILE_NAME,
            SqlAuthentication::Windows,
        )
    } else if uses_login(settings) {
        DatabaseConnection::new(DatabaseKind::SqlServer, &server, "", SqlAuthentication::Sql)
            .with_login(value(settings, "sqlServer.userName").trim(), server_password)
    } else {
        DatabaseConnection::new(DatabaseKind::SqlServer, &server, "", SqlAuthentication::Windows)
    }
}

/// The container's options as the saved settings say (`AppOptions.Docker`) - `sa_password` decrypted.
pub fn docker_options(settings: &Settings, sa_password: &str) -> DockerOptions {
    DockerOptions {
        container_name: value(settings, "docker.containerName"),
        container_ip: value(settings, "sqlServer.host"),
        volume_name: value(settings, "docker.volumeName"),
        sql_user: container_user(settings),
        sa_password: sa_password.to_string(),
        default_port: value(settings, "sqlServer.port").trim().parse().unwrap_or(1433),
        collation: value(settings, "docker.collation"),
        mssql_pid: value(settings, "docker.edition"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Settings {
        let mut s = Settings::default();
        s.set("sqlServer.saPassword", "dpapi:x");
        s
    }

    #[test]
    fn the_form_of_the_saved_settings_saves_them_unchanged() {
        let s = saved();
        let form = Form::load(&s, "", "", "Sa-Secret-1");
        let mut after = s.clone();
        form.apply(&mut after).unwrap();
        // What the defaults are, written out - and the sa password as typed, for the caller to encrypt.
        assert_eq!(value(&after, "projects.sitePort"), "80");
        assert_eq!(value(&after, "sqlServer.saPassword"), "Sa-Secret-1");
        assert_eq!(value(&after, "sqlServer.userName"), "sa");
        assert_eq!(release_sources(&after), rules::DEFAULT_RELEASE_SOURCES.map(String::from).to_vec());
        let again = Form::load(&after, "", "", "Sa-Secret-1");
        assert!(again.snapshot() == form.snapshot());
    }

    #[test]
    fn values_are_trimmed_as_they_are_saved() {
        let mut form = Form::load(&saved(), "", "", "x");
        form.hostname_suffix = " .dnndev.me. ".into();
        form.ping_path = " KeepAlive.aspx".into();
        form.release_sources = vec![" https://a/r ".into(), " ".into()];
        let mut s = saved();
        form.apply(&mut s).unwrap();
        assert_eq!(value(&s, "projects.hostnameSuffix"), "dnndev.me");
        assert_eq!(value(&s, "projects.keepWarm.pingPath"), "/KeepAlive.aspx");
        assert_eq!(release_sources(&s), vec!["https://a/r".to_string()]);
    }

    #[test]
    fn numbers_and_the_host_password_are_checked_first() {
        let mut form = Form::load(&saved(), "", "", "x");
        form.site_port = "eighty".into();
        assert_eq!(
            form.apply(&mut saved()).unwrap_err(),
            ("Site port must be a number between 1 and 65535.".to_string(), Category::Projects)
        );
        form.site_port = "80".into();
        form.container_port = "".into();
        assert_eq!(form.apply(&mut saved()).unwrap_err().1, Category::Sql);
        form.container_port = "1433".into();
        form.host_password = "short".into();
        assert_eq!(form.apply(&mut saved()).unwrap_err().0, "Host password: The password needs at least 7 characters.");
    }

    #[test]
    fn trying_another_connection_type_and_going_back_leaves_nothing_to_save() {
        let s = saved();
        let form = Form::load(&s, "", "", "x");
        let mut tried = form.clone();
        tried.set_server_type(LOCAL_DB_FILE);
        assert_eq!(tried.server_name, DatabaseConnection::LOCAL_DB_SERVER);
        assert_ne!(tried.snapshot(), form.snapshot());
        tried.set_server_type(SQL_SERVER);
        assert_eq!(tried.server_name, r".\SQLEXPRESS");
        tried.server_name = "db1".into();
        tried.set_server_type(LOCAL_DB_FILE);
        tried.set_server_type(SQL_SERVER);
        assert_eq!(tried.server_name, "db1", "each type keeps the server typed for it");
        tried.set_server_type(CONTAINER);
        assert_eq!(tried.snapshot(), form.snapshot());
    }

    #[test]
    fn only_the_chosen_types_fields_are_saved() {
        let mut s = saved();
        s.set("sqlServer.host", "keep-me");
        let mut form = Form::load(&s, "", "", "x");
        form.set_server_type(SQL_SERVER);
        form.sql_auth = true;
        form.server_user = " dnn ".into();
        form.container_host = "changed".into();
        form.apply(&mut s).unwrap();
        assert_eq!(value(&s, "sqlServer.type"), "sqlServer");
        assert_eq!(value(&s, "sqlServer.authentication"), "sql");
        assert_eq!(value(&s, "sqlServer.userName"), "dnn");
        assert_eq!(value(&s, "sqlServer.host"), "keep-me");
        assert_eq!(value(&s, "sqlServer.saPassword"), "dpapi:x", "not the container: its password stays as saved");
        assert!(uses_login(&s));
        let c = database_on_server(&s, "", "pw");
        assert_eq!((c.kind, c.server.as_str(), c.user.as_str()), (DatabaseKind::SqlServer, r".\SQLEXPRESS", "dnn"));
    }

    #[test]
    fn the_container_is_reached_on_its_host_and_port() {
        let s = saved();
        let c = database_on_server(&s, "pw", "");
        assert_eq!((c.kind, c.server.as_str(), c.user.as_str()), (DatabaseKind::Container, "localhost,1433", "sa"));
        let d = docker_options(&s, "pw");
        assert_eq!(
            (d.container_name.as_str(), d.default_port, d.mssql_pid.as_str()),
            ("dnn-sqlserver", 1433, "Developer")
        );
    }

    #[test]
    fn hints_follow_the_form() {
        let mut form = Form::load(&saved(), "", "", "x");
        assert_eq!(form.url_preview(), "Sites answer at http://<project>.dnndev.me");
        form.site_port = "8080".into();
        assert_eq!(form.url_preview(), "Sites answer at http://<project>.dnndev.me:8080");
        assert_eq!(form.defaults_hint(), "Empty e-mail: host@dnndev.me. Empty password: New project asks.");
        form.language = "nl-NL".into();
        assert!(form.defaults_hint().ends_with(" Nederlands (Nederland) (nl-NL) needs internet while installing."));
    }
}
