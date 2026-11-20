//! The bottom panel's Logs tab (`Controls/LogsView`): a website - DNN Manager's own first, then each IIS site - and
//! one of its logs (DNN, IIS, Windows event logs, the hosts file, DNN Manager's daily log); the newest lines, followed
//! as they come. What to read and how is `dnnm_logs`; this is the view.

use dnnm_ui::Icon;
use dnnm_ui::dialog::{SelectLook, SelectRow};
use dnnm_ui::px;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{ClickEvent, Context, Entity, ListHorizontalSizingBehavior, SharedString, Task, UniformListScrollHandle, Window, div, prelude::*, uniform_list};

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
    scroll: UniformListScrollHandle,
    /// Polling only while the tab is on screen (`LogsView.Paused`).
    pub shown: bool,
    /// The panel's Find bar's matches, by line.
    find_marks: Arc<std::collections::HashMap<usize, ui::LineMarks>>,
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
            self.scroll.scroll_to_item(m.line, gpui::ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn new(store: Entity<Store>) -> Self {
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
            scroll: UniformListScrollHandle::new(),
            shown: false,
            find_marks: Arc::default(),
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
        }
        if update.lines.is_empty() && !update.replaced {
            // Still nothing: the hint shows after a moment, so a timer repaint is enough.
            cx.notify();
            return;
        }
        self.lines.extend(update.lines.iter().map(|l| {
            let (text, level) = logs::log_line(l);
            (SharedString::from(text), level)
        }));
        if self.lines.len() > MAX_LINES {
            let over = self.lines.len() - MAX_LINES;
            self.lines.drain(..over);
        }
        // Follows the end, as a log view does.
        self.scroll.scroll_to_item(self.lines.len().saturating_sub(1), gpui::ScrollStrategy::Top);
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

impl Render for LogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let lines = self.lines.clone();
        // A long line isn't cut: the list scrolls sideways, as wide as its longest line.
        let longest = lines.iter().enumerate().max_by_key(|(_, (text, _))| text.chars().count()).map(|(i, _)| i);
        let colors = (c.log_fg, c.log_warn, c.log_fail, c.log_muted);
        let marks = self.find_marks.clone();
        let list = uniform_list("log-lines", lines.len(), move |range, _, cx| {
            range
                .map(|i| {
                    let (text, level) = &lines[i];
                    let color = match level {
                        LogLineLevel::Normal => colors.0,
                        LogLineLevel::Warning => colors.1,
                        LogLineLevel::Error => colors.2,
                        LogLineLevel::Note => colors.3,
                    };
                    let line = div().pad(px(0.0), px(12.0)).whitespace_nowrap().text_color(color);
                    match marks.get(&i) {
                        Some(found) => line.child(ui::marked_text(text.clone(), found, cx)),
                        None => line.child(text.clone()),
                    }
                })
                .collect()
        })
        .track_scroll(self.scroll.clone())
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(longest)
        .size_full();
        let header = self.render_header(cx);
        div()
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
            })
    }
}
