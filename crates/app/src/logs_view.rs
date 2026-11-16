//! The bottom panel's Logs tab (`Controls/LogsView`): a website - DNN Manager's own first, then each IIS site - and
//! one of its logs (DNN, IIS, Windows event logs, the hosts file, DNN Manager's daily log); the newest lines, followed
//! as they come. What to read and how is `dnnm_logs`; this is the view.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    ClickEvent, Context, Entity, FontWeight, SharedString, Task, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};

use dnnm_logs::catalog::{self, SiteLogSource};
use dnnm_logs::tail::{LogTail, MAX_LINES};
use dnnm_logs::view::{self as logs, LogLineLevel, LogMenuItem};

use crate::store::{Store, ToastKind};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tooltip, icons};

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
    opened_at: Option<Instant>,
    menu: Option<Open>,
    scroll: UniformListScrollHandle,
    /// Polling only while the tab is on screen (`LogsView.Paused`).
    pub shown: bool,
    _poll: Option<Task<()>>,
}

impl LogsView {
    pub fn new(store: Entity<Store>) -> Self {
        Self {
            store,
            site: None,
            sources: Vec::new(),
            current: None,
            lines: Vec::new(),
            tail: None,
            opened_at: None,
            menu: None,
            scroll: UniformListScrollHandle::new(),
            shown: false,
            _poll: None,
        }
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
        self._poll = None;
        let Some(source) = index.and_then(|i| self.sources.get(i)).cloned() else {
            cx.notify();
            return;
        };
        let tail = Arc::new(Mutex::new(source.open()));
        self.tail = Some(tail.clone());
        self.opened_at = Some(Instant::now());
        // The newest lines off the UI thread, then the new ones every second (two for the hosts file).
        self._poll = Some(cx.spawn(async move |this, cx| {
            let first = tail.clone();
            let update = cx
                .background_executor()
                .spawn(async move { first.lock().map(|mut t| t.start(dnnm_logs::tail::INITIAL_LINES)).ok() })
                .await;
            let Some(update) = update else { return };
            let every = tail.lock().map(|t| t.poll_every()).unwrap_or(std::time::Duration::from_secs(1));
            if this.update(cx, |v, cx| v.add(update, cx)).is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(every).await;
                let Ok(shown) = this.update(cx, |v, _| v.shown) else { return };
                if !shown {
                    continue;
                }
                let next = tail.clone();
                let update =
                    cx.background_executor().spawn(async move { next.lock().map(|mut t| t.poll()).ok() }).await;
                let Some(update) = update else { return };
                if this.update(cx, |v, cx| v.add(update, cx)).is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    fn add(&mut self, update: dnnm_logs::tail::TailUpdate, cx: &mut Context<Self>) {
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

    /// The pickers and the log's facts - the header row of the tab.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let picker = |id: &'static str, text: String, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .min_w(px(160.0))
                .max_w(px(260.0))
                .h(px(24.0))
                .pad(px(0.0), px(6.0))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(c.field_border)
                .bg(c.input_bg)
                .cursor_pointer()
                .text_size(theme.text.caption)
                .tooltip(Tooltip::text(tip))
                .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text))
                .child(ui::icon(icons::CHEVRON_DOWN, 9.0, c.text_primary, cx))
        };
        let site_text = self.site.as_ref().map(LogSite::label).unwrap_or_default();
        let log_text = self.current_source().map(|s| s.title.clone()).unwrap_or_default();
        let facts = self.current_source().and_then(|s| s.file_path.clone()).map(|path| {
            let meta = std::fs::metadata(&path).ok();
            let size = meta.as_ref().map(|m| dnnm_application::project::view::byte_size(m.len() as f64));
            format!("{}{}", path.display(), size.map(|s| format!("  ·  {s}")).unwrap_or_default())
        });
        let folder = self.folder();
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(picker("log-site", site_text, "Website").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.menu = Some(Open::Sites);
                cx.notify();
            })))
            .child(picker("log-source", log_text, "Log").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.menu = Some(Open::Logs);
                cx.notify();
            })))
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
                    ui::icon_button("log-folder", icons::FOLDER, c.text_primary, true, cx)
                        .tooltip(Tooltip::text("Open folder"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Err(e) = dnnm_infrastructure::shell::open_folder(&folder) {
                                this.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                            }
                        })),
                )
            })
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let open = self.menu.as_ref()?;
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let item = |id: SharedString, text: String, chosen: bool, indent: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.0))
                .pad(px(3.0), px(if indent { 20.0 } else { 8.0 }))
                .rounded(px(3.0))
                .cursor_pointer()
                .when(chosen, |d| d.bg(c.selection_bg))
                .hover(move |s| s.bg(c.hover_bg))
                .child(text)
        };
        let mut list = div().flex().flex_col();
        match open {
            Open::Sites => {
                let mut choices = vec![LogSite::App];
                choices.extend(
                    self.store.read(cx).projects.iter().map(|p| LogSite::Site(p.name.clone())).collect::<Vec<_>>(),
                );
                for (i, site) in choices.into_iter().enumerate() {
                    let chosen = self.site.as_ref() == Some(&site);
                    let label = site.label();
                    list = list.child(item(format!("log-site-{i}").into(), label, chosen, false).on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.show_site(site.clone(), cx)),
                    ));
                }
            }
            Open::Logs => {
                for entry in logs::menu(&self.sources) {
                    match entry {
                        LogMenuItem::Heading(group) => {
                            list = list.child(
                                div()
                                    .pad(px(4.0), px(8.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c.text_muted)
                                    .child(group),
                            );
                        }
                        LogMenuItem::Source(index) => {
                            let source = &self.sources[index];
                            let tip = source.description.clone();
                            list = list.child(
                                item(
                                    format!("log-source-{index}").into(),
                                    source.title.clone(),
                                    self.current == Some(index),
                                    true,
                                )
                                .tooltip(Tooltip::text(tip))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open(Some(index), cx))),
                            );
                        }
                    }
                }
            }
        }
        Some(
            div()
                .id("logs-menu-backdrop")
                .absolute()
                .inset_0()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.menu = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("logs-menu")
                        .absolute()
                        .top(px(30.0))
                        .left(px(if matches!(open, Open::Sites) { 8.0 } else { 176.0 }))
                        .min_w(px(220.0))
                        .max_h(px(360.0))
                        .overflow_y_scroll()
                        .p(px(4.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(c.card_border)
                        .bg(c.card_bg)
                        .shadow_lg()
                        .occlude()
                        .text_size(theme.text.caption)
                        .text_color(c.text_primary)
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(list),
                ),
        )
    }
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
        let colors = (c.log_fg, c.log_warn, c.log_fail, c.log_muted);
        let list = uniform_list("log-lines", lines.len(), move |range, _, _| {
            range
                .map(|i| {
                    let (text, level) = &lines[i];
                    let color = match level {
                        LogLineLevel::Normal => colors.0,
                        LogLineLevel::Warning => colors.1,
                        LogLineLevel::Error => colors.2,
                        LogLineLevel::Note => colors.3,
                    };
                    div().pad(px(0.0), px(12.0)).whitespace_nowrap().text_color(color).child(text.clone())
                })
                .collect()
        })
        .track_scroll(self.scroll.clone())
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
            .children(self.render_menu(cx))
    }
}
