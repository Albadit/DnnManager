//! The user's settings: the rows of `dnnmanager.db`'s `settings` table, shared with the WPF app. Each setting is a
//! row whose key is its path in camelCase (`appearance.theme`); a list is a count row plus `key[i]` rows. Keys this
//! build doesn't know are kept as they are, so both apps can use the same database.

use std::collections::BTreeMap;

/// The settings version this build writes (`UserSettings.CurrentVersion`). A database with a higher one is newer than
/// this build, and its settings aren't used.
pub const CURRENT_VERSION: u32 = 3;

/// Light, dark or as Windows is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

/// The sidebar's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarPosition {
    Left,
    Right,
}

/// Customize Layout's density: cards with gaps (Default) or flush with lines (Compact).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Default,
    Compact,
}

/// Where the bottom panel reaches (`layout.panelAlignment`): under the page only (Center), under the sidebar too
/// (Justify), or toward the side named (Left / Right) - VS Code's panel alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAlignment {
    Center,
    Justify,
    Left,
    Right,
}

impl PanelAlignment {
    pub fn key(self) -> &'static str {
        match self {
            PanelAlignment::Center => "center",
            PanelAlignment::Justify => "justify",
            PanelAlignment::Left => "left",
            PanelAlignment::Right => "right",
        }
    }
}

/// The projects folder when none is chosen: `C:\DNN` on Windows; on macOS and Linux `DNN` in the home folder
/// (`/Users/you/DNN`, `/home/you/DNN`) - a Windows path is no folder there.
pub fn default_projects_folder() -> String {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(std::path::PathBuf::from);
    projects_folder_for(cfg!(windows), home.as_deref())
}

fn projects_folder_for(windows: bool, home: Option<&std::path::Path>) -> String {
    if windows {
        return r"C:\DNN".into();
    }
    home.unwrap_or(std::path::Path::new("/")).join("DNN").display().to_string()
}

/// The defaults of `UserSettings` for what this build reads (the projects folder's is [`default_projects_folder`]). A missing row reads as its default.
const DEFAULTS: &[(&str, &str)] = &[
    ("projects.hostnameSuffix", "dnndev.me"),
    ("projects.sitePort", "80"),
    ("projects.keepWarm.pingMinutes", "5"),
    ("sqlServer.type", "container"),
    ("sqlServer.host", "localhost"),
    ("sqlServer.port", "1433"),
    ("docker.containerName", "dnn-sqlserver"),
    ("appearance.theme", "system"),
    ("appearance.uiScale", "100"),
    ("appearance.fontSize", "13"),
    ("appearance.animations", "true"),
    ("terminal.defaultShell", "powershell"),
    ("terminal.fontSize", "13"),
    ("window.keepRunningWhenClosed", "true"),
    ("updates.checkAtStart", "true"),
    ("layout.sidebarPosition", "left"),
    ("layout.panelAlignment", "center"),
    ("layout.menuBarVisible", "true"),
    ("layout.statusBarVisible", "true"),
    ("layout.density", "default"),
    ("layout.quickInputPosition", "top"),
];

/// The columns the Projects table shows by default (`ProjectColumns.Default`).
pub const DEFAULT_PROJECT_COLUMNS: &[&str] = &["url", "dnn", "database", "sql", "cpu", "memory", "pid", "lastStarted"];

/// The settings rows, with typed accessors for the ones the app reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    rows: BTreeMap<String, String>,
}

impl Settings {
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String)>) -> Self {
        Self { rows: rows.into_iter().collect() }
    }

    pub fn rows(&self) -> &BTreeMap<String, String> {
        &self.rows
    }

    /// The row's value, or the setting's default.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.rows.get(key).map(String::as_str).or_else(|| DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v))
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.rows.insert(key.to_string(), value.into());
    }

    fn bool(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    fn number(&self, key: &str, min: u32, max: u32, fallback: u32) -> u32 {
        self.get(key).and_then(|v| v.parse().ok()).filter(|n| (min..=max).contains(n)).unwrap_or(fallback)
    }

    /// A list setting: the count row, then `key[0]`, `key[1]`… None when it has no count row.
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        let count: usize = self.rows.get(key)?.parse().ok()?;
        Some((0..count).filter_map(|i| self.rows.get(&format!("{key}[{i}]")).cloned()).collect())
    }

    /// Replaces a list setting's rows.
    pub fn set_list(&mut self, key: &str, items: &[String]) {
        let prefix = format!("{key}[");
        self.rows.retain(|k, _| !k.starts_with(&prefix));
        self.rows.insert(key.to_string(), items.len().to_string());
        for (i, item) in items.iter().enumerate() {
            self.rows.insert(format!("{key}[{i}]"), item.clone());
        }
    }

    /// The settings version the rows were written with; 0 for a database without one.
    pub fn version(&self) -> u32 {
        self.rows.get("version").and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    /// Written by a newer DNN Manager: not used, so this build doesn't misread them.
    pub fn is_newer_than_build(&self) -> bool {
        self.version() > CURRENT_VERSION
    }

    pub fn theme(&self) -> ThemeChoice {
        match self.get("appearance.theme").unwrap_or_default().to_ascii_lowercase().as_str() {
            "light" => ThemeChoice::Light,
            "dark" => ThemeChoice::Dark,
            _ => ThemeChoice::System,
        }
    }

    pub fn set_theme(&mut self, theme: ThemeChoice) {
        let value = match theme {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        };
        self.set("appearance.theme", value);
    }

    /// The text size of body text, 10 to 20 (13 by default).
    pub fn font_size(&self) -> u32 {
        self.number("appearance.fontSize", 10, 20, 13)
    }

    /// UI scale in percent, 80 to 200.
    pub fn ui_scale(&self) -> u32 {
        self.number("appearance.uiScale", 80, 200, 100)
    }

    pub fn animations(&self) -> bool {
        self.bool("appearance.animations")
    }

    pub fn sidebar_position(&self) -> SidebarPosition {
        if self.get("layout.sidebarPosition").is_some_and(|v| v.eq_ignore_ascii_case("right")) {
            SidebarPosition::Right
        } else {
            SidebarPosition::Left
        }
    }

    pub fn density(&self) -> Density {
        if self.get("layout.density").is_some_and(|v| v.eq_ignore_ascii_case("compact")) {
            Density::Compact
        } else {
            Density::Default
        }
    }

    pub fn panel_alignment(&self) -> PanelAlignment {
        match self.get("layout.panelAlignment").unwrap_or_default().to_ascii_lowercase().as_str() {
            "justify" => PanelAlignment::Justify,
            "left" => PanelAlignment::Left,
            "right" => PanelAlignment::Right,
            _ => PanelAlignment::Center,
        }
    }

    /// The command palette in the middle of the window (`layout.quickInputPosition` "center"), not at its top.
    pub fn quick_input_centered(&self) -> bool {
        self.get("layout.quickInputPosition").is_some_and(|v| v.eq_ignore_ascii_case("center"))
    }

    pub fn status_bar_visible(&self) -> bool {
        self.bool("layout.statusBarVisible")
    }

    pub fn menu_bar_visible(&self) -> bool {
        self.bool("layout.menuBarVisible")
    }

    /// The projects folder: a site whose folder is inside it is a DNN Manager project.
    pub fn base_directory(&self) -> String {
        self.get("projects.baseDirectory").map_or_else(default_projects_folder, str::to_string)
    }

    /// The Projects table's optional columns, in the order chosen.
    pub fn project_columns(&self) -> Vec<String> {
        self.list("appearance.projectColumns")
            .unwrap_or_else(|| DEFAULT_PROJECT_COLUMNS.iter().map(|c| c.to_string()).collect())
    }

    pub fn set_project_columns(&mut self, columns: &[String]) {
        self.set_list("appearance.projectColumns", columns);
    }
}

/// Where the settings are kept - `dnnmanager.db` in infrastructure, a map in tests.
pub trait SettingsStore: Send + Sync {
    fn load(&self) -> Result<Settings, String>;
    /// Writes the rows that differ from `before`; the others stay as the database has them.
    fn save(&self, before: &Settings, after: &Settings) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(rows: &[(&str, &str)]) -> Settings {
        Settings::from_rows(rows.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn the_projects_folder_is_c_dnn_on_windows_and_in_the_home_folder_elsewhere() {
        use std::path::Path;
        assert_eq!(projects_folder_for(true, Some(Path::new("/Users/ann"))), r"C:\DNN");
        let mac = projects_folder_for(false, Some(Path::new("/Users/ann")));
        assert_eq!(Path::new(&mac), Path::new("/Users/ann/DNN"));
        assert_eq!(Path::new(&projects_folder_for(false, None)), Path::new("/DNN"));
        if cfg!(windows) {
            assert_eq!(default_projects_folder(), r"C:\DNN");
        }
    }

    #[test]
    fn missing_rows_read_as_defaults() {
        let s = Settings::default();
        assert_eq!(s.theme(), ThemeChoice::System);
        assert_eq!(s.font_size(), 13);
        assert_eq!(s.base_directory(), default_projects_folder());
        assert_eq!(s.project_columns().len(), 8);
        assert!(s.animations() && s.status_bar_visible());
    }

    #[test]
    fn reads_lists_and_typed_values() {
        let s = settings(&[
            ("appearance.theme", "Dark"),
            ("appearance.fontSize", "99"),
            ("appearance.projectColumns", "2"),
            ("appearance.projectColumns[0]", "size"),
            ("appearance.projectColumns[1]", "path"),
            ("layout.density", "compact"),
        ]);
        assert_eq!(s.theme(), ThemeChoice::Dark);
        assert_eq!(s.font_size(), 13, "out of range falls back");
        assert_eq!(s.project_columns(), vec!["size", "path"]);
        assert_eq!(s.density(), Density::Compact);
    }

    #[test]
    fn set_list_replaces_old_items() {
        let mut s = settings(&[("x", "3"), ("x[0]", "a"), ("x[1]", "b"), ("x[2]", "c"), ("x.y", "kept")]);
        s.set_list("x", &["z".to_string()]);
        assert_eq!(s.list("x"), Some(vec!["z".to_string()]));
        assert!(!s.rows().contains_key("x[2]"));
        assert_eq!(s.get("x.y"), Some("kept"));
    }

    #[test]
    fn newer_settings_are_flagged() {
        assert!(settings(&[("version", "4")]).is_newer_than_build());
        assert!(!settings(&[("version", "3")]).is_newer_than_build());
    }
}
