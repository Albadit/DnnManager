//! A single-line text field, adapted from GPUI's `examples/input.rs` (Zed Industries, Apache-2.0): the theme's
//! colours, a change event for the views that follow it, and Esc to clear.
//!
//! What a key does is [`crate::text_edit`]'s, with each OS's own keys ([`keys`]): Ctrl on Windows and Linux, ⌘ on
//! macOS for copy, paste, cut, select all, undo and redo; moving and deleting by word (Ctrl+arrows / ⌥+arrows) and to
//! the line's ends (Home / End, and ⌘← / ⌘→ on macOS). The mouse places the caret, drags a selection (also past the
//! field's edges), double-clicks a word and triple-clicks the line; a right-click opens Undo, Cut, Copy, Paste and
//! Select all. A text longer than the field scrolls to keep the caret in sight.

use crate::keys::Os;
use crate::menu::{ContextMenu, MenuItem};
use crate::px;
use crate::text_edit::{Move, TextEdit};
use std::ops::Range;
use std::time::{Duration, Instant};

/// The caret's blink: shown and hidden this long each (Windows' default caret blink time).
const BLINK: Duration = Duration::from_millis(530);

use gpui::{App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, DispatchPhase, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, prelude::*, relative, size};

use crate::theme::Theme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToStart,
        Left,
        Right,
        WordLeft,
        WordRight,
        Home,
        End,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectToStart,
        SelectToEnd,
        SelectAll,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Clear,
    ]
);

/// What a key does in a text field - one per action above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToStart,
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    SelectLeft,
    SelectRight,
    SelectWordLeft,
    SelectWordRight,
    SelectToStart,
    SelectToEnd,
    SelectAll,
    ShowCharacterPalette,
    Paste,
    Cut,
    Copy,
    Undo,
    Redo,
    Clear,
}

/// The text field's keys on `os`, as GPUI key bindings write them. Up and Down are left alone: the lists a field
/// filters (the palette, the selects) move with them.
pub fn keys(os: Os) -> Vec<(String, EditKey)> {
    use EditKey::*;
    let p = os.primary();
    let mut keys: Vec<(String, EditKey)> = [
        ("backspace", Backspace),
        ("shift-backspace", Backspace),
        ("delete", Delete),
        ("left", Left),
        ("right", Right),
        ("shift-left", SelectLeft),
        ("shift-right", SelectRight),
        ("home", Home),
        ("end", End),
        ("shift-home", SelectToStart),
        ("shift-end", SelectToEnd),
        ("escape", Clear),
    ]
    .into_iter()
    .map(|(k, e)| (k.to_string(), e))
    .collect();
    for (key, edit) in [("a", SelectAll), ("c", Copy), ("x", Cut), ("v", Paste), ("z", Undo), ("shift-z", Redo)] {
        keys.push((format!("{p}-{key}"), edit));
    }
    let own: &[(&str, EditKey)] = if os == Os::Mac {
        &[
            ("alt-left", WordLeft),
            ("alt-right", WordRight),
            ("alt-shift-left", SelectWordLeft),
            ("alt-shift-right", SelectWordRight),
            ("cmd-left", Home),
            ("cmd-right", End),
            ("cmd-up", Home),
            ("cmd-down", End),
            ("cmd-shift-left", SelectToStart),
            ("cmd-shift-right", SelectToEnd),
            ("cmd-shift-up", SelectToStart),
            ("cmd-shift-down", SelectToEnd),
            ("alt-backspace", DeleteWordLeft),
            ("alt-delete", DeleteWordRight),
            ("cmd-backspace", DeleteToStart),
            // The Emacs keys every macOS text field has: ⌃A and ⌃E to the line's start and end.
            ("ctrl-a", Home),
            ("ctrl-e", End),
            ("ctrl-cmd-space", ShowCharacterPalette),
        ]
    } else {
        &[
            ("ctrl-left", WordLeft),
            ("ctrl-right", WordRight),
            ("ctrl-shift-left", SelectWordLeft),
            ("ctrl-shift-right", SelectWordRight),
            ("ctrl-backspace", DeleteWordLeft),
            ("ctrl-delete", DeleteWordRight),
            ("ctrl-y", Redo),
            // The old IBM keys Windows' and Linux's text boxes still take.
            ("ctrl-insert", Copy),
            ("shift-insert", Paste),
            ("shift-delete", Cut),
        ]
    };
    keys.extend(own.iter().map(|(k, e)| (k.to_string(), *e)));
    keys
}

/// The text field's key bindings on this OS, in the "TextInput" context.
pub fn bind_keys(cx: &mut App) {
    let context = Some("TextInput");
    let bindings: Vec<KeyBinding> = keys(Os::CURRENT)
        .into_iter()
        .map(|(key, edit)| {
            let key = key.as_str();
            match edit {
                EditKey::Backspace => KeyBinding::new(key, Backspace, context),
                EditKey::Delete => KeyBinding::new(key, Delete, context),
                EditKey::DeleteWordLeft => KeyBinding::new(key, DeleteWordLeft, context),
                EditKey::DeleteWordRight => KeyBinding::new(key, DeleteWordRight, context),
                EditKey::DeleteToStart => KeyBinding::new(key, DeleteToStart, context),
                EditKey::Left => KeyBinding::new(key, Left, context),
                EditKey::Right => KeyBinding::new(key, Right, context),
                EditKey::WordLeft => KeyBinding::new(key, WordLeft, context),
                EditKey::WordRight => KeyBinding::new(key, WordRight, context),
                EditKey::Home => KeyBinding::new(key, Home, context),
                EditKey::End => KeyBinding::new(key, End, context),
                EditKey::SelectLeft => KeyBinding::new(key, SelectLeft, context),
                EditKey::SelectRight => KeyBinding::new(key, SelectRight, context),
                EditKey::SelectWordLeft => KeyBinding::new(key, SelectWordLeft, context),
                EditKey::SelectWordRight => KeyBinding::new(key, SelectWordRight, context),
                EditKey::SelectToStart => KeyBinding::new(key, SelectToStart, context),
                EditKey::SelectToEnd => KeyBinding::new(key, SelectToEnd, context),
                EditKey::SelectAll => KeyBinding::new(key, SelectAll, context),
                EditKey::ShowCharacterPalette => KeyBinding::new(key, ShowCharacterPalette, context),
                EditKey::Paste => KeyBinding::new(key, Paste, context),
                EditKey::Cut => KeyBinding::new(key, Cut, context),
                EditKey::Copy => KeyBinding::new(key, Copy, context),
                EditKey::Undo => KeyBinding::new(key, Undo, context),
                EditKey::Redo => KeyBinding::new(key, Redo, context),
                EditKey::Clear => KeyBinding::new(key, Clear, context),
            }
        })
        .collect();
    cx.bind_keys(bindings);
}

/// The keys of the text field that take a modifier on `os` (⌘C, Ctrl+Left …) - a shortcut on one of them would take
/// it away from every text field.
pub fn modified_keys(os: Os) -> Vec<String> {
    keys(os).into_iter().map(|(k, _)| k).filter(|k| k.contains('-') && !k.starts_with("shift-")).collect()
}

/// The text changed - typed, pasted, cut or cleared.
#[derive(Debug, Clone)]
pub struct TextChanged(pub SharedString);

pub struct TextInput {
    focus_handle: FocusHandle,
    edit: TextEdit,
    placeholder: SharedString,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// How far the text is scrolled left, to keep the caret in sight in a text longer than the field.
    scroll_x: Pixels,
    /// A mouse press in the field is held: moving drags the selection.
    is_selecting: bool,
    /// Esc empties it first (a search box); else Esc goes on to close what it is in (a dialog's field).
    clears_on_escape: bool,
    /// The caret blinks from here - moved on by every edit and move, so the caret shows at once after one.
    blink_from: Instant,
    /// What the caret was last at (the text's length, the selection) - a change restarts the blink.
    caret_at: (usize, Range<usize>),
    /// Redraws while focused, for the blink; dropped when the field loses the keyboard.
    _blink: Option<gpui::Task<()>>,
    /// The right-click menu, while it is open.
    menu: Option<ContextMenu>,
}

impl EventEmitter<TextChanged> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            // A tab stop: Tab and Shift+Tab reach every field.
            focus_handle: cx.focus_handle().tab_stop(true),
            edit: TextEdit::default(),
            placeholder: placeholder.into(),
            last_layout: None,
            last_bounds: None,
            scroll_x: Pixels::ZERO,
            is_selecting: false,
            clears_on_escape: false,
            blink_from: Instant::now(),
            caret_at: (0, 0..0),
            _blink: None,
            menu: None,
        }
    }

    /// A search box: Esc empties it first, then closes what it is in (`SearchBox`).
    pub fn search(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self { clears_on_escape: true, ..Self::new(placeholder, cx) }
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    /// Whether the caret shows now: solid for a moment after an edit or a move, then on and off every `BLINK`.
    fn caret_visible(&self) -> bool {
        (self.blink_from.elapsed().as_millis() / BLINK.as_millis()).is_multiple_of(2)
    }

    pub fn text(&self) -> &str {
        self.edit.text()
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.edit.set_text(&text.into());
        cx.emit(TextChanged(self.edit.text().to_string().into()));
        cx.notify();
    }

    /// Runs `change` on the text and redraws; tells the views that follow it when the text changed.
    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut TextEdit)) {
        let before = self.edit.text().to_string();
        change(&mut self.edit);
        if self.edit.text() != before {
            cx.emit(TextChanged(self.edit.text().to_string().into()));
        }
        cx.notify();
    }

    /// Esc: a filled search box is emptied; otherwise the key goes on (to close what it is in).
    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        if self.edit.text().is_empty() || !self.clears_on_escape {
            cx.propagate();
        } else {
            self.set_text("", cx);
        }
    }

    fn move_caret(&mut self, to: Move, cx: &mut Context<Self>) {
        self.change(cx, |e| e.move_caret(to, Os::CURRENT));
    }

    fn select(&mut self, to: Move, cx: &mut Context<Self>) {
        self.change(cx, |e| e.select(to, Os::CURRENT));
    }

    fn delete(&mut self, to: Move, cx: &mut Context<Self>) {
        self.change(cx, |e| {
            e.delete(to, Os::CURRENT);
        });
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.edit.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        }
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.edit.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            self.change(cx, |e| {
                e.cut();
            });
        }
    }

    /// A line: what is pasted comes in on it, its line breaks as spaces.
    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let line = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
            self.change(cx, |e| e.paste(&line));
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.index_for_mouse_position(event.position);
        match event.click_count {
            // A triple-click: the whole line.
            3.. => self.change(cx, |e| e.select_all()),
            2 => self.change(cx, |e| e.select_word_at(at)),
            _ if event.modifiers.shift => self.change(cx, |e| e.select_to(at)),
            _ => self.change(cx, |e| e.set_caret(at)),
        }
        // A drag goes on from a single click only - a double or triple click keeps what it selected.
        self.is_selecting = event.click_count < 2;
    }

    /// Dragging, inside the field or past its edges: the selection follows the mouse.
    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let at = self.index_for_mouse_position(position);
        if at != self.edit.caret() {
            self.change(cx, |e| e.select_to(at));
        }
    }

    fn open_menu(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let os = Os::CURRENT;
        let shortcut = |key: &str| Some(crate::keys::label(os, os != Os::Mac, false, false, os == Os::Mac, key));
        let redo = if os == Os::Mac {
            crate::keys::label(os, false, false, true, true, "Z")
        } else {
            crate::keys::label(os, true, false, false, false, "Y")
        };
        let weak = cx.entity().downgrade();
        let focus = self.focus_handle.clone();
        // Each item runs on the field and gives it the keyboard back.
        let item = |label: &str, enabled: bool, keys: Option<String>, run: fn(&mut TextInput, &mut Context<TextInput>)| {
            let (weak, focus) = (weak.clone(), focus.clone());
            MenuItem::new(label.to_string(), move |window, cx| {
                window.focus(&focus);
                let _ = weak.update(cx, run);
            })
            .enabled(enabled)
            .shortcut(keys)
        };
        let selected = self.edit.selected_text().is_some();
        let pasteable = cx.read_from_clipboard().and_then(|item| item.text()).is_some_and(|t| !t.is_empty());
        let all = !self.edit.text().is_empty() && self.edit.selection().len() < self.edit.text().len();
        let items = vec![
            item("Undo", self.edit.can_undo(), shortcut("Z"), |input, cx| input.change(cx, |e| {
                e.undo();
            })),
            item("Redo", self.edit.can_redo(), Some(redo), |input, cx| input.change(cx, |e| {
                e.redo();
            })),
            MenuItem::Separator,
            item("Cut", selected, shortcut("X"), |input, cx| input.cut(cx)),
            item("Copy", selected, shortcut("C"), |input, cx| input.copy(cx)),
            item("Paste", pasteable, shortcut("V"), |input, cx| input.paste(cx)),
            MenuItem::Separator,
            item("Select all", all, shortcut("A"), |input, cx| input.change(cx, |e| e.select_all())),
        ];
        self.menu = Some(ContextMenu::new(event.position, items));
        cx.notify();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.edit.text().is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if line.text.as_ref() != self.edit.text() {
            return self.edit.caret();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.scroll_x)
    }

    fn offset_from_utf16(text: &str, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in text.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.edit.text().chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        let text = self.edit.text();
        Self::offset_from_utf16(text, range_utf16.start)..Self::offset_from_utf16(text, range_utf16.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        self.edit.text().get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.edit.selection()), reversed: self.edit.reversed() })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.edit.marked().map(|range| self.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.edit.unmark();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|r| self.range_from_utf16(&r));
        // A line: a line break typed or put in by an IME doesn't go in.
        let line = new_text.replace(['\r', '\n'], "");
        self.change(cx, |e| e.replace(range, &line));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|r| self.range_from_utf16(&r));
        // The selection is within the new text, in its UTF-16 units.
        let selected = new_selected_range_utf16
            .map(|r| Self::offset_from_utf16(new_text, r.start)..Self::offset_from_utf16(new_text, r.end));
        self.change(cx, |e| e.replace_and_mark(range, new_text, selected));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let left = bounds.left() - self.scroll_x;
        Some(Bounds::from_corners(
            point(left + last_layout.x_for_index(range.start), bounds.top()),
            point(left + last_layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let last_layout = self.last_layout.as_ref()?;
        // An empty field shows its placeholder: there is no character to point at (macOS asks, for its dictionary
        // look-up and the like).
        if last_layout.text.as_ref() != self.edit.text() {
            return None;
        }
        let utf8_index = last_layout.index_for_x(point.x - bounds.left() + self.scroll_x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// How far a line `line_width` wide is scrolled in a field `width` wide so the caret at `caret_x` shows: as little
/// as can be from `scroll`, and never past the text's end.
fn scroll_for(scroll: Pixels, caret_x: Pixels, line_width: Pixels, width: Pixels) -> Pixels {
    let caret_width = px(1.0);
    let mut scroll = scroll;
    if caret_x < scroll {
        scroll = caret_x;
    } else if caret_x + caret_width > scroll + width {
        scroll = caret_x + caret_width - width;
    }
    scroll.min((line_width + caret_width - width).max(Pixels::ZERO)).max(Pixels::ZERO)
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content: SharedString = input.edit.text().to_string().into();
        let selected_range = input.edit.selection();
        let cursor = input.edit.caret();
        let style = window.text_style();
        let colors = Theme::get(cx).colors;

        let (display_text, text_color) =
            if content.is_empty() { (input.placeholder.clone(), colors.text_muted) } else { (content, style.color) };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.edit.marked() {
            vec![
                TextRun { len: marked_range.start, ..run.clone() },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }),
                    ..run.clone()
                },
                TextRun { len: display_text.len() - marked_range.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);

        let cursor_pos = if input.edit.text().is_empty() { Pixels::ZERO } else { line.x_for_index(cursor) };
        let scroll_x = scroll_for(input.scroll_x, cursor_pos, line.width, bounds.size.width);
        let left = bounds.left() - scroll_x;
        // The caret: 1 px wide and as high as the text itself (1.2 x its size) - not the whole, taller line box - in
        // the line's middle, as Windows' and VS Code's.
        let caret_height = (font_size * 1.2).round().min(bounds.size.height);
        let caret_top = bounds.top() + ((bounds.size.height - caret_height) / 2.0).round();
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(Bounds::new(point(left + cursor_pos, caret_top), size(px(1.), caret_height)), colors.text_primary)),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(left + line.x_for_index(selected_range.start), bounds.top()),
                        point(left + line.x_for_index(selected_range.end), bounds.bottom()),
                    ),
                    colors.accent.opacity(0.3),
                )),
                None,
            )
        };
        PrepaintState { line: Some(line), cursor, selection, scroll_x }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let line = prepaint.line.take().unwrap();
        let scroll_x = prepaint.scroll_x;
        // The text scrolls inside the field: nothing of it is drawn past its edges.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            line.paint(point(bounds.left() - scroll_x, bounds.top()), window.line_height(), window, cx).unwrap();
            if focus_handle.is_focused(window)
                && self.input.read(cx).caret_visible()
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });

        // A drag goes on past the field's edges (GPUI's `on_mouse_move` only sees the mouse over the element), and
        // ends wherever the button comes up.
        let input = self.input.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) && input.read(cx).is_selecting {
                input.update(cx, |input, cx| input.drag_to(event.position, cx));
            }
        });
        let input = self.input.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && input.read(cx).is_selecting {
                input.update(cx, |input, _| input.is_selecting = false);
            }
        });

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_x = scroll_x;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An edit or a move shows the caret at once, then it blinks again.
        let caret_at = (self.edit.text().len(), self.edit.selection());
        if caret_at != self.caret_at {
            self.caret_at = caret_at;
            self.blink_from = Instant::now();
        }
        // The blink: a redraw at each change while the field has the keyboard - none without it.
        let focused = self.focus_handle.is_focused(window);
        if focused && self._blink.is_none() {
            self.blink_from = Instant::now();
            self._blink = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(BLINK).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        } else if !focused && self._blink.is_some() {
            self._blink = None;
        }
        let field = div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.delete(Move::Left, cx)))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.delete(Move::Right, cx)))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| this.delete(Move::WordLeft, cx)))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| this.delete(Move::WordRight, cx)))
            .on_action(cx.listener(|this, _: &DeleteToStart, _, cx| this.delete(Move::LineStart, cx)))
            .on_action(cx.listener(|this, _: &Left, _, cx| this.move_caret(Move::Left, cx)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.move_caret(Move::Right, cx)))
            .on_action(cx.listener(|this, _: &WordLeft, _, cx| this.move_caret(Move::WordLeft, cx)))
            .on_action(cx.listener(|this, _: &WordRight, _, cx| this.move_caret(Move::WordRight, cx)))
            .on_action(cx.listener(|this, _: &Home, _, cx| this.move_caret(Move::LineStart, cx)))
            .on_action(cx.listener(|this, _: &End, _, cx| this.move_caret(Move::LineEnd, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.select(Move::Left, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.select(Move::Right, cx)))
            .on_action(cx.listener(|this, _: &SelectWordLeft, _, cx| this.select(Move::WordLeft, cx)))
            .on_action(cx.listener(|this, _: &SelectWordRight, _, cx| this.select(Move::WordRight, cx)))
            .on_action(cx.listener(|this, _: &SelectToStart, _, cx| this.select(Move::LineStart, cx)))
            .on_action(cx.listener(|this, _: &SelectToEnd, _, cx| this.select(Move::LineEnd, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.change(cx, |e| e.select_all())))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, window, _| window.show_character_palette()))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.change(cx, |e| {
                    e.undo();
                })
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.change(cx, |e| {
                    e.redo();
                })
            }))
            .on_action(cx.listener(Self::clear))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::open_menu))
            .w_full()
            .overflow_hidden()
            .child(TextElement { input: cx.entity() });
        // Last: the menu borrows `cx` for as long as it is drawn.
        let menu = self.menu.as_ref().map(|menu| {
            let (weak, focus) = (cx.entity().downgrade(), self.focus_handle.clone());
            crate::menu::render(
                menu,
                move |window, cx| {
                    window.focus(&focus);
                    let _ = weak.update(cx, |input, cx| {
                        input.menu = None;
                        cx.notify();
                    });
                },
                |_, _, _| {},
                window,
                cx,
            )
        });
        field.children(menu)
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_has_its_own_editing_keys() {
        let key = |os: Os, key: &str| keys(os).into_iter().find(|(k, _)| k == key).map(|(_, e)| e);
        // Copy, paste, cut, select all, undo and redo: ⌘ on macOS, Ctrl elsewhere.
        for (letter, edit) in [("c", EditKey::Copy), ("v", EditKey::Paste), ("x", EditKey::Cut), ("a", EditKey::SelectAll), ("z", EditKey::Undo)] {
            assert_eq!(key(Os::Mac, &format!("cmd-{letter}")), Some(edit));
            assert_eq!(key(Os::Windows, &format!("ctrl-{letter}")), Some(edit));
            assert_eq!(key(Os::Linux, &format!("ctrl-{letter}")), Some(edit));
            assert_eq!(key(Os::Windows, &format!("cmd-{letter}")), None, "the Windows key isn't Ctrl");
        }
        assert_eq!(key(Os::Mac, "ctrl-a"), Some(EditKey::Home), "⌃A goes to the start on macOS - it doesn't select all");
        assert_eq!(key(Os::Mac, "cmd-shift-z"), Some(EditKey::Redo));
        assert_eq!(key(Os::Windows, "ctrl-y"), Some(EditKey::Redo));
        assert_eq!(key(Os::Linux, "ctrl-shift-z"), Some(EditKey::Redo));
        assert_eq!(key(Os::Mac, "ctrl-y"), None);
        // Words and the line's ends.
        assert_eq!(key(Os::Mac, "alt-left"), Some(EditKey::WordLeft));
        assert_eq!(key(Os::Mac, "cmd-right"), Some(EditKey::End));
        assert_eq!(key(Os::Mac, "cmd-shift-left"), Some(EditKey::SelectToStart));
        assert_eq!(key(Os::Mac, "alt-backspace"), Some(EditKey::DeleteWordLeft));
        assert_eq!(key(Os::Mac, "cmd-backspace"), Some(EditKey::DeleteToStart));
        assert_eq!(key(Os::Windows, "ctrl-left"), Some(EditKey::WordLeft));
        assert_eq!(key(Os::Linux, "ctrl-shift-right"), Some(EditKey::SelectWordRight));
        assert_eq!(key(Os::Windows, "ctrl-backspace"), Some(EditKey::DeleteWordLeft));
        assert_eq!(key(Os::Mac, "ctrl-left"), None, "⌃← switches Spaces on macOS");
        for os in Os::ALL {
            assert_eq!(key(os, "shift-home"), Some(EditKey::SelectToStart));
            assert_eq!(key(os, "shift-backspace"), Some(EditKey::Backspace), "{os:?}: Shift held while deleting");
            assert_eq!(key(os, "up"), None, "{os:?}: Up and Down belong to the lists fields filter");
        }
    }

    #[test]
    fn every_key_is_bound_once_and_parses() {
        for os in Os::ALL {
            let keys = keys(os);
            for (i, (key, _)) in keys.iter().enumerate() {
                assert!(gpui::Keystroke::parse(key).is_ok(), "{os:?}: {key}");
                assert!(!keys[..i].iter().any(|(k, _)| k == key), "{os:?}: {key} twice");
            }
        }
    }

    #[test]
    fn the_modified_keys_leave_out_plain_and_shift_keys() {
        let mac = modified_keys(Os::Mac);
        assert!(mac.contains(&"cmd-c".to_string()) && mac.contains(&"alt-left".to_string()));
        assert!(!mac.iter().any(|k| k == "left" || k == "shift-home"));
        assert!(modified_keys(Os::Windows).contains(&"ctrl-delete".to_string()));
    }

    #[test]
    fn the_caret_stays_in_sight() {
        let p = |v: f32| px(v);
        // Short text: no scroll.
        assert_eq!(scroll_for(p(0.0), p(50.0), p(80.0), p(200.0)), p(0.0));
        // The caret past the right edge: scrolled just enough.
        assert_eq!(scroll_for(p(0.0), p(250.0), p(300.0), p(200.0)), p(51.0));
        // Back left of the view: scrolled to it.
        assert_eq!(scroll_for(p(100.0), p(40.0), p(300.0), p(200.0)), p(40.0));
        // Text deleted: never scrolled past its end.
        assert_eq!(scroll_for(p(100.0), p(120.0), p(150.0), p(200.0)), p(0.0));
    }
}
