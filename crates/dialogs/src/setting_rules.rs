//! The rules of the settings the Settings page edits - `Configuration/UserSettings.cs` (`Validate`, the defaults),
//! `Configuration/SettingRules.cs`, `Abstractions/DnnInstall.cs`'s `DnnAccountRules` and the keep-warm page rules - and
//! the settings rows the page reads and writes: lists (`key[i]`), lists of records (`key[i].name`) and the shortcuts'
//! dictionary (`keyboard.shortcuts{id}`), as `Data/ValueRows.cs` lays them out.
//!
//! Ported here because `dnnm-application`'s `Settings` only knows the rows the app reads so far; the projects folder's
//! rule is `dnnm_system::files::folder_guard::projects_folder_problem`, already ported there.

use std::collections::BTreeMap;

use dnnm_application::settings::Settings;
use dnnm_dnn::upgrades::{DnnUpgradeStep, UpgradeStop, VERSIONS, upgrade_stop_version, upgrade_stops_problem};
use dnnm_system::files::folder_guard::projects_folder_problem;
use dnnm_system::prereq::{IisFeature, default_features, is_iis_feature_name};

/// A value that isn't allowed: `key` is its settings key, e.g. `projects.sitePort` (`SettingsProblem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsProblem {
    pub key: String,
    pub message: String,
}

/// The categories of the Settings page, as the tags of its list (`General`, `Projects`, `Releases`, `Sql`, `Docker`,
/// `Iis`, `Keyboard`, `Help`, `About`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    General,
    Projects,
    Releases,
    Sql,
    Docker,
    Iis,
    Keyboard,
    Help,
    About,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::General,
        Category::Projects,
        Category::Releases,
        Category::Sql,
        Category::Docker,
        Category::Iis,
        Category::Keyboard,
        Category::Help,
        Category::About,
    ];

    /// The tag the WPF app names it by (`SettingsPage.Category`).
    pub fn tag(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Projects => "Projects",
            Category::Releases => "Releases",
            Category::Sql => "Sql",
            Category::Docker => "Docker",
            Category::Iis => "Iis",
            Category::Keyboard => "Keyboard",
            Category::Help => "Help",
            Category::About => "About",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.tag().eq_ignore_ascii_case(tag))
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Projects => "Projects",
            Category::Releases => "DNN releases",
            Category::Sql => "Database server",
            Category::Docker => "Docker container",
            Category::Iis => "IIS",
            Category::Keyboard => "Keyboard shortcuts",
            Category::Help => "Help",
            Category::About => "About",
        }
    }

    /// The words the search box finds it by (`SettingsPage._categories`), without the lists added at run time (the
    /// IIS features, the commands).
    pub fn keywords(self) -> &'static str {
        match self {
            Category::General => {
                "general start sign in startup close closing quit exit background tray notification area keep running appearance scale zoom ui font text size bigger smaller animations animation motion slide fade reduce effects terminal shell powershell command prompt git bash font family size"
            }
            Category::Projects => {
                "projects folder base directory hostname suffix site port address url dnn defaults install installation automatic manual setup wizard host account username password e-mail email website name language culture template keep warm alive keepalive warm-up idle time-out timeout interval ping cold start slow fast recycle backups backup deployments deployment packages clone copy delete old days retention keep onedrive"
            }
            Category::Releases => {
                "dnn releases repositories github versions install packages keep download upgrade path stops steps"
            }
            Category::Sql => {
                "database server sql server express localdb file connection type local container docker host port user username sa password windows authentication login username ssms management studio remember test connection"
            }
            Category::Docker => {
                "docker container name volume edition mssql_pid collation desktop engine install start set up docker-compose compose yml test"
            }
            Category::Iis => "iis windows features test set up enable",
            Category::Keyboard => {
                "keyboard shortcuts shortcut keys key bindings keybindings hotkeys accelerators command palette"
            }
            Category::Help => {
                "help getting started guide welcome tutorial onboarding introduction beginner tour walkthrough what's new whats new changes user guide documentation docs manual f1"
            }
            Category::About => {
                "about version build commit release channel update upgrade install new latest license mit repository github documentation docs runtime .net framework windows architecture administrator iis docker components libraries your files folders settings backups logs packages"
            }
        }
    }
}

/// Whether every word of `search` is in `keywords`, ignoring case (`Search_TextChanged`).
pub fn matches_all_words(search: &str, keywords: &str) -> bool {
    let keywords = keywords.to_lowercase();
    search.split_whitespace().all(|w| keywords.contains(&w.to_lowercase()))
}

// ─── Defaults (`UserSettings`) ──────────────────────────────────────────────

/// The default of every scalar setting the page reads - a missing row has it.
const DEFAULTS: &[(&str, &str)] = &[
    ("projects.baseDirectory", r"C:\DNN"),
    ("projects.hostnameSuffix", "dnndev.me"),
    ("projects.sitePort", "80"),
    ("projects.keepDnnPackages", "false"),
    ("projects.dnnDefaults.installMode", "automatic"),
    ("projects.dnnDefaults.hostUsername", "host"),
    ("projects.dnnDefaults.hostEmail", "admin@admin.com"),
    ("projects.dnnDefaults.websiteName", "My Website"),
    ("projects.dnnDefaults.language", "en-US"),
    ("projects.dnnDefaults.template", "Default Website"),
    ("projects.keepWarm.pingMinutes", "5"),
    ("projects.keepWarm.warmUpPath", "/"),
    ("projects.keepWarm.pingPath", "/KeepAlive.aspx"),
    ("sqlServer.type", "container"),
    ("sqlServer.host", "localhost"),
    ("sqlServer.port", "1433"),
    ("sqlServer.saPassword", ""),
    ("sqlServer.server", r".\SQLEXPRESS"),
    ("sqlServer.authentication", "windows"),
    ("sqlServer.userName", ""),
    ("docker.containerName", "dnn-sqlserver"),
    ("docker.volumeName", "dnn_sqlserver_data"),
    ("docker.edition", "Developer"),
    ("docker.collation", "Latin1_General_CI_AS"),
    ("ssms.rememberPassword", "false"),
    ("appearance.theme", "system"),
    ("appearance.uiScale", "100"),
    ("appearance.fontSize", "13"),
    ("appearance.animations", "true"),
    ("terminal.defaultShell", "powershell"),
    ("terminal.fontFamily", ""),
    ("terminal.fontSize", "13"),
    ("window.keepRunningWhenClosed", "true"),
    ("updates.checkAtStart", "true"),
    ("layout.sidebarPosition", "left"),
    ("layout.panelAlignment", "center"),
    ("layout.menuBarVisible", "true"),
    ("layout.statusBarVisible", "true"),
    ("layout.quickInputPosition", "top"),
    ("layout.density", "default"),
    ("backups.keepDays", "0"),
];

/// `ProjectSettings.DnnReleaseSources`' default.
pub const DEFAULT_RELEASE_SOURCES: [&str; 2] = [
    "https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases",
    "https://api.github.com/repos/DNN-Connect/Dnn.Platform/releases",
];

/// The setting's value: its row, or its default ("" for a key without one).
pub fn value(settings: &Settings, key: &str) -> String {
    settings
        .rows()
        .get(key)
        .cloned()
        .unwrap_or_else(|| DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()).unwrap_or_default())
}

/// A true/false setting, as `bool.Parse` reads it; false for anything else.
pub fn flag(settings: &Settings, key: &str) -> bool {
    value(settings, key).trim().eq_ignore_ascii_case("true")
}

fn bool_text(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

pub fn set_flag(settings: &mut Settings, key: &str, on: bool) {
    settings.set(key, bool_text(on));
}

fn int(settings: &Settings, key: &str) -> Option<i64> {
    value(settings, key).trim().parse().ok()
}

/// The DNN release sources: the list, or its default without one.
pub fn release_sources(settings: &Settings) -> Vec<String> {
    settings
        .list("projects.dnnReleaseSources")
        .unwrap_or_else(|| DEFAULT_RELEASE_SOURCES.iter().map(|s| s.to_string()).collect())
}

/// The edits of DNN's upgrade path (`projects.dnnUpgradeStops[i].version` …).
pub fn upgrade_stops(settings: &Settings) -> Vec<UpgradeStop> {
    let key = "projects.dnnUpgradeStops";
    let count: usize = settings.rows().get(key).and_then(|c| c.parse().ok()).unwrap_or(0);
    let rows = settings.rows();
    (0..count)
        .map(|i| {
            let field = |name: &str| rows.get(&format!("{key}[{i}].{name}")).cloned().unwrap_or_default();
            UpgradeStop {
                version: field("version"),
                net_framework_release: field("netFrameworkRelease").trim().parse().unwrap_or(0),
                sql_server_major: field("sqlServerMajor").trim().parse().unwrap_or(0),
                note: field("note"),
                replaces_listed: field("replacesListed").trim().eq_ignore_ascii_case("true"),
                removed: field("removed").trim().eq_ignore_ascii_case("true"),
            }
        })
        .collect()
}

/// The IIS features Settings → IIS checks: `iis.requiredFeatures`, or the defaults without the list.
pub fn iis_features(settings: &Settings) -> Vec<IisFeature> {
    let key = "iis.requiredFeatures";
    let Some(count) = settings.rows().get(key).and_then(|c| c.parse::<usize>().ok()) else { return default_features() };
    let rows = settings.rows();
    (0..count)
        .map(|i| IisFeature {
            name: rows.get(&format!("{key}[{i}].name")).cloned().unwrap_or_default(),
            label: rows.get(&format!("{key}[{i}].label")).cloned().unwrap_or_default(),
        })
        .collect()
}

/// Replaces `iis.requiredFeatures`' rows - an item that is gone goes.
pub fn set_iis_features(settings: &mut Settings, features: &[IisFeature]) {
    let key = "iis.requiredFeatures";
    remove_items(settings, key);
    settings.set(key, features.len().to_string());
    for (i, f) in features.iter().enumerate() {
        settings.set(&format!("{key}[{i}].name"), f.name.clone());
        settings.set(&format!("{key}[{i}].label"), f.label.clone());
    }
}

/// The keyboard shortcuts changed from their defaults, by command id (`keyboard.shortcuts{id}`); "" takes one away.
pub fn custom_shortcuts(settings: &Settings) -> BTreeMap<String, String> {
    let start = "keyboard.shortcuts{";
    settings
        .rows()
        .iter()
        .filter_map(|(k, v)| {
            let rest = k.strip_prefix(start)?;
            let end = rest.find('}')?;
            // A row of an entry's own value (`{id}.x`) isn't an entry.
            (end == rest.len() - 1).then(|| (unescape_data(&rest[..end]), v.clone()))
        })
        .collect()
}

/// Replaces `keyboard.shortcuts`' rows with `shortcuts`.
pub fn set_custom_shortcuts(settings: &mut Settings, shortcuts: &BTreeMap<String, String>) {
    let key = "keyboard.shortcuts";
    remove_items(settings, key);
    settings.set(key, shortcuts.len().to_string());
    for (id, shortcut) in shortcuts {
        settings.set(&format!("{key}{{{}}}", escape_data(id)), shortcut.clone());
    }
}

/// Removes the rows of a list's or dictionary's items (`key[…`, `key{…`), keeping the key's own row.
fn remove_items(settings: &mut Settings, key: &str) {
    let kept: Vec<(String, String)> = settings
        .rows()
        .iter()
        .filter(|(k, _)| !(k.len() > key.len() && k.starts_with(key) && matches!(k.as_bytes()[key.len()], b'[' | b'{')))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    *settings = Settings::from_rows(kept);
}

/// `Uri.EscapeDataString`: every byte but `A-Z a-z 0-9 - _ . ~` as `%XX`.
pub fn escape_data(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// `Uri.UnescapeDataString`: `%XX` back to its byte; anything else as it is.
pub fn unescape_data(text: &str) -> String {
    String::from_utf8_lossy(&percent_decode(text)).into_owned()
}

// ─── The rules (`SettingRules`, `DnnAccountRules`, `KeepWarmSettings`) ───────

/// `SettingRules.IsReleaseSource`: https, or http only to this computer.
pub fn is_release_source(source: &str) -> bool {
    let lower = source.trim().to_ascii_lowercase();
    if source.trim() != source || source.chars().any(char::is_whitespace) {
        return false;
    }
    let (https, rest) = match (lower.strip_prefix("https://"), lower.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (_, Some(rest)) => (false, rest),
        _ => return false,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        format!("[{}", v6.split(']').next().unwrap_or_default()) + "]"
    } else {
        host_port.split(':').next().unwrap_or_default().to_string()
    };
    if host.is_empty() {
        return false;
    }
    https || host == "localhost" || host == "[::1]" || host.starts_with("127.")
}

/// `SettingRules.IsCollation`: letters, digits and `_` only.
pub fn is_collation(collation: &str) -> bool {
    !collation.trim().is_empty() && collation.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `SettingRules.IsHostnameSuffix`: RFC 1123 labels between dots, at most 200 characters, not an IP address.
pub fn is_hostname_suffix(suffix: &str) -> bool {
    let s = suffix.trim().trim_matches('.');
    if suffix.trim().is_empty() || s.is_empty() || s.len() > 200 {
        return false;
    }
    let labels: Vec<&str> = s.split('.').collect();
    labels.iter().all(|label| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) && !labels.last().is_some_and(|l| l.chars().all(|c| c.is_ascii_digit()))
}

/// `SettingRules.IsDockerName`: a letter or digit, then letters, digits, `_`, `.` and `-`; at most 128.
pub fn is_docker_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// `DnnAccountRules.Languages`: the cultures DNN's installer offers.
pub const LANGUAGES: [&str; 6] = ["en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "nl-NL"];
/// `DnnAccountRules.Templates`.
pub const TEMPLATES: [&str; 2] = ["Default Website", "Blank Website"];

/// `Languages.Name`: the language's own name and its code - .NET's native names, written out (no culture data here).
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

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub fn user_name_problem(user_name: &str) -> Option<&'static str> {
    if user_name.trim().is_empty() {
        return Some("Enter the host account's user name.");
    }
    if utf16_len(user_name) > 100 {
        return Some("The user name can have at most 100 characters.");
    }
    (!user_name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '@')))
        .then_some("The user name can only have letters, digits and . _ - @.")
}

/// `DnnAccountRules.PasswordProblem` - never says the password.
pub fn password_problem(password: &str) -> Option<&'static str> {
    let length = utf16_len(password);
    if length < 7 {
        Some("The password needs at least 7 characters.")
    } else if length > 128 {
        Some("The password can have at most 128 characters.")
    } else if password.contains('<') || password.contains("&#") {
        Some("The password can't contain < or &# - DNN's login form refuses them.")
    } else {
        None
    }
}

pub fn email_problem(email: &str) -> Option<&'static str> {
    let ok = match email.find('@') {
        Some(at) => {
            at > 0
                && at < email.len() - 1
                && !email[at + 1..].contains('@')
                && !email.chars().any(char::is_whitespace)
                && email[at + 1..].contains('.')
        }
        None => false,
    };
    (!ok).then_some("Enter an e-mail address, e.g. host@example.com.")
}

/// `KeepWarmSettings.NormalizePath`: trimmed, starting with a `/`; empty stays empty.
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') { trimmed.to_string() } else { format!("/{trimmed}") }
}

/// `KeepWarmSettings.IsInstallerPath`: a folder named Install anywhere in the path, however it is written (escapes
/// inside escapes, trailing dots or spaces Windows ignores), or a path that can't be read.
pub fn is_installer_path(path_and_query: &str) -> bool {
    let path = path_and_query.split(['?', '#']).next().unwrap_or_default().replace('\\', "/");
    let mut path = path;
    for _ in 0..4 {
        let bytes = percent_decode(&path);
        let Ok(unescaped) = String::from_utf8(bytes) else { return true };
        if unescaped == path {
            break;
        }
        path = unescaped.replace('\\', "/");
    }
    path.split('/').any(|segment| segment.trim_end_matches(['.', ' ']).eq_ignore_ascii_case("install"))
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// `KeepWarmSettings.PathProblem`: what is wrong with a page to request on the site; None when it can be requested.
pub fn keep_warm_path_problem(path: &str) -> Option<&'static str> {
    let p = normalize_path(path);
    if p.is_empty() {
        return Some("is required, e.g. /KeepAlive.aspx.");
    }
    if utf16_len(&p) > 2000 {
        return Some("can have at most 2000 characters.");
    }
    if p.starts_with("//") || p.contains("://") {
        return Some("must be a page of the site, e.g. /KeepAlive.aspx - not an address.");
    }
    if p.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("can't contain spaces.");
    }
    if p.contains('\\') {
        return Some("must use / between folders.");
    }
    if p.contains('#') {
        return Some("can't contain #.");
    }
    if p.split('?').next().unwrap_or_default().split('/').any(|s| s == "." || s == "..") {
        return Some("can't contain . or .. folders.");
    }
    if is_installer_path(&p) || p.to_ascii_lowercase().contains("mode=") {
        return Some("can't be a page of DNN's installer - requesting it can install or upgrade DNN.");
    }
    None
}

/// `KeepWarmSettings.PingIntervals`.
pub const PING_INTERVALS: [u32; 8] = [1, 2, 5, 10, 15, 20, 30, 60];

/// The keep-warm interval as the list shows it: "5 minutes (default)" (`KeepWarmRules.Span`).
pub fn ping_interval_text(minutes: u32) -> String {
    let span = if minutes == 1 { "1 minute".to_string() } else { format!("{minutes} minutes") };
    if minutes == 5 { format!("{span} (default)") } else { span }
}

/// `BackupSettings.KeepDayChoices`.
pub const KEEP_DAY_CHOICES: [u32; 8] = [0, 7, 14, 30, 60, 90, 180, 365];

/// How Backups' list words a number of days (`BackupKeepDaysItem`).
pub fn keep_days_text(days: u32) -> String {
    match days {
        0 => "Keep them for good (default)".into(),
        7 => "Delete them after a week".into(),
        30 => "Delete them after a month".into(),
        365 => "Delete them after a year".into(),
        d if d % 30 == 0 => format!("Delete them after {} months", d / 30),
        d => format!("Delete them after {d} days"),
    }
}

/// `AppearanceSettings.UiScales` and `FontSizes`.
pub const UI_SCALES: [u32; 7] = [80, 90, 100, 110, 125, 150, 175];
pub const FONT_SIZES: [u32; 8] = [11, 12, 13, 14, 15, 16, 17, 18];
/// The terminal's font sizes Settings offers: 9 to 20, 22, 24, 28.
pub fn terminal_font_sizes() -> Vec<u32> {
    (9..=20).chain([22, 24, 28]).collect()
}

/// The hint over "Edit upgrade path…" (`ShowUpgradePath`).
pub fn upgrade_path_hint(stops: &[UpgradeStop]) -> String {
    let known = VERSIONS.last().map(DnnUpgradeStep::name).unwrap_or_default();
    let edits: Vec<(&UpgradeStop, _)> =
        stops.iter().filter_map(|s| upgrade_stop_version(&s.version).map(|v| (s, v))).collect();
    let part = |what: &str, which: &dyn Fn(&UpgradeStop, &dnnm_dnn::version::Version) -> bool| {
        let names: Vec<String> =
            edits.iter().filter(|(s, v)| which(s, v)).map(|(_, v)| DnnUpgradeStep::name(v)).collect();
        (!names.is_empty()).then(|| format!("{what} {}", names.join(", ")))
    };
    let parts: Vec<String> = [
        part("added", &|s, v| !s.removed && !VERSIONS.contains(v)),
        part("changed", &|s, v| !s.removed && VERSIONS.contains(v)),
        part("taken off", &|s, _| s.removed),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        format!(
            "Upgrade DNN goes through DNN's own path, known up to DNN {known}. Edit it when DNN lists a new version, or export it to use on another PC."
        )
    } else {
        format!(
            "Upgrade DNN goes through DNN's path, known up to DNN {known}, with your changes: {}.",
            parts.join("; ")
        )
    }
}

// ─── Validate (`UserSettings.Validate`) ──────────────────────────────────────

const SQL_TYPES: [&str; 3] = ["container", "sqlServer", "localDbFile"];
const AUTHENTICATIONS: [&str; 2] = ["windows", "sql"];
const THEMES: [&str; 3] = ["system", "light", "dark"];
const SIDEBAR_POSITIONS: [&str; 2] = ["left", "right"];
const PANEL_ALIGNMENTS: [&str; 4] = ["left", "right", "center", "justify"];
const QUICK_INPUT_POSITIONS: [&str; 2] = ["top", "center"];
const DENSITIES: [&str; 2] = ["default", "compact"];
const INSTALL_MODES: [&str; 2] = ["automatic", "manual"];
pub const MAX_KEEP_DAYS: i64 = 3650;

fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|a| a.eq_ignore_ascii_case(value))
}

/// The values that aren't allowed, each with its key; empty when the settings are usable.
pub fn validate(settings: &Settings) -> Vec<SettingsProblem> {
    let mut problems = Vec::new();
    let mut check = |ok: bool, key: &str, message: String| {
        if !ok {
            problems.push(SettingsProblem { key: key.into(), message });
        }
    };
    let v = |key: &str| value(settings, key);
    let is_port = |key: &str| int(settings, key).is_some_and(|p| (1..=65535).contains(&p));
    let has = |key: &str| !v(key).trim().is_empty();

    let folder_problem = projects_folder_problem(&v("projects.baseDirectory"));
    check(folder_problem.is_none(), "projects.baseDirectory", folder_problem.unwrap_or_default());
    check(is_port("projects.sitePort"), "projects.sitePort", "must be a number between 1 and 65535.".into());
    check(
        is_hostname_suffix(&v("projects.hostnameSuffix")),
        "projects.hostnameSuffix",
        "is required and must be a host name - letters, digits, dots and hyphens, e.g. dnndev.me.".into(),
    );
    let sources = release_sources(settings);
    check(!sources.is_empty(), "projects.dnnReleaseSources", "needs at least one URL.".into());
    for source in &sources {
        check(
            is_release_source(source),
            "projects.dnnReleaseSources",
            format!("has a URL that isn't https (http only for this computer): {source}"),
        );
    }
    let stops_problem = upgrade_stops_problem(&upgrade_stops(settings));
    check(
        stops_problem.is_none(),
        "projects.dnnUpgradeStops",
        format!("has a version that can't be used: {}", stops_problem.unwrap_or_default()),
    );

    check(
        one_of(&v("projects.dnnDefaults.installMode"), &INSTALL_MODES),
        "projects.dnnDefaults.installMode",
        format!("must be one of: {}.", INSTALL_MODES.join(", ")),
    );
    let user_problem = user_name_problem(&v("projects.dnnDefaults.hostUsername"));
    check(user_problem.is_none(), "projects.dnnDefaults.hostUsername", user_problem.unwrap_or_default().into());
    let email = v("projects.dnnDefaults.hostEmail");
    check(
        email.is_empty() || email_problem(&email).is_none(),
        "projects.dnnDefaults.hostEmail",
        "must be an e-mail address, or empty for host@ plus the hostname suffix.".into(),
    );
    check(
        utf16_len(&v("projects.dnnDefaults.websiteName")) <= 128,
        "projects.dnnDefaults.websiteName",
        "can have at most 128 characters.".into(),
    );
    check(
        LANGUAGES.contains(&v("projects.dnnDefaults.language").as_str()),
        "projects.dnnDefaults.language",
        format!("must be one of: {}.", LANGUAGES.join(", ")),
    );
    check(
        TEMPLATES.contains(&v("projects.dnnDefaults.template").as_str()),
        "projects.dnnDefaults.template",
        format!("must be one of: {}.", TEMPLATES.join(", ")),
    );

    check(
        int(settings, "projects.keepWarm.pingMinutes").is_some_and(|m| (1..=60).contains(&m)),
        "projects.keepWarm.pingMinutes",
        "must be between 1 and 60 minutes.".into(),
    );
    for key in ["projects.keepWarm.warmUpPath", "projects.keepWarm.pingPath"] {
        let problem = keep_warm_path_problem(&v(key));
        check(problem.is_none(), key, problem.unwrap_or_default().into());
    }

    let sql_type = v("sqlServer.type");
    check(one_of(&sql_type, &SQL_TYPES), "sqlServer.type", format!("must be one of: {}.", SQL_TYPES.join(", ")));
    if !sql_type.eq_ignore_ascii_case("container") {
        check(has("sqlServer.server"), "sqlServer.server", "is required.".into());
    }
    if sql_type.eq_ignore_ascii_case("sqlServer") {
        let authentication = v("sqlServer.authentication");
        check(
            one_of(&authentication, &AUTHENTICATIONS),
            "sqlServer.authentication",
            format!("must be one of: {}.", AUTHENTICATIONS.join(", ")),
        );
        check(
            !authentication.eq_ignore_ascii_case("sql") || has("sqlServer.userName"),
            "sqlServer.userName",
            "is required for SQL Server authentication.".into(),
        );
    }
    check(has("sqlServer.host"), "sqlServer.host", "is required.".into());
    check(is_port("sqlServer.port"), "sqlServer.port", "must be a number between 1 and 65535.".into());
    check(!v("sqlServer.saPassword").is_empty(), "sqlServer.saPassword", "is required.".into());
    check(
        is_docker_name(&v("docker.containerName")),
        "docker.containerName",
        "is required and must be a Docker name - a letter or digit, then letters, digits, '_', '.' and '-', e.g. dnn-sqlserver.".into(),
    );
    check(
        is_docker_name(&v("docker.volumeName")),
        "docker.volumeName",
        "is required and must be a Docker name - a letter or digit, then letters, digits, '_', '.' and '-', e.g. dnn_sqlserver_data.".into(),
    );
    check(has("docker.edition"), "docker.edition", "is required.".into());
    check(
        is_collation(&v("docker.collation")),
        "docker.collation",
        "must be a collation name such as Latin1_General_CI_AS (letters, digits and _).".into(),
    );

    for feature in iis_features(settings) {
        check(
            is_iis_feature_name(&feature.name),
            "iis.requiredFeatures",
            format!(
                "has a feature whose name isn't a Windows feature name (letters, digits, '-', '_' and '.'): {}",
                feature.name
            ),
        );
    }

    check(
        one_of(&v("appearance.theme"), &THEMES),
        "appearance.theme",
        format!("must be one of: {}.", THEMES.join(", ")),
    );
    check(
        int(settings, "appearance.uiScale").is_some_and(|s| (50..=200).contains(&s)),
        "appearance.uiScale",
        "must be a percentage between 50 and 200.".into(),
    );
    check(
        v("appearance.fontSize").trim().parse::<f64>().is_ok_and(|s| (8.0..=32.0).contains(&s)),
        "appearance.fontSize",
        "must be a number between 8 and 32.".into(),
    );
    check(
        int(settings, "terminal.fontSize").is_some_and(|s| (8..=32).contains(&s)),
        "terminal.fontSize",
        "must be a number between 8 and 32.".into(),
    );
    for (key, allowed) in [
        ("layout.sidebarPosition", &SIDEBAR_POSITIONS[..]),
        ("layout.panelAlignment", &PANEL_ALIGNMENTS[..]),
        ("layout.quickInputPosition", &QUICK_INPUT_POSITIONS[..]),
        ("layout.density", &DENSITIES[..]),
    ] {
        check(one_of(&v(key), allowed), key, format!("must be one of: {}.", allowed.join(", ")));
    }
    check(
        int(settings, "backups.keepDays").is_some_and(|d| (0..=MAX_KEEP_DAYS).contains(&d)),
        "backups.keepDays",
        format!("must be a number of days between 1 and {MAX_KEEP_DAYS}, or 0 to keep them for good."),
    );
    problems
}

/// The form's label of a settings key, so a problem reads "Site port must be…" (`SettingsPage.Labels`).
pub fn label_of(key: &str) -> &str {
    match key {
        "appearance.uiScale" => "UI scale",
        "appearance.fontSize" => "Font size",
        "projects.baseDirectory" => "Projects folder",
        "projects.sitePort" => "Site port",
        "projects.hostnameSuffix" => "Hostname suffix",
        "projects.dnnReleaseSources" => "DNN repositories",
        "projects.dnnUpgradeStops" => "Upgrade path",
        "projects.dnnDefaults.installMode" => "Installation",
        "projects.dnnDefaults.hostUsername" => "Host username:",
        "projects.dnnDefaults.hostEmail" => "Host e-mail",
        "projects.dnnDefaults.websiteName" => "Website name",
        "projects.dnnDefaults.language" => "Language",
        "projects.dnnDefaults.template" => "Site template",
        "projects.keepWarm.pingMinutes" => "Keep warm interval",
        "projects.keepWarm.warmUpPath" => "Warm-up page",
        "projects.keepWarm.pingPath" => "Keep-alive page",
        "sqlServer.type" => "Connection type",
        "sqlServer.server" => "Server",
        "sqlServer.authentication" => "Authentication",
        "sqlServer.userName" => "Username",
        "sqlServer.host" => "Server host",
        "sqlServer.port" => "Port",
        "sqlServer.saPassword" => "Password",
        "docker.containerName" => "Container name",
        "docker.volumeName" => "Volume name",
        "docker.edition" => "Edition",
        "docker.collation" => "Collation",
        "backups.keepDays" => "Keep backups",
        other => other,
    }
}

/// The category that edits `key` (`SettingsPage.CategoryOf`).
pub fn category_of(key: &str) -> Option<Category> {
    if key == "projects.dnnReleaseSources" || key == "projects.dnnUpgradeStops" {
        return Some(Category::Releases);
    }
    let starts = |p: &str| key.starts_with(p);
    if starts("terminal.") || starts("appearance.") || starts("window.") {
        Some(Category::General)
    } else if starts("projects.") || starts("backups.") {
        Some(Category::Projects)
    } else if starts("sqlServer.") {
        Some(Category::Sql)
    } else if starts("docker.") {
        Some(Category::Docker)
    } else if starts("iis.") {
        Some(Category::Iis)
    } else {
        None
    }
}

/// The first problem as the page shows it - "Site port must be…" - with the category it is in.
pub fn first_problem(settings: &Settings) -> Option<(String, Option<Category>)> {
    validate(settings).into_iter().next().map(|p| (format!("{} {}", label_of(&p.key), p.message), category_of(&p.key)))
}

/// The banner's text when `DNNMANAGER_*` variables are set (`ShowEnvironmentOverrides`); None without them.
pub fn environment_overrides(names: impl IntoIterator<Item = String>) -> Option<String> {
    let mut overrides: Vec<String> =
        names.into_iter().filter(|k| k.len() >= 11 && k[..11].eq_ignore_ascii_case("DNNMANAGER_")).collect();
    if overrides.is_empty() {
        return None;
    }
    overrides.sort_by_key(|k| k.to_lowercase());
    Some(format!("These environment variables override the saved settings: {}", overrides.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(rows: &[(&str, &str)]) -> Settings {
        let mut s = Settings::from_rows(rows.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        if !s.rows().contains_key("sqlServer.saPassword") {
            s.set("sqlServer.saPassword", "dpapi:x");
        }
        if !s.rows().contains_key("projects.baseDirectory") {
            s.set("projects.baseDirectory", r"C:\DNN-tests");
        }
        s
    }

    fn keys(s: &Settings) -> Vec<String> {
        validate(s).into_iter().map(|p| p.key).collect()
    }

    #[test]
    fn the_defaults_are_allowed() {
        assert_eq!(validate(&settings(&[])), vec![]);
    }

    #[test]
    fn each_rule_names_its_key() {
        let s = settings(&[
            ("projects.sitePort", "0"),
            ("projects.hostnameSuffix", "1.2.3.4"),
            ("projects.dnnReleaseSources", "1"),
            ("projects.dnnReleaseSources[0]", "http://example.com/releases"),
            ("projects.dnnDefaults.hostUsername", "bad name"),
            ("projects.dnnDefaults.hostEmail", "nope"),
            ("projects.keepWarm.pingMinutes", "61"),
            ("projects.keepWarm.pingPath", "/Install/install.aspx"),
            ("sqlServer.port", "abc"),
            ("docker.containerName", "-x"),
            ("docker.collation", "Latin1 General"),
            ("appearance.uiScale", "300"),
            ("layout.density", "tight"),
            ("backups.keepDays", "4000"),
        ]);
        assert_eq!(
            keys(&s),
            vec![
                "projects.sitePort",
                "projects.hostnameSuffix",
                "projects.dnnReleaseSources",
                "projects.dnnDefaults.hostUsername",
                "projects.dnnDefaults.hostEmail",
                "projects.keepWarm.pingMinutes",
                "projects.keepWarm.pingPath",
                "sqlServer.port",
                "docker.containerName",
                "docker.collation",
                "appearance.uiScale",
                "layout.density",
                "backups.keepDays",
            ]
        );
    }

    #[test]
    fn the_first_problem_reads_as_the_page_says_it() {
        let s = settings(&[("projects.sitePort", "70000")]);
        assert_eq!(
            first_problem(&s),
            Some(("Site port must be a number between 1 and 65535.".into(), Some(Category::Projects)))
        );
        let s = settings(&[("sqlServer.type", "sqlServer"), ("sqlServer.authentication", "sql")]);
        assert_eq!(
            first_problem(&s),
            Some(("Username is required for SQL Server authentication.".into(), Some(Category::Sql)))
        );
    }

    #[test]
    fn a_container_needs_no_server_but_another_type_does() {
        assert!(keys(&settings(&[("sqlServer.server", "")])).is_empty());
        assert_eq!(
            keys(&settings(&[("sqlServer.type", "localDbFile"), ("sqlServer.server", " ")])),
            vec!["sqlServer.server"]
        );
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn projects_folder_rules_come_from_the_folder_guard() {
        let s = settings(&[("projects.baseDirectory", r"C:\")]);
        assert_eq!(
            first_problem(&s).unwrap().0,
            r"Projects folder can't be a drive - use a folder of its own, e.g. C:\DNN."
        );
        assert_eq!(first_problem(&settings(&[("projects.baseDirectory", "DNN")])).unwrap().0, {
            r"Projects folder must be a full path, e.g. C:\DNN."
        });
    }

    #[test]
    fn hostname_suffix_rules() {
        assert!(is_hostname_suffix("dnndev.me"));
        assert!(is_hostname_suffix(" .dnndev.me. "));
        assert!(!is_hostname_suffix("dnn_dev.me"));
        assert!(!is_hostname_suffix("-a.me"));
        assert!(!is_hostname_suffix("10.0.0.1"));
        assert!(!is_hostname_suffix(""));
        assert!(!is_hostname_suffix(&"a".repeat(64)));
    }

    #[test]
    fn release_sources_are_https_or_local_http() {
        assert!(is_release_source("https://api.github.com/repos/x/y/releases"));
        assert!(is_release_source("http://localhost:8080/releases"));
        assert!(is_release_source("http://127.0.0.1/r"));
        assert!(is_release_source("http://[::1]:5000/r"));
        assert!(!is_release_source("http://example.com/r"));
        assert!(!is_release_source("ftp://x/r"));
        assert!(!is_release_source("https://"));
        assert!(!is_release_source("not a url"));
    }

    #[test]
    fn docker_names_and_collations() {
        assert!(is_docker_name("dnn-sqlserver") && is_docker_name("dnn_sqlserver_data") && is_docker_name("a.b"));
        assert!(!is_docker_name("") && !is_docker_name("_x") && !is_docker_name("a b") && !is_docker_name("a;b"));
        assert!(!is_docker_name(&"a".repeat(129)));
        assert!(is_collation("Latin1_General_CI_AS"));
        assert!(!is_collation("Latin1_General_CI_AS; DROP") && !is_collation(" "));
    }

    #[test]
    fn keep_warm_pages_are_pages_of_the_site_never_the_installer() {
        assert_eq!(normalize_path(" KeepAlive.aspx "), "/KeepAlive.aspx");
        assert_eq!(keep_warm_path_problem("/KeepAlive.aspx"), None);
        assert_eq!(keep_warm_path_problem("/"), None);
        assert_eq!(keep_warm_path_problem(""), Some("is required, e.g. /KeepAlive.aspx."));
        assert!(keep_warm_path_problem("//evil.com/x").unwrap().contains("not an address"));
        assert!(keep_warm_path_problem("http://x/y").unwrap().contains("not an address"));
        assert_eq!(keep_warm_path_problem("/a b"), Some("can't contain spaces."));
        assert_eq!(keep_warm_path_problem(r"/a\b"), Some("must use / between folders."));
        assert_eq!(keep_warm_path_problem("/a#b"), Some("can't contain #."));
        assert_eq!(keep_warm_path_problem("/a/../b"), Some("can't contain . or .. folders."));
        for installer in
            ["/Install/Install.aspx", "/%49nstall/x", "/%2549nstall/x", "/install./x", "/Default.aspx?mode=wizard"]
        {
            assert_eq!(
                keep_warm_path_problem(installer),
                Some("can't be a page of DNN's installer - requesting it can install or upgrade DNN."),
                "{installer}"
            );
        }
        assert!(!is_installer_path("/Installer.aspx"));
    }

    #[test]
    fn account_rules() {
        assert_eq!(user_name_problem("host"), None);
        assert_eq!(user_name_problem("a.b_c-d@e"), None);
        assert_eq!(user_name_problem(" "), Some("Enter the host account's user name."));
        assert_eq!(password_problem("short"), Some("The password needs at least 7 characters."));
        assert_eq!(
            password_problem("abc<defgh"),
            Some("The password can't contain < or &# - DNN's login form refuses them.")
        );
        assert_eq!(password_problem("Secret-123"), None);
        assert_eq!(email_problem("host@dnndev.me"), None);
        assert!(
            email_problem("host@dnndev").is_some()
                && email_problem("@x.y").is_some()
                && email_problem("a b@x.y").is_some()
        );
    }

    #[test]
    fn lists_records_and_the_shortcuts_dictionary_round_trip() {
        let mut s = settings(&[("keyboard.shortcuts", "1"), ("keyboard.shortcuts{old}", "F2"), ("x.y", "kept")]);
        let map = BTreeMap::from([
            ("project.start".to_string(), "Ctrl+F5".to_string()),
            ("view.close".to_string(), String::new()),
            ("a b{c}".to_string(), "Alt+1".to_string()),
        ]);
        set_custom_shortcuts(&mut s, &map);
        assert_eq!(custom_shortcuts(&s), map);
        assert_eq!(s.rows()["keyboard.shortcuts"], "3");
        assert!(s.rows().contains_key("keyboard.shortcuts{a%20b%7Bc%7D}"));
        assert!(!s.rows().contains_key("keyboard.shortcuts{old}"));
        assert_eq!(s.rows()["x.y"], "kept");

        assert_eq!(iis_features(&s).len(), 16, "no list: the defaults");
        set_iis_features(&mut s, &[IisFeature { name: "IIS-WebServer".into(), label: "Web".into() }]);
        assert_eq!(iis_features(&s), vec![IisFeature { name: "IIS-WebServer".into(), label: "Web".into() }]);
        assert!(!s.rows().contains_key("iis.requiredFeatures[1].name"));
    }

    #[test]
    fn upgrade_stops_are_read_from_their_rows() {
        let s = settings(&[
            ("projects.dnnUpgradeStops", "1"),
            ("projects.dnnUpgradeStops[0].version", "10.4.2"),
            ("projects.dnnUpgradeStops[0].netFrameworkRelease", "528040"),
            ("projects.dnnUpgradeStops[0].removed", "False"),
        ]);
        let stops = upgrade_stops(&s);
        assert_eq!(stops[0].version, "10.4.2");
        assert_eq!(stops[0].net_framework_release, 528040);
        assert!(!stops[0].removed);
        assert!(upgrade_path_hint(&[]).starts_with("Upgrade DNN goes through DNN's own path, known up to DNN "));
        let removed = UpgradeStop { version: VERSIONS[3].to_string(), removed: true, ..Default::default() };
        assert!(upgrade_path_hint(&[removed]).contains("with your changes: taken off "));
    }

    #[test]
    fn list_texts() {
        assert_eq!(ping_interval_text(5), "5 minutes (default)");
        assert_eq!(ping_interval_text(1), "1 minute");
        assert_eq!(keep_days_text(0), "Keep them for good (default)");
        assert_eq!(keep_days_text(90), "Delete them after 3 months");
        assert_eq!(keep_days_text(14), "Delete them after 14 days");
        assert_eq!(language_name("de-DE"), "Deutsch (Deutschland) (de-DE)");
    }

    #[test]
    fn categories_are_found_by_every_word() {
        assert!(matches_all_words("Docker  PORT", Category::Sql.keywords()));
        assert!(!matches_all_words("docker tour", Category::Sql.keywords()));
        assert!(matches_all_words("", Category::About.keywords()));
        assert_eq!(category_of("terminal.fontSize"), Some(Category::General));
        assert_eq!(category_of("projects.dnnUpgradeStops"), Some(Category::Releases));
        assert_eq!(category_of("layout.density"), None);
    }

    #[test]
    fn environment_overrides_are_listed_in_order() {
        let names = ["PATH", "dnnmanager_B", "DNNMANAGER_a"].map(String::from);
        assert_eq!(
            environment_overrides(names).as_deref(),
            Some("These environment variables override the saved settings: DNNMANAGER_a, dnnmanager_B")
        );
        assert_eq!(environment_overrides(["PATH".to_string()]), None);
    }

    #[test]
    fn escaping_matches_uri_escape_data_string() {
        assert_eq!(escape_data("project.start"), "project.start");
        assert_eq!(escape_data("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(unescape_data("a%20b%2F%C3%A9"), "a b/é");
        assert_eq!(unescape_data("100%"), "100%");
    }
}
