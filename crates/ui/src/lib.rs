//! DNN Manager's theme (`theme`) and controls, styled as `Themes/Controls/*.xaml` styles them: buttons, icon buttons, link buttons, the
//! toggle switch, the table check box, icons and tooltips. Each takes its sizes and colours from the theme.

pub mod dialog;
pub mod icons;
pub mod menu;
pub mod spacing;
pub mod text_input;
pub mod theme;

use gpui::{
    AnyView, App, ClickEvent, Div, ElementId, FontWeight, Hsla, Pixels, SharedString, Stateful, Window, div,
    prelude::*, px,
};

pub use crate::spacing::Pad;
use crate::theme::Theme;

/// A glyph of the icon font (`Themes/Icons.xaml`), in a box of its size so it lines up with text.
pub fn icon(glyph: &'static str, size: f32, color: Hsla, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(px(size + 2.0))
        .font_family(Theme::get(cx).icon_font.clone())
        .text_size(px(size))
        .line_height(px(size + 2.0))
        .text_color(color)
        .child(glyph)
}

/// A tooltip: the theme's card with its text.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div().pad(px(10.0), px(8.0)).child(
            div()
                .max_w(px(420.0))
                .pad(px(4.0), px(8.0))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(theme.colors.card_border)
                .bg(theme.colors.card_bg)
                .shadow_md()
                .text_size(theme.text.caption)
                .text_color(theme.colors.text_primary)
                .font_family(theme.font.clone())
                .child(self.text.clone()),
        )
    }
}

/// How a button looks: the default (secondary), Primary (accent) or Danger (red - it deletes or stops something).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Secondary,
    Primary,
    Danger,
}

/// A text button (`ButtonStyles.xaml`): 30 px high, 14,6 padding, radius 4.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let (bg, hover, border, fg) = match kind {
        ButtonKind::Secondary => (c.card_bg, c.hover_bg, c.card_border, c.text_primary),
        ButtonKind::Primary => (c.accent, c.accent_hover, c.accent, c.on_accent),
        ButtonKind::Danger => (c.danger, c.danger_hover, c.danger, c.on_accent),
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_h(theme.metrics.control_height)
        .pad(px(6.0), px(14.0))
        .rounded(theme.metrics.control_radius)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_size(theme.text.body)
        .when(kind != ButtonKind::Secondary, |d| d.font_weight(FontWeight::SEMIBOLD))
        .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
        .when(!enabled, |d| d.opacity(if kind == ButtonKind::Secondary { 0.5 } else { 0.45 }))
        .child(label.into())
}

/// An icon button (`IconButton`): 28 × 28, transparent until hovered; a third of its strength when disabled.
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    color: Hsla,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let hover = theme.colors.hover_bg;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.0))
        .rounded(theme.metrics.control_radius)
        .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
        .when(!enabled, |d| d.opacity(0.35))
        .child(icon(glyph, 13.0, color, cx))
}

/// A link (`LinkButton`): LinkFg text, underlined under the mouse.
pub fn link(id: impl Into<ElementId>, text: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    div()
        .id(id)
        .cursor_pointer()
        .text_color(theme.colors.link_fg)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .hover(|s| s.underline())
        .child(text.into())
}

/// The toggle switch (`ToggleSwitch`): a 32 × 18 track, a 12 px knob, the label beside it.
pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    label: impl Into<SharedString>,
    on_toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .text_color(c.text_primary)
        .on_click(move |_: &ClickEvent, window, cx| on_toggle(&!on, window, cx))
        .child(
            div()
                .relative()
                .flex_none()
                .w(px(32.0))
                .h(px(18.0))
                .rounded(px(9.0))
                .bg(if on { c.accent } else { c.input_border })
                .child(
                    div()
                        .absolute()
                        .top(px(3.0))
                        .left(px(if on { 17.0 } else { 3.0 }))
                        .size(px(12.0))
                        .rounded_full()
                        .bg(c.on_accent),
                ),
        )
        .child(label.into())
}

/// A table row's check box (`TableCheckBox`): 16 px, accent filled with a check mark when ticked.
pub fn check_box(id: impl Into<ElementId>, checked: bool, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let focus = c.focus_border;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .rounded(theme.metrics.small_radius)
        .border_1()
        .cursor_pointer()
        .when(checked, |d| d.bg(c.accent).border_color(c.accent))
        .when(!checked, |d| d.bg(c.input_bg).border_color(c.input_border).hover(move |s| s.border_color(focus)))
        .when(checked, |d| d.child(icon(icons::CHECK, 10.0, c.on_accent, cx)))
}

/// A status dot: filled while running, a ring while stopped, amber while changing.
pub fn status_dot(color: Hsla, filled: bool, size: Pixels) -> Div {
    div().flex_none().size(size).rounded_full().border(px(1.5)).border_color(color).when(filled, |d| d.bg(color))
}
