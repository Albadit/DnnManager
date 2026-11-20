//! A project's Details (`Pages/Projects/ProjectView.xaml`): over the Projects table, its name, state and address,
//! Start / Stop / Restart / Open site / Open folder, and the tabs General, IIS, DNN, Database and Advanced - each a
//! list of sections (`InspectorPanel`) from `dnnm_diagnostics`, read off the UI thread.

use dnnm_ui::px;
use dnnm_ui::Icon;
use gpui::{ClickEvent, Context, Entity, EventEmitter, FontWeight, Hsla, SharedString, Task, Window, div, prelude::*};

use dnnm_application::project::{SiteRunState, view};
use dnnm_application::use_cases::SiteAction;
use dnnm_diagnostics::{Health, ProjectSnapshot, Section, TabKind};

use crate::store::{Store, ToastKind};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tip, icons};

/// The view asks to go back to the table (← or Esc).
pub struct ProjectViewClosed;

/// A tab's edit button: the action and the project.
pub struct ProjectViewAction(pub crate::workspace::ProjectAction, pub String);

impl EventEmitter<ProjectViewAction> for ProjectView {}

/// "More actions": the project's menu, at this point and with this corner of it there - under the ⋮ button, their right
/// edges level; the same menu as right-clicking its row.
pub struct ProjectViewMenu(pub gpui::Point<gpui::Pixels>, pub gpui::Corner);

impl EventEmitter<ProjectViewMenu> for ProjectView {}


pub struct ProjectView {
    store: Entity<Store>,
    pub name: String,
    tab: TabKind,
    snapshot: Option<ProjectSnapshot>,
    issues: Vec<dnnm_diagnostics::DetectedIssue>,
    /// Where the ⋮ button is: its menu opens under it.
    more_anchor: ui::Anchor,
    /// The page's keyboard: it takes it as it opens, so Esc goes back and Tab starts at its buttons.
    pub focus: gpui::FocusHandle,
    _read: Option<Task<()>>,
}

impl EventEmitter<ProjectViewClosed> for ProjectView {}

impl ProjectView {
    pub fn new(store: Entity<Store>, name: String, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store,
            name,
            tab: TabKind::General,
            snapshot: None,
            issues: Vec::new(),
            more_anchor: ui::Anchor::default(),
            focus: cx.focus_handle(),
            _read: None,
        };
        view.read(cx);
        view
    }

    pub fn set_tab(&mut self, tab: TabKind, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    pub fn tab(&self) -> TabKind {
        self.tab
    }

    /// Reads the site's facts again - off the UI thread (IIS, web.config, folders, bin).
    pub fn read(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let Some(project) = store.projects.iter().find(|p| p.name == self.name).cloned() else { return };
        let iis = store.iis().clone();
        let backups = dnnm_infrastructure::paths::app_data_directory().join("backups").join(&project.name);
        self._read = Some(cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move { dnnm_diagnostics::snapshot::read_local(&project, &*iis, Some(&backups), None) })
                .await;
            let needs_database = snapshot.needs_database();
            let directory = snapshot.project.directory.clone();
            let _ = this.update(cx, |v, cx| {
                v.issues = dnnm_diagnostics::tabs::issues(&snapshot);
                v.snapshot = Some(snapshot);
                cx.notify();
            });
            // Then the database, with the site's own connection - its tabs show "Reading…" until it answers.
            if needs_database {
                let inspection = cx
                    .background_executor()
                    .spawn(async move {
                        dnnm_infrastructure::web_config::read_database_connection(&directory.join("web.config"))
                            .ok()
                            .map(|c| dnnm_wiring::database_inspector::inspect_database(&c))
                    })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    // Taken only with an inspection to add - else the snapshot stays as read.
                    if let Some(inspection) = inspection
                        && let Some(snapshot) = v.snapshot.take()
                    {
                        let snapshot = snapshot.with_database(inspection);
                        v.issues = dnnm_diagnostics::tabs::issues(&snapshot);
                        v.snapshot = Some(snapshot);
                    }
                    cx.notify();
                });
            }
        }));
    }

    fn sections(&mut self, cx: &mut Context<Self>) -> Vec<Section> {
        if self.snapshot.is_none() && self._read.is_none() {
            self.read(cx);
        }
        match &self.snapshot {
            None => dnnm_diagnostics::model::reading(),
            Some(s) => dnnm_diagnostics::tabs::tab(s, self.tab, &self.issues),
        }
    }

    fn health_color(health: Health, c: &dnnm_ui::theme::Colors) -> Option<Hsla> {
        match health {
            Health::None => None,
            Health::Ok => Some(c.success_text),
            Health::Warning => Some(c.log_warn),
            Health::Bad => Some(c.error_text),
            Health::Unknown => Some(c.text_muted),
        }
    }

    fn render_section(&self, index: usize, section: &Section, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        // Title, then the rows (3 px above and below each), then the table, then the note. With no rows the table sits
        // 14 px under the title.
        // Where it was read from: an info icon beside the title.
        let title = div().text_size(theme.text.heading).font_weight(FontWeight::SEMIBOLD).child(section.title.clone());
        let header = match section.source.clone() {
            Some(source) => ui::with_tip(title, SharedString::from(format!("source-{index}")), source, cx),
            None => div().flex().child(title),
        };
        let mut rows = div().flex().flex_col().gap(px(6.0)).pad(px(3.0), px(0.0));
        for (r, row) in section.rows.iter().enumerate() {
            let color = Self::health_color(row.health, &c);
            let value: gpui::AnyElement = match row.link() {
                Some(url) => {
                    let url = url.to_string();
                    ui::link(SharedString::from(format!("row-{index}-{r}")), row.value.clone(), cx)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Err(e) = dnnm_infrastructure::shell::open_url(&url) {
                                this.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                            }
                        }))
                        .into_any_element()
                }
                None => div()
                    .when_some(color.filter(|_| row.health != Health::Ok), |d, col| d.text_color(col))
                    .when(row.health == Health::Bad, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(row.value.clone())
                    .into_any_element(),
            };
            // A detail that explains goes in an info icon beside the label; one that says what is wrong stays in view.
            let problem = matches!(row.health, Health::Warning | Health::Bad);
            let tip = row.detail.clone().filter(|_| !problem);
            let label = div().text_color(c.text_muted).child(row.label.clone());
            rows = rows.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(div().flex_none().w(px(198.0)).child(match tip {
                        Some(tip) => ui::with_tip(label, SharedString::from(format!("detail-{index}-{r}")), tip, cx),
                        None => label,
                    }))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .flex_1()
                            .min_w_0()
                            .child(div().flex_none().w(px(16.0)).pad(px(5.0), px(0.0)).when_some(color, |d, col| {
                                d.child(div().size(px(7.0)).rounded_full().bg(col))
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(value)
                                    .when_some(row.detail.clone().filter(|_| problem), |d, detail| {
                                        d.child(div().text_size(theme.text.hint).text_color(c.text_muted).child(detail))
                                    }),
                            ),
                    ),
            );
        }
        let mut body = div().flex().flex_col().gap(px(6.0));
        let mut has_body = false;
        if !section.rows.is_empty() {
            body = body.child(rows);
            has_body = true;
        }
        if let Some(table) = &section.table {
            let with_health = table.rows.iter().any(|r| r.health != Health::None);
            let columns = table.columns.len().max(1);
            // Each column as wide as its longest text (a DataGrid's auto columns), within reason; the last takes the
            // rest. A cell's lines count apart (Other aliases lists one per line).
            let widths: Vec<gpui::Pixels> = (0..columns)
                .map(|i| {
                    let longest = std::iter::once(table.columns.get(i).map(|h| h.chars().count()).unwrap_or(0))
                        .chain(table.rows.iter().filter_map(|r| r.cells.get(i)).flat_map(|t| t.lines().map(|l| l.chars().count())))
                        .max()
                        .unwrap_or(0);
                    px((longest as f32 * 7.2 + 24.0).clamp(56.0, 420.0))
                })
                .collect();
            // The columns are 8 px apart: each width less the gap, so the header and the lines line up.
            let mut grid = div().flex().flex_col();
            let mut head = div().flex().pad(px(2.0), px(0.0)).border_b_1().border_color(c.card_border);
            if with_health {
                head = head.child(div().flex_none().w(px(16.0)));
            }
            let mut head_cells = div().flex().flex_1().min_w_0().gap(px(8.0));
            for (i, col) in table.columns.iter().enumerate() {
                let cell = div().text_size(theme.text.small).text_color(c.text_muted).child(col.clone());
                head_cells = head_cells.child(if i + 1 == columns {
                    cell.flex_1().min_w(px(56.0))
                } else {
                    // As wide as its text, narrower when the card has no room for all of them.
                    cell.flex_shrink().min_w(px(32.0)).w(widths[i] - px(8.0)).overflow_hidden().whitespace_nowrap()
                });
            }
            grid = grid.child(head.child(head_cells));
            for (r, row) in table.rows.iter().enumerate() {
                let mut line = div().flex().items_start().pad(px(3.0), px(0.0));
                if with_health {
                    let color = Self::health_color(row.health, &c);
                    line = line.child(div().flex_none().w(px(16.0)).pad(px(6.0), px(0.0)).when_some(color, |d, col| {
                        d.child(div().size(px(7.0)).rounded_full().bg(col))
                    }));
                }
                let mut cells = div().flex().flex_1().min_w_0().gap(px(8.0));
                for (i, text) in row.cells.iter().enumerate() {
                    let last = i + 1 == row.cells.len();
                    let cell = div()
                        .id(SharedString::from(format!("cell-{index}-{r}-{i}")))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(text.clone())
                        .when(last, |d| d.flex_1().min_w(px(56.0)))
                        .when(!last, |d| d.flex_shrink().min_w(px(32.0)).w(widths.get(i).copied().unwrap_or(px(180.0)) - px(8.0)))
                        .when_some(row.detail.clone().filter(|_| last), |d, tip| d.tip(tip));
                    cells = cells.child(cell);
                }
                grid = grid.child(line.child(cells));
            }
            body = body.child(grid);
            has_body = true;
        }
        // Title to rows 8 px (+3 above the first row); title to a table alone 14 px.
        let head_gap = if section.rows.is_empty() { px(14.0) } else { px(8.0) };
        let top = div().flex().flex_col().gap(head_gap).child(header).when(has_body, |d| d.child(body));
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pad(px(12.0), px(16.0))
            .rounded(theme.metrics.card_radius)
            .border_1()
            .border_color(c.card_border)
            .bg(c.card_bg)
            .child(top)
            .when_some(section.note.clone(), |d, note| {
                d.child(div().text_size(theme.text.hint).text_color(c.text_muted).child(note))
            })
    }
}

impl Render for ProjectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let store = self.store.read(cx);
        let busy = store.is_busy();
        let project = store.projects.iter().find(|p| p.name == self.name).cloned();
        let pending = store.pending.get(&self.name).cloned();
        let Some(p) = project else {
            // The site went (removed in IIS): back to the table - once IIS has been read, not while it is being read.
            if store.connection == crate::store::Connection::Live {
                cx.emit(ProjectViewClosed);
            }
            return div();
        };
        let state = view::run_state(&p.site);
        let idle = !busy && pending.is_none();
        let state_text: SharedString = pending.unwrap_or_else(|| view::state_text(&p.site).into());
        let header_button = |id: &'static str, glyph: Icon, label: &'static str, tip: &'static str, on: bool| {
            ui::button(id, "", ui::ButtonKind::Secondary, on, cx)
                .gap(px(icons::LABEL_GAP))
                .tip(tip)
                .child(ui::icon(glyph, c.text_primary, cx))
                .child(label)
        };
        let name = self.name.clone();
        let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
        let url = p.site_url.clone();
        let dir = p.directory.clone();
        // 8 px above the header (20 from the top with the page's 12) and 8 below it (18 to the tabs).
        let header = div()
            .flex()
            .items_start()
            .gap(px(6.0))
            .pad(px(8.0), px(0.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .flex_1()
                    .min_w_0()
                    .gap(px(10.0))
                    .child(
                        div().flex_none().pad(px(2.0), px(0.0)).child(
                            ui::icon_button("details-back", Icon::Back, c.text_primary, true, cx)
                                .size(px(32.0))
                                .tip("Back (Esc)")
                                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(ProjectViewClosed))),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().text_size(theme.text.display).font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .text_color(c.text_muted)
                                    .child(state_text)
                                    .when(!url.is_empty(), |d| {
                                        let url = url.clone();
                                        d.child(ui::link("details-url", url.clone(), cx).tip("Open site").on_click(
                                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                if let Err(e) = dnnm_infrastructure::shell::open_url(&url) {
                                                    this.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                                                }
                                            }),
                                        ))
                                    }),
                            ),
                    ),
            )
            // The buttons 4 px lower than the title, 6 px apart.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pad(px(4.0), px(0.0))
                    .when(matches!(state, SiteRunState::Stopped | SiteRunState::Unknown), |d| {
                        d.child(
                            header_button("details-start", Icon::Play, "Start", "Start - turn the website on so it can be opened", idle)
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let n = n1.clone();
                                    this.store.update(cx, |s, cx| s.control_sites(vec![n], SiteAction::Start, cx));
                                })),
                        )
                    })
                    .when(state == SiteRunState::Running, |d| {
                        d.child(
                            header_button(
                                "details-stop",
                                Icon::Stop,
                                "Stop",
                                "Stop - turn the website off until you start it again; nothing is deleted",
                                idle,
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let n = n2.clone();
                                this.store.update(cx, |s, cx| s.control_sites(vec![n], SiteAction::Stop, cx));
                            })),
                        )
                        .child(
                            header_button(
                                "details-restart",
                                Icon::Refresh,
                                "Restart",
                                "Restart the website's background process - can fix problems; it is unavailable for a few seconds",
                                idle,
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let n = n3.clone();
                                this.store.update(cx, |s, cx| s.control_sites(vec![n], SiteAction::Restart, cx));
                            })),
                        )
                    })
                    .child(
                        header_button("details-site", Icon::Globe, "Open site", "Open the website in your browser", !url.is_empty())
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Err(e) = dnnm_infrastructure::shell::open_url(&url) {
                                    this.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                                }
                            })),
                    )
                    .child(
                        header_button("details-folder", Icon::Folder, "Open folder", "Open the website's files in File Explorer", true)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Err(e) = dnnm_infrastructure::shell::open_folder(&dir) {
                                    this.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                                }
                            })),
                    )
                    // Icon-only, but as high as the buttons beside it (`ui::button` is 30 px whatever it holds).
                    .child(
                        ui::button("details-more", "", ui::ButtonKind::Secondary, true, cx)
                            .tip("More actions: logs, clear cache, export, backups, upgrade")
                            .child(ui::icon(Icon::More, c.text_primary, cx))
                            .relative()
                            .child(self.more_anchor.measure())
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                let (position, corner) = this.more_anchor.below(true);
                                cx.emit(ProjectViewMenu(position, corner));
                            })),
                    ),
            );

        let mut tabs = div().flex().gap(px(22.0));
        for kind in TabKind::ALL {
            let chosen = kind == self.tab;
            tabs = tabs.child(
                ui::focusable_bare(div().id(SharedString::from(format!("details-tab-{}", kind.title()))), cx)
                    .pad(px(7.0), px(2.0))
                    // Its line only - not `focusable_bare`'s border all round, which the chosen tab's accent would
                    // paint on every side. Focused, the line takes the focus colour over the hover fill.
                    .border_0()
                    .border_b_2()
                    .cursor_pointer()
                    .border_color(if chosen { c.accent } else { gpui::transparent_black() })
                    .text_color(if chosen { c.text_primary } else { c.text_muted })
                    .when(chosen, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .hover(move |s| s.text_color(c.text_primary))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_tab(kind, cx)))
                    .child(kind.title()),
            );
        }

        // Each tab's edits (`ProjectView.xaml`): General Rename…; IIS Edit host names…, Edit app pool…; DNN Upgrade DNN…,
        // Change host password…; Database Change connection….
        use crate::workspace::ProjectAction as A;
        let edits: &[A] = match self.tab {
            TabKind::General => &[A::Rename],
            TabKind::Iis => &[A::EditBindings, A::EditAppPool],
            TabKind::Dnn if p.dnn_version.is_some() => &[A::UpgradeDnn, A::ChangeHostPassword],
            TabKind::Database => &[A::ChangeConnection],
            _ => &[],
        };
        let can_edit = dnnm_wiring::rename_project::can_edit(busy, &p.directory);
        let mut buttons = div().flex().items_center().gap(px(8.0));
        for action in edits.iter().copied() {
            let name = self.name.clone();
            buttons = buttons.child(
                ui::button(SharedString::from(format!("edit-{}", action.label())), action.label(), ui::ButtonKind::Secondary, can_edit, cx)
                    .tip(action.tooltip())
                    .when(can_edit, |b| {
                        b.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(ProjectViewAction(action, name.clone()))))
                    }),
            );
        }
        // The tabs on the left, the tab's edit buttons on the right of the same line - over the line under them.
        let tab_bar = div()
            .flex()
            .items_end()
            .justify_between()
            .gap(px(12.0))
            .border_b_1()
            .border_color(c.card_border)
            .child(tabs)
            .when(!edits.is_empty(), |d| d.child(div().pad(px(4.0), px(0.0)).child(buttons)));
        // The cards 14 px apart.
        let mut cards = div().flex().flex_col().gap(px(14.0));
        for (i, section) in self.sections(cx).iter().enumerate() {
            cards = cards.child(self.render_section(i, section, cx));
        }
        let content = cards;
        div()
            .key_context("Details")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .justify_center()
            .text_color(c.text_primary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(px(1100.0))
                    .gap(px(14.0))
                    .pad(px(12.0), px(28.0))
                    .child(div().flex().flex_col().gap(px(10.0)).child(header).child(tab_bar))
                    .child(div().id("details-content").flex_1().min_h_0().overflow_y_scroll().child(content)),
            )
    }
}
