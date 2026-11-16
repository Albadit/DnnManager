//! The command palette (`Controls/CommandPalette`): over the top of the window, VS Code's quick input. Ctrl+P lists
//! the projects (typing `>` switches to the commands), Ctrl+Shift+P the commands; a pick lists a choice to make
//! ("which project?"). Arrows move (round at the ends), Enter runs, Esc closes.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, SharedString, Subscription,
    Window, actions, div, prelude::*, px,
};

use dnnm_ui::Pad;
use dnnm_ui::theme::Theme;
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, icons};

actions!(palette, [SelectNext, SelectPrevious, PageDown, PageUp, Confirm, Dismiss]);

pub fn bind_keys(cx: &mut App) {
    let context = Some("Palette");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("pagedown", PageDown, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("escape", Dismiss, context),
    ]);
}

pub type ItemAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// One row: its title, a detail on the right (or its group's name on the group's first row), a shortcut.
#[derive(Clone)]
pub struct PaletteItem {
    pub title: SharedString,
    pub detail: SharedString,
    pub keywords: SharedString,
    pub shortcut: SharedString,
    /// "recently used", "other commands" - or empty.
    pub group: SharedString,
    /// Ticked (the theme in use).
    pub checked: bool,
    pub action: ItemAction,
}

impl PaletteItem {
    pub fn new(title: impl Into<SharedString>, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self {
            title: title.into(),
            detail: SharedString::default(),
            keywords: SharedString::default(),
            shortcut: SharedString::default(),
            group: SharedString::default(),
            checked: false,
            action: Rc::new(action),
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn keywords(mut self, keywords: impl Into<SharedString>) -> Self {
        self.keywords = keywords.into();
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = shortcut.into();
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = group.into();
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// `PaletteFilter.Rank`: the items with every typed word in their title, detail or keywords, group by group as
/// listed; in a group those whose title starts with the text first, then those with a word starting with the first
/// word, then the rest, each in their own order.
pub fn rank(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split(' ').map(str::trim).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    if words.is_empty() {
        return (0..items.len()).collect();
    }
    let text = query.trim().to_lowercase();
    let mut groups: Vec<&str> = Vec::new();
    for item in items {
        if !groups.contains(&item.group.as_ref()) {
            groups.push(item.group.as_ref());
        }
    }
    let mut ranked: Vec<(usize, usize, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let all = format!("{} {} {}", item.title, item.detail, item.keywords).to_lowercase();
            if !words.iter().all(|w| all.contains(w.as_str())) {
                return None;
            }
            let title = item.title.to_lowercase();
            let closeness = if title.starts_with(&text) {
                0
            } else if title.split([' ', ':', '-', '.']).any(|w| w.starts_with(words[0].as_str())) {
                1
            } else {
                2
            };
            let group = groups.iter().position(|g| *g == item.group.as_ref()).unwrap_or(0);
            Some((group, closeness, index))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, _, index)| index).collect()
}

/// What the palette lists.
pub enum Mode {
    /// Projects; with `>` the commands. `commands`: opened on the commands (Ctrl+Shift+P).
    Quick { projects: Vec<PaletteItem>, commands: Vec<PaletteItem> },
    /// A choice to make, with its prompt in the box.
    Pick { prompt: SharedString, items: Vec<PaletteItem> },
    /// A checklist that stays open (Customize Layout): its title row with ↺, and the list read again after each
    /// choice (`ShowSettings`).
    Settings {
        title: SharedString,
        prompt: SharedString,
        refresh: Rc<dyn Fn(&mut App) -> Vec<PaletteItem>>,
        reset: Rc<dyn Fn(&mut App)>,
        items: Vec<PaletteItem>,
    },
}

/// The palette asks to be closed.
pub struct PaletteClosed;

pub struct Palette {
    /// `layout.quickInputPosition` "center": in the middle of the window, not at its top.
    pub centered: bool,
    mode: Mode,
    query: Entity<TextInput>,
    text: String,
    selected: usize,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<PaletteClosed> for Palette {}

impl Palette {
    pub fn new(mode: Mode, start_with_commands: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextInput::new("", cx));
        if start_with_commands {
            query.update(cx, |q, cx| q.set_text(">", cx));
        }
        let subscription = cx.subscribe(&query, |this, _, e: &TextChanged, cx| {
            this.text = e.0.to_string();
            this.selected = 0;
            cx.notify();
        });
        let handle = query.read(cx).focus_handle(cx);
        window.focus(&handle);
        let text = if start_with_commands { ">".to_string() } else { String::new() };
        Self { centered: false, mode, query, text, selected: 0, focus: cx.focus_handle(), _subscription: subscription }
    }

    /// The source list, the text to rank by, and the box's placeholder.
    fn source(&self) -> (&[PaletteItem], &str, SharedString) {
        match &self.mode {
            Mode::Pick { prompt, items } | Mode::Settings { prompt, items, .. } => {
                (items, self.text.as_str(), prompt.clone())
            }
            Mode::Quick { commands, .. } if self.text.starts_with('>') => {
                (commands, &self.text[1..], "Commands".into())
            }
            Mode::Quick { projects, .. } => {
                (projects, self.text.as_str(), "Go to project - type > for the commands".into())
            }
        }
    }

    fn shown(&self) -> Vec<PaletteItem> {
        let (items, text, _) = self.source();
        rank(items, text).into_iter().map(|i| items[i].clone()).collect()
    }

    fn step(&mut self, by: isize, wrap: bool, cx: &mut Context<Self>) {
        let count = self.shown().len() as isize;
        if count == 0 {
            return;
        }
        let next = self.selected as isize + by;
        self.selected = if wrap && (next < 0 || next >= count) {
            next.rem_euclid(count) as usize
        } else {
            next.clamp(0, count - 1) as usize
        };
        cx.notify();
    }

    fn run(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.shown().into_iter().nth(index) else { return };
        if let Mode::Settings { refresh, .. } = &self.mode {
            // Kept open: the choice is made, the list shows it, on the same row.
            let refresh = refresh.clone();
            (item.action)(window, cx);
            if let Mode::Settings { items, .. } = &mut self.mode {
                *items = refresh(cx);
            }
            self.selected = index;
            cx.notify();
            return;
        }
        // Closed first: what the item opens (another pick, a dialog) can take the keyboard from there.
        cx.emit(PaletteClosed);
        let action = item.action.clone();
        window.defer(cx, move |window, cx| action(window, cx));
    }
}

impl Focusable for Palette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let top = if self.centered { (window.viewport_size().height / 2.0 - px(240.0)).max(px(44.0)) } else { px(1.0) };
        let c = theme.colors;
        let (_, _, placeholder) = self.source();
        let shown = self.shown();
        let empty = self.text.trim_start_matches('>').is_empty();
        self.query
            .update(cx, |q, _| q.set_placeholder(if empty { placeholder.clone() } else { SharedString::default() }));
        let mut list = div().id("palette-results").flex().flex_col().max_h(px(440.0)).overflow_y_scroll();
        if shown.is_empty() {
            list = list.child(
                div()
                    .pad(px(0.0), px(8.0))
                    .min_h(px(22.0))
                    .flex()
                    .items_center()
                    .text_color(c.text_primary)
                    .child("No matching results"),
            );
        }
        let mut group: Option<SharedString> = None;
        for (i, item) in shown.iter().enumerate() {
            let starts_group = !item.group.is_empty() && group.as_ref() != Some(&item.group);
            let separator = starts_group && group.is_some();
            group = Some(item.group.clone());
            let side = if starts_group { item.group.clone() } else { item.detail.clone() };
            let selected = i == self.selected;
            let separator_line = div().pad(px(3.0), px(0.0)).child(div().h(px(1.0)).bg(c.picker_group_border));
            list = list.when(separator, |d| d.child(separator_line)).child(
                div()
                    .id(SharedString::from(format!("palette-{i}")))
                    .flex()
                    .items_center()
                    .min_h(px(22.0))
                    .gap(px(8.0))
                    .pad(px(0.0), px(8.0))
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(c.selection_bg))
                    .when(!selected, |d| d.hover(move |s| s.bg(c.hover_bg)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.run(i, window, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_none()
                            .w(px(16.0))
                            .when(item.checked, |d| d.child(ui::icon(icons::CHECK, 11.0, c.text_primary, cx))),
                    )
                    .child(
                        // The title, its group or detail and its shortcut, 12 px apart.
                        div()
                            .flex()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(item.title.clone()),
                            )
                            .when(!side.is_empty(), |d| {
                                d.child(
                                    div()
                                        .max_w(px(260.0))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(theme.text.caption)
                                        .text_color(c.text_muted)
                                        .child(side),
                                )
                            })
                            .when(!item.shortcut.is_empty(), |d| {
                                d.child(
                                    div()
                                        .pad(px(0.0), px(5.0))
                                        .rounded(px(3.0))
                                        .bg(c.hover_bg)
                                        .text_size(theme.text.small)
                                        .text_color(c.text_muted)
                                        .child(item.shortcut.clone()),
                                )
                            }),
                    ),
            );
        }
        div()
            .id("palette-overlay")
            .key_context("Palette")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pad(top, px(16.0))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.step(-1, true, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.step(10, false, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.step(-10, false, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                let index = this.selected;
                this.run(index, window, cx)
            }))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(PaletteClosed)))
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PaletteClosed)))
            .child(
                div()
                    .id("palette-box")
                    .w_full()
                    .max_w(px(600.0))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .p(px(6.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(c.card_border)
                    .bg(c.card_bg)
                    .shadow_xl()
                    .occlude()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .text_color(c.text_primary)
                    .when_some(
                        match &self.mode {
                            Mode::Settings { title, reset, refresh, .. } => {
                                Some((title.clone(), reset.clone(), refresh.clone()))
                            }
                            _ => None,
                        },
                        |d, (title, reset, refresh)| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(div().w(px(44.0)))
                                    .child(div().text_size(theme.text.caption).child(title))
                                    .child(
                                        div()
                                            .flex()
                                            .child(
                                                ui::icon_button("palette-reset", "\u{E7A7}", c.text_primary, true, cx)
                                                    .size(px(22.0))
                                                    .tooltip(ui::Tooltip::text("Reset"))
                                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                        reset(cx);
                                                        if let Mode::Settings { items, .. } = &mut this.mode {
                                                            *items = refresh(cx);
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                ui::icon_button("palette-close", icons::CLOSE, c.text_primary, true, cx)
                                                    .size(px(22.0))
                                                    .tooltip(ui::Tooltip::text("Close"))
                                                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PaletteClosed))),
                                            ),
                                    ),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(theme.metrics.search_height)
                            .pad(px(0.0), px(6.0))
                            .rounded(theme.metrics.control_radius)
                            .border_1()
                            .border_color(c.focus_border)
                            .bg(c.input_bg)
                            .child(self.query.clone()),
                    )
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, group: &str) -> PaletteItem {
        PaletteItem::new(title.to_string(), |_, _| {}).group(group.to_string())
    }

    #[test]
    fn ranks_by_group_then_closeness_then_order() {
        let items = vec![
            item("Project: Start project", "recently used"),
            item("Project: Stop project", "other commands"),
            item("Layout: Show or hide the sidebar", "other commands"),
            item("Project: Restart project", "other commands"),
            item("Start IIS", "other commands"),
        ];
        // "start": the recently used group first; then the title starting with it before the word match.
        assert_eq!(rank(&items, "start"), vec![0, 4, 3]);
        assert_eq!(rank(&items, ""), vec![0, 1, 2, 3, 4]);
        assert_eq!(rank(&items, "hide side"), vec![2]);
        assert!(rank(&items, "nothing").is_empty());
    }
}
