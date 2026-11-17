//! What a site's database says, for its Details (`DatabaseInspector.cs`'s records) - and the trait the app implements
//! to read it, with the site's own web.config connection (its login, or Windows authentication as DNN Manager's
//! user), never DNN Manager's settings. Read-only: SELECTs and server properties. The queries, word for word as the
//! C# runs them, are in [`queries`]; this crate runs none of them (the SQL client is another crate's).

use std::collections::BTreeMap;

use dnnm_infrastructure::web_config::DatabaseConnection;

use crate::format::LocalDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqlServerFacts {
    pub instance: Option<String>,
    pub version: Option<String>,
    pub level: Option<String>,
    pub edition: Option<String>,
    pub collation: Option<String>,
    pub major_version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DatabaseFile {
    pub name: String,
    /// "ROWS", "LOG", "FILESTREAM"…
    pub kind: String,
    pub size_mb: f64,
    pub used_mb: Option<f64>,
    /// [`max_size_text`] of `max_size`.
    pub max_size: String,
    /// [`growth_text`] of `growth` and `is_percent_growth`.
    pub growth: String,
    pub physical_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DatabaseProperties {
    pub name: String,
    pub compatibility_level: Option<i32>,
    pub collation: Option<String>,
    pub recovery_model: Option<String>,
    pub state: Option<String>,
    pub read_only: Option<bool>,
    pub auto_shrink: Option<bool>,
    pub auto_close: Option<bool>,
    pub snapshot_isolation: Option<String>,
    pub read_committed_snapshot: Option<bool>,
    pub page_verify: Option<String>,
    pub created: Option<LocalDateTime>,
    pub files: Vec<DatabaseFile>,
    pub connection_encrypted: Option<String>,
    pub signed_in_as: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DnnPortalDetails {
    pub id: i32,
    pub name: String,
    pub default_language: Option<String>,
    pub home_directory: Option<String>,
    pub expired: bool,
    pub primary_alias: Option<String>,
    /// Alias and its culture.
    pub other_aliases: Vec<(String, Option<String>)>,
}

/// What DNN's own tables say - each part None when its table isn't there, or can't be read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DnnDatabaseFacts {
    /// The objectQualifier; None for none.
    pub qualifier: Option<String>,
    pub version: Option<String>,
    pub installed: Option<LocalDateTime>,
    pub upgraded: Option<LocalDateTime>,
    pub portals: Vec<DnnPortalDetails>,
    pub users: Option<i64>,
    pub roles: Option<i64>,
    pub schedule_items: Option<i64>,
    pub schedule_enabled: Option<i64>,
    pub event_log_records: Option<i64>,
    pub event_log_oldest: Option<LocalDateTime>,
    pub event_log_newest: Option<LocalDateTime>,
    pub desktop_modules: Option<i64>,
    /// Package type → how many.
    pub packages: BTreeMap<String, i64>,
    /// Authentication type and whether it is enabled.
    pub authentication_providers: Vec<(String, bool)>,
    /// The [`queries::HOST_SETTING_NAMES`] the database has, by name.
    pub host_settings: BTreeMap<String, String>,
}

impl DnnDatabaseFacts {
    pub fn host_setting(&self, name: &str) -> Option<&str> {
        self.host_settings.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DatabaseInspection {
    /// Why the database couldn't be reached at all; None when it was.
    pub problem: Option<String>,
    pub server: Option<SqlServerFacts>,
    pub database: Option<DatabaseProperties>,
    pub dnn: Option<DnnDatabaseFacts>,
    /// What couldn't be read of it (a permission, a table) - the rest is still there. Each as
    /// [`part_note`] words it.
    pub notes: Vec<String>,
}

/// Reads a site's database (`DatabaseInspector.InspectAsync`) - implemented by the app with its SQL client: connect
/// with `connection` as it is (application name "DNN Manager (Details)", an 8 s time-out), run [`queries`], and fill
/// in what each part could read. A connection that fails is `problem` (its first line); a part that fails is a note.
pub trait DatabaseInspector: Send + Sync {
    fn inspect(&self, connection: &DatabaseConnection) -> DatabaseInspection;
}

/// The note for a part that couldn't be read: "The portals couldn't be read: <first line of the error>".
pub fn part_note(what: &str, error: &str) -> String {
    let mut chars = what.chars();
    let what = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
    format!("{what} couldn't be read: {}", first_line(error))
}

pub fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or_default().trim()
}

/// sys.database_files' max_size (in 8 KB pages) in words.
pub fn max_size_text(max_pages: i32) -> String {
    match max_pages {
        -1 => "unlimited".into(),
        0 => "no growth".into(),
        268_435_456 => "2 TB".into(),
        pages => format!("{} MB", crate::format::group_thousands(f64::from(pages) * 8.0 / 1024.0, 0)),
    }
}

/// sys.database_files' growth in words: pages, or a percentage.
pub fn growth_text(growth: i32, is_percent: bool) -> String {
    if growth == 0 {
        "none".into()
    } else if is_percent {
        format!("{growth}%")
    } else {
        format!("{} MB", crate::format::group_thousands(f64::from(growth) * 8.0 / 1024.0, 0))
    }
}

/// The queries `DatabaseInspector` runs, in its order. `{Table}` in a DNN query is that table's name with the
/// objectQualifier found next to the Version table (`DnnTables.QualifierAsync`), as `DnnTables.Name` writes it:
/// `dbo.[dnn_Portals]` - a qualifier only of letters, digits and `_`.
pub mod queries {
    /// The host settings worth seeing while developing - never a secret one.
    pub const HOST_SETTING_NAMES: [&str; 12] = [
        "SchedulerMode",
        "PerformanceSetting",
        "UseFriendlyUrls",
        "AutoAddPortalAlias",
        "DebugMode",
        "MaxUploadSize",
        "UseCustomErrorMessages",
        "EventLogBuffer",
        "SMTPServer",
        "DisableUpgradeCheck",
        "FileExtensions",
        "CacheProvider",
    ];

    /// Part "the SQL Server's properties".
    pub const SERVER: &str = "SELECT CAST(SERVERPROPERTY('ServerName') AS nvarchar(256)), CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)), CAST(SERVERPROPERTY('ProductLevel') AS nvarchar(128)), CAST(SERVERPROPERTY('Edition') AS nvarchar(256)), CAST(SERVERPROPERTY('Collation') AS nvarchar(256)), CAST(SERVERPROPERTY('ProductMajorVersion') AS int)";
    /// Part "the database's properties".
    pub const DATABASE: &str = "SELECT name, compatibility_level, collation_name, recovery_model_desc, state_desc, is_read_only, is_auto_shrink_on, is_auto_close_on, snapshot_isolation_state_desc, is_read_committed_snapshot_on, page_verify_option_desc, create_date FROM sys.databases WHERE database_id = DB_ID()";
    /// Part "the database's files": max_size and growth through [`super::max_size_text`] / [`super::growth_text`].
    pub const FILES: &str = "SELECT name, type_desc, size * 8.0 / 1024, FILEPROPERTY(name, 'SpaceUsed') * 8.0 / 1024, max_size, growth, is_percent_growth, physical_name FROM sys.database_files";
    /// Not every login may look at its own connection (VIEW SERVER STATE) - then [`SIGNED_IN_AS`] alone.
    pub const CONNECTION: &str =
        "SELECT encrypt_option, SUSER_SNAME() FROM sys.dm_exec_connections WHERE session_id = @@SPID";
    pub const SIGNED_IN_AS: &str = "SELECT SUSER_SNAME()";
    /// Part "DNN's Version table": version, CreatedDate (upgraded), MIN(CreatedDate) (installed).
    pub const VERSION: &str = "SELECT TOP 1 CONCAT(Major, '.', Minor, '.', Build), CreatedDate, (SELECT MIN(CreatedDate) FROM {Version}) FROM {Version} ORDER BY Major DESC, Minor DESC, Build DESC";
    /// Parts "the users", "the roles", "the modules".
    pub const USERS: &str = "SELECT COUNT(*) FROM {Users}";
    pub const ROLES: &str = "SELECT COUNT(*) FROM {Roles}";
    pub const MODULES: &str = "SELECT COUNT(*) FROM {DesktopModules}";
    /// Part "the scheduler's jobs".
    pub const SCHEDULE: &str = "SELECT COUNT(*), SUM(CASE WHEN Enabled = 1 THEN 1 ELSE 0 END) FROM {Schedule}";
    /// Part "the event log".
    pub const EVENT_LOG: &str = "SELECT COUNT_BIG(*), MIN(LogCreateDate), MAX(LogCreateDate) FROM {EventLog}";
    /// Part "the installed extensions".
    pub const PACKAGES: &str = "SELECT PackageType, COUNT(*) FROM {Packages} GROUP BY PackageType";
    /// Part "the authentication providers".
    pub const AUTHENTICATION: &str =
        "SELECT AuthenticationType, IsEnabled FROM {Authentication} ORDER BY AuthenticationType";
    /// Part "the host settings": SettingName IN (@n0 … @n11) with [`HOST_SETTING_NAMES`] as the parameters.
    pub const HOST_SETTINGS: &str = "SELECT SettingName, SettingValue FROM {HostSettings} WHERE SettingName IN ";
    /// Part "the portals" (`PortalsAsync`): which columns this DNN has, then the portals, then their aliases.
    pub const PORTAL_COLUMNS: &str = "SELECT OBJECT_ID('{PortalLocalization}'), COL_LENGTH('{PortalAlias}', 'IsPrimary'), COL_LENGTH('{PortalAlias}', 'CultureCode')";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_files_and_notes_as_the_wpf_app() {
        assert_eq!(max_size_text(-1), "unlimited");
        assert_eq!(max_size_text(268_435_456), "2 TB");
        assert_eq!(max_size_text(1280), "10 MB");
        assert_eq!(growth_text(0, false), "none");
        assert_eq!(growth_text(10, true), "10%");
        assert_eq!(growth_text(8192, false), "64 MB");
        assert_eq!(
            part_note("the portals", "Invalid object name\nmore"),
            "The portals couldn't be read: Invalid object name"
        );
    }
}
