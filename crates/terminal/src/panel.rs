//! The Terminal tab of the bottom panel: the shown terminal, and the list of terminals on the right - ports the
//! terminal parts of `Presentation/Controls/TerminalPanel.xaml(.cs)`.
//!
//! In the list: a click selects a terminal and puts the keyboard on it; a second click (or Enter) goes into the
//! terminal; F2 or a double-click renames it, Del closes it, Alt+Up / Alt+Down move it; the right-click menu has
//! Rename and Close terminal. The panel's header (the app's) carries [`TerminalPanelView::toolbar`]: + for a new
//! terminal, the arrow beside it for another shell or an Administrator terminal.

use crate::scale::px;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gpui::{AnyElement, AnyView, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Subscription, Window, anchored, deferred, div, prelude::*};

use crate::session::TerminalSession;
use crate::shells::{Shells, TerminalShell};
use crate::view::{TerminalStyle, TerminalView, TerminalViewEvent};

const LIST_WIDTH: f32 = 176.0;
const MAX_TITLE: usize = 60;
/// The icon sizes of dnnm-ui's `icons` (this crate doesn't depend on it): actions 13, chevrons 10.
const ICON_SIZE: f32 = 13.0;
const ICON_SIZE_SMALL: f32 = 10.0;

/// The panel's icons: the asset paths of dnnm-ui's `Icon`s, given by the app (this crate doesn't depend on dnnm-ui),
/// so they are the same drawings as everywhere else. `add`, `delete` and `shell` at `IconSize::Medium`
/// (`ICON_SIZE`), `chevron_down` at `IconSize::Small` (`ICON_SIZE_SMALL`).
#[derive(Debug, Clone, PartialEq)]
pub struct PanelIcons {
    pub add: SharedString,
    pub chevron_down: SharedString,
    pub delete: SharedString,
    pub shell: SharedString,
}

/// The panel's colours and fonts, from the app's theme (named after the XAML keys).
#[derive(Debug, Clone, PartialEq)]
pub struct PanelColors {
    /// `LogMuted` - a terminal in the list, and the empty text.
    pub log_muted: Hsla,
    /// `LogHeaderFg` - the shown terminal in the list.
    pub log_header_fg: Hsla,
    pub hover_bg: Hsla,
    pub selection_bg: Hsla,
    pub accent: Hsla,
    pub card_bg: Hsla,
    pub card_border: Hsla,
    pub text_primary: Hsla,
    pub text_muted: Hsla,
    pub warn_banner_bg: Hsla,
    pub warn_banner_border: Hsla,
    /// `OutWarnText` - the Administrator terminal's line.
    pub out_warn_text: Hsla,
    /// The UI font (Segoe UI).
    pub font: SharedString,
    pub icons: PanelIcons,
    /// `TextCaption` and `TextBody`.
    pub caption_size: Pixels,
    pub body_size: Pixels,
}

/// What the panel asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalPanelEvent {
    /// A terminal couldn't start - shown as an error dialog (the C# `Dialogs.Error`).
    Error(String),
}

struct Tab {
    id: usize,
    title: String,
    /// Shown under the name when the mouse rests on the terminal.
    details: Vec<(&'static str, String)>,
    is_administrator: bool,
    view: Entity<TerminalView>,
    _exited: Subscription,
}

enum Menu {
    /// Under the shells button, their right edges level (the corner of the menu at the point).
    Shells(Point<Pixels>, gpui::Corner),
    /// A terminal's right-click menu, where the mouse is.
    Tab(usize, Point<Pixels>),
}

pub struct TerminalPanelView {
    /// Where the shells button was last drawn: its menu opens under it.
    shells_button: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    tabs: Vec<Tab>,
    selected: Option<usize>,
    next_id: usize,
    list_focus: FocusHandle,
    renaming: Option<(usize, String)>,
    menu: Option<Menu>,
    shells: Shells,
    default_shell: String,
    working_directory: String,
    terminal_style: TerminalStyle,
    colors: PanelColors,
}

impl EventEmitter<TerminalPanelEvent> for TerminalPanelView {}

impl TerminalPanelView {
    /// `shells` from [`crate::find_shells`] (found once, off the UI thread); `default_shell` the setting
    /// `terminal.defaultShell`; `working_directory` the projects folder, where a new shell starts.
    pub fn new(
        shells: Shells,
        default_shell: impl Into<String>,
        working_directory: impl Into<String>,
        terminal_style: TerminalStyle,
        colors: PanelColors,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            tabs: Vec::new(),
            selected: None,
            next_id: 0,
            // A tab stop: Tab reaches the list, then its keys (`list_key_down`) work it.
            list_focus: cx.focus_handle().tab_stop(true),
            renaming: None,
            menu: None,
            shells_button: Default::default(),
            shells,
            default_shell: default_shell.into(),
            working_directory: working_directory.into(),
            terminal_style,
            colors,
        }
    }

    /// The settings changed (Settings page): the default shell and where a new shell starts.
    pub fn set_settings(&mut self, default_shell: impl Into<String>, working_directory: impl Into<String>) {
        self.default_shell = default_shell.into();
        self.working_directory = working_directory.into();
    }

    /// The theme or the terminal font changed: every terminal is drawn with it.
    pub fn set_style(&mut self, terminal_style: TerminalStyle, colors: PanelColors, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| view.set_style(terminal_style.clone(), cx));
        }
        self.terminal_style = terminal_style;
        self.colors = colors;
        cx.notify();
    }

    /// Shells are open - closing the window ends what runs in them.
    pub fn open_terminals(&self) -> usize {
        self.tabs.len()
    }

    /// The shown terminal's view.
    pub fn shown(&self) -> Option<&Entity<TerminalView>> {
        self.selected.and_then(|i| self.tabs.get(i)).map(|tab| &tab.view)
    }

    /// Puts the keyboard in the shown terminal - a new one when none is open (Ctrl+`, the Terminal tab shown), as VS
    /// Code does.
    pub fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            self.new_terminal(None, None, window, cx);
        } else {
            self.focus_terminal(window, cx);
        }
    }

    pub fn focus_terminal(&self, window: &mut Window, cx: &App) {
        if let Some(view) = self.shown() {
            window.focus(&view.focus_handle(cx));
        }
    }

    /// Opens a terminal with `shell` (the default one when None) in `directory` (the projects folder when None) and
    /// shows it. A terminal runs as the signed-in user, without DNN Manager's administrator rights - unless the shell is
    /// one of the administrator shells. When it can't start without them, that is said, and nothing starts in its place.
    pub fn new_terminal(
        &mut self,
        shell: Option<TerminalShell>,
        directory: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shell) = shell.or_else(|| self.shells.default_shell(&self.default_shell).cloned()) else { return };
        if let Some(refusal) = &shell.refusal {
            cx.emit(TerminalPanelEvent::Error(refusal.clone()));
            return;
        }
        let in_project = directory.is_some();
        let mut directory = directory.unwrap_or_else(|| self.working_directory.clone());
        if !Path::new(&directory).is_dir() {
            directory = std::env::var("USERPROFILE").unwrap_or_default();
        }

        // "PowerShell", "PowerShell (2)"… - or the folder's name for a terminal opened on a project; "Administrator: …"
        // for one with those rights, as Windows titles its windows.
        let shell_name = shell.title();
        let name = if in_project {
            let folder =
                Path::new(directory.trim_end_matches('\\')).file_name().map(|f| f.to_string_lossy().into_owned());
            format!("{} - {shell_name}", folder.unwrap_or_default())
        } else {
            shell_name
        };
        let mut title = name.clone();
        let mut n = 2;
        while self.tabs.iter().any(|t| t.title == title) {
            title = format!("{name} ({n})");
            n += 1;
        }

        let session = match TerminalSession::start(&shell.command_line(), &directory, 100, 24, shell.as_administrator) {
            Ok(session) => session,
            #[cfg(windows)]
            Err(error) if crate::token::is_unavailable(&error) => {
                cx.emit(TerminalPanelEvent::Error(format!(
                        "{error}\r\n\r\nNothing was started with administrator rights in its place - for a terminal with them, choose New Administrator terminal (the arrow beside +)."
                    )));
                return;
            }
            Err(error) => {
                cx.emit(TerminalPanelEvent::Error(format!("Could not start {}: {error}", shell.name)));
                return;
            }
        };

        let mut details = vec![
            ("Terminal", shell.name.to_string()),
            (
                "Runs as",
                if shell.as_administrator {
                    "Administrator - every command has DNN Manager's administrator rights".to_string()
                } else {
                    "You, without administrator rights".to_string()
                },
            ),
            ("Process ID", session.process_id().to_string()),
            ("Program", shell.exe_path.clone()),
        ];
        if !shell.arguments.is_empty() {
            details.push(("Arguments", shell.arguments.to_string()));
        }
        details.push(("Started in", directory));
        details.push(("Started", now_text()));

        let style = self.terminal_style.clone();
        let view = cx.new(|cx| TerminalView::new(session, style, cx));
        let id = self.next_id;
        self.next_id += 1;
        // The shell ended by itself ("exit"): its tab goes too.
        let exited = cx.subscribe(&view, move |panel, _, event, cx| match event {
            TerminalViewEvent::Exited => panel.close(id, cx),
        });
        self.tabs.push(Tab {
            id,
            title,
            details,
            is_administrator: shell.as_administrator,
            view: view.clone(),
            _exited: exited,
        });
        self.selected = Some(self.tabs.len() - 1);
        window.focus(&view.focus_handle(cx));
        cx.notify();
    }

    /// Opens a terminal with administrator rights - the shell the settings name when it may run so, otherwise Command
    /// Prompt - in `directory` (the projects folder when None).
    pub fn new_administrator_terminal(
        &mut self,
        directory: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shell = self.shells.default_administrator_shell(&self.default_shell).cloned();
        if shell.is_some() {
            self.new_terminal(shell, directory, window, cx);
        }
    }

    fn index_of(&self, id: usize) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Closes terminal `id`, ending its shell; the next one is shown.
    fn close(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else { return };
        let tab = self.tabs.remove(index);
        tab.view.update(cx, |view, _| view.close());
        if self.renaming.as_ref().is_some_and(|(r, _)| *r == id) {
            self.renaming = None;
        }
        self.selected = match self.selected {
            _ if self.tabs.is_empty() => None,
            Some(selected) if selected > index => Some(selected - 1),
            Some(selected) => Some(selected.min(self.tabs.len() - 1)),
            None => None,
        };
        cx.notify();
    }

    /// Ends every shell - when the window closes, or the terminal is switched off.
    pub fn close_all(&mut self, cx: &mut Context<Self>) {
        for id in self.tabs.iter().map(|t| t.id).collect::<Vec<_>>() {
            self.close(id, cx);
        }
    }

    /// Moves the shown terminal by `by` places in the list (kept within it), still the one shown.
    fn move_selected(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(from) = self.selected else { return };
        let to = (from as isize + by).clamp(0, self.tabs.len() as isize - 1) as usize;
        if to != from {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            self.selected = Some(to);
            cx.notify();
        }
    }

    fn begin_rename(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.index_of(id) {
            self.renaming = Some((id, self.tabs[index].title.clone()));
            self.selected = Some(index);
            window.focus(&self.list_focus);
            cx.notify();
        }
    }

    fn end_rename(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some((id, text)) = self.renaming.take() else { return };
        let name = text.trim();
        if keep
            && !name.is_empty()
            && let Some(index) = self.index_of(id)
        {
            self.tabs[index].title = name.to_string();
        }
        cx.notify();
    }

    // In the list: F2 renames the selected terminal, Del closes it, Enter puts the keyboard in it, Alt+Up / Alt+Down
    // move it up or down the list, Up / Down choose another. While its name is being typed, the keys are the name's.
    fn list_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let key = keystroke.key.as_str();
        if let Some((_, text)) = &mut self.renaming {
            match key {
                "enter" => self.end_rename(true, cx),
                "escape" => self.end_rename(false, cx),
                "backspace" => {
                    text.pop();
                    cx.notify();
                }
                _ => {
                    if let Some(typed) = keystroke.key_char.as_deref().filter(|_| !modifiers.control && !modifiers.alt && !modifiers.platform)
                        && text.chars().count() + typed.chars().count() <= MAX_TITLE
                    {
                        text.push_str(typed);
                        cx.notify();
                    }
                }
            }
            return cx.stop_propagation();
        }
        let Some(selected) = self.selected else { return };
        let id = self.tabs[selected].id;
        if modifiers.alt && !modifiers.control && !modifiers.shift && matches!(key, "up" | "down") {
            self.move_selected(if key == "up" { -1 } else { 1 }, cx);
            return cx.stop_propagation();
        }
        if modifiers.modified() {
            return;
        }
        match key {
            "f2" => self.begin_rename(id, window, cx),
            "delete" => self.close(id, cx),
            "enter" => self.focus_terminal(window, cx),
            "up" if selected > 0 => {
                self.selected = Some(selected - 1);
                cx.notify();
            }
            "down" if selected + 1 < self.tabs.len() => {
                self.selected = Some(selected + 1);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// A click on a terminal in the list puts the keyboard there - on that entry - so Del, F2 and Enter act on it at
    /// once; a second click on it (the keyboard already there) goes into the terminal. A double-click renames it.
    fn tab_mouse_down(&mut self, id: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else { return };
        if self.renaming.as_ref().is_some_and(|(r, _)| *r == id) {
            return;
        }
        self.end_rename(true, cx);
        self.menu = None;
        if event.click_count == 2 {
            self.begin_rename(id, window, cx);
        } else if self.selected == Some(index) && self.list_focus.is_focused(window) {
            self.focus_terminal(window, cx);
        } else {
            self.selected = Some(index);
            window.focus(&self.list_focus);
        }
        cx.notify();
    }

    // ─── Rendering ────────────────────────────────────────────────────────

    /// The + and the arrow beside it, for the panel's header while the Terminal tab is shown.
    pub fn toolbar(panel: &Entity<Self>, cx: &App) -> AnyElement {
        let colors = panel.read(cx).colors.clone();
        let new_panel = panel.clone();
        let menu_panel = panel.clone();
        let button = panel.read(cx).shells_button.clone();
        let measured = button.clone();
        // Its tip is left out while its list is open - it would cover the list.
        let shells_open = matches!(panel.read(cx).menu, Some(Menu::Shells(..)));
        div()
            .flex()
            .items_center()
            .child(
                panel_button("terminal-new", colors.icons.add.clone(), ICON_SIZE, &colors, 26.0)
                    .tip(vec![Line::Plain("New terminal".into())], &colors)
                    .on_click(move |_, window, cx| {
                        new_panel.update(cx, |panel, cx| panel.new_terminal(None, None, window, cx));
                    }),
            )
            .child(
                panel_button("terminal-shells", colors.icons.chevron_down.clone(), ICON_SIZE_SMALL, &colors, 18.0)
                    .when(!shells_open, |d| {
                        d.tip(vec![Line::Plain("Choose shell - or New Administrator terminal".into())], &colors)
                    })
                    .relative()
                    // Its place, for the menu to open under it.
                    .child(
                        gpui::canvas(move |bounds, _, _| measured.set(Some(bounds)), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .on_click(move |event, _, cx| {
                        let (position, corner) = match button.get() {
                            Some(b) => (gpui::point(b.right(), b.bottom() + px(4.)), gpui::Corner::TopRight),
                            None => (event.position(), gpui::Corner::TopLeft),
                        };
                        menu_panel.update(cx, |panel, cx| {
                            // A second click closes it.
                            panel.menu = match panel.menu {
                                Some(Menu::Shells(..)) => None,
                                _ => Some(Menu::Shells(position, corner)),
                            };
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }

    fn render_list(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let list_focused = self.list_focus.is_focused(window);
        let mut list = div()
            .id("terminal-list")
            .key_context("TerminalList")
            .track_focus(&self.list_focus)
            .on_key_down(cx.listener(Self::list_key_down))
            .flex_none()
            .w(px(LIST_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(colors.card_border)
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .overflow_y_scroll()
            .font_family(colors.font.clone())
            .text_size(colors.body_size);
        for (index, tab) in self.tabs.iter().enumerate() {
            let id = tab.id;
            let shown = self.selected == Some(index);
            let group: SharedString = format!("terminal-tab-{id}").into();
            let renaming = self.renaming.as_ref().filter(|(r, _)| *r == id).map(|(_, text)| text.clone());
            let mut lines = vec![
                Line::Bold(tab.title.clone()),
                Line::Muted(
                    "Click again or Enter: type in it · Del: close it · F2 or double-click: rename · Alt+↑/↓: move"
                        .into(),
                ),
            ];
            lines.extend(tab.details.iter().map(|(k, v)| Line::Fact(k, v.clone())));
            let name = match renaming {
                Some(text) => div()
                    .flex_1()
                    .pad(px(0.), px(3.))
                    .border_1()
                    .border_color(colors.accent)
                    .text_color(colors.text_primary)
                    .child(format!("{text}|")),
                // Clamped to one line rather than `truncate()`: GPUI keeps the first size it measures for text that doesn't wrap,
                // so that only clipped it in a narrow list (dnnm-ui's `one_line` does the same).
                None => div().flex_1().min_w_0().overflow_hidden().line_clamp(1).text_ellipsis().child(tab.title.clone()),
            };
            let row = div()
                .id(("terminal-tab", id))
                .group(group.clone())
                .flex()
                .items_center()
                .flex_none()
                .min_h(px(24.))
                .pad(px(0.), px(6.))
                .rounded(px(4.))
                .border_1()
                .border_color(if shown && list_focused { colors.accent } else { gpui::transparent_black() })
                .cursor_pointer()
                .text_color(if shown { colors.log_header_fg } else { colors.log_muted })
                .when(shown, |row| row.bg(if list_focused { colors.selection_bg } else { colors.hover_bg }))
                .when(!shown, |row| row.hover(|style| style.bg(colors.hover_bg)))
                .tip(lines, colors)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |panel, event, window, cx| panel.tab_mouse_down(id, event, window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                        panel.menu = Some(Menu::Tab(id, event.position));
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.))
                        .child(gpui::svg().path(colors.icons.shell.clone()).size(px(ICON_SIZE)).flex_none().text_color(colors.log_muted))
                        .child(name),
                )
                .child(
                    panel_button(("terminal-close", id), colors.icons.delete.clone(), ICON_SIZE, colors, 22.0)
                        // Inside the list's 22 px row.
                        .h(px(22.))
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .tip(vec![Line::Plain("Close terminal (Del)".into())], colors)
                        .on_click(cx.listener(move |panel, _, _, cx| panel.close(id, cx))),
                );
            list = list.child(row);
        }
        list
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = &self.colors;
        let (position, corner, items) = match self.menu.as_ref()? {
            // The installed shells, the default one marked - as you; then the same as Administrator, those that may
            // not run so greyed with why.
            Menu::Shells(position, corner) => {
                let default = self.shells.default_shell(&self.default_shell).map(|s| s.key);
                // The items 4 px in from the sides; the line across the menu 4 px from what is above and below it.
                let mut user = div().flex().flex_col().pad(px(0.), px(4.));
                for shell in &self.shells.user {
                    let label = if Some(shell.key) == default {
                        format!("{}  (default)", shell.name)
                    } else {
                        shell.name.to_string()
                    };
                    user = user.child(self.shell_item(shell, label, cx));
                }
                let mut administrator = div().flex().flex_col().pad(px(0.), px(4.)).child(
                    div()
                        .id("terminal-admin-header")
                        .pad(px(4.), px(6.))
                        .text_color(colors.text_muted)
                        .child("New Administrator terminal")
                        .tip(
                            vec![Line::Plain(
                                "Every command typed in it runs with DNN Manager's administrator rights - only for what needs them."
                                    .into(),
                            )],
                            colors,
                        ),
                );
                for shell in &self.shells.administrator {
                    administrator = administrator.child(self.shell_item(shell, format!("    {}", shell.name), cx));
                }
                let line = div().h(px(1.)).bg(colors.card_border);
                (*position, *corner, vec![user.into_any_element(), line.into_any_element(), administrator.into_any_element()])
            }
            Menu::Tab(id, position) => {
                let id = *id;
                let rename = menu_item(("terminal-rename", id), "Rename", Some("F2"), true, colors)
                    .on_click(cx.listener(move |panel, _, window, cx| {
                        panel.menu = None;
                        panel.begin_rename(id, window, cx);
                    }))
                    .into_any_element();
                let close = menu_item(("terminal-close-menu", id), "Close terminal", Some("Del"), true, colors)
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.menu = None;
                        panel.close(id, cx);
                    }))
                    .into_any_element();
                (
                    *position,
                    gpui::Corner::TopLeft,
                    vec![div().flex().flex_col().pad(px(0.), px(4.)).child(rename).child(close).into_any_element()],
                )
            }
        };
        let menu = div()
            .id("terminal-menu")
            .occlude()
            .min_w(px(200.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .pad(px(4.), px(0.))
            .rounded(px(6.))
            .border_1()
            .border_color(colors.card_border)
            .bg(colors.card_bg)
            .shadow_lg()
            .font_family(colors.font.clone())
            .text_size(colors.body_size)
            .text_color(colors.text_primary)
            // A press outside closes it - not one on the shells button, whose own click does.
            .on_mouse_down_out(cx.listener(|panel, e: &MouseDownEvent, _, cx| {
                if !panel.shells_button.get().is_some_and(|b| b.contains(&e.position)) {
                    panel.menu = None;
                    cx.notify();
                }
            }))
            .children(items);
        Some(
            deferred(anchored().anchor(corner).position(position).snap_to_window().child(menu))
                .with_priority(2)
                .into_any_element(),
        )
    }

    fn shell_item(&self, shell: &TerminalShell, label: String, cx: &mut Context<Self>) -> AnyElement {
        let enabled = shell.refusal.is_none();
        let tooltip = shell.refusal.clone().unwrap_or_else(|| shell.exe_path.clone());
        let id: SharedString = format!("terminal-shell-{}-{}", shell.key, shell.as_administrator).into();
        let chosen = shell.clone();
        menu_item(id, &label, None, enabled, &self.colors)
            // Greyed, it still says why.
            .tip(vec![Line::Plain(tooltip)], &self.colors)
            .when(enabled, |item| {
                item.on_click(cx.listener(move |panel, _, window, cx| {
                    panel.menu = None;
                    panel.new_terminal(Some(chosen.clone()), None, window, cx);
                }))
            })
            .into_any_element()
    }
}

impl Focusable for TerminalPanelView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.shown().map(|view| view.focus_handle(cx)).unwrap_or_else(|| self.list_focus.clone())
    }
}

impl Render for TerminalPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors.clone();
        let shown = self.selected.and_then(|i| self.tabs.get(i));
        let mut pane = div().flex().flex_col().flex_1().h_full().min_w(px(0.));
        match shown {
            Some(tab) => {
                // One line over an Administrator terminal: what is typed there runs with DNN Manager's rights.
                if tab.is_administrator {
                    pane = pane.child(
                        div()
                            .pad(px(3.), px(8.))
                            .bg(colors.warn_banner_bg)
                            .border_b_1()
                            .border_color(colors.warn_banner_border)
                            .text_color(colors.out_warn_text)
                            .text_size(colors.caption_size)
                            .font_family(colors.font.clone())
                            .child("Administrator terminal - every command typed here runs with administrator rights."),
                    );
                }
                pane = pane.child(div().flex_1().min_h(px(0.)).child(tab.view.clone()));
            }
            None => {
                pane = pane.items_center().justify_center().child(
                    div()
                        .text_color(colors.log_muted)
                        .font_family(colors.font.clone())
                        .text_size(colors.body_size)
                        .child("No terminal open - + starts one."),
                );
            }
        }
        div()
            .flex()
            .size_full()
            .bg(self.terminal_style.background)
            .child(pane)
            .child(self.render_list(window, cx))
            .children(self.render_menu(cx))
    }
}

/// A container's one padding: `vertical` above and below, `horizontal` left and right (dnnm-ui's `Pad`; this crate
/// doesn't depend on dnnm-ui). Space between elements is the parent's flex gap.
trait Pad: Styled + Sized {
    fn pad(mut self, vertical: Pixels, horizontal: Pixels) -> Self {
        let style = self.style();
        style.padding.top = Some(vertical.into());
        style.padding.bottom = Some(vertical.into());
        style.padding.left = Some(horizontal.into());
        style.padding.right = Some(horizontal.into());
        self
    }
}

impl<T: Styled> Pad for T {}

/// A transparent button with an icon (the XAML's `PanelButton`): `icon` an asset path of [`PanelIcons`], `size` px.
fn panel_button(
    id: impl Into<gpui::ElementId>,
    icon: SharedString,
    size: f32,
    colors: &PanelColors,
    width: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        // A tab stop, pressed by Enter or Space - as dnnm-ui's controls (its "Control" context, so the key presses
        // only this); a mouse press leaves the focus where it was. Its focus look is the hover's.
        .tab_index(0)
        .key_context("Control")
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| window.prevent_default())
        .focus(|style| style.bg(colors.hover_bg).border_color(colors.accent))
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(width))
        // The panel's buttons (`PanelButton`): 26 high, the glyph in LogMuted - as Clear, Find, Maximize and Hide.
        .h(px(26.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(colors.hover_bg))
        .child(gpui::svg().path(icon).size(px(size)).flex_none().text_color(colors.log_muted))
}

fn menu_item(
    id: impl Into<gpui::ElementId>,
    label: &str,
    gesture: Option<&'static str>,
    enabled: bool,
    colors: &PanelColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .justify_between()
        .gap(px(24.))
        .pad(px(4.), px(10.))
        .rounded(px(4.))
        .when(enabled, |item| item.cursor_pointer().hover(|style| style.bg(colors.hover_bg)))
        .when(!enabled, |item| item.opacity(0.5))
        .child(label.to_string())
        .children(gesture.map(|g| div().text_color(colors.text_muted).child(g)))
}

/// A line of a tooltip.
#[derive(Clone)]
enum Line {
    Plain(String),
    Bold(String),
    Muted(String),
    Fact(&'static str, String),
}

/// A tooltip, lined up with its element as dnnm-ui's: under it and centred on it (above it without room below).
struct Tip {
    lines: Vec<Line>,
    colors: PanelColors,
    /// Where its element was drawn.
    of: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Its card's size, learnt as it is first drawn (out of sight).
    size: Rc<Cell<Option<gpui::Size<Pixels>>>>,
}

impl Render for Tip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let colors = &self.colors;
        let lines = self.lines.iter().map(|line| match line {
            Line::Plain(text) => div().child(text.clone()),
            Line::Bold(text) => div().font_weight(FontWeight::SEMIBOLD).child(text.clone()),
            // 2 px above and 4 below before; 3 each way now.
            Line::Muted(text) => div().pad(px(3.), px(0.)).text_color(colors.text_muted).child(text.clone()),
            Line::Fact(name, value) => div()
                .flex()
                .child(div().w(px(84.)).flex_none().text_color(colors.text_muted).child(*name))
                .child(div().flex_1().child(value.clone())),
        });
        let measured = self.size.clone();
        let card = div()
                .relative()
                .max_w(px(520.))
                .pad(px(4.), px(8.))
                .rounded(px(4.))
                .border_1()
                .border_color(colors.card_border)
                .bg(colors.card_bg)
                .shadow_md()
                .text_size(colors.caption_size)
                .text_color(colors.text_primary)
                .font_family(colors.font.clone())
                .children(lines)
                .child(
                    gpui::canvas(
                        move |b, _, cx| {
                            if measured.get() != Some(b.size) {
                                measured.set(Some(b.size));
                                // Drawn again with it, once this frame is done (a refresh asked for while drawing is
                                // dropped).
                                let this = this.clone();
                                cx.defer(move |cx| {
                                    this.update(cx, |_, cx| cx.notify()).ok();
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );
        let origin = match (self.of.get(), self.size.get()) {
            (Some(of), Some(size)) => {
                let (gap, window_size) = (px(4.), window.viewport_size());
                let x = (of.center().x - size.width / 2.).min(window_size.width - size.width - gap).max(gap);
                let below = of.bottom() + gap;
                let y = if below + size.height <= window_size.height { below } else { of.top() - gap - size.height };
                gpui::point(x, y.max(px(0.)))
            }
            _ => gpui::point(px(0.), px(0.)),
        };
        // Not seen until it knows its size (`anchored` keeps it inside the window, so off-screen showed it top left).
        let card = if self.size.get().is_some() { card } else { card.invisible() };
        anchored().position_mode(gpui::AnchoredPositionMode::Window).position(origin).child(card)
    }
}

/// An element's tooltip of `lines`, lined up with it.
trait WithTip: ParentElement + StatefulInteractiveElement + Sized {
    fn tip(self, lines: Vec<Line>, colors: &PanelColors) -> Self {
        let of: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
        let measured = of.clone();
        let colors = colors.clone();
        self.child(gpui::canvas(move |b, _, _| measured.set(Some(b)), |_, _, _, _| {}).absolute().inset_0()).tooltip(
            move |_, cx| -> AnyView {
                let (lines, colors, of) = (lines.clone(), colors.clone(), of.clone());
                cx.new(|_| Tip { lines, colors, of, size: Rc::default() }).into()
            },
        )
    }
}

impl<E: ParentElement + StatefulInteractiveElement + Sized> WithTip for E {}

/// Now, in the user's own short date and time format (off Windows, ISO).
#[cfg(not(windows))]
fn now_text() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm), UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rest / 3_600, rest % 3_600 / 60)
}

/// Now, as the user's Windows writes a short date and time (.NET's "g").
#[cfg(windows)]
fn now_text() -> String {
    use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::core::PCWSTR;
    // SAFETY: the buffers are passed as slices with their lengths; the time outlives the calls.
    unsafe {
        let now = GetLocalTime();
        let mut date = [0u16; 64];
        let mut time = [0u16; 64];
        let d = GetDateFormatEx(
            PCWSTR::null(),
            DATE_SHORTDATE,
            Some(&now),
            PCWSTR::null(),
            Some(&mut date),
            PCWSTR::null(),
        );
        let t = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&now), PCWSTR::null(), Some(&mut time));
        let text = |buffer: &[u16], length: i32| String::from_utf16_lossy(&buffer[..(length.max(1) - 1) as usize]);
        format!("{} {}", text(&date, d), text(&time, t))
    }
}
