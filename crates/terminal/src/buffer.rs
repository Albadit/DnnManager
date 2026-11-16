//! The screen of a terminal and its VT parser - ports `Presentation/Terminal/TerminalBuffer.cs` (with its `LineRing`).
//!
//! A grid of cells with the cursor, the lines that scrolled off the top (the scrollback) and the alternate screen
//! full-screen programs switch to. [`TerminalBuffer::feed`] takes what the program printed - text with VT / xterm escape
//! sequences, as a Windows pseudo console sends it - and applies it. Not thread-safe: fed and read on the UI thread.

use std::collections::VecDeque;

/// A colour of a cell: 0 is the default, [`INDEXED`] | n one of the 256 palette colours, [`RGB`] | 0xRRGGBB a direct
/// colour.
pub const INDEXED: u32 = 0x0100_0000;
/// See [`INDEXED`].
pub const RGB: u32 = 0x0200_0000;

const MAX_SCROLLBACK: usize = 5000;

/// Bold, dim, italic, underline, inverse - as bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CellStyle(pub u8);

impl CellStyle {
    pub const NONE: CellStyle = CellStyle(0);
    pub const BOLD: CellStyle = CellStyle(1);
    pub const DIM: CellStyle = CellStyle(2);
    pub const ITALIC: CellStyle = CellStyle(4);
    pub const UNDERLINE: CellStyle = CellStyle(8);
    pub const INVERSE: CellStyle = CellStyle(16);

    pub fn has(self, flag: CellStyle) -> bool {
        self.0 & flag.0 != 0
    }

    fn set(&mut self, flag: CellStyle) {
        self.0 |= flag.0;
    }

    fn clear(&mut self, flag: CellStyle) {
        self.0 &= !flag.0;
    }
}

/// One character of the screen; `'\0'` is a cell nothing was written to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub style: CellStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
    String,
    StringEscape,
    SkipOne,
}

pub struct TerminalBuffer {
    columns: usize,
    rows: usize,
    cursor_x: usize,
    cursor_y: usize,
    cursor_visible: bool,
    application_cursor_keys: bool,
    bracketed_paste: bool,
    scrollback: VecDeque<Vec<Cell>>,
    screen: Vec<Vec<Cell>>,
    // The main screen, kept while the alternate one is shown.
    main_screen: Option<Vec<Vec<Cell>>>,
    main_cursor: (usize, usize),
    saved: (usize, usize, Cell),
    pen: Cell,
    // The cursor sits on the last column with a character just written there: the next one wraps.
    wrap_pending: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    state: State,
    sequence: String,
    replies: Vec<String>,
}

impl TerminalBuffer {
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.max(2);
        let rows = rows.max(1);
        Self {
            columns,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            application_cursor_keys: false,
            bracketed_paste: false,
            scrollback: VecDeque::new(),
            screen: new_screen(columns, rows),
            main_screen: None,
            main_cursor: (0, 0),
            saved: (0, 0, Cell::default()),
            pen: Cell::default(),
            wrap_pending: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            state: State::Ground,
            sequence: String::new(),
            replies: Vec::new(),
        }
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// The program asked for cursor keys as application keys (ESC O A rather than ESC [ A).
    pub fn application_cursor_keys(&self) -> bool {
        self.application_cursor_keys
    }

    /// The program wants a paste wrapped in markers, so it can tell it from typing.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// The number of lines that scrolled off the top.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Scrollback plus screen - the lines a view can scroll through.
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Line `index` of scrollback plus screen (0 = the oldest scrollback line). A scrollback line keeps the width it
    /// had, so it may be shorter or longer than [`Self::columns`].
    pub fn line(&self, index: usize) -> &[Cell] {
        if index < self.scrollback.len() {
            &self.scrollback[index]
        } else {
            &self.screen[index - self.scrollback.len()]
        }
    }

    /// The answers the program asked for (e.g. the cursor position) since the last call - to be written to its input.
    pub fn take_replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.replies)
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.max(2);
        let rows = rows.max(1);
        if columns == self.columns && rows == self.rows {
            return;
        }
        let on_main = self.main_screen.is_none();
        let screen = std::mem::take(&mut self.screen);
        self.screen = self.resized(screen, columns, rows, on_main, on_main);
        if let Some(main) = self.main_screen.take() {
            self.main_screen = Some(self.resized(main, columns, rows, true, false));
        }
        self.columns = columns;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.cursor_x = self.cursor_x.min(columns - 1);
        self.cursor_y = self.cursor_y.min(rows - 1);
        self.wrap_pending = false;
    }

    // Fewer rows: the top ones go (to the scrollback on the main screen), so the cursor's line stays. The pseudo
    // console redraws its screen after a resize anyway.
    fn resized(
        &mut self,
        mut screen: Vec<Vec<Cell>>,
        columns: usize,
        rows: usize,
        keep_bottom: bool,
        to_scrollback: bool,
    ) -> Vec<Vec<Cell>> {
        let drop = if keep_bottom { self.cursor_y.saturating_sub(rows - 1) } else { 0 };
        let mut kept = screen.split_off(drop.min(screen.len()));
        if to_scrollback {
            for line in screen {
                self.push_scrollback(line);
            }
        }
        self.cursor_y -= drop;
        kept.resize_with(rows, Vec::new);
        for line in &mut kept {
            line.resize(columns, Cell::default());
        }
        kept
    }

    fn push_scrollback(&mut self, line: Vec<Cell>) {
        if self.scrollback.len() == MAX_SCROLLBACK {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(line);
    }

    // ─── Parsing ──────────────────────────────────────────────────────────

    pub fn feed(&mut self, text: &str) {
        for c in text.chars() {
            match self.state {
                State::Ground => {
                    if c == '\x1b' {
                        self.state = State::Escape;
                    } else if c < ' ' {
                        self.control(c);
                    } else if c != '\x7f' {
                        self.print(c);
                    }
                }
                State::Escape => {
                    self.state = State::Ground;
                    match c {
                        '[' => {
                            self.sequence.clear();
                            self.state = State::Csi;
                        }
                        // OSC (window title…), DCS, SOS, PM, APC: strings up to BEL or ESC \ - nothing here uses them.
                        ']' | 'P' | 'X' | '^' | '_' => self.state = State::String,
                        // Character set choices take one more character.
                        '(' | ')' | '*' | '+' | '#' | '%' => self.state = State::SkipOne,
                        '7' => self.save_cursor(),
                        '8' => self.restore_cursor(),
                        'D' => self.line_feed(),
                        'E' => {
                            self.cursor_x = 0;
                            self.line_feed();
                        }
                        'M' => self.reverse_line_feed(),
                        'c' => self.reset(),
                        _ => {}
                    }
                }
                State::Csi => {
                    // Parameters and intermediates (0x20-0x3F) are collected; a letter (0x40-0x7E) ends the sequence.
                    if ('\x40'..='\x7e').contains(&c) {
                        self.state = State::Ground;
                        let body = std::mem::take(&mut self.sequence);
                        self.csi(&body, c);
                        self.sequence = body;
                    } else if c == '\x1b' {
                        self.state = State::Escape;
                    } else if c < ' ' {
                        self.control(c);
                    } else if self.sequence.len() < 64 {
                        self.sequence.push(c);
                    }
                }
                State::String => {
                    if c == '\x07' {
                        self.state = State::Ground;
                    } else if c == '\x1b' {
                        self.state = State::StringEscape;
                    }
                }
                State::StringEscape => self.state = if c == '\\' { State::Ground } else { State::String },
                State::SkipOne => self.state = State::Ground,
            }
        }
    }

    fn control(&mut self, c: char) {
        match c {
            '\r' => {
                self.cursor_x = 0;
                self.wrap_pending = false;
            }
            '\n' | '\x0b' | '\x0c' => self.line_feed(),
            '\x08' => {
                self.cursor_x = self.cursor_x.saturating_sub(1);
                self.wrap_pending = false;
            }
            '\t' => {
                self.cursor_x = (self.columns - 1).min((self.cursor_x / 8 + 1) * 8);
                self.wrap_pending = false;
            }
            _ => {}
        }
    }

    fn print(&mut self, c: char) {
        if self.wrap_pending {
            self.cursor_x = 0;
            self.line_feed();
        }
        let mut cell = self.pen;
        cell.ch = c;
        self.screen[self.cursor_y][self.cursor_x] = cell;
        if self.cursor_x == self.columns - 1 {
            self.wrap_pending = true;
        } else {
            self.cursor_x += 1;
        }
    }

    fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.cursor_y == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_y < self.rows - 1 {
            self.cursor_y += 1;
        }
    }

    fn reverse_line_feed(&mut self) {
        self.wrap_pending = false;
        if self.cursor_y == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor_y > 0 {
            self.cursor_y -= 1;
        }
    }

    /// The scroll region moves up `count` lines; on the main screen, lines leaving the top are kept.
    fn scroll_up(&mut self, count: usize) {
        self.scroll_up_keeping(count, true);
    }

    fn scroll_up_keeping(&mut self, count: usize, keep: bool) {
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top + 1) {
            let blank = self.blank_line();
            let top = self.screen.remove(self.scroll_top);
            self.screen.insert(self.scroll_bottom, blank);
            if keep && self.scroll_top == 0 && self.main_screen.is_none() {
                self.push_scrollback(top);
            }
        }
    }

    fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top + 1) {
            let blank = self.blank_line();
            self.screen.remove(self.scroll_bottom);
            self.screen.insert(self.scroll_top, blank);
        }
    }

    // Erased cells take the current background, as terminals do.
    fn blank(&self) -> Cell {
        Cell { bg: self.pen.bg, ..Cell::default() }
    }

    fn blank_line(&self) -> Vec<Cell> {
        vec![self.blank(); self.columns]
    }

    fn save_cursor(&mut self) {
        self.saved = (self.cursor_x, self.cursor_y, self.pen);
    }

    fn restore_cursor(&mut self) {
        self.cursor_x = self.saved.0.min(self.columns - 1);
        self.cursor_y = self.saved.1.min(self.rows - 1);
        self.pen = self.saved.2;
        self.wrap_pending = false;
    }

    fn reset(&mut self) {
        self.main_screen = None;
        self.screen = new_screen(self.columns, self.rows);
        self.pen = Cell::default();
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.cursor_visible = true;
        self.application_cursor_keys = false;
        self.bracketed_paste = false;
        self.wrap_pending = false;
    }

    // ─── CSI sequences ────────────────────────────────────────────────────

    fn csi(&mut self, body: &str, final_char: char) {
        // "?" (and > = !) mark private sequences; a trailing space, $ " or ' is an intermediate of ones not handled here.
        let is_private = body.starts_with(['?', '>', '=', '!']);
        if body.ends_with([' ', '$', '"', '\'']) {
            return;
        }
        let numbers = numbers(if is_private { &body[1..] } else { body });
        let arg = |index: usize, fallback: usize| match numbers.get(index) {
            Some(&n) if n > 0 => n,
            _ => fallback,
        };

        if is_private {
            if body.starts_with('?') && matches!(final_char, 'h' | 'l') {
                for &mode in &numbers {
                    self.set_mode(mode, final_char == 'h');
                }
            }
            return;
        }

        let (x, y) = (self.cursor_x as isize, self.cursor_y as isize);
        let a = |index: usize, fallback: usize| arg(index, fallback) as isize;
        match final_char {
            'A' => self.move_to(x, y - a(0, 1), true),
            'B' | 'e' => self.move_to(x, y + a(0, 1), true),
            'C' | 'a' => self.move_to(x + a(0, 1), y, false),
            'D' => self.move_to(x - a(0, 1), y, false),
            'E' => self.move_to(0, y + a(0, 1), false),
            'F' => self.move_to(0, y - a(0, 1), false),
            'G' | '`' => self.move_to(a(0, 1) - 1, y, false),
            'd' => self.move_to(x, a(0, 1) - 1, false),
            'H' | 'f' => self.move_to(a(1, 1) - 1, a(0, 1) - 1, false),
            'J' => self.erase_display(arg(0, 0)),
            'K' => self.erase_line(arg(0, 0)),
            'X' => self.fill(self.cursor_y, self.cursor_x, self.columns.min(self.cursor_x + arg(0, 1))),
            'P' => self.delete_chars(arg(0, 1)),
            '@' => self.insert_chars(arg(0, 1)),
            'L' => self.insert_lines(arg(0, 1)),
            'M' => self.delete_lines(arg(0, 1)),
            'S' => self.scroll_up(arg(0, 1)),
            'T' => self.scroll_down(arg(0, 1)),
            'm' => self.sgr(&numbers),
            'r' => {
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows) - 1;
                if top < bottom && bottom < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows - 1;
                }
                self.move_to(0, 0, false);
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            // The program asks where the cursor is / what kind of terminal this is.
            'n' if arg(0, 0) == 6 => self.replies.push(format!("\x1b[{};{}R", self.cursor_y + 1, self.cursor_x + 1)),
            'n' if arg(0, 0) == 5 => self.replies.push("\x1b[0n".into()),
            'c' => self.replies.push("\x1b[?1;0c".into()),
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: usize, on: bool) {
        match mode {
            1 => self.application_cursor_keys = on,
            25 => self.cursor_visible = on,
            2004 => self.bracketed_paste = on,
            47 | 1047 | 1049 => {
                if on && self.main_screen.is_none() {
                    let alternate = new_screen(self.columns, self.rows);
                    self.main_screen = Some(std::mem::replace(&mut self.screen, alternate));
                    self.main_cursor = (self.cursor_x, self.cursor_y);
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                } else if !on && let Some(main) = self.main_screen.take() {
                    self.screen = main;
                    (self.cursor_x, self.cursor_y) = self.main_cursor;
                }
                self.scroll_top = 0;
                self.scroll_bottom = self.rows - 1;
                self.wrap_pending = false;
            }
            _ => {}
        }
    }

    fn move_to(&mut self, x: isize, y: isize, within_region: bool) {
        self.cursor_x = x.clamp(0, self.columns as isize - 1) as usize;
        // Moving up or down stops at the scroll region's edge when the cursor is inside it.
        let (min, max) = if within_region && self.cursor_y >= self.scroll_top && self.cursor_y <= self.scroll_bottom {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows - 1)
        };
        self.cursor_y = y.clamp(min as isize, max as isize) as usize;
        self.wrap_pending = false;
    }

    fn fill(&mut self, y: usize, from: usize, to: usize) {
        let blank = self.blank();
        let to = to.min(self.columns);
        for cell in self.screen[y].iter_mut().take(to).skip(from) {
            *cell = blank;
        }
    }

    fn erase_line(&mut self, mode: usize) {
        self.wrap_pending = false;
        let (x, y, columns) = (self.cursor_x, self.cursor_y, self.columns);
        match mode {
            0 => self.fill(y, x, columns),
            1 => self.fill(y, 0, x + 1),
            2 => self.fill(y, 0, columns),
            _ => {}
        }
    }

    fn erase_display(&mut self, mode: usize) {
        self.wrap_pending = false;
        let (x, y, columns) = (self.cursor_x, self.cursor_y, self.columns);
        match mode {
            0 => {
                self.fill(y, x, columns);
                for row in y + 1..self.rows {
                    self.fill(row, 0, columns);
                }
            }
            1 => {
                for row in 0..y {
                    self.fill(row, 0, columns);
                }
                self.fill(y, 0, x + 1);
            }
            2 => {
                for row in 0..self.rows {
                    self.fill(row, 0, columns);
                }
            }
            3 => self.scrollback.clear(),
            _ => {}
        }
    }

    fn delete_chars(&mut self, count: usize) {
        let (x, y, columns) = (self.cursor_x, self.cursor_y, self.columns);
        let count = count.min(columns - x);
        self.screen[y].copy_within(x + count..columns, x);
        self.fill(y, columns - count, columns);
    }

    fn insert_chars(&mut self, count: usize) {
        let (x, y, columns) = (self.cursor_x, self.cursor_y, self.columns);
        let count = count.min(columns - x);
        self.screen[y].copy_within(x..columns - count, x + count);
        self.fill(y, x, x + count);
    }

    // Inserting / deleting lines shifts the rest of the scroll region, from the cursor's line down.
    fn insert_lines(&mut self, count: usize) {
        if self.cursor_y < self.scroll_top || self.cursor_y > self.scroll_bottom {
            return;
        }
        let top = self.scroll_top;
        self.scroll_top = self.cursor_y;
        self.scroll_down(count);
        self.scroll_top = top;
        self.cursor_x = 0;
    }

    fn delete_lines(&mut self, count: usize) {
        if self.cursor_y < self.scroll_top || self.cursor_y > self.scroll_bottom {
            return;
        }
        let top = self.scroll_top;
        self.scroll_top = self.cursor_y;
        // Deleted lines aren't history: keep them out of the scrollback.
        self.scroll_up_keeping(count, false);
        self.scroll_top = top;
        self.cursor_x = 0;
    }

    /// Colours and styles for what is printed next (SGR).
    fn sgr(&mut self, codes: &[usize]) {
        if codes.is_empty() {
            self.pen = Cell::default();
            return;
        }
        let mut i = 0;
        while i < codes.len() {
            let code = codes[i];
            let pen = &mut self.pen;
            match code {
                0 => *pen = Cell::default(),
                1 => pen.style.set(CellStyle::BOLD),
                2 => pen.style.set(CellStyle::DIM),
                3 => pen.style.set(CellStyle::ITALIC),
                4 => pen.style.set(CellStyle::UNDERLINE),
                7 => pen.style.set(CellStyle::INVERSE),
                22 => {
                    pen.style.clear(CellStyle::BOLD);
                    pen.style.clear(CellStyle::DIM);
                }
                23 => pen.style.clear(CellStyle::ITALIC),
                24 => pen.style.clear(CellStyle::UNDERLINE),
                27 => pen.style.clear(CellStyle::INVERSE),
                30..=37 => pen.fg = INDEXED | (code - 30) as u32,
                39 => pen.fg = 0,
                40..=47 => pen.bg = INDEXED | (code - 40) as u32,
                49 => pen.bg = 0,
                90..=97 => pen.fg = INDEXED | (code - 90 + 8) as u32,
                100..=107 => pen.bg = INDEXED | (code - 100 + 8) as u32,
                38 | 48 => {
                    // 38;5;n - palette colour n; 38;2;r;g;b - direct colour. 48 the same for the background.
                    let byte = |n: usize| n.min(255) as u32;
                    let colour = if i + 2 < codes.len() && codes[i + 1] == 5 {
                        i += 2;
                        INDEXED | byte(codes[i])
                    } else if i + 4 < codes.len() && codes[i + 1] == 2 {
                        let rgb = byte(codes[i + 2]) << 16 | byte(codes[i + 3]) << 8 | byte(codes[i + 4]);
                        i += 4;
                        RGB | rgb
                    } else {
                        i = codes.len(); // malformed - ignore the rest
                        0
                    };
                    if code == 38 {
                        pen.fg = colour;
                    } else {
                        pen.bg = colour;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn new_screen(columns: usize, rows: usize) -> Vec<Vec<Cell>> {
    vec![vec![Cell::default(); columns]; rows]
}

fn numbers(body: &str) -> Vec<usize> {
    let mut numbers = Vec::new();
    let mut value = 0usize;
    let mut any = false;
    for c in body.chars() {
        if let Some(digit) = c.to_digit(10) {
            value = (value * 10 + digit as usize).min(99999);
            any = true;
        } else if c == ';' || c == ':' {
            numbers.push(value);
            value = 0;
            any = true;
        }
    }
    if any {
        numbers.push(value);
    }
    numbers
}

/// A line's text: empty cells as spaces.
pub fn line_text(line: &[Cell]) -> String {
    line.iter().map(|cell| if cell.ch == '\0' { ' ' } else { cell.ch }).collect()
}

// The 16 ANSI colours: Windows Terminal's Campbell on the dark theme, One Half Light on the light one (the bright
// colours of a dark scheme can't be read on white).
const DARK_PALETTE: [u32; 16] = [
    0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC, 0x767676, 0xE74856, 0x16C60C,
    0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
];
const LIGHT_PALETTE: [u32; 16] = [
    0x383A42, 0xE45649, 0x50A14F, 0xC18301, 0x0184BC, 0xA626A4, 0x0997B3, 0x7A7F87, 0x4F525D, 0xDF6C75, 0x3E8E3D,
    0xA67101, 0x0B76C5, 0x9B2C9A, 0x0A8B9E, 0x23262B,
];

/// A cell colour (not 0, the default) as 0xRRGGBB - the 16 ANSI colours from the theme's palette, then the 6 x 6 x 6
/// cube and the greys of the 256-colour palette, or the direct colour.
pub fn colour_rgb(colour: u32, dark: bool) -> u32 {
    let index = colour & 0xFF_FFFF;
    if colour & RGB != 0 {
        return index;
    }
    let index = index as usize;
    if index < 16 {
        return if dark { DARK_PALETTE[index] } else { LIGHT_PALETTE[index] };
    }
    if index < 232 {
        let level = |n: usize| if n == 0 { 0 } else { (55 + n * 40) as u32 };
        let n = index - 16;
        return level(n / 36) << 16 | level(n / 6 % 6) << 8 | level(n % 6);
    }
    let grey = (8 + (index.min(255) - 232) * 10) as u32;
    grey << 16 | grey << 8 | grey
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buffer: &TerminalBuffer, y: usize) -> String {
        line_text(buffer.line(buffer.scrollback_len() + y)).trim_end().to_string()
    }

    #[test]
    fn prints_text_and_moves_the_cursor() {
        let mut b = TerminalBuffer::new(10, 3);
        b.feed("hi\r\nyou");
        assert_eq!(row(&b, 0), "hi");
        assert_eq!(row(&b, 1), "you");
        assert_eq!(b.cursor(), (3, 1));
    }

    #[test]
    fn wraps_at_the_last_column_only_when_the_next_character_comes() {
        let mut b = TerminalBuffer::new(4, 3);
        b.feed("abcd");
        assert_eq!(b.cursor(), (3, 0));
        b.feed("e");
        assert_eq!(row(&b, 0), "abcd");
        assert_eq!(row(&b, 1), "e");
        assert_eq!(b.cursor(), (1, 1));
    }

    #[test]
    fn lines_scrolling_off_the_top_go_to_the_scrollback() {
        let mut b = TerminalBuffer::new(10, 2);
        b.feed("one\r\ntwo\r\nthree");
        assert_eq!(b.scrollback_len(), 1);
        assert_eq!(line_text(b.line(0)).trim_end(), "one");
        assert_eq!(row(&b, 0), "two");
        assert_eq!(row(&b, 1), "three");
        assert_eq!(b.total_lines(), 3);
    }

    #[test]
    fn scrollback_keeps_the_newest_lines() {
        let mut b = TerminalBuffer::new(10, 1);
        for i in 0..MAX_SCROLLBACK + 10 {
            b.feed(&format!("{i}\r\n"));
        }
        assert_eq!(b.scrollback_len(), MAX_SCROLLBACK);
        assert_eq!(line_text(b.line(0)).trim_end(), "10");
    }

    #[test]
    fn cursor_position_erase_line_and_display() {
        // As PowerShell's PSReadLine redraws: ESC[H home, ESC[2J clear, ESC[K erase to the end of the line.
        let mut b = TerminalBuffer::new(10, 3);
        b.feed("aaaaaaaaaa\r\nbbbbbbbbbb\r\ncccccccccc");
        b.feed("\x1b[2;4H\x1b[K");
        assert_eq!(row(&b, 1), "bbb");
        b.feed("\x1b[1K");
        assert_eq!(line_text(b.line(1)), "          ");
        b.feed("\x1b[2J\x1b[H");
        assert_eq!(row(&b, 0), "");
        assert_eq!(row(&b, 2), "");
        assert_eq!(b.cursor(), (0, 0));
        b.feed("\x1b[3J");
        assert_eq!(b.scrollback_len(), 0);
    }

    #[test]
    fn relative_moves_and_columns() {
        let mut b = TerminalBuffer::new(20, 5);
        b.feed("\x1b[3;5H\x1b[2A\x1b[3C");
        assert_eq!(b.cursor(), (7, 0));
        b.feed("\x1b[10D\x1b[B");
        assert_eq!(b.cursor(), (0, 1));
        b.feed("\x1b[12G\x1b[4d");
        assert_eq!(b.cursor(), (11, 3));
        b.feed("\x1b[99;99H");
        assert_eq!(b.cursor(), (19, 4));
    }

    #[test]
    fn sgr_colours_and_styles() {
        let mut b = TerminalBuffer::new(20, 2);
        b.feed("\x1b[1;31mR\x1b[0m\x1b[38;5;200mP\x1b[48;2;1;2;3mD\x1b[39;49m\x1b[7;4mI\x1b[m.");
        let line = b.line(0);
        assert_eq!(line[0].fg, INDEXED | 1);
        assert!(line[0].style.has(CellStyle::BOLD));
        assert_eq!(line[1].fg, INDEXED | 200);
        assert_eq!(line[1].style, CellStyle::NONE);
        assert_eq!(line[2].bg, RGB | 0x010203);
        assert_eq!(line[2].fg, INDEXED | 200);
        assert!(line[3].style.has(CellStyle::INVERSE) && line[3].style.has(CellStyle::UNDERLINE));
        assert_eq!((line[3].fg, line[3].bg), (0, 0));
        assert_eq!(line[4], Cell { ch: '.', ..Cell::default() });
    }

    #[test]
    fn bright_colours_and_malformed_sgr() {
        let mut b = TerminalBuffer::new(10, 1);
        b.feed("\x1b[92;103mx\x1b[38;9;1my");
        let line = b.line(0);
        assert_eq!((line[0].fg, line[0].bg), (INDEXED | 10, INDEXED | 11));
        assert_eq!(line[1].fg, 0);
    }

    #[test]
    fn erased_cells_take_the_background() {
        let mut b = TerminalBuffer::new(4, 1);
        b.feed("\x1b[44m\x1b[2K");
        assert!(b.line(0).iter().all(|c| c.bg == INDEXED | 4 && c.ch == '\0'));
    }

    #[test]
    fn osc_title_and_charset_sequences_are_skipped() {
        let mut b = TerminalBuffer::new(20, 1);
        b.feed("\x1b]0;Administrator: C:\\WINDOWS\\system32\\cmd.exe\x07\x1b(Bok\x1b]2;t\x1b\\!");
        assert_eq!(row(&b, 0), "ok!");
    }

    #[test]
    fn private_modes() {
        let mut b = TerminalBuffer::new(10, 2);
        b.feed("\x1b[?25l\x1b[?1h\x1b[?2004h");
        assert!(!b.cursor_visible());
        assert!(b.application_cursor_keys());
        assert!(b.bracketed_paste());
        b.feed("\x1b[?25h\x1b[?1l\x1b[?2004l");
        assert!(b.cursor_visible() && !b.application_cursor_keys() && !b.bracketed_paste());
    }

    #[test]
    fn alternate_screen_keeps_the_main_one() {
        let mut b = TerminalBuffer::new(10, 2);
        b.feed("main");
        b.feed("\x1b[?1049h");
        assert_eq!(row(&b, 0), "");
        b.feed("vim\r\n\r\n\r\n");
        // Lines scrolling off the alternate screen aren't history.
        assert_eq!(b.scrollback_len(), 0);
        b.feed("\x1b[?1049l");
        assert_eq!(row(&b, 0), "main");
        assert_eq!(b.cursor(), (4, 0));
    }

    #[test]
    fn scroll_region_and_reverse_index() {
        let mut b = TerminalBuffer::new(5, 4);
        b.feed("1\r\n2\r\n3\r\n4");
        b.feed("\x1b[2;3r");
        assert_eq!(b.cursor(), (0, 0));
        b.feed("\x1b[3;1H\n");
        assert_eq!((row(&b, 0), row(&b, 1), row(&b, 2), row(&b, 3)), ("1".into(), "3".into(), "".into(), "4".into()));
        assert_eq!(b.scrollback_len(), 0);
        b.feed("\x1b[2;1H\x1bM");
        assert_eq!((row(&b, 1), row(&b, 2)), ("".into(), "3".into()));
    }

    #[test]
    fn insert_and_delete_characters_and_lines() {
        let mut b = TerminalBuffer::new(6, 3);
        b.feed("abcdef\x1b[1;2H\x1b[2P");
        assert_eq!(row(&b, 0), "adef");
        b.feed("\x1b[1;2H\x1b[1@");
        assert_eq!(row(&b, 0), "a def");
        b.feed("\x1b[1;3H\x1b[2X");
        assert_eq!(row(&b, 0), "a   f");
        b.feed("\x1b[2;1Hx\r\ny\x1b[2;1H\x1b[L");
        assert_eq!((row(&b, 1), row(&b, 2)), ("".into(), "x".into()));
        b.feed("\x1b[M\x1b[M");
        assert_eq!(b.scrollback_len(), 0);
        assert_eq!((row(&b, 0), row(&b, 1)), ("a   f".into(), "".into()));
    }

    #[test]
    fn cursor_save_restore_and_reports() {
        let mut b = TerminalBuffer::new(10, 3);
        b.feed("\x1b[2;3H\x1b7\x1b[H\x1b8");
        assert_eq!(b.cursor(), (2, 1));
        b.feed("\x1b[1;1H\x1b[s\x1b[3;3H\x1b[u");
        assert_eq!(b.cursor(), (0, 0));
        b.feed("\x1b[2;5H\x1b[6n\x1b[5n\x1b[c");
        assert_eq!(b.take_replies(), vec!["\x1b[2;5R".to_string(), "\x1b[0n".into(), "\x1b[?1;0c".into()]);
        assert!(b.take_replies().is_empty());
    }

    #[test]
    fn controls_tab_backspace() {
        let mut b = TerminalBuffer::new(20, 1);
        b.feed("a\tb\x08c");
        assert_eq!(row(&b, 0), "a       c");
        b.feed("\t\t\t");
        assert_eq!(b.cursor(), (19, 0));
    }

    #[test]
    fn a_sequence_split_over_two_feeds() {
        let mut b = TerminalBuffer::new(10, 1);
        b.feed("\x1b[3");
        b.feed("1mx");
        assert_eq!(b.line(0)[0].fg, INDEXED | 1);
        assert_eq!(row(&b, 0), "x");
    }

    #[test]
    fn resize_keeps_the_cursor_line_and_moves_the_top_to_the_scrollback() {
        let mut b = TerminalBuffer::new(10, 4);
        b.feed("1\r\n2\r\n3\r\n4");
        b.resize(5, 2);
        assert_eq!((b.columns(), b.rows()), (5, 2));
        assert_eq!(b.scrollback_len(), 2);
        assert_eq!((row(&b, 0), row(&b, 1)), ("3".into(), "4".into()));
        assert_eq!(b.cursor(), (1, 1));
        b.resize(1, 0);
        assert_eq!((b.columns(), b.rows()), (2, 1));
    }

    #[test]
    fn reset_clears_everything() {
        let mut b = TerminalBuffer::new(10, 2);
        b.feed("\x1b[31mx\x1b[?25l\x1bc");
        assert_eq!(row(&b, 0), "");
        assert!(b.cursor_visible());
        b.feed("y");
        assert_eq!(b.line(b.scrollback_len())[0].fg, 0);
    }

    #[test]
    fn palette_colours() {
        assert_eq!(colour_rgb(INDEXED | 1, true), 0xC50F1F);
        assert_eq!(colour_rgb(INDEXED | 1, false), 0xE45649);
        assert_eq!(colour_rgb(INDEXED | 16, true), 0x000000);
        assert_eq!(colour_rgb(INDEXED | 231, true), 0xFFFFFF);
        assert_eq!(colour_rgb(INDEXED | 232, true), 0x080808);
        assert_eq!(colour_rgb(RGB | 0x123456, true), 0x123456);
    }
}
