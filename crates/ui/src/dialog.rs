//! What every dialog shares (`Controls/*Dialog.xaml`, `Themes/Controls/InputStyles.xaml`): the frame - its title, the
//! content with a 20 px margin, the button bar under a line with the buttons on the right - and the form pieces: a
//! field's label, a text field, a check box and a radio button with their text, and `FieldError`'s red line.
//!
//! A dialog is a GPUI view that emits [`DialogEvent`]; the app shows it over the window (dimmed behind, Esc cancels)
//! and acts on what it returns.

use crate::px;
use crate::{Icon, IconSize};
use std::rc::Rc;

use gpui::{App, ClickEvent, Div, ElementId, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, MouseButton, MouseDownEvent, SharedString, Stateful, Window, actions, anchored, canvas, deferred, div, prelude::*};

use crate::text_input::TextInput;
use crate::{Pad, Tip};
use crate::theme::Theme;
use crate::{ButtonKind, button, icon_sized, icons};

/// How a dialog ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogEvent<T> {
    /// OK, with what was chosen.
    Done(T),
    /// Cancel, Esc or ✕.
    Cancelled,
}

/// The dialog's frame: `title` on top, `body`, then the button bar (`buttons`, right-aligned). `width` as the XAML
/// window's `Width` (480 for most).
pub fn frame(
    title: impl Into<SharedString>,
    width: f32,
    body: impl IntoElement,
    buttons: impl IntoElement,
    cx: &App,
) -> Div {
    frame_with_tip(title, None::<SharedString>, width, body, buttons, cx)
}

/// [`frame`] with the dialog's explanation as an info tip beside its title.
pub fn frame_with_tip(
    title: impl Into<SharedString>,
    tip: Option<impl Into<SharedString>>,
    width: f32,
    body: impl IntoElement,
    buttons: impl IntoElement,
    cx: &App,
) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    div()
        .w(px(width))
        .max_h(px(680.0))
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .border_1()
        .border_color(c.card_border)
        .bg(c.window_bg)
        .shadow_xl()
        .overflow_hidden()
        .text_size(theme.text.body)
        .text_color(c.text_primary)
        .font_family(theme.font.clone())
        .child(
            // The title and the body: 17 above the title, 12 between them, 17 under the body.
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .gap(px(12.0))
                .pad(px(17.0), px(20.0))
                .child({
                    let title = div().text_size(theme.text.heading).font_weight(FontWeight::SEMIBOLD).child(title.into());
                    match tip {
                        Some(tip) => crate::with_tip(title, "dialog-title-tip", tip, cx),
                        None => div().flex().child(title),
                    }
                })
                .child(div().id("dialog-body").flex_1().min_h_0().overflow_y_scroll().child(body)),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.0))
                .pad(px(12.0), px(20.0))
                .border_t_1()
                .border_color(c.card_border)
                .bg(c.card_bg)
                .child(buttons),
        )
}

/// The OK and Cancel buttons of a form: OK primary (or red when it deletes or replaces), disabled while the form has a
/// problem.
pub fn ok_cancel(
    ok: impl Into<SharedString>,
    ok_enabled: bool,
    danger: bool,
    on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let kind = if danger { ButtonKind::Danger } else { ButtonKind::Primary };
    div()
        .flex()
        .gap(px(8.0))
        .child(button("dialog-ok", ok, kind, ok_enabled, cx).when(ok_enabled, |b| b.on_click(on_ok)))
        .child(button("dialog-cancel", "Cancel", ButtonKind::Secondary, true, cx).on_click(on_cancel))
}

/// A field's label, above it (`FieldLabel`). It carries no spacing: the form's flex column puts 4 px between the
/// label and its field and 10 px above the label.
pub fn label(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_color(Theme::get(cx).colors.text_primary).child(text.into())
}

/// A hint under a field, muted and small. No spacing of its own: 4 px above it is the form's gap.
pub fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = Theme::get(cx);
    div().text_size(theme.text.hint).text_color(theme.colors.text_muted).child(text.into())
}

/// `FieldError`: what is wrong with the form, in red with the error glyph - nothing when there is no problem. No
/// spacing of its own: 10 px above it is the form's gap.
pub fn field_error(problem: Option<SharedString>, cx: &App) -> Option<Div> {
    let c = Theme::get(cx).colors;
    problem.map(|p| {
        div()
            .flex()
            .items_start()
            .gap(px(icons::LABEL_GAP))
            .text_color(c.error_text)
            .child(crate::line_icon(Icon::Error, IconSize::Medium, c.error_text, cx))
            .child(p)
    })
}

/// A text field (`TextBox`): the theme's border, the focus colour while it has the keyboard.
pub fn text_field(input: &Entity<TextInput>, window: &Window, cx: &App) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    div()
        .flex()
        .items_center()
        .h(theme.metrics.control_height)
        .pad(px(0.0), px(6.0))
        .rounded(theme.metrics.control_radius)
        .border_1()
        .border_color(if focused { c.focus_border } else { c.field_border })
        .bg(c.input_bg)
        .child(input.clone())
}

/// A check box with its text (`CheckBox`), clickable on both; Tab reaches it and Space ticks it.
pub fn check(id: impl Into<ElementId>, checked: bool, text: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    crate::focusable_bare(div().id(id), cx)
        .rounded(theme.metrics.control_radius)
        .flex()
        .items_center()
        .gap(px(8.0))
        .pad(px(4.0), px(0.0))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(px(16.0))
                .rounded(theme.metrics.small_radius)
                .border_1()
                .when(checked, |d| {
                    d.bg(c.accent).border_color(c.accent).child(icon_sized(Icon::Check, IconSize::Small, c.on_accent, cx))
                })
                .when(!checked, |d| d.bg(c.input_bg).border_color(c.input_border)),
        )
        .child(text.into())
}

/// A radio button with its text (`RadioButton`); Tab reaches it and Space chooses it.
pub fn radio(id: impl Into<ElementId>, chosen: bool, text: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    crate::focusable_bare(div().id(id), cx)
        .rounded(theme.metrics.control_radius)
        .flex()
        .items_center()
        .gap(px(8.0))
        .pad(px(4.0), px(0.0))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(px(16.0))
                .rounded_full()
                .border_1()
                .border_color(if chosen { c.accent } else { c.input_border })
                .bg(c.input_bg)
                .when(chosen, |d| d.child(div().size(px(8.0)).rounded_full().bg(c.accent))),
        )
        .child(text.into())
}

actions!(select, [SelectNext, SelectPrevious, SelectFirst, SelectLast, SelectPageDown, SelectPageUp, SelectConfirm, SelectClose]);

/// The keys of a [`select`] that has the focus, in its "Select" context. That context is deeper than a dialog's
/// "Modal", so Esc closes an open list before it closes the dialog (and goes on to the dialog when the list is shut).
pub fn bind_keys(cx: &mut App) {
    let context = Some("Select");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("home", SelectFirst, context),
        KeyBinding::new("end", SelectLast, context),
        KeyBinding::new("pagedown", SelectPageDown, context),
        KeyBinding::new("pageup", SelectPageUp, context),
        KeyBinding::new("enter", SelectConfirm, context),
        KeyBinding::new("space", SelectConfirm, context),
        KeyBinding::new("escape", SelectClose, context),
    ]);
}

/// A choice from a list (`ComboBox`): the value shown with a chevron; the `items` (each its text and whether it is
/// the value now) open under it while `open`, over whatever comes after it (an overlay, as wide as the box at least,
/// scrolling past 320 px, with a scroll bar that can be dragged).
///
/// The select asks for its list to open or shut through `on_toggle` - a press on the box, a press anywhere outside
/// the box and its list, and the keys - and reports the row chosen (by a press or Enter) through `on_choose`, then
/// asks to shut. The caller keeps `open`. A press on the box gives it the focus; then Down / Up / Enter / Space open
/// the list, Up / Down / Home / End / Page Up / Page Down move the highlight, Enter or Space choose it and Esc shuts.
pub fn select(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    open: bool,
    items: Vec<(SharedString, bool)>,
    on_toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
    on_choose: impl Fn(&usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let rows = items.into_iter().map(|(text, chosen)| SelectRow::option(text, chosen)).collect();
    select_rows(id, value, open, rows, SelectLook::default(), on_toggle, on_choose, cx)
}

/// One row of a select's list: an option to choose, or a heading over the options after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectRow {
    /// An option: its text, whether it is the value now, and a tip shown over it under the mouse.
    Option { text: SharedString, chosen: bool, tip: Option<SharedString> },
    /// A group's heading: muted and bold, skipped by the keys and the mouse.
    Heading(SharedString),
}

impl SelectRow {
    /// An option without a tip.
    pub fn option(text: impl Into<SharedString>, chosen: bool) -> Self {
        SelectRow::Option { text: text.into(), chosen, tip: None }
    }
}

/// How a select looks apart from its rows: the dialogs' size, or the smaller one of a tab's header (the Logs
/// pickers); and a tip over the box while its list is shut.
#[derive(Debug, Clone, Default)]
pub struct SelectLook {
    /// 24 px high with caption text (box and list) instead of the theme's control height and body text.
    pub compact: bool,
    /// Shown over the box under the mouse; left out while the list is open (it would cover the list).
    pub tip: Option<SharedString>,
}

/// The height of a [`SelectLook::compact`] select.
const COMPACT_HEIGHT: f32 = 24.0;

/// [`select`] with headings among its rows, tips on its options and a [`SelectLook`]. `on_choose` gets the option's
/// place among the options - headings don't count - so the keys, the mouse and the caller agree on which is which.
/// Options under headings are indented.
#[allow(clippy::too_many_arguments)]
pub fn select_rows(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    open: bool,
    rows: Vec<SelectRow>,
    look: SelectLook,
    on_toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
    on_choose: impl Fn(&usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let id: ElementId = id.into();
    let key = SharedString::from(id.to_string());
    let row_of = option_rows(&rows);
    let count = row_of.len();
    let chosen = chosen_option(&rows);
    let grouped = rows.iter().any(|r| matches!(r, SelectRow::Heading(_)));
    let (height, text_size) =
        if look.compact { (px(COMPACT_HEIGHT), theme.text.caption) } else { (theme.metrics.control_height, theme.text.body) };
    let (scroll, focus, highlight, placed) = SELECTS.with(|s| {
        let mut s = s.borrow_mut();
        let state = s.entry(key.to_string()).or_insert_with(|| SelectState::new(cx));
        state.row_of = row_of;
        state.follow(open, chosen);
        (state.scroll.clone(), state.focus.clone(), state.highlight, state.place)
    });
    let on_toggle: Handler<bool> = Rc::new(on_toggle);
    let on_choose: Handler<usize> = Rc::new(on_choose);

    // Up, Down and the rest: open a shut list; in an open one, move the highlight.
    let nav = |to: Move| {
        let (key, toggle) = (key.clone(), on_toggle.clone());
        move |window: &mut Window, cx: &mut App| {
            if open {
                move_highlight(&key, count, to, window);
            } else {
                toggle(&true, window, cx);
            }
        }
    };
    let next = nav(Move::Next);
    let previous = nav(Move::Previous);
    let first = nav(Move::First);
    let last = nav(Move::Last);
    let page_down = nav(Move::PageDown);
    let page_up = nav(Move::PageUp);
    let confirm = {
        let (key, toggle, choose) = (key.clone(), on_toggle.clone(), on_choose.clone());
        move |_: &SelectConfirm, window: &mut Window, cx: &mut App| {
            if !open {
                return toggle(&true, window, cx);
            }
            if let Some(i) = SELECTS.with(|s| s.borrow().get(key.as_ref()).and_then(|s| s.highlight)) {
                choose(&i, window, cx);
            }
            toggle(&false, window, cx);
        }
    };
    let close = {
        let toggle = on_toggle.clone();
        move |_: &SelectClose, window: &mut Window, cx: &mut App| {
            if open {
                toggle(&false, window, cx);
            } else {
                // A shut list: Esc is the dialog's.
                cx.propagate();
            }
        }
    };
    let press = {
        let (toggle, focus) = (on_toggle.clone(), focus.clone());
        move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            window.focus(&focus);
            toggle(&!open, window, cx);
            // The press is the box's: a view's own "pressed elsewhere, close it" mustn't shut what it just opened.
            cx.stop_propagation();
        }
    };
    let press_out = {
        let (key, toggle) = (key.clone(), on_toggle.clone());
        move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            if !open {
                return;
            }
            let list = SELECTS.with(|s| s.borrow().get(key.as_ref()).and_then(|s| s.place).and_then(|p| p.list));
            if !list.is_some_and(|list| list.contains(&window.mouse_position())) {
                toggle(&false, window, cx);
            }
        }
    };

    let measured = key.clone();
    div()
        .relative()
        .child(
            div()
                .id(id)
                .key_context("Select")
                .track_focus(&focus)
                .relative()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .h(height)
                .pad(px(0.0), px(6.0))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(if open { c.focus_border } else { c.field_border })
                .focus(move |s| s.border_color(c.focus_border))
                .bg(c.input_bg)
                .text_size(text_size)
                .cursor_pointer()
                .when_some(look.tip.filter(|_| !open), |d, tip| d.tip(tip))
                .on_mouse_down(MouseButton::Left, press)
                .on_mouse_down_out(press_out)
                .on_action(move |_: &SelectNext, window, cx| next(window, cx))
                .on_action(move |_: &SelectPrevious, window, cx| previous(window, cx))
                .on_action(move |_: &SelectFirst, window, cx| first(window, cx))
                .on_action(move |_: &SelectLast, window, cx| last(window, cx))
                .on_action(move |_: &SelectPageDown, window, cx| page_down(window, cx))
                .on_action(move |_: &SelectPageUp, window, cx| page_up(window, cx))
                .on_action(confirm)
                .on_action(close)
                // Where it is and how tall the window is, for the list's place and width.
                .child(
                    canvas(
                        move |bounds, window, _| {
                            let window_height = window.viewport_size().height;
                            SELECTS.with(|s| {
                                if let Some(state) = s.borrow_mut().get_mut(measured.as_ref()) {
                                    let list = state.place.and_then(|p| p.list);
                                    state.place = Some(Place { bounds, window_height, list });
                                }
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(crate::one_line(value.into()))
                .child(icon_sized(Icon::ChevronDown, IconSize::Small, c.text_primary, cx)),
        )
        .when(open, |d| {
            let mut option = 0;
            let rows = rows.into_iter().map(|row| {
                let (text, chosen, tip) = match row {
                    SelectRow::Heading(text) => {
                        return div()
                            .pad(px(4.0), px(8.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.text_muted)
                            .child(text)
                            .into_any_element();
                    }
                    SelectRow::Option { text, chosen, tip } => (text, chosen, tip),
                };
                let i = option;
                option += 1;
                let lit = highlight == Some(i);
                let (choose, toggle, hover_key) = (on_choose.clone(), on_toggle.clone(), key.clone());
                div()
                    .id((SharedString::from(format!("{key}-option")), i))
                    .flex()
                    .pad(px(3.0), px(if grouped { 20.0 } else { 8.0 }))
                    .cursor_pointer()
                    .when_some(tip, |d, tip| d.tip(tip))
                    .when(chosen, |d| d.bg(c.selection_bg))
                    // The highlight - the row under the mouse or picked with the keys - over the chosen row as well.
                    .when(lit, |d| d.bg(if chosen { c.selection_bg.blend(c.command_center_hover_bg) } else { c.hover_bg }))
                    .on_mouse_move(move |_, window, _| {
                        let moved = SELECTS.with(|s| {
                            let mut s = s.borrow_mut();
                            let Some(state) = s.get_mut(hover_key.as_ref()) else { return false };
                            let moved = state.highlight != Some(i);
                            state.highlight = Some(i);
                            moved
                        });
                        if moved {
                            window.refresh();
                        }
                    })
                    // On the press: a view may close its lists on a press elsewhere, before a click could arrive.
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        choose(&i, window, cx);
                        toggle(&false, window, cx);
                    })
                    .child(div().flex_1().min_w_0().child(text))
                    // Clear of the scroll bar over the right edge.
                    .child(div().flex_none().w(px(crate::SCROLLBAR_WIDTH - 4.0)))
                    .into_any_element()
            });
            // The frame (border, shadow) around the scrolled options and the scroll bar over their right edge.
            let scrolled = div()
                .id(SharedString::from(format!("{key}-list")))
                .max_h(px(320.0))
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .pad(px(4.0), px(0.0))
                .children(rows);
            let list_key = key.clone();
            let list = div()
                .relative()
                .occlude()
                .min_w(placed.map_or(px(220.0), |p| p.bounds.size.width))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(c.card_border)
                .bg(c.card_bg)
                .shadow_lg()
                .text_size(text_size)
                .text_color(c.text_primary)
                .child(scrolled)
                .child(crate::scrollbar(&scroll, cx))
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let changed = SELECTS.with(|s| {
                                let mut s = s.borrow_mut();
                                let Some(place) = s.get_mut(list_key.as_ref()).and_then(|s| s.place.as_mut()) else {
                                    return false;
                                };
                                let changed = place.list.map(|l| l.size.height) != Some(bounds.size.height);
                                place.list = Some(bounds);
                                changed
                            });
                            // Placed again with its height (above the box when it doesn't fit below) once this frame
                            // is done - a refresh asked for while drawing is dropped.
                            if changed {
                                let handle = window.window_handle();
                                cx.defer(move |cx| {
                                    handle.update(cx, |_, window, _| window.refresh()).ok();
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );
            // Under the box; above it when the window has no room below and more above.
            let anchored = match placed {
                Some(p) => {
                    let height = p.list.map_or(px(320.0), |l| l.size.height);
                    let below = p.window_height - p.bounds.bottom() - px(2.0);
                    let above = p.bounds.top() - px(2.0);
                    if height > below && above > below {
                        anchored()
                            .anchor(gpui::Corner::BottomLeft)
                            .position(gpui::point(p.bounds.left(), p.bounds.top() - px(2.0)))
                    } else {
                        anchored().position(gpui::point(p.bounds.left(), p.bounds.bottom() + px(2.0)))
                    }
                }
                None => anchored(),
            };
            d.child(deferred(anchored.snap_to_window().child(list)).with_priority(2))
        })
}

/// A select's `on_toggle` or `on_choose`, shared by the box, its keys and its rows.
type Handler<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

/// A key's move of a select's highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Next,
    Previous,
    First,
    Last,
    PageDown,
    PageUp,
}

impl Move {
    /// The option highlighted after this move from `highlight`, among `count` options, `page` of them seen at once.
    /// Nothing highlighted yet: End goes to the last, any other move to the first. It stops at either end (no
    /// wrapping round). No options: nothing.
    fn from(self, highlight: Option<usize>, page: usize, count: usize) -> Option<usize> {
        let last = count.checked_sub(1)?;
        let to = match (self, highlight) {
            (Move::Last, _) => last,
            (Move::First, _) | (_, None) => 0,
            (Move::Next, Some(h)) => h + 1,
            (Move::Previous, Some(h)) => h.saturating_sub(1),
            (Move::PageDown, Some(h)) => h + page,
            (Move::PageUp, Some(h)) => h.saturating_sub(page),
        };
        Some(to.min(last))
    }
}

/// Moves a select's highlight by `to` and scrolls it into view.
fn move_highlight(key: &str, count: usize, to: Move, window: &mut Window) {
    SELECTS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(state) = s.get_mut(key) else { return };
        let seen = state.scroll.bounds().size.height;
        let row = state.scroll.bounds_for_item(0).map_or(px(0.0), |b| b.size.height);
        let page = if row > px(0.0) { ((seen / row).floor() as usize).saturating_sub(1).max(1) } else { 1 };
        if let Some(next) = to.from(state.highlight, page, count) {
            state.highlight = Some(next);
            state.scroll.scroll_to_item(state.row(next));
        }
    });
    window.refresh();
}

/// What a select keeps between frames.
struct SelectState {
    /// Its list's scroll position, kept while the list is open (and its scroll bar drawn from it).
    scroll: gpui::ScrollHandle,
    /// The box's focus, for the keys.
    focus: FocusHandle,
    /// The row the keys or the mouse are on, while the list is open.
    highlight: Option<usize>,
    /// Whether the list was open the last time it was drawn.
    was_open: bool,
    /// Where it was laid out.
    place: Option<Place>,
    /// Each option's row in the list (headings take rows too), for scrolling an option into view.
    row_of: Vec<usize>,
}

impl SelectState {
    fn new(cx: &App) -> Self {
        Self {
            scroll: gpui::ScrollHandle::new(),
            // A tab stop: Tab reaches the box, then Down / Up / Enter / Space open it.
            focus: cx.focus_handle().tab_stop(true),
            highlight: None,
            was_open: false,
            place: None,
            row_of: Vec::new(),
        }
    }

    /// The list row of option `i`.
    fn row(&self, i: usize) -> usize {
        self.row_of.get(i).copied().unwrap_or(i)
    }

    /// Opened: the highlight starts on the chosen option, scrolled into view. Shut: no highlight.
    fn follow(&mut self, open: bool, chosen: Option<usize>) {
        if open && !self.was_open {
            self.highlight = chosen;
            match chosen {
                Some(i) => self.scroll.scroll_to_item(self.row(i)),
                None => self.scroll.set_offset(gpui::point(px(0.0), px(0.0))),
            }
        }
        if !open {
            self.highlight = None;
        }
        self.highlight = self.highlight.filter(|h| *h < self.row_of.len());
        self.was_open = open;
    }
}

/// The list row of each option in `rows`, in order: the options' places with the headings counted.
fn option_rows(rows: &[SelectRow]) -> Vec<usize> {
    rows.iter().enumerate().filter(|(_, r)| matches!(r, SelectRow::Option { .. })).map(|(i, _)| i).collect()
}

/// Which option (among the options, headings not counted) is the value now.
fn chosen_option(rows: &[SelectRow]) -> Option<usize> {
    rows.iter()
        .filter_map(|r| match r {
            SelectRow::Option { chosen, .. } => Some(*chosen),
            SelectRow::Heading(_) => None,
        })
        .position(|chosen| chosen)
}

/// Where a select's box was last laid out, the window's height then, and where its open list was.
#[derive(Clone, Copy)]
struct Place {
    bounds: gpui::Bounds<gpui::Pixels>,
    window_height: gpui::Pixels,
    list: Option<gpui::Bounds<gpui::Pixels>>,
}

thread_local! {
    /// Each select's state, by its id.
    static SELECTS: std::cell::RefCell<std::collections::HashMap<String, SelectState>> = Default::default();
}

#[cfg(test)]
mod tests {
    use super::{Move, SelectRow, chosen_option, option_rows};

    #[test]
    fn up_and_down_move_one_option_and_stop_at_the_ends() {
        assert_eq!(Move::Next.from(Some(0), 5, 3), Some(1));
        assert_eq!(Move::Next.from(Some(2), 5, 3), Some(2));
        assert_eq!(Move::Previous.from(Some(1), 5, 3), Some(0));
        assert_eq!(Move::Previous.from(Some(0), 5, 3), Some(0));
    }

    #[test]
    fn nothing_highlighted_starts_at_the_first_or_with_end_the_last() {
        for to in [Move::Next, Move::Previous, Move::First, Move::PageDown, Move::PageUp] {
            assert_eq!(to.from(None, 5, 3), Some(0), "{to:?}");
        }
        assert_eq!(Move::Last.from(None, 5, 3), Some(2));
    }

    #[test]
    fn home_end_and_the_page_keys() {
        assert_eq!(Move::First.from(Some(7), 5, 10), Some(0));
        assert_eq!(Move::Last.from(Some(1), 5, 10), Some(9));
        assert_eq!(Move::PageDown.from(Some(1), 5, 10), Some(6));
        assert_eq!(Move::PageDown.from(Some(8), 5, 10), Some(9));
        assert_eq!(Move::PageUp.from(Some(6), 5, 10), Some(1));
        assert_eq!(Move::PageUp.from(Some(3), 5, 10), Some(0));
    }

    #[test]
    fn an_empty_list_has_nothing_to_highlight() {
        assert_eq!(Move::Next.from(None, 5, 0), None);
        assert_eq!(Move::Last.from(Some(0), 5, 0), None);
    }

    #[test]
    fn headings_take_a_row_but_are_not_options() {
        let rows = vec![
            SelectRow::Heading("DNN".into()),
            SelectRow::option("Log4Net", false),
            SelectRow::option("Event log", true),
            SelectRow::Heading("IIS".into()),
            SelectRow::option("Requests", false),
        ];
        // Options 0, 1, 2 are rows 1, 2, 4: the keys count options, the list scrolls to rows.
        assert_eq!(option_rows(&rows), vec![1, 2, 4]);
        assert_eq!(chosen_option(&rows), Some(1));
    }

    #[test]
    fn nothing_chosen_without_a_chosen_option() {
        let rows = vec![SelectRow::Heading("Sites".into()), SelectRow::option("a", false)];
        assert_eq!(chosen_option(&rows), None);
        assert!(option_rows(&[SelectRow::Heading("x".into())]).is_empty());
    }
}
