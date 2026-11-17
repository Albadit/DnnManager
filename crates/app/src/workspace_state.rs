//! The workspace kept between starts (`Services/WorkspaceStates.cs`): the window's place and size, the sidebar and
//! panel, the page, and the Projects table's search, filter, sort, expanded rows and selection - rows of the
//! `state` table in areas `window` and `workspace`, the same keys the WPF app writes, so both remember alike.

use dnnm_infrastructure::app_db::AppDatabase;

/// The `window` area (`WindowState`). Sizes and positions in device-independent pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowState {
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub maximized: bool,
    /// The activity bar (the page entries) hidden.
    pub sidebar_hidden: bool,
    /// The primary side bar's width (the key the wide sidebar had).
    pub sidebar_width: Option<f32>,
    pub primary_open: bool,
    pub secondary_open: bool,
    pub secondary_width: Option<f32>,
    pub panel_open: bool,
    pub panel_height: Option<f32>,
    pub panel_maximized: bool,
    /// The Output's stage list's width, as dragged.
    pub output_stages_width: Option<f32>,
}

/// The Projects table's part of the `workspace` area (`ProjectsTableState`, under `projects.`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectsTableState {
    pub search: Option<String>,
    pub only_running: bool,
    /// The sort column's key as the WPF table names it (`Name`, `State`, `Url`, `CpuSort`…); None: not sorted.
    pub sort_by: Option<String>,
    pub sort_descending: bool,
    pub expanded: Vec<String>,
    pub selected: Option<String>,
}

/// The `workspace` area (`WorkspaceState`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceState {
    /// `Projects` or `Setup` (New project).
    pub page: Option<String>,
    pub settings_category: Option<String>,
    /// The project whose Details were open, and their tab (`General`, `IIS`…).
    pub project: Option<String>,
    pub project_tab: Option<String>,
    pub projects: ProjectsTableState,
}

const WINDOW: &str = "window";
const WORKSPACE: &str = "workspace";

fn get(db: &AppDatabase, area: &str, key: &str) -> Option<String> {
    db.state(area, key).ok().flatten()
}

fn number(db: &AppDatabase, area: &str, key: &str) -> Option<f32> {
    // .NET writes doubles in the invariant culture: "1069" or "758.5".
    get(db, area, key).and_then(|v| v.parse().ok()).filter(|v: &f32| v.is_finite())
}

fn flag(db: &AppDatabase, area: &str, key: &str) -> bool {
    get(db, area, key).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

fn text(db: &AppDatabase, area: &str, key: &str) -> Option<String> {
    get(db, area, key).filter(|v| !v.is_empty())
}

fn list(db: &AppDatabase, area: &str, key: &str) -> Vec<String> {
    let count: usize = get(db, area, key).and_then(|c| c.parse().ok()).unwrap_or(0);
    (0..count).filter_map(|i| get(db, area, &format!("{key}[{i}]"))).collect()
}

fn put(db: &AppDatabase, area: &str, key: &str, value: Option<String>) {
    // A value that is null in .NET has no row: the WPF app would read an empty one as a broken number.
    let _ = match value {
        Some(value) => db.set_state(area, key, &value),
        None => db.delete_state(area, key),
    };
}

fn put_number(db: &AppDatabase, area: &str, key: &str, value: Option<f32>) {
    put(db, area, key, value.map(|v| format!("{}", v.round())));
}

fn put_flag(db: &AppDatabase, area: &str, key: &str, value: bool) {
    put(db, area, key, Some(if value { "true" } else { "false" }.into()));
}

fn put_list(db: &AppDatabase, area: &str, key: &str, items: &[String]) {
    // The rows of a longer list from before go, so nothing stale is left behind.
    for i in items.len()..list(db, area, key).len() {
        put(db, area, &format!("{key}[{i}]"), None);
    }
    put(db, area, key, Some(items.len().to_string()));
    for (i, item) in items.iter().enumerate() {
        put(db, area, &format!("{key}[{i}]"), Some(item.clone()));
    }
}

impl WindowState {
    pub fn load(db: &AppDatabase) -> Self {
        Self {
            left: number(db, WINDOW, "left"),
            top: number(db, WINDOW, "top"),
            width: number(db, WINDOW, "width"),
            height: number(db, WINDOW, "height"),
            maximized: flag(db, WINDOW, "maximized"),
            sidebar_hidden: flag(db, WINDOW, "sidebarHidden"),
            sidebar_width: number(db, WINDOW, "sidebarWidth"),
            primary_open: flag(db, WINDOW, "primarySideBarOpen"),
            secondary_open: flag(db, WINDOW, "secondarySideBarOpen"),
            secondary_width: number(db, WINDOW, "secondarySideBarWidth"),
            panel_open: flag(db, WINDOW, "panelOpen"),
            panel_height: number(db, WINDOW, "panelHeight"),
            panel_maximized: flag(db, WINDOW, "panelMaximized"),
            output_stages_width: number(db, WINDOW, "outputStagesWidth"),
        }
    }

    pub fn save(&self, db: &AppDatabase) {
        put_number(db, WINDOW, "left", self.left);
        put_number(db, WINDOW, "top", self.top);
        put_number(db, WINDOW, "width", self.width);
        put_number(db, WINDOW, "height", self.height);
        put_flag(db, WINDOW, "maximized", self.maximized);
        put_flag(db, WINDOW, "sidebarHidden", self.sidebar_hidden);
        put_number(db, WINDOW, "sidebarWidth", self.sidebar_width);
        put_flag(db, WINDOW, "primarySideBarOpen", self.primary_open);
        put_flag(db, WINDOW, "secondarySideBarOpen", self.secondary_open);
        put_number(db, WINDOW, "secondarySideBarWidth", self.secondary_width);
        put_flag(db, WINDOW, "panelOpen", self.panel_open);
        put_number(db, WINDOW, "panelHeight", self.panel_height);
        put_flag(db, WINDOW, "panelMaximized", self.panel_maximized);
        put_number(db, WINDOW, "outputStagesWidth", self.output_stages_width);
    }
}

impl WorkspaceState {
    pub fn load(db: &AppDatabase) -> Self {
        Self {
            page: text(db, WORKSPACE, "page"),
            settings_category: text(db, WORKSPACE, "settingsCategory"),
            project: text(db, WORKSPACE, "project"),
            project_tab: text(db, WORKSPACE, "projectTab"),
            projects: ProjectsTableState {
                search: text(db, WORKSPACE, "projects.search"),
                only_running: flag(db, WORKSPACE, "projects.onlyRunning"),
                sort_by: text(db, WORKSPACE, "projects.sortBy"),
                sort_descending: flag(db, WORKSPACE, "projects.sortDescending"),
                expanded: list(db, WORKSPACE, "projects.expanded"),
                selected: text(db, WORKSPACE, "projects.selected"),
            },
        }
    }

    pub fn save(&self, db: &AppDatabase) {
        put(db, WORKSPACE, "page", self.page.clone());
        put(db, WORKSPACE, "settingsCategory", self.settings_category.clone());
        put(db, WORKSPACE, "project", self.project.clone());
        put(db, WORKSPACE, "projectTab", self.project_tab.clone());
        let p = &self.projects;
        put(db, WORKSPACE, "projects.search", p.search.clone());
        put_flag(db, WORKSPACE, "projects.onlyRunning", p.only_running);
        put(db, WORKSPACE, "projects.sortBy", p.sort_by.clone());
        put_flag(db, WORKSPACE, "projects.sortDescending", p.sort_descending);
        put_list(db, WORKSPACE, "projects.expanded", &p.expanded);
        put(db, WORKSPACE, "projects.selected", p.selected.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_reads_the_wpf_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        // As the WPF app left them on this PC.
        db.set_state("window", "width", "1069").unwrap();
        db.set_state("window", "left", "-1364").unwrap();
        db.set_state("window", "panelOpen", "false").unwrap();
        db.set_state("workspace", "projects.selected", "dashboard").unwrap();
        let window = WindowState::load(&db);
        assert_eq!((window.width, window.left, window.panel_open), (Some(1069.0), Some(-1364.0), false));
        assert_eq!(WorkspaceState::load(&db).projects.selected.as_deref(), Some("dashboard"));

        let state = WorkspaceState {
            page: Some("Setup".into()),
            settings_category: None,
            project: Some("dnn10".into()),
            project_tab: Some("IIS".into()),
            projects: ProjectsTableState {
                search: Some("dnn".into()),
                only_running: true,
                sort_by: Some("CpuSort".into()),
                sort_descending: true,
                expanded: vec!["a".into(), "b".into()],
                selected: None,
            },
        };
        state.save(&db);
        assert_eq!(WorkspaceState::load(&db), state);
    }
}
