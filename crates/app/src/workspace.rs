//! The window (`MainWindow.xaml`): the title bar with the command center and the layout and window buttons, the
//! sidebar, the page, the bottom panel, the status bar, the toasts, the gear's and Help's menus, Settings and
//! Troubleshoot over the page, the dialogs and the tour.

use dnnm_ui::px;
use dnnm_ui::{Icon, IconSize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, Image, ImageFormat, KeyBinding, MouseButton, SharedString, Subscription, Window, WindowAppearance, WindowBounds, WindowControlArea, actions, div, img, prelude::*};

use dnnm_application::iis::{IisServerAction, IisServerState};
use dnnm_application::settings::ThemeChoice;

mod actions;
mod background;
mod commands;
mod help;
mod modals;
mod servers;
mod shortcuts;
mod source_control;
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
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::keys::Os;
use dnnm_ui::{self as ui, Pad, Tip};

actions!(
    workspace,
    [
        ToggleSidebar,
        ToggleSecondarySidebar,
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
        MaximizePanel,
        FindInPanel,
        FindNext,
        FindPrevious,
        FindClose,
        FindToggleCase,
        FindToggleWord,
        FindToggleRegex,
        GoToSourceControl,
        HideApp,
        HideOtherApps,
        MinimizeWindow
    ]
);

/// The default key bindings and the user's changes to them.
pub fn apply_shortcuts(settings: &dnnm_application::settings::Settings, cx: &mut App) {
    shortcuts::apply(settings, cx);
}

/// Settings → General's UI scale for every size the app draws (dnnm-ui's and the terminal's `px`) - before a theme is
/// made from the settings, since its font sizes are sizes too.
pub fn apply_ui_scale(settings: &dnnm_application::settings::Settings) {
    ui::set_ui_scale(settings.ui_scale());
    dnnm_terminal::scale::set_ui_scale(ui::ui_scale());
}

/// The default shortcuts (`AppCommands`), as `commands` lists them for Windows. `secondary-` is GPUI's Ctrl, or Cmd on
/// macOS ([`Shortcut::default_on`](dnnm_dialogs::settings_keyboard::Shortcut::default_on)); Ctrl+Tab and Ctrl+` keep
/// Ctrl there too, as ⌘Tab and ⌘` are macOS's own.
pub fn bind_keys(cx: &mut App) {
    let find_option = |letter: char| find_option_key(letter, Os::CURRENT).0;
    cx.bind_keys([
        // As VS Code: Ctrl+B the primary side bar, Ctrl+Alt+B the secondary one.
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-alt-b", ToggleSecondarySidebar, None),
        // The panel's Find bar (`PanelSearch`): Ctrl+F opens it on the shown tab; in it, Enter / Shift+Enter go to the
        // next / previous match, Esc closes it, Alt+C / W / R switch Match Case, Whole Word, Regular Expression.
        KeyBinding::new("secondary-f", FindInPanel, None),
        KeyBinding::new("enter", FindNext, Some("PanelFind")),
        KeyBinding::new("shift-enter", FindPrevious, Some("PanelFind")),
        KeyBinding::new("escape", FindClose, Some("PanelFind")),
        KeyBinding::new(&find_option('c'), FindToggleCase, Some("PanelFind")),
        KeyBinding::new(&find_option('w'), FindToggleWord, Some("PanelFind")),
        KeyBinding::new(&find_option('r'), FindToggleRegex, Some("PanelFind")),
        KeyBinding::new("secondary-j", TogglePanel, None),
        KeyBinding::new("secondary-1", GoToProjects, None),
        KeyBinding::new("secondary-2", GoToNewProject, None),
        KeyBinding::new("secondary-3", GoToSourceControl, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-w", CloseView, None),
        KeyBinding::new("escape", CloseView, Some("Modal")),
        KeyBinding::new("escape", CloseView, Some("Details")),
        KeyBinding::new("secondary-shift-f", SearchProjects, None),
        KeyBinding::new("secondary-shift-u", ShowOutput, None),
        KeyBinding::new("secondary-shift-p", ShowCommands, None),
        KeyBinding::new("secondary-p", QuickOpen, None),
        KeyBinding::new("ctrl-`", ShowTerminal, None),
        KeyBinding::new("secondary-shift-m", MaximizePanel, None),
        KeyBinding::new("ctrl-shift-`", NewTerminal, None),
        KeyBinding::new("f1", RunCommand { id: "help.page".into() }, None),
        KeyBinding::new("ctrl-tab", RunCommand { id: "view.next".into() }, None),
        KeyBinding::new("ctrl-shift-tab", RunCommand { id: "view.previous".into() }, None),
    ]);
    // What every Mac app does: ⌘Q quits (Quit's default there - `commands` - so Settings shows it and it can be
    // changed), ⌘H hides it, ⌥⌘H the others, ⌘M minimizes the window.
    if Os::CURRENT == Os::Mac {
        cx.bind_keys([
            KeyBinding::new("cmd-q", RunCommand { id: "app.quit".into() }, None),
            KeyBinding::new("cmd-h", HideApp, None),
            KeyBinding::new("alt-cmd-h", HideOtherApps, None),
            KeyBinding::new("cmd-m", MinimizeWindow, None),
        ]);
    }
}

/// macOS's menu bar, which every Mac app has: the app's menu (Settings, Hide, Quit), Edit (for the text fields) and
/// Window. Each item shows the key bound to its action, so it is set again whenever the shortcuts change.
pub fn set_app_menus(cx: &mut App) {
    use dnnm_ui::text_input;
    use gpui::{Menu, MenuItem as Item, OsAction};
    cx.set_menus(vec![
        Menu {
            name: "DNN Manager".into(),
            items: vec![
                Item::action("Settings…", OpenSettings),
                Item::separator(),
                Item::action("Hide DNN Manager", HideApp),
                Item::action("Hide Others", HideOtherApps),
                Item::separator(),
                Item::action("Quit DNN Manager", RunCommand { id: "app.quit".into() }),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                Item::os_action("Undo", text_input::Undo, OsAction::Undo),
                Item::os_action("Redo", text_input::Redo, OsAction::Redo),
                Item::separator(),
                Item::os_action("Cut", text_input::Cut, OsAction::Cut),
                Item::os_action("Copy", text_input::Copy, OsAction::Copy),
                Item::os_action("Paste", text_input::Paste, OsAction::Paste),
                Item::os_action("Select All", text_input::SelectAll, OsAction::SelectAll),
            ],
        },
        Menu { name: "Window".into(), items: vec![Item::action("Minimize", MinimizeWindow)] },
    ]);
}

/// Why New project is locked now: off Windows (no IIS), or while an operation runs.
fn setup_lock(store: &Store) -> Option<SharedString> {
    dnnm_dialogs::setup_page::lock_reason(cfg!(windows), store.running_title().as_ref().map(|t| t.as_ref())).map(Into::into)
}

/// A switch of the Find bar's (`c`ase, `w`hole word, `r`egular expression): its key binding and how it is written -
/// Alt+C, or ⌥⌘C on macOS, where ⌥C alone types ç.
fn find_option_key(letter: char, os: Os) -> (String, String) {
    let mac = os == Os::Mac;
    let binding = if mac { format!("alt-cmd-{letter}") } else { format!("alt-{letter}") };
    (binding, dnnm_ui::keys::label(os, false, true, false, mac, &letter.to_ascii_uppercase().to_string()))
}

const APP_ICON: &[u8] = include_bytes!("../assets/dnn-32.png");

/// The layers over the page that hold the keyboard while open - Tab goes round their controls only.
struct Layers {
    /// Settings or Troubleshoot.
    modal: FocusHandle,
    dialog: FocusHandle,
    palette: FocusHandle,
    prompt: FocusHandle,
}

impl Layers {
    fn new(cx: &mut App) -> Self {
        Self { modal: cx.focus_handle(), dialog: cx.focus_handle(), palette: cx.focus_handle(), prompt: cx.focus_handle() }
    }
}

/// The bottom panel's tabs (`TerminalPanel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Output,
    Logs,
    Terminal,
}

/// A part that slides open and shut - a side bar, the panel: whether it was shown last frame, and since when it has
/// been moving. Kept on the workspace so a show or hide from anywhere (a toggle, a command, a page) slides.
struct Reveal {
    shown: bool,
    since: Option<std::time::Instant>,
}

impl Reveal {
    fn new(shown: bool) -> Self {
        Self { shown, since: None }
    }

    /// How much of it is out now, 0 to 1, given whether it should be shown - and whether it is still moving. A change
    /// starts a slide of `ui::MOTION` (none with animations off).
    fn amount(&mut self, shown: bool, animate: bool) -> (f32, bool) {
        if shown != self.shown {
            self.shown = shown;
            self.since = animate.then(std::time::Instant::now);
        }
        let end = if shown { 1.0 } else { 0.0 };
        let Some(since) = self.since else { return (end, false) };
        let done = since.elapsed().as_secs_f32() / ui::MOTION.as_secs_f32();
        if done >= 1.0 {
            self.since = None;
            return (end, false);
        }
        let eased = ui::motion_easing(done);
        (if shown { eased } else { 1.0 - eased }, true)
    }
}

/// A sash being dragged (`Controls/Sash`).
#[derive(Debug, Clone, Copy)]
enum Drag {
    Sidebar { start_x: gpui::Pixels, start_width: gpui::Pixels },
    Panel { start_y: gpui::Pixels, start_height: gpui::Pixels },
    /// The secondary side bar's edge.
    Secondary { start_x: gpui::Pixels, start_width: gpui::Pixels },
    /// The edge between the Output's stage list and its lines.
    Stages { start_x: gpui::Pixels, start_width: gpui::Pixels },
    /// Where the primary side bar's and the panel's sashes meet: both at once.
    Corner { start_x: gpui::Pixels, start_width: gpui::Pixels, start_y: gpui::Pixels, start_height: gpui::Pixels },
}

/// The panel's Find bar (`PanelSearch`), while open: what it looks for and how, and which match it is on.
struct PanelFind {
    input: Entity<TextInput>,
    match_case: bool,
    whole_word: bool,
    use_regex: bool,
    /// The match it is on (0-based), and whether to bring it into view at the next draw.
    current: usize,
    reveal: bool,
    /// What the last search found: the count, stopped before the end (too long), the pattern's error.
    count: usize,
    stopped: bool,
    error: Option<String>,
    _changed: Subscription,
    /// Redraws when the searched text grows: the logs, the terminal shown.
    _watching: Vec<Subscription>,
    watched_terminal: Option<gpui::EntityId>,
    /// The tab it last searched: on another, it starts again at that tab's first match.
    searched_tab: Option<PanelTab>,
}

/// Matches by line, each line's byte ranges with whether it is the current match.
pub(crate) fn marks_by_line(
    matches: &[dnnm_logs::search::Match],
    current: Option<usize>,
) -> std::collections::HashMap<usize, ui::LineMarks> {
    let mut marks: std::collections::HashMap<usize, ui::LineMarks> = std::collections::HashMap::new();
    for (i, m) in matches.iter().enumerate() {
        marks.entry(m.line).or_default().push((m.start..m.start + m.len, Some(i) == current));
    }
    marks
}

/// What runs once the What's new dialog at the start has gone (the guide).
type AfterNotes = Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// The sidebar is dragged between these (`MainWindow.Layout.cs`); under half the smallest it closes.
const MIN_SIDEBAR: f32 = 160.0;
const MAX_SIDEBAR: f32 = 600.0;
const MIN_PANEL: f32 = 90.0;
/// The Output's stage list: its width unless dragged, and how narrow and wide a drag makes it.
const STAGES_WIDTH: f32 = 220.0;
const MIN_STAGES: f32 = 150.0;
const MAX_STAGES: f32 = 520.0;
/// Of the window's height, what the panel leaves at least: the title and status bars and a page that still shows its
/// title, toolbar and a few rows (less and its parts run into each other).
const PANEL_LEAVES: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Projects,
    NewProject,
    SourceControl,
}

pub struct Workspace {
    store: Entity<Store>,
    /// The panel's Find bar, while open.
    find: Option<PanelFind>,
    /// Where the title bar's Help button is: its menu opens under it.
    help_anchor: ui::Anchor,
    /// Where the activity bar's gear is: its menu opens beside it, their bottoms level.
    gear_anchor: ui::Anchor,
    /// Its matches in the Output tab, by line; the line to bring into view at the next draw; the lines' scroll.
    output_marks: std::collections::HashMap<usize, ui::LineMarks>,
    output_reveal: std::cell::Cell<Option<usize>>,
    output_scroll: gpui::ScrollHandle,
    /// The Output's lines as selectable text, and the keyboard's place there (for Copy and Select all).
    output_selection: dnnm_ui::selection::LinesSelection,
    output_text_focus: FocusHandle,
    /// The Output's stage list's scroll position: it follows the stage that runs.
    stages_scroll: gpui::ScrollHandle,
    /// How many Output lines and which stage were last followed - the view keeps up with the run while it is at the end.
    followed_lines: std::cell::Cell<usize>,
    followed_stage: std::cell::Cell<Option<usize>>,
    /// While an operation runs and the panel shows: redraws every second, for the Output's clock.
    output_tick: Option<gpui::Task<()>>,
    page: Page,
    projects: Entity<ProjectsPage>,
    /// New project - made the first time it is shown.
    setup: Option<Entity<dnnm_dialogs::setup_page::SetupPage>>,
    /// Source Control, made the first time it is shown.
    source_control: Option<Entity<dnnm_dialogs::source_control_page::SourceControlPage>>,
    /// Its files, in the primary side bar while it is the page shown.
    source_control_side: Option<Entity<dnnm_dialogs::source_control_page::SourceControlSide>>,
    /// The watch on the Source Control page's project folder, and the task that waits for it to be quiet.
    source_control_watch: Option<(dnnm_infrastructure::source_control::ProjectWatcher, gpui::Task<()>)>,
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
    /// The primary side bar's width.
    sidebar_width: Option<gpui::Pixels>,
    /// The primary and secondary side bars (VS Code's), shown - empty for now; the secondary's width.
    primary_visible: bool,
    secondary_visible: bool,
    /// The side bars' and the panel's slide as they open and shut.
    primary_reveal: Reveal,
    secondary_reveal: Reveal,
    panel_reveal: Reveal,
    secondary_width: Option<gpui::Pixels>,
    /// The Output's stage list's width, once dragged.
    output_stages_width: Option<gpui::Pixels>,
    /// The mouse on the corner where the primary side bar's and the panel's sashes meet: both lines lit.
    corner_hover: bool,
    panel_height: Option<gpui::Pixels>,
    panel_maximized: bool,
    /// A drag of the sidebar's edge or the panel's: where it began and the size then.
    drag: Option<Drag>,
    logs: Entity<LogsView>,
    /// The Terminal tab - once the shells on this PC are known (read off the UI thread at start).
    terminal: Option<Entity<TerminalPanelView>>,
    focus: FocusHandle,
    /// The focus of each layer over the page that keeps the keyboard inside it ([`ui::focus::trap`]).
    layers: Layers,
    icon: Arc<Image>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        background::install_move_pointer(window);
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
                // An open list moves at once when Quick Input Position changes (Customize Layout is one).
                if let Some(palette) = this.palette.clone() {
                    let centered = store.read(cx).settings.quick_input_centered();
                    palette.update(cx, |p, cx| {
                        if p.centered != centered {
                            p.centered = centered;
                            cx.notify();
                        }
                    });
                }
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
                if let Some(setup) = this.setup.clone() {
                    if read != this.setup_sites_read {
                        this.setup_sites_read = read;
                        let sites = store.read(cx).all_sites.clone();
                        setup.update(cx, |p, cx| p.set_sites(sites, cx));
                    }
                    let lock = setup_lock(store.read(cx));
                    setup.update(cx, |p, cx| p.set_locked(lock, cx));
                }
                this.sync_source_control_projects(cx);
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
                Some("SourceControl") => ws.go(Page::SourceControl, window, cx),
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
            source_control: None,
            source_control_side: None,
            source_control_watch: None,
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
            primary_visible: saved_window.primary_open,
            secondary_visible: saved_window.secondary_open,
            primary_reveal: Reveal::new(saved_window.primary_open),
            secondary_reveal: Reveal::new(saved_window.secondary_open),
            panel_reveal: Reveal::new(saved_window.panel_open),
            secondary_width: saved_window.secondary_width.map(px),
            output_stages_width: saved_window.output_stages_width.map(px),
            corner_hover: false,
            panel_height: saved_window.panel_height.map(px),
            panel_maximized: saved_window.panel_maximized,
            drag: None,
            output_tick: None,
            find: None,
            help_anchor: ui::Anchor::default(),
            gear_anchor: ui::Anchor::default(),
            output_marks: Default::default(),
            output_reveal: Default::default(),
            output_scroll: gpui::ScrollHandle::new(),
            output_selection: dnnm_ui::selection::LinesSelection::default(),
            output_text_focus: cx.focus_handle(),
            stages_scroll: gpui::ScrollHandle::new(),
            followed_lines: Default::default(),
            followed_stage: Default::default(),
            logs: cx.new(|cx| LogsView::new(store_for_logs, cx)),
            terminal: None,
            focus,
            layers: Layers::new(cx),
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
        // The window on the screen: in the screen's pixels, whatever the UI scale.
        state.left = Some(bounds.origin.x / gpui::px(1.0));
        state.top = Some(bounds.origin.y / gpui::px(1.0));
        state.width = Some(bounds.size.width / gpui::px(1.0));
        state.height = Some(bounds.size.height / gpui::px(1.0));
        state.maximized = maximized;
        state.sidebar_hidden = !self.sidebar_visible;
        state.panel_open = self.panel_visible;
        state.sidebar_width = self.sidebar_width.map(|w| w / px(1.0));
        state.primary_open = self.primary_visible;
        state.secondary_open = self.secondary_visible;
        state.secondary_width = self.secondary_width.map(|w| w / px(1.0));
        state.output_stages_width = self.output_stages_width.map(|w| w / px(1.0));
        state.panel_height = self.panel_height.map(|h| h / px(1.0));
        state.panel_maximized = self.panel_maximized;
        state.save(&db);
        let mut workspace = WorkspaceState::load(&db);
        workspace.page = Some(match (&self.settings, self.page) {
            (Some(modal), _) => modal.key().into(),
            (None, Page::Projects) => "Projects".into(),
            (None, Page::NewProject) => "Setup".into(),
            (None, Page::SourceControl) => "SourceControl".into(),
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
        // A new UI scale changes the theme's sizes (its body text among them), so the windows are drawn again below.
        apply_ui_scale(settings);
        let appearance = appearance.unwrap_or_else(|| cx.window_appearance());
        let next = Theme::new(choice, appearance, settings.font_size(), settings.density())
            .with_animations(settings.animations() && dnnm_system::motion::system_animations());
        let current = Theme::get(cx);
        if current.dark != next.dark
            || current.text.body != next.text.body
            || current.density != next.density
            || current.animations != next.animations
        {
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
            // Source Control's files are in the primary side bar, as VS Code's: going there opens it.
            if page == Page::SourceControl {
                ws.primary_visible = true;
            }
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
        let lock = setup_lock(store);
        let page = cx.new(|cx| SetupPage::new(lookups, settings, sites, window, cx));
        page.update(cx, |p, cx| p.set_locked(lock, cx));
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
        self.report_update_result(cx);
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

    /// Next / previous page (Ctrl+Tab, Ctrl+Shift+Tab): Projects → New project → Source Control, round.
    fn step_page(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        const ORDER: [Page; 3] = [Page::Projects, Page::NewProject, Page::SourceControl];
        let at = ORDER.iter().position(|p| *p == self.page).unwrap_or(0);
        let next = if forward { (at + 1) % ORDER.len() } else { (at + ORDER.len() - 1) % ORDER.len() };
        self.go(ORDER[next], window, cx);
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
            self.projects.update(cx, |p, cx| p.close_details(window, cx));
        }
    }

    fn resize_sidebar_by(&mut self, by: f32, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        let now = self.sidebar_width.unwrap_or(Theme::get(cx).metrics.sidebar_width);
        self.primary_visible = true;
        self.sidebar_width = Some((now + px(by)).clamp(px(MIN_SIDEBAR), px(MAX_SIDEBAR)));
        cx.notify();
    }

    fn resize_panel_by(&mut self, by: f32, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.panel_height.unwrap_or(Theme::get(cx).metrics.panel_height);
        let most = (window.viewport_size().height - px(PANEL_LEAVES)).max(px(MIN_PANEL));
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
                ws.primary_visible = false;
                ws.secondary_visible = false;
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

    /// The activity bar: the page entries' column (Customize Layout).
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    /// Ctrl+B: the primary side bar.
    pub(crate) fn toggle_primary(&mut self, cx: &mut Context<Self>) {
        self.primary_visible = !self.primary_visible;
        cx.notify();
    }

    /// Ctrl+Alt+B: the secondary side bar.
    pub(crate) fn toggle_secondary(&mut self, cx: &mut Context<Self>) {
        self.secondary_visible = !self.secondary_visible;
        cx.notify();
    }

    fn toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.panel_visible = !self.panel_visible;
        cx.notify();
    }

    fn show_output(&mut self, cx: &mut Context<Self>) {
        self.panel_visible = true;
        self.panel_tab = PanelTab::Output;
        self.store.update(cx, |s, _| s.output_unseen_failure = false);
        cx.notify();
    }

    /// Whether the panel is up on `tab` - a toggle for that tab hides the panel then.
    fn panel_shows(&self, tab: PanelTab) -> bool {
        self.panel_visible && self.panel_tab == tab
    }

    /// Toggle Output: the panel on the Output tab, or hidden when it is already there.
    pub(crate) fn toggle_output(&mut self, cx: &mut Context<Self>) {
        if self.panel_shows(PanelTab::Output) {
            self.toggle_panel(cx);
        } else {
            self.show_output(cx);
        }
    }

    /// Toggle terminal (Ctrl+`): the panel on the Terminal tab, or hidden when it is already there.
    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_shows(PanelTab::Terminal) {
            self.toggle_panel(cx);
        } else {
            self.show_terminal(window, cx);
        }
    }

    /// Toggle DNN Manager's log: the Logs tab on the app's own log, or the panel hidden when it is already there.
    pub(crate) fn toggle_app_log(&mut self, cx: &mut Context<Self>) {
        let on_app_log = self.logs.read(cx).showing_app_log();
        if self.panel_shows(PanelTab::Logs) && on_app_log {
            self.toggle_panel(cx);
        } else {
            self.show_logs(crate::logs_view::LogSite::App, cx);
        }
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
        ui::focus::begin_frame(&self.focus, window);
        self.keep_output_clock(cx);
        self.update_find(cx);
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let gap = theme.metrics.card_gap;
        let compact_density = gap == px(0.0);
        let status_bar_visible = self.store.read(cx).settings.status_bar_visible();
        let has_prompt = self.store.read(cx).prompt.is_some();
        // The activity bar: always its icons alone (VS Code's), this wide with the gap to the window's side.
        let sidebar_width = theme.metrics.sidebar_compact_width;

        let page: gpui::AnyElement = match self.page {
            Page::Projects => self.projects.clone().into_any_element(),
            Page::NewProject => self.setup_page(window, cx).into_any_element(),
            Page::SourceControl => self.source_control_page(window, cx).into_any_element(),
        };
        // What the primary side bar holds: Source Control's files on its page; empty on the others.
        let mut primary_content: Option<gpui::AnyElement> =
            (self.page == Page::SourceControl).then(|| self.source_control_side(cx).into_any_element());

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
        // Which parts the panel runs under besides the page (`PanelAlignment`): the side on its left, its right, both.
        let (panel_left, panel_right) = match settings.panel_alignment() {
            dnnm_application::settings::PanelAlignment::Justify => (true, true),
            dnnm_application::settings::PanelAlignment::Left => (true, false),
            dnnm_application::settings::PanelAlignment::Right => (false, true),
            dnnm_application::settings::PanelAlignment::Center => (false, false),
        };
        // The primary side bar's side (the activity bar's), and whether the panel runs under it.
        let under_primary = if sidebar_right { panel_right } else { panel_left };
        let primary_width = self.sidebar_width.unwrap_or(theme.metrics.sidebar_width);
        let secondary_width = self.secondary_width.unwrap_or(theme.metrics.sidebar_width);
        // Never more than leaves the page its room - also a height saved in a taller window.
        let panel_height = self
            .panel_height
            .unwrap_or(theme.metrics.panel_height)
            .min((window.viewport_size().height - px(PANEL_LEAVES)).max(px(MIN_PANEL)));
        let maximized = self.panel_visible && self.panel_maximized;
        // How far each part is out - a fraction while it slides open or shut, the next frame asked for until it stops.
        let animate = theme.animations;
        let (primary_out, primary_moving) = self.primary_reveal.amount(self.primary_visible, animate);
        let (secondary_out, secondary_moving) = self.secondary_reveal.amount(self.secondary_visible, animate);
        let (panel_out, panel_moving) = self.panel_reveal.amount(self.panel_visible, animate && !self.panel_maximized);
        if primary_moving || secondary_moving || panel_moving {
            window.request_animation_frame();
        }
        // The sash's grab area: the gap between the cards; in Compact, where the parts touch, 4 px around their line.
        let sash_size = if compact_density { px(4.0) } else { gap };
        // A sash (`Sash.cs`): three dots in its middle say it can be dragged (in Compact a 1 px line marks it instead);
        // under the mouse and while dragged a blue line with round ends runs along it - as thick as the sash itself, so
        // every lit sash is the same and none sits half a pixel off (a thinner line in the 5 px gap would).
        // The lit line is always as thick as the sash and only its colour changes: GPUI's group hover takes a colour but
        // not a size, so a 1 px line widened on hover stayed 1 px under the mouse (and was thick only while dragged).
        let sash_line = move |vertical: bool, group: &'static str, active: bool| {
            let rest = div().absolute().bg(c.sidebar_border);
            let rest = if vertical {
                rest.top_0().bottom_0().left((sash_size - px(1.0)) / 2.0).w(px(1.0))
            } else {
                rest.left_0().right_0().top((sash_size - px(1.0)) / 2.0).h(px(1.0))
            };
            div()
                .absolute()
                .inset_0()
                .when(compact_density, |d| d.child(rest))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .group_hover(group, move |s| s.bg(c.accent))
                        .when(active, |d| d.bg(c.accent)),
                )
        };
        let grip = move |vertical: bool| {
            let dot = || div().flex_none().size(px(2.0)).rounded_full().bg(c.text_muted);
            div().flex().gap(px(2.0)).when(vertical, |d| d.flex_col()).child(dot()).child(dot()).child(dot())
        };
        // Lit while dragged - and both while their corner is under the mouse or dragged.
        let corner = matches!(self.drag, Some(Drag::Corner { .. })) || self.corner_hover;
        let (dragging_primary, dragging_secondary, dragging_panel) = (
            corner || matches!(self.drag, Some(Drag::Sidebar { .. })),
            matches!(self.drag, Some(Drag::Secondary { .. })),
            corner || matches!(self.drag, Some(Drag::Panel { .. })),
        );
        // Where the primary side bar's and the panel's sashes meet: dragged, both move - an invisible square over
        // their crossing (the two lines light up), a little larger than a sash to be easy to catch.
        let corner_size = sash_size + px(4.0);
        let corner_handle = move |cx: &mut Context<Self>| {
            div()
                .id("sash-corner")
                .absolute()
                .size(corner_size)
                // Windows' Move pointer: it moves two edges. GPUI has none, so `move_pointer` sets it while the mouse is on
                // the corner or drags it (the crosshair is what GPUI shows where that can't, off Windows).
                .cursor(gpui::CursorStyle::Crosshair)
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.corner_hover = *hovered;
                    background::move_pointer(*hovered || matches!(this.drag, Some(Drag::Corner { .. })));
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(Drag::Corner {
                            start_x: e.position.x,
                            start_width: primary_width,
                            start_y: e.position.y,
                            start_height: panel_height,
                        });
                        cx.stop_propagation();
                    }),
                )
        };
        let has_corner = self.primary_visible && self.panel_visible && !maximized;
        // A vertical sash: its line, its dots (not in Compact), dragged sideways.
        let side_sash = |id: &'static str, active: bool| {
            div()
                .id(id)
                .group(id)
                .flex_none()
                .w(sash_size)
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .relative()
                .child(sash_line(true, id, active))
                .when(!compact_density, |d| {
                    d.child(div().absolute().inset_0().flex().items_center().justify_center().child(grip(true)))
                })
                .cursor(gpui::CursorStyle::ResizeLeftRight)
        };
        // A side bar with its content (the page's, when it has one). `out` of it shows while it slides: its content keeps
        // its width and is cut off, so nothing in it moves about.
        let side_card = |id: &'static str, width: gpui::Pixels, out: f32, content: Option<gpui::AnyElement>| {
            div().flex_none().w(width * out).h_full().overflow_hidden().child(
                div()
                    .id(id)
                    .w(width)
                    .h_full()
                    .overflow_hidden()
                    .bg(c.window_bg)
                    .when(!compact_density, |d| {
                        d.rounded(theme.metrics.card_radius).border_1().border_color(c.card_border)
                    })
                    .children(content),
            )
        };

        // The primary side: the activity bar (its icons alone), the primary side bar and its sash - from the window's
        // side inwards.
        let activity = self.sidebar_visible.then(|| {
            div()
                .flex_none()
                .flex()
                .h_full()
                // `sidebar_width` counts the gap to the window's side, which is the body's padding.
                .child(div().flex_none().w(sidebar_width - gap).h_full().child(self.render_sidebar(cx)))
        });
        let primary = (primary_out > 0.0).then(|| {
            let sash = side_sash("sidebar-sash", dragging_primary)
                .when(has_corner && under_primary, |d| {
                    d.child(corner_handle(cx).bottom(-corner_size).left((sash_size - corner_size) / 2.0))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(Drag::Sidebar { start_x: e.position.x, start_width: primary_width });
                        cx.stop_propagation();
                    }),
                );
            (side_card("primary-side-bar", primary_width, primary_out, primary_content.take()), sash)
        });
        let primary_group = (activity.is_some() || primary.is_some()).then(|| {
            // The primary side bar and its sash side by side (touching); the activity bar the card gap from them (none
            // in Compact, where a line divides them).
            let side = primary.map(|(card, sash)| {
                let mut both = vec![card.into_any_element(), sash.into_any_element()];
                if sidebar_right {
                    both.reverse();
                }
                div().flex().flex_none().h_full().children(both)
            });
            // With the side bar shut, the gap stays between the activity bar and the page (and the panel) - its sash
            // gave it. While the side bar slides, the gap before it shrinks with it, so the page doesn't jump at the end.
            let mut parts: Vec<gpui::AnyElement> = Vec::new();
            if let Some(a) = activity {
                parts.push(a.into_any_element());
                let spacer = if side.is_some() { gap * primary_out } else { gap };
                parts.push(div().flex_none().w(spacer).into_any_element());
            }
            if let Some(side) = side {
                parts.push(side.into_any_element());
            }
            if sidebar_right {
                parts.reverse();
            }
            div().flex().flex_none().h_full().children(parts)
        });
        // The secondary side: its sash and the secondary side bar - from the page outwards.
        let secondary_group = (secondary_out > 0.0).then(|| {
            let sash = side_sash("secondary-sash", dragging_secondary).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    this.drag = Some(Drag::Secondary { start_x: e.position.x, start_width: secondary_width });
                    cx.stop_propagation();
                }),
            );
            let card = side_card("secondary-side-bar", secondary_width, secondary_out, None);
            let parts: Vec<gpui::AnyElement> = if sidebar_right {
                vec![card.into_any_element(), sash.into_any_element()]
            } else {
                vec![sash.into_any_element(), card.into_any_element()]
            };
            div().flex().flex_none().h_full().children(parts)
        });

        // The toasts sit at the page's bottom right - over the page, above the panel (with the panel maximized, the
        // window's).
        let page_card = (!maximized).then(|| {
            div().relative().flex_1().min_h_0().min_w_0().child(card(page, c.window_bg)).child(self.render_toasts(cx))
        });
        let panel_part = (maximized || panel_out > 0.0).then(|| {
            let panel = card(
                div()
                    .relative()
                    .size_full()
                    .child(self.render_panel(cx))
                    .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::Panel, &self.tour_targets))
                    .into_any_element(),
                c.log_bg,
            );
            let part = div()
                .flex()
                .flex_col()
                .when(maximized, |d| d.flex_1().min_h_0())
                .when(!maximized, |d| d.flex_none().h(panel_height + sash_size))
                .child(
                    // The panel's edge: dragged, its height - down to its smallest, never closed by it.
                    div()
                        .id("panel-sash")
                        .group("panel-sash")
                        .flex_none()
                        .h(sash_size)
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .relative()
                        .child(sash_line(false, "panel-sash", dragging_panel))
                        .when(!compact_density, |d| {
                            d.child(div().absolute().inset_0().flex().items_center().justify_center().child(grip(false)))
                        })
                        // Beside the primary side bar's sash when the panel doesn't run under it.
                        .when(has_corner && !under_primary, |d| {
                            let handle = corner_handle(cx).top((sash_size - corner_size) / 2.0);
                            d.child(if sidebar_right { handle.right(-corner_size) } else { handle.left(-corner_size) })
                        })
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                                this.drag = Some(Drag::Panel { start_y: e.position.y, start_height: panel_height });
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(div().flex_1().min_h_0().child(panel));
            // While it slides, `out` of its height shows - the panel itself keeps its height and is cut off.
            if maximized {
                part
            } else {
                div().flex_none().h((panel_height + sash_size) * panel_out).overflow_hidden().child(part)
            }
        });
        let bottom_gap = !self.panel_visible && !status_bar_visible;
        // The gap under the cards when neither the panel nor the status bar is there (the sides': the body's padding).
        let bottom_gutter = || div().flex_none().h(gap);
        // The sides as they sit on screen; each beside the panel too, or over it when the panel runs under it.
        let (left_group, right_group) =
            if sidebar_right { (secondary_group, primary_group) } else { (primary_group, secondary_group) };
        let (left_over, left_beside) = if panel_left { (left_group, None) } else { (None, left_group) };
        let (right_over, right_beside) = if panel_right { (right_group, None) } else { (None, right_group) };
        let top = div().flex().flex_1().min_h_0().children(left_over).children(page_card).children(right_over);
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(!maximized, |d| d.child(top))
            .children(panel_part)
            .when(bottom_gap, |d| d.child(bottom_gutter()));
        let body = div()
            .flex()
            .flex_1()
            .min_h_0()
            .pad(px(0.0), gap)
            .bg(c.sidebar_bg)
            .children(left_beside)
            .child(column)
            .children(right_beside);

        div()
            .id("workspace")
            // A popup open - Settings, a prompt, a dialog, a menu, the Columns popup - and Esc closes it.
            .key_context(
                if self.settings.is_some()
                    || has_prompt
                    || self.dialog.is_some()
                    || self.menu.is_some()
                    || self.projects.read(cx).columns_open()
                {
                    "Workspace Modal"
                } else {
                    "Workspace"
                },
            )
            .track_focus(&self.focus)
            // Tab / Shift+Tab: the next or previous control, inside the dialog or menu that is open.
            .on_action(|_: &ui::focus::FocusNext, window, cx| ui::focus::move_focus(true, window, cx))
            .on_action(|_: &ui::focus::FocusPrevious, window, cx| ui::focus::move_focus(false, window, cx))
            .child(ui::focus::watch_mouse())
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, window, cx| {
                let Some(drag) = this.drag else { return };
                // The button let go outside the window: the drag is over (its sash no longer lit).
                if e.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    cx.notify();
                    return;
                }
                match drag {
                    // The side bars stop at their smallest width - a drag doesn't close them (as the panel).
                    Drag::Sidebar { start_x, start_width } => {
                        let moved = if sidebar_right { start_x - e.position.x } else { e.position.x - start_x };
                        this.sidebar_width = Some((start_width + moved).clamp(px(MIN_SIDEBAR), px(MAX_SIDEBAR)));
                    }
                    Drag::Secondary { start_x, start_width } => {
                        let moved = if sidebar_right { e.position.x - start_x } else { start_x - e.position.x };
                        this.secondary_width = Some((start_width + moved).clamp(px(MIN_SIDEBAR), px(MAX_SIDEBAR)));
                    }
                    Drag::Stages { start_x, start_width } => {
                        let wanted = start_width + (e.position.x - start_x);
                        this.output_stages_width = Some(wanted.clamp(px(MIN_STAGES), px(MAX_STAGES)));
                    }
                    // The panel stops at its smallest height - dragging it down doesn't close it (Ctrl+J, ✕ do).
                    Drag::Panel { start_y, start_height } => {
                        let wanted = start_height + (start_y - e.position.y);
                        let most = window.viewport_size().height - px(PANEL_LEAVES);
                        this.panel_height = Some(wanted.clamp(px(MIN_PANEL), most.max(px(MIN_PANEL))));
                    }
                    Drag::Corner { start_x, start_width, start_y, start_height } => {
                        let moved = if sidebar_right { start_x - e.position.x } else { e.position.x - start_x };
                        this.sidebar_width = Some((start_width + moved).clamp(px(MIN_SIDEBAR), px(MAX_SIDEBAR)));
                        let wanted = start_height + (start_y - e.position.y);
                        let most = window.viewport_size().height - px(PANEL_LEAVES);
                        this.panel_height = Some(wanted.clamp(px(MIN_PANEL), most.max(px(MIN_PANEL))));
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // A corner drag ended: its Move pointer only while the mouse is still on the corner.
                    if matches!(this.drag, Some(Drag::Corner { .. })) {
                        background::move_pointer(this.corner_hover);
                    }
                    // Drawn again, so the sash that was dragged is no longer lit.
                    if this.drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &MaximizePanel, _, cx| {
                this.panel_visible = true;
                this.panel_maximized = !this.panel_maximized;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_primary(cx)))
            .on_action(cx.listener(|_, _: &HideApp, _, cx| cx.hide()))
            .on_action(cx.listener(|_, _: &HideOtherApps, _, cx| cx.hide_other_apps()))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
            .on_action(cx.listener(|this, _: &ToggleSecondarySidebar, _, cx| this.toggle_secondary(cx)))
            .on_action(cx.listener(|this, _: &TogglePanel, _, cx| this.toggle_panel(cx)))
            .on_action(cx.listener(|this, _: &ShowCommands, window, cx| this.show_palette(true, window, cx)))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.show_palette(false, window, cx)))
            .on_action(cx.listener(|this, _: &ShowTerminal, window, cx| this.toggle_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| this.new_terminal(None, false, window, cx)))
            .on_action(cx.listener(|this, _: &GoToProjects, window, cx| this.go(Page::Projects, window, cx)))
            .on_action(cx.listener(|this, _: &GoToNewProject, window, cx| this.go(Page::NewProject, window, cx)))
            .on_action(cx.listener(|this, _: &GoToSourceControl, window, cx| this.go(Page::SourceControl, window, cx)))
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
                if this.settings.is_none() && this.projects.update(cx, |p, cx| p.close_details(window, cx)) {
                    return;
                }
                this.close_modal(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SearchProjects, window, cx| this.search_projects(window, cx)))
            // Ctrl+F finds in what is in front: Settings' own search while it is open (a binding takes the key before
            // the page's key handler could, and the focus may not be in the page), else the panel's Find bar.
            .on_action(cx.listener(|this, _: &FindInPanel, window, cx| {
                if let Some(Modal::Settings(page, _)) = &this.settings {
                    page.update(cx, |p, cx| p.focus_search(window, cx));
                } else {
                    this.open_find(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowOutput, _, cx| this.toggle_output(cx)))
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
                d.child(ui::fade_in(
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
                                ui::focus::trap(div().id("modal-box"), &self.layers.modal)
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
                    "modal-fade",
                    cx,
                ))
            })
            .when(maximized, |d| d.child(self.render_toasts(cx)))
            .when_some(self.render_menu(window, cx), |d, menu| d.child(menu))
            .when_some(self.dialog.as_ref().map(|(view, _)| view.clone()), |d, view| {
                d.child(ui::fade_in(
                    ui::focus::trap(div().id("dialog-backdrop"), &self.layers.dialog)
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(c.modal_backdrop)
                        .occlude()
                        .child(view),
                    "dialog-fade",
                    cx,
                ))
            })
            .when_some(self.palette.clone(), |d, palette| {
                d.child(ui::fade_in(
                    ui::focus::trap(div().absolute().inset_0(), &self.layers.palette).child(palette),
                    "palette-fade",
                    cx,
                ))
            })
            .when_some(self.tour.as_ref().map(|(t, _)| t.clone()), |d, tour| d.child(tour))
            .when(has_prompt, |d| d.child(self.render_prompt(cx)))
    }
}

impl Workspace {
    /// The title bar: icon and name, the command center in the middle, Help and the layout buttons, then the window's.
    fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // macOS has its own window buttons at the left (the traffic lights, placed in `main`): ours are left out there,
        // and the logo moves past them. Windows and Linux get ours at the right.
        let mac = cfg!(target_os = "macos");
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let h = theme.metrics.title_bar_height;
        let menu_bar = self.store.read(cx).settings.menu_bar_visible();
        let maximized = window.is_maximized();
        let sidebar_right =
            self.store.read(cx).settings.sidebar_position() == dnnm_application::settings::SidebarPosition::Right;
        let layout_button = |id: &'static str, tip: String| {
            ui::focusable_bare(
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
                    .tip(tip)
                    // Out of the caption's drag area: a press here is a click, not the start of a move.
                    .occlude(),
                cx,
            )
        };
        let caption = |id: &'static str, glyph: Icon, area: WindowControlArea, hover| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(theme.metrics.caption_button_width)
                .h(h)
                .window_control_area(area)
                // Only its own area under the mouse - not the caption's drag area too.
                .occlude()
                .hover(move |s| s.bg(hover))
                .child(ui::icon_sized(glyph, IconSize::Small, c.text_primary, cx))
        };
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            // The logo at the left, the buttons at the right (the command center is a layer over the whole bar).
            .justify_between()
            .h(h)
            .bg(c.sidebar_bg)
            .when(theme.metrics.card_gap == px(0.0), |d| d.border_b_1().border_color(c.sidebar_border))
            .window_control_area(WindowControlArea::Drag)
            // Over the workspace for the mouse: its focus handling marks every press as handled, which on Windows
            // stops a press on the caption from moving the window. The bar's own buttons are on top of it.
            .occlude()
            // The command center, centred on the window.
            .child(
                div().absolute().inset_0().flex().items_center().justify_center().child(
                    ui::focusable(div().id("command-center"), cx)
                        .flex()
                        .items_center()
                        .gap(px(icons::LABEL_GAP))
                        .w(relative_width(window, 0.38, 600.0))
                        .h(px(26.0))
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
                        .tip("Search projects and commands")
                        .occlude()
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.show_palette(false, window, cx)))
                        .relative()
                        .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::CommandCenter, &self.tour_targets))
                        .child(ui::icon(Icon::Search, c.command_center_fg, cx))
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
                    // Room for the traffic lights: up to `MAC_CAPTION` with the padding and gap in front of the logo. Native
                    // pixels, not the UI scale - the traffic lights don't grow with it.
                    .when(mac, |d| d.child(div().flex_none().w(gpui::px(MAC_CAPTION) - px(12.0) - px(10.0))))
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
            .child(
                // Help, a line, the layout buttons, a line, the window's buttons.
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    // Update, while GitHub has a newer release: blue, as VS Code's.
                    .when_some(self.update_available.as_ref().map(|r| r.tag.clone()), |row, tag| {
                        row.child(
                            ui::focusable_bare(
                                div()
                                    .id("update")
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .h(theme.metrics.layout_button_height)
                                    .pad(px(0.0), px(8.0))
                                    .rounded(px(5.0))
                                    .bg(c.accent)
                                    .hover(move |s| s.bg(c.accent_hover))
                                    .cursor_pointer()
                                    .text_color(c.on_accent)
                                    .text_size(px(12.0))
                                    .tip(format!("Install DNN Manager {tag} - it closes and opens again by itself"))
                                    // Out of the caption's drag area: a press here is a click, not the start of a move.
                                    .occlude(),
                                cx,
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.install_update(window, cx)))
                            .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::UpdateButton, &self.tour_targets))
                            .child("Update"),
                        )
                    })
                    .child(
                        layout_button("help", dnnm_dialogs::help_menu::HELP_BUTTON_TOOLTIP.to_string())
                            .relative()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    // Under the button, their left edges level.
                                    let (position, corner) = this.help_anchor.below(false);
                                    this.open_help_menu(position, cx);
                                    if let Some(menu) = this.menu.as_mut() {
                                        menu.anchor = corner;
                                    }
                                }),
                            )
                            // Enter or Space on it (the mouse opens it on the press, above).
                            .on_click(cx.listener(|this, e: &ClickEvent, _, cx| {
                                if e.is_keyboard() {
                                    let (position, corner) = this.help_anchor.below(false);
                                    this.open_help_menu(position, cx);
                                    if let Some(menu) = this.menu.as_mut() {
                                        menu.anchor = corner;
                                    }
                                }
                            }))
                            .child(self.help_anchor.measure())
                            .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::HelpButton, &self.tour_targets))
                            // The size of the layout icons beside it: at 13 px the ? in its ring was a smudge.
                            .child(ui::icon_sized(Icon::Question, IconSize::Nav, c.text_primary, cx)),
                    )
                    .child(div().w(px(1.0)).h(px(16.0)).bg(c.sidebar_divider))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                layout_button("customize-layout", "Customize Layout…".to_string())
                                    .on_click(
                                        cx.listener(|this, _: &ClickEvent, window, cx| this.customize_layout(window, cx)),
                                    )
                                    .child(icons::layout_icon(LayoutIcon::Customize, false, c.text_primary)),
                            )
                            .child(
                                layout_button(
                                    "toggle-sidebar",
                                    shortcuts::with_shortcut(
                                        if self.primary_visible { "Hide Primary Side Bar" } else { "Show Primary Side Bar" },
                                        "view.toggleSidebar",
                                        &self.store.read(cx).settings,
                                    ),
                                )
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_primary(cx)))
                                .child(icons::layout_icon(
                                    if sidebar_right { LayoutIcon::SidebarRight } else { LayoutIcon::SidebarLeft },
                                    self.primary_visible,
                                    c.text_primary,
                                )),
                            )
                            .child(
                                layout_button(
                                    "toggle-panel",
                                    if self.panel_visible { "Hide the panel" } else { "Show the panel: Output" }.to_string(),
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
                            )
                            .child(
                                layout_button(
                                    "toggle-secondary",
                                    shortcuts::with_shortcut(
                                        if self.secondary_visible {
                                            "Hide Secondary Side Bar"
                                        } else {
                                            "Show Secondary Side Bar"
                                        },
                                        "view.toggleSecondarySidebar",
                                        &self.store.read(cx).settings,
                                    ),
                                )
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_secondary(cx)))
                                .child(icons::layout_icon(
                                    if sidebar_right { LayoutIcon::SidebarLeft } else { LayoutIcon::SidebarRight },
                                    self.secondary_visible,
                                    c.text_primary,
                                )),
                            ),
                    )
                    // Without our window buttons the help and layout buttons end 12 px from the edge (5 + the 7 px gap).
                    .when(mac, |d| d.child(div().flex_none().w(px(5.0))))
                    .when(!mac, |d| {
                        d.child(div().w(px(1.0)).h(px(16.0)).bg(c.sidebar_divider)).child(
                            div()
                                .flex()
                                .items_center()
                                .child(caption("minimize", Icon::Minimize, WindowControlArea::Min, c.hover_bg))
                                .child(caption(
                                    "maximize",
                                    if maximized { Icon::Restore } else { Icon::Maximize },
                                    WindowControlArea::Max,
                                    c.hover_bg,
                                ))
                                .child(caption("close", Icon::Close, WindowControlArea::Close, c.close_hover)),
                        )
                    }),
            )
    }

    /// The activity bar: Projects and New project, the gear at the bottom - their icons alone (VS Code's).
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // An entry is a square this big: its 16 px glyph, 8 px around it.
        const ENTRY: f32 = 32.0;
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let compact_density = theme.metrics.card_gap == px(0.0);
        let sidebar_right =
            self.store.read(cx).settings.sidebar_position() == dnnm_application::settings::SidebarPosition::Right;
        // How far the activity bar's inner edge is from the window's side.
        let sidebar_edge =
            theme.metrics.sidebar_compact_width + if compact_density { px(0.0) } else { theme.metrics.card_gap };
        // The gear's menu opens upwards from it; Help's opens down from the title bar.
        let gear_open = self.menu.as_ref().is_some_and(|m| m.anchor != gpui::Corner::TopLeft);
        // An entry: its icon alone, what it is in its tooltip (VS Code's activity bar).
        let entry = |id: &'static str, glyph: Icon, tip: String, selected: bool| {
            ui::focusable_bare(div().id(id), cx)
                .flex()
                .items_center()
                .justify_center()
                .size(px(ENTRY))
                .flex_none()
                .rounded(theme.metrics.card_radius)
                .cursor_pointer()
                .text_size(theme.text.medium)
                .text_color(if selected { c.sidebar_active_fg } else { c.sidebar_fg })
                .when(selected, |d| d.bg(c.sidebar_selected))
                .when(!selected, |d| d.hover(move |s| s.bg(c.sidebar_hover)))
                .tip_beside(tip)
                .child(ui::icon_sized(glyph, IconSize::Nav, if selected { c.sidebar_active_fg } else { c.sidebar_fg }, cx))
        };
        // The entries' groups: as much room above the first and below the last as beside them (the bar's width less
        // its edge lines and the entry, halved), 4 px between them.
        let edges = if compact_density { px(1.0) } else { px(2.0) };
        let room = (theme.metrics.sidebar_compact_width - edges - px(ENTRY)) / 2.0;
        let group = move || div().flex().flex_col().items_center().gap(px(4.0)).p(room);
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(c.sidebar_bg)
            .when(!compact_density, |d| d.rounded(theme.metrics.card_radius).border_1().border_color(c.card_border))
            // Its edge in Compact: a line on the side towards the page.
            .when(compact_density && !sidebar_right, |d| d.border_r_1().border_color(c.sidebar_border))
            .when(compact_density && sidebar_right, |d| d.border_l_1().border_color(c.sidebar_border))
            .child(
                group()
                    .relative()
                    .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::SidebarPages, &self.tour_targets))
                    .child(
                        entry(
                            "nav-projects",
                            Icon::List,
                            shortcuts::with_shortcut(
                                "Projects - every website on this computer",
                                "pages.projects",
                                &self.store.read(cx).settings,
                            ),
                            self.page == Page::Projects,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.go(Page::Projects, window, cx))),
                    )
                    .child(
                        entry(
                            "nav-new",
                            Icon::Add,
                            shortcuts::with_shortcut(
                                "New project - create a website, or one from a backup",
                                "pages.newProject",
                                &self.store.read(cx).settings,
                            ),
                            self.page == Page::NewProject,
                        )
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| this.go(Page::NewProject, window, cx)),
                        ),
                    )
                    .child(
                        entry(
                            "nav-source-control",
                            Icon::SourceControl,
                            shortcuts::with_shortcut(
                                "Source Control - compare a project with its servers and deploy it over SFTP, FTPS or FTP",
                                "pages.sourceControl",
                                &self.store.read(cx).settings,
                            ),
                            self.page == Page::SourceControl,
                        )
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| this.go(Page::SourceControl, window, cx)),
                        ),
                    ),
            )
            .child(group().child(
                entry("nav-gear", Icon::Settings, "Settings, themes, Troubleshoot and updates".to_string(), gear_open)
                    .relative()
                    .child(self.gear_anchor.measure())
                    .child(dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::ManageButton, &self.tour_targets))
                    // Enter or Space on it: as a press of the mouse (below).
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        if e.is_keyboard() {
                            this.toggle_gear_menu(e.position(), sidebar_right, sidebar_edge, window, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.toggle_gear_menu(e.position, sidebar_right, sidebar_edge, window, cx);
                        }),
                    ),
            ))
    }

    /// Opens the gear's menu, or closes it when open: beside the gear - right of the sidebar (left of it on the
    /// right) - its bottom level with the gear's (`Manage_Click`). `at` is where it was pressed.
    fn toggle_gear_menu(
        &mut self,
        at: gpui::Point<gpui::Pixels>,
        sidebar_right: bool,
        sidebar_edge: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.take().is_none() {
            let bottom = self.gear_anchor.bounds().map_or(at.y + px(17.0), |b| b.bottom());
            let (x, corner) = if sidebar_right {
                (window.viewport_size().width - sidebar_edge - px(6.0), gpui::Corner::BottomRight)
            } else {
                (sidebar_edge + px(6.0), gpui::Corner::BottomLeft)
            };
            self.open_gear_menu(gpui::point(x, bottom), cx);
            if let Some(menu) = self.menu.as_mut() {
                menu.anchor = corner;
            }
        }
        cx.notify();
    }

    /// The running operation's question (`MessageDialog`): its glyph - a question, or a warning in red when the yes
    /// deletes or replaces something -, the text, and its answers; Esc and a click beside it answer no.
    fn render_prompt(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let Some(prompt) = store.prompt.as_ref() else { return div().into_any_element() };
        let (glyph, glyph_color) =
            if prompt.danger { (Icon::Warning, c.error_text) } else { (Icon::Question, c.accent) };
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
        ui::focus::trap(div().id("prompt-backdrop"), &self.layers.prompt)
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
                            // Centred on the question's first line, however many lines it takes.
                            .child(ui::line_icon(glyph, IconSize::Large, glyph_color, cx))
                            .child(
                                // `min_w_0`: without it a long line (a path) widens the text past the dialog instead of wrapping.
                                div().id("prompt-text").flex_1().min_w_0().max_h(px(360.0)).overflow_y_scroll().child(question),
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
    fn render_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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
                    window,
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// The bottom panel's Output tab (`Controls/PipelineView`): the last operation's status, title and lines.
    /// While an operation runs and the panel is open, the Output's clock and the running stage's time move: a redraw
    /// every second (`PipelineView`'s clock) - stopped once it has ended or the panel is closed.
    fn keep_output_clock(&mut self, cx: &mut Context<Self>) {
        let running = |ws: &Workspace, cx: &App| {
            ws.panel_visible && ws.store.read(cx).runs.last().is_some_and(|r| r.status == RunStatus::Running)
        };
        if !running(self, cx) || self.output_tick.is_some() {
            return;
        }
        self.output_tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let go_on = this
                    .update(cx, |ws, cx| {
                        let go_on = running(ws, cx);
                        if go_on {
                            cx.notify();
                        } else {
                            ws.output_tick = None;
                        }
                        go_on
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        }));
    }

    /// Ctrl+F: the panel's Find bar on the shown tab - opened (the panel too), or its text selected to type over.
    pub(crate) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_visible = true;
        if self.find.is_none() {
            let input = cx.new(|cx| TextInput::new("Find", cx));
            let changed = cx.subscribe(&input, |this, _, _: &TextChanged, cx| {
                if let Some(find) = this.find.as_mut() {
                    find.current = 0;
                    find.reveal = true;
                }
                cx.notify();
            });
            // A log that grows is searched again.
            let logs = cx.observe(&self.logs, |_, _, cx| cx.notify());
            self.find = Some(PanelFind {
                input,
                match_case: false,
                whole_word: false,
                use_regex: false,
                current: 0,
                reveal: true,
                count: 0,
                stopped: false,
                error: None,
                _changed: changed,
                _watching: vec![logs],
                watched_terminal: None,
                searched_tab: None,
            });
        }
        if let Some(find) = &self.find {
            let handle = find.input.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            self.clear_find_marks(cx);
            window.focus(&self.focus);
            cx.notify();
        }
    }

    /// Enter / Shift+Enter, ↓ / ↑: the next or previous match, round from the last to the first.
    fn step_find(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(find) = self.find.as_mut()
            && find.count > 0
        {
            find.current = (find.current as isize + by).rem_euclid(find.count as isize) as usize;
            find.reveal = true;
            cx.notify();
        }
    }

    fn toggle_find_option(&mut self, option: fn(&mut PanelFind) -> &mut bool, cx: &mut Context<Self>) {
        if let Some(find) = self.find.as_mut() {
            let on = option(find);
            *on = !*on;
            find.current = 0;
            find.reveal = true;
            cx.notify();
        }
    }

    /// No highlights left anywhere: the bar closed, or its tab no longer shown.
    fn clear_find_marks(&mut self, cx: &mut Context<Self>) {
        self.output_marks.clear();
        self.logs.update(cx, |l, cx| l.set_find(&[], None, false, cx));
        if let Some(view) = self.terminal.as_ref().and_then(|t| t.read(cx).shown().cloned()) {
            view.update(cx, |v, cx| v.set_find(Default::default(), Default::default(), None, cx));
        }
    }

    /// Searches the shown tab with the Find bar's text and options (`PanelSearch`): its count for the bar, and its
    /// matches highlighted there - the current one brought into view when asked. Every draw: the text may have grown.
    fn update_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else { return };
        if !self.panel_visible {
            return;
        }
        let on_terminal = self.panel_tab == PanelTab::Terminal && self.terminal.is_some();
        let on_logs = self.panel_tab == PanelTab::Logs;
        let text = find.input.read(cx).text().to_string();
        let query = dnnm_logs::search::SearchQuery::new(&text, find.match_case, find.whole_word, find.use_regex);
        let terminal_view = self.terminal.as_ref().filter(|_| on_terminal).and_then(|t| t.read(cx).shown().cloned());
        let lines: Vec<String> = if let Some(view) = &terminal_view {
            view.read(cx).find_lines()
        } else if on_logs {
            self.logs.read(cx).find_lines().iter().map(|l| l.to_string()).collect()
        } else {
            self.store.read(cx).runs.last().map(|r| r.lines.iter().map(|l| l.text.to_string()).collect()).unwrap_or_default()
        };
        let result = query.find(&lines);
        let count = result.matches.len();
        let find = self.find.as_mut().expect("checked above");
        if find.searched_tab != Some(self.panel_tab) {
            find.searched_tab = Some(self.panel_tab);
            find.current = 0;
            find.reveal = true;
        }
        if find.current >= count {
            find.current = 0;
        }
        let current = (count > 0).then_some(find.current);
        let reveal = std::mem::take(&mut find.reveal);
        find.count = count;
        find.stopped = result.stopped;
        find.error = query.error().map(str::to_string);
        // A terminal's output grows too: the one shown is watched while the bar is open.
        let terminal_id = terminal_view.as_ref().map(|v| v.entity_id());
        if terminal_id != find.watched_terminal {
            find.watched_terminal = terminal_id;
            find._watching.truncate(1);
            if let Some(view) = &terminal_view {
                let watch = cx.observe(view, |_, _, cx| cx.notify());
                if let Some(find) = self.find.as_mut() {
                    find._watching.push(watch);
                }
            }
        }
        let current_line = current.and_then(|i| result.matches.get(i)).map(|m| m.line);

        // Each tab: its own matches; the others none.
        self.output_marks =
            if !on_logs && !on_terminal { marks_by_line(&result.matches, current) } else { Default::default() };
        if !on_logs && !on_terminal && reveal {
            self.output_reveal.set(current_line);
        }
        let log_matches: &[dnnm_logs::search::Match] = if on_logs { &result.matches } else { &[] };
        self.logs.update(cx, |l, cx| l.set_find(log_matches, current.filter(|_| on_logs), reveal && on_logs, cx));
        if let Some(view) = self.terminal.as_ref().and_then(|t| t.read(cx).shown().cloned()) {
            let marks = if on_terminal { terminal_marks(&lines, &result.matches, current) } else { Default::default() };
            let colors = ui::find_colors(cx);
            let line = current_line.filter(|_| reveal && on_terminal);
            view.update(cx, |v, cx| v.set_find(marks, colors, line, cx));
        }
    }

    /// The Find bar, over the top right of the shown tab (`PanelSearch`): the text with Match Case, Whole Word and
    /// Regular Expression in its box, the count, previous, next and close.
    fn render_find_bar(&self, find: &PanelFind, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let option = |id: &'static str, label: &'static str, tip: String, on: bool| {
            ui::focusable_bare(div().id(id), cx)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(22.0))
                .h(px(18.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(if on { c.accent } else { gpui::transparent_black() })
                .when(on, |d| d.bg(c.accent.opacity(0.25)))
                .when(!on, |d| d.hover(move |s| s.bg(c.hover_bg)))
                .cursor_pointer()
                .text_size(theme.text.small)
                .text_color(if on { c.text_primary } else { c.log_muted })
                .tip(tip)
                .child(label)
        };
        let count_text: SharedString = if find.error.is_some() {
            "Invalid pattern".into()
        } else if find.input.read(cx).text().is_empty() {
            "".into()
        } else if find.count == 0 {
            "No results".into()
        } else if find.stopped {
            format!("{} of {}+", find.current + 1, find.count).into()
        } else {
            format!("{} of {}", find.current + 1, find.count).into()
        };
        let nav = |id: &'static str, glyph: Icon, tip: &'static str| {
            ui::focusable_bare(div().id(id), cx)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(26.0))
                .rounded(theme.metrics.control_radius)
                .cursor_pointer()
                .hover(move |s| s.bg(c.hover_bg))
                .tip(tip)
                .child(ui::icon(glyph, c.log_muted, cx))
        };
        div()
            .id("panel-find")
            .key_context("PanelFind")
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.step_find(1, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.step_find(-1, cx)))
            .on_action(cx.listener(|this, _: &FindClose, window, cx| this.close_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindToggleCase, _, cx| this.toggle_find_option(|f| &mut f.match_case, cx)))
            .on_action(cx.listener(|this, _: &FindToggleWord, _, cx| this.toggle_find_option(|f| &mut f.whole_word, cx)))
            .on_action(cx.listener(|this, _: &FindToggleRegex, _, cx| this.toggle_find_option(|f| &mut f.use_regex, cx)))
            .absolute()
            .top(px(41.0))
            .right(px(20.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(c.card_border)
            .bg(c.card_bg)
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .id("panel-find-box")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .w(px(260.0))
                    .h(theme.metrics.search_height)
                    .pad(px(0.0), px(6.0))
                    .rounded(theme.metrics.control_radius)
                    .border_1()
                    .border_color(if find.error.is_some() { c.error_text } else { c.focus_border })
                    .bg(c.input_bg)
                    .text_size(theme.text.body)
                    .when_some(find.error.clone(), |d, error| d.tip(error))
                    .child(div().flex_1().min_w_0().child(find.input.clone()))
                    .child(
                        option("find-case", "Aa", format!("Match Case ({})", find_option_key('c', Os::CURRENT).1), find.match_case)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_find_option(|f| &mut f.match_case, cx))),
                    )
                    .child(
                        option("find-word", "ab", format!("Match Whole Word ({})", find_option_key('w', Os::CURRENT).1), find.whole_word)
                            .underline()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_find_option(|f| &mut f.whole_word, cx))),
                    )
                    .child(
                        option("find-regex", ".*", format!("Use Regular Expression ({})", find_option_key('r', Os::CURRENT).1), find.use_regex)
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_find_option(|f| &mut f.use_regex, cx))),
                    ),
            )
            .child(
                div()
                    .min_w(px(70.0))
                    .pad(px(0.0), px(4.0))
                    .text_size(theme.text.caption)
                    .text_color(if find.error.is_some() { c.error_text } else { c.text_muted })
                    .child(count_text),
            )
            .child(
                nav("find-previous", Icon::ChevronUp, "Previous match (Shift+Enter)")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step_find(-1, cx))),
            )
            .child(
                nav("find-next", Icon::ChevronDown, "Next match (Enter)")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step_find(1, cx))),
            )
            .child(
                nav("find-close", Icon::Close, "Close (Esc)")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_find(window, cx))),
            )
            .into_any_element()
    }

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let unseen = store.output_unseen_failure;
        let run = store.runs.last().cloned();
        let busy = store.is_busy();
        let on_terminal = self.panel_tab == PanelTab::Terminal && self.terminal.is_some();
        let on_logs = self.panel_tab == PanelTab::Logs;
        // The logs are followed only while they are seen (`LogsView.Paused`).
        let logs_seen = on_logs && self.panel_visible;
        self.logs.update(cx, |l, _| l.shown = logs_seen);

        let tab = |id: &'static str, label: &'static str, tip: &'static str, chosen: bool| {
            ui::focusable_bare(div().id(id), cx)
                .flex()
                .items_center()
                .gap(px(5.0))
                .h(px(26.0))
                .pad(px(0.0), px(8.0))
                .rounded(theme.metrics.control_radius)
                .cursor_pointer()
                .text_size(theme.text.caption)
                .when(chosen, |d| d.bg(c.sidebar_selected).text_color(c.log_header_fg))
                .when(!chosen, |d| d.text_color(c.log_muted).hover(move |s| s.bg(c.hover_bg).text_color(c.log_header_fg)))
                .tip(tip)
                .child(label)
        };
        // A panel button (`PanelButton`): 26 x 26, transparent until hovered, the icon in LogMuted.
        let panel_button = |id: &'static str, icon: gpui::Div, enabled: bool| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(26.0))
                .rounded(theme.metrics.control_radius)
                .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                .when(enabled, |d| ui::focusable_bare(d, cx))
                .when(!enabled, |d| d.opacity(0.35))
                .child(icon)
        };
        // The header: the tabs and buttons 26 high, 4 px from its edges all round (as much beside them as above), a
        // line under it.
        let tabs = div()
            .flex()
            .flex_none()
            .items_center()
            .p(px(4.0))
            .border_b_1()
            .border_color(c.card_border)
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
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
                            panel_button("clear-output", ui::icon(Icon::Delete, c.log_muted, cx), !busy)
                                .tip("Clear output")
                                .when(!busy, |b| {
                                    b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.store.update(cx, |s, cx| s.clear_output(cx));
                                    }))
                                }),
                        )
                    })
                    .child(
                        panel_button("find-in-panel", ui::icon(Icon::Search, c.log_muted, cx), true)
                            .tip(shortcuts::with_shortcut("Find", "view.search", &self.store.read(cx).settings))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_find(window, cx))),
                    )
                    .child(div().flex_none().w(px(1.0)).h(px(16.0)).bg(c.card_border))
                    .child({
                        let maximized = self.panel_maximized;
                        panel_button(
                            "maximize-panel",
                            ui::icon(
                                if maximized { Icon::PanelRestore } else { Icon::PanelMaximize },
                                c.log_muted,
                                cx,
                            ),
                            true,
                        )
                        .tip(shortcuts::with_shortcut(
                            if maximized { "Restore panel size" } else { "Maximize panel" },
                            "panel.maximize",
                            &self.store.read(cx).settings,
                        ))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.panel_maximized = !this.panel_maximized;
                            cx.notify();
                        }))
                    })
                    .child(
                        panel_button("hide-panel", ui::icon(Icon::Close, c.log_muted, cx), true)
                            .tip("Hide panel (Ctrl+J)")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.panel_visible = false;
                                cx.notify();
                            })),
                    ),
            );

        let find_bar = self.find.as_ref().map(|find| self.render_find_bar(find, cx));
        if on_logs {
            return div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .child(tabs)
                .child(div().flex_1().min_h_0().child(self.logs.clone()))
                .children(find_bar);
        }
        if let Some(terminal) = self.terminal.clone().filter(|_| on_terminal) {
            return div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .child(tabs)
                .child(div().flex_1().min_h_0().child(terminal))
                .children(find_bar);
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
        div().relative().size_full().flex().flex_col().child(tabs).child(content).children(find_bar)
    }

    /// One operation in the Output tab (`PipelineView`): the header - status pill, title, issues, time and duration -,
    /// the stage rail with the facts and counts, and the lines.
    fn render_run(&self, run: &Run, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        // The pill (`PipelineView`): a spinner while it runs, else how it ended.
        let (pill, pill_bg, pill_fg, pill_glyph) = match run.status {
            RunStatus::Running => ("Running", c.out_pill_running_bg, c.out_pill_running_fg, None),
            RunStatus::Finished => ("Finished", c.out_pill_ok_bg, c.out_pill_ok_fg, Some(Icon::Check)),
            RunStatus::Failed => ("Failed", c.out_pill_fail_bg, c.out_pill_fail_fg, Some(Icon::Close)),
            RunStatus::Cancelled => ("Cancelled", c.out_pill_neutral_bg, c.out_pill_neutral_fg, Some(Icon::Close)),
        };
        let now = SystemTime::now();
        let warnings = run.lines.iter().filter(|l| l.kind == LineKind::Warn).count();
        let errors = run.lines.iter().filter(|l| l.kind == LineKind::Fail).count();
        let ended = run.duration.map(|d| run.started + d);
        // "13:20:34 → 13:23:01" once it ended; "13:20:34 →" and the time so far, by the second, while it runs.
        let time_range = match ended {
            Some(end) => format!("{} → {}", clock(run.started), clock(end)),
            None => format!("{} →", clock(run.started)),
        };
        let elapsed = run.duration.unwrap_or_else(|| now.duration_since(run.started).unwrap_or_default());
        let duration = span(elapsed, run.duration.is_none());
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
            .gap(px(14.0))
            .child(
                // The pill and the title 12 px apart, the issues 14 px after them - the title cut short with "…"
                // before the times on the right are.
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    // Tight: 4 px between the mark and the word, 6 inside each round end.
                                    .gap(px(4.0))
                                    .pad(px(2.0), px(6.0))
                                    .rounded(px(10.0))
                                    .bg(pill_bg)
                                    .child(match pill_glyph {
                                        Some(glyph) => ui::icon_sized(glyph, IconSize::Small, pill_fg, cx).into_any_element(),
                                        None => ui::spinner("output-pill-spinner", icons::SIZE_SMALL, pill_fg, cx),
                                    })
                                    .child(
                                        div()
                                            .text_size(theme.text.small)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(pill_fg)
                                            .child(pill),
                                    ),
                            )
                            .child(
                                ui::one_line(run.title.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c.out_heading),
                            ),
                    )
                    .when_some(issues, |d, (text, color)| {
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(icons::LABEL_GAP))
                                .child(ui::icon(Icon::Warning, color, cx))
                                .child(div().text_color(color).child(text)),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(14.0))
                    .child(div().text_color(c.log_muted).child(time_range))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c.out_heading).child(duration)),
            );

        // The stage rail: each stage with how it went - the heading and the stages children of the scrolled list, 2 px
        // apart (the heading 1.5 px more padded above and below), so the list can bring the running stage into view.
        let mut stages = div()
            .id("output-stages")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.stages_scroll)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pad(px(10.0), px(4.0));
        let stage_list = run.stages();
        // The running stage in view as the run moves on to it (row `i` is child `i + 1`, after the heading).
        let running_stage = stage_list.iter().position(|(_, state)| *state == LineKind::Info);
        if running_stage.is_some() && running_stage != self.followed_stage.get() {
            self.followed_stage.set(running_stage);
            if let Some(i) = running_stage {
                self.stages_scroll.scroll_to_item(i + 1);
            }
        }
        if !stage_list.is_empty() {
            stages = stages.child(
                div()
                    .pad(px(1.5), px(6.0))
                    .text_size(theme.text.small)
                    .text_color(c.log_muted)
                    .child(format!(
                        "Stages · {}/{}",
                        stage_list.iter().filter(|(_, state)| *state != LineKind::Info).count(),
                        stage_list.len()
                    )),
            );
        }
        for (i, (name, state)) in stage_list.into_iter().enumerate() {
            let running = state == LineKind::Info;
            let mark = match state {
                LineKind::Fail => ui::icon(Icon::Error, c.out_error, cx).into_any_element(),
                LineKind::Warn => ui::icon(Icon::Warning, c.log_warn, cx).into_any_element(),
                LineKind::Info => ui::spinner(("stage-spinner", i), icons::SIZE, c.out_pill_running_fg, cx),
                _ => ui::icon(Icon::Check, c.success_text, cx).into_any_element(),
            };
            let took = run.stage_duration(i, now).map(|d| span(d, running)).unwrap_or_default();
            stages = stages.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(icons::LABEL_GAP))
                    .pad(px(4.0), px(6.0))
                    .rounded(theme.metrics.control_radius)
                    .when(running, |d| d.bg(c.selection_bg))
                    .child(mark)
                    .child(ui::one_line(name).flex_1())
                    .child(div().flex_none().text_size(theme.text.small).text_color(c.log_muted).child(took)),
            );
        }
        let summary_row = |key: SharedString, value: SharedString| {
            div()
                .flex()
                .w_full()
                .justify_between()
                .gap(px(8.0))
                .pad(px(1.0), px(0.0))
                // The name whole (they are short); the value on the right, cut short with "…" in a narrow list.
                .child(div().flex_none().whitespace_nowrap().text_color(c.log_muted).child(key))
                .child(ui::one_line(value).flex_1().text_right().text_color(c.log_fg))
        };
        let mut summary = div()
            .flex()
            .flex_col()
            .w_full()
            .pad(px(7.0), px(0.0))
            .border_t_1()
            .border_color(c.card_border);
        for (name, value) in &run.facts {
            summary = summary.child(summary_row(name.clone(), value.clone()));
        }
        summary = summary
            .child(summary_row("Warnings".into(), warnings.to_string().into()))
            .child(summary_row("Errors".into(), errors.to_string().into()));
        let stages_width = self.output_stages_width.unwrap_or(px(STAGES_WIDTH));
        let rail = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(stages_width)
            .h_full()
            .gap(px(10.0))
            .child(stages)
            // The summary's line stops 10 px short of the rail's sides.
            .child(div().flex().flex_col().flex_none().pad(px(0.0), px(10.0)).child(summary));

        // The lines, each a child of the scrolled column so the Find bar can bring one into view; a stage's heading 3 px
        // padded above and below, so stages stand 6 px apart without a spacer between them. Their text wraps in the
        // panel's width and can be selected - dragged over, double-clicked (a word), triple-clicked (a line), copied
        // with Ctrl / ⌘ + C or the right-click menu.
        let selection = self.output_selection.clone();
        selection.begin(&format!("{}@{:?}", run.title, run.started));
        let texts: std::rc::Rc<Vec<SharedString>> = std::rc::Rc::new(run.lines.iter().map(|l| l.text.clone()).collect());
        let (press, drag, release, release_out) = (selection.clone(), selection.clone(), selection.clone(), selection.clone());
        let (keys, press_texts, key_texts) = (selection.clone(), texts.clone(), texts.clone());
        let mut lines = div()
            .id("output-lines")
            .track_focus(&self.output_text_focus)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.output_scroll)
            .flex()
            .flex_col()
            .pad(px(8.0), px(12.0))
            .font_family(theme.mono_font.clone())
            .cursor(gpui::CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, move |e: &gpui::MouseDownEvent, window, _| {
                press.press(e.position, e.modifiers.shift, e.click_count, &|i| press_texts.get(i).cloned());
                window.refresh();
            })
            .on_mouse_move(move |e: &gpui::MouseMoveEvent, window, _| {
                if e.pressed_button == Some(MouseButton::Left) && drag.drag(e.position) {
                    window.refresh();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| release.release())
            .on_mouse_up_out(MouseButton::Left, move |_, _, _| release_out.release())
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| this.output_text_menu(e.position, window, cx)),
            )
            .on_key_down(move |e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if !k.modifiers.secondary() || k.modifiers.alt || k.modifiers.shift {
                    return;
                }
                match k.key.as_str() {
                    "c" => {
                        if let Some(text) = keys.copied(&|i| key_texts.get(i).cloned()) {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        }
                    }
                    "a" => {
                        keys.select_all(key_texts.len(), key_texts.last().map_or(0, |t| t.len()));
                        window.refresh();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            });
        let reveal = self.output_reveal.take();
        // New lines keep the view at the end while it is there (scrolled up to read, it stays put); a Find match to
        // bring into view goes first.
        let count = run.lines.len();
        if count < self.followed_lines.get() {
            self.followed_lines.set(0);
        }
        if reveal.is_none() && count > self.followed_lines.get() {
            let at_end = {
                let hidden = self.output_scroll.max_offset().height;
                hidden <= px(0.0) || -self.output_scroll.offset().y >= hidden - px(24.0)
            };
            if at_end || self.followed_lines.get() == 0 {
                self.output_scroll.scroll_to_item(count - 1);
            }
            self.followed_lines.set(count);
        }
        let mut stage_index = 0;
        for (index, line) in run.lines.iter().enumerate() {
            // A stage's heading carries its time on the right (`PipelineLog`).
            let took = (line.kind == LineKind::Stage).then(|| {
                let running = run.status == RunStatus::Running && stage_index + 1 == run.stage_starts().len();
                let took = run.stage_duration(stage_index, now).map(|d| span(d, running)).unwrap_or_default();
                stage_index += 1;
                took
            });
            let color = match line.kind {
                LineKind::Stage => c.out_heading,
                LineKind::Info => c.log_fg,
                LineKind::Success => c.success_text,
                LineKind::Warn => c.log_warn,
                LineKind::Fail => c.log_fail,
            };
            if reveal == Some(index) {
                self.output_scroll.scroll_to_item(index);
            }
            let (other, current) = ui::find_colors(cx);
            let marks: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = self
                .output_marks
                .get(&index)
                .map(|marks| {
                    marks
                        .iter()
                        .map(|(range, is_current)| {
                            let background_color = Some(if *is_current { current } else { other });
                            (range.clone(), gpui::HighlightStyle { background_color, ..Default::default() })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let text = selection.line(index, line.text.clone(), marks, cx);
            lines = lines.child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(10.0))
                    .when(line.kind == LineKind::Stage, |d| d.pad(px(3.0), px(0.0)).font_weight(FontWeight::SEMIBOLD))
                    .child(div().flex_none().text_color(c.log_muted).child(clock(line.at)))
                    // Wraps in the panel's width rather than running past it.
                    .child(div().flex_1().min_w_0().text_color(color).child(text))
                    .when_some(took, |d, took| {
                        d.child(div().flex_none().font_weight(FontWeight::NORMAL).text_color(c.log_muted).child(took))
                    }),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex().flex_1().min_h_0().child(rail).child(self.stages_sash(stages_width, cx)).child(lines))
    }

    /// The Output's right-click menu: Copy what is selected, Select all.
    fn output_text_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.output_text_focus);
        let Some(run) = self.store.read(cx).runs.last() else { return };
        let texts: std::rc::Rc<Vec<SharedString>> = std::rc::Rc::new(run.lines.iter().map(|l| l.text.clone()).collect());
        let selection = self.output_selection.clone();
        let keys = dnnm_ui::keys::Os::CURRENT;
        let shortcut = |key: &str| {
            Some(dnnm_ui::keys::label(keys, keys != dnnm_ui::keys::Os::Mac, false, false, keys == dnnm_ui::keys::Os::Mac, key))
        };
        let (copy_sel, copy_texts) = (selection.clone(), texts.clone());
        let (all_sel, all_texts) = (selection.clone(), texts.clone());
        let items = vec![
            dnnm_ui::menu::MenuItem::new("Copy", move |_, cx| {
                if let Some(text) = copy_sel.copied(&|i| copy_texts.get(i).cloned()) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
            })
            .enabled(selection.has_selection())
            .shortcut(shortcut("C")),
            dnnm_ui::menu::MenuItem::new("Select all", move |window, _| {
                all_sel.select_all(all_texts.len(), all_texts.last().map_or(0, |t| t.len()));
                window.refresh();
            })
            .enabled(!texts.is_empty())
            .shortcut(shortcut("A")),
        ];
        self.menu = Some(dnnm_ui::menu::ContextMenu::new(position, items));
        cx.notify();
    }

    /// The edge between the Output's stage list and its lines: a 1 px line, dragged sideways to make the list wider or
    /// narrower - lit in the accent, 4 px wide, under the mouse and while dragged (as the window's sashes).
    fn stages_sash(&self, width: gpui::Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Theme::get(cx).colors;
        let dragging = matches!(self.drag, Some(Drag::Stages { .. }));
        div()
            .id("output-stages-sash")
            .group("output-stages-sash")
            .relative()
            .flex_none()
            .w(px(1.0))
            .h_full()
            .bg(c.card_border)
            .child(
                // The grab area and the lit line: 4 px over the 1 px line - it takes no room from the list or lines.
                div()
                    .id("output-stages-grip")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-2.0))
                    .w(px(4.0))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .group_hover("output-stages-sash", move |s| s.bg(c.accent))
                    .when(dragging, |d| d.bg(c.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                            this.drag = Some(Drag::Stages { start_x: e.position.x, start_width: width });
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
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
        let iis_button = |id: &'static str, glyph: Icon, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.0))
                .rounded(px(3.0))
                .when(!busy, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                .when(!busy, |d| ui::focusable_bare(d, cx))
                .when(busy, |d| d.opacity(0.35))
                .tip(tip)
                .child(ui::icon(glyph, c.text_primary, cx))
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
                        Icon::Refresh,
                        "Restart IIS - every website on this computer is unavailable for a few seconds; asks first",
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.store.update(cx, |s, cx| s.control_iis(IisServerAction::Restart, cx))
                    })),
                )
                .child(
                    iis_button(
                        "iis-stop",
                        Icon::Stop,
                        "Stop IIS - every website on this computer stops until you start it again; asks first",
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.store.update(cx, |s, cx| s.control_iis(IisServerAction::Stop, cx))
                    })),
                )
            })
            .when(state == IisServerState::Stopped && !busy, |d| {
                d.child(
                    iis_button("iis-start", Icon::Play, "Start IIS, the web server, so your websites can be opened")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.store.update(cx, |s, cx| s.control_iis(IisServerAction::Start, cx))
                        })),
                )
            });
        let fmt = dnnm_application::project::view::byte_size;
        // Not read (yet): dashes, as the disk has - never a 0 that looks like a reading.
        let memory_known = host.memory_total_bytes > 0;
        let ram = if memory_known {
            format!("RAM {}", fmt(host.memory_used_bytes as f64))
        } else {
            "RAM --.-- GB".to_string()
        };
        let cpu = match host.cpu_percent {
            Some(percent) => format!("CPU {}%", dnnm_application::project::view::decimal(percent, 2)),
            None => "CPU --.--%".to_string(),
        };
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
                            .tip(if memory_known {
                                format!(
                                    "Memory in use: {} of {}",
                                    fmt(host.memory_used_bytes as f64),
                                    fmt(host.memory_total_bytes as f64)
                                )
                            } else {
                                "The memory in use can't be read".to_string()
                            })
                            .child(ram),
                    )
                    .child(
                        div()
                            .id("cpu")
                            .tip(format!(
                                "Processor use of this PC ({} logical processors)",
                                std::thread::available_parallelism().map_or(1, |n| n.get())
                            ))
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

    /// The toasts, bottom right of what holds them (the page; the window while the panel is maximized): the running
    /// operation, then one message - the newest at the bottom.
    fn render_toasts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let toasts = store.toasts.clone();
        let running = store.running_title();
        let undoing = store.undoing;
        let mut stack = div().absolute().right(px(16.0)).bottom(px(16.0)).flex().flex_col().items_end().gap(px(8.0));
        if let Some(title) = running {
            stack = stack.child(
                ui::focusable(div().id("operation-toast"), cx)
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
                                .tip(if undoing {
                                    "Stop undoing - what isn't undone yet stays as it is"
                                } else {
                                    "Cancel - DNN Manager undoes what the operation did"
                                })
                                .on_click(cx.listener(
                                    |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.store.update(cx, |s, cx| s.cancel_operation(cx));
                                    },
                                )),
                            ),
                    )
                    .child(div().pad(px(0.0), px(6.0)).child(progress_bar(&theme))),
            );
        }
        let waiting = toasts.len().saturating_sub(1);
        for toast in toasts.into_iter().take(1) {
            let (glyph, color) = match toast.kind {
                ToastKind::Success => (Icon::Check, c.success_text),
                ToastKind::Warning => (Icon::Warning, c.log_warn),
                ToastKind::Error => (Icon::Error, c.error_text),
            };
            let id = toast.id;
            stack = stack.child(
                div()
                    .id(SharedString::from(format!("toast-{id}")))
                    .flex()
                    .items_start()
                    .gap(px(icons::LABEL_GAP))
                    .w(px(360.0))
                    .pad(px(10.0), px(14.0))
                    .rounded(px(6.0))
                    .bg(c.card_bg)
                    .border_1()
                    .border_color(c.card_border)
                    .shadow_lg()
                    .occlude()
                    .child(ui::line_icon(glyph, IconSize::Medium, color, cx))
                    // `min_w_0`: the text wraps in the toast's width instead of running past it.
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(8.0)).child(toast.text.clone())
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
                            Icon::Close,
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

/// Something under way of unknown length: a third of a 2 px track running across it again and again (1.4 s), eased
/// at both ends - the whole track lit while animations are off.
fn progress_bar(theme: &Theme) -> gpui::AnyElement {
    use gpui::AnimationExt as _;
    let c = theme.colors;
    let track = div().relative().h(px(2.0)).rounded(px(1.0)).overflow_hidden().bg(c.card_border);
    if !theme.animations {
        return track.bg(c.accent).into_any_element();
    }
    track
        .child(
            div().absolute().top_0().h_full().w(gpui::relative(0.35)).rounded(px(1.0)).bg(c.accent).with_animation(
                "operation-progress",
                gpui::Animation::new(std::time::Duration::from_millis(1400)).repeat().with_easing(ui::motion_easing),
                // From just off the left end to just off the right one.
                |bar, t| bar.left(gpui::relative(-0.35 + 1.35 * t)),
            ),
        )
        .into_any_element()
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
    let (primary, secondary) = (w.primary_visible, w.secondary_visible);
    let (toggle_primary, toggle_secondary) = (weak.clone(), weak.clone());
    // Each option with the C# app's layout icon, the part it is about always filled in - the tick says whether it is shown.
    use dnnm_ui::shapes::layout;
    let right_sidebar = get("layout.sidebarPosition") == "right";
    let toggle_sidebar = weak.clone();
    let toggle_panel = weak.clone();
    group(
        &mut items,
        "Visibility",
        vec![
            PaletteItem::new("Menu Bar", setting("layout.menuBarVisible", if settings.menu_bar_visible() { "false" } else { "true" }))
                .icon(layout("menu-bar", true))
                .checked(settings.menu_bar_visible()),
            // The page entries' column (VS Code's activity bar), then the two side bars, empty for now.
            PaletteItem::new("Activity Bar", move |_, cx| {
                let _ = toggle_sidebar.update(cx, |ws, cx| ws.toggle_sidebar(cx));
            })
            .keywords("sidebar pages")
            .icon(layout(if right_sidebar { "activity-bar-right" } else { "activity-bar-left" }, true))
            .checked(sidebar),
            PaletteItem::new("Primary Side Bar", move |_, cx| {
                let _ = toggle_primary.update(cx, |ws, cx| ws.toggle_primary(cx));
            })
            .keywords("sidebar side bar")
            .shortcut("Ctrl+B")
            .icon(layout(if right_sidebar { "sidebar-right" } else { "sidebar-left" }, true))
            .checked(primary),
            PaletteItem::new("Secondary Side Bar", move |_, cx| {
                let _ = toggle_secondary.update(cx, |ws, cx| ws.toggle_secondary(cx));
            })
            .keywords("sidebar side bar")
            .shortcut("Ctrl+Alt+B")
            .icon(layout(if right_sidebar { "sidebar-left" } else { "sidebar-right" }, true))
            .checked(secondary),
            PaletteItem::new("Panel", move |_, cx| {
                let _ = toggle_panel.update(cx, |ws, cx| ws.toggle_panel(cx));
            })
            .shortcut("Ctrl+J")
            .icon(layout("panel", true))
            .checked(panel),
            PaletteItem::new(
                "Status Bar",
                setting("layout.statusBarVisible", if settings.status_bar_visible() { "false" } else { "true" }),
            )
            .icon(layout("status-bar", true))
            .checked(settings.status_bar_visible()),
        ],
    );
    let side = get("layout.sidebarPosition");
    group(
        &mut items,
        "Primary Side Bar Position",
        vec![
            // Where the activity bar and the primary side bar sit (the secondary on the other side); filled in either
            // way - the place is the picture.
            PaletteItem::new("Left", setting("layout.sidebarPosition", "left"))
                .icon(layout("activity-bar-left", true))
                .checked(side != "right"),
            PaletteItem::new("Right", setting("layout.sidebarPosition", "right"))
                .icon(layout("activity-bar-right", true))
                .checked(side == "right"),
        ],
    );
    let align = get("layout.panelAlignment");
    let center = !matches!(align.as_str(), "left" | "right" | "justify");
    group(
        &mut items,
        "Panel Alignment",
        vec![
            PaletteItem::new("Left", setting("layout.panelAlignment", "left"))
                .icon(layout("panel-left", true))
                .checked(align == "left"),
            PaletteItem::new("Right", setting("layout.panelAlignment", "right"))
                .icon(layout("panel-right", true))
                .checked(align == "right"),
            PaletteItem::new("Center", setting("layout.panelAlignment", "center"))
                .icon(layout("panel-center", true))
                .checked(center),
            PaletteItem::new("Justify", setting("layout.panelAlignment", "justify"))
                .icon(layout("panel-justify", true))
                .checked(align == "justify"),
        ],
    );
    let quick = get("layout.quickInputPosition");
    group(
        &mut items,
        "Quick Input Position",
        vec![
            PaletteItem::new("Top", setting("layout.quickInputPosition", "top"))
                .icon(layout("quick-input-top", false))
                .checked(quick != "center"),
            PaletteItem::new("Center", setting("layout.quickInputPosition", "center"))
                .icon(layout("quick-input-center", false))
                .checked(quick == "center"),
        ],
    );
    let density = get("layout.density");
    group(
        &mut items,
        "Layout Density",
        vec![
            PaletteItem::new("Default", setting("layout.density", "default"))
                .icon(layout("default", false))
                .checked(density != "compact"),
            PaletteItem::new("Compact", setting("layout.density", "compact"))
                .icon(layout("compact", false))
                .checked(density == "compact"),
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
        icons: dnnm_terminal::PanelIcons {
            add: Icon::Add.path(IconSize::Medium),
            chevron_down: Icon::ChevronDown.path(IconSize::Small),
            delete: Icon::Delete.path(IconSize::Medium),
            shell: Icon::Terminal.path(IconSize::Medium),
        },
        caption_size: theme.text.caption,
        body_size: theme.text.body,
    };
    (style, colors)
}

/// How far the title bar's logo starts on macOS: past the traffic lights (12 px in, three 14 px buttons 6 px apart)
/// and a gap as wide as the one in front of them.
pub const MAC_CAPTION: f32 = 78.0;

/// The command center's width: a share of the window, at most `max` (`SizeCommandCenter`).
fn relative_width(window: &Window, share: f32, max: f32) -> gpui::Pixels {
    let w = window.viewport_size().width * share;
    if w > px(max) { px(max) } else { w }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "14:03:27" in local time.
/// Byte-range matches in `lines` as the terminal's cells: (first column, columns, current) by line - a character a cell.
fn terminal_marks(
    lines: &[String],
    matches: &[dnnm_logs::search::Match],
    current: Option<usize>,
) -> std::collections::HashMap<usize, Vec<(usize, usize, bool)>> {
    let mut marks: std::collections::HashMap<usize, Vec<(usize, usize, bool)>> = std::collections::HashMap::new();
    for (i, m) in matches.iter().enumerate() {
        let Some(line) = lines.get(m.line) else { continue };
        let column = line[..m.start].chars().count();
        let columns = line[m.start..m.start + m.len].chars().count();
        marks.entry(m.line).or_default().push((column, columns, Some(i) == current));
    }
    marks
}

fn clock(at: SystemTime) -> String {
    dnnm_infrastructure::local_time::hms(at)
}

/// A time as the Output says it (`OutputRun.DurationText`): "0.25s", "14.61s" once known, whole seconds while it still
/// runs ("18s"); "2m 26s" past a minute, "1h 4m" past an hour.
fn span(d: Duration, running: bool) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if running {
        format!("{secs}s")
    } else {
        format!("{:.2}s", d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_the_output_says_them() {
        assert_eq!(span(Duration::from_millis(250), false), "0.25s");
        assert_eq!(span(Duration::from_millis(14_610), false), "14.61s");
        assert_eq!(span(Duration::from_millis(18_900), true), "18s");
        assert_eq!(span(Duration::from_secs(146), false), "2m 26s");
        assert_eq!(span(Duration::from_secs(3_840), true), "1h 4m");
    }
}
