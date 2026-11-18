//! DNN Manager's theme (`theme`) and controls, styled as `Themes/Controls/*.xaml` styles them: buttons, icon buttons, link buttons, the
//! toggle switch, the table check box, icons and tooltips. Each takes its sizes and colours from the theme.

pub mod dialog;
pub mod icons;
pub mod menu;
pub mod shapes;
pub mod scale;
pub mod spacing;
pub mod text_input;
pub mod theme;

use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyView, App, ClickEvent, Div, ElementId, FontWeight, Hsla, Pixels, SharedString, Stateful, Transformation, Window, div, percentage, prelude::*};

pub use crate::scale::{px, set_ui_scale, ui_scale};
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

/// How long a change of state takes on screen - a fade in, a switch's slide. 200 ms with [`MOTION_EASING`]: long
/// enough to be seen (150 ms with a strong ease-out did 80 % of the move in the first 50 ms - it read as a jump), short
/// enough not to wait for.
pub const MOTION: Duration = Duration::from_millis(200);

/// The easing of [`MOTION`]: slow at both ends (GPUI's quadratic ease-in-out).
pub fn motion_easing(t: f32) -> f32 {
    gpui::ease_in_out(t)
}

/// Fades `element` in over [`MOTION`] as it appears (menus, dialogs, the palette) - at once when animations are off.
/// The `id` keeps the fade from starting again while the element stays on screen.
pub fn fade_in<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>, cx: &App) -> gpui::AnyElement {
    if !Theme::get(cx).animations {
        return element.into_any_element();
    }
    element
        .with_animation(
            id,
            Animation::new(MOTION).with_easing(motion_easing),
            |el, t| el.opacity(t),
        )
        .into_any_element()
}

/// Pulses `element` - opacity from 1 to `low` and back every `period` - for something under way; still when
/// animations are off.
pub fn pulse<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    low: f32,
    period: Duration,
    cx: &App,
) -> gpui::AnyElement {
    if !Theme::get(cx).animations {
        return element.into_any_element();
    }
    element
        .with_animation(id, Animation::new(period).repeat(), move |el, t| {
            // Down in the first half, up in the second, eased at both ends.
            let x = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
            let eased = x * x * (3.0 - 2.0 * x);
            el.opacity(1.0 - (1.0 - low) * eased)
        })
        .into_any_element()
}

/// Where a tooltip goes beside what it explains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipPlace {
    /// Under it, centred on it (above it when there's no room below) - most things.
    Below,
    /// Beside it, centred on its height, on the side with room - the activity bar's entries (VS Code's).
    Beside,
}

/// A tooltip: the theme's card with its text. Given its element's [`Anchor`] it lines up with the element - under it
/// and centred, or beside it -, the same for every element; without one it opens by the mouse (GPUI's own place).
pub struct Tooltip {
    text: SharedString,
    at: Option<(Anchor, TipPlace)>,
    /// The card's size, measured as it is first drawn: with it the card is centred on its element.
    size: std::rc::Rc<std::cell::Cell<Option<gpui::Size<Pixels>>>>,
}

impl Tooltip {
    /// By the mouse.
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone(), at: None, size: Default::default() }).into()
    }

    /// Lined up with the element `anchor` measures (see [`Tip`]).
    pub fn at(
        text: impl Into<SharedString>,
        anchor: Anchor,
        place: TipPlace,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| {
            cx.new(|_| Tooltip { text: text.clone(), at: Some((anchor.clone(), place)), size: Default::default() })
                .into()
        }
    }
}

/// Where a `size` card goes for an element at `of` in a `window` this big: 4 px from it, inside the window.
pub fn tip_origin(
    of: gpui::Bounds<Pixels>,
    size: gpui::Size<Pixels>,
    window: gpui::Size<Pixels>,
    place: TipPlace,
) -> gpui::Point<Pixels> {
    const GAP: f32 = 4.0;
    let clamp = |v: Pixels, len: Pixels, max: Pixels| v.min(max - len - px(GAP)).max(px(GAP));
    match place {
        TipPlace::Below => {
            let x = clamp(of.center().x - size.width / 2.0, size.width, window.width);
            let below = of.bottom() + px(GAP);
            let y = if below + size.height <= window.height { below } else { of.top() - px(GAP) - size.height };
            gpui::point(x, y.max(px(0.0)))
        }
        TipPlace::Beside => {
            let right = of.right() + px(GAP);
            let x = if right + size.width <= window.width { right } else { of.left() - px(GAP) - size.width };
            let y = clamp(of.center().y - size.height / 2.0, size.height, window.height);
            gpui::point(x.max(px(0.0)), y)
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let card = div()
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
            .child(self.text.clone());
        let Some(bounds) = self.at.as_ref().and_then(|(anchor, place)| anchor.bounds().map(|b| (b, *place))) else {
            // By the mouse: 10 px below and 8 right of it.
            return div().pad(px(10.0), px(8.0)).child(card).into_any_element();
        };
        let (of, place) = bounds;
        // First drawn out of sight to learn its size, then where it belongs - the next frame, asked for here.
        let size = self.size.clone();
        let measured = size.clone();
        let this = cx.entity().downgrade();
        let card = card.relative().child(
            gpui::canvas(
                move |b, _, cx| {
                    if measured.get() != Some(b.size) {
                        measured.set(Some(b.size));
                        // Drawn again with it, once this frame is done (a refresh asked for while drawing is dropped).
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
        // The first frame, its size not known yet, it is laid out but not seen: `anchored` keeps whatever it holds
        // inside the window, so a place off-screen showed it at the top left for a frame.
        let (origin, card) = match size.get() {
            Some(size) => (tip_origin(of, size, window.viewport_size(), place), card),
            None => (gpui::point(px(0.0), px(0.0)), card.invisible()),
        };
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(origin)
            .child(card)
            .into_any_element()
    }
}

/// An element's tooltip lined up with it ([`Tooltip::at`]): `.tip(text)` under it, `.tip_beside(text)` beside it.
pub trait Tip: ParentElement + StatefulInteractiveElement + Sized {
    fn tip(self, text: impl Into<SharedString>) -> Self {
        self.tip_at(text, TipPlace::Below)
    }

    fn tip_beside(self, text: impl Into<SharedString>) -> Self {
        self.tip_at(text, TipPlace::Beside)
    }

    fn tip_at(self, text: impl Into<SharedString>, place: TipPlace) -> Self {
        let anchor = Anchor::default();
        self.child(anchor.measure()).tooltip(Tooltip::at(text, anchor, place))
    }
}

impl<E: ParentElement + StatefulInteractiveElement + Sized> Tip for E {}

/// Where an element was last drawn, in window coordinates - a button, so what it opens (a menu, a list, a popup)
/// lines up with it instead of with the mouse. Cheap to clone: every clone shares the place.
#[derive(Clone, Default)]
pub struct Anchor(std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>);

impl Anchor {
    /// Put as a child of the element to follow: an invisible layer over it that records its bounds as it is drawn.
    pub fn measure(&self) -> impl IntoElement {
        let cell = self.0.clone();
        gpui::canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
    }

    pub fn bounds(&self) -> Option<gpui::Bounds<Pixels>> {
        self.0.get()
    }

    /// Where something it opens goes: under it, its left edges level (`Corner::TopLeft`), 4 px apart - or, with
    /// `right`, their right edges level (for a button at the right of the window).
    pub fn below(&self, right: bool) -> (gpui::Point<Pixels>, gpui::Corner) {
        match self.bounds() {
            Some(b) if right => (gpui::point(b.right(), b.bottom() + px(4.0)), gpui::Corner::TopRight),
            Some(b) => (gpui::point(b.left(), b.bottom() + px(4.0)), gpui::Corner::TopLeft),
            None => (gpui::point(px(0.0), px(0.0)), gpui::Corner::TopLeft),
        }
    }

    /// Whether `position` is on it - a press on the button that opened something isn't a press outside it.
    pub fn contains(&self, position: gpui::Point<Pixels>) -> bool {
        self.bounds().is_some_and(|b| b.contains(&position))
    }
}

/// A drawn icon of [`shapes`] (a layout icon, maximize) in the box a glyph of its size gets, so it lines up the same.
pub fn shape_icon(path: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size + 2.0))
        .child(gpui::svg().path(path.into()).size(px(size)).text_color(color))
}

/// Something under way (`StepSpinner`): three quarters of a ring, turning once a second - still while animations are
/// off. In the box a glyph of its size gets.
pub fn spinner(id: impl Into<ElementId>, size: f32, color: Hsla, cx: &App) -> gpui::AnyElement {
    let ring = gpui::svg().path(shapes::SPINNER_PATH).size(px(size)).text_color(color);
    let ring = if Theme::get(cx).animations {
        ring.with_animation(id, Animation::new(Duration::from_secs(1)).repeat(), |ring, t| {
            ring.with_transformation(Transformation::rotate(percentage(t)))
        })
        .into_any_element()
    } else {
        ring.into_any_element()
    };
    div().flex().flex_none().items_center().justify_center().size(px(size + 2.0)).child(ring).into_any_element()
}

/// One line's find matches (the panel's Find bar): byte ranges, each with whether it is the current match.
pub type LineMarks = Vec<(std::ops::Range<usize>, bool)>;

/// The colours of find matches: every match, and the current one (`FindMatchBg`, `FindCurrentBg`).
pub fn find_colors(cx: &App) -> (Hsla, Hsla) {
    let c = Theme::get(cx).colors;
    (c.log_warn.opacity(0.30), c.log_warn.opacity(0.75))
}

/// A line of text with its find matches highlighted.
pub fn marked_text(text: impl Into<SharedString>, marks: &LineMarks, cx: &App) -> gpui::StyledText {
    let (other, current) = find_colors(cx);
    let highlights = marks.iter().map(|(range, is_current)| {
        let background_color = Some(if *is_current { current } else { other });
        (range.clone(), gpui::HighlightStyle { background_color, ..Default::default() })
    });
    gpui::StyledText::new(text.into()).with_highlights(highlights.collect::<Vec<_>>())
}

/// An icon in front of text that may wrap (an error, a check's result, a toast): one line of that text high, so the
/// icon centres on the first line whatever the font size. Use it in an `items_start` row, `LABEL_GAP` from the text.
pub fn line_icon(glyph: &'static str, size: f32, color: Hsla, cx: &App) -> Div {
    div()
        .relative()
        .flex_none()
        .w(px(size + 2.0))
        // An invisible character: the box takes the height of one line of the text around it.
        .child("\u{200B}")
        .child(div().absolute().inset_0().flex().items_center().justify_center().child(icon(glyph, size, color, cx)))
}

/// An explanation kept out of the way: a small muted info icon beside a title or label, its text in a tooltip.
pub fn info_tip(id: impl Into<ElementId>, text: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    info_tip_lowered(id, text, px(1.0), cx)
}

/// How far an [`info_tip`] sits below the middle of the line it is centred on, beside text this big: about a tenth of
/// it. Centred on the line, the ⓘ lines up with the capitals; most of a word is lower case, so it looked high - this
/// puts it in the middle of the letters as they're seen (1 px beside a label or heading, 2 beside a page's title).
pub fn info_tip_drop(text_size: Pixels) -> Pixels {
    (text_size * 0.09).round().max(px(1.0))
}

fn info_tip_lowered(id: impl Into<ElementId>, text: impl Into<SharedString>, drop: Pixels, cx: &App) -> Stateful<Div> {
    let theme = Theme::get(cx);
    let (muted, hover) = (theme.colors.text_muted, theme.colors.text_primary);
    div()
        .id(id)
        .flex_none()
        .relative()
        .top(drop)
        .text_color(muted)
        .hover(move |s| s.text_color(hover))
        .tip(text)
        // Beside a check box or radio its click is its own: it doesn't tick or choose the row.
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(icons::SIZE + 2.0))
                .font_family(theme.icon_font.clone())
                .text_size(px(icons::SIZE))
                .line_height(px(icons::SIZE + 2.0))
                .child(icons::INFO),
        )
}

/// How wide a [`scrollbar`]'s track is: the strip at the right of a list where its thumb runs and can be grabbed.
pub const SCROLLBAR_WIDTH: f32 = 12.0;

/// A vertical scroll bar for the scroll area `handle` tracks: a 6 px thumb at the right (8 px while the mouse is on
/// it or it is dragged), as tall as the share that is seen, where the view is - none while everything fits. Drag the
/// thumb to scroll; a press beside it jumps there and drags from the middle of the thumb. Put it as a sibling after the
/// scrolled element inside a `relative()` box. GPUI 0.2.2 has no scroll bar of its own; this draws in the paint phase,
/// reading the handle after the scrolled element was laid out, so the thumb is where the list is this frame (not the
/// last).
pub fn scrollbar(handle: &gpui::ScrollHandle, cx: &App) -> impl IntoElement {
    let handle = handle.clone();
    let colors = Theme::get(cx).colors;
    let (rest, active) = (colors.scroll_thumb, colors.scroll_thumb_hover);
    gpui::canvas(
        {
            let handle = handle.clone();
            // A hitbox only while there is something to scroll: the strip takes the presses over the list's right edge
            // (the wheel still scrolls the list under it).
            move |track, window, _| {
                (handle.max_offset().height > px(0.0))
                    .then(|| window.insert_hitbox(track, gpui::HitboxBehavior::BlockMouseExceptScroll))
            }
        },
        move |track, hitbox, window, _| {
            let Some(hitbox) = hitbox else { return };
            let seen = handle.bounds().size.height;
            let hidden = handle.max_offset().height;
            if hidden <= px(0.0) || seen <= px(0.0) {
                return;
            }
            let length = (track.size.height * (seen / (seen + hidden))).max(px(20.0)).min(track.size.height);
            let along = (-handle.offset().y / hidden).clamp(0.0, 1.0);
            let top = track.top() + (track.size.height - length) * along;
            let dragging = THUMB_DRAG.with(|d| d.borrow().as_ref().is_some_and(|d| d.track == track));
            let hovered = hitbox.is_hovered(window);
            let width = if hovered || dragging { px(8.0) } else { px(6.0) };
            let thumb = gpui::Bounds::new(gpui::point(track.right() - width - px(2.0), top), gpui::size(width, length));
            window.paint_quad(gpui::fill(thumb, if hovered || dragging { active } else { rest }).corner_radii(width / 2.0));
            window.set_cursor_style(gpui::CursorStyle::Arrow, &hitbox);

            // Drawn again when the mouse comes onto the strip or leaves it.
            window.on_mouse_event({
                let hitbox = hitbox.clone();
                move |_: &gpui::MouseMoveEvent, phase, window, _| {
                    if phase == gpui::DispatchPhase::Bubble && hitbox.is_hovered(window) != hovered {
                        window.refresh();
                    }
                }
            });
            window.on_mouse_event({
                let handle = handle.clone();
                move |e: &gpui::MouseDownEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Bubble || e.button != gpui::MouseButton::Left || !hitbox.is_hovered(window) {
                        return;
                    }
                    let on_thumb = e.position.y >= thumb.top() && e.position.y <= thumb.bottom();
                    let grab = if on_thumb { e.position.y - thumb.top() } else { length / 2.0 };
                    let drag = ThumbDrag { handle: handle.clone(), grab, track, length };
                    drag.move_to(e.position.y);
                    THUMB_DRAG.with(|d| *d.borrow_mut() = Some(drag));
                    window.refresh();
                    // The press is the scroll bar's - not a choice of the row under it, nor a press outside a popup.
                    cx.stop_propagation();
                }
            });
            if dragging {
                // Anywhere in the window while the button is held, so the thumb follows a mouse that leaves the strip.
                window.on_mouse_event(|e: &gpui::MouseMoveEvent, phase, window, _| {
                    if phase != gpui::DispatchPhase::Capture {
                        return;
                    }
                    let held = e.pressed_button == Some(gpui::MouseButton::Left);
                    THUMB_DRAG.with(|d| {
                        let mut d = d.borrow_mut();
                        match d.as_ref() {
                            Some(drag) if held => drag.move_to(e.position.y),
                            _ => *d = None,
                        }
                    });
                    window.refresh();
                });
                window.on_mouse_event(|e: &gpui::MouseUpEvent, phase, window, _| {
                    if phase == gpui::DispatchPhase::Capture && e.button == gpui::MouseButton::Left {
                        THUMB_DRAG.with(|d| d.borrow_mut().take());
                        window.refresh();
                    }
                });
            }
        },
    )
    .absolute()
    .top(px(4.0))
    .bottom(px(4.0))
    .right_0()
    .w(px(SCROLLBAR_WIDTH))
}

/// A [`scrollbar`]'s thumb being dragged: the list it scrolls, how far below the thumb's top it was grabbed, and the
/// track and thumb length when it was (both stay put while the button is held).
struct ThumbDrag {
    handle: gpui::ScrollHandle,
    grab: Pixels,
    track: gpui::Bounds<Pixels>,
    length: Pixels,
}

impl ThumbDrag {
    /// Scrolls the list so the thumb's grabbed point is at `y`.
    fn move_to(&self, y: Pixels) {
        let room = self.track.size.height - self.length;
        let hidden = self.handle.max_offset().height;
        if room <= px(0.0) || hidden <= px(0.0) {
            return;
        }
        let along = ((y - self.grab - self.track.top()) / room).clamp(0.0, 1.0);
        self.handle.set_offset(gpui::point(self.handle.offset().x, -(hidden * along)));
    }
}

thread_local! {
    /// The one scroll bar thumb being dragged, if any - there is one mouse.
    static THUMB_DRAG: std::cell::RefCell<Option<ThumbDrag>> = const { std::cell::RefCell::new(None) };
}

/// `list` - an element that scrolls up and down (`overflow_y_scroll`) - tracked by `handle`, with a [`scrollbar`] over
/// its right edge.
pub fn with_scrollbar(list: Stateful<Div>, handle: &gpui::ScrollHandle, cx: &App) -> Div {
    div().relative().child(list.track_scroll(handle)).child(scrollbar(handle, cx))
}

/// `text` on one line, cut short with "…" where its box ends (give it `flex_1()` to take what room its row leaves).
/// Not `truncate()`: GPUI 0.2.2 keeps the first size it measures for text that doesn't wrap, and in a shrinking flex item
/// that first measure is at full width - so the text was clipped, not shortened. Text that may wrap is measured again
/// whenever its width changes; clamped to one line with an ellipsis, it is shortened at the width it really gets.
pub fn one_line(text: impl IntoElement) -> Div {
    div().min_w_0().overflow_hidden().line_clamp(1).text_ellipsis().child(text)
}

/// A title or label with its explanation as an [`info_tip`] beside it.
pub fn with_tip(title: impl IntoElement, id: impl Into<ElementId>, tip: impl Into<SharedString>, cx: &App) -> Div {
    div().flex().items_center().gap(px(icons::LABEL_GAP)).child(title).child(info_tip(id, tip, cx))
}

/// [`with_tip`] beside text `text_size` big - a page's title -, the ⓘ in the middle of its letters.
pub fn with_tip_sized(
    title: impl IntoElement,
    text_size: Pixels,
    id: impl Into<ElementId>,
    tip: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let drop = info_tip_drop(text_size);
    div().flex().items_center().gap(px(icons::LABEL_GAP)).child(title).child(info_tip_lowered(id, tip, drop, cx))
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
    let label: SharedString = label.into();
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
        // An empty label adds nothing: a button given its icon and text afterwards would otherwise start with an
        // empty child and a gap, its content pushed right.
        .when(!label.is_empty(), |d| d.child(label))
}

/// An icon button (`IconButton`): 28 × 28, transparent until hovered; a third of its strength when disabled.
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    color: Hsla,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    // A close glyph a size smaller: it is drawn larger than the others (`SIZE_CLOSE`).
    let size = if glyph == icons::CLOSE { icons::SIZE_CLOSE } else { icons::SIZE };
    icon_button_sized(id, glyph, size, color, enabled, cx)
}

/// [`icon_button`] with its glyph at `size` - where the C# app set one of its own (a list's title buttons).
pub fn icon_button_sized(
    id: impl Into<ElementId>,
    glyph: &'static str,
    size: f32,
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
        .child(icon(glyph, size, color, cx))
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
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let theme = Theme::get(cx).clone();
    let c = theme.colors;
    // What it showed last frame: a flip while it is on screen slides the knob; one that appears already on or off
    // (a page opening) doesn't. Kept by GPUI as long as the switch is drawn frame after frame.
    let seen = window.use_keyed_state(id.clone(), cx, |_, _| SwitchSeen { on, moving: false });
    if seen.read(cx).on != on {
        seen.update(cx, |seen, _| *seen = SwitchSeen { on, moving: true });
    }
    let slide = theme.animations && seen.read(cx).moving;
    // The knob 3 px in from either end; the track the field border's colour off, the accent on - faded over the
    // slide by an accent layer on top.
    let (off_left, on_left) = (3.0, 17.0);
    let place = move |t: f32| if on { off_left + (on_left - off_left) * t } else { on_left - (on_left - off_left) * t };
    let lit = move |t: f32| if on { t } else { 1.0 - t };
    let track = move |t: f32| {
        div()
            .relative()
            .flex_none()
            .w(px(32.0))
            .h(px(18.0))
            .rounded(px(9.0))
            .bg(c.input_border)
            .child(div().absolute().inset_0().rounded(px(9.0)).bg(c.accent).opacity(lit(t)))
            .child(div().absolute().top(px(3.0)).left(px(place(t))).size(px(12.0)).rounded_full().bg(c.on_accent))
    };
    let track = if slide {
        // Keyed by the state slid to, so a flip back starts a new slide.
        div()
            .child(track(0.0).with_animation(
                ElementId::NamedInteger("switch-slide".into(), u64::from(on)),
                Animation::new(MOTION).with_easing(motion_easing),
                move |_, t| track(t),
            ))
            .into_any_element()
    } else {
        track(1.0).into_any_element()
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .text_color(c.text_primary)
        .on_click(move |_: &ClickEvent, window, cx| on_toggle(&!on, window, cx))
        .child(track)
        .child(label.into())
}

/// A switch's last shown state, and whether it has flipped since it appeared (then its knob slides).
struct SwitchSeen {
    on: bool,
    moving: bool,
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
        .when(checked, |d| d.child(icon(icons::CHECK, icons::SIZE_SMALL, c.on_accent, cx)))
}

/// A status dot: filled while running, a ring while stopped, amber while changing.
pub fn status_dot(color: Hsla, filled: bool, size: Pixels) -> Div {
    div().flex_none().size(size).rounded_full().border(px(1.5)).border_color(color).when(filled, |d| d.bg(color))
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, size};

    use super::{TipPlace, info_tip_drop, tip_origin};
    // GPUI's own `px` here: a `const` needs it, and the placement is worked out at 100 %.
    use gpui::px;

    const WINDOW: gpui::Size<gpui::Pixels> = gpui::Size { width: px(1000.0), height: px(800.0) };

    #[test]
    fn an_info_tip_drops_a_tenth_of_its_text_beside_it() {
        assert_eq!(info_tip_drop(px(22.0)), px(2.0));
        assert_eq!(info_tip_drop(px(13.0)), px(1.0));
        assert_eq!(info_tip_drop(px(8.0)), px(1.0));
    }

    #[test]
    fn a_tip_goes_under_its_element_centred_on_it() {
        let button = Bounds::new(point(px(400.0), px(100.0)), size(px(28.0), px(28.0)));
        let at = tip_origin(button, size(px(100.0), px(27.0)), WINDOW, TipPlace::Below);
        assert_eq!(at, point(px(364.0), px(132.0)));
    }

    #[test]
    fn a_tip_stays_inside_the_window_and_goes_above_without_room_below() {
        let button = Bounds::new(point(px(980.0), px(780.0)), size(px(20.0), px(20.0)));
        let at = tip_origin(button, size(px(200.0), px(27.0)), WINDOW, TipPlace::Below);
        assert_eq!(at, point(px(796.0), px(749.0)));
    }

    #[test]
    fn an_activity_bar_tip_goes_beside_it_centred_on_its_height() {
        let entry = Bounds::new(point(px(12.0), px(45.0)), size(px(32.0), px(32.0)));
        let at = tip_origin(entry, size(px(250.0), px(28.0)), WINDOW, TipPlace::Beside);
        assert_eq!(at, point(px(48.0), px(47.0)));
        // On the right of the window, to its left.
        let entry = Bounds::new(point(px(956.0), px(45.0)), size(px(32.0), px(32.0)));
        let at = tip_origin(entry, size(px(250.0), px(28.0)), WINDOW, TipPlace::Beside);
        assert_eq!(at, point(px(702.0), px(47.0)));
    }
}
