//! The window (`MainWindow.xaml`): the title bar with the command center and the layout and window buttons, the
//! sidebar, the page, the bottom panel, the status bar, the toasts, the gear's and Help's menus, Settings and
//! Troubleshoot over the page, the dialogs and the tour.

use std::sync::Arc;
use std::time::SystemTime;

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, Image, ImageFormat, KeyBinding, MouseButton,
    SharedString, Subscription, Window, WindowAppearance, WindowBounds, WindowControlArea, actions, div, img, prelude::*, px,
};

use dnnm_application::iis::{IisServerAction, IisServerState};
use dnnm_application::settings::ThemeChoice;

mod actions;
mod background;
mod commands;
mod help;
mod modals;
mod shortcuts;
mod updates;

pub use actions::ProjectAction;
pub use shortcuts::RunCommand;
use modals::Modal;

use crate::pages::projects::{ProjectsEvent, ProjectsPage};
use crate::logs_view::LogsView;
use crate::palette::{Mode, Palette, PaletteClosed, PaletteItem};
use dnnm_terminal::{PanelColors, TerminalPanelEvent, TerminalPanelView, TerminalStyle};
use crate::workspace_state::{WindowState, WorkspaceState};
use crate::runner::LineKind;
use crate::store::{Run, RunStatus, Store, ToastKind};
use dnnm_ui::theme::Theme;
use dnnm_ui::icons::{self, LayoutIcon};
use dnnm_ui::{self as ui, Pad, Tooltip};

actions!(
    workspace,
    [
        ToggleSidebar,
        TogglePanel,
        GoToProjects,
        GoToNewProject,
        OpenSettings,
        CloseView,
        SearchProjects,
        ShowOutput,
        ShowCommands,
        QuickOpen,
        ShowTerminal,
        NewTerminal,
        MaximizePanel
    ]
);

/// The default key bindings and the user's changes to them.
pub fn apply_shortcuts(settings: &dnnm_application::settings::Settings, cx: &mut App) {
    shortcuts::apply(settings, cx);
}

/// The default shortcuts (`AppCommands`).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-j", TogglePanel, None),
        KeyBinding::new("ctrl-1", GoToProjects, None),
        KeyBinding::new("ctrl-2", GoToNewProject, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-w", CloseView, None),
        KeyBinding::new("escape", CloseView, Some("Modal")),
        KeyBinding::new("escape", CloseView, Some("Details")),
        KeyBinding::new("ctrl-shift-f", SearchProjects, None),
        KeyBinding::new("ctrl-shift-u", ShowOutput, None),
        KeyBinding::new("ctrl-shift-p", ShowCommands, None),
        KeyBinding::new("ctrl-p", QuickOpen, None),
        KeyBinding::new("ctrl-`", ShowTerminal, None),
        KeyBinding::new("ctrl-shift-m", MaximizePanel, None),
        KeyBinding::new("ctrl-shift-`", NewTerminal, None),
        KeyBinding::new("f1", RunCommand { id: "help.page".into() }, None),
        KeyBinding::new("ctrl-tab", RunCommand { id: "view.next".into() }, None),
        KeyBinding::new("ctrl-shift-tab", RunCommand { id: "view.previous".into() }, None),
    ]);
}

const APP_ICON: &[u8] = include_bytes!("../assets/dnn-32.png");
/// Below this width the sidebar shows only its icons (`MainWindow.IsCompact`).
const NARROW_WINDOW: f32 = 1100.0;

/// The bottom panel's tabs (`TerminalPanel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Output,
    Logs,
    Terminal,
}

/// A sash being dragged (`Controls/Sash`).
#[derive(Debug, Clone, Copy)]
enum Drag {
    Sidebar { start_x: gpui::Pixels, start_width: gpui::Pixels },
    Panel { start_y: gpui::Pixels, start_height: gpui::Pixels },
}

/// What runs once the What's new dialog at the start has gone (the guide).
type AfterNotes = Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// The sidebar is dragged between these (`MainWindow.Layout.cs`); under half the smallest it closes.
const MIN_SIDEBAR: f32 = 160.0;
const MAX_SIDEBAR: f32 = 600.0;
const MIN_PANEL: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Projects,
    NewProject,
}

pub struct Workspace {
    store: Entity<Store>,
    page: Page,
    projects: Entity<ProjectsPage>,
    /// New project - made the first time it is shown.
    setup: Option<Entity<dnnm_dialogs::setup_page::SetupPage>>,
    /// The IIS read New project last got (`Store::sites_read`).
    setup_sites_read: u64,
    /// Settings or Troubleshoot, open over the page.
    settings: Option<Modal>,
    sidebar_visible: bool,
    panel_visible: bool,
    /// The gear's or Help's menu, while open.
    menu: Option<dnnm_ui::menu::ContextMenu>,
    /// The tour, while it runs; the parts it points at record where they are drawn.
    tour: Option<(Entity<dnnm_dialogs::tour::GuidedTour>, Subscription)>,
    tour_targets: dnnm_dialogs::tour::TourTargets,
    panel_before_tour: bool,
    /// Off while a factory reset removes the workspace: nothing is saved over it.
    keep_workspace: bool,
    /// What Check for Updates found (About's Update line), when, and a newer release.
    update_status: dnnm_dialogs::about::UpdateStatus,
    update_checked: Option<std::time::Instant>,
    update_available: Option<dnnm_system::updates::AppRelease>,
    /// The notification area's icon, while "Keep DNN Manager running when you close the window" is on.
    tray: Option<dnnm_tray::TrayIcon>,
    /// The window is hidden (running in the background).
    hidden: bool,
    /// Closing quits now rather than hiding; quitting once the cancelled operation has stopped; an unsaved password
    /// may be lost.
    quitting: bool,
    quit_when_idle: bool,
    password_may_go: bool,
    /// The store's failures already seen.
    failures_seen: u64,
    /// What follows the What's new dialog at the start (the guide).
    after_notes: Option<AfterNotes>,
    /// The command palette, while it is open.
    palette: Option<Entity<Palette>>,
    /// The dialog shown over the window (one at a time, as WPF's modal windows), and what listens to it.
    dialog: Option<(gpui::AnyView, Subscription)>,
    panel_tab: PanelTab,
    /// The sidebar's and the panel's sizes (dragged), the panel taking the page's room (Ctrl+Shift+M).
    sidebar_width: Option<gpui::Pixels>,
    panel_height: Option<gpui::Pixels>,
    panel_maximized: bool,
    /// A drag of the sidebar's edge or the panel's: where it began and the size then.
    drag: Option<Drag>,
    logs: Entity<LogsView>,
    /// The Terminal tab - once the shells on this PC are known (read off the UI thread at start).
    terminal: Option<Entity<TerminalPanelView>>,
    focus: FocusHandle,
    icon: Arc<Image>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let db = store.read(cx).db().clone();
        let store_for_logs = store.clone();
        let saved_window = WindowState::load(&db);
        let saved = WorkspaceState::load(&db);
        let tour_targets = dnnm_dialogs::tour::TourTargets::new();
        let projects = cx.new(|cx| {
            let mut page = ProjectsPage::new(store.clone(), cx);
            page.restore(&saved.projects, cx);
            page.tour_targets = Some(tour_targets.clone());
            page
        });
        let page_events = cx.subscribe_in(&projects, window, |this, _, e: &ProjectsEvent, window, cx| match e {
            ProjectsEvent::OpenTerminal(dir) => this.new_terminal(Some(dir.display().to_string()), false, window, cx),
            ProjectsEvent::ShowLogs(site) => this.show_logs(crate::logs_view::LogSite::Site(site.clone()), cx),
            ProjectsEvent::Action(action, site) => this.project_action(*action, site.clone(), window, cx),
            ProjectsEvent::Run(run, site) => this.project_run(run.clone(), site.clone(), cx),
        });
        // Closing keeps the workspace for the next start (`WorkspaceService.SaveNow`).
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |ws, cx| ws.close_requested(window, cx)).unwrap_or(true)
        });
        let subscriptions = vec![
            page_events,
            cx.observe_in(&store, window, |this, store, window, cx| {
                // The theme follows the settings (the gear's Themes, Settings → General).
                this.apply_theme(store.read(cx).settings.theme(), None, cx);
                // Quitting once the cancelled operation has stopped and undone itself.
                if this.quit_when_idle && !store.read(cx).is_busy() {
                    this.quit_when_idle = false;
                    this.quit_cleanly(window, cx);
                    cx.quit();
                    return;
                }
                let failures = store.read(cx).failures;
                if failures != this.failures_seen {
                    this.failures_seen = failures;
                    if let Some((title, error)) = store.read(cx).last_failure.clone() {
                        this.notice_failure(&title, &error);
                    }
                }
                let read = store.read(cx).sites_read;
                if let Some(setup) = this.setup.clone()
                    && read != this.setup_sites_read
                {
                    this.setup_sites_read = read;
                    let sites = store.read(cx).all_sites.clone();
                    setup.update(cx, |p, cx| p.set_sites(sites, cx));
                }
                cx.notify();
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                let choice = this.store.read(cx).settings.theme();
                this.apply_theme(choice, Some(window.appearance()), cx);
                cx.notify();
            }),
        ];
        cx.spawn_in(window, async move |this, cx| {
            let shells = cx.background_executor().spawn(async { dnnm_terminal::find_shells() }).await;
            let _ = this.update_in(cx, |ws, window, cx| ws.make_terminal(shells, window, cx));
        })
        .detach();
        if let Some(name) = saved.project.clone().filter(|_| saved.page.as_deref() != Some("Setup")) {
            let tab = saved.project_tab.clone();
            projects.update(cx, |p, cx| p.restore_details(&name, tab.as_deref(), window, cx));
        }
        let focus = cx.focus_handle();
        window.focus(&focus);
        // DNN Manager has never run here: no version and no window place saved (`RestoreWorkspace`).
        let first_start = saved_window.width.is_none() && db.state("version", "lastRun").ok().flatten().is_none();
        let reopen = saved.page.clone();
        cx.defer_in(window, move |ws, window, cx| {
            match reopen.as_deref() {
                Some("Settings") => ws.open_settings_on(None, window, cx),
                Some("Troubleshoot") => ws.open_troubleshoot(window, cx),
                Some("Setup") => ws.go(Page::NewProject, window, cx),
                _ => {}
            }
            // The fixture is a UI check: no icon, nothing deleted, no onboarding state written.
            if ws.store.read(cx).real_system {
                ws.follow_keep_running(window, cx);
                ws.checks_at_start(cx);
                ws.help_at_start(first_start, window, cx);
            }
        });
        Self {
            store,
            page: Page::Projects,
            projects,
            setup: None,
            setup_sites_read: 0,
            settings: None,
            sidebar_visible: !saved_window.sidebar_hidden,
            panel_visible: saved_window.panel_open,
            menu: None,
            tour: None,
            tour_targets,
            panel_before_tour: false,
            keep_workspace: true,
            update_status: dnnm_dialogs::about::UpdateStatus::default(),
            update_checked: None,
            update_available: None,
            after_notes: None,
            tray: None,
            hidden: false,
            quitting: false,
            quit_when_idle: false,
            password_may_go: false,
            failures_seen: 0,
            palette: None,
            dialog: None,
            panel_tab: PanelTab::Output,
            sidebar_width: saved_window.sidebar_width.map(px),
            panel_height: saved_window.panel_height.map(px),
            panel_maximized: saved_window.panel_maximized,
            drag: None,
            logs: cx.new(|_| LogsView::new(store_for_logs)),
            terminal: None,
            focus,
            icon: Arc::new(Image::from_bytes(ImageFormat::Png, APP_ICON.to_vec())),
            _subscriptions: subscriptions,
        }
    }

    /// The Terminal tab, with the shells found on this PC and the terminal settings.
    fn make_terminal(&mut self, shells: dnnm_terminal::Shells, window: &mut Window, cx: &mut Context<Self>) {
        // No shell can be offered: no Terminal tab, as in WPF.
        if shells.user.is_empty() {
            return;
        }
        let settings = self.store.read(cx).settings.clone();
        let default_shell = settings.get("terminal.defaultShell").unwrap_or(dnnm_terminal::shells::DEFAULT_SHELL).to_string();
        let (style, colors) = terminal_style(&settings, cx);
        let projects_folder = settings.base_directory();
        let panel = cx.new(|cx| TerminalPanelView::new(shells, default_shell, projects_folder, style, colors, cx));
        let sub = cx.subscribe_in(&panel, window, |this, _, e: &TerminalPanelEvent, _, cx| match e {
            TerminalPanelEvent::Error(message) => {
                let message = message.clone();
                this.store.update(cx, |s, cx| s.toast(ToastKind::Error, message, false, cx));
            }
        });
        self._subscriptions.push(sub);
        self.terminal = Some(panel);
        cx.notify();
    }

    /// The Logs tab on a website's logs - DNN Manager's own, or a project's ("Show project logs").
    pub(crate) fn show_logs(&mut self, site: crate::logs_view::LogSite, cx: &mut Context<Self>) {
        self.panel_visible = true;
        self.panel_tab = PanelTab::Logs;
        self.logs.update(cx, |l, cx| l.show_site(site, cx));
        cx.notify();
    }

    /// Ctrl+`: the panel on the Terminal tab - a terminal started when none is open.
    pub(crate) fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal.clone() else { return };
        self.panel_visible = true;
        self.panel_tab = PanelTab::Terminal;
        terminal.update(cx, |t, cx| t.show_terminal(window, cx));
        cx.notify();
    }

    /// A new terminal - in `directory` (a project's folder) or the projects folder; `admin`: with administrator rights.
    pub(crate) fn new_terminal(&mut self, directory: Option<String>, admin: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal.clone() else { return };
        self.panel_visible = true;
        self.panel_tab = PanelTab::Terminal;
        terminal.update(cx, |t, cx| {
            if admin {
                t.new_administrator_terminal(directory, window, cx)
            } else {
                t.new_terminal(None, directory, window, cx)
            }
        });
        cx.notify();
    }

    /// Keeps the window, the layout, the page and the table for the next start.
    fn save_workspace(&self, window: &Window, cx: &App) {
        if !self.keep_workspace {
            return;
        }
        let db = self.store.read(cx).db().clone();
        let (maximized, bounds) = match window.window_bounds() {
            WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (true, b),
            WindowBounds::Windowed(b) => (false, b),
        };
        let mut state = WindowState::load(&db);
        state.left = Some(bounds.origin.x / px(1.0));
        state.top = Some(bounds.origin.y / px(1.0));
        state.width = Some(bounds.size.width / px(1.0));
        state.height = Some(bounds.size.height / px(1.0));
        state.maximized = maximized;
        state.sidebar_hidden = !self.sidebar_visible;
        state.panel_open = self.panel_visible;
        state.sidebar_width = self.sidebar_width.map(|w| w / px(1.0));
        state.panel_height = self.panel_height.map(|h| h / px(1.0));
        state.panel_maximized = self.panel_maximized;
        state.save(&db);
        let mut workspace = WorkspaceState::load(&db);
        workspace.page = Some(match (&self.settings, self.page) {
            (Some(modal), _) => modal.key().into(),
            (None, Page::Projects) => "Projects".into(),
            (None, Page::NewProject) => "Setup".into(),
        });
        workspace.projects = self.projects.read(cx).capture(cx);
        let details = self.projects.read(cx).details_of(cx);
        workspace.project = details.as_ref().map(|(name, _)| name.clone());
        workspace.project_tab = details.map(|(_, tab)| tab);
        workspace.save(&db);
    }

    /// Rebuilds the theme when the choice, the font size or Windows' light/dark mode changed.
    fn apply_theme(&mut self, choice: ThemeChoice, appearance: Option<WindowAppearance>, cx: &mut Context<Self>) {
        let settings = &self.store.read(cx).settings;
        let appearance = appearance.unwrap_or_else(|| cx.window_appearance());
        let next = Theme::new(choice, appearance, settings.font_size(), settings.density());
        let current = Theme::get(cx);
        if current.dark != next.dark || current.text.body != next.text.body || current.density != next.density {
            cx.set_global(next);
            if let Some(terminal) = self.terminal.clone() {
                let (style, colors) = terminal_style(&self.store.read(cx).settings, cx);
                terminal.update(cx, |t, cx| t.set_style(style, colors, cx));
            }
            cx.refresh_windows();
        }
    }

    fn go(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal_then(window, cx, move |ws, window, cx| {
            let before = ws.page;
            ws.page = page;
            ws.store.update(cx, |s, _| s.projects_shown = page == Page::Projects);
            if page == Page::NewProject && before != page {
                let setup = ws.setup_page(window, cx);
                setup.update(cx, |p, cx| p.shown(cx));
            } else if before == Page::NewProject
                && page != before
                && let Some(setup) = ws.setup.clone()
            {
                setup.update(cx, |p, cx| p.hidden(cx));
            }
            cx.notify();
        });
    }

    /// New project (`SetupPage`), made the first time it is shown; what it asks for runs as an operation.
    fn setup_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<dnnm_dialogs::setup_page::SetupPage> {
        if let Some(setup) = &self.setup {
            return setup.clone();
        }
        use dnnm_dialogs::setup_page::{SetupPage, SetupPageEvent};
        let store = self.store.read(cx);
        let services = store.services();
        let lookups: Arc<dyn dnnm_application::use_cases::setup_project::NewProjectLookups> =
            Arc::new(dnnm_wiring::setup_project::page_lookups(&services));
        let (settings, sites) = (store.settings.clone(), store.all_sites.clone());
        self.setup_sites_read = store.sites_read;
        let page = cx.new(|cx| SetupPage::new(lookups, settings, sites, window, cx));
        let sub = cx.subscribe_in(&page, window, |ws, page, e: &SetupPageEvent, _, cx| {
            let services = ws.store.read(cx).services();
            let page = page.downgrade();
            match e.clone() {
                SetupPageEvent::CreateProject(request) => {
                    let title = dnnm_wiring::setup_project::setup_title(&request.project_name);
                    let toast = dnnm_wiring::setup_project::setup_done(&request);
                    ws.store.update(cx, |s, cx| {
                        s.run_then(
                            title,
                            Some(toast),
                            move |op| dnnm_wiring::setup_project::setup_project(&services, *request, op),
                            move |outcome, cx| {
                                if *outcome == dnnm_application::operation::Outcome::Finished {
                                    let _ = page.update(cx, |p, cx| p.created(cx));
                                }
                            },
                            cx,
                        )
                    });
                }
                SetupPageEvent::ImportProject(request) => {
                    let request = dnnm_application::use_cases::host_project::ImportRequest {
                        project_name: request.project_name,
                        zip_path: request.zip_path,
                        backup_file: request.backup_file_path,
                        host_name: Some(request.host_name).filter(|h| !h.trim().is_empty()),
                        port: request.port,
                    };
                    let title = dnnm_wiring::host_project::import_title(&request.project_name);
                    let toast = dnnm_wiring::host_project::import_done(&request.project_name);
                    ws.store.update(cx, |s, cx| {
                        s.run_then(
                            title,
                            Some(toast),
                            move |op| dnnm_wiring::host_project::import_project(&services, request, op),
                            move |outcome, cx| {
                                if *outcome == dnnm_application::operation::Outcome::Finished {
                                    let _ = page.update(cx, |p, cx| p.created(cx));
                                }
                            },
                            cx,
                        )
                    });
                }
            }
        });
        self._subscriptions.push(sub);
        self.setup = Some(page.clone());
        page
    }

    /// At the start, off the UI thread (`Program.DeleteExpiredBackups`, `AppUpdater`): backups older than
    /// `backups.keepDays` deleted, and GitHub asked for a newer release when `updates.checkAtStart` is on.
    fn checks_at_start(&mut self, cx: &mut Context<Self>) {
        let settings = self.store.read(cx).settings.clone();
        if settings.get("updates.checkAtStart").is_none_or(|v| v.eq_ignore_ascii_case("true")) {
            self.check_for_updates(None, false, cx);
        }
        cx.background_executor()
            .spawn(async move {
                let expired = dnnm_wiring::backups::delete_expired_backups(&settings);
                if let Some(info) = expired.info() {
                    log::info!("{info}");
                }
                if let Some(warning) = expired.warning() {
                    log::warn!("{warning}");
                }
            })
            .detach();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_on(None, window, cx);
    }

    /// Next / previous page (Ctrl+Tab): Projects ↔ New project.
    fn step_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if self.page == Page::Projects { Page::NewProject } else { Page::Projects };
        self.go(next, window, cx);
    }

    /// Whether Close this view has something to close: Settings or Troubleshoot, or a project's details.
    fn can_close_view(&self, cx: &App) -> bool {
        self.settings.is_some() || (self.page == Page::Projects && self.projects.read(cx).details_of(cx).is_some())
    }

    /// Close this view (Ctrl+W): Settings or Troubleshoot, else the details back to the table.
    fn close_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.close_modal(window, cx);
        } else {
            self.projects.update(cx, |p, cx| p.close_details(cx));
        }
    }

    fn resize_sidebar_by(&mut self, by: f32, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        let now = self.sidebar_width.unwrap_or(Theme::get(cx).metrics.sidebar_width);
        self.sidebar_visible = true;
        self.sidebar_width = Some((now + px(by)).clamp(px(MIN_SIDEBAR), px(MAX_SIDEBAR)));
        cx.notify();
    }

    fn resize_panel_by(&mut self, by: f32, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.panel_height.unwrap_or(Theme::get(cx).metrics.panel_height);
        let most = (window.viewport_size().height - px(160.0)).max(px(MIN_PANEL));
        self.panel_visible = true;
        self.panel_height = Some((now + px(by)).clamp(px(MIN_PANEL), most));
        cx.notify();
    }

    /// The gear's menu (`Manage_Click`): the palette, Settings, Keyboard Shortcuts, Themes, Customize Layout,
    /// Troubleshoot and Check for Updates - beside the gear, its bottom level with it.
    fn open_gear_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        use dnnm_ui::menu::MenuItem;
        let weak = cx.entity().downgrade();
        let settings = self.store.read(cx).settings.clone();
        let entry = |label: &'static str, id: &'static str| {
            let weak = weak.clone();
            MenuItem::new(label, move |window, cx| {
                let _ = weak.update(cx, |ws, cx| {
                    ws.menu = None;
                    ws.run_command(id, window, cx);
                });
            })
            .shortcut(shortcuts::shortcut_text(id, &settings))
        };
        let current = settings.theme();
        let themes = [("Dark", ThemeChoice::Dark), ("Light", ThemeChoice::Light), ("System", ThemeChoice::System)]
            .into_iter()
            .map(|(label, choice)| {
                let weak = weak.clone();
                let mark = if current == choice { "✓  " } else { "    " };
                MenuItem::new(format!("{mark}{label}"), move |_, cx| {
                    let _ = weak.update(cx, |ws, cx| ws.set_theme(choice, cx));
                })
            })
            .collect();
        let items = vec![
            entry("Command Palette…", "workbench.commandPalette"),
            MenuItem::Separator,
            entry("Settings", "settings.open"),
            entry("Keyboard Shortcuts", "settings.keyboard"),
            MenuItem::submenu("Themes", themes),
            entry("Customize Layout…", "layout.customize"),
            MenuItem::Separator,
            entry("Troubleshoot", "troubleshoot.open"),
            entry("Check for Updates…", "app.checkUpdates"),
        ];
        let mut menu = dnnm_ui::menu::ContextMenu::new(position, items);
        menu.anchor = gpui::Corner::BottomLeft;
        self.menu = Some(menu);
        cx.notify();
    }

    /// Opens the palette: the projects (Ctrl+P), or the commands (Ctrl+Shift+P).
    pub(crate) fn show_palette(&mut self, commands: bool, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let projects = self
            .store
            .read(cx)
            .projects
            .iter()
            .map(|p| {
                let name = p.name.clone();
                let weak = weak.clone();
                PaletteItem::new(p.name.clone(), move |window, cx| {
                    let name = name.clone();
                    let _ = weak.update(cx, |ws, cx| ws.go_to_project(&name, window, cx));
                })
                .detail(dnnm_application::project::view::url(p))
            })
            .collect();
        let command_items = self.command_items(cx);
        self.open_palette(Mode::Quick { projects, commands: command_items }, commands, window, cx);
    }

    fn open_palette(&mut self, mode: Mode, commands: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let centered = self.store.read(cx).settings.quick_input_centered();
        let palette = cx.new(|cx| {
            let mut palette = Palette::new(mode, commands, window, cx);
            palette.centered = centered;
            palette
        });
        let sub = cx.subscribe_in(&palette, window, |this, _, _: &PaletteClosed, window, cx| {
            this.palette = None;
            window.focus(&this.focus);
            cx.notify();
        });
        self._subscriptions.push(sub);
        self.palette = Some(palette);
        cx.notify();
    }

    /// "Go to project": its Details.
    fn go_to_project(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Page::Projects, window, cx);
        let name = name.to_string();
        self.projects.update(cx, |p, cx| p.open_details(&name, window, cx));
    }

    /// Shows a dialog over the window; `done` gets what it returns (OK), nothing on Cancel or Esc.
    pub(crate) fn show_dialog<V, T>(
        &mut self,
        view: Entity<V>,
        done: impl Fn(&mut Workspace, T, &mut Window, &mut Context<Workspace>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        V: Render + gpui::EventEmitter<dnnm_ui::dialog::DialogEvent<T>>,
        T: Clone + 'static,
    {
        self.menu = None;
        let sub = cx.subscribe_in(&view, window, move |this, _, e: &dnnm_ui::dialog::DialogEvent<T>, window, cx| {
            this.dialog = None;
            window.focus(&this.focus);
            if let dnnm_ui::dialog::DialogEvent::Done(value) = e {
                done(this, value.clone(), window, cx);
            }
            cx.notify();
        });
        self.dialog = Some((view.into(), sub));
        cx.notify();
    }

    /// Customize Layout (`MainWindow.Layout.cs`): the parts shown, the sidebar's side, where the panel reaches, where
    /// the palette opens, the density - a checklist that stays open; ↺ puts it all back.
    pub(crate) fn customize_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let refresh_weak = weak.clone();
        let refresh: crate::palette::RefreshItems =
            std::rc::Rc::new(move |cx| refresh_weak.upgrade().map(|ws| layout_items(&ws, cx)).unwrap_or_default());
        let reset_weak = weak.clone();
        let reset: std::rc::Rc<dyn Fn(&mut App)> = std::rc::Rc::new(move |cx| {
            let _ = reset_weak.update(cx, |ws, cx| {
                ws.sidebar_visible = true;
                ws.panel_visible = false;
                ws.store.update(cx, |s, cx| {
                    s.update_settings(
                        |st| {
                            for (key, value) in [
                                ("layout.menuBarVisible", "true"),
                                ("layout.statusBarVisible", "true"),
                                ("layout.sidebarPosition", "left"),
                                ("layout.panelAlignment", "center"),
                                ("layout.quickInputPosition", "top"),
                                ("layout.density", "default"),
                            ] {
                                st.set(key, value);
                            }
                        },
                        cx,
                    )
                });
                cx.notify();
            });
        });
        let items = layout_items_of(self, weak, cx);
        self.open_palette(
            Mode::Settings {
                title: "Customize Layout".into(),
                prompt: "Search layout options".into(),
                refresh,
                reset,
                items,
            },
            false,
            window,
            cx,
        );
    }

    fn search_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Page::Projects, window, cx);
        self.projects.update(cx, |p, cx| p.focus_search(window, cx));
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.panel_visible = !self.panel_visible;
        cx.notify();
    }

    fn show_output(&mut self, cx: &mut Context<Self>) {
        self.panel_visible = true;
        self.store.update(cx, |s, _| s.output_unseen_failure = false);
        cx.notify();
    }

    fn open_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Err(e) = dnnm_infrastructure::shell::open_url(url) {
            self.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
        }
    }

    fn open_folder(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Err(e) = dnnm_infrastructure::shell::open_folder(path) {
            self.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
        }
    }

    fn set_theme(&mut self, choice: ThemeChoice, cx: &mut Context<Self>) {
        self.menu = None;
        self.store.update(cx, |s, cx| s.update_settings(|settings| settings.set_theme(choice), cx));
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let narrow = window.viewport_size().width < px(NARROW_WINDOW);
        let gap = theme.metrics.card_gap;
        let compact_density = gap == px(0.0);
        let status_bar_visible = self.store.read(cx).settings.status_bar_visible();
        let has_prompt = self.store.read(cx).prompt.is_some();
        let sidebar_width = if narrow { theme.metrics.sidebar_compact_width } else { theme.metrics.sidebar_width };

        let page: gpui::AnyElement = match self.page {
            Page::Projects => self.projects.clone().into_any_element(),
            Page::NewProject => self.setup_page(window, cx).into_any_element(),
        };

        let card = |content: gpui::AnyElement, bg| {
            div()
                .size_full()
                .bg(bg)
                .overflow_hidden()
                .when(!compact_density, |d| d.rounded(theme.metrics.card_radius).border_1().border_color(c.card_border))
                .child(content)
        };

        let settings = self.store.read(cx).settings.clone();
        let sidebar_right = settings.sidebar_position() == dnnm_application::settings::SidebarPosition::Right;
        let panel_under_sidebar = match settings.panel_alignment() {
            dnnm_application::settings::PanelAlignment::Justify => true,
            dnnm_application::settings::PanelAlignment::Left => !sidebar_right,
            dnnm_application::settings::PanelAlignment::Right => sidebar_right,
            dnnm_application::settings::PanelAlignment::Center => false,
        };
        let sidebar_width = if narrow { sidebar_width } else { self.sidebar_width.unwrap_or(sidebar_width) };
        let panel_height = self.panel_height.unwrap_or(theme.metrics.panel_height);
        let maximized = self.panel_visible && self.panel_maximized;
        let sash_size = if compact_density { px(6.0) } else { gap };
        let sidebar = self.sidebar_visible.then(|| {
            div()
                .flex_none()
                // `sidebar_width` counts the gap to the window's side, which is the body's padding.
                .w(sidebar_width - gap)
                .h_full()
                .child(self.render_sidebar(narrow, cx))
        });
        // The sidebar's edge: dragged, its width; not while it shows only its icons.
        let sidebar_sash = (self.sidebar_visible && !narrow).then(|| {
            div()
                .id("sidebar-sash")
                .flex_none()
                .w(sash_size)
                .h_full()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(Drag::Sidebar { start_x: e.position.x, start_width: sidebar_width });
                        cx.stop_propagation();
                    }),
                )
        });
        let page_card = (!maximized).then(|| div().flex_1().min_h_0().min_w_0().child(card(page, c.window_bg)));
        let panel_part = self.panel_visible.then(|| {
            let panel = card(
                div()
                    .relative()
                    .size_full()
                    .child(self.render_panel(cx))
                    .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::Panel, &self.tour_targets))
                    .into_any_element(),
                c.log_bg,
            );
            div()
                .flex()
                .flex_col()
                .when(maximized, |d| d.flex_1().min_h_0())
                .when(!maximized, |d| d.flex_none().h(panel_height + sash_size))
                .child(
                    // The panel's edge: dragged, its height; closed when dragged far enough down.
                    div()
                        .id("panel-sash")
                        .flex_none()
                        .h(sash_size)
                        .w_full()
                        .when(compact_density, |d| d.border_b_1().border_color(c.card_border))
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                                this.drag = Some(Drag::Panel { start_y: e.position.y, start_height: panel_height });
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(div().flex_1().min_h_0().child(panel))
        });
        let bottom_gap = !self.panel_visible && !status_bar_visible;
        // The gap under the cards when neither the panel nor the status bar is there (the sides': the body's padding).
        let bottom_gutter = || div().flex_none().h(gap);
        let body = if panel_under_sidebar {
            let mut top = div().flex().flex_1().min_h_0();
            let page_col = div().flex().flex_col().flex_1().min_w_0().children(page_card);
            top = if sidebar_right {
                top.child(page_col).children(sidebar_sash).children(sidebar)
            } else {
                top.children(sidebar).children(sidebar_sash).child(page_col)
            };
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .pad(px(0.0), gap)
                .bg(c.sidebar_bg)
                .when(!maximized, |d| d.child(top))
                .when_some(panel_part, |d, panel| d.child(div().flex().flex_col().when(maximized, |d| d.flex_1().min_h_0()).child(panel)))
                .when(bottom_gap, |d| d.child(bottom_gutter()))
        } else {
            let column = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .children(page_card)
                .children(panel_part)
                .when(bottom_gap, |d| d.child(bottom_gutter()));
            let row = div().flex().flex_1().min_h_0().pad(px(0.0), gap).bg(c.sidebar_bg);
            if sidebar_right {
                row.child(column).children(sidebar_sash).children(sidebar)
            } else {
                row.children(sidebar).children(sidebar_sash).child(column)
            }
        };

        div()
            .id("workspace")
            .key_context(
                if self.settings.is_some() || has_prompt || self.dialog.is_some() || self.menu.is_some() {
                    "Workspace Modal"
                } else {
                    "Workspace"
                },
            )
            .track_focus(&self.focus)
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, window, cx| {
                let Some(drag) = this.drag else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    return;
                }
                match drag {
                    Drag::Sidebar { start_x, start_width } => {
                        let moved = if sidebar_right { start_x - e.position.x } else { e.position.x - start_x };
                        let wanted = start_width + moved;
                        if wanted < px(MIN_SIDEBAR / 2.0) {
                            this.sidebar_visible = false;
                            this.drag = None;
                        } else {
                            this.sidebar_width = Some(wanted.clamp(px(MIN_SIDEBAR), px(MAX_SIDEBAR)));
                        }
                    }
                    Drag::Panel { start_y, start_height } => {
                        let wanted = start_height + (start_y - e.position.y);
                        let most = window.viewport_size().height - px(160.0);
                        if wanted < px(MIN_PANEL / 2.0) {
                            this.panel_visible = false;
                            this.drag = None;
                        } else {
                            this.panel_height = Some(wanted.clamp(px(MIN_PANEL), most.max(px(MIN_PANEL))));
                        }
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
            .on_action(cx.listener(|this, _: &MaximizePanel, _, cx| {
                this.panel_visible = true;
                this.panel_maximized = !this.panel_maximized;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &TogglePanel, _, cx| this.toggle_panel(cx)))
            .on_action(cx.listener(|this, _: &ShowCommands, window, cx| this.show_palette(true, window, cx)))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.show_palette(false, window, cx)))
            .on_action(cx.listener(|this, _: &ShowTerminal, window, cx| this.show_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| this.new_terminal(None, false, window, cx)))
            .on_action(cx.listener(|this, _: &GoToProjects, window, cx| this.go(Page::Projects, window, cx)))
            .on_action(cx.listener(|this, _: &GoToNewProject, window, cx| this.go(Page::NewProject, window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, e: &RunCommand, window, cx| this.run_command(&e.id, window, cx)))
            .on_action(cx.listener(|this, _: &CloseView, window, cx| {
                if this.menu.take().is_some() {
                    cx.notify();
                    return;
                }
                if let Some(guide) = this.dialog.as_ref().and_then(|(v, _)| v.clone().downcast::<dnnm_dialogs::guide::GuideDialog>().ok()) {
                    // The guide says how it was left (its skip is remembered).
                    guide.update(cx, |g, cx| g.skip(cx));
                    return;
                }
                if this.dialog.take().is_some() {
                    window.focus(&this.focus);
                    cx.notify();
                    return;
                }
                if this.store.read(cx).prompt.is_some() {
                    this.store.update(cx, |s, cx| s.answer_prompt(None, cx));
                    return;
                }
                if this.settings.is_none() && this.projects.update(cx, |p, cx| p.close_details(cx)) {
                    return;
                }
                this.close_modal(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SearchProjects, window, cx| this.search_projects(window, cx)))
            .on_action(cx.listener(|this, _: &ShowOutput, _, cx| this.show_output(cx)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.sidebar_bg)
            .font_family(theme.font.clone())
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            .child(self.render_title_bar(window, cx))
            .child(body)
            .when(status_bar_visible, |d| d.child(self.render_status_bar(sidebar_width, cx)))
            .when_some(self.settings.as_ref().map(|m| m.view()), |d, view| {
                d.child(
                    div()
                        .id("modal-backdrop")
                        .absolute()
                        .top(theme.metrics.title_bar_height)
                        .bottom(if status_bar_visible { theme.metrics.status_bar_height } else { px(0.0) })
                        .left_0()
                        .right_0()
                        .bg(c.modal_backdrop)
                        .flex()
                        .justify_center()
                        .pad(px(0.0), px(40.0))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_modal(window, cx)))
                        .child(
                            // The box's column, the box in it 20 px under its top and 28 px over its bottom.
                            div().relative().w_full().max_w(px(1280.0)).child(
                                div()
                                    .id("modal-box")
                                    .absolute()
                                    .top(px(20.0))
                                    .bottom(px(28.0))
                                    .left_0()
                                    .right_0()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(c.card_border)
                                    .bg(c.window_bg)
                                    .shadow_xl()
                                    .overflow_hidden()
                                    .occlude()
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .child(view),
                            ),
                        ),
                )
            })
            .child(self.render_toasts(status_bar_visible, cx))
            .when_some(self.render_menu(cx), |d, menu| d.child(menu))
            .when_some(self.dialog.as_ref().map(|(view, _)| view.clone()), |d, view| {
                d.child(
                    div()
                        .id("dialog-backdrop")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(c.modal_backdrop)
                        .occlude()
                        .child(view),
                )
            })
            .when_some(self.palette.clone(), |d, palette| d.child(palette))
            .when_some(self.tour.as_ref().map(|(t, _)| t.clone()), |d, tour| d.child(tour))
            .when(has_prompt, |d| d.child(self.render_prompt(cx)))
    }
}

impl Workspace {
    /// The title bar: icon and name, the command center in the middle, Help and the layout buttons, then the window's.
    fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let h = theme.metrics.title_bar_height;
        let menu_bar = self.store.read(cx).settings.menu_bar_visible();
        let maximized = window.is_maximized();
        let sidebar_right =
            self.store.read(cx).settings.sidebar_position() == dnnm_application::settings::SidebarPosition::Right;
        let layout_button = |id: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(theme.metrics.layout_button_width)
                .h(theme.metrics.layout_button_height)
                .rounded(px(5.0))
                .cursor_pointer()
                .hover(move |s| s.bg(c.hover_bg))
                .tooltip(Tooltip::text(tip))
        };
        let caption = |id: &'static str, glyph: &'static str, area: WindowControlArea, hover| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(theme.metrics.caption_button_width)
                .h(h)
                .window_control_area(area)
                .hover(move |s| s.bg(hover))
                .child(ui::icon(glyph, 10.0, c.text_primary, cx))
        };
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(h)
            .bg(c.sidebar_bg)
            .when(theme.metrics.card_gap == px(0.0), |d| d.border_b_1().border_color(c.sidebar_border))
            .window_control_area(WindowControlArea::Drag)
            // The command center, centred on the window.
            .child(
                div().absolute().inset_0().flex().items_center().justify_center().child(
                    div()
                        .id("command-center")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .w(relative_width(window, 0.38, 600.0))
                        .h(px(22.0))
                        .pad(px(0.0), px(8.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(c.command_center_border)
                        .bg(c.command_center_bg)
                        .justify_center()
                        .text_size(theme.text.caption)
                        .text_color(c.command_center_fg)
                        .cursor_pointer()
                        .hover(move |s| s.bg(c.command_center_hover_bg).border_color(c.command_center_hover_border))
                        .tooltip(Tooltip::text("Search projects and commands"))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.show_palette(false, window, cx)))
                        .relative()
                        .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::CommandCenter, &self.tour_targets))
                        .child(ui::icon(icons::SEARCH, 12.0, c.command_center_fg, cx))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child("Search projects and commands"),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .pad(px(0.0), px(12.0))
                    .child(img(self.icon.clone()).size(px(16.0)))
                    .when(menu_bar, |d| {
                        d.child(
                            div()
                                .text_size(theme.text.caption)
                                .text_color(c.text_primary)
                                .child("DNN Manager"),
                        )
                    }),
            )
            .child(div().flex_1())
            .child(
                // Help, a line, the layout buttons, a line, the window's buttons.
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        layout_button("help", dnnm_dialogs::help_menu::HELP_BUTTON_TOOLTIP)
                            .relative()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.open_help_menu(gpui::point(e.position.x - px(8.0), px(30.0)), cx);
                                }),
                            )
                            .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::HelpButton, &self.tour_targets))
                            .child(ui::icon(icons::QUESTION, 13.0, c.text_primary, cx)),
                    )
                    .child(div().w(px(1.0)).h(px(16.0)).bg(c.sidebar_divider))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                layout_button("customize-layout", "Customize Layout…")
                                    .on_click(
                                        cx.listener(|this, _: &ClickEvent, window, cx| this.customize_layout(window, cx)),
                                    )
                                    .child(icons::layout_icon(LayoutIcon::Customize, false, c.text_primary)),
                            )
                            .child(
                                layout_button(
                                    "toggle-sidebar",
                                    if self.sidebar_visible {
                                        "Hide the pages on the left"
                                    } else {
                                        "Show the pages on the left"
                                    },
                                )
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.sidebar_visible = !this.sidebar_visible;
                                    cx.notify();
                                }))
                                .child(icons::layout_icon(
                                    if sidebar_right { LayoutIcon::SidebarRight } else { LayoutIcon::SidebarLeft },
                                    self.sidebar_visible,
                                    c.text_primary,
                                )),
                            )
                            .child(
                                layout_button(
                                    "toggle-panel",
                                    if self.panel_visible { "Hide the panel" } else { "Show the panel: Output" },
                                )
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.panel_visible = !this.panel_visible;
                                    cx.notify();
                                }))
                                .child(icons::layout_icon(
                                    LayoutIcon::Panel,
                                    self.panel_visible,
                                    c.text_primary,
                                )),
                            ),
                    )
                    .child(div().w(px(1.0)).h(px(16.0)).bg(c.sidebar_divider))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(caption("minimize", icons::MINIMIZE, WindowControlArea::Min, c.hover_bg))
                            .child(caption(
                                "maximize",
                                if maximized { icons::RESTORE } else { icons::MAXIMIZE },
                                WindowControlArea::Max,
                                c.hover_bg,
                            ))
                            .child(caption("close", icons::CLOSE, WindowControlArea::Close, c.close_hover)),
                    ),
            )
    }

    /// The sidebar: Projects and New project; the gear at the bottom. Icons only in a narrow window.
    fn render_sidebar(&self, narrow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let compact_density = theme.metrics.card_gap == px(0.0);
        let sidebar_right =
            self.store.read(cx).settings.sidebar_position() == dnnm_application::settings::SidebarPosition::Right;
        // How far the sidebar's inner edge is from the window's side.
        let width = if narrow {
            theme.metrics.sidebar_compact_width
        } else {
            self.sidebar_width.unwrap_or(theme.metrics.sidebar_width)
        };
        let sidebar_edge = width + if compact_density { px(0.0) } else { theme.metrics.card_gap };
        // The gear's menu opens upwards from it; Help's opens down from the title bar.
        let gear_open = self.menu.as_ref().is_some_and(|m| m.anchor != gpui::Corner::TopLeft);
        let entry = |id: &'static str, glyph: &'static str, label: &'static str, tip: &'static str, selected: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.0))
                .pad(px(9.0), px(if narrow { 3.0 } else { 7.0 }))
                .rounded(theme.metrics.card_radius)
                .cursor_pointer()
                .text_size(theme.text.medium)
                .text_color(if selected { c.sidebar_active_fg } else { c.sidebar_fg })
                .when(selected, |d| d.bg(c.sidebar_selected))
                .when(!selected, |d| d.hover(move |s| s.bg(c.sidebar_hover)))
                .tooltip(Tooltip::text(tip))
                .child(div().flex().justify_center().w(px(30.0)).child(ui::icon(
                    glyph,
                    16.0,
                    if selected { c.sidebar_active_fg } else { c.sidebar_fg },
                    cx,
                )))
                .when(!narrow, |d| d.child(div().child(label)))
        };
        // The entries' groups: 9 px over the first and under the last, 2 px between them, inset from the sides.
        let group = || div().flex().flex_col().gap(px(2.0)).pad(px(9.0), px(if narrow { 6.0 } else { 8.0 }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(c.sidebar_bg)
            .when(!compact_density, |d| d.rounded(theme.metrics.card_radius).border_1().border_color(c.card_border))
            .when(compact_density, |d| d.border_r_1().border_color(c.sidebar_border))
            .child(
                group()
                    .relative()
                    .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::SidebarPages, &self.tour_targets))
                    .child(
                        entry(
                            "nav-projects",
                            icons::LIST,
                            "Projects",
                            "Projects - every website on this computer (Ctrl+1)",
                            self.page == Page::Projects,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.go(Page::Projects, window, cx))),
                    )
                    .child(
                        entry(
                            "nav-new",
                            icons::ADD,
                            "New project",
                            "New project - create a website, or one from a backup (Ctrl+2)",
                            self.page == Page::NewProject,
                        )
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| this.go(Page::NewProject, window, cx)),
                        ),
                    ),
            )
            .child(group().child(
                entry("nav-gear", icons::SETTINGS, "Settings", "Settings, themes, Troubleshoot and updates", gear_open)
                    .relative()
                    .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::ManageButton, &self.tour_targets))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            if this.menu.take().is_none() {
                                // Beside the gear - right of the sidebar (left of it on the right) - its bottom level
                                // with the gear's (`Manage_Click`).
                                let bottom = e.position.y + px(17.0);
                                let (x, corner) = if sidebar_right {
                                    (window.viewport_size().width - sidebar_edge - px(6.0), gpui::Corner::BottomRight)
                                } else {
                                    (sidebar_edge + px(6.0), gpui::Corner::BottomLeft)
                                };
                                this.open_gear_menu(gpui::point(x, bottom), cx);
                                if let Some(menu) = this.menu.as_mut() {
                                    menu.anchor = corner;
                                }
                            }
                            cx.notify();
                        }),
                    ),
            ))
    }

    /// The running operation's question (`MessageDialog`): its glyph - a question, or a warning in red when the yes
    /// deletes or replaces something -, the text, and its answers; Esc and a click beside it answer no.
    fn render_prompt(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let Some(prompt) = store.prompt.as_ref() else { return div().into_any_element() };
        let (glyph, glyph_color) =
            if prompt.danger { (icons::WARNING, c.error_text) } else { (icons::QUESTION, c.accent) };
        let question = prompt.question.clone();
        let answers = prompt.answers.clone();
        let danger = prompt.danger;
        let mut buttons = div().flex().justify_end().gap(px(8.0));
        for (i, answer) in answers.into_iter().enumerate() {
            let kind = match (i, danger) {
                (0, true) => ui::ButtonKind::Danger,
                (0, false) => ui::ButtonKind::Primary,
                _ => ui::ButtonKind::Secondary,
            };
            let choice = Some(i);
            buttons =
                buttons.child(ui::button(SharedString::from(format!("prompt-{i}")), answer, kind, true, cx).on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.store.update(cx, |s, cx| s.answer_prompt(choice, cx));
                    }),
                ));
        }
        div()
            .id("prompt-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.modal_backdrop)
            .occlude()
            .child(
                div()
                    .id("prompt")
                    .w(px(480.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(c.card_border)
                    .bg(c.window_bg)
                    .shadow_xl()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(14.0))
                            .pad(px(19.0), px(20.0))
                            .child(ui::icon(glyph, 26.0, glyph_color, cx))
                            .child(
                                div().id("prompt-text").flex_1().max_h(px(360.0)).overflow_y_scroll().child(question),
                            ),
                    )
                    .child(
                        div()
                            .pad(px(12.0), px(20.0))
                            .border_t_1()
                            .border_color(c.card_border)
                            .bg(c.card_bg)
                            .child(buttons),
                    ),
            )
            .into_any_element()
    }

    /// The gear's or Help's menu, with a backdrop that closes it.
    fn render_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.menu.as_ref()?;
        let (close, hover) = (cx.entity().downgrade(), cx.entity().downgrade());
        Some(
            div()
                .id("menu-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }),
                )
                .child(dnnm_ui::menu::render(
                    menu,
                    move |_, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.menu = None;
                            cx.notify();
                        });
                    },
                    move |open, _, cx| {
                        let _ = hover.update(cx, |this, cx| {
                            if let Some(menu) = this.menu.as_mut()
                                && menu.open_submenu != open
                            {
                                menu.open_submenu = open;
                                cx.notify();
                            }
                        });
                    },
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// The bottom panel's Output tab (`Controls/PipelineView`): the last operation's status, title and lines.
    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let unseen = store.output_unseen_failure;
        let run = store.runs.last().cloned();
        let busy = store.is_busy();

        let tab = |id: &'static str, label: &'static str, tip: &'static str, chosen: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(5.0))
                .pad(px(3.0), px(8.0))
                .rounded(theme.metrics.control_radius)
                .cursor_pointer()
                .text_size(theme.text.caption)
                .when(chosen, |d| d.bg(c.sidebar_selected).text_color(c.log_header_fg))
                .when(!chosen, |d| d.text_color(c.log_muted).hover(move |s| s.bg(c.hover_bg).text_color(c.log_header_fg)))
                .tooltip(Tooltip::text(tip))
                .child(label)
        };
        let on_terminal = self.panel_tab == PanelTab::Terminal && self.terminal.is_some();
        let on_logs = self.panel_tab == PanelTab::Logs;
        // The logs are followed only while they are seen (`LogsView.Paused`).
        let logs_seen = on_logs && self.panel_visible;
        self.logs.update(cx, |l, _| l.shown = logs_seen);
        let tabs = div()
            .flex()
            .items_center()
            .h(px(35.0))
            .pad(px(0.0), px(8.0))
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .pad(px(0.0), px(2.0))
                    .child(
                        tab("tab-output", "Output", "Output - what DNN Manager does, stage by stage", !on_terminal && !on_logs)
                            .when(unseen, |d| d.child(div().size(px(6.0)).rounded_full().bg(c.out_error)))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.panel_tab = PanelTab::Output;
                                this.store.update(cx, |s, _| s.output_unseen_failure = false);
                                cx.notify();
                            })),
                    )
                    .child(
                        tab(
                            "tab-logs",
                            "Logs",
                            "Logs - what a website, IIS, Windows or DNN Manager wrote down; look here when something goes wrong",
                            on_logs,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.panel_tab = PanelTab::Logs;
                            cx.notify();
                        })),
                    )
                    .when(self.terminal.is_some(), |d| {
                        d.child(
                            tab("tab-terminal", "Terminal", "Terminal - a command line, for advanced users", on_terminal)
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.show_terminal(window, cx))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .when_some(self.terminal.clone().filter(|_| on_terminal), |d, terminal| {
                        d.child(TerminalPanelView::toolbar(&terminal, cx))
                    })
                    .when(!on_terminal && !on_logs, |d| {
                        d.child(
                            ui::icon_button("clear-output", icons::DELETE, c.text_primary, !busy, cx)
                                .tooltip(Tooltip::text("Clear output"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.store.update(cx, |s, cx| s.clear_output(cx));
                                })),
                        )
                    })
                    .child(
                        ui::icon_button("hide-panel", icons::CLOSE, c.text_primary, true, cx)
                            .tooltip(Tooltip::text("Hide panel (Ctrl+J)"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.panel_visible = false;
                                cx.notify();
                            })),
                    ),
            );

        if on_logs {
            return div().size_full().flex().flex_col().child(tabs).child(div().flex_1().min_h_0().child(self.logs.clone()));
        }
        if let Some(terminal) = self.terminal.clone().filter(|_| on_terminal) {
            return div().size_full().flex().flex_col().child(tabs).child(div().flex_1().min_h_0().child(terminal));
        }
        let content = match run {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.log_muted)
                .child("No operation has run yet - what DNN Manager does shows here, stage by stage."),
            Some(run) => self.render_run(&run, cx),
        };
        div().size_full().flex().flex_col().child(tabs).child(content)
    }

    /// One operation in the Output tab (`PipelineView`): the header - status pill, title, issues, time and duration -,
    /// the stage rail with the facts and counts, and the lines.
    fn render_run(&self, run: &Run, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let (pill, pill_bg, pill_fg, pill_glyph) = match run.status {
            RunStatus::Running => ("RUNNING", c.out_pill_running_bg, c.out_pill_running_fg, icons::REFRESH),
            RunStatus::Finished => ("FINISHED", c.out_pill_ok_bg, c.out_pill_ok_fg, icons::CHECK),
            RunStatus::Failed => ("FAILED", c.out_pill_fail_bg, c.out_pill_fail_fg, icons::CLOSE),
            RunStatus::Cancelled => ("CANCELLED", c.out_pill_neutral_bg, c.out_pill_neutral_fg, icons::STOP),
        };
        let warnings = run.lines.iter().filter(|l| l.kind == LineKind::Warn).count();
        let errors = run.lines.iter().filter(|l| l.kind == LineKind::Fail).count();
        let ended = run.duration.map(|d| run.started + d);
        let time_range = match ended {
            Some(end) => format!("{} - {}", clock(run.started), clock(end)),
            None => format!("{} -", clock(run.started)),
        };
        let duration = run.duration.map(|d| format!("{:.1} s", d.as_secs_f32())).unwrap_or_default();
        let issues = match (warnings, errors) {
            (0, 0) => None,
            (w, 0) => Some((format!("{w} warning{}", if w == 1 { "" } else { "s" }), c.log_warn)),
            (0, e) => Some((format!("{e} error{}", if e == 1 { "" } else { "s" }), c.out_error)),
            (w, e) => Some((format!("{e} error{}, {w} warning{}", plural(e), plural(w)), c.out_error)),
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.0))
            .pad(px(0.0), px(10.0))
            .border_b_1()
            .border_color(c.card_border)
            .child(
                // The pill and the title 12 px apart, the issues 14 px after them.
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .pad(px(2.0), px(9.0))
                                    .rounded(px(10.0))
                                    .bg(pill_bg)
                                    .child(ui::icon(pill_glyph, 10.0, pill_fg, cx))
                                    .child(
                                        div()
                                            .text_size(theme.text.small)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(pill_fg)
                                            .child(pill),
                                    ),
                            )
                            .child(
                                div().font_weight(FontWeight::SEMIBOLD).text_color(c.out_heading).child(run.title.clone()),
                            ),
                    )
                    .when_some(issues, |d, (text, color)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(ui::icon(icons::WARNING, 11.0, color, cx))
                                .child(div().text_color(color).child(text)),
                        )
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .child(div().text_color(c.log_muted).child(time_range))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c.out_heading).child(duration)),
            );

        // The stage rail: each stage with how it went.
        // The heading 5 px over the stages, the stages 2 px apart.
        let mut stages = div().flex().flex_col().gap(px(5.0)).pad(px(10.0), px(4.0));
        let mut stage_rows = div().flex().flex_col().gap(px(2.0));
        let stage_list = run.stages();
        if !stage_list.is_empty() {
            stages = stages.child(
                div()
                    .pad(px(0.0), px(6.0))
                    .text_size(theme.text.small)
                    .text_color(c.log_muted)
                    .child(format!("STAGES ({})", stage_list.len())),
            );
        }
        for (name, state) in stage_list {
            let (glyph, color) = match state {
                LineKind::Fail => (icons::ERROR, c.out_error),
                LineKind::Warn => (icons::WARNING, c.log_warn),
                LineKind::Info => (icons::REFRESH, c.out_pill_running_fg),
                _ => (icons::CHECK, c.success_text),
            };
            stage_rows = stage_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .pad(px(4.0), px(6.0))
                    .rounded(theme.metrics.control_radius)
                    .child(div().flex().flex_none().justify_center().w(px(14.0)).child(ui::icon(glyph, 11.0, color, cx)))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(name)),
            );
        }
        let stages = stages.child(stage_rows);
        let summary_row = |key: SharedString, value: SharedString| {
            div()
                .flex()
                .w_full()
                .justify_between()
                .gap(px(8.0))
                .pad(px(1.0), px(0.0))
                .child(div().text_color(c.log_muted).overflow_hidden().text_ellipsis().child(key))
                .child(div().text_color(c.log_fg).child(value))
        };
        let mut summary = div()
            .flex()
            .flex_col()
            .pad(px(7.0), px(0.0))
            .border_t_1()
            .border_color(c.card_border);
        for (name, value) in &run.facts {
            summary = summary.child(summary_row(name.clone(), value.clone()));
        }
        summary = summary
            .child(summary_row("Warnings".into(), warnings.to_string().into()))
            .child(summary_row("Errors".into(), errors.to_string().into()));
        let rail = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(220.0))
            .h_full()
            .gap(px(10.0))
            .border_r_1()
            .border_color(c.card_border)
            .child(div().id("output-stages").flex_1().min_h_0().overflow_y_scroll().child(stages))
            // The summary's line stops 10 px short of the rail's sides.
            .child(div().flex_none().pad(px(0.0), px(10.0)).child(summary));

        // The lines in groups, each from a stage's line on, 6 px apart.
        let mut lines = div().flex().flex_col().gap(px(6.0)).pad(px(8.0), px(12.0)).font_family(theme.mono_font.clone());
        let mut lines_group: Option<gpui::Div> = None;
        for line in &run.lines {
            let color = match line.kind {
                LineKind::Stage => c.out_heading,
                LineKind::Info => c.log_fg,
                LineKind::Success => c.success_text,
                LineKind::Warn => c.log_warn,
                LineKind::Fail => c.log_fail,
            };
            if line.kind == LineKind::Stage
                && let Some(done) = lines_group.take()
            {
                lines = lines.child(done);
            }
            let group = lines_group.take().unwrap_or_else(|| div().flex().flex_col());
            lines_group = Some(
                group.child(
                    div()
                        .flex()
                        .gap(px(10.0))
                        .when(line.kind == LineKind::Stage, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(div().flex_none().text_color(c.log_muted).child(clock(line.at)))
                        .child(div().text_color(color).child(line.text.clone())),
                ),
            );
        }
        let lines = lines.children(lines_group);
        div().flex_1().min_h_0().flex().flex_col().child(header).child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(rail)
                .child(div().id("output-lines").flex_1().min_h_0().overflow_y_scroll().child(lines)),
        )
    }

    /// The status bar: IIS under the sidebar, this PC's figures, the version.
    fn render_status_bar(&self, sidebar_width: gpui::Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let state = store.iis_state;
        let busy = store.is_busy();
        let host = store.host.clone();
        let (text, color, filled) = match state {
            IisServerState::Running => ("IIS running", c.success_text, true),
            IisServerState::Stopped => ("IIS stopped", c.error_text, false),
            IisServerState::Starting => ("IIS starting…", c.log_warn, true),
            IisServerState::Stopping => ("IIS stopping…", c.log_warn, true),
            IisServerState::NotInstalled if !cfg!(windows) => ("IIS: Windows only", c.text_muted, false),
            IisServerState::NotInstalled => ("IIS not installed", c.text_muted, false),
            IisServerState::Unknown => ("IIS unknown", c.text_muted, false),
        };
        let iis_button = |id: &'static str, glyph: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(20.0))
                .rounded(px(3.0))
                .when(!busy, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                .when(busy, |d| d.opacity(0.35))
                .tooltip(Tooltip::text(tip))
                .child(ui::icon(glyph, 11.0, c.text_primary, cx))
        };
        let iis = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w(sidebar_width)
            .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::IisStatus, &self.tour_targets))
            .pad(px(0.0), px(10.0))
            .child(ui::status_dot(color, filled, px(8.0)))
            .child(div().text_color(color).child(text))
            .when(state == IisServerState::Running && !busy, |d| {
                d.child(
                    iis_button(
                        "iis-restart",
                        icons::REFRESH,
                        "Restart IIS - every website on this computer is unavailable for a few seconds; asks first",
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.store.update(cx, |s, cx| s.control_iis(IisServerAction::Restart, cx))
                    })),
                )
                .child(
                    iis_button(
                        "iis-stop",
                        icons::STOP,
                        "Stop IIS - every website on this computer stops until you start it again; asks first",
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.store.update(cx, |s, cx| s.control_iis(IisServerAction::Stop, cx))
                    })),
                )
            })
            .when(state == IisServerState::Stopped && !busy, |d| {
                d.child(
                    iis_button("iis-start", icons::PLAY, "Start IIS, the web server, so your websites can be opened")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.store.update(cx, |s, cx| s.control_iis(IisServerAction::Start, cx))
                        })),
                )
            });
        let fmt = dnnm_application::project::view::byte_size;
        let ram = if host.memory_total_bytes == 0 {
            format!("RAM {} GB", dnnm_application::project::view::decimal(0.0, 2))
        } else {
            format!("RAM {}", fmt(host.memory_used_bytes as f64))
        };
        let cpu = format!("CPU {}%", dnnm_application::project::view::decimal(host.cpu_percent.unwrap_or(0.0), 2));
        let disk = match &host.disk_root {
            None => "Disk: --.-- GB used (limit --.-- GB)".to_string(), // as WPF writes it, whatever the culture
            Some(_) => {
                format!("Disk: {} used (limit {})", fmt(host.disk_used_bytes as f64), fmt(host.disk_total_bytes as f64))
            }
        };
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(theme.metrics.status_bar_height)
            .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::StatusBar, &self.tour_targets))
            .bg(c.status_bar_bg)
            .when(theme.metrics.card_gap == px(0.0), |d| d.border_t_1().border_color(c.sidebar_border))
            .text_size(theme.text.caption)
            .text_color(c.text_primary)
            .child(iis)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(16.0))
                    .pad(px(0.0), px(10.0))
                    .child(
                        div()
                            .id("ram")
                            .tooltip(Tooltip::text(format!(
                                "Memory in use: {} of {}",
                                fmt(host.memory_used_bytes as f64),
                                fmt(host.memory_total_bytes as f64)
                            )))
                            .child(ram),
                    )
                    .child(
                        div()
                            .id("cpu")
                            .tooltip(Tooltip::text(format!(
                                "Processor use of this PC ({} logical processors)",
                                std::thread::available_parallelism().map_or(1, |n| n.get())
                            )))
                            .child(cpu),
                    )
                    .child(div().child(disk)),
            )
            .child(div().pad(px(0.0), px(10.0)).text_color(c.text_muted).child(concat!(
                "v",
                env!("CARGO_PKG_VERSION"),
                " (GPUI)"
            )))
    }

    /// The toasts, bottom right over the page: newest last.
    fn render_toasts(&self, status_bar: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let toasts = store.toasts.clone();
        let running = store.running_title();
        let undoing = store.undoing;
        let bottom = px(16.0)
            + if status_bar { theme.metrics.status_bar_height } else { px(0.0) }
            + if self.panel_visible { theme.metrics.panel_height + theme.metrics.card_gap } else { px(0.0) };
        let mut stack = div().absolute().right(px(24.0)).bottom(bottom).flex().flex_col().items_end().gap(px(8.0));
        if let Some(title) = running {
            stack = stack.child(
                div()
                    .id("operation-toast")
                    .w(px(340.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pad(px(10.0), px(14.0))
                    .rounded(px(6.0))
                    .bg(c.card_bg)
                    .border_1()
                    .border_color(c.card_border)
                    .shadow_lg()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.panel_visible = true;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div().font_weight(FontWeight::SEMIBOLD).overflow_hidden().text_ellipsis().child(title),
                            )
                            .child(
                                ui::button(
                                    "cancel-operation",
                                    if undoing { "Stop undoing" } else { "Cancel" },
                                    ui::ButtonKind::Danger,
                                    true,
                                    cx,
                                )
                                .min_h(px(24.0))
                                .pad(px(0.0), px(10.0))
                                .tooltip(Tooltip::text(if undoing {
                                    "Stop undoing - what isn't undone yet stays as it is"
                                } else {
                                    "Cancel - DNN Manager undoes what the operation did"
                                }))
                                .on_click(cx.listener(
                                    |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.store.update(cx, |s, cx| s.cancel_operation(cx));
                                    },
                                )),
                            ),
                    )
                    .child(div().pad(px(0.0), px(6.0)).child(div().h(px(2.0)).rounded(px(1.0)).bg(c.accent))),
            );
        }
        let waiting = toasts.len().saturating_sub(1);
        for toast in toasts.into_iter().take(1) {
            let (glyph, color) = match toast.kind {
                ToastKind::Success => (icons::CHECK, c.success_text),
                ToastKind::Warning => (icons::WARNING, c.log_warn),
                ToastKind::Error => (icons::ERROR, c.error_text),
            };
            let id = toast.id;
            stack = stack.child(
                div()
                    .id(SharedString::from(format!("toast-{id}")))
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .w(px(360.0))
                    .pad(px(10.0), px(14.0))
                    .rounded(px(6.0))
                    .bg(c.card_bg)
                    .border_1()
                    .border_color(c.card_border)
                    .shadow_lg()
                    .occlude()
                    .child(ui::icon(glyph, 14.0, color, cx))
                    .child(div().flex_1().flex().flex_col().gap(px(8.0)).child(toast.text.clone())
                            .when(waiting > 0, |d| {
                                d.child(
                                    div()
                                        .text_size(theme.text.caption)
                                        .text_color(c.text_muted)
                                        .child(format!("{waiting} more - shown when you close this one")),
                                )
                            })
                            .when(
                        toast.show_output,
                        |d| {
                            d.child(
                                ui::link(SharedString::from(format!("toast-out-{id}")), "Show output", cx).on_click(
                                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.panel_visible = true;
                                        this.store.update(cx, |s, cx| {
                                            s.output_unseen_failure = false;
                                            s.close_toast(id, cx);
                                        });
                                    }),
                                ),
                            )
                        },
                    ))
                    .child(
                        ui::icon_button(
                            SharedString::from(format!("toast-close-{id}")),
                            icons::CLOSE,
                            c.text_muted,
                            true,
                            cx,
                        )
                        .size(px(20.0))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.store.update(cx, |s, cx| s.close_toast(id, cx));
                        })),
                    ),
            );
        }
        stack.on_mouse_down(MouseButton::Left, |_, _, _| {})
    }
}

/// The Customize Layout checklist, as it stands now.
fn layout_items(ws: &Entity<Workspace>, cx: &App) -> Vec<PaletteItem> {
    layout_items_of(ws.read(cx), ws.downgrade(), cx)
}

/// The same, from the workspace at hand - while it is being updated it can't be read through its handle.
fn layout_items_of(w: &Workspace, weak: gpui::WeakEntity<Workspace>, cx: &App) -> Vec<PaletteItem> {
    let settings = w.store.read(cx).settings.clone();
    let setting = |key: &'static str, value: &'static str| {
        let weak = weak.clone();
        move |_: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |ws, cx| ws.store.update(cx, |s, cx| s.update_settings(|st| st.set(key, value), cx)));
        }
    };
    let get = |key: &str| settings.get(key).unwrap_or_default().to_ascii_lowercase();
    let mut items = Vec::new();
    let group = |items: &mut Vec<PaletteItem>, name: &str, list: Vec<PaletteItem>| {
        // The group's name shows on its first row, a line between groups (the palette's grouping).
        items.extend(list.into_iter().map(|item| item.group(name.to_string())));
    };
    let (sidebar, panel) = (w.sidebar_visible, w.panel_visible);
    let toggle_sidebar = weak.clone();
    let toggle_panel = weak.clone();
    group(
        &mut items,
        "Visibility",
        vec![
            PaletteItem::new("Menu Bar", setting("layout.menuBarVisible", if settings.menu_bar_visible() { "false" } else { "true" }))
                .checked(settings.menu_bar_visible()),
            PaletteItem::new("Sidebar", move |_, cx| {
                let _ = toggle_sidebar.update(cx, |ws, cx| ws.toggle_sidebar(cx));
            })
            .shortcut("Ctrl+B")
            .checked(sidebar),
            PaletteItem::new("Panel", move |_, cx| {
                let _ = toggle_panel.update(cx, |ws, cx| ws.toggle_panel(cx));
            })
            .shortcut("Ctrl+J")
            .checked(panel),
            PaletteItem::new(
                "Status Bar",
                setting("layout.statusBarVisible", if settings.status_bar_visible() { "false" } else { "true" }),
            )
            .checked(settings.status_bar_visible()),
        ],
    );
    let side = get("layout.sidebarPosition");
    group(
        &mut items,
        "Sidebar Position",
        vec![
            PaletteItem::new("Left", setting("layout.sidebarPosition", "left")).checked(side != "right"),
            PaletteItem::new("Right", setting("layout.sidebarPosition", "right")).checked(side == "right"),
        ],
    );
    let align = get("layout.panelAlignment");
    group(
        &mut items,
        "Panel Alignment",
        vec![
            PaletteItem::new("Left", setting("layout.panelAlignment", "left")).checked(align == "left"),
            PaletteItem::new("Right", setting("layout.panelAlignment", "right")).checked(align == "right"),
            PaletteItem::new("Center", setting("layout.panelAlignment", "center"))
                .checked(!matches!(align.as_str(), "left" | "right" | "justify")),
            PaletteItem::new("Justify", setting("layout.panelAlignment", "justify")).checked(align == "justify"),
        ],
    );
    let quick = get("layout.quickInputPosition");
    group(
        &mut items,
        "Quick Input Position",
        vec![
            PaletteItem::new("Top", setting("layout.quickInputPosition", "top")).checked(quick != "center"),
            PaletteItem::new("Center", setting("layout.quickInputPosition", "center")).checked(quick == "center"),
        ],
    );
    let density = get("layout.density");
    group(
        &mut items,
        "Layout Density",
        vec![
            PaletteItem::new("Default", setting("layout.density", "default")).checked(density != "compact"),
            PaletteItem::new("Compact", setting("layout.density", "compact")).checked(density == "compact"),
        ],
    );
    items
}

/// The terminal's look from the theme and the terminal settings (`terminal.fontFamily`, `terminal.fontSize`).
fn terminal_style(settings: &dnnm_application::settings::Settings, cx: &App) -> (TerminalStyle, PanelColors) {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let size = settings
        .get("terminal.fontSize")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(dnnm_terminal::shells::DEFAULT_FONT_SIZE);
    let style = TerminalStyle {
        font_families: dnnm_terminal::shells::font_families(settings.get("terminal.fontFamily").unwrap_or_default()),
        font_size: px(size),
        background: c.log_bg,
        foreground: c.log_fg,
        accent: c.accent,
        scroll_thumb: c.scroll_thumb,
        dark: theme.dark,
    };
    let colors = PanelColors {
        log_muted: c.log_muted,
        log_header_fg: c.log_header_fg,
        hover_bg: c.hover_bg,
        selection_bg: c.selection_bg,
        accent: c.accent,
        card_bg: c.card_bg,
        card_border: c.card_border,
        text_primary: c.text_primary,
        text_muted: c.text_muted,
        warn_banner_bg: c.warn_banner_bg,
        warn_banner_border: c.warn_banner_border,
        out_warn_text: c.log_warn,
        font: theme.font.clone(),
        icon_font: theme.icon_font.clone(),
        caption_size: theme.text.caption,
        body_size: theme.text.body,
    };
    (style, colors)
}

/// The command center's width: a share of the window, at most `max` (`SizeCommandCenter`).
fn relative_width(window: &Window, share: f32, max: f32) -> gpui::Pixels {
    let w = window.viewport_size().width * share;
    if w > px(max) { px(max) } else { w }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "14:03:27" in local time.
fn clock(at: SystemTime) -> String {
    dnnm_infrastructure::local_time::hms(at)
}
