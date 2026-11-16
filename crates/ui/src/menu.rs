//! A context menu (`Themes/Controls/MenuStyles.xaml`): items with their state, bold for the default one, separators,
//! and submenus that open beside their item under the mouse. Placed where the mouse was, in window coordinates.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, FontWeight, Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*,
    px,
};

use crate::theme::Theme;
use crate::{Pad, Tooltip, icon, icons};

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
}

impl ContextMenu {
    pub fn new(position: Point<Pixels>, items: Vec<MenuItem>) -> Self {
        Self { position, items, open_submenu: None, anchor: gpui::Corner::TopLeft }
    }
}

/// Draws a menu's list. `on_close` is called after an item runs; `on_hover_submenu` when the mouse rests on a
/// submenu's item (its index), to open it.
fn list(
    id: impl Into<ElementId>,
    items: &[MenuItem],
    open: Option<usize>,
    on_close: MenuAction,
    on_hover_submenu: HoverSubmenu,
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
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(24.0))
                .pad(px(4.0), px(10.0))
                .rounded(theme.metrics.control_radius)
                .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(c.hover_bg)))
                .when(!enabled, |d| d.text_color(c.text_muted))
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
                        .when_some(tooltip.clone(), |d, tip| d.tooltip(Tooltip::text(tip)))
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
                let mut entry = inset().relative().child(
                    row(enabled)
                        .when(is_open, |d| d.bg(c.hover_bg))
                        .on_hover(move |hovered, window, cx| {
                            if *hovered && enabled {
                                hover(Some(i), window, cx);
                            }
                        })
                        .child(label.clone())
                        .child(icon(icons::CHEVRON_RIGHT, 9.0, c.text_muted, cx)),
                );
                if is_open {
                    let noop: HoverSubmenu = Rc::new(|_, _, _| {});
                    entry = entry.child(
                        div().absolute().top(px(-5.0)).left_full().child(list(
                            SharedString::from(format!("{id}-sub-{i}")),
                            sub,
                            None,
                            on_close.clone(),
                            noop,
                            cx,
                        )),
                    );
                }
                menu.child(entry)
            }
        };
    }
    menu
}

/// The menu over everything, at its position (kept inside the window).
pub fn render(
    menu: &ContextMenu,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    on_hover_submenu: impl Fn(Option<usize>, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let content = list("context-menu", &menu.items, menu.open_submenu, Rc::new(on_close), Rc::new(on_hover_submenu), cx);
    deferred(anchored().anchor(menu.anchor).position(menu.position).snap_to_window().child(content)).with_priority(2)
}
