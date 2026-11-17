//! What every dialog shares (`Controls/*Dialog.xaml`, `Themes/Controls/InputStyles.xaml`): the frame - its title, the
//! content with a 20 px margin, the button bar under a line with the buttons on the right - and the form pieces: a
//! field's label, a text field, a check box and a radio button with their text, and `FieldError`'s red line.
//!
//! A dialog is a GPUI view that emits [`DialogEvent`]; the app shows it over the window (dimmed behind, Esc cancels)
//! and acts on what it returns.

use crate::px;
use gpui::{AnyElement, App, ClickEvent, Div, ElementId, Entity, Focusable, FontWeight, SharedString, Stateful, Window, anchored, canvas, deferred, div, prelude::*};

use crate::Pad;
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::{ButtonKind, button, icon, icons};

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
            .child(crate::line_icon(icons::ERROR, icons::SIZE, c.error_text, cx))
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

/// A check box with its text (`CheckBox`), clickable on both.
pub fn check(id: impl Into<ElementId>, checked: bool, text: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    div()
        .id(id)
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
                    d.bg(c.accent).border_color(c.accent).child(icon(icons::CHECK, icons::SIZE_SMALL, c.on_accent, cx))
                })
                .when(!checked, |d| d.bg(c.input_bg).border_color(c.input_border)),
        )
        .child(text.into())
}

/// A radio button with its text (`RadioButton`).
pub fn radio(id: impl Into<ElementId>, chosen: bool, text: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    div()
        .id(id)
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

/// A choice from a list (`ComboBox`): the value shown with a chevron; the options open under it while `open`, over
/// whatever comes after it (an overlay, as wide as the box at least, scrolling past 320 px).
pub fn select(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    open: bool,
    options: Vec<AnyElement>,
    cx: &App,
) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let id: ElementId = id.into();
    let key = id.to_string();
    let placed = SELECT_PLACES.with(|w| w.borrow().get(&key).copied());
    // Its list's scroll position, kept while the list is open (and its scroll bar drawn from it).
    let scroll = SELECT_SCROLLS.with(|s| s.borrow_mut().entry(key.clone()).or_default().clone());
    let measured = key.clone();
    let list_key = key.clone();
    div()
        .relative()
        .child(
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .justify_between()
                .h(theme.metrics.control_height)
                .pad(px(0.0), px(6.0))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(if open { c.focus_border } else { c.field_border })
                .bg(c.input_bg)
                .cursor_pointer()
                // Where it is and how tall the window is, for the list's place and width.
                .child(
                    canvas(
                        move |bounds, window, _| {
                            let place = Place { bounds, window_height: window.viewport_size().height, list_height: None };
                            SELECT_PLACES.with(|w| {
                                let mut w = w.borrow_mut();
                                let list_height = w.get(&measured).and_then(|p| p.list_height);
                                w.insert(measured.clone(), Place { list_height, ..place });
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(crate::one_line(value.into()))
                .child(icon(icons::CHEVRON_DOWN, icons::SIZE_SMALL, c.text_primary, cx)),
        )
        .when(open, |d| {
            // The frame (border, shadow) around the scrolled options and the scroll bar over their right edge.
            let scrolled = div()
                .id(SharedString::from(format!("{key}-list")))
                .relative()
                .max_h(px(320.0))
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .pad(px(4.0), px(0.0))
                .children(options)
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let changed = SELECT_PLACES.with(|w| {
                                let mut places = w.borrow_mut();
                                let Some(p) = places.get_mut(&list_key) else { return false };
                                let changed = p.list_height != Some(bounds.size.height);
                                p.list_height = Some(bounds.size.height);
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
            let list = div()
                .relative()
                .occlude()
                .min_w(placed.map_or(px(220.0), |p| p.bounds.size.width))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(c.card_border)
                .bg(c.card_bg)
                .shadow_lg()
                .text_color(c.text_primary)
                .child(scrolled)
                .child(crate::scrollbar(&scroll, cx));
            // Under the box; above it when the window has no room below and more above.
            let anchored = match placed {
                Some(p) => {
                    let height = p.list_height.unwrap_or(px(320.0));
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

/// Where a select's box was last laid out, the window's height then, and its open list's height.
#[derive(Clone, Copy)]
struct Place {
    bounds: gpui::Bounds<gpui::Pixels>,
    window_height: gpui::Pixels,
    list_height: Option<gpui::Pixels>,
}

thread_local! {
    /// Each select's place, by its id.
    static SELECT_PLACES: std::cell::RefCell<std::collections::HashMap<String, Place>> = Default::default();
    /// Each select's list's scroll handle, by its id.
    static SELECT_SCROLLS: std::cell::RefCell<std::collections::HashMap<String, gpui::ScrollHandle>> = Default::default();
}

/// One option of [`select`].
pub fn option(id: impl Into<ElementId>, text: impl Into<SharedString>, chosen: bool, cx: &App) -> Stateful<Div> {
    let c = Theme::get(cx).colors;
    div()
        .id(id)
        .pad(px(3.0), px(8.0))
        .cursor_pointer()
        .when(chosen, |d| d.bg(c.selection_bg))
        .hover(move |s| s.bg(c.hover_bg))
        .child(text.into())
}
