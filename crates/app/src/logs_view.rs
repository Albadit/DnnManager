//! The bottom panel's Logs tab (`Controls/LogsView`): a website - DNN Manager's own first, then each IIS site - and
//! one of its logs (DNN, IIS, Windows event logs, the hosts file, DNN Manager's daily log); the newest lines, followed
//! as they come. What to read and how is `dnnm_logs`; this is the view.

use dnnm_ui::Icon;
use dnnm_ui::dialog::{SelectLook, SelectRow};
use dnnm_ui::px;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{ClickEvent, Context, Entity, FocusHandle, ListAlignment, ListState, MouseButton, SharedString, Task, Window, div, list, prelude::*};

use dnnm_logs::catalog::{self, SiteLogSource};
use dnnm_logs::tail::{LogTail, MAX_LINES};
use dnnm_logs::view::{self as logs, LogLineLevel, LogMenuItem};

use crate::store::{Store, ToastKind};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tip};

/// Which website's logs: DNN Manager's own, or an IIS site's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogSite {
    App,
    Site(String),
}

impl LogSite {
    fn label(&self) -> String {
        match self {
            LogSite::App => logs::APP_CHOICE.to_string(),
            LogSite::Site(name) => name.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Sites,
    Logs,
}

pub struct LogsView {
    store: Entity<Store>,
    site: Option<LogSite>,
    sources: Vec<SiteLogSource>,
    current: Option<usize>,
    lines: Vec<(SharedString, LogLineLevel)>,
    tail: Option<Arc<Mutex<Box<dyn LogTail>>>>,
    /// The shown log file's size, read with its lines off the UI thread (render doesn't touch the disk).
    file_size: Option<u64>,
    opened_at: Option<Instant>,
    /// Which picker's list is open.
    menu: Option<Open>,
    /// The lines as a list of rows of their own heights - a line wraps in the panel's width.
    scroll: ListState,
    /// The lines as selectable text, and the keyboard's place there (Copy, Select all).
    selection: ui::selection::LinesSelection,
    text_focus: FocusHandle,
    /// Polling only while the tab is on screen (`LogsView.Paused`).
    pub shown: bool,
    /// The panel's Find bar's matches, by line.
    find_marks: Arc<std::collections::HashMap<usize, ui::LineMarks>>,
    /// The right-click menu over the lines, while it is open.
    text_menu: Option<ui::menu::ContextMenu>,
    _poll: Option<Task<()>>,
}

impl LogsView {
    /// The lines shown - what the panel's Find bar searches.
    pub fn find_lines(&self) -> Vec<SharedString> {
        self.lines.iter().map(|(text, _)| text.clone()).collect()
    }

    /// The Find bar's matches to highlight (`current` the one it is on), and whether to scroll to that one.
    pub fn set_find(
        &mut self,
        matches: &[dnnm_logs::search::Match],
        current: Option<usize>,
        reveal: bool,
        cx: &mut Context<Self>,
    ) {
        let marks = crate::workspace::marks_by_line(matches, current);
        if *self.find_marks != marks {
            self.find_marks = Arc::new(marks);
            cx.notify();
        }
        if reveal && let Some(m) = current.and_then(|i| matches.get(i)) {
            self.scroll.scroll_to_reveal_item(m.line);
            cx.notify();
        }
    }

    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        Self {
            store,
            site: None,
            sources: Vec::new(),
            current: None,
            lines: Vec::new(),
            tail: None,
            file_size: None,
            opened_at: None,
            menu: None,
            scroll: ListState::new(0, ListAlignment::Top, px(400.0)),
            selection: ui::selection::LinesSelection::default(),
            text_focus: cx.focus_handle(),
            shown: false,
            find_marks: Arc::default(),
            text_menu: None,
            _poll: None,
        }
    }

    /// Whether it is on DNN Manager's own log.
    pub fn showing_app_log(&self) -> bool {
        matches!(self.site, Some(LogSite::App))
    }

    /// Shows a website's logs - its first log, or DNN Manager's newest log (`ShowSite`).
    pub fn show_site(&mut self, site: LogSite, cx: &mut Context<Self>) {
        self.menu = None;
        let store = self.store.read(cx);
        let sources = match &site {
            LogSite::App => catalog::for_app(&catalog::logs_directory(), &catalog::hosts_file()),
            LogSite::Site(name) => match store.projects.iter().find(|p| &p.name == name) {
                Some(p) => {
                    let iis_logs = store.iis().log_directory(name);
                    catalog::for_site(p.site.id, &p.directory, &p.site.app_pool, iis_logs.as_deref())
                }
                None => Vec::new(),
            },
        };
        self.site = Some(site);
        self.sources = sources;
        let pick = logs::choose(&self.sources, |_| false);
        self.open(pick, cx);
    }

    fn open(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.menu = None;
        self.current = index;
        self.lines.clear();
        self.scroll.reset(0);
        self.tail = None;
        self.file_size = None;
        self._poll = None;
        let Some(source) = index.and_then(|i| self.sources.get(i)).cloned() else {
            cx.notify();
            return;
        };
        let tail = Arc::new(Mutex::new(source.open()));
        self.tail = Some(tail.clone());
        let path = source.file_path.clone();
        let size_of = move || path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len());
        self.opened_at = Some(Instant::now());
        // The newest lines off the UI thread, then the new ones every second (two for the hosts file).
        self._poll = Some(cx.spawn(async move |this, cx| {
            let first = tail.clone();
            let size = size_of.clone();
            let update = cx
                .background_executor()
                .spawn(async move {
                    first.lock().map(|mut t| t.start(dnnm_logs::tail::INITIAL_LINES)).ok().map(|u| (u, size()))
                })
                .await;
            let Some((update, size)) = update else { return };
            let every = tail.lock().map(|t| t.poll_every()).unwrap_or(std::time::Duration::from_secs(1));
            if this.update(cx, |v, cx| v.add(update, size, cx)).is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(every).await;
                let Ok(shown) = this.update(cx, |v, _| v.shown) else { return };
                if !shown {
                    continue;
                }
                let next = tail.clone();
                let size = size_of.clone();
                let update = cx
                    .background_executor()
                    .spawn(async move { next.lock().map(|mut t| t.poll()).ok().map(|u| (u, size())) })
                    .await;
                let Some((update, size)) = update else { return };
                if this.update(cx, |v, cx| v.add(update, size, cx)).is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    fn add(&mut self, update: dnnm_logs::tail::TailUpdate, size: Option<u64>, cx: &mut Context<Self>) {
        self.file_size = size;
        if update.replaced {
            self.lines.clear();
            self.scroll.reset(0);
        }
        if update.lines.is_empty() && !update.replaced {
            // Still nothing: the hint shows after a moment, so a timer repaint is enough.
            cx.notify();
            return;
        }
        let before = self.lines.len();
        self.lines.extend(update.lines.iter().map(|l| {
            let (text, level) = logs::log_line(l);
            (SharedString::from(text), level)
        }));
        // The list measures only the new rows.
        self.scroll.splice(before..before, self.lines.len() - before);
        if self.lines.len() > MAX_LINES {
            let over = self.lines.len() - MAX_LINES;
            self.lines.drain(..over);
            self.scroll.splice(0..over, 0);
        }
        // Follows the end, as a log view does.
        self.scroll.scroll_to_reveal_item(self.lines.len().saturating_sub(1));
        cx.notify();
    }

    fn current_source(&self) -> Option<&SiteLogSource> {
        self.current.and_then(|i| self.sources.get(i))
    }

    fn folder(&self) -> Option<PathBuf> {
        let source = self.current_source()?;
        source.file_path.as_ref().and_then(|f| f.parent().map(PathBuf::from))
    }

    /// The pickers and the log's facts - the header row of the tab. The pickers are the app's select
    /// (`ui::dialog::select_rows`), in its compact size: the same keys, highlight and list as every other select.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        // Opened or shut by its box, the keys or a press outside; a picker shuts only its own list, so a press on the
        // other picker (outside this one) doesn't shut the list that press opened.
        let toggle = |which: Open| {
            cx.listener(move |this: &mut Self, open: &bool, _, cx| {
                if *open {
                    this.menu = Some(which);
                } else if this.menu == Some(which) {
                    this.menu = None;
                } else {
                    return;
                }
                cx.notify();
            })
        };
        let mut sites = vec![LogSite::App];
        sites.extend(self.store.read(cx).projects.iter().map(|p| LogSite::Site(p.name.clone())));
        let site_rows =
            sites.iter().map(|site| SelectRow::option(site.label(), self.site.as_ref() == Some(site))).collect();
        let site_text = self.site.as_ref().map(LogSite::label).unwrap_or_default();
        let choose_site = cx.listener(move |this, i: &usize, _, cx| {
            if let Some(site) = sites.get(*i) {
                this.show_site(site.clone(), cx);
            }
        });

        // The logs grouped under headings; the n-th option is the n-th source in the list's order.
        let mut source_of = Vec::new();
        let log_rows = logs::menu(&self.sources)
            .into_iter()
            .map(|entry| match entry {
                LogMenuItem::Heading(group) => SelectRow::Heading(group.into()),
                LogMenuItem::Source(index) => {
                    source_of.push(index);
                    let source = &self.sources[index];
                    SelectRow::Option {
                        text: source.title.clone().into(),
                        chosen: self.current == Some(index),
                        tip: Some(source.description.clone().into()),
                    }
                }
            })
            .collect();
        let log_text = self.current_source().map(|s| s.title.clone()).unwrap_or_default();
        let choose_log = cx.listener(move |this, i: &usize, _, cx| {
            if let Some(index) = source_of.get(*i) {
                this.open(Some(*index), cx);
            }
        });

        let facts = self.current_source().and_then(|s| s.file_path.clone()).map(|path| {
            let size = self.file_size.map(|n| dnnm_application::project::view::byte_size(n as f64));
            format!("{}{}", path.display(), size.map(|s| format!("  ·  {s}")).unwrap_or_default())
        });
        let folder = self.folder();
        let (sites_open, logs_open) = (self.menu == Some(Open::Sites), self.menu == Some(Open::Logs));
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(picker("log-site", site_text, sites_open, "Website", site_rows, toggle(Open::Sites), choose_site, cx))
            .child(picker("log-source", log_text, logs_open, "Log", log_rows, toggle(Open::Logs), choose_log, cx))
            .when_some(facts, |d, facts| {
                d.child(
                    div()
                        .max_w(px(520.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(theme.text.caption)
                        .text_color(c.log_muted)
                        .child(facts),
                )
            })
            .when_some(folder, |d, folder| {
                d.child(
                    ui::icon_button("log-folder", Icon::Folder, c.text_primary, true, cx)
                        .tip("Open folder")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Err(e) = dnnm_infrastructure::shell::open_folder(&folder) {
                                this.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                            }
                        })),
                )
            })
    }
}

/// One of the header's pickers: the compact select, 160 to 260 px wide as its value needs.
#[allow(clippy::too_many_arguments)]
fn picker(
    id: &'static str,
    value: String,
    open: bool,
    tip: &'static str,
    rows: Vec<SelectRow>,
    on_toggle: impl Fn(&bool, &mut Window, &mut gpui::App) + 'static,
    on_choose: impl Fn(&usize, &mut Window, &mut gpui::App) + 'static,
    cx: &gpui::App,
) -> gpui::Div {
    div().min_w(px(160.0)).max_w(px(260.0)).child(ui::dialog::select_rows(
        id,
        value,
        open,
        rows,
        SelectLook { compact: true, tip: Some(tip.into()) },
        on_toggle,
        on_choose,
        cx,
    ))
}

impl LogsView {
    /// The right-click menu over the lines: Copy what is selected, Select all.
    fn text_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.text_focus);
        let lines: std::rc::Rc<Vec<SharedString>> = std::rc::Rc::new(self.lines.iter().map(|(t, _)| t.clone()).collect());
        let os = ui::keys::Os::CURRENT;
        let mac = os == ui::keys::Os::Mac;
        let shortcut = |key: &str| Some(ui::keys::label(os, !mac, false, false, mac, key));
        let (copy, all) = (self.selection.clone(), self.selection.clone());
        let (copy_lines, all_lines) = (lines.clone(), lines.clone());
        let items = vec![
            ui::menu::MenuItem::new("Copy", move |_, cx| {
                if let Some(text) = copy.copied(&|i| copy_lines.get(i).cloned()) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
            })
            .enabled(self.selection.has_selection())
            .shortcut(shortcut("C")),
            ui::menu::MenuItem::new("Select all", move |window, _| {
                all.select_all(all_lines.len(), all_lines.last().map_or(0, |t| t.len()));
                window.refresh();
            })
            .enabled(!lines.is_empty())
            .shortcut(shortcut("A")),
        ];
        self.text_menu = Some(ui::menu::ContextMenu::new(position, items));
        cx.notify();
    }
}

impl Render for LogsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        if self.site.is_none() {
            // The first time the tab shows: DNN Manager's own log.
            self.show_site(LogSite::App, cx);
        }
        let hint = match self.current_source() {
            None => Some(logs::CHOOSE_HINT.to_string()),
            Some(source)
                if self.lines.is_empty()
                    && self.opened_at.is_some_and(|t| t.elapsed() >= logs::EMPTY_HINT_AFTER) =>
            {
                Some(logs::empty_hint(source).to_string())
            }
            _ => None,
        };
        let lines = std::rc::Rc::new(self.lines.clone());
        let colors = (c.log_fg, c.log_warn, c.log_fail, c.log_muted);
        let marks = self.find_marks.clone();
        let (other, current) = ui::find_colors(cx);
        // A line wraps in the panel's width, and can be selected - dragged over, double-clicked (a word),
        // triple-clicked (a line), copied with Ctrl / ⌘ + C or the right-click menu.
        let selection = self.selection.clone();
        selection.begin(&format!("{:?}", self.current_source().map(|s| s.title.clone())));
        let (rows, row_selection) = (lines.clone(), selection.clone());
        let list = list(self.scroll.clone(), move |i, _, cx| {
            let Some((text, level)) = rows.get(i) else { return div().into_any_element() };
            let color = match level {
                LogLineLevel::Normal => colors.0,
                LogLineLevel::Warning => colors.1,
                LogLineLevel::Error => colors.2,
                LogLineLevel::Note => colors.3,
            };
            let found: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = marks
                .get(&i)
                .map(|found| {
                    found
                        .iter()
                        .map(|(range, is_current)| {
                            let background_color = Some(if *is_current { current } else { other });
                            (range.clone(), gpui::HighlightStyle { background_color, ..Default::default() })
                        })
                        .collect()
                })
                .unwrap_or_default();
            div().pad(px(0.0), px(12.0)).text_color(color).child(row_selection.line(i, text.clone(), found, cx)).into_any_element()
        })
        .size_full();
        let text_of = move |lines: &std::rc::Rc<Vec<(SharedString, LogLineLevel)>>, i: usize| lines.get(i).map(|(t, _)| t.clone());
        let (press, drag, release, release_out, keys) =
            (selection.clone(), selection.clone(), selection.clone(), selection.clone(), selection.clone());
        let (press_lines, key_lines) = (lines.clone(), lines.clone());
        let list = div()
            .id("log-lines")
            .track_focus(&self.text_focus)
            .size_full()
            .cursor(gpui::CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, move |e: &gpui::MouseDownEvent, window, _| {
                press.press(e.position, e.modifiers.shift, e.click_count, &|i| text_of(&press_lines, i));
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
                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| this.text_menu(e.position, window, cx)),
            )
            .on_key_down(move |e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if !k.modifiers.secondary() || k.modifiers.alt || k.modifiers.shift {
                    return;
                }
                match k.key.as_str() {
                    "c" => {
                        if let Some(text) = keys.copied(&|i| text_of(&key_lines, i)) {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        }
                    }
                    "a" => {
                        keys.select_all(key_lines.len(), key_lines.last().map_or(0, |(t, _)| t.len()));
                        window.refresh();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            })
            .child(list);
        let header = self.render_header(cx);
        let view = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().flex_none().pad(px(0.0), px(8.0)).child(header))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(theme.mono_font.clone())
                    .text_size(theme.text.caption)
                    .child(list),
            )
            .when_some(hint, |d, hint| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(theme.font.clone())
                        .text_color(c.log_muted)
                        .child(hint),
                )
            });
        // Last: the menu borrows `cx` for as long as it is drawn.
        let menu = self.text_menu.as_ref().map(|menu| {
            let weak = cx.entity().downgrade();
            ui::menu::render(
                menu,
                move |_, cx| {
                    let _ = weak.update(cx, |v, cx| {
                        v.text_menu = None;
                        cx.notify();
                    });
                },
                |_, _, _| {},
                window,
                cx,
            )
        });
        view.children(menu)
    }
}
