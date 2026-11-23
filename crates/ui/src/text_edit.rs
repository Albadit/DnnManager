//! A line of text being edited - its text, the selection, the IME's marked text and undo - apart from GPUI, so what
//! each key does is tested on any machine for every OS. [`crate::text_input`] draws it and binds the keys.
//!
//! Offsets are UTF-8 byte offsets on grapheme boundaries. The selection runs from an anchor (where it started) to the
//! caret; the caret is where typing goes and what Shift+arrow moves.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::keys::Os;

/// Where a key moves the caret to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// One character (grapheme).
    Left,
    Right,
    /// The start of the word before (Ctrl+Left; ⌥← on macOS).
    WordLeft,
    /// The next word (Ctrl+Right; ⌥→ on macOS): on Windows the start of the next word, on macOS and Linux the end of
    /// this one - as each one's own text boxes do.
    WordRight,
    /// The start or end of the line (Home / End; ⌘← / ⌘→ on macOS).
    LineStart,
    LineEnd,
}

/// The undo steps kept; the oldest goes first.
const UNDO_KEPT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    text: String,
    anchor: usize,
    caret: usize,
}

/// What the last edit was, so a run of typing (or of deleting) is one undo step, as in the OSes' own text boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Typing,
    Deleting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(grapheme: &str) -> Class {
    match grapheme.chars().next() {
        Some(c) if c.is_whitespace() => Class::Space,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        _ => Class::Other,
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextEdit {
    text: String,
    anchor: usize,
    caret: usize,
    /// The IME's composition, underlined while it lasts.
    marked: Option<Range<usize>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The run the last edit was part of, and where the caret was after it: the next edit of the same kind there
    /// joins the same undo step.
    run: Option<(Run, usize)>,
}

impl TextEdit {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The selection, start before end.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    /// Whether the caret is at the selection's start (it was made leftwards).
    pub fn reversed(&self) -> bool {
        self.caret < self.anchor
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    pub fn selected_text(&self) -> Option<&str> {
        let range = self.selection();
        (!range.is_empty()).then(|| &self.text[range])
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// New text from outside (not typed): the caret at its end, nothing to undo.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.anchor = self.text.len();
        self.caret = self.text.len();
        self.marked = None;
        self.undo.clear();
        self.redo.clear();
        self.run = None;
    }

    // ─── Moving and selecting ──────────────────────────────────────────

    /// Moves the caret, leaving no selection. Left / Right with a selection go to its start / end, as everywhere.
    pub fn move_caret(&mut self, to: Move, os: Os) {
        let range = self.selection();
        let at = match to {
            Move::Left if !range.is_empty() => range.start,
            Move::Right if !range.is_empty() => range.end,
            _ => self.target(to, os),
        };
        self.set_caret(at);
    }

    /// Moves the caret and keeps the anchor: Shift with a move key.
    pub fn select(&mut self, to: Move, os: Os) {
        let at = self.target(to, os);
        self.select_to(at);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
        self.run = None;
    }

    /// A click: the caret at `offset`, no selection.
    pub fn set_caret(&mut self, offset: usize) {
        let offset = self.boundary_at(offset);
        self.anchor = offset;
        self.caret = offset;
        self.run = None;
    }

    /// A drag or a Shift+click: from the anchor to `offset`.
    pub fn select_to(&mut self, offset: usize) {
        self.caret = self.boundary_at(offset);
        self.run = None;
    }

    /// A double-click: the word at `offset` (or the run of spaces or of punctuation there).
    pub fn select_word_at(&mut self, offset: usize) {
        let graphemes: Vec<(usize, &str)> = self.text.grapheme_indices(true).collect();
        if graphemes.is_empty() {
            return;
        }
        // The grapheme the offset is in or before - the last one at the end of the text.
        let i = graphemes.iter().rposition(|(at, _)| *at <= offset).unwrap_or(0).min(graphemes.len() - 1);
        let c = class(graphemes[i].1);
        let mut start = i;
        while start > 0 && class(graphemes[start - 1].1) == c {
            start -= 1;
        }
        let mut end = i + 1;
        while end < graphemes.len() && class(graphemes[end].1) == c {
            end += 1;
        }
        self.anchor = graphemes[start].0;
        self.caret = graphemes.get(end).map_or(self.text.len(), |g| g.0);
        self.run = None;
    }

    // ─── Editing ───────────────────────────────────────────────────────

    /// Backspace, Delete and their word and line kin: the selection when there is one, else from the caret to where
    /// `to` goes. False when there was nothing to delete.
    pub fn delete(&mut self, to: Move, os: Os) -> bool {
        let mut range = self.selection();
        if range.is_empty() {
            let at = self.target(to, os);
            range = self.caret.min(at)..self.caret.max(at);
        }
        if range.is_empty() {
            return false;
        }
        self.edit(range, "", Some(Run::Deleting), true);
        true
    }

    /// Typing, or the IME's text: `text` in place of `range` (None: the marked text, else the selection).
    pub fn replace(&mut self, range: Option<Range<usize>>, text: &str) {
        let composing = self.marked.is_some();
        let range = range.or_else(|| self.marked.clone()).unwrap_or_else(|| self.selection());
        let range = self.clamp(range);
        if composing {
            // The IME's composition made its undo step when it started.
            self.apply(range, text);
            self.run = None;
        } else if !(text.is_empty() && range.is_empty()) {
            // A run of typing is one undo step; a space starts the next one, as a word is undone at a time.
            let joins = range.is_empty() && !text.starts_with(char::is_whitespace);
            self.edit(range, text, (!text.is_empty()).then_some(Run::Typing), joins);
        }
        self.marked = None;
    }

    /// A paste: `text` in place of the selection, an undo step of its own.
    pub fn paste(&mut self, text: &str) {
        let range = self.marked.take().unwrap_or_else(|| self.selection());
        let range = self.clamp(range);
        if !(text.is_empty() && range.is_empty()) {
            self.edit(range, text, None, false);
        }
    }

    /// The IME's composition: `text` in place of `range` (or of what it marked before, or of the selection), marked;
    /// `selected` is the selection within `text`.
    pub fn replace_and_mark(&mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>) {
        let range = range.or_else(|| self.marked.clone()).unwrap_or_else(|| self.selection());
        let range = self.clamp(range);
        if self.marked.is_none() {
            self.push_undo();
        }
        self.apply(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        match selected {
            Some(selected) => {
                let start = self.boundary_at(range.start + selected.start.min(text.len()));
                let end = self.boundary_at(range.start + selected.end.min(text.len()));
                self.anchor = start;
                self.caret = end;
            }
            None => {
                self.anchor = range.start + text.len();
                self.caret = self.anchor;
            }
        }
        self.run = None;
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// Cut: the selected text, taken out; None when nothing is selected.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected_text()?.to_string();
        self.edit(self.selection(), "", None, false);
        Some(text)
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else { return false };
        let now = self.snapshot();
        self.redo.push(now);
        self.restore(previous);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        let now = self.snapshot();
        self.undo.push(now);
        self.restore(next);
        true
    }

    // ─── Inside ────────────────────────────────────────────────────────

    /// `text` in place of `range` as one undo step - or, when it `may_join`, as part of the last one if it continues
    /// that one's run where it stopped.
    fn edit(&mut self, range: Range<usize>, text: &str, run: Option<Run>, may_join: bool) {
        let continues = may_join
            && matches!((run, self.run), (Some(kind), Some((last, at))) if kind == last && at == self.caret);
        if !continues {
            self.push_undo();
        }
        self.apply(range, text);
        self.run = run.map(|kind| (kind, self.caret));
    }

    fn apply(&mut self, range: Range<usize>, text: &str) {
        self.text.replace_range(range.clone(), text);
        self.anchor = range.start + text.len();
        self.caret = self.anchor;
    }

    fn push_undo(&mut self) {
        let snapshot = self.snapshot();
        self.undo.push(snapshot);
        if self.undo.len() > UNDO_KEPT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { text: self.text.clone(), anchor: self.anchor, caret: self.caret }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.anchor = snapshot.anchor;
        self.caret = snapshot.caret;
        self.marked = None;
        self.run = None;
    }

    /// `offset` inside the text and on a character's start (an IME or a stale layout may give one that isn't).
    fn boundary_at(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let (start, end) = (self.boundary_at(range.start), self.boundary_at(range.end));
        start.min(end)..start.max(end)
    }

    /// Where `to` goes from the caret.
    fn target(&self, to: Move, os: Os) -> usize {
        let at = self.caret;
        match to {
            Move::Left => self.text[..at].grapheme_indices(true).next_back().map_or(0, |(i, _)| i),
            Move::Right => self.text[at..].graphemes(true).next().map_or(at, |g| at + g.len()),
            Move::WordLeft => self.word_start_before(at),
            Move::WordRight if os == Os::Windows => self.next_word_start(at),
            Move::WordRight => self.word_end_after(at),
            Move::LineStart => 0,
            Move::LineEnd => self.text.len(),
        }
    }

    /// Back over spaces, then over the word (or run of punctuation) before them.
    fn word_start_before(&self, offset: usize) -> usize {
        let graphemes: Vec<(usize, &str)> = self.text[..offset].grapheme_indices(true).collect();
        let mut i = graphemes.len();
        while i > 0 && class(graphemes[i - 1].1) == Class::Space {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let c = class(graphemes[i - 1].1);
        while i > 0 && class(graphemes[i - 1].1) == c {
            i -= 1;
        }
        graphemes.get(i).map_or(0, |g| g.0)
    }

    /// Over spaces, then to the end of the word after them (macOS, Linux).
    fn word_end_after(&self, offset: usize) -> usize {
        let rest: Vec<(usize, &str)> = self.text[offset..].grapheme_indices(true).collect();
        let mut i = 0;
        while i < rest.len() && class(rest[i].1) == Class::Space {
            i += 1;
        }
        if let Some(&(_, g)) = rest.get(i) {
            let c = class(g);
            while i < rest.len() && class(rest[i].1) == c {
                i += 1;
            }
        }
        rest.get(i).map_or(self.text.len(), |g| offset + g.0)
    }

    /// To the end of this word, then over the spaces after it: the next word's start (Windows).
    fn next_word_start(&self, offset: usize) -> usize {
        let rest: Vec<(usize, &str)> = self.text[offset..].grapheme_indices(true).collect();
        let mut i = 0;
        if let Some(&(_, g)) = rest.first() {
            let c = class(g);
            if c != Class::Space {
                while i < rest.len() && class(rest[i].1) == c {
                    i += 1;
                }
            }
        }
        while i < rest.len() && class(rest[i].1) == Class::Space {
            i += 1;
        }
        rest.get(i).map_or(self.text.len(), |g| offset + g.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str, caret: usize) -> TextEdit {
        let mut e = TextEdit::default();
        e.set_text(text);
        e.set_caret(caret);
        e
    }

    #[test]
    fn arrows_move_by_character_and_collapse_a_selection() {
        let mut e = edit("héllo", 0);
        e.move_caret(Move::Right, Os::Windows);
        e.move_caret(Move::Right, Os::Windows);
        assert_eq!(e.caret(), 3, "é is two bytes, one step");
        e.select(Move::Right, Os::Mac);
        assert_eq!(e.selection(), 3..4);
        e.move_caret(Move::Left, Os::Mac);
        assert_eq!((e.selection(), e.caret()), (3..3, 3), "Left with a selection goes to its start");
        e.move_caret(Move::LineEnd, Os::Linux);
        e.move_caret(Move::Right, Os::Linux);
        assert_eq!(e.caret(), 6, "nothing past the end");
        e.move_caret(Move::LineStart, Os::Linux);
        e.move_caret(Move::Left, Os::Linux);
        assert_eq!(e.caret(), 0, "nothing before the start");
    }

    #[test]
    fn a_family_emoji_is_one_step() {
        let family = "👨‍👩‍👧";
        let mut e = edit(&format!("a{family}b"), 1);
        e.move_caret(Move::Right, Os::Mac);
        assert_eq!(e.caret(), 1 + family.len());
        assert!(e.delete(Move::Left, Os::Mac));
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn word_moves_follow_each_os() {
        let text = "hello wide, world";
        // Left: the start of the word before, on every OS.
        for os in Os::ALL {
            let mut e = edit(text, text.len());
            e.move_caret(Move::WordLeft, os);
            assert_eq!(e.caret(), 12, "{os:?}");
            e.move_caret(Move::WordLeft, os);
            assert_eq!(e.caret(), 10, "{os:?}: the comma is a word of its own");
            e.move_caret(Move::WordLeft, os);
            assert_eq!(e.caret(), 6, "{os:?}");
        }
        // Right: Windows goes to the next word's start, macOS and Linux to this word's end.
        let mut windows = edit(text, 0);
        windows.move_caret(Move::WordRight, Os::Windows);
        assert_eq!(windows.caret(), 6);
        for os in [Os::Mac, Os::Linux] {
            let mut e = edit(text, 0);
            e.move_caret(Move::WordRight, os);
            assert_eq!(e.caret(), 5, "{os:?}");
            e.move_caret(Move::WordRight, os);
            assert_eq!(e.caret(), 10, "{os:?}");
        }
        let mut end = edit(text, text.len() - 2);
        end.move_caret(Move::WordRight, Os::Windows);
        assert_eq!(end.caret(), text.len());
    }

    #[test]
    fn shift_with_a_move_selects_from_the_anchor() {
        let mut e = edit("one two three", 4);
        e.select(Move::WordRight, Os::Mac);
        assert_eq!(e.selected_text(), Some("two"));
        e.select(Move::LineStart, Os::Mac);
        assert_eq!((e.selected_text(), e.reversed()), (Some("one "), true), "back past the anchor: reversed");
        e.select(Move::LineEnd, Os::Windows);
        assert_eq!(e.selected_text(), Some("two three"));
        e.select_all();
        assert_eq!(e.selected_text(), Some("one two three"));
    }

    #[test]
    fn deleting_by_character_word_and_line() {
        let mut e = edit("C:\\inetpub\\wwwroot", 18);
        assert!(e.delete(Move::WordLeft, Os::Windows));
        assert_eq!(e.text(), "C:\\inetpub\\");
        assert!(e.delete(Move::WordLeft, Os::Windows));
        assert_eq!(e.text(), "C:\\inetpub", "a backslash is a word of its own");
        let mut e = edit("one two", 0);
        assert!(e.delete(Move::WordRight, Os::Windows));
        assert_eq!(e.text(), "two", "Ctrl+Delete takes the space too");
        let mut e = edit("one two", 0);
        assert!(e.delete(Move::WordRight, Os::Mac));
        assert_eq!(e.text(), " two", "⌥⌦ stops at the word's end");
        let mut e = edit("one two", 4);
        assert!(e.delete(Move::LineStart, Os::Mac));
        assert_eq!((e.text(), e.caret()), ("two", 0), "⌘⌫");
        assert!(!e.delete(Move::Left, Os::Mac), "nothing before the start");
        e.set_caret(1);
        e.select(Move::Right, Os::Mac);
        assert!(e.delete(Move::WordLeft, Os::Mac));
        assert_eq!(e.text(), "to", "a selection is deleted whatever the key");
    }

    #[test]
    fn typing_replaces_the_selection_and_paste_takes_its_place() {
        let mut e = edit("Default Website", 0);
        e.select_word_at(9);
        assert_eq!(e.selected_text(), Some("Website"));
        e.replace(None, "Site");
        assert_eq!((e.text(), e.caret()), ("Default Site", 12));
        e.select_word_at(7);
        assert_eq!(e.selected_text(), Some(" "), "a double-click on a space takes the spaces");
        e.select_word_at(100);
        assert_eq!(e.selected_text(), Some("Site"), "past the end: the last word");
        assert_eq!(e.cut().as_deref(), Some("Site"));
        assert_eq!(e.text(), "Default ");
        assert_eq!(e.cut(), None, "nothing selected, nothing cut");
        // A paste is a step of its own, also amid typing.
        e.replace(None, "x");
        e.paste("Site");
        e.replace(None, "y");
        assert_eq!(e.text(), "Default xSitey");
        e.undo();
        e.undo();
        assert_eq!(e.text(), "Default x");
    }

    #[test]
    fn undo_takes_back_a_run_of_typing_at_once_and_redo_puts_it_back() {
        let mut e = TextEdit::default();
        for c in ["m", "y"] {
            e.replace(None, c);
        }
        for c in [" ", "s", "i", "t", "e"] {
            e.replace(None, c);
        }
        assert_eq!(e.text(), "my site");
        assert!(e.undo());
        assert_eq!(e.text(), "my", "the second word was its own step");
        assert!(e.undo());
        assert_eq!(e.text(), "");
        assert!(!e.undo());
        assert!(e.redo() && e.redo());
        assert_eq!((e.text(), e.caret()), ("my site", 7));
        assert!(!e.redo());
        // Backspaces in a row are one step; an edit after an undo drops what could be redone.
        e.delete(Move::Left, Os::Windows);
        e.delete(Move::Left, Os::Windows);
        assert_eq!(e.text(), "my si");
        assert!(e.undo());
        assert_eq!(e.text(), "my site");
        e.undo();
        e.replace(None, "x");
        assert!(!e.can_redo());
        // Moving the caret ends a run.
        let mut e = TextEdit::default();
        e.replace(None, "a");
        e.move_caret(Move::LineStart, Os::Mac);
        e.replace(None, "b");
        e.undo();
        assert_eq!(e.text(), "a");
    }

    #[test]
    fn set_text_starts_afresh() {
        let mut e = TextEdit::default();
        e.replace(None, "typed");
        e.set_text("given");
        assert_eq!((e.text(), e.caret(), e.can_undo()), ("given", 5, false));
    }

    #[test]
    fn an_ime_composition_is_marked_and_one_undo_step() {
        let mut e = edit("ab", 1);
        // macOS's dead key: ⌥E marks "´", then E replaces it with "é".
        e.replace_and_mark(None, "´", Some(2..2));
        assert_eq!((e.text(), e.marked(), e.caret()), ("a´b", Some(1..3), 3));
        e.replace_and_mark(None, "ké", Some(0..3));
        assert_eq!((e.text(), e.selection()), ("akéb", 1..4), "the selection is within the new text");
        e.replace(None, "é");
        assert_eq!((e.text(), e.marked(), e.caret()), ("aéb", None, 3));
        assert!(e.undo());
        assert_eq!(e.text(), "ab");
        // A range from an IME past the end, or inside a character, is kept inside the text.
        let mut e = edit("é", 0);
        e.replace(Some(1..9), "e");
        assert_eq!(e.text(), "e");
        e.replace_and_mark(Some(0..0), "x", Some(5..9));
        assert_eq!(e.selection(), 1..1);
    }

    #[test]
    fn undo_keeps_a_hundred_steps() {
        let mut e = TextEdit::default();
        for i in 0..150 {
            e.replace(None, " ");
            e.replace(None, &i.to_string());
        }
        let mut steps = 0;
        while e.undo() {
            steps += 1;
        }
        assert_eq!(steps, UNDO_KEPT);
    }
}
