//! Text that can be selected across lines - the Output's and the Logs' lines, wrapped in their width. GPUI 0.2.2 has
//! no selectable text: each line is a `StyledText` whose layout is noted once it is painted (a canvas after it, so
//! its bounds are known), the mouse is mapped to a line and a character through those layouts, and the selection is
//! drawn as a highlight in the text itself.
//!
//! Drag selects (from line to line), Shift+click extends, a double-click takes a word, a triple-click the line; the
//! view binds Copy and Select all (Ctrl / ⌘ + C, A) and a right-click menu to [`LinesSelection::copied`] and
//! [`LinesSelection::select_all`].

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{App, HighlightStyle, IntoElement, ParentElement, Pixels, Point, SharedString, Styled, StyledText, TextLayout, canvas, div};

use crate::theme::Theme;

/// A place in the text: its line and the byte in it.
pub type Place = (usize, usize);

#[derive(Default)]
struct Inner {
    anchor: Option<Place>,
    head: Option<Place>,
    selecting: bool,
    /// The lines painted last frame, with their layouts.
    painted: BTreeMap<usize, TextLayout>,
    /// What the selection is of (a run, a log): another one clears it.
    key: Option<String>,
}

/// A selection over numbered lines - cheap to clone into the handlers of the view that draws them.
#[derive(Clone, Default)]
pub struct LinesSelection(Rc<RefCell<Inner>>);

/// The selection's ends, in order.
pub fn ordered(a: Place, b: Place) -> (Place, Place) {
    if a <= b { (a, b) } else { (b, a) }
}

/// The part of line `line` (`len` bytes) inside the selection from `start` to `end`.
pub fn range_on(start: Place, end: Place, line: usize, len: usize) -> Option<Range<usize>> {
    if line < start.0 || line > end.0 {
        return None;
    }
    let from = if line == start.0 { start.1.min(len) } else { 0 };
    let to = if line == end.0 { end.1.min(len) } else { len };
    (from < to || (line != end.0 && from == to)).then_some(from..to)
}

/// The text from `start` to `end`, lines joined by a line break.
pub fn text_between(start: Place, end: Place, text_of: &dyn Fn(usize) -> Option<SharedString>) -> String {
    let mut out = Vec::new();
    for line in start.0..=end.0 {
        let Some(text) = text_of(line) else { continue };
        if let Some(range) = range_on(start, end, line, text.len()) {
            out.push(text.get(range).unwrap_or_default().to_string());
        }
    }
    out.join("\n")
}

/// The word at `at` in `text`: letters, digits and `_`, or else the run of one other character class.
pub fn word_at(text: &str, at: usize) -> Range<usize> {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let Some(i) = chars.iter().rposition(|(b, _)| *b <= at) else { return 0..0 };
    let k = class(chars[i].1);
    let mut start = i;
    while start > 0 && class(chars[start - 1].1) == k {
        start -= 1;
    }
    let mut end = i + 1;
    while end < chars.len() && class(chars[end].1) == k {
        end += 1;
    }
    chars[start].0..chars.get(end).map_or(text.len(), |c| c.0)
}

/// `marks` with `selection` on top: the marks' parts outside it kept, sorted and not overlapping - as GPUI wants them.
fn merged(
    marks: Vec<(Range<usize>, HighlightStyle)>,
    selection: Option<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some((sel, style)) = selection else { return marks };
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for (r, s) in marks {
        if r.end <= sel.start || r.start >= sel.end {
            out.push((r, s));
            continue;
        }
        if r.start < sel.start {
            out.push((r.start..sel.start, s));
        }
        if r.end > sel.end {
            out.push((sel.end..r.end, s));
        }
    }
    if sel.start < sel.end {
        out.push((sel, style));
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

impl LinesSelection {
    /// A new frame: the lines are noted again as they are painted. `key` names what is shown - another one than the
    /// last clears the selection.
    pub fn begin(&self, key: &str) {
        let mut inner = self.0.borrow_mut();
        inner.painted.clear();
        if inner.key.as_deref() != Some(key) {
            inner.key = Some(key.to_string());
            inner.anchor = None;
            inner.head = None;
            inner.selecting = false;
        }
    }

    /// Line `line` as an element: its `text` with its `marks` (Find's) and the selection's background, wrapping in
    /// its width, noted where it is painted.
    pub fn line(
        &self,
        line: usize,
        text: SharedString,
        marks: Vec<(Range<usize>, HighlightStyle)>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let selected = {
            let inner = self.0.borrow();
            match (inner.anchor, inner.head) {
                (Some(a), Some(h)) if a != h => {
                    let (start, end) = ordered(a, h);
                    range_on(start, end, line, text.len()).filter(|r| r.start < r.end)
                }
                _ => None,
            }
        };
        let style = HighlightStyle { background_color: Some(Theme::get(cx).colors.accent.opacity(0.35)), ..Default::default() };
        let styled = StyledText::new(text).with_highlights(merged(marks, selected.map(|r| (r, style))));
        let layout = styled.layout().clone();
        let painted = self.clone();
        div()
            .relative()
            .w_full()
            .child(styled)
            .child(
                canvas(|_, _, _| {}, move |_, _, _, _| {
                    painted.0.borrow_mut().painted.insert(line, layout.clone());
                })
                .absolute()
                .size_0(),
            )
    }

    /// The place under `position`: on a painted line, else the nearest end of the lines painted.
    pub fn hit(&self, position: Point<Pixels>) -> Option<Place> {
        let inner = self.0.borrow();
        let (first, _) = inner.painted.iter().next()?;
        let mut nearest: Option<Place> = None;
        for (&line, layout) in &inner.painted {
            let bounds = layout.bounds();
            if position.y < bounds.top() {
                return Some(nearest.unwrap_or((*first, 0)));
            }
            if position.y <= bounds.bottom() {
                let at = layout.index_for_position(position).unwrap_or_else(|i| i);
                return Some((line, at.min(layout.len())));
            }
            nearest = Some((line, layout.len()));
        }
        nearest
    }

    /// A press: a click places the start (Shift: extends to here), a double-click takes the word, a triple-click the
    /// line. `text_of` gives a line's text.
    pub fn press(&self, position: Point<Pixels>, shift: bool, clicks: usize, text_of: &dyn Fn(usize) -> Option<SharedString>) {
        let Some(place) = self.hit(position) else { return };
        let mut inner = self.0.borrow_mut();
        match clicks {
            2 | 3 => {
                let text = text_of(place.0).unwrap_or_default();
                let range = if clicks == 2 { word_at(&text, place.1) } else { 0..text.len() };
                inner.anchor = Some((place.0, range.start));
                inner.head = Some((place.0, range.end));
                inner.selecting = false;
            }
            _ if shift && inner.anchor.is_some() => {
                inner.head = Some(place);
                inner.selecting = true;
            }
            _ => {
                inner.anchor = Some(place);
                inner.head = Some(place);
                inner.selecting = true;
            }
        }
    }

    /// The mouse moved with the button held: the selection follows. Whether it changed.
    pub fn drag(&self, position: Point<Pixels>) -> bool {
        if !self.0.borrow().selecting {
            return false;
        }
        let Some(place) = self.hit(position) else { return false };
        let mut inner = self.0.borrow_mut();
        let changed = inner.head != Some(place);
        inner.head = Some(place);
        changed
    }

    pub fn release(&self) {
        self.0.borrow_mut().selecting = false;
    }

    pub fn is_selecting(&self) -> bool {
        self.0.borrow().selecting
    }

    pub fn has_selection(&self) -> bool {
        let inner = self.0.borrow();
        matches!((inner.anchor, inner.head), (Some(a), Some(h)) if a != h)
    }

    /// Everything: `lines` lines, the last `last_len` bytes long.
    pub fn select_all(&self, lines: usize, last_len: usize) {
        if lines == 0 {
            return;
        }
        let mut inner = self.0.borrow_mut();
        inner.anchor = Some((0, 0));
        inner.head = Some((lines - 1, last_len));
        inner.selecting = false;
    }

    pub fn clear(&self) {
        let mut inner = self.0.borrow_mut();
        inner.anchor = None;
        inner.head = None;
        inner.selecting = false;
    }

    /// The selected text, lines joined by line breaks; None without a selection.
    pub fn copied(&self, text_of: &dyn Fn(usize) -> Option<SharedString>) -> Option<String> {
        let inner = self.0.borrow();
        match (inner.anchor, inner.head) {
            (Some(a), Some(h)) if a != h => {
                let (start, end) = ordered(a, h);
                Some(text_between(start, end, text_of))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(i: usize) -> Option<SharedString> {
        ["first line", "second", "", "fourth line here"].get(i).map(|s| SharedString::from(*s))
    }

    #[test]
    fn a_selection_spans_lines_in_either_direction() {
        let (start, end) = ordered((3, 6), (0, 6));
        assert_eq!((start, end), ((0, 6), (3, 6)));
        assert_eq!(range_on(start, end, 0, 10), Some(6..10));
        assert_eq!(range_on(start, end, 1, 6), Some(0..6));
        assert_eq!(range_on(start, end, 2, 0), Some(0..0), "an empty line inside counts");
        assert_eq!(range_on(start, end, 3, 16), Some(0..6));
        assert_eq!(range_on(start, end, 4, 3), None);
        assert_eq!(text_between(start, end, &lines), "line\nsecond\n\nfourth");
        assert_eq!(text_between((1, 1), (1, 4), &lines), "eco");
    }

    #[test]
    fn a_double_click_takes_the_word() {
        let text = "Using DNN 10.4.1 (https://github.com)";
        assert_eq!(&text[word_at(text, 7)], "DNN");
        assert_eq!(&text[word_at(text, 17)], "(");
        assert_eq!(&text[word_at(text, 20)], "https");
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn the_selection_goes_over_find_marks() {
        let mark = HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() };
        let sel = HighlightStyle { font_weight: Some(gpui::FontWeight::THIN), ..Default::default() };
        let out = merged(vec![(0..4, mark), (6..10, mark)], Some((2..8, sel)));
        let ranges: Vec<Range<usize>> = out.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [0..2, 2..8, 8..10], "sorted, not overlapping");
        assert_eq!(merged(vec![(0..4, mark)], None).len(), 1);
    }

    #[test]
    fn select_all_and_clear() {
        let s = LinesSelection::default();
        assert!(!s.has_selection());
        s.select_all(4, 16);
        assert_eq!(s.copied(&lines).unwrap(), "first line\nsecond\n\nfourth line here");
        s.clear();
        assert_eq!(s.copied(&lines), None);
        s.begin("run 1");
        s.select_all(4, 16);
        s.begin("run 1");
        assert!(s.has_selection(), "the same run keeps it");
        s.begin("run 2");
        assert!(!s.has_selection(), "another run clears it");
    }
}
