//! Moving through the window with the keyboard alone: Tab / Shift+Tab go from control to control, Enter or Space press
//! the one with the focus, a ring shows which one that is, and a dialog or menu that is open keeps the focus inside it.
//!
//! GPUI 0.2.2 has the parts but uses none of them by itself: a div with `tab_index` is a tab stop, `focus_next` /
//! `focus_prev` walk the tab stops in the order they were painted (wrapping at the ends), and a focused element with
//! `on_click` is clicked when Enter or Space comes up (`div.rs`, the `KeyUpEvent` listener under `is_focused`). This
//! module binds the keys, gives every control the same tab stop and ring ([`focusable`]), and keeps track of the
//! layers that hold the focus ([`trap`]); the workspace handles [`FocusNext`] / [`FocusPrevious`] with [`move_focus`]
//! and calls [`begin_frame`] as it draws.
//!
//! The ring shows only while the keyboard is being used - from a Tab or a key press on a control until the next mouse
//! press - as in Windows and the browsers. A mouse press clicks a control without moving the focus to it.

use std::cell::RefCell;

use gpui::{
    App, ClickEvent, Div, FocusHandle, Global, InteractiveElement, KeyBinding, NoAction, ParentElement, Stateful,
    WeakFocusHandle, Window, actions, canvas, prelude::*,
};

use crate::theme::Theme;

actions!(focus, [FocusNext, FocusPrevious]);

/// The key context of a control with the focus ([`focusable`]): deeper than any page's or dialog's, so Enter and
/// Space press the control and nothing else.
pub const CONTROL_CONTEXT: &str = "Control";

/// Tab and Shift+Tab everywhere; Enter and Space left to the focused control. A binding wins over the key handlers
/// below it, so a view that needs Tab itself (the terminal) binds it to `NoAction` in its own context.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
        // GPUI clicks the focused control as Enter or Space comes up. `NoAction` here keeps the key down from also
        // running a shallower binding - the projects table's Enter (open the row), the guide's Enter (Next).
        KeyBinding::new("enter", NoAction, Some(CONTROL_CONTEXT)),
        KeyBinding::new("space", NoAction, Some(CONTROL_CONTEXT)),
    ]);
}

/// Whether the focus was last moved with the keyboard - the ring shows while it was.
#[derive(Default)]
struct KeyboardFocus(bool);

impl Global for KeyboardFocus {}

/// Whether focus rings show: the keyboard moved the focus last, not a mouse press.
pub fn keyboard_focus(cx: &App) -> bool {
    cx.try_global::<KeyboardFocus>().is_some_and(|k| k.0)
}

/// Starts or ends showing focus rings (a Tab starts it, a mouse press ends it), drawing the window again when it
/// changes so every ring appears or goes.
pub fn set_keyboard_focus(on: bool, window: &mut Window, cx: &mut App) {
    if keyboard_focus(cx) != on {
        cx.set_global(KeyboardFocus(on));
        window.refresh();
    }
}

/// A control the keyboard can reach: a tab stop (in the order it is drawn), pressed by Enter or Space through its
/// `on_click`, with the focus ring while the keyboard is in use - its 1 px border in the focus colour. For a control
/// with a border of its own (a button, a check box); [`focusable_bare`] for one without.
pub fn focusable(element: Stateful<Div>, cx: &App) -> Stateful<Div> {
    focusable_ring(element, Theme::get(cx).colors.focus_border, cx)
}

/// [`focusable`] with the ring in `ring` - a filled button's on-accent colour, which shows against the accent fill.
///
/// The ring is the border, not a shadow spread past the control: GPUI 0.2.2's Windows shadow shader integrates the
/// shadow over its blur, so one with no blur (a crisp ring) draws nothing (`shaders.hlsl`, `shadow_fragment`).
pub fn focusable_ring(element: Stateful<Div>, ring: gpui::Hsla, cx: &App) -> Stateful<Div> {
    tab_stop(element, cx).when(keyboard_focus(cx), |d| d.focus(move |s| s.border_color(ring)))
}

/// [`focusable`] for a control drawn without a border or background (an icon button, a tab, a link, a menu row): it
/// gets a transparent 1 px border - call this before setting a border of its own - and, while focused, the focus
/// colour on it and the hover fill inside.
pub fn focusable_bare(element: Stateful<Div>, cx: &App) -> Stateful<Div> {
    let c = Theme::get(cx).colors;
    let (ring, fill) = (c.focus_border, c.hover_bg);
    tab_stop(element, cx)
        .border_1()
        .border_color(gpui::transparent_black())
        .when(keyboard_focus(cx), |d| d.focus(move |s| s.border_color(ring).bg(fill)))
}

fn tab_stop(element: Stateful<Div>, _cx: &App) -> Stateful<Div> {
    element
        .tab_index(0)
        .key_context(CONTROL_CONTEXT)
        // A mouse press clicks it but leaves the focus where it was (GPUI would move it here: its focusing listener
        // runs after this one and skips a press whose default is prevented) - the projects table keeps the arrow keys
        // when a row's check box is clicked, a field keeps its caret when a button beside it is.
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| window.prevent_default())
        // Pressed from the keyboard: the rings stay on.
        .on_click(|e: &ClickEvent, window, cx| {
            if e.is_keyboard() {
                set_keyboard_focus(true, window, cx);
            }
        })
}

/// Put once in the window: a mouse press anywhere ends the keyboard's rings. Listened for in the capture phase, before
/// any element can stop it, and without a hitbox, so a popup over everything doesn't hide the press from it.
pub fn watch_mouse() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            window.on_mouse_event(|_: &gpui::MouseDownEvent, phase, window, cx| {
                if phase == gpui::DispatchPhase::Capture {
                    set_keyboard_focus(false, window, cx);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

// --- Layers that hold the focus: dialogs, prompts, menus, Settings ---

/// The layers drawn last and those the last check saw, and where each one gives the focus back when it closes.
#[derive(Default)]
struct Layers {
    /// Painted in the last frame, bottom to top.
    painted: Vec<FocusHandle>,
    /// What the last check ([`after_frame`]) saw painted - not the frame before: GPUI may draw twice before the
    /// check runs, and a menu shut in the first of them would never be seen to close.
    checked: Vec<FocusHandle>,
    /// Each open layer's way back: the control that had the focus before it opened.
    back: Vec<(FocusHandle, WeakFocusHandle)>,
    /// What had the focus at the last check - for a layer that takes the focus itself as it opens (Settings its
    /// search), the way back to what had it before.
    last: Option<WeakFocusHandle>,
    /// The window's own focus (the workspace's), where the focus goes when it has nowhere else.
    root: Option<FocusHandle>,
    /// Whether this frame's check ([`after_frame`]) is waiting to run.
    checking: bool,
}

thread_local! {
    static LAYERS: RefCell<Layers> = RefCell::new(Layers::default());
}

/// Makes `element` a layer that holds the focus while it is drawn - a dialog, a prompt, a menu: Tab and Shift+Tab go
/// round its controls only, the focus moves into it when it opens (onto its first control while the keyboard is in
/// use, unless it took the focus itself), and back to where it was when it closes. The topmost layer drawn wins.
pub fn trap<E: InteractiveElement + ParentElement>(element: E, handle: &FocusHandle) -> E {
    let marked = handle.clone();
    element.track_focus(handle).child(
        // Painted with the layer - after what is under it, a menu's after everything - so the order is the screen's.
        canvas(|_, _, _| {}, move |_, _, _, _| LAYERS.with(|l| l.borrow_mut().painted.push(marked)))
            .absolute()
            .size_0(),
    )
}

/// Called as the window is drawn, before the layers are: starts this frame's list, and checks it once it is on
/// screen - a layer that opened takes the focus, one that closed gives it back. `root` is the window's own focus.
pub fn begin_frame(root: &FocusHandle, window: &mut Window) {
    let schedule = LAYERS.with(|l| {
        let mut l = l.borrow_mut();
        l.painted.clear();
        l.root = Some(root.clone());
        !std::mem::replace(&mut l.checking, true)
    });
    if schedule {
        // At the start of the next frame the one drawn now is GPUI's rendered frame: its tab stops and focus tree.
        window.on_next_frame(after_frame);
    }
}

/// The layers on screen against those the last check saw: one that closed gives the focus back, one that opened
/// takes it.
fn after_frame(window: &mut Window, cx: &mut App) {
    let (opened, closed, root) = LAYERS.with(|l| {
        let mut l = l.borrow_mut();
        l.checking = false;
        let opened: Vec<FocusHandle> = l.painted.iter().filter(|h| !l.checked.contains(h)).cloned().collect();
        let closed: Vec<FocusHandle> = l.checked.iter().filter(|h| !l.painted.contains(h)).cloned().collect();
        l.checked = l.painted.clone();
        (opened, closed, l.root.clone())
    });
    let focused = window.focused(cx);
    // Whether the focus is on nothing that is drawn: its element gone (GPUI keeps a removed element's handle a frame
    // longer, so `focused` isn't None yet - but the window's tree no longer holds it), or on the window itself.
    let stranded = match (&focused, &root) {
        (None, _) => true,
        (Some(f), Some(root)) => f == root || !root.contains_focused(window, cx),
        (Some(_), None) => false,
    };
    // Closed: back to the control that opened it, when the focus was left stranded - while the keyboard is in use.
    // The mouse leaves it where it is.
    let mut returned = None;
    for layer in &closed {
        let back = LAYERS.with(|l| {
            let mut l = l.borrow_mut();
            let at = l.back.iter().position(|(h, _)| h == layer)?;
            l.back.remove(at).1.upgrade()
        });
        if let Some(back) = back {
            if (stranded || focused.as_ref() == Some(layer)) && keyboard_focus(cx) {
                window.focus(&back);
            }
            // Handed on to a layer opening as this one shuts (a menu item that opened Settings).
            returned = Some(back);
        }
    }
    // Opened: the focus goes into it - onto its first control while the keyboard is in use. Its way back: that of a
    // layer that closed just now (a menu whose item opened this), else what had the focus - still, or at the last
    // check when the layer took it itself.
    let last = LAYERS.with(|l| l.borrow().last.as_ref().and_then(|w| w.upgrade()));
    for layer in &opened {
        let inside = layer.contains_focused(window, cx);
        let current = (!inside && !stranded).then(|| focused.clone()).flatten();
        let back = returned.clone().or(current).or(last.clone());
        if let Some(back) = back.filter(|b| b != layer) {
            LAYERS.with(|l| l.borrow_mut().back.push((layer.clone(), back.downgrade())));
        }
        if !inside {
            window.focus(layer);
            if keyboard_focus(cx) {
                move_focus(true, window, cx);
            }
        }
    }
    // Nothing drawn has the focus - its element gone: the keys go to the window again (GPUI would send them nowhere,
    // and Tab would start again from the top).
    if let Some(root) = root.as_ref()
        && !root.contains_focused(window, cx)
    {
        window.focus(root);
    }
    // Where the focus is now, for the next layer to open - not while one is opening (it has the focus already).
    if opened.is_empty() {
        let now = window.focused(cx).filter(|f| Some(f) != root.as_ref()).map(|f| f.downgrade());
        LAYERS.with(|l| l.borrow_mut().last = now);
    }
}

/// Tab (`forward`) or Shift+Tab: the next or previous control - inside the topmost layer while one is open, round the
/// whole window otherwise - with the rings on.
pub fn move_focus(forward: bool, window: &mut Window, cx: &mut App) {
    set_keyboard_focus(true, window, cx);
    let layer = LAYERS.with(|l| l.borrow().painted.last().cloned());
    let mut first: Option<FocusHandle> = None;
    // GPUI walks every tab stop of the window; past the ones outside the layer until one inside it (or round once).
    for _ in 0..10_000 {
        if forward {
            window.focus_next();
        } else {
            window.focus_prev();
        }
        let now = window.focused(cx);
        let Some(layer) = layer.as_ref() else { return };
        if layer.contains_focused(window, cx) {
            return;
        }
        if now.is_none() || now == first {
            // No control in the layer: the layer itself.
            window.focus(layer);
            return;
        }
        if first.is_none() {
            first = now;
        }
    }
}
