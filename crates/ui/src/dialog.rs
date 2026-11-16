//! What every dialog shares (`Controls/*Dialog.xaml`, `Themes/Controls/InputStyles.xaml`): the frame - its title, the
//! content with a 20 px margin, the button bar under a line with the buttons on the right - and the form pieces: a
//! field's label, a text field, a check box and a radio button with their text, and `FieldError`'s red line.
//!
//! A dialog is a GPUI view that emits [`DialogEvent`]; the app shows it over the window (dimmed behind, Esc cancels)
//! and acts on what it returns.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, Focusable, FontWeight, SharedString, Stateful, Window, div,
    prelude::*, px,
};

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
                .child(div().text_size(theme.text.heading).font_weight(FontWeight::SEMIBOLD).child(title.into()))
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
            .gap(px(6.0))
            .text_color(c.error_text)
            .child(icon(icons::ERROR, 12.0, c.error_text, cx))
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
                    d.bg(c.accent).border_color(c.accent).child(icon(icons::CHECK, 10.0, c.on_accent, cx))
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

/// A choice from a list (`ComboBox`): the value shown with a chevron; the options open under it while `open`.
pub fn select(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    open: bool,
    options: Vec<AnyElement>,
    cx: &App,
) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    div()
        .relative()
        .child(
            div()
                .id(id)
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
                .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(value.into()))
                .child(icon(icons::CHEVRON_DOWN, 10.0, c.text_primary, cx)),
        )
        .when(open, |d| {
            d.child(
                div()
                    .absolute()
                    .top(theme.metrics.control_height + px(2.0))
                    .left_0()
                    .right_0()
                    .pad(px(4.0), px(0.0))
                    .rounded(theme.metrics.control_radius)
                    .border_1()
                    .border_color(c.card_border)
                    .bg(c.card_bg)
                    .shadow_lg()
                    .children(options),
            )
        })
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
