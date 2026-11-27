//! The Source Control page's main view, apart from the page's state: a file's two copies as VS Code's diff editor
//! shows them - side by side in two equal halves (the server's left, the project's right), each with its line
//! numbers, a changed line's own words marked inside it, a blank where the other side has no line, and the overview
//! ruler on the right edge with every change's place (a click goes there) - or, when only one side has the file,
//! that copy alone as an editor shows it. The line between the halves is dragged to make one wider; a sideways wheel
//! (Shift+wheel, a touchpad) or a half's scroll bar moves both halves sideways together.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use dnnm_application::source_control::diff::{Line, RowKind, SideBySide, side_by_side, word_changes};
use dnnm_application::source_control::highlight::{Token, highlight, language_of};
use dnnm_ui::{Pad, px};
use dnnm_ui::theme::Theme;
use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, CursorStyle, Hsla, HighlightStyle, Pixels, ScrollStrategy, SharedString,
    Stateful, StyledText, UniformListScrollHandle, canvas, div, prelude::*, relative, uniform_list,
};

/// Each line's coloured pieces (index: line number - 1).
type Spans = Vec<Vec<(Range<usize>, Token)>>;

/// A text diff, with each side's lines coloured and a changed line's differing words.
pub struct TextDiff {
    pub sides: SideBySide,
    /// Each side's widest line, in characters - how far the halves scroll sideways.
    pub widest: (usize, usize),
    left: Arc<Spans>,
    right: Arc<Spans>,
    /// The differing words of a changed line, by its number on its side.
    left_words: Arc<HashMap<usize, Vec<Range<usize>>>>,
    right_words: Arc<HashMap<usize, Vec<Range<usize>>>>,
}

impl TextDiff {
    pub fn new(path: &str, left: &str, right: &str) -> Self {
        let sides = side_by_side(left, right);
        let language = language_of(path);
        let (l, r): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
        let (mut left_words, mut right_words) = (HashMap::new(), HashMap::new());
        for row in sides.rows.iter().filter(|r| r.kind == RowKind::Changed) {
            if let (Some(a), Some(b)) = (&row.left, &row.right) {
                let (wl, wr) = word_changes(&a.text, &b.text);
                left_words.insert(a.number, wl);
                right_words.insert(b.number, wr);
            }
        }
        let widest = |lines: &[&str]| lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        Self {
            widest: (widest(&l), widest(&r)),
            left: Arc::new(highlight(language, &l)),
            right: Arc::new(highlight(language, &r)),
            left_words: Arc::new(left_words),
            right_words: Arc::new(right_words),
            sides,
        }
    }

    /// The rows where a change starts (F7).
    pub fn changes(&self) -> &[usize] {
        &self.sides.changes
    }

    /// Only the line endings or nothing at all differ.
    pub fn same_text(&self) -> bool {
        self.sides.changes.is_empty()
    }
}

/// One copy of a file, when the other side has none: its lines, coloured.
pub struct OneFile {
    lines: Arc<Vec<String>>,
    spans: Arc<Spans>,
    /// The server's copy (else the project's).
    pub server: bool,
    /// Its widest line, in characters.
    pub widest: usize,
}

impl OneFile {
    pub fn new(path: &str, text: &str, server: bool) -> Self {
        let lines: Vec<&str> = text.lines().collect();
        let spans = highlight(language_of(path), &lines);
        let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        Self { lines: Arc::new(lines.into_iter().map(str::to_string).collect()), spans: Arc::new(spans), server, widest }
    }
}

/// The line number column's width.
pub const NUMBER_WIDTH: f32 = 48.0;
/// Space either side of a line's code.
const CODE_PAD: f32 = 4.0;
/// The narrowest a half gets when the line between them is dragged, as a share of the width.
const SPLIT_MIN: f32 = 0.15;
/// A sideways scroll bar's height, and its thumb's least width.
const HBAR_HEIGHT: f32 = 10.0;
const THUMB_MIN: f32 = 24.0;

/// Dragged: the line between the two halves.
#[derive(Debug, Clone, Copy)]
pub struct SplitDrag;

/// Dragged: a half's sideways scroll bar (`left`: the server's).
#[derive(Debug, Clone, Copy)]
pub struct HScrollDrag {
    pub left: bool,
}

/// Where the line between the halves may go.
pub fn clamp_split(split: f32) -> f32 {
    split.clamp(SPLIT_MIN, 1.0 - SPLIT_MIN)
}

/// The two halves' widths in an area `width` wide.
pub fn halves(width: Pixels, split: f32) -> (Pixels, Pixels) {
    let left = width * split;
    (left, width - left)
}

/// How far a half `half` wide can scroll sideways: its widest line (`widest` characters) past the code it shows.
pub fn overflow(widest: usize, char_width: Pixels, half: Pixels) -> Pixels {
    let content = char_width * widest as f32 + px(CODE_PAD * 2.0);
    (content - (half - px(NUMBER_WIDTH))).max(Pixels::ZERO)
}

/// A half's sideways scroll bar: the thumb's left edge in the track (the code's width) and its width - None when the
/// half has nothing to scroll.
pub fn thumb(widest: usize, char_width: Pixels, half: Pixels, scroll_x: Pixels) -> Option<(Pixels, Pixels)> {
    let max = overflow(widest, char_width, half);
    let track = half - px(NUMBER_WIDTH);
    if max <= Pixels::ZERO || track <= Pixels::ZERO {
        return None;
    }
    let width = (track * (track / (track + max))).max(px(THUMB_MIN)).min(track);
    Some(((track - width) * (scroll_x.min(max) / max), width))
}

/// How the halves are laid out: where the line between them is (the left's share), how far both are scrolled
/// sideways (the same for both, as VS Code keeps them), the width of a character, the labels over them, and where
/// the area's bounds are put each frame (for the page's drags and scrolls).
pub struct Layout {
    pub split: f32,
    pub scroll_x: Pixels,
    pub char_width: Pixels,
    pub labels: (String, String),
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// The line number column.
fn number(n: Option<usize>, c: Hsla) -> gpui::Div {
    div()
        .w(px(NUMBER_WIDTH))
        .flex_none()
        .pad(px(0.0), px(6.0))
        .flex()
        .justify_end()
        .text_color(c)
        .child(n.map(|n| n.to_string()).unwrap_or_default())
}

/// A line's code, moved `scroll_x` to the left and cut off at the half's edge.
fn code(scroll_x: Pixels, line: StyledText) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .child(div().relative().left(-scroll_x).pad(px(0.0), px(CODE_PAD)).whitespace_nowrap().child(line))
}

/// A half's sideways scroll bar along its bottom, its thumb dragged with [`HScrollDrag`].
fn hbar(left: bool, x: Pixels, half: Pixels, widest: usize, layout: &Layout, cx: &App) -> Option<AnyElement> {
    let (thumb_left, width) = thumb(widest, layout.char_width, half, layout.scroll_x)?;
    let c = Theme::get(cx).colors;
    Some(
        div()
            .id(if left { "sc-hbar-left" } else { "sc-hbar-right" })
            .absolute()
            .bottom_0()
            .left(x + px(NUMBER_WIDTH) + thumb_left)
            .w(width)
            .h(px(HBAR_HEIGHT))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_drag(HScrollDrag { left }, |_, _, _, cx| cx.new(|_| gpui::EmptyView))
            .child(div().w_full().h(px(6.0)).rounded(px(3.0)).bg(c.scroll_thumb).hover(move |s| s.bg(c.scroll_thumb_hover)))
            .into_any_element(),
    )
}

/// The labels over the halves, the rows, the line between the halves (dragged with [`SplitDrag`]) and the sideways
/// scroll bars, in `area` - the page's element, with its scroll and drag listeners. `widest`: each half's widest
/// line in characters (one half: a file only one side has).
fn split_area(area: Stateful<gpui::Div>, list: AnyElement, widest: (usize, Option<usize>), layout: &Layout, cx: &App) -> AnyElement {
    let c = Theme::get(cx).colors;
    let two = widest.1.is_some();
    let split = if two { layout.split } else { 1.0 };
    let (left_w, right_w) = halves(layout.bounds.get().size.width, split);
    let bounds = layout.bounds.clone();
    let label = |text: String| {
        div().min_w_0().pad(px(3.0), px(12.0)).child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(text))
    };
    let labels = div()
        .flex()
        .flex_none()
        .text_color(c.text_muted)
        .border_b_1()
        .border_color(c.card_border)
        .child(label(layout.labels.0.clone()).w(relative(split)).flex_none())
        .when(two, |d| d.child(label(layout.labels.1.clone()).flex_1()));
    area.relative()
        .size_full()
        .flex()
        .flex_col()
        // Its bounds each frame: the page turns a drag or a wheel into a share of the width or a scroll.
        .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
        .child(labels)
        .child(div().flex_1().min_h_0().child(list))
        .when(two, |d| {
            // The line, and on it a grip wider than the line - easy to take hold of, lit under the mouse as the side
            // bars' edges are - centred on it from a wrapper of no width.
            d.child(div().absolute().top_0().bottom_0().left(relative(split)).w(px(1.0)).bg(c.card_border)).child(
                div().absolute().top_0().bottom_0().left(relative(split)).w(Pixels::ZERO).child(
                    div()
                        .id("sc-diff-split")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(-2.0))
                        .w(px(5.0))
                        .cursor(CursorStyle::ResizeLeftRight)
                        .hover(move |s| s.bg(c.focus_border))
                        .on_drag(SplitDrag, |_, _, _, cx| cx.new(|_| gpui::EmptyView)),
                ),
            )
        })
        .children(hbar(true, Pixels::ZERO, left_w, widest.0, layout, cx))
        .children(widest.1.and_then(|w| hbar(false, left_w, right_w, w, layout, cx)))
        .into_any_element()
}

/// A list in the view scrolls up and down only: a sideways wheel (Shift+wheel) is the page's, for both halves at once
/// - else GPUI turns it into an up-and-down scroll of a list that can't scroll sideways.
fn rows_only(mut list: gpui::UniformList, mono: SharedString) -> gpui::UniformList {
    list.style().restrict_scroll_to_axis = Some(true);
    list.font_family(mono).size_full()
}

/// The two copies side by side, as VS Code's diff editor.
pub fn side_by_side_view(
    text: &TextDiff,
    at: Option<usize>,
    scroll: &UniformListScrollHandle,
    mono: SharedString,
    layout: &Layout,
    area: Stateful<gpui::Div>,
    cx: &App,
) -> AnyElement {
    let rows = Arc::new(text.sides.rows.clone());
    let (left, right) = (text.left.clone(), text.right.clone());
    let (left_words, right_words) = (text.left_words.clone(), text.right_words.clone());
    let (split, scroll_x) = (layout.split, layout.scroll_x);
    let list = uniform_list("sc-diff", rows.len(), {
        let rows = rows.clone();
        move |range, _, cx| {
            let c = Theme::get(cx).colors;
            range
                .map(|i| {
                    let row = &rows[i];
                    let (left_bg, right_bg) = match row.kind {
                        RowKind::Same => (None, None),
                        RowKind::Removed => (Some(c.diff_removed_bg), None),
                        RowKind::Added => (None, Some(c.diff_added_bg)),
                        RowKind::Changed => (Some(c.diff_removed_bg), Some(c.diff_added_bg)),
                    };
                    let half = |line: Option<&Line>,
                                bg: Option<Hsla>,
                                spans: &Spans,
                                words: &HashMap<usize, Vec<Range<usize>>>,
                                word_bg: Hsla| {
                        div()
                            .flex()
                            .overflow_hidden()
                            .when_some(bg, |d, bg| d.bg(bg))
                            // No line here: the other side's lines have nothing to line up with (VS Code's fill).
                            .when(line.is_none(), |d| d.bg(c.diff_filler))
                            .child(number(line.map(|l| l.number), c.text_muted))
                            .children(line.map(|l| {
                                let words = words.get(&l.number).map(Vec::as_slice).unwrap_or_default();
                                code(scroll_x, code_line(&l.text, spans.get(l.number - 1).map(Vec::as_slice), words, word_bg, cx))
                            }))
                    };
                    div()
                        .flex()
                        .w_full()
                        .when(at == Some(i), |d| d.border_t_1().border_color(c.focus_border))
                        .child(
                            half(row.left.as_ref(), left_bg, &left, &left_words, c.diff_removed_text_bg)
                                .w(relative(split))
                                .flex_none(),
                        )
                        .child(half(row.right.as_ref(), right_bg, &right, &right_words, c.diff_added_text_bg).flex_1().min_w_0())
                })
                .collect()
        }
    })
    .track_scroll(scroll.clone());
    let list = rows_only(list, mono);
    // The ruler: where the server's lines went (red, its left half) and the project's came (green, its right half).
    let marks = runs(rows.len(), |i| {
        let k = rows[i].kind;
        (matches!(k, RowKind::Removed | RowKind::Changed), matches!(k, RowKind::Added | RowKind::Changed))
    });
    let area = split_area(area, list.into_any_element(), (text.widest.0, Some(text.widest.1)), layout, cx);
    with_ruler("sc-diff-ruler", area, rows.len(), marks, scroll, cx)
}

/// One copy alone, as an editor shows a file: its line numbers and its code.
pub fn one_file_view(
    file: &OneFile,
    scroll: &UniformListScrollHandle,
    mono: SharedString,
    layout: &Layout,
    area: Stateful<gpui::Div>,
    cx: &App,
) -> AnyElement {
    let (lines, spans) = (file.lines.clone(), file.spans.clone());
    let scroll_x = layout.scroll_x;
    let list = uniform_list("sc-one-file", lines.len(), move |range, _, cx| {
        let c = Theme::get(cx).colors;
        range
            .map(|i| {
                div()
                    .flex()
                    .w_full()
                    .child(number(Some(i + 1), c.text_muted))
                    .child(code(scroll_x, code_line(&lines[i], spans.get(i).map(Vec::as_slice), &[], c.diff_added_text_bg, cx)))
            })
            .collect()
    })
    .track_scroll(scroll.clone());
    let list = rows_only(list, mono);
    let area = split_area(area, list.into_any_element(), (file.widest, None), layout, cx);
    div()
        .relative()
        .size_full()
        .child(area)
        .child(dnnm_ui::scrollbar(&scroll.0.borrow().base_handle, cx))
        .into_any_element()
}

/// A run of rows marked on the ruler: where it starts, how many rows, and in which half (true: the left, red).
struct Mark {
    start: usize,
    len: usize,
    left: bool,
}

/// The runs of rows `marked` says are removed (left) and added (right).
fn runs(rows: usize, marked: impl Fn(usize) -> (bool, bool)) -> Vec<Mark> {
    let mut out: Vec<Mark> = Vec::new();
    let mut open: [Option<usize>; 2] = [None, None];
    for i in 0..=rows {
        let (removed, added) = if i < rows { marked(i) } else { (false, false) };
        for (side, on) in [(0, removed), (1, added)] {
            match (open[side], on) {
                (None, true) => open[side] = Some(i),
                (Some(start), false) => {
                    out.push(Mark { start, len: i - start, left: side == 0 });
                    open[side] = None;
                }
                _ => {}
            }
        }
    }
    out
}

/// The diff's area with VS Code's overview ruler on its right edge (each change in its place along the whole file; a click
/// goes there) and the app's scroll bar over it.
fn with_ruler(
    id: &'static str,
    list: AnyElement,
    rows: usize,
    marks: Vec<Mark>,
    scroll: &UniformListScrollHandle,
    cx: &App,
) -> AnyElement {
    let c = Theme::get(cx).colors;
    let ruler_width = 14.0;
    let total = rows.max(1) as f32;
    let ruler = div().id(id).absolute().top_0().bottom_0().right_0().w(px(ruler_width)).children(marks.into_iter().enumerate().map(
        |(k, m)| {
            let scroll = scroll.clone();
            div()
                .id(SharedString::from(format!("{id}-{k}")))
                .absolute()
                .top(relative(m.start as f32 / total))
                .h(relative(m.len as f32 / total))
                .min_h(px(3.0))
                .w(px(ruler_width / 2.0))
                .when(m.left, |d| d.left_0())
                .when(!m.left, |d| d.right_0())
                .cursor_pointer()
                .on_click(move |_: &ClickEvent, window, _| {
                    scroll.scroll_to_item(m.start, ScrollStrategy::Center);
                    window.refresh();
                })
                .bg(if m.left { c.diff_ruler_removed } else { c.diff_ruler_added })
        },
    ));
    div()
        .relative()
        .size_full()
        // The text stops short of the ruler.
        .child(div().size_full().flex().child(div().flex_1().min_w_0().h_full().child(list)).child(div().w(px(ruler_width)).flex_none()))
        .child(ruler)
        .child(dnnm_ui::scrollbar(&scroll.0.borrow().base_handle, cx))
        .into_any_element()
}

/// A line of code with its syntax colours (pieces from [`highlight`]; none: plain) and, behind them, the words that
/// differ marked in `word_bg`.
pub fn code_line(text: &str, spans: Option<&[(Range<usize>, Token)]>, words: &[Range<usize>], word_bg: Hsla, cx: &App) -> StyledText {
    let c = Theme::get(cx).colors;
    let color = |token: Token| match token {
        Token::Keyword => c.syntax_keyword,
        Token::String => c.syntax_string,
        Token::Comment => c.syntax_comment,
        Token::Number => c.syntax_number,
        Token::Tag => c.syntax_tag,
        Token::Attribute => c.syntax_attribute,
    };
    let valid = |r: &Range<usize>| r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end);
    let spans: Vec<&(Range<usize>, Token)> = spans.unwrap_or_default().iter().filter(|(r, _)| valid(r)).collect();
    let words: Vec<&Range<usize>> = words.iter().filter(|r| valid(r)).collect();
    // GPUI's runs are in order and don't overlap: cut the line where any piece starts or ends, and give each cut its
    // colour and its background.
    let mut cuts: Vec<usize> = spans.iter().flat_map(|(r, _)| [r.start, r.end]).chain(words.iter().flat_map(|r| [r.start, r.end])).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let highlights: Vec<(Range<usize>, HighlightStyle)> = cuts
        .windows(2)
        .filter_map(|w| {
            let (a, b) = (w[0], w[1]);
            let fg = spans.iter().find(|(r, _)| r.start <= a && b <= r.end).map(|(_, t)| color(*t));
            let bg = words.iter().any(|r| r.start <= a && b <= r.end).then_some(word_bg);
            (fg.is_some() || bg.is_some()).then(|| (a..b, HighlightStyle { color: fg, background_color: bg, ..Default::default() }))
        })
        .collect();
    StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_between_the_halves_stays_off_the_edges() {
        assert_eq!(clamp_split(0.5), 0.5);
        assert_eq!(clamp_split(0.0), SPLIT_MIN);
        assert_eq!(clamp_split(1.0), 1.0 - SPLIT_MIN);
        assert_eq!(halves(px(1000.0), 0.3), (px(300.0), px(700.0)));
    }

    #[test]
    fn a_half_scrolls_as_far_as_its_widest_line_goes_past_it() {
        // 100 characters of 8: 800 + the padding, in 400 less the numbers.
        let max = overflow(100, px(8.0), px(400.0));
        assert_eq!(max, px(800.0 + CODE_PAD * 2.0) - (px(400.0) - px(NUMBER_WIDTH)));
        assert_eq!(overflow(10, px(8.0), px(400.0)), Pixels::ZERO, "short lines: nothing to scroll");
    }

    #[test]
    fn the_thumb_shows_how_much_is_seen_and_where() {
        assert_eq!(thumb(10, px(8.0), px(400.0), Pixels::ZERO), None);
        let (left, width) = thumb(200, px(8.0), px(400.0), Pixels::ZERO).unwrap();
        let track = px(400.0) - px(NUMBER_WIDTH);
        assert_eq!(left, Pixels::ZERO);
        assert!(width < track && width >= px(THUMB_MIN));
        let max = overflow(200, px(8.0), px(400.0));
        let (end, _) = thumb(200, px(8.0), px(400.0), max * 2.0).unwrap();
        assert_eq!(end, track - width, "scrolled past the end: the thumb stops at the track's end");
    }
}
