//! A context menu (`Themes/Controls/MenuStyles.xaml`): items with their state, bold for the default one, separators,
//! and submenus that open beside their item under the mouse. Placed where the mouse was, in window coordinates.
//!
//! From the keyboard: the menu takes the focus as it opens (its first item while the keyboard is in use), Down / Up
//! and Tab / Shift+Tab go from item to item - a submenu's items follow its entry -, Enter or Space runs one or opens
//! its submenu, Esc closes it; the focus goes back to what opened it ([`crate::focus::trap`]).

use crate::px;
use crate::{Icon, IconSize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Bounds, ClickEvent, ElementId, FontWeight, KeyBinding, Pixels, Point, SharedString, Window, actions, anchored, canvas, deferred, div, point, prelude::*};

use crate::focus::{FocusNext, FocusPrevious};
use crate::theme::Theme;
use crate::{Pad, Tip, focus, icon_sized};

actions!(menu, [CloseMenu]);

/// The keys of an open menu, in its "Menu" context: deeper than the page's, so its Esc closes the menu first.
pub fn bind_keys(cx: &mut App) {
    let context = Some("Menu");
    cx.bind_keys([
        KeyBinding::new("down", FocusNext, context),
        KeyBinding::new("up", FocusPrevious, context),
        KeyBinding::new("escape", CloseMenu, context),
    ]);
}

pub type MenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Called when the mouse rests on a submenu's item (its index), or leaves them (None).
pub type HoverSubmenu = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;

/// One entry of a menu.
#[derive(Clone)]
pub enum MenuItem {
    Item {
        label: SharedString,
        enabled: bool,
        bold: bool,
        tooltip: Option<SharedString>,
        /// Its shortcut, on the right (`InputGestureText`).
        shortcut: Option<SharedString>,
        action: MenuAction,
    },
    /// A heading in a submenu ("DNN", "IIS"): muted, not clickable.
    Heading(SharedString),
    Separator,
    Submenu { label: SharedString, enabled: bool, items: Vec<MenuItem> },
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        MenuItem::Item {
            label: label.into(),
            enabled: true,
            bold: false,
            tooltip: None,
            shortcut: None,
            action: Rc::new(action),
        }
    }

    pub fn enabled(mut self, on: bool) -> Self {
        if let MenuItem::Item { enabled, .. } | MenuItem::Submenu { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }

    pub fn bold(mut self) -> Self {
        if let MenuItem::Item { bold, .. } = &mut self {
            *bold = true;
        }
        self
    }

    pub fn tooltip(mut self, text: Option<impl Into<SharedString>>) -> Self {
        if let MenuItem::Item { tooltip, .. } = &mut self {
            *tooltip = text.map(Into::into);
        }
        self
    }

    pub fn shortcut(mut self, text: Option<impl Into<SharedString>>) -> Self {
        if let MenuItem::Item { shortcut, .. } = &mut self {
            *shortcut = text.map(Into::into);
        }
        self
    }

    pub fn submenu(label: impl Into<SharedString>, items: Vec<MenuItem>) -> Self {
        MenuItem::Submenu { label: label.into(), enabled: true, items }
    }

    /// A greyed entry that only says something ("No logs found for this site").
    pub fn note(label: impl Into<SharedString>) -> Self {
        MenuItem::new(label, |_, _| {}).enabled(false)
    }
}

/// The open menu: where, what, and which submenu is open (by its index path).
pub struct ContextMenu {
    pub position: Point<Pixels>,
    pub items: Vec<MenuItem>,
    pub open_submenu: Option<usize>,
    /// The menu's corner at `position` - the gear's opens upwards, its bottom-left there.
    pub anchor: gpui::Corner,
    /// Where each submenu's entry was drawn and how wide the open submenu was, last frame: a submenu opens beside its
    /// entry on whichever side it fits.
    placement: Placement,
}

#[derive(Clone, Default)]
struct Placement {
    entries: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    window_width: Rc<Cell<Pixels>>,
    submenu_width: Rc<Cell<Option<Pixels>>>,
}

impl ContextMenu {
    pub fn new(position: Point<Pixels>, items: Vec<MenuItem>) -> Self {
        Self { position, items, open_submenu: None, anchor: gpui::Corner::TopLeft, placement: Placement::default() }
    }
}

/// Records the bounds of the element it fills (and the window's width) as it is laid out.
fn measure(record: impl Fn(Bounds<Pixels>, Pixels) + 'static) -> impl IntoElement {
    canvas(move |bounds, window, _| record(bounds, window.viewport_size().width), |_, _, _, _| {})
        .absolute()
        .inset_0()
}

/// Draws a menu's list. `on_close` is called after an item runs; `on_hover_submenu` when the mouse rests on a
/// submenu's item (its index), to open it.
fn list(
    id: impl Into<ElementId>,
    items: &[MenuItem],
    open: Option<usize>,
    on_close: MenuAction,
    on_hover_submenu: HoverSubmenu,
    placement: &Placement,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let id: ElementId = id.into();
    let mut menu = div()
        .id(id.clone())
        .min_w(px(200.0))
        .flex()
        .flex_col()
        .pad(px(4.0), px(0.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(c.card_border)
        .bg(c.card_bg)
        .shadow_lg()
        .occlude()
        .text_size(theme.text.body)
        .text_color(c.text_primary)
        .font_family(theme.font.clone());
    for (i, item) in items.iter().enumerate() {
        let row = |enabled: bool| {
            let row = div()
                .id(SharedString::from(format!("{id}-{i}")))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(24.0))
                .pad(px(4.0), px(10.0))
                .rounded(theme.metrics.control_radius)
                .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                .when(!enabled, |d| d.text_color(c.text_muted));
            // A greyed item is passed over by the keys, as Windows' menus do.
            if enabled { focus::focusable_bare(row, cx) } else { row }
        };
        // A row's 4 px inset from the menu's sides; separators run edge to edge.
        let inset = || div().pad(px(0.0), px(4.0));
        menu = match item {
            MenuItem::Separator => menu.child(div().pad(px(4.0), px(0.0)).child(div().h(px(1.0)).bg(c.card_border))),
            MenuItem::Heading(text) => menu.child(
                div()
                    .pad(px(4.0), px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.text_muted)
                    .child(text.clone()),
            ),
            MenuItem::Item { label, enabled, bold, tooltip, shortcut, action } => {
                let (action, close, hover) = (action.clone(), on_close.clone(), on_hover_submenu.clone());
                let enabled = *enabled;
                menu.child(inset().child(
                    row(enabled)
                        .when(*bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .when_some(tooltip.clone(), |d, tip| d.tip(tip))
                        .on_hover(move |hovered, window, cx| {
                            if *hovered {
                                hover(None, window, cx);
                            }
                        })
                        .when(enabled, |d| {
                            d.on_click(move |_: &ClickEvent, window, cx| {
                                close(window, cx);
                                action(window, cx);
                            })
                        })
                        .child(label.clone())
                        .when_some(shortcut.clone(), |d, keys| d.child(div().text_color(c.text_muted).child(keys))),
                ))
            }
            MenuItem::Submenu { label, enabled, items: sub } => {
                let hover = on_hover_submenu.clone();
                let enabled = *enabled;
                let is_open = open == Some(i);
                let entries = placement.entries.clone();
                let window_width = placement.window_width.clone();
                let mut entry = inset().relative().child(measure(move |bounds, width| {
                    entries.borrow_mut().insert(i, bounds);
                    window_width.set(width);
                }));
                let press = on_hover_submenu.clone();
                entry = entry.child(
                    row(enabled)
                        .when(is_open, |d| d.bg(c.hover_bg))
                        .on_hover(move |hovered, window, cx| {
                            if *hovered && enabled {
                                hover(Some(i), window, cx);
                            }
                        })
                        // Enter or Space (or a click) opens it - or shuts it again; its items come next for Tab.
                        .when(enabled, |d| {
                            d.on_click(move |_: &ClickEvent, window, cx| press((!is_open).then_some(i), window, cx))
                        })
                        .child(label.clone())
                        .child(icon_sized(Icon::ChevronRight, IconSize::Small, c.text_muted, cx)),
                );
                if is_open && let Some(at) = placement.entries.borrow().get(&i).copied() {
                    let noop: HoverSubmenu = Rc::new(|_, _, _| {});
                    let measured = placement.submenu_width.clone();
                    let content = list(
                        SharedString::from(format!("{id}-sub-{i}")),
                        sub,
                        None,
                        on_close.clone(),
                        noop,
                        &Placement::default(),
                        cx,
                    )
                    .relative()
                    .child(measure(move |bounds, _| measured.set(Some(bounds.size.width))));
                    // Beside the menu, its first item level with the entry: on the right when it fits in the window,
                    // else on the left. Until its width is known (its first frame), it is taken as 260 px.
                    let width = placement.submenu_width.get().unwrap_or(px(260.0));
                    let (gap, top) = (px(4.0), at.top() - px(5.0));
                    let fits_right = at.right() + gap + width <= placement.window_width.get();
                    let placed = if fits_right {
                        anchored().position(point(at.right() + gap, top))
                    } else {
                        anchored().anchor(gpui::Corner::TopRight).position(point(at.left() - gap, top))
                    };
                    entry = entry.child(placed.snap_to_window().child(content));
                }
                menu.child(entry)
            }
        };
    }
    menu
}

/// The menu over everything, at its position (kept inside the window). It holds the focus while it is open
/// ([`focus::trap`]); Esc closes it.
pub fn render(
    menu: &ContextMenu,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    on_hover_submenu: impl Fn(Option<usize>, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let on_close: MenuAction = Rc::new(on_close);
    // The menu's own focus, kept while it is drawn - a new one each time a menu opens.
    let handle = window.use_keyed_state("context-menu-focus", cx, |_, cx| cx.focus_handle()).read(cx).clone();
    let escape = on_close.clone();
    let content = list(
        "context-menu",
        &menu.items,
        menu.open_submenu,
        on_close.clone(),
        Rc::new(on_hover_submenu),
        &menu.placement,
        cx,
    )
    .key_context("Menu")
    .on_action(move |_: &CloseMenu, window, cx| escape(window, cx));
    let content = crate::fade_in(focus::trap(content, &handle), "context-menu-fade", cx);
    // Under the menu, over everything else: a layer as large as the window that closes it at a press anywhere - the
    // panel, the title bar, wherever it was opened from. (Its submenus are inside the menu, so a press there is not
    // outside it.)
    let (left, right) = (on_close.clone(), on_close);
    let catcher = div()
        .id("context-menu-catcher")
        .size(px(100_000.0))
        // The press only closes the menu - it does not reach what is under it.
        .occlude()
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| left(window, cx))
        .on_mouse_down(gpui::MouseButton::Right, move |_, window, cx| right(window, cx));
    div()
        .child(deferred(anchored().position(Point::default()).snap_to_window().child(catcher)).with_priority(2))
        .child(
            deferred(anchored().anchor(menu.anchor).position(menu.position).snap_to_window().child(content))
                .with_priority(3),
        )
}
