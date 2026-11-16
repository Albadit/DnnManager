//! Draws a terminal session's screen and sends what is typed to its shell - ports
//! `Presentation/Terminal/TerminalView.cs`.
//!
//! Mouse wheel scrolls back through the output; drag selects text; Ctrl+C copies a selection (and interrupts the
//! program when nothing is selected), Ctrl+Shift+C copies; Ctrl+V or Ctrl+Shift+V pastes; a right-click copies when
//! text is selected and pastes when not. Shift+PageUp/PageDown and Ctrl+Shift+arrows/PageUp/PageDown scroll.

use std::ops::Range;

use futures::StreamExt;
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Context, CursorStyle, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, Font, FontFallbacks, FontStyle, FontWeight, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString,
    Size, Task, TextRun, UTF16Selection, UnderlineStyle, Window, canvas, div, fill, font, outline, point, prelude::*,
    px, rgb, size,
};

use crate::buffer::{Cell, CellStyle, colour_rgb};
use crate::session::{SessionEvent, TerminalSession};

/// Padding around the grid (left, top, right, bottom), as the C# view's `Padding`.
const PAD_LEFT: f32 = 8.0;
const PAD_TOP: f32 = 6.0;
const PAD_RIGHT: f32 = 4.0;
const PAD_BOTTOM: f32 = 6.0;
/// The scrollbar's column on the right (the C# panel's ScrollBar next to the view).
const SCROLLBAR: f32 = 10.0;

/// How the terminal looks - from the app's theme and the terminal settings, passed in so this crate doesn't depend
/// on the app.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalStyle {
    /// First choice first - [`crate::shells::font_families`] of the setting `terminal.fontFamily`.
    pub font_families: Vec<String>,
    /// `terminal.fontSize` (8-32, default 13).
    pub font_size: Pixels,
    /// `LogBg`.
    pub background: Hsla,
    /// `LogFg` - the default text colour and the cursor.
    pub foreground: Hsla,
    /// `Accent` - the selection, at 35 %.
    pub accent: Hsla,
    /// `ScrollThumb`.
    pub scroll_thumb: Hsla,
    /// The dark theme is shown: Campbell's 16 colours, One Half Light's otherwise.
    pub dark: bool,
}

impl TerminalStyle {
    fn font(&self) -> Font {
        let mut families = self.font_families.iter();
        let mut result = font(SharedString::from(families.next().cloned().unwrap_or_else(|| "Consolas".into())));
        let fallbacks: Vec<String> = families.cloned().collect();
        if !fallbacks.is_empty() {
            result.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
        }
        result
    }
}

/// What a terminal view tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalViewEvent {
    /// The shell ended by itself (`exit`) - its tab goes too.
    Exited,
}

pub struct TerminalView {
    session: TerminalSession,
    style: TerminalStyle,
    focus_handle: FocusHandle,
    // The first line shown, in scrollback + screen lines; None while following the output.
    view_top: Option<usize>,
    // A selection from where the mouse went down to where it is: (line, column) in scrollback + screen lines.
    selection: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
    cell: Size<Pixels>,
    grid_origin: Point<Pixels>,
    scroll_remainder: Pixels,
    // Text an input method is composing - sent once it is committed.
    marked: Option<String>,
    _events: Task<()>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    /// Shows `session`, applying its output as it comes. Typed keys go to it while the view has the keyboard
    /// ([`Focusable`]).
    pub fn new(mut session: TerminalSession, style: TerminalStyle, cx: &mut Context<Self>) -> Self {
        let events = session.take_events();
        let task = cx.spawn(async move |this, cx| {
            let Some(mut events) = events else { return };
            // Everything that arrived meanwhile in one go: one redraw per batch, not per chunk.
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                let updated = this.update(cx, |view, cx| {
                    let mut exited = false;
                    for event in batch {
                        match event {
                            SessionEvent::Output(bytes) => view.session.apply_output(&bytes),
                            SessionEvent::Exited => exited = true,
                        }
                    }
                    cx.notify();
                    if exited {
                        cx.emit(TerminalViewEvent::Exited);
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        let font_size = style.font_size;
        Self {
            session,
            style,
            focus_handle: cx.focus_handle(),
            view_top: None,
            selection: None,
            selecting: false,
            cell: size(font_size * 0.6, font_size * 1.3),
            grid_origin: point(px(PAD_LEFT), px(PAD_TOP)),
            scroll_remainder: px(0.),
            marked: None,
            _events: task,
        }
    }

    pub fn session(&self) -> &TerminalSession {
        &self.session
    }

    /// A new theme or font: the grid is measured again with it.
    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        if self.style != style {
            self.style = style;
            cx.notify();
        }
    }

    /// Ends the shell and everything running in it.
    pub fn close(&mut self) {
        self.session.close();
    }

    // ─── Scrolling ────────────────────────────────────────────────────────

    /// How far the view can be scrolled: the number of lines that scrolled off the top.
    pub fn scroll_maximum(&self) -> usize {
        self.session.buffer().scrollback_len()
    }

    /// The first line shown, from 0 (the oldest line) to [`Self::scroll_maximum`] (the bottom).
    pub fn scroll_position(&self) -> usize {
        self.view_top.unwrap_or(usize::MAX).min(self.scroll_maximum())
    }

    pub fn scroll_to(&mut self, position: usize, cx: &mut Context<Self>) {
        let top = position.min(self.scroll_maximum());
        // At the bottom again: follow the output.
        let view_top = if top >= self.scroll_maximum() { None } else { Some(top) };
        if view_top != self.view_top {
            self.view_top = view_top;
            cx.notify();
        }
    }

    fn scroll_by(&mut self, lines: isize, cx: &mut Context<Self>) {
        let position = (self.scroll_position() as isize + lines).max(0) as usize;
        self.scroll_to(position, cx);
    }

    // ─── Keyboard ─────────────────────────────────────────────────────────

    fn send(&mut self, text: &str, cx: &mut Context<Self>) {
        // Typing goes to the prompt at the bottom - show it.
        self.scroll_to(usize::MAX, cx);
        self.clear_selection(cx);
        self.session.write(text);
    }

    /// Keys that aren't text: sent as the control characters and escape sequences a console expects. Text itself
    /// arrives through the input handler (`replace_text_in_range`), after Windows has translated the key.
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let (ctrl, shift, alt) = (modifiers.control, modifiers.shift, modifiers.alt);
        let key = keystroke.key.as_str();
        let rows = self.session.buffer().rows() as isize;

        if ctrl && !alt {
            if key == "c" && (shift || self.has_selection()) {
                self.copy(cx);
                return cx.stop_propagation();
            }
            if key == "v" {
                self.paste(cx);
                return cx.stop_propagation();
            }
            if shift && matches!(key, "up" | "down" | "pageup" | "pagedown") {
                let lines = match key {
                    "up" => -1,
                    "down" => 1,
                    "pageup" => -rows,
                    _ => rows,
                };
                self.scroll_by(lines, cx);
                return cx.stop_propagation();
            }
            // Ctrl+A … Ctrl+Z are the control characters 1 … 26 (Ctrl+C interrupts, Ctrl+D ends input…).
            if let [letter @ b'a'..=b'z'] = key.as_bytes() {
                let control = char::from(letter - b'a' + 1).to_string();
                self.send(&control, cx);
                return cx.stop_propagation();
            }
            if key == "space" {
                self.send("\0", cx);
                return cx.stop_propagation();
            }
        }
        if shift && matches!(key, "pageup" | "pagedown") {
            self.scroll_by(if key == "pageup" { -rows } else { rows }, cx);
            return cx.stop_propagation();
        }
        if let Some(sequence) = key_sequence(key, ctrl, shift, self.session.buffer().application_cursor_keys()) {
            self.send(&sequence, cx);
            cx.stop_propagation();
        }
    }

    // ─── Clipboard ────────────────────────────────────────────────────────

    fn has_selection(&self) -> bool {
        self.selection.is_some_and(|(start, end)| start != end)
    }

    fn ordered_selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let (a, b) = self.selection.filter(|(start, end)| start != end)?;
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.take().is_some() {
            cx.notify();
        }
    }

    /// The selected text - each line without its trailing blanks.
    pub fn selected_text(&self) -> Option<String> {
        let (from, to) = self.ordered_selection()?;
        Some(selection_text(self.session.buffer(), from, to))
    }

    /// Copies the selected text.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.clear_selection(cx);
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        if !text.is_empty() {
            let text = paste_text(&text, self.session.buffer().bracketed_paste());
            self.send(&text, cx);
        }
    }

    // ─── Mouse ────────────────────────────────────────────────────────────

    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let buffer = self.session.buffer();
        let row = ((position.y - self.grid_origin.y) / self.cell.height).floor().max(0.) as usize;
        let column = ((position.x - self.grid_origin.x) / self.cell.width).floor().max(0.) as usize;
        (self.scroll_position() + row.min(buffer.rows() - 1), column.min(buffer.columns() - 1))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let cell = self.cell_at(event.position);
        self.selection = Some((cell, cell));
        self.selecting = true;
        cx.notify();
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let cell = self.cell_at(event.position);
        if let Some((start, end)) = self.selection
            && end != cell
        {
            self.selection = Some((start, cell));
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        if !self.has_selection() {
            self.clear_selection(cx);
        }
    }

    // Like the Windows console: a right-click copies the selection, or pastes when there is none.
    fn right_click(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if self.has_selection() {
            self.copy(cx);
        } else {
            self.paste(cx);
        }
        cx.stop_propagation();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // A wheel notch is three lines, as in the C# view (Windows' default lines per notch).
        let delta = event.delta.pixel_delta(self.cell.height).y + self.scroll_remainder;
        let lines = (delta / self.cell.height).trunc();
        self.scroll_remainder = delta - self.cell.height * lines;
        if lines != 0. {
            self.scroll_by(-(lines as isize), cx);
        }
        cx.stop_propagation();
    }

    // ─── Size ─────────────────────────────────────────────────────────────

    // As many columns and rows as fit - the shell is told, and wraps and redraws for the new size.
    fn fit(&mut self, bounds: Bounds<Pixels>, cell: Size<Pixels>, cx: &mut Context<Self>) {
        self.cell = cell;
        self.grid_origin = bounds.origin + point(px(PAD_LEFT), px(PAD_TOP));
        let width = bounds.size.width - px(PAD_LEFT + PAD_RIGHT + SCROLLBAR);
        let height = bounds.size.height - px(PAD_TOP + PAD_BOTTOM);
        let columns = (width / cell.width).floor();
        let rows = (height / cell.height).floor();
        if columns >= 2. && rows >= 1. && self.session.resize(columns as usize, rows as usize) {
            cx.notify();
        }
    }
}

/// The escape sequence of a key that isn't text (Enter, Backspace, the arrows, F1…), or None.
pub fn key_sequence(key: &str, ctrl: bool, shift: bool, application_cursor_keys: bool) -> Option<String> {
    // Cursor keys: ESC [ A, or ESC O A when the program switched to application keys.
    let arrow = if application_cursor_keys { "\x1bO" } else { "\x1b[" };
    let sequence = match key {
        "enter" => "\r".to_string(),
        "backspace" => if ctrl { "\x17" } else { "\x7f" }.to_string(),
        "tab" => if shift { "\x1b[Z" } else { "\t" }.to_string(),
        "escape" => "\x1b".to_string(),
        "up" => format!("{arrow}A"),
        "down" => format!("{arrow}B"),
        "right" if ctrl => "\x1b[1;5C".to_string(),
        "left" if ctrl => "\x1b[1;5D".to_string(),
        "right" => format!("{arrow}C"),
        "left" => format!("{arrow}D"),
        "home" => format!("{arrow}H"),
        "end" => format!("{arrow}F"),
        "insert" => "\x1b[2~".to_string(),
        "delete" => "\x1b[3~".to_string(),
        "pageup" => "\x1b[5~".to_string(),
        "pagedown" => "\x1b[6~".to_string(),
        "f1" => "\x1bOP".to_string(),
        "f2" => "\x1bOQ".to_string(),
        "f3" => "\x1bOR".to_string(),
        "f4" => "\x1bOS".to_string(),
        "f5" => "\x1b[15~".to_string(),
        "f6" => "\x1b[17~".to_string(),
        "f7" => "\x1b[18~".to_string(),
        "f8" => "\x1b[19~".to_string(),
        "f9" => "\x1b[20~".to_string(),
        "f10" => "\x1b[21~".to_string(),
        "f11" => "\x1b[23~".to_string(),
        "f12" => "\x1b[24~".to_string(),
        _ => return None,
    };
    Some(sequence)
}

/// A paste as the shell gets it: Enter as a carriage return; the markers tell a program that asked for them it is a
/// paste.
pub fn paste_text(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text }
}

/// The text from `from` to `to` (inclusive; (line, column) in scrollback + screen lines), each line without its
/// trailing blanks, joined by CRLF.
pub fn selection_text(buffer: &crate::TerminalBuffer, from: (usize, usize), to: (usize, usize)) -> String {
    let mut lines = Vec::new();
    for index in from.0..=to.0.min(buffer.total_lines().saturating_sub(1)) {
        let line = buffer.line(index);
        let a = if index == from.0 { from.1.min(line.len()) } else { 0 };
        let b = if index == to.0 { (to.1 + 1).min(line.len()) } else { line.len() };
        let part = crate::buffer::line_text(&line[a..b.max(a)]);
        lines.push(part.trim_end().to_string());
    }
    lines.join("\r\n")
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    // Text typed (or committed by an input method): Enter, Backspace, Escape and Ctrl combinations come from key_down.
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if text.chars().next().is_some_and(|c| c >= ' ' && c != '\x7f') {
            self.send(text, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
    }

    // An input method's window goes at the cursor.
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let buffer = self.session.buffer();
        let (x, y) = buffer.cursor();
        let row = (buffer.scrollback_len() + y).checked_sub(self.scroll_position())?;
        Some(Bounds::new(
            self.grid_origin + point(self.cell.width * x as f32, self.cell.height * row as f32),
            self.cell,
        ))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// What prepaint measured and shaped for paint.
struct Frame {
    quads: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    overlays: Vec<PaintQuad>,
    line_height: Pixels,
}

fn colour(value: u32) -> Hsla {
    rgb(value).into()
}

fn prepaint(view: &Entity<TerminalView>, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) -> Frame {
    let style = view.read(cx).style.clone();
    let base_font = style.font();
    let font_size = style.font_size;
    let text_system = window.text_system().clone();
    let font_id = text_system.resolve_font(&base_font);
    let cell_width = text_system.advance(font_id, font_size, 'M').map(|s| s.width).unwrap_or(font_size * 0.6);
    let cell_height = (text_system.ascent(font_id, font_size) - text_system.descent(font_id, font_size)).ceil();
    let cell = size(cell_width, cell_height.max(px(1.)));
    view.update(cx, |view, cx| view.fit(bounds, cell, cx));

    let focused = view.read(cx).focus_handle.is_focused(window);
    let view = view.read(cx);
    let buffer = view.session.buffer();
    let origin = view.grid_origin;
    let top = view.scroll_position();
    let selection = view.ordered_selection();
    let mut frame = Frame { quads: Vec::new(), lines: Vec::new(), overlays: Vec::new(), line_height: cell.height };

    for row in 0..buffer.rows() {
        let index = top + row;
        if index >= buffer.total_lines() {
            break;
        }
        let line = buffer.line(index);
        let y = origin.y + cell.height * row as f32;
        // Runs of cells that look the same are drawn in one go: their backgrounds, then the text over them.
        let mut text = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut start = 0;
        while start < line.len() {
            let first = line[start];
            let same = |c: &Cell| c.fg == first.fg && c.bg == first.bg && c.style == first.style;
            let end = start + line[start..].iter().take_while(|c| same(c)).count();
            let mut fg = if first.fg == 0 { style.foreground } else { colour(colour_rgb(first.fg, style.dark)) };
            let mut bg = (first.bg != 0).then(|| colour(colour_rgb(first.bg, style.dark)));
            if first.style.has(CellStyle::INVERSE) {
                (fg, bg) = (bg.unwrap_or(style.background), Some(fg));
            }
            if let Some(bg) = bg {
                frame.quads.push(fill(
                    Bounds::new(
                        point(origin.x + cell.width * start as f32, y),
                        size(cell.width * (end - start) as f32, cell.height),
                    ),
                    bg,
                ));
            }
            if first.style.has(CellStyle::DIM) {
                fg = fg.opacity(0.6);
            }
            let run_text = crate::buffer::line_text(&line[start..end]);
            let mut run_font = base_font.clone();
            if first.style.has(CellStyle::BOLD) {
                run_font.weight = FontWeight::BOLD;
            }
            if first.style.has(CellStyle::ITALIC) {
                run_font.style = FontStyle::Italic;
            }
            let underline = first.style.has(CellStyle::UNDERLINE).then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(fg),
                wavy: false,
            });
            runs.push(TextRun {
                len: run_text.len(),
                font: run_font,
                color: fg,
                background_color: None,
                underline,
                strikethrough: None,
            });
            text.push_str(&run_text);
            start = end;
        }
        if !text.trim().is_empty() || line.iter().any(|c| c.style.has(CellStyle::UNDERLINE)) {
            // Each glyph on its own cell, whatever the font's own advance (fallback fonts, bold).
            let shaped = text_system.shape_line(text.into(), font_size, &runs, Some(cell.width));
            frame.lines.push((point(origin.x, y), shaped));
        }

        if let Some((from, to)) = selection
            && index >= from.0
            && index <= to.0
        {
            let a = if index == from.0 { from.1 } else { 0 };
            let b = if index == to.0 { to.1 + 1 } else { buffer.columns() };
            if b > a {
                frame.overlays.push(fill(
                    Bounds::new(
                        point(origin.x + cell.width * a as f32, y),
                        size(cell.width * (b - a) as f32, cell.height),
                    ),
                    style.accent.opacity(0.35),
                ));
            }
        }
    }

    // The cursor: a block while the view has the keyboard, an outline when it doesn't.
    let (cursor_x, cursor_y) = buffer.cursor();
    if buffer.cursor_visible()
        && let Some(row) = (buffer.scrollback_len() + cursor_y).checked_sub(top).filter(|&row| row < buffer.rows())
    {
        let rect =
            Bounds::new(point(origin.x + cell.width * cursor_x as f32, origin.y + cell.height * row as f32), cell);
        frame.overlays.push(if focused {
            fill(rect, style.foreground.opacity(0.55))
        } else {
            outline(rect, style.foreground, BorderStyle::Solid)
        });
    }

    // The scrollbar's thumb, while there is scrollback.
    let total = buffer.total_lines() as f32;
    if buffer.scrollback_len() > 0 {
        let track = bounds.size.height - px(4.);
        let thumb_height = (track * (buffer.rows() as f32 / total)).max(px(16.));
        let thumb_top = (track - thumb_height) * (top as f32 / buffer.scrollback_len() as f32);
        frame.overlays.push(
            fill(
                Bounds::new(
                    point(bounds.right() - px(SCROLLBAR - 2.), bounds.top() + px(2.) + thumb_top),
                    size(px(SCROLLBAR - 4.), thumb_height),
                ),
                style.scroll_thumb,
            )
            .corner_radii(px(3.)),
        );
    }
    frame
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let focus = self.focus_handle.clone();
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(self.style.background)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::right_click))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(
                canvas(
                    move |bounds, window, cx| prepaint(&entity, bounds, window, cx),
                    move |bounds, frame: Frame, window, cx| {
                        for quad in frame.quads {
                            window.paint_quad(quad);
                        }
                        for (origin, line) in &frame.lines {
                            let _ = line.paint(*origin, frame.line_height, window, cx);
                        }
                        for quad in frame.overlays {
                            window.paint_quad(quad);
                        }
                        window.handle_input(&focus, ElementInputHandler::new(bounds, paint_entity), cx);
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalBuffer;

    #[test]
    fn keys_as_a_console_expects_them() {
        assert_eq!(key_sequence("enter", false, false, false).as_deref(), Some("\r"));
        assert_eq!(key_sequence("backspace", false, false, false).as_deref(), Some("\x7f"));
        assert_eq!(key_sequence("backspace", true, false, false).as_deref(), Some("\x17"));
        assert_eq!(key_sequence("tab", false, true, false).as_deref(), Some("\x1b[Z"));
        assert_eq!(key_sequence("up", false, false, false).as_deref(), Some("\x1b[A"));
        assert_eq!(key_sequence("up", false, false, true).as_deref(), Some("\x1bOA"));
        assert_eq!(key_sequence("left", true, false, true).as_deref(), Some("\x1b[1;5D"));
        assert_eq!(key_sequence("f5", false, false, false).as_deref(), Some("\x1b[15~"));
        assert_eq!(key_sequence("a", false, false, false), None);
    }

    #[test]
    fn paste_turns_newlines_into_enter_and_brackets_when_asked() {
        assert_eq!(paste_text("a\r\nb\nc", false), "a\rb\rc");
        assert_eq!(paste_text("x", true), "\x1b[200~x\x1b[201~");
    }

    #[test]
    fn selection_text_trims_each_line() {
        let mut buffer = TerminalBuffer::new(10, 3);
        buffer.feed("hello   \r\nworld\r\nend");
        assert_eq!(selection_text(&buffer, (0, 1), (1, 9)), "ello\r\nworld");
        assert_eq!(selection_text(&buffer, (2, 0), (2, 1)), "en");
    }
}
