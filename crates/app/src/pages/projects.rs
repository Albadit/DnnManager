//! The Projects page (`Pages/ProjectsPage.xaml`): every DNN website in IIS - its state, bindings, DNN install and
//! database - with search, the columns menu, "Only show running", ticking rows for the bulk actions, expanding a row
//! for its facts, and Start / Stop / Restart per row.

use dnnm_ui::px;
use dnnm_ui::{Icon, IconSize};
use std::collections::HashSet;

use gpui::{KeyBinding, actions, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, MouseButton, SharedString, Subscription, TextRun, Window, div, prelude::*};

use dnnm_application::iis::IisServerState;
use dnnm_application::project::{NONE, ProjectState, SiteRunState, view};
use dnnm_application::use_cases::SiteAction;
use dnnm_infrastructure::shell;

use super::columns::{Column, SortBy, compare};
use super::project_view::{ProjectView, ProjectViewAction, ProjectViewClosed, ProjectViewMenu};
use crate::store::{Connection, Store, ToastKind};
use dnnm_ui::theme::Theme;
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Pad, Tip};

/// How the table's columns lie: which are shown, their widths, and how far the middle is scrolled sideways.
#[derive(Clone, Copy)]
struct TableLayout<'a> {
    columns: &'a [Column],
    widths: &'a Widths,
    offset: f32,
    /// The table has the keyboard and the keyboard is in use: the selected row shows the focus.
    focus_ring: bool,
}

/// The "Open with…" submenu from what was found: editors, then each SSMS with its two entries under its name (WPF
/// nests them one level deeper; the menu here opens one level).
fn open_with_items(
    entries: &[dnnm_openwith::OpenWithEntry],
    project: &str,
    dir: &std::path::Path,
    tip: Option<&str>,
    weak: &gpui::WeakEntity<ProjectsPage>,
) -> Vec<dnnm_ui::menu::MenuItem> {
    use dnnm_openwith::EntryKind;
    use dnnm_ui::menu::MenuItem;
    let item = |entry: &dnnm_openwith::OpenWithEntry| {
        let (weak, request) = (
            weak.clone(),
            dnnm_wiring::open_with::OpenWithRequest {
                entry: entry.clone(),
                project_name: project.to_string(),
                project_dir: dir.to_path_buf(),
                database_tip: tip.map(str::to_string),
            },
        );
        MenuItem::new(entry.label.clone(), move |_, cx| {
            let request = request.clone();
            let _ = weak.update(cx, |this, cx| this.store.update(cx, |s, cx| s.open_with(request, cx)));
        })
        .tooltip(entry.tooltip.clone())
    };
    let mut items = Vec::new();
    for entry in entries {
        match &entry.kind {
            EntryKind::Separator => items.push(MenuItem::Separator),
            EntryKind::Disabled => items.push(MenuItem::note(entry.label.clone()).tooltip(entry.tooltip.clone())),
            EntryKind::Ssms(sub) => {
                items.push(MenuItem::Heading(entry.label.clone().into()));
                for e in sub {
                    items.push(match e.kind {
                        EntryKind::Disabled => MenuItem::note(e.label.clone()).tooltip(e.tooltip.clone()),
                        _ => item(e),
                    });
                }
            }
            _ => items.push(item(entry)),
        }
    }
    items
}

/// The keep-warm flame for a look (`KeepWarmFlame`): outline while off, filled in the flame colour while warm,
/// greyed while paused, with a red dot when something is wrong.
/// Pulses while a ping is under way (`FLAME_PULSE`), with animations on.
fn flame_icon(
    id: impl Into<gpui::ElementId>,
    look: dnnm_keepwarm::view::KeepWarmLook,
    muted: Hsla,
    cx: &gpui::App,
) -> gpui::AnyElement {
    use dnnm_keepwarm::view::{FLAME_DOT_OVERHANG, FLAME_DOT_SIZE, FLAME_PULSE, FLAME_SIZE, FlameBrush, flame, flame_width};
    let theme = Theme::get(cx);
    let style = flame(look);
    let color = match style.brush {
        FlameBrush::TextMuted => muted,
        FlameBrush::KeepWarmFg => theme.colors.keep_warm_fg,
    };
    // In the same box as a glyph of its size (`ui::icon`), centred - so it lines up with the icons beside it.
    let icon = div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(px(FLAME_SIZE + 2.0))
        .child(
            gpui::svg()
                .path(crate::assets::flame_asset(style.path))
                .w(px(flame_width(FLAME_SIZE)))
                .h(px(FLAME_SIZE))
                .text_color(color),
        )
        .when(style.dot, |d| {
            d.child(
                div()
                    .absolute()
                    // At the bottom right, 2 px past the side and 1 px past the bottom (`FLAME_DOT_OVERHANG`).
                    .bottom(px(-FLAME_DOT_OVERHANG.1))
                    .right(px(-FLAME_DOT_OVERHANG.0))
                    .size(px(FLAME_DOT_SIZE))
                    .rounded_full()
                    .bg(theme.colors.error_text),
            )
        });
    if style.pulse {
        let (high, low, half) = FLAME_PULSE;
        ui::pulse(icon.opacity(high), id, low, half * 2, cx)
    } else {
        icon.into_any_element()
    }
}

/// The Actions column: 56 for Start/Stop and Restart, 28 for keep warm, the divider and Remove, and padding.
const ACTIONS_WIDTH: f32 = 148.0;

/// The columns' widths for the rows shown: each as wide as its widest text plus the cell's 8 px padding on each
/// side, never under the XAML column's MinWidth - WPF's `Width="Auto"`.
struct Widths {
    name: f32,
    columns: Vec<(Column, f32)>,
}

impl Widths {
    fn measure(window: &Window, theme: &Theme, rows: &[ProjectState], columns: &[Column]) -> Self {
        let size = theme.text.body;
        let measure = |text: &str, bold: bool| -> f32 {
            if text.is_empty() {
                return 0.0;
            }
            let mut font = gpui::font(theme.font.clone());
            if bold {
                font.weight = FontWeight::SEMIBOLD;
            }
            let run = TextRun {
                len: text.len(),
                font,
                color: theme.colors.text_primary,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(SharedString::from(text.to_string()), size, &[run], None);
            line.width / px(1.0)
        };
        const PADDING: f32 = 16.0;
        // The header is semibold and leaves room for the sort arrow.
        let header = |text: &str| measure(text, true) + PADDING + 12.0;
        let name = rows.iter().map(|p| measure(&p.name, true) + PADDING).fold(header("Name").max(150.0), f32::max);
        let columns = columns
            .iter()
            .map(|c| {
                let widest = rows.iter().map(|p| measure(&c.text(p), false) + PADDING).fold(0.0, f32::max);
                (*c, widest.max(header(c.header())).max(c.min_width()).ceil())
            })
            .collect();
        Self { name: name.ceil(), columns }
    }

    /// Check box, expander, status dot and Name - frozen on the left (`FrozenColumnCount="4"`).
    fn frozen(&self) -> f32 {
        40.0 + 32.0 + 24.0 + self.name
    }

    /// The scrolling columns' total width.
    fn middle(&self, columns: &[Column]) -> f32 {
        columns.iter().map(|c| self.of(*c)).sum()
    }

    fn of(&self, column: Column) -> f32 {
        self.columns.iter().find(|(c, _)| *c == column).map_or(column.min_width(), |(_, w)| *w)
    }
}

actions!(
    projects_table,
    [
        SelectDown,
        SelectUp,
        TickRow,
        TickAll,
        ExpandRow,
        CollapseRow,
        OpenRow,
        StartRow,
        StopRow,
        RestartRow,
        RowMenu,
        RemoveRow
    ]
);

/// The table's keys (`ProjectsPage.Grid_PreviewKeyDown`), only while the table has the keyboard.
pub fn bind_keys(cx: &mut gpui::App) {
    let context = Some("ProjectsTable");
    cx.bind_keys([
        KeyBinding::new("down", SelectDown, context),
        KeyBinding::new("up", SelectUp, context),
        KeyBinding::new("space", TickRow, context),
        KeyBinding::new("secondary-a", TickAll, context),
        KeyBinding::new("right", ExpandRow, context),
        KeyBinding::new("left", CollapseRow, context),
        KeyBinding::new("enter", OpenRow, context),
        KeyBinding::new("f5", StartRow, context),
        KeyBinding::new("shift-f5", StopRow, context),
        KeyBinding::new("secondary-shift-r", RestartRow, context),
        KeyBinding::new("shift-f10", RowMenu, context),
        KeyBinding::new("delete", RemoveRow, context),
    ]);
}

/// What the page asks of the window.
pub enum ProjectsEvent {
    /// "Open in terminal": a terminal in the project's folder.
    OpenTerminal(std::path::PathBuf),
    /// "View logs": the Logs tab on the site.
    ShowLogs(String),
    /// An edit of a project (its dialog, then its operation).
    Action(crate::workspace::ProjectAction, String),
    /// A backup, or putting one back - run without a dialog.
    Run(ProjectRun, String),
}

/// What a project's menu runs without a dialog: Export ▸ a backup, or Restore backup ▸ one of its backups.
#[derive(Debug, Clone)]
pub enum ProjectRun {
    Backup(dnnm_wiring::backups::BackupKind),
    Restore(dnnm_application::use_cases::backups::ProjectBackup),
}

impl gpui::EventEmitter<ProjectsEvent> for ProjectsPage {}

pub struct ProjectsPage {
    store: Entity<Store>,
    search: Entity<TextInput>,
    query: String,
    only_running: bool,
    checked: HashSet<String>,
    expanded: HashSet<String>,
    /// None until a header is clicked: sorted by name, without an arrow (as the WPF table starts).
    sort: Option<(SortBy, bool)>,
    columns_open: bool,
    /// Where the Columns button is: its popup opens under it.
    columns_anchor: ui::Anchor,
    /// A project's Details, shown in place of the table.
    details: Option<Entity<ProjectView>>,
    /// Where the tour finds the table and the toolbar.
    pub tour_targets: Option<dnnm_dialogs::tour::TourTargets>,
    /// The row the open menu is for - the "Open with…" entries arrive later and are only put in that row's menu.
    menu_row: Option<String>,
    /// The row the keyboard and the menu act on (`DataGrid.SelectedItem`).
    selected: Option<String>,
    /// The right-click menu, while it is open.
    menu: Option<dnnm_ui::menu::ContextMenu>,
    table_focus: FocusHandle,
    /// How far the middle columns are scrolled sideways; the frozen columns and Actions stay put.
    h_offset: f32,
    /// The middle columns' visible width, measured as the table is laid out.
    view_width: f32,
    /// A drag of the horizontal scrollbar's thumb: where the mouse and the offset were when it began.
    thumb_drag: Option<(f32, f32)>,
    /// The middle columns' full width while a thumb drag goes on.
    drag_content_width: f32,
    /// Where the scrollbar's track starts in the window, for a click beside the thumb.
    track_x: f32,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ProjectsPage {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::search("Search projects", cx));
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, _, e: &TextChanged, cx| {
                this.query = e.0.trim().to_lowercase();
                cx.notify();
            }),
        ];
        Self {
            store,
            search,
            query: String::new(),
            only_running: false,
            checked: HashSet::new(),
            expanded: HashSet::new(),
            sort: None,
            columns_open: false,
            columns_anchor: ui::Anchor::default(),
            details: None,
            tour_targets: None,
            selected: None,
            menu: None,
            menu_row: None,
            // One tab stop for the whole table: the arrows, Enter, F5… work its rows (`bind_keys`); the controls in
            // the rows are left out of Tab's way (`tab_stop(false)` in `render_row`).
            table_focus: cx.focus_handle().tab_stop(true),
            h_offset: 0.0,
            view_width: 0.0,
            thumb_drag: None,
            drag_content_width: 0.0,
            track_x: 0.0,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.search.read(cx).focus_handle(cx);
        window.focus(&handle);
    }

    fn columns(&self, cx: &Context<Self>) -> Vec<Column> {
        let chosen = self.store.read(cx).settings.project_columns();
        // Shown in the menu's order, whatever order they were saved in.
        Column::ALL.into_iter().filter(|c| chosen.iter().any(|k| k == c.key())).collect()
    }

    fn set_columns(&mut self, columns: Vec<Column>, cx: &mut Context<Self>) {
        let keys: Vec<String> = columns.iter().map(|c| c.key().to_string()).collect();
        self.store.update(cx, |s, cx| s.update_settings(|settings| settings.set_project_columns(&keys), cx));
    }

    /// The rows the search and "Only show running" leave, sorted.
    fn visible(&self, projects: &[ProjectState]) -> Vec<ProjectState> {
        let mut rows: Vec<ProjectState> = projects
            .iter()
            .filter(|p| !self.only_running || view::run_state(&p.site) == SiteRunState::Running)
            .filter(|p| self.query.is_empty() || view::search_key(p).contains(&self.query))
            .cloned()
            .collect();
        let (by, ascending) = self.sort.unwrap_or((SortBy::Name, true));
        rows.sort_by(|a, b| if ascending { compare(by, a, b) } else { compare(by, b, a) });
        rows
    }

    /// Shows a project's Details in place of the table (the name's link, Enter, "Open project details").
    pub fn open_details(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| ProjectView::new(self.store.clone(), name.to_string(), cx));
        // The details have the keyboard while open (Esc goes back); closed, the table has it again, on the same row.
        window.focus(&view.read(cx).focus);
        let sub = cx.subscribe_in(&view, window, |this, _, _: &ProjectViewClosed, window, cx| {
            this.details = None;
            window.focus(&this.table_focus);
            cx.notify();
        });
        self._subscriptions.push(sub);
        let more = cx.subscribe(&view, |this, view, e: &ProjectViewMenu, cx| {
            let name = view.read(cx).name.clone();
            this.open_menu(&name, e.0, cx);
            if let Some(menu) = this.menu.as_mut() {
                menu.anchor = e.1;
            }
        });
        self._subscriptions.push(more);
        let action = cx.subscribe(&view, |_, _, e: &ProjectViewAction, cx| {
            cx.emit(ProjectsEvent::Action(e.0, e.1.clone()));
        });
        self._subscriptions.push(action);
        self.details = Some(view);
        cx.notify();
    }

    /// Whether the Columns popup or a project's menu is open (Esc closes it).
    pub fn columns_open(&self) -> bool {
        self.columns_open || self.menu.is_some()
    }

    /// Esc on the page: the Columns popup closes first, then the Details go back to the table; false when neither was
    /// open (Esc goes on to what is around it). Back from the Details, the table has the keyboard again.
    pub fn close_details(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.menu.take().is_some() {
            cx.notify();
            return true;
        }
        if self.columns_open {
            self.columns_open = false;
            cx.notify();
            return true;
        }
        let open = self.details.take().is_some();
        if open {
            window.focus(&self.table_focus);
        }
        cx.notify();
        open
    }

    /// The project whose Details are shown, and their tab - kept for the next start.
    pub fn details_of(&self, cx: &gpui::App) -> Option<(String, String)> {
        self.details.as_ref().map(|d| {
            let view = d.read(cx);
            (view.name.clone(), view.tab().title().to_string())
        })
    }

    /// The Details as they were left: the project, on its tab.
    pub fn restore_details(&mut self, name: &str, tab: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_details(name, window, cx);
        let tab = dnnm_diagnostics::TabKind::ALL.into_iter().find(|k| Some(k.title()) == tab);
        if let (Some(tab), Some(view)) = (tab, self.details.clone()) {
            view.update(cx, |v, cx| v.set_tab(tab, cx));
        }
    }

    /// Puts the table back as it was left (`ProjectsTableState`).
    pub fn restore(&mut self, state: &crate::workspace_state::ProjectsTableState, cx: &mut Context<Self>) {
        self.only_running = state.only_running;
        self.sort = state.sort_by.as_deref().and_then(SortBy::from_key).map(|by| (by, !state.sort_descending));
        self.expanded = state.expanded.iter().cloned().collect();
        self.selected = state.selected.clone();
        if let Some(search) = state.search.clone().filter(|s| !s.is_empty()) {
            self.search.update(cx, |s, cx| s.set_text(search, cx));
        }
        cx.notify();
    }

    /// The table as it is now, to keep for the next start.
    pub fn capture(&self, cx: &gpui::App) -> crate::workspace_state::ProjectsTableState {
        let mut expanded: Vec<String> = self.expanded.iter().cloned().collect();
        expanded.sort();
        let search = self.search.read(cx).text().to_string();
        crate::workspace_state::ProjectsTableState {
            search: (!search.is_empty()).then_some(search),
            only_running: self.only_running,
            sort_by: self.sort.map(|(by, _)| by.key().to_string()),
            sort_descending: self.sort.is_some_and(|(_, ascending)| !ascending),
            expanded,
            selected: self.selected.clone(),
        }
    }

    /// Moves the selection down (1) or up (-1) the rows shown.
    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows = self.visible(&self.store.read(cx).projects);
        if rows.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|s| rows.iter().position(|p| &p.name == s));
        let next = match at {
            None => 0,
            Some(i) => (i as isize + by).clamp(0, rows.len() as isize - 1) as usize,
        };
        self.selected = Some(rows[next].name.clone());
        cx.notify();
    }

    fn selected_row(&self, cx: &gpui::App) -> Option<ProjectState> {
        let name = self.selected.as_ref()?;
        self.store.read(cx).projects.iter().find(|p| &p.name == name).cloned()
    }

    fn select(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(name.to_string());
        window.focus(&self.table_focus);
        cx.notify();
    }

    /// The right-click menu of a row (`ProjectMenu.Fill`), in its order - what isn't ported isn't listed.
    fn open_menu(&mut self, name: &str, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        use dnnm_ui::menu::MenuItem;
        let store = self.store.read(cx);
        let Some(p) = store.projects.iter().find(|p| p.name == name).cloned() else { return };
        let busy = store.is_busy();
        let idle = !busy && !store.pending.contains_key(name);
        let status = store.keep_warm_status(name);
        let state = view::run_state(&p.site);
        let weak = cx.entity().downgrade();
        let mut items = Vec::new();
        let control = |action: SiteAction| {
            let (weak, name) = (weak.clone(), name.to_string());
            move |_: &mut Window, cx: &mut gpui::App| {
                let _ = weak.update(cx, |this, cx| this.control(vec![name.clone()], action, cx));
            }
        };
        if matches!(state, SiteRunState::Stopped | SiteRunState::Stopping | SiteRunState::Unknown) {
            items.push(
                MenuItem::new("Start", control(SiteAction::Start))
                    .enabled(idle && matches!(state, SiteRunState::Stopped | SiteRunState::Unknown)),
            );
        }
        if matches!(state, SiteRunState::Running | SiteRunState::Starting) {
            items.push(MenuItem::new("Stop", control(SiteAction::Stop)).enabled(idle && state == SiteRunState::Running));
            items.push(
                MenuItem::new("Restart", control(SiteAction::Restart)).enabled(idle && state == SiteRunState::Running),
            );
        }
        let on = status.is_on();
        let has_url = !p.site_url.is_empty();
        {
            let (weak, name) = (weak.clone(), name.to_string());
            items.push(
                MenuItem::new(dnnm_keepwarm::view::action(on), move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let name = name.clone();
                        this.store.update(cx, |s, cx| s.set_keep_warm(&[name], !on, cx));
                    });
                })
                .enabled(dnnm_keepwarm::view::can_toggle(on, has_url))
                .tooltip(if on {
                    Some(status.text.to_string())
                } else {
                    dnnm_keepwarm::view::unavailable(on, has_url).map(str::to_string)
                }),
            );
        }
        items.push(MenuItem::Separator);
        // "Open with…": filled when the editors and SSMS have been looked for (off the UI thread).
        items.push(MenuItem::submenu(dnnm_wiring::open_with::OPEN_WITH, vec![MenuItem::note(dnnm_wiring::open_with::READING)]));
        let open_with_index = items.len() - 1;
        items.push(MenuItem::Separator);
        {
            let (weak, name) = (weak.clone(), name.to_string());
            items.push(
                MenuItem::new("Details…", move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.open_details(&name, window, cx));
                })
                .bold(),
            );
        }
        {
            let (weak, url) = (weak.clone(), p.site_url.clone());
            items.push(
                MenuItem::new("Open site", move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.open_url(&url, cx));
                })
                .enabled(has_url),
            );
        }
        {
            let (weak, dir) = (weak.clone(), p.directory.clone());
            items.push(MenuItem::new("Open folder", move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.open_folder(&dir, cx));
            }));
        }
        {
            let (weak, dir) = (weak.clone(), p.directory.clone());
            items.push(
                MenuItem::new("Open in terminal", move |_, cx| {
                    let dir = dir.clone();
                    let _ = weak.update(cx, |_, cx| cx.emit(ProjectsEvent::OpenTerminal(dir)));
                })
                .enabled(p.directory.is_dir()),
            );
        }
        items.push(MenuItem::Separator);
        {
            let (weak, name) = (weak.clone(), name.to_string());
            let tip = if dnnm_wiring::clear_site_cache::is_dnn(&p.directory) {
                dnnm_wiring::clear_site_cache::CLEAR_CACHE_TOOLTIP_DNN
            } else {
                dnnm_wiring::clear_site_cache::CLEAR_CACHE_TOOLTIP_OTHER
            };
            items.push(
                MenuItem::new(dnnm_wiring::clear_site_cache::CLEAR_CACHE_MENU_ITEM, move |_, cx| {
                    let name = name.clone();
                    let _ = weak.update(cx, |this, cx| this.store.update(cx, |s, cx| s.clear_cache(name, cx)));
                })
                .enabled(idle)
                .tooltip(Some(tip)),
            );
        }
        {
            let (weak, name) = (weak.clone(), name.to_string());
            items.push(MenuItem::new("View logs", move |_, cx| {
                let name = name.clone();
                let _ = weak.update(cx, |_, cx| cx.emit(ProjectsEvent::ShowLogs(name)));
            }));
        }
        items.push(MenuItem::Separator);
        // Rename…, Clone…, Upgrade DNN… (a DNN site), Export ▸, Restore backup ▸ - then Remove… (`ProjectMenu.Fill`).
        use crate::workspace::ProjectAction;
        use dnnm_wiring::backups::{BackupKind, texts};
        let can_edit = dnnm_wiring::rename_project::can_edit(busy, &p.directory);
        let edit = |action: ProjectAction| {
            let (weak, name) = (weak.clone(), name.to_string());
            let tip = action.tooltip();
            MenuItem::new(action.label(), move |_, cx| {
                let name = name.clone();
                let _ = weak.update(cx, |_, cx| cx.emit(ProjectsEvent::Action(action, name)));
            })
            .enabled(can_edit)
            .tooltip((!tip.is_empty()).then_some(tip))
        };
        let run = |label: &'static str, run: ProjectRun, enabled: bool| {
            let (weak, name) = (weak.clone(), name.to_string());
            MenuItem::new(label, move |_, cx| {
                let (run, name) = (run.clone(), name.clone());
                let _ = weak.update(cx, |_, cx| cx.emit(ProjectsEvent::Run(run, name)));
            })
            .enabled(enabled)
        };
        items.push(edit(ProjectAction::Rename));
        items.push(edit(ProjectAction::Clone));
        if p.dnn_version.is_some() {
            items.push(edit(ProjectAction::UpgradeDnn));
        }
        let backups_folder = dnnm_wiring::backups::backups_folder(name);
        let open_backups = {
            let (weak, folder) = (weak.clone(), backups_folder.clone());
            MenuItem::new(texts::OPEN_BACKUPS_FOLDER, move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.open_folder(&folder, cx));
            })
            .enabled(backups_folder.is_dir())
        };
        items.push(MenuItem::submenu(
            texts::EXPORT,
            vec![
                edit(ProjectAction::ExportForDeployment),
                MenuItem::Separator,
                run(BackupKind::Both.label(), ProjectRun::Backup(BackupKind::Both), !busy),
                run(BackupKind::Site.label(), ProjectRun::Backup(BackupKind::Site), !busy),
                run(BackupKind::Database.label(), ProjectRun::Backup(BackupKind::Database), !busy),
                MenuItem::Separator,
                open_backups,
            ],
        ));
        // Restore backup ▸: the complete backups, newest first - read off the UI thread, "Reading…" until then.
        items.push(MenuItem::submenu(texts::RESTORE_BACKUP, vec![MenuItem::note(texts::READING)]).enabled(p.directory.is_dir()));
        let restore_index = items.len() - 1;
        {
            let (weak, name) = (weak.clone(), name.to_string());
            items.push(
                MenuItem::new(dnnm_wiring::remove_project::REMOVE_MENU_ITEM, move |_, cx| {
                    let name = name.clone();
                    let _ = weak.update(cx, |this, cx| this.store.update(cx, |s, cx| s.remove_projects(vec![name], cx)));
                })
                .enabled(idle),
            );
        }
        self.menu = Some(dnnm_ui::menu::ContextMenu::new(position, items));
        self.menu_row = Some(name.to_string());
        let services = store.services();
        let (dir, database, tip) = (p.directory.clone(), p.database_name.clone(), view::database_tip(&p));
        let row = name.to_string();
        {
            let row = name.to_string();
            cx.spawn(async move |this, cx| {
                let site = row.clone();
                let backups = cx
                    .background_executor()
                    .spawn(async move { dnnm_wiring::backups::restorable_backups(&site) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.menu_row.as_deref() != Some(row.as_str()) {
                        return;
                    }
                    let weak = cx.entity().downgrade();
                    let mut items: Vec<MenuItem> = backups
                        .into_iter()
                        .map(|backup| {
                            let (weak, row) = (weak.clone(), row.clone());
                            let (label, folder) = (backup.menu_label(), backup.folder.display().to_string());
                            MenuItem::new(label, move |_, cx| {
                                let run = ProjectRun::Restore(backup.clone());
                                let row = row.clone();
                                let _ = weak.update(cx, |_, cx| cx.emit(ProjectsEvent::Run(run, row)));
                            })
                            .enabled(can_edit)
                            .tooltip(Some(folder))
                        })
                        .collect();
                    if items.is_empty() {
                        items.push(MenuItem::note(dnnm_wiring::backups::texts::NO_BACKUP));
                    }
                    if let Some(menu) = this.menu.as_mut()
                        && let Some(MenuItem::Submenu { items: sub, .. }) = menu.items.get_mut(restore_index)
                    {
                        *sub = items;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.spawn(async move |this, cx| {
            let (d, db, t) = (dir.clone(), database.clone(), tip.clone());
            let entries = cx
                .background_executor()
                .spawn(async move { dnnm_wiring::open_with::entries(&services, &d, db.as_deref(), t.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.menu_row.as_deref() != Some(row.as_str()) {
                    return;
                }
                let weak = cx.entity().downgrade();
                let items = open_with_items(&entries, &row, &dir, tip.as_deref(), &weak);
                if let Some(menu) = this.menu.as_mut()
                    && let Some(MenuItem::Submenu { items: sub, enabled, .. }) = menu.items.get_mut(open_with_index)
                {
                    *enabled = !items.is_empty();
                    *sub = items;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens the Columns popup ("Projects: Choose columns…").
    pub fn open_columns(&mut self, cx: &mut Context<Self>) {
        self.columns_open = true;
        cx.notify();
    }

    pub fn sort_by(&mut self, by: SortBy, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            Some((current, ascending)) if current == by => Some((by, !ascending)),
            _ => Some((by, true)),
        };
        cx.notify();
    }

    fn control(&mut self, names: Vec<String>, action: SiteAction, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.control_sites(names, action, cx));
    }

    fn open_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Err(e) = shell::open_url(url) {
            self.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
        }
    }

    fn open_folder(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Err(e) = shell::open_folder(path) {
            self.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
        }
    }

    /// The text over the empty table (`ProjectsPage.xaml.cs:329-337`); None while it has rows.
    fn overlay(&self, store: &Store, visible: usize, q: &str) -> Option<String> {
        if visible > 0 {
            return None;
        }
        Some(match () {
            _ if store.connection == Connection::Connecting => "Loading projects…".into(),
            // IIS is a Windows web server: elsewhere the projects folder's DNN folders are listed.
            _ if !cfg!(windows) && store.projects.is_empty() && q.is_empty() => {
                format!(
                    "IIS runs only on Windows, so the DNN folders in your projects folder are listed here - {} has none. Choose the folder in Settings → Projects.",
                    store.settings.base_directory()
                )
            }
            _ if store.iis_state == IisServerState::NotInstalled => {
                "IIS isn't installed - enable its Windows features in Settings → IIS.".into()
            }
            _ if store.projects.is_empty() => {
                if let Connection::Reconnecting(reason) = &store.connection {
                    reason.clone()
                } else {
                    "IIS has no DNN websites yet - create one with New project.".into()
                }
            }
            _ if self.only_running && q.is_empty() => "No project is running.".into(),
            _ if self.only_running => format!("No running project matches \"{q}\"."),
            _ => format!("No project matches \"{q}\"."),
        })
    }
}

impl Focusable for ProjectsPage {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProjectsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(details) = self.details.clone() {
            // Details' "More actions" opens the row's menu over it.
            return div()
                .id("projects-page")
                .track_focus(&self.focus)
                .relative()
                .size_full()
                .child(details)
                .children(self.render_row_menu(window, cx));
        }
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let all_count = store.projects.len();
        let rows = self.visible(&store.projects);
        let busy = store.is_busy();
        let pending = store.pending.clone();
        let connection = store.connection.clone();
        let query_text = self.search.read(cx).text().trim().to_string();
        let overlay = self.overlay(store, rows.len(), &query_text);
        let columns = self.columns(cx);
        let widths = Widths::measure(window, &theme, &rows, &columns);
        // Ticks of rows that went (removed, filtered away) don't count.
        self.checked.retain(|n| rows.iter().any(|p| &p.name == n));
        let checked_rows: Vec<&ProjectState> = rows.iter().filter(|p| self.checked.contains(&p.name)).collect();

        // ─── Header: title, subtitle, and "Reconnecting…" while the monitor can't read ───
        // 12 px above it (24 from the top with the page's 12) and 12 below it (16 to the toolbar).
        let header = div()
            .flex()
            .justify_between()
            .pad(px(12.0), px(0.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(ui::with_tip_sized(
                        div()
                            .text_size(theme.text.display)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.text_primary)
                            .child("Projects"),
                        theme.text.display,
                        "projects-about",
                        "Every DNN website in IIS - its state, bindings, DNN install and database.",
                        cx,
                    )),
            )
            .when_some(
                match &connection {
                    Connection::Reconnecting(reason) => Some(reason.clone()),
                    _ => None,
                },
                |d, reason| {
                    d.child(
                        div()
                            .id("live")
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .pad(px(0.0), px(2.0))
                            .text_size(theme.text.caption)
                            .text_color(c.text_muted)
                            .tip(reason)
                            .child(ui::status_dot(c.log_warn, false, px(8.0)))
                            .child("Reconnecting…"),
                    )
                },
            );

        // ─── Toolbar: search, columns, only running; bulk actions on the right while rows are ticked ───
        let bulk_names: Vec<String> = checked_rows.iter().map(|p| p.name.clone()).collect();
        let any_stopped = checked_rows
            .iter()
            .any(|p| matches!(view::run_state(&p.site), SiteRunState::Stopped | SiteRunState::Unknown));
        let any_running = checked_rows.iter().any(|p| view::run_state(&p.site) == SiteRunState::Running);
        let group_button = |id: &'static str, glyph: Icon, tip: &'static str, enabled: bool, color: Hsla| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(34.0))
                .h(px(26.0))
                .rounded(px(3.0))
                .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                .when(enabled, |d| ui::focusable_bare(d, cx))
                .tip(tip)
                .child(ui::icon(glyph, color, cx).when(!enabled, |d| d.opacity(0.35)))
        };
        let bulk = (!checked_rows.is_empty()).then(|| {
            let start_names = bulk_names.clone();
            let stop_names = bulk_names.clone();
            let restart_names = bulk_names.clone();
            // The buttons side by side; the divider before Remove 2 px from each.
            let buttons = div()
                .flex()
                .items_center()
                .child(
                    group_button(
                        "bulk-start",
                        Icon::Play,
                        "Start the selected websites",
                        !busy && any_stopped,
                        c.text_primary,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.control(start_names.clone(), SiteAction::Start, cx)
                    })),
                )
                .child(
                    group_button(
                        "bulk-stop",
                        Icon::Stop,
                        "Stop the selected websites - nothing is deleted",
                        !busy && any_running,
                        c.text_primary,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.control(stop_names.clone(), SiteAction::Stop, cx)
                    })),
                )
                .child(
                    group_button(
                        "bulk-restart",
                        Icon::Refresh,
                        "Restart the selected websites - each is unavailable for a few seconds",
                        !busy && any_running,
                        c.text_primary,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.control(restart_names.clone(), SiteAction::Restart, cx)
                    })),
                )
                .child({
                    let names = bulk_names.clone();
                    let store = self.store.read(cx);
                    let all_warm = names.iter().all(|n| store.keep_warm_status(n).is_on());
                    let with_url = checked_rows.iter().any(|p| !p.site_url.is_empty());
                    div()
                        .id("bulk-keepwarm")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(34.0))
                        .h(px(26.0))
                        .rounded(px(3.0))
                        .when(all_warm || with_url, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                        .when(all_warm || with_url, |d| ui::focusable_bare(d, cx))
                        .tip(dnnm_keepwarm::view::bulk_tooltip(all_warm))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let names = names.clone();
                            this.store.update(cx, |s, cx| s.set_keep_warm(&names, !all_warm, cx));
                        }))
                        .child(flame_icon(
                            "bulk-flame",
                            if all_warm {
                                dnnm_keepwarm::view::KeepWarmLook::Warm
                            } else {
                                dnnm_keepwarm::view::KeepWarmLook::Off
                            },
                            c.text_primary,
                            cx,
                        ))
                });
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .p(px(2.0))
                .rounded(px(5.0))
                .bg(c.card_bg)
                .border_1()
                .border_color(c.card_border)
                .child(buttons)
                .child(div().w(px(1.0)).h(px(18.0)).bg(c.card_border))
                .child({
                    let names = bulk_names.clone();
                    let idle = !busy && checked_rows.iter().all(|p| !pending.contains_key(&p.name));
                    group_button(
                        "bulk-remove",
                        Icon::Delete,
                        dnnm_wiring::remove_project::REMOVE_BULK_TOOLTIP,
                        idle,
                        c.error_text,
                    )
                    .when(idle, |b| {
                        b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let names = names.clone();
                            this.store.update(cx, |s, cx| s.remove_projects(names, cx));
                        }))
                    })
                })
        });

        let columns_button = ui::focusable_bare(div().id("columns"), cx)
            .flex()
            .items_center()
            .justify_center()
            .size(px(32.0))
            .rounded(theme.metrics.control_radius)
            .cursor_pointer()
            .when(self.columns_open, |d| d.bg(c.selection_bg))
            .hover(move |s| s.bg(c.hover_bg))
            .tip("Choose which columns the list shows")
            .relative()
            .child(self.columns_anchor.measure())
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.columns_open = !this.columns_open;
                cx.notify();
            }))
            // At the sidebar's size: at 13 px its frame came out 10 px wide, small in the 32 px button.
            .child(ui::icon_sized(
                Icon::Columns,
                dnnm_ui::IconSize::Nav,
                if self.columns_open { c.link_fg } else { c.text_primary },
                cx,
            ));

        let only_running = self.only_running;
        let toolbar = div()
            .relative()
            .children(self.tour_targets.as_ref().map(|t| dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::ProjectsToolbar, t)))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("search")
                            .flex()
                            .items_center()
                                    .gap(px(6.0))
                                    .w(px(280.0))
                                    .h(theme.metrics.search_height)
                                    .pad(px(0.0), px(6.0))
                                    .rounded(theme.metrics.control_radius)
                                    .border_1()
                                    .border_color(c.field_border)
                                    .bg(c.input_bg)
                                    .text_color(c.text_primary)
                                    .tip("Search projects")
                                    .child(self.search.clone()),
                            )
                            .child(columns_button),
                    )
                    .child(div().child(ui::switch(
                        "only-running",
                        only_running,
                        "Only show running",
                        cx.listener(|this, on: &bool, _, cx| {
                            this.only_running = *on;
                            cx.notify();
                        }),
                        window,
                        cx,
                    ))),
            )
            .children(bulk);

        // ─── The table ───
        let all_checked = !rows.is_empty() && rows.iter().all(|p| self.checked.contains(&p.name));
        let sort = self.sort;
        let sort_mark = move |by: SortBy| {
            sort.and_then(|(current, ascending)| {
                (current == by).then_some(if ascending { Icon::ChevronUp } else { Icon::ChevronDown })
            })
        };
        // `inset`: the cell's room left and right - 8 px, as the rows' cells (the widths include it).
        let header_cell =
            |id: SharedString, label: &'static str, width: Option<f32>, by: Option<SortBy>, right: bool, inset: f32| {
                div()
                    .id(id)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .when(right, |d| d.justify_end())
                    .when_some(width, |d, w| d.w(px(w)))
                    .pad(px(6.0), px(inset))
                    .when(by.is_some(), |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                    .when(by.is_some(), |d| ui::focusable_bare(d, cx))
                    .child(label)
                    .when_some(by.and_then(sort_mark), |d, glyph| d.child(ui::icon_sized(glyph, IconSize::Small, c.text_muted, cx)))
            };
        let mut header_row = div()
            .flex()
            .flex_none()
            .min_w_full()
            .bg(c.header_bg)
            .border_b_1()
            .border_color(c.card_border)
            .text_color(c.text_muted)
            .font_weight(FontWeight::SEMIBOLD)
            .child(div().flex().flex_none().items_center().justify_center().w(px(40.0)).child(
                ui::check_box("check-all", all_checked, cx).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        let names: Vec<String> =
                            this.visible(&this.store.read(cx).projects).into_iter().map(|p| p.name).collect();
                        if all_checked {
                            this.checked.clear();
                        } else {
                            this.checked.extend(names);
                        }
                        cx.notify();
                    },
                )),
            ))
            .child(div().flex_none().w(px(32.0)))
            .child(
                header_cell("h-state".into(), "", Some(24.0), Some(SortBy::State), false, 0.0)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.sort_by(SortBy::State, cx))),
            )
            .child(
                header_cell("h-name".into(), "Name", Some(widths.name), Some(SortBy::Name), false, 8.0)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.sort_by(SortBy::Name, cx))),
            );
        let content_width = widths.middle(&columns);
        let max_offset = (content_width - self.view_width).max(0.0);
        self.h_offset = self.h_offset.clamp(0.0, max_offset);
        let offset = self.h_offset;
        // Scrolled sideways by moving it left, as the rows' middles.
        let mut header_middle = div().flex().flex_none().relative().left(px(-offset));
        for column in &columns {
            let column = *column;
            header_middle = header_middle.child(
                header_cell(
                    format!("h-{}", column.key()).into(),
                    column.header(),
                    Some(widths.of(column)),
                    Some(SortBy::Column(column)),
                    column.numeric(),
                    8.0,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.sort_by(SortBy::Column(column), cx))),
            );
        }
        // The middle's visible width, measured here, decides how far it can scroll.
        let page = cx.entity().downgrade();
        let measure = gpui::canvas(
            move |bounds, _, cx| {
                let width = bounds.size.width / px(1.0);
                let changed = page
                    .update(cx, |this, _| {
                        let changed = (this.view_width - width).abs() > 0.5;
                        this.view_width = width;
                        changed
                    })
                    .unwrap_or(false);
                // Drawn again with it once this frame is done - a notify while drawing is dropped.
                if changed {
                    let page = page.clone();
                    cx.defer(move |cx| {
                        page.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        header_row = header_row
            .child(div().relative().flex_1().min_w_0().overflow_hidden().child(measure).child(header_middle))
            .child(
                div()
                    .flex_none()
                    .w(px(ACTIONS_WIDTH))
                    .border_l_1()
                    .border_color(c.card_border)
                    .pad(px(6.0), px(11.0))
                    .child("Actions"),
            );

        let focus_ring = ui::focus::keyboard_focus(cx) && self.table_focus.is_focused(window);
        let layout = TableLayout { columns: &columns, widths: &widths, offset, focus_ring };
        let mut body = div().flex().flex_col().min_w_full();
        for p in &rows {
            body = body.child(self.render_row(p, &layout, pending.get(&p.name).cloned(), busy, cx));
        }

        let table = div()
            .relative()
            .children(self.tour_targets.as_ref().map(|t| dnnm_dialogs::tour::target(dnnm_dialogs::tour::TourPart::ProjectsTable, t)))
            .id("projects-table")
            .key_context("ProjectsTable")
            .track_focus(&self.table_focus)
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &TickRow, _, cx| {
                if let Some(name) = this.selected.clone() {
                    if !this.checked.remove(&name) {
                        this.checked.insert(name);
                    }
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &TickAll, _, cx| {
                let names: Vec<String> = this.visible(&this.store.read(cx).projects).into_iter().map(|p| p.name).collect();
                this.checked.extend(names);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExpandRow, _, cx| {
                if let Some(name) = this.selected.clone() {
                    this.expanded.insert(name);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &CollapseRow, _, cx| {
                if let Some(name) = this.selected.clone() {
                    this.expanded.remove(&name);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &OpenRow, window, cx| {
                if let Some(name) = this.selected.clone() {
                    this.open_details(&name, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StartRow, _, cx| {
                if let Some(p) = this.selected_row(cx)
                    && matches!(view::run_state(&p.site), SiteRunState::Stopped | SiteRunState::Unknown)
                {
                    this.control(vec![p.name], SiteAction::Start, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StopRow, _, cx| {
                if let Some(p) = this.selected_row(cx)
                    && view::run_state(&p.site) == SiteRunState::Running
                {
                    this.control(vec![p.name], SiteAction::Stop, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RestartRow, _, cx| {
                if let Some(p) = this.selected_row(cx)
                    && view::run_state(&p.site) == SiteRunState::Running
                {
                    this.control(vec![p.name], SiteAction::Restart, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RemoveRow, _, cx| {
                let idle = !this.store.read(cx).is_busy();
                if let Some(name) = this.selected.clone().filter(|_| idle) {
                    this.store.update(cx, |s, cx| s.remove_projects(vec![name], cx));
                }
            }))
            .on_action(cx.listener(|this, _: &RowMenu, window, cx| {
                if let Some(name) = this.selected.clone() {
                    // Shift+F10: the menu where the pointer is, as the keyboard has no place of its own here.
                    let position = window.mouse_position();
                    this.open_menu(&name, position, cx);
                }
            }))
            .relative()
            .flex_1()
            .min_h_0()
            .rounded(theme.metrics.card_radius)
            .border_1()
            .border_color(c.card_border)
            .bg(c.card_bg)
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, window, cx| {
                let delta = e.delta.pixel_delta(window.line_height());
                let sideways = if e.modifiers.shift { delta.y } else { delta.x } / px(1.0);
                if sideways != 0.0 && max_offset > 0.0 {
                    this.h_offset = (this.h_offset - sideways).clamp(0.0, max_offset);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(header_row)
                    .child(div().id("table-y").flex_1().min_h_0().overflow_y_scroll().child(body))
                    .when(max_offset > 0.0, |d| {
                        d.child(self.render_h_scrollbar(widths.frozen(), content_width, max_offset, cx))
                    }),
            )
            .when_some(overlay, |d, text| {
                d.child(
                    div().absolute().inset_0().flex().items_center().justify_center().child(
                        div()
                            .max_w(px(520.0))
                            .pad(px(10.0), px(16.0))
                            .rounded(px(6.0))
                            .bg(c.card_bg)
                            .border_1()
                            .border_color(c.card_border)
                            .text_size(theme.text.medium)
                            .text_color(c.text_primary)
                            .child(text),
                    ),
                )
            });

        // ─── Footer: "4 of 12 projects", "Selected 2 of 12" ───
        let shown = rows.len();
        let count_text = if shown == all_count {
            format!("{all_count} project{}", if all_count == 1 { "" } else { "s" })
        } else {
            format!("{shown} of {all_count} projects")
        };
        let footer = div()
            .flex()
            .justify_between()
            .pad(px(0.0), px(2.0))
            .child(div().text_color(c.text_muted).child(count_text))
            .when(!checked_rows.is_empty(), |d| {
                d.child(div().text_color(c.text_primary).child(format!("Selected {} of {shown}", checked_rows.len())))
            });

        div()
            .id("projects-page")
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                let Some((start_x, start_offset)) = this.thumb_drag else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    this.thumb_drag = None;
                    cx.notify();
                    return;
                }
                let content = this.drag_content_width;
                if this.view_width > 0.0 {
                    let moved = (e.position.x / px(1.0) - start_x) * content / this.view_width;
                    this.h_offset = (start_offset + moved).clamp(0.0, (content - this.view_width).max(0.0));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // The thumb's colour follows the drag: drawn again as it ends.
                    if this.thumb_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pad(px(12.0), px(28.0))
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            // Header 16 px over the toolbar (its own 12 + 4), toolbar 10 over the table, table 8 over the footer.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(10.0))
                    .child(div().flex().flex_col().gap(px(4.0)).child(header).child(toolbar))
                    .child(table),
            )
            .child(footer)
            .when(self.columns_open, |d| d.child(self.render_columns_menu(&columns, window, cx)))
            .children(self.render_row_menu(window, cx))
    }
}

impl ProjectsPage {
    fn render_row(
        &self,
        p: &ProjectState,
        layout: &TableLayout,
        pending: Option<SharedString>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let TableLayout { columns, widths, offset, focus_ring } = *layout;
        let name = p.name.clone();
        let checked = self.checked.contains(&name);
        let expanded = self.expanded.contains(&name);
        let state = view::run_state(&p.site);
        let idle = pending.is_none() && !busy;
        let changing = pending.is_some() || matches!(state, SiteRunState::Starting | SiteRunState::Stopping);
        let state_text: SharedString = pending.clone().unwrap_or_else(|| view::state_text(&p.site).into());
        let (dot_color, filled) = if changing {
            (c.log_warn, true)
        } else {
            match state {
                SiteRunState::Running => (c.success_text, true),
                SiteRunState::Unknown => (c.text_muted, false),
                _ => (c.text_muted, false),
            }
        };
        let show_start = pending.is_none()
            && matches!(state, SiteRunState::Stopped | SiteRunState::Stopping | SiteRunState::Unknown);
        let show_stop = pending.is_none() && matches!(state, SiteRunState::Running | SiteRunState::Starting);
        let can_start = idle && matches!(state, SiteRunState::Stopped | SiteRunState::Unknown);
        let can_stop = idle && state == SiteRunState::Running;

        let id = |what: &str| SharedString::from(format!("{what}-{name}"));
        let n = name.clone();
        let check = ui::check_box(id("check"), checked, cx).tab_stop(false).on_click(cx.listener(
            move |this, _: &ClickEvent, _, cx| {
                if !this.checked.remove(&n) {
                    this.checked.insert(n.clone());
                }
                cx.notify();
            },
        ));
        let n = name.clone();
        let expand = div()
            .id(id("expand"))
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.0))
            .rounded(theme.metrics.control_radius)
            .cursor_pointer()
            .hover(move |s| s.bg(c.hover_bg))
            .tip("Show a few facts under the row")
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.expanded.remove(&n) {
                    this.expanded.insert(n.clone());
                }
                cx.notify();
            }))
            .child(ui::icon_sized(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, IconSize::Small, c.link_fg, cx));

        let mut line = div()
            .flex()
            .flex_none()
            .min_w_full()
            .h(theme.metrics.table_row_height)
            .items_center()
            .child(div().flex().flex_none().justify_center().w(px(40.0)).child(check))
            .child(div().flex().flex_none().justify_center().w(px(32.0)).child(expand))
            .child(
                div()
                    .id(id("dot"))
                    .flex()
                    .flex_none()
                    .justify_center()
                    .w(px(24.0))
                    .tip(state_text.clone())
                    .child(ui::status_dot(dot_color, filled, px(9.0))),
            )
            .child(
                div().flex_none().w(px(widths.name)).pad(px(0.0), px(8.0)).child({
                    let open = name.clone();
                    div()
                        .id(id("name"))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.link_fg)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .tip("Show everything about this website")
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_details(&open, window, cx)))
                        .child(name.clone())
                }),
            );

        let mut middle = div().flex().flex_none().h_full().items_center().relative().left(px(-offset));
        for column in columns {
            let text = column.text(p);
            let cell = div()
                .flex()
                .flex_none()
                .items_center()
                .w(px(widths.of(*column)))
                .pad(px(0.0), px(8.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .when(column.numeric(), |d| d.justify_end());
            let cell = match column {
                Column::Url if !p.site_url.is_empty() => {
                    let url = p.site_url.clone();
                    cell.child(
                        ui::link(id("url"), text, cx)
                            .tab_stop(false)
                            .tip("Open the website in your browser")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_url(&url, cx))),
                    )
                }
                Column::Url => cell.text_color(c.text_muted).child(text),
                Column::Path => {
                    let dir = p.directory.clone();
                    cell.child(
                        ui::link(id("path"), text, cx)
                            .tab_stop(false)
                            .tip("Open the website's files in File Explorer")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_folder(&dir, cx))),
                    )
                }
                Column::Sql => {
                    let color = match text.as_str() {
                        "Live" => c.success_text,
                        "Offline" => c.error_text,
                        _ => c.text_primary,
                    };
                    let tip = view::sql_tip(p).unwrap_or_default();
                    cell.child(div().id(id("sql")).text_color(color).tip(tip).child(text))
                }
                Column::Database => {
                    let color = if p.database_problem.is_some() { c.log_warn } else { c.text_primary };
                    let tip = view::database_tip(p).unwrap_or_default();
                    cell.child(div().id(id("db")).text_color(color).tip(tip).child(text))
                }
                Column::Status => cell.text_color(c.text_muted).child(state_text.clone()),
                Column::Id => cell.text_color(c.text_muted).child(text),
                _ => cell.child(text),
            };
            middle = middle.child(cell);
        }

        // Actions: Start or Stop, Restart; a thin bar while the row's operation runs; Remove after a divider.
        let n1 = name.clone();
        let n2 = name.clone();
        let n3 = name.clone();
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .w(px(ACTIONS_WIDTH))
            .h_full()
            .pad(px(0.0), px(9.0))
            .border_l_1()
            .border_color(c.card_border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(px(56.0))
                    .when(pending.is_some(), |d| {
                        d.pad(px(0.0), px(4.0)).child(div().h(px(3.0)).w_full().rounded(px(2.0)).bg(c.accent).opacity(0.8))
                    })
                    .when(show_start, |d| {
                        d.child(
                            ui::icon_button(id("start"), Icon::Play, c.link_fg, can_start, cx)
                                .tab_stop(false)
                                .tip("Start - turn the website on so it can be opened")
                                .when(can_start, |b| {
                                    b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.control(vec![n1.clone()], SiteAction::Start, cx)
                                    }))
                                }),
                        )
                    })
                    .when(show_stop, |d| {
                        d.child(
                            ui::icon_button(id("stop"), Icon::Stop, c.link_fg, can_stop, cx)
                                .tab_stop(false)
                                .tip(
                                    "Stop - turn the website off until you start it again; nothing is deleted",
                                )
                                .when(can_stop, |b| {
                                    b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.control(vec![n2.clone()], SiteAction::Stop, cx)
                                    }))
                                }),
                        )
                        .child(
                            ui::icon_button(id("restart"), Icon::Refresh, c.link_fg, can_stop, cx)
                                .tab_stop(false)
                                .tip(
                                    "Restart the website's background process - can fix problems; it is unavailable for a few seconds",
                                )
                                .when(can_stop, |b| {
                                    b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.control(vec![n3.clone()], SiteAction::Restart, cx)
                                    }))
                                }),
                        )
                    }),
            )
            // Keep warm, then Remove 8 px either side of a divider.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child({
                        let status = self.store.read(cx).keep_warm_status(&name);
                        let on = status.is_on();
                        let has_url = !p.site_url.is_empty();
                        let can = dnnm_keepwarm::view::can_toggle(on, has_url);
                        let tip = dnnm_keepwarm::view::unavailable(on, has_url)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("{}\n{}", dnnm_keepwarm::view::tip(on), status.text));
                        let site = name.clone();
                        div()
                            .id(id("keepwarm"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.0))
                            .rounded(theme.metrics.control_radius)
                            .when(can, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                            .when(!can, |d| d.opacity(0.35))
                            .tip(tip)
                            .when(can, |d| {
                                d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let site = site.clone();
                                    this.store.update(cx, |s, cx| s.set_keep_warm(&[site], !on, cx));
                                }))
                            })
                            .child(flame_icon(id("flame"), dnnm_keepwarm::view::look(&status), c.text_muted, cx))
                    })
                    .child(div().w(px(1.0)).h(px(18.0)).bg(c.card_border))
                    .child({
                        let site = name.clone();
                        ui::icon_button(id("remove"), Icon::Delete, c.error_text, idle, cx)
                            .tab_stop(false)
                            .tip(dnnm_wiring::remove_project::REMOVE_ROW_TOOLTIP)
                            .when(idle, |b| {
                                b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let site = site.clone();
                                    this.store.update(cx, |s, cx| s.remove_projects(vec![site], cx));
                                }))
                            })
                    }),
            );
        line = line.child(div().flex_1().min_w_0().h_full().overflow_hidden().child(middle)).child(actions);

        let selected = self.selected.as_deref() == Some(name.as_str());
        let row_bg = if checked || selected { Some(c.selection_bg) } else { None };
        let (pick, menu_for) = (name.clone(), name.clone());
        div()
            .id(id("row"))
            .relative()
            .flex()
            .flex_col()
            .min_w_full()
            .border_b_1()
            .border_color(c.grid_line)
            .when_some(row_bg, |d, bg| d.bg(bg))
            // The keyboard's place in the table: a bar of the focus colour down the selected row's left edge.
            .when(selected && focus_ring, |d| {
                d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.0)).bg(c.focus_border))
            })
            .when(!checked && !selected, |d| d.hover(move |s| s.bg(c.hover_bg)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.select(&pick, window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                    this.select(&menu_for, window, cx);
                    this.open_menu(&menu_for, e.position, cx);
                }),
            )
            .child(line)
            .when(expanded, |d| d.child(self.render_details(p, cx)))
    }

    /// The sideways scrollbar under the middle columns: the thumb shows the visible share, drags and jumps.
    fn render_h_scrollbar(
        &self,
        frozen: f32,
        content: f32,
        max_offset: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = Theme::get(cx).colors;
        let view = self.view_width.max(1.0);
        let thumb_width = (view * view / content).max(24.0);
        let thumb_left = if max_offset > 0.0 { self.h_offset / max_offset * (view - thumb_width) } else { 0.0 };
        let dragging = self.thumb_drag.is_some();
        let page = cx.entity().downgrade();
        let track_measure = gpui::canvas(
            move |bounds, _, cx| {
                let x = bounds.origin.x / px(1.0);
                let _ = page.update(cx, |this, _| this.track_x = x);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .flex()
            .flex_none()
            .h(px(10.0))
            .child(div().flex_none().w(px(frozen)))
            .child(
                div()
                    .id("h-track")
                    .relative()
                    .flex_1()
                    .h_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                            // A click beside the thumb moves a page that way.
                            let page = this.view_width * 0.9;
                            let x = e.position.x / px(1.0) - this.track_x;
                            this.h_offset = if x < thumb_left {
                                (this.h_offset - page).max(0.0)
                            } else {
                                (this.h_offset + page).min(max_offset)
                            };
                            cx.notify();
                        }),
                    )
                    .child(track_measure)
                    .child(
                        div()
                            .id("h-thumb")
                            .absolute()
                            .top(px(2.0))
                            .h(px(6.0))
                            .left(px(thumb_left))
                            .w(px(thumb_width))
                            .rounded(px(3.0))
                            .bg(if dragging { c.scroll_thumb_hover } else { c.scroll_thumb })
                            .hover(move |s| s.bg(c.scroll_thumb_hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                                    this.thumb_drag = Some((e.position.x / px(1.0), this.h_offset));
                                    this.drag_content_width = content;
                                    cx.stop_propagation();
                                }),
                            ),
                    ),
            )
            .child(div().flex_none().w(px(ACTIONS_WIDTH)))
    }

    /// The expanded row's facts (`ProjectRow.Details`), 250 px each, wrapping.
    /// The row's menu while open, over a backdrop that closes it - over the table or a project's details.
    fn render_row_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.menu.as_ref()?;
        let close = cx.entity().downgrade();
        let hover = cx.entity().downgrade();
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("menu-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        })),
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

    fn render_details(&self, p: &ProjectState, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let c = theme.colors;
        let pids = view::pids(&p.site);
        let bindings: Vec<String> = p
            .site
            .bindings
            .iter()
            .map(|b| {
                format!("{} {}:{}:{}", b.protocol, b.address, b.port.map(|p| p.to_string()).unwrap_or_default(), b.host)
            })
            .collect();
        let database = match &p.database_name {
            None => NONE.to_string(),
            Some(_) => format!("{} - {}", view::database(p), view::sql_tip(p).unwrap_or_default()),
        };
        let facts = [
            ("Site", view::url(p)),
            ("Physical path", p.directory.display().to_string()),
            ("App pool", view::app_pool(&p.site)),
            ("Bindings", if bindings.is_empty() { NONE.into() } else { bindings.join("\n") }),
            ("Site ID", p.site.id.to_string()),
            ("Database", database),
            ("DNN version", view::dnn(p)),
            (
                "Worker process",
                if pids == NONE {
                    "none - started on the first request".into()
                } else {
                    format!("PID {pids}, started {}", view::last_started(p))
                },
            ),
            (dnnm_keepwarm::view::DETAILS_LABEL, self.store.read(cx).keep_warm_status(&p.name).text.to_string()),
        ];
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(16.0))
            .gap_y(px(8.0))
            .pad(px(12.0), px(20.0))
            .border_t_1()
            .border_color(c.grid_line)
            .children(facts.into_iter().map(|(key, value)| {
                div()
                    .w(px(250.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().w_full().text_size(theme.text.hint).text_color(c.text_muted).child(key))
                    .child(div().w_full().text_color(c.text_primary).overflow_hidden().text_ellipsis().child(value))
            }))
    }

    /// The Columns popup: a switch per column, then Hide all / Default / Show all.
    fn render_columns_menu(&self, shown: &[Column], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        // The switches 8 px apart, 4 px in from the top and bottom of the list.
        let mut list = div().flex().flex_col().gap(px(8.0)).pad(px(4.0), px(0.0));
        for column in Column::ALL {
            let on = shown.contains(&column);
            let current = shown.to_vec();
            list = list.child(div().child(ui::switch(
                SharedString::from(format!("col-{}", column.key())),
                on,
                column.menu_name(),
                cx.listener(move |this, on: &bool, _, cx| {
                    let mut next = current.clone();
                    if *on {
                        next.push(column);
                    } else {
                        next.retain(|c| *c != column);
                    }
                    this.set_columns(next, cx);
                }),
                window,
                cx,
            )));
        }
        // Under the Columns button, its left edge level with the button's, over everything (the panel too); a press
        // anywhere else closes it - on the button itself its own click does.
        let (position, corner) = self.columns_anchor.below(false);
        let anchor = self.columns_anchor.clone();
        // It holds the keyboard while open, as a menu does; Esc closes it (the workspace's "Modal").
        let focus = window.use_keyed_state("columns-menu-focus", cx, |_, cx| cx.focus_handle()).read(cx).clone();
        gpui::deferred(
            gpui::anchored().anchor(corner).position(position).snap_to_window().child(
                ui::focus::trap(div().id("columns-menu"), &focus)
                    .on_mouse_down_out(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        if !anchor.contains(e.position) {
                            this.columns_open = false;
                            cx.notify();
                        }
                    }))
                    .min_w(px(220.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .pad(px(10.0), px(14.0))
                    .rounded(px(6.0))
                    .bg(c.card_bg)
                    .border_1()
                    .border_color(c.card_border)
                    .shadow_lg()
                    .occlude()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(list)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                ui::link("cols-hide", "Hide all", cx).on_click(
                                    cx.listener(|this, _: &ClickEvent, _, cx| this.set_columns(Vec::new(), cx)),
                                ),
                            )
                            .child(ui::link("cols-default", "Default", cx).on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| {
                                    let defaults = dnnm_application::settings::DEFAULT_PROJECT_COLUMNS
                                        .iter()
                                        .filter_map(|k| Column::from_key(k))
                                        .collect();
                                    this.set_columns(defaults, cx)
                                },
                            )))
                            .child(ui::link("cols-show", "Show all", cx).on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| this.set_columns(Column::ALL.to_vec(), cx)),
                            )),
                    ),
            ),
        )
        .with_priority(2)
    }
}
