//! The commands (`MainWindow.Commands.cs`): what the palette lists and the shortcuts run - word for word, with
//! their ids, areas and default shortcuts. A command whose feature isn't ported isn't listed; a project command
//! asks which project ("choose a project").

use gpui::{App, Context, SharedString, WeakEntity, Window};

use dnnm_application::iis::{IisServerAction, IisServerState};
use dnnm_application::project::{SiteRunState, view};
use dnnm_application::settings::ThemeChoice;
use dnnm_application::use_cases::SiteAction;

use dnnm_dialogs::setting_rules::Category;

use super::{Page, Workspace};
use crate::pages::columns::{Column, SortBy};
use crate::palette::{Mode, PaletteItem};

/// The palette's recent commands: state area `palette`, a list `recent` (`PaletteState`, 8 kept).
const RECENT_AREA: &str = "palette";
const RECENT_KEY: &str = "recent";
const RECENT_KEPT: usize = 8;

/// One command: id (what `keyboard.shortcuts` stores), title, area, shortcut, keywords.
struct Command {
    id: &'static str,
    title: &'static str,
    area: &'static str,
    shortcut: &'static str,
    keywords: &'static str,
    run: Run,
}

/// What a command does.
enum Run {
    /// On the workspace, if it can run now.
    Now(fn(&mut Workspace, &mut Window, &mut Context<Workspace>), fn(&Workspace, &App) -> bool),
    /// On a project the user picks: which ones fit, and what to do with it.
    Project(
        fn(&dnnm_application::project::ProjectState) -> bool,
        fn(&mut Workspace, String, &mut Window, &mut Context<Workspace>),
    ),
    /// Sorts the Projects table.
    Sort(SortBy),
    /// Keep warm on (true) or off for a project the user picks.
    KeepWarm(bool),
    /// An edit of a project the user picks.
    Action(super::ProjectAction),
    /// Settings, open on a category.
    Settings(Category),
    /// Starts, stops or restarts IIS (asks first for stop and restart).
    Iis(IisServerAction),
}

fn always(_: &Workspace, _: &App) -> bool {
    true
}

fn idle(ws: &Workspace, cx: &App) -> bool {
    !ws.store.read(cx).is_busy()
}

fn commands() -> Vec<Command> {
    use Run::*;
    vec![
        // ── Application ──
        Command {
            id: "workbench.commandPalette",
            title: "Show all commands",
            area: "Application",
            shortcut: "Ctrl+Shift+P",
            keywords: "command palette actions run",
            run: Now(|ws, window, cx| ws.show_palette(true, window, cx), always),
        },
        Command {
            id: "app.checkUpdates",
            title: "Check for updates",
            area: "Application",
            shortcut: "",
            keywords: "version release github",
            run: Now(|ws, _, cx| ws.check_for_updates(None, true, cx), always),
        },
        Command {
            id: "app.whatsNew",
            title: "What's new in this version",
            area: "Application",
            shortcut: "",
            keywords: "release notes changes changelog version",
            run: Now(|ws, window, cx| ws.show_whats_new(window, cx), |ws, cx| !ws.is_new_user(cx)),
        },
        Command {
            id: "app.restart",
            title: "Restart DNN Manager",
            area: "Application",
            shortcut: "",
            keywords: "reload",
            run: Now(|ws, window, cx| ws.restart(window, cx), idle),
        },
        Command {
            id: "app.quit",
            title: "Quit DNN Manager",
            area: "Application",
            // ⌘Q on macOS, where every app quits so (the table is written for Windows: Ctrl becomes ⌘ there).
            // Windows and Linux have none: their windows close with Alt+F4.
            shortcut: if cfg!(target_os = "macos") { "Ctrl+Q" } else { "" },
            keywords: "exit close stop background notification area tray",
            run: Now(|ws, window, cx| ws.quit(window, cx), always),
        },
        Command {
            id: "app.colorTheme",
            title: "Color theme…",
            area: "Application",
            shortcut: "",
            keywords: "dark light modern system appearance",
            run: Now(|ws, window, cx| ws.show_theme_pick(window, cx), always),
        },
        // ── Help ──
        Command {
            id: "help.page",
            title: "Help for this page",
            area: "Help",
            shortcut: "F1",
            keywords: "explain what is this question how",
            run: Now(|ws, window, cx| ws.show_page_help(window, cx), always),
        },
        Command {
            id: "help.gettingStarted",
            title: "Getting started guide",
            area: "Help",
            shortcut: "",
            keywords: "welcome onboarding tutorial introduction beginner first start how to",
            run: Now(|ws, window, cx| ws.show_guide(window, cx), always),
        },
        Command {
            id: "help.tour",
            title: "Take the tour",
            area: "Help",
            shortcut: "",
            keywords: "tutorial walkthrough show around highlight window",
            run: Now(|ws, window, cx| ws.start_tour(window, cx), always),
        },
        Command {
            id: "help.userGuide",
            title: "User guide on GitHub",
            area: "Help",
            shortcut: "",
            keywords: "documentation docs manual read",
            run: Now(|ws, _, cx| ws.open_link(dnnm_dialogs::help_menu::USER_GUIDE_URL, cx), always),
        },
        // ── Projects ──
        Command {
            id: "projects.quickOpen",
            title: "Go to project…",
            area: "Projects",
            shortcut: "Ctrl+P",
            keywords: "open find quick",
            run: Now(|ws, window, cx| ws.show_palette(false, window, cx), always),
        },
        Command {
            id: "projects.search",
            title: "Search projects",
            area: "Projects",
            shortcut: "Ctrl+Shift+F",
            keywords: "find filter",
            run: Now(|ws, window, cx| ws.search_projects(window, cx), always),
        },
        Command {
            id: "projects.columns",
            title: "Choose columns…",
            area: "Projects",
            shortcut: "",
            keywords: "show hide table",
            run: Now(
                |ws, window, cx| {
                    ws.go(Page::Projects, window, cx);
                    ws.projects.update(cx, |p, cx| p.open_columns(cx));
                },
                always,
            ),
        },
        sort("projects.sortByName", "Sort projects by name", SortBy::Name),
        sort("projects.sortByState", "Sort projects by status", SortBy::State),
        sort("projects.sortByUrl", "Sort projects by site", SortBy::Column(Column::Url)),
        sort("projects.sortByCpuSort", "Sort projects by CPU", SortBy::Column(Column::Cpu)),
        sort("projects.sortByMemorySort", "Sort projects by memory", SortBy::Column(Column::Memory)),
        sort("projects.sortByLastStartedSort", "Sort projects by last started", SortBy::Column(Column::LastStarted)),
        sort("projects.sortBySizeBytes", "Sort projects by size", SortBy::Column(Column::Size)),
        // ── Project ──
        Command {
            id: "project.open",
            title: "Open project details",
            area: "Project",
            shortcut: "",
            keywords: "details inspect information",
            run: Run::Project(|_| true, |ws, name, window, cx| ws.go_to_project(&name, window, cx)),
        },
        Command {
            id: "project.start",
            title: "Start project",
            area: "Project",
            shortcut: "F5",
            keywords: "run on",
            run: Project(
                |p| matches!(view::run_state(&p.site), SiteRunState::Stopped | SiteRunState::Unknown),
                |ws, name, _, cx| ws.store.update(cx, |s, cx| s.control_sites(vec![name], SiteAction::Start, cx)),
            ),
        },
        Command {
            id: "project.stop",
            title: "Stop project",
            area: "Project",
            shortcut: "Shift+F5",
            keywords: "off",
            run: Project(
                |p| view::run_state(&p.site) == SiteRunState::Running,
                |ws, name, _, cx| ws.store.update(cx, |s, cx| s.control_sites(vec![name], SiteAction::Stop, cx)),
            ),
        },
        Command {
            id: "project.restart",
            title: "Restart project",
            area: "Project",
            shortcut: "Ctrl+Shift+R",
            keywords: "recycle reload",
            run: Project(
                |p| view::run_state(&p.site) == SiteRunState::Running,
                |ws, name, _, cx| ws.store.update(cx, |s, cx| s.control_sites(vec![name], SiteAction::Restart, cx)),
            ),
        },
        Command {
            id: "project.openWebsite",
            title: "Open website",
            area: "Project",
            shortcut: "",
            keywords: "browser browse url",
            run: Project(
                |p| !p.site_url.is_empty(),
                |ws, name, _, cx| {
                    let url = ws.store.read(cx).projects.iter().find(|p| p.name == name).map(|p| p.site_url.clone());
                    if let Some(url) = url {
                        ws.open_url(&url, cx);
                    }
                },
            ),
        },
        Command {
            id: "project.openFolder",
            title: "Open project folder",
            area: "Project",
            shortcut: "",
            keywords: "explorer files",
            run: Project(
                |p| p.directory.is_dir(),
                |ws, name, _, cx| {
                    let dir = ws.store.read(cx).projects.iter().find(|p| p.name == name).map(|p| p.directory.clone());
                    if let Some(dir) = dir {
                        ws.open_folder(&dir, cx);
                    }
                },
            ),
        },
        Command {
            id: "project.keepWarm",
            title: "Keep website warm",
            area: "Project",
            shortcut: "",
            keywords: "ping alive fast",
            run: Run::KeepWarm(true),
        },
        Command {
            id: "project.stopKeepWarm",
            title: "Stop keeping website warm",
            area: "Project",
            shortcut: "",
            keywords: "ping alive",
            run: Run::KeepWarm(false),
        },
        Command {
            id: "project.terminal",
            title: "Open terminal in project folder",
            area: "Project",
            shortcut: "",
            keywords: "command line shell powershell cmd",
            run: Run::Project(
                |p| p.directory.is_dir(),
                |ws, name, window, cx| {
                    let dir = ws.store.read(cx).projects.iter().find(|p| p.name == name).map(|p| p.directory.clone());
                    ws.new_terminal(dir.map(|d| d.display().to_string()), false, window, cx);
                },
            ),
        },
        Command {
            id: "project.logs",
            title: "Show project logs",
            area: "Project",
            shortcut: "",
            keywords: "log errors events iis dnn",
            run: Run::Project(
                |_| true,
                |ws, name, _, cx| ws.show_logs(crate::logs_view::LogSite::Site(name), cx),
            ),
        },
        Command {
            id: dnnm_wiring::rename_project::COMMAND_ID,
            title: dnnm_wiring::rename_project::COMMAND,
            area: "Project",
            shortcut: "",
            keywords: dnnm_wiring::rename_project::COMMAND_KEYWORDS,
            run: Run::Action(super::ProjectAction::Rename),
        },
        Command {
            id: dnnm_wiring::edit_bindings::COMMAND_ID,
            title: dnnm_wiring::edit_bindings::COMMAND,
            area: "Project",
            shortcut: "",
            keywords: dnnm_wiring::edit_bindings::COMMAND_KEYWORDS,
            run: Run::Action(super::ProjectAction::EditBindings),
        },
        Command {
            id: dnnm_wiring::edit_app_pool::COMMAND_ID,
            title: dnnm_wiring::edit_app_pool::COMMAND,
            area: "Project",
            shortcut: "",
            keywords: dnnm_wiring::edit_app_pool::COMMAND_KEYWORDS,
            run: Run::Action(super::ProjectAction::EditAppPool),
        },
        Command {
            id: "project.editDatabase",
            title: "Change database connection…",
            area: "Project",
            shortcut: "",
            keywords: "sql server web.config connection string sitesqlserver",
            run: Run::Action(super::ProjectAction::ChangeConnection),
        },
        Command {
            id: "project.exportForDeployment",
            title: "Export for deployment…",
            area: "Project",
            shortcut: "",
            keywords: "deploy production package zip publish",
            run: Run::Action(super::ProjectAction::ExportForDeployment),
        },
        Command {
            id: dnnm_wiring::upgrade_dnn::texts::COMMAND,
            title: "Upgrade DNN…",
            area: "Project",
            shortcut: "",
            keywords: dnnm_wiring::upgrade_dnn::texts::COMMAND_KEYWORDS,
            run: Run::Action(super::ProjectAction::UpgradeDnn),
        },
        // ── IIS ──
        iis("iis.start", "Start IIS", IisServerAction::Start),
        iis("iis.stop", "Stop IIS", IisServerAction::Stop),
        iis("iis.restart", "Restart IIS", IisServerAction::Restart),
        // ── Pages ──
        Command {
            id: "pages.projects",
            title: "Go to Projects",
            area: "Pages",
            shortcut: "Ctrl+1",
            keywords: "list websites sites",
            run: Now(|ws, window, cx| ws.go(Page::Projects, window, cx), always),
        },
        Command {
            id: "pages.newProject",
            title: "New project",
            area: "Pages",
            shortcut: "Ctrl+2",
            keywords: "create add site website setup import",
            run: Now(|ws, window, cx| ws.go(Page::NewProject, window, cx), always),
        },
        Command {
            id: "pages.sourceControl",
            title: "Source Control",
            area: "Pages",
            shortcut: "Ctrl+3",
            keywords: "deploy sftp ftp upload download server compare diff sync environment production",
            run: Now(|ws, window, cx| ws.go(Page::SourceControl, window, cx), always),
        },
        Command {
            id: "settings.open",
            title: "Open Settings",
            area: "Pages",
            shortcut: "Ctrl+,",
            keywords: "preferences options configuration",
            run: Now(|ws, window, cx| ws.open_settings_on(None, window, cx), always),
        },
        Command {
            id: "troubleshoot.open",
            title: "Run troubleshooting",
            area: "Pages",
            shortcut: "",
            keywords: "restart reset clean up data",
            run: Now(|ws, window, cx| ws.open_troubleshoot(window, cx), always),
        },
        settings("settings.general", Category::General, "General", "start sign in background tray scale font terminal"),
        settings("settings.projects", Category::Projects, "Projects", "folder hostname port dnn defaults keep warm"),
        settings("settings.releases", Category::Releases, "DNN releases", "repositories github versions packages upgrade path"),
        settings("settings.databaseServer", Category::Sql, "Database server", "sql server express localdb connection ssms"),
        settings("settings.docker", Category::Docker, "Docker container", "container volume edition collation compose"),
        settings("settings.iis", Category::Iis, "IIS", "windows features"),
        settings("settings.keyboard", Category::Keyboard, "Keyboard shortcuts", "keys bindings hotkeys"),
        settings("settings.help", Category::Help, "Help", "getting started guide tour tutorial what's new"),
        settings("settings.about", Category::About, "About", "version license repository folders"),
        Command {
            id: "view.next",
            title: "Next page",
            area: "Pages",
            shortcut: "Ctrl+Tab",
            keywords: "switch view",
            run: Now(|ws, window, cx| ws.step_page(true, window, cx), always),
        },
        Command {
            id: "view.previous",
            title: "Previous page",
            area: "Pages",
            shortcut: "Ctrl+Shift+Tab",
            keywords: "switch view",
            run: Now(|ws, window, cx| ws.step_page(false, window, cx), always),
        },
        Command {
            id: "view.close",
            title: "Close this view",
            area: "Pages",
            shortcut: "Ctrl+W",
            keywords: "back details settings troubleshoot",
            run: Now(|ws, window, cx| ws.close_view(window, cx), |ws, cx| ws.can_close_view(cx)),
        },
        // ── Layout ──
        Command {
            id: "view.toggleSidebar",
            title: "Toggle primary side bar visibility",
            area: "Layout",
            shortcut: "Ctrl+B",
            keywords: "sidebar side bar",
            run: Now(|ws, _, cx| ws.toggle_primary(cx), always),
        },
        Command {
            id: "view.toggleSecondarySidebar",
            title: "Toggle secondary side bar visibility",
            area: "Layout",
            shortcut: "Ctrl+Alt+B",
            keywords: "sidebar side bar auxiliary right",
            run: Now(|ws, _, cx| ws.toggle_secondary(cx), always),
        },
        Command {
            id: "view.toggleActivityBar",
            title: "Toggle activity bar visibility",
            area: "Layout",
            shortcut: "",
            keywords: "pages navigation sidebar",
            run: Now(|ws, _, cx| ws.toggle_sidebar(cx), always),
        },
        Command {
            id: "view.sidebarWider",
            title: "Make the primary side bar wider",
            area: "Layout",
            shortcut: "",
            keywords: "size width sash",
            run: Now(|ws, window, cx| ws.resize_sidebar_by(KEYBOARD_RESIZE_STEP, window, cx), always),
        },
        Command {
            id: "view.sidebarNarrower",
            title: "Make the primary side bar narrower",
            area: "Layout",
            shortcut: "",
            keywords: "size width sash",
            run: Now(|ws, window, cx| ws.resize_sidebar_by(-KEYBOARD_RESIZE_STEP, window, cx), always),
        },
        Command {
            id: "view.toggleMenuBar",
            title: "Toggle menu bar visibility",
            area: "Layout",
            shortcut: "",
            keywords: "title",
            run: Now(
                |ws, _, cx| {
                    let visible = ws.store.read(cx).settings.menu_bar_visible();
                    ws.store.update(cx, |s, cx| {
                        s.update_settings(|st| st.set("layout.menuBarVisible", (!visible).to_string()), cx)
                    });
                },
                always,
            ),
        },
        Command {
            id: "view.toggleStatusBar",
            title: "Toggle status bar visibility",
            area: "Layout",
            shortcut: "",
            keywords: "bottom",
            run: Now(
                |ws, _, cx| {
                    let visible = ws.store.read(cx).settings.status_bar_visible();
                    ws.store.update(cx, |s, cx| {
                        s.update_settings(|st| st.set("layout.statusBarVisible", (!visible).to_string()), cx)
                    });
                },
                always,
            ),
        },
        // ── Terminal ──
        Command {
            id: "terminal.toggle",
            title: "Toggle terminal",
            area: "Terminal",
            shortcut: "Ctrl+`",
            keywords: "command line shell console show hide",
            run: Now(|ws, window, cx| ws.toggle_terminal(window, cx), |ws, _| ws.terminal.is_some()),
        },
        Command {
            id: "terminal.new",
            title: "New terminal",
            area: "Terminal",
            shortcut: "Ctrl+Shift+`",
            keywords: "command line shell console open",
            run: Now(|ws, window, cx| ws.new_terminal(None, false, window, cx), |ws, _| ws.terminal.is_some()),
        },
        Command {
            id: "terminal.newAdmin",
            title: "New Administrator terminal",
            area: "Terminal",
            shortcut: "",
            keywords: "elevated admin command line shell console",
            run: Now(|ws, window, cx| ws.new_terminal(None, true, window, cx), |ws, _| ws.terminal.is_some()),
        },
        Command {
            id: "layout.customize",
            title: "Customize layout…",
            area: "Layout",
            shortcut: "",
            keywords: "panel sidebar status bar menu density position alignment",
            run: Now(|ws, window, cx| ws.customize_layout(window, cx), always),
        },
        Command {
            id: "view.search",
            title: "Find in the panel",
            area: "Panel",
            shortcut: "Ctrl+F",
            keywords: "search output logs terminal match regex",
            run: Now(|ws, window, cx| ws.open_find(window, cx), always),
        },
        Command {
            id: "panel.maximize",
            title: "Toggle maximized panel",
            area: "Panel",
            shortcut: "Ctrl+Shift+M",
            keywords: "bigger full",
            run: Now(
                |ws, _, cx| {
                    ws.panel_visible = true;
                    ws.panel_maximized = !ws.panel_maximized;
                    cx.notify();
                },
                always,
            ),
        },
        // ── Panel ──
        Command {
            id: "panel.toggle",
            title: "Toggle panel visibility",
            area: "Panel",
            shortcut: "Ctrl+J",
            keywords: "output bottom",
            run: Now(|ws, _, cx| ws.toggle_panel(cx), always),
        },
        Command {
            id: "panel.taller",
            title: "Make the panel taller",
            area: "Panel",
            shortcut: "",
            keywords: "size height sash bigger",
            run: Now(|ws, window, cx| ws.resize_panel_by(KEYBOARD_RESIZE_STEP, window, cx), always),
        },
        Command {
            id: "panel.shorter",
            title: "Make the panel shorter",
            area: "Panel",
            shortcut: "",
            keywords: "size height sash smaller",
            run: Now(|ws, window, cx| ws.resize_panel_by(-KEYBOARD_RESIZE_STEP, window, cx), always),
        },
        Command {
            id: "panel.output",
            title: "Toggle Output",
            area: "Panel",
            shortcut: "Ctrl+Shift+U",
            keywords: "log operation show hide",
            run: Now(|ws, _, cx| ws.toggle_output(cx), always),
        },
        Command {
            id: "panel.appLog",
            title: "Toggle DNN Manager's log",
            area: "Panel",
            shortcut: "",
            keywords: "logs file errors show hide",
            run: Now(|ws, _, cx| ws.toggle_app_log(cx), always),
        },
        Command {
            id: "panel.clearOutput",
            title: "Clear the output",
            area: "Panel",
            shortcut: "",
            keywords: "empty",
            run: Now(|ws, _, cx| ws.store.update(cx, |s, cx| s.clear_output(cx)), idle),
        },
    ]
}

/// How far "Make the sidebar wider" and its kin move a sash (`KeyboardResizeStep`).
const KEYBOARD_RESIZE_STEP: f32 = 20.0;

fn settings(id: &'static str, category: Category, title: &'static str, keywords: &'static str) -> Command {
    Command { id, title, area: "Settings", shortcut: "", keywords, run: Run::Settings(category) }
}

/// A command's default shortcut as this OS has it ("Ctrl+P", "⌘P" on macOS), None without one.
pub(super) fn default_shortcut(id: &str) -> Option<String> {
    commands().into_iter().find(|c| c.id == id).and_then(|c| default_here(c.shortcut))
}

/// A default from the table - written for Windows - as this OS has it.
fn default_here(shortcut: &str) -> Option<String> {
    dnnm_dialogs::settings_keyboard::Shortcut::default_on(shortcut, dnnm_ui::keys::Os::CURRENT).map(|s| s.to_string())
}

/// Every command, for Settings → Keyboard shortcuts - in the order they are matched.
pub(super) fn shortcut_commands() -> Vec<dnnm_dialogs::settings_keyboard::ShortcutCommand> {
    commands()
        .into_iter()
        .map(|c| {
            let default = default_here(c.shortcut);
            dnnm_dialogs::settings_keyboard::ShortcutCommand::new(c.id, c.title, c.area, default.as_deref())
        })
        .collect()
}

fn sort(id: &'static str, title: &'static str, by: SortBy) -> Command {
    Command { id, title, area: "Projects", shortcut: "", keywords: "order", run: Run::Sort(by) }
}

fn iis(id: &'static str, title: &'static str, action: IisServerAction) -> Command {
    Command { id, title, area: "IIS", shortcut: "", keywords: "web server iisreset", run: Run::Iis(action) }
}

impl Command {
    fn label(&self) -> String {
        format!("{}: {}", self.area, self.title)
    }
}

/// IIS can be started while it is stopped, stopped and restarted while it runs - and only while nothing else runs.
fn iis_available(action: IisServerAction, ws: &Workspace, cx: &App) -> bool {
    let state = ws.store.read(cx).iis_state;
    idle(ws, cx)
        && match action {
            IisServerAction::Start => state == IisServerState::Stopped,
            _ => state == IisServerState::Running,
        }
}

impl Workspace {
    /// The palette's recent commands, the newest first.
    fn recent_commands(&self, cx: &App) -> Vec<String> {
        let db = self.store.read(cx).db().clone();
        let count: usize = db.state(RECENT_AREA, RECENT_KEY).ok().flatten().and_then(|c| c.parse().ok()).unwrap_or(0);
        (0..count.min(RECENT_KEPT))
            .filter_map(|i| db.state(RECENT_AREA, &format!("{RECENT_KEY}[{i}]")).ok().flatten())
            .collect()
    }

    fn remember_command(&self, id: &str, cx: &App) {
        let mut recent = self.recent_commands(cx);
        recent.retain(|r| r != id);
        recent.insert(0, id.to_string());
        recent.truncate(RECENT_KEPT);
        let db = self.store.read(cx).db().clone();
        let _ = db.set_state(RECENT_AREA, RECENT_KEY, &recent.len().to_string());
        for (i, r) in recent.iter().enumerate() {
            let _ = db.set_state(RECENT_AREA, &format!("{RECENT_KEY}[{i}]"), r);
        }
    }

    /// The commands that can run now - a project command when any project fits it. Those last run from the palette
    /// first, "recently used", then "other commands" - as VS Code lists them.
    pub(super) fn command_items(&self, cx: &mut Context<Self>) -> Vec<PaletteItem> {
        let recent = self.recent_commands(cx);
        let weak = cx.entity().downgrade();
        let projects = self.store.read(cx).projects.clone();
        let mut all = commands();
        all.sort_by_key(|c| recent.iter().position(|r| r == c.id).unwrap_or(usize::MAX));
        let mut items = Vec::new();
        for command in all {
            let detail = match &command.run {
                Run::Project(fits, _) if projects.iter().any(|p| p.dnn_version.is_some() && fits(p)) => {
                    "choose a project"
                }
                Run::Project(..) => continue,
                Run::Action(_) => {
                    let busy = self.store.read(cx).is_busy();
                    if !projects.iter().any(|p| p.dnn_version.is_some() && dnnm_wiring::rename_project::can_edit(busy, &p.directory)) {
                        continue;
                    }
                    "choose a project"
                }
                Run::KeepWarm(on) => {
                    let store = self.store.read(cx);
                    let fits = |p: &&dnnm_application::project::ProjectState| {
                        let warm = store.keep_warm_status(&p.name).is_on();
                        if *on { !warm && !p.site_url.is_empty() } else { warm }
                    };
                    if !projects.iter().any(|p| p.dnn_version.is_some() && fits(&p)) {
                        continue;
                    }
                    "choose a project"
                }
                Run::Now(_, available) if !available(self, cx) => continue,
                Run::Iis(action) if !iis_available(*action, self, cx) => continue,
                Run::Now(..) | Run::Sort(_) | Run::Iis(_) | Run::Settings(_) => "",
            };
            let group = if recent.is_empty() {
                ""
            } else if recent.iter().any(|r| r == command.id) {
                "recently used"
            } else {
                "other commands"
            };
            let id = command.id;
            let weak = weak.clone();
            items.push(
                PaletteItem::new(command.label(), move |window, cx| {
                    let _ = weak.update(cx, |ws, cx| ws.run_command(id, window, cx));
                })
                .detail(detail)
                .keywords(command.keywords)
                .shortcut(super::shortcuts::shortcut_text(command.id, &self.store.read(cx).settings).unwrap_or_default())
                .group(group),
            );
        }
        // None of the recent ones can run now: no "other commands" without "recently used" above.
        if !items.iter().any(|i| i.group.as_ref() == "recently used") {
            for item in &mut items {
                item.group = SharedString::default();
            }
        }
        items
    }

    /// Runs a command by its id - from the palette (remembered as recently used) or a shortcut.
    pub(super) fn run_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = commands().into_iter().find(|c| c.id == id) else { return };
        self.remember_command(id, cx);
        match command.run {
            Run::Now(run, available) => {
                if available(self, cx) {
                    run(self, window, cx);
                }
            }
            Run::Action(action) => {
                let weak: WeakEntity<Workspace> = cx.entity().downgrade();
                let busy = self.store.read(cx).is_busy();
                let items = self
                    .store
                    .read(cx)
                    .projects
                    .iter()
                    .filter(|p| p.dnn_version.is_some() && dnnm_wiring::rename_project::can_edit(busy, &p.directory))
                    .map(|p| {
                        let (name, weak) = (p.name.clone(), weak.clone());
                        PaletteItem::new(p.name.clone(), move |window, cx| {
                            let name = name.clone();
                            let _ = weak.update(cx, |ws, cx| ws.project_action(action, name, window, cx));
                        })
                        .detail(view::url(p))
                    })
                    .collect();
                let prompt: SharedString = format!("{} - choose a project", command.title).into();
                self.open_palette(Mode::Pick { prompt, items }, false, window, cx);
            }
            Run::KeepWarm(on) => {
                let weak: WeakEntity<Workspace> = cx.entity().downgrade();
                let store = self.store.read(cx);
                let items = store
                    .projects
                    .iter()
                    .filter(|p| p.dnn_version.is_some())
                    .filter(|p| {
                        let warm = store.keep_warm_status(&p.name).is_on();
                        if on { !warm && !p.site_url.is_empty() } else { warm }
                    })
                    .map(|p| {
                        let name = p.name.clone();
                        let weak = weak.clone();
                        PaletteItem::new(p.name.clone(), move |_, cx| {
                            let name = name.clone();
                            let _ = weak.update(cx, |ws, cx| ws.store.update(cx, |s, cx| s.set_keep_warm(&[name], on, cx)));
                        })
                        .detail(view::url(p))
                    })
                    .collect();
                let prompt: SharedString = format!("{} - choose a project", command.title).into();
                self.open_palette(Mode::Pick { prompt, items }, false, window, cx);
            }
            Run::Settings(category) => self.open_settings_on(Some(category), window, cx),
            Run::Sort(by) => {
                self.go(Page::Projects, window, cx);
                self.projects.update(cx, |p, cx| p.sort_by(by, cx));
            }
            Run::Iis(action) => {
                if iis_available(action, self, cx) {
                    self.store.update(cx, |s, cx| s.control_iis(action, cx));
                }
            }
            Run::Project(fits, run) => {
                let weak: WeakEntity<Workspace> = cx.entity().downgrade();
                let items = self
                    .store
                    .read(cx)
                    .projects
                    .iter()
                    .filter(|p| p.dnn_version.is_some() && fits(p))
                    .map(|p| {
                        let name = p.name.clone();
                        let weak = weak.clone();
                        PaletteItem::new(p.name.clone(), move |window, cx| {
                            let name = name.clone();
                            let _ = weak.update(cx, |ws, cx| run(ws, name, window, cx));
                        })
                        .detail(view::url(p))
                    })
                    .collect();
                let prompt: SharedString = format!("{} - choose a project", command.title).into();
                self.open_palette(Mode::Pick { prompt, items }, false, window, cx);
            }
        }
    }

    /// The themes to choose from, the one in use ticked (`ShowThemePick`).
    pub(super) fn show_theme_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.store.read(cx).settings.theme();
        let weak = cx.entity().downgrade();
        let items =
            [("Dark Modern", ThemeChoice::Dark), ("Light Modern", ThemeChoice::Light), ("System", ThemeChoice::System)]
                .into_iter()
                .map(|(name, choice)| {
                    let weak = weak.clone();
                    PaletteItem::new(name, move |_, cx| {
                        let _ = weak.update(cx, |ws, cx| ws.set_theme(choice, cx));
                    })
                    .checked(current == choice)
                })
                .collect();
        self.open_palette(Mode::Pick { prompt: "Select Color Theme".into(), items }, false, window, cx);
    }
}
