//! The Source Control page's main view, apart from the page's state: a file's two copies as VS Code's diff editor
//! shows them - side by side in two equal halves (the server's left, the project's right), each with its line
//! numbers, a changed line's own words marked inside it, a blank where the other side has no line, and the overview
//! ruler on the right edge with every change's place (a click goes there) - or in one column (Inline), or, when only
//! one side has the file, that copy alone as an editor shows it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use dnnm_application::source_control::diff::{Inline, Line, RowKind, SideBySide, side_by_side, word_changes};
use dnnm_application::source_control::highlight::{Token, highlight, language_of};
use dnnm_ui::{Pad, px};
use dnnm_ui::theme::Theme;
use gpui::{
    AnyElement, App, ClickEvent, Hsla, HighlightStyle, ScrollStrategy, SharedString, StyledText, UniformListScrollHandle,
    div, prelude::*, relative, uniform_list,
};

/// Each line's coloured pieces (index: line number - 1).
type Spans = Vec<Vec<(Range<usize>, Token)>>;

/// A text diff, both ways it is shown, with each side's lines coloured and a changed line's differing words.
pub struct TextDiff {
    pub sides: SideBySide,
    pub inline: Inline,
    left: Arc<Spans>,
    right: Arc<Spans>,
    /// The differing words of a changed line, by its number on its side.
    left_words: Arc<HashMap<usize, Vec<Range<usize>>>>,
    right_words: Arc<HashMap<usize, Vec<Range<usize>>>>,
}

impl TextDiff {
    pub fn new(path: &str, left: &str, right: &str) -> Self {
        let sides = side_by_side(left, right);
        let inline = sides.inline();
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
        Self {
            left: Arc::new(highlight(language, &l)),
            right: Arc::new(highlight(language, &r)),
            left_words: Arc::new(left_words),
            right_words: Arc::new(right_words),
            sides,
            inline,
        }
    }

    pub fn changes(&self, inline: bool) -> &[usize] {
        if inline { &self.inline.changes } else { &self.sides.changes }
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
}

impl OneFile {
    pub fn new(path: &str, text: &str, server: bool) -> Self {
        let lines: Vec<&str> = text.lines().collect();
        let spans = highlight(language_of(path), &lines);
        Self { lines: Arc::new(lines.into_iter().map(str::to_string).collect()), spans: Arc::new(spans), server }
    }
}

/// The line number column.
fn number(n: Option<usize>, width: f32, c: Hsla) -> gpui::Div {
    div()
        .w(px(width))
        .flex_none()
        .pad(px(0.0), px(6.0))
        .flex()
        .justify_end()
        .text_color(c)
        .child(n.map(|n| n.to_string()).unwrap_or_default())
}

/// The two copies side by side, as VS Code's diff editor: two equal halves.
pub fn side_by_side_view(text: &TextDiff, at: Option<usize>, scroll: &UniformListScrollHandle, mono: SharedString, cx: &App) -> AnyElement {
    let rows = Arc::new(text.sides.rows.clone());
    let (left, right) = (text.left.clone(), text.right.clone());
    let (left_words, right_words) = (text.left_words.clone(), text.right_words.clone());
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
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .when_some(bg, |d, bg| d.bg(bg))
                            // No line here: the other side's lines have nothing to line up with (VS Code's fill).
                            .when(line.is_none(), |d| d.bg(c.diff_filler))
                            .child(number(line.map(|l| l.number), 48.0, c.text_muted))
                            .children(line.map(|l| {
                                let words = words.get(&l.number).map(Vec::as_slice).unwrap_or_default();
                                div().pad(px(0.0), px(4.0)).whitespace_nowrap().child(code_line(
                                    &l.text,
                                    spans.get(l.number - 1).map(Vec::as_slice),
                                    words,
                                    word_bg,
                                    cx,
                                ))
                            }))
                    };
                    div()
                        .flex()
                        .w_full()
                        .when(at == Some(i), |d| d.border_t_1().border_color(c.focus_border))
                        .child(half(row.left.as_ref(), left_bg, &left, &left_words, c.diff_removed_text_bg))
                        .child(div().w(px(1.0)).flex_none().bg(c.card_border))
                        .child(half(row.right.as_ref(), right_bg, &right, &right_words, c.diff_added_text_bg))
                })
                .collect()
        }
    })
    .track_scroll(scroll.clone())
    .font_family(mono)
    .size_full();
    // The ruler: where the server's lines went (red, its left half) and the project's came (green, its right half).
    let marks = runs(rows.len(), |i| {
        let k = rows[i].kind;
        (matches!(k, RowKind::Removed | RowKind::Changed), matches!(k, RowKind::Added | RowKind::Changed))
    });
    with_ruler("sc-diff-ruler", list.into_any_element(), rows.len(), marks, scroll, cx)
}

/// The changes in one column: a change's server lines, then its project lines.
pub fn inline_view(text: &TextDiff, at: Option<usize>, scroll: &UniformListScrollHandle, mono: SharedString, cx: &App) -> AnyElement {
    let rows = Arc::new(text.inline.rows.clone());
    let (left, right) = (text.left.clone(), text.right.clone());
    let (left_words, right_words) = (text.left_words.clone(), text.right_words.clone());
    let list = uniform_list("sc-diff-inline", rows.len(), {
        let rows = rows.clone();
        move |range, _, cx| {
            let c = Theme::get(cx).colors;
            range
                .map(|i| {
                    let row = &rows[i];
                    let (bg, sign) = match row.kind {
                        RowKind::Removed => (Some(c.diff_removed_bg), "-"),
                        RowKind::Added => (Some(c.diff_added_bg), "+"),
                        _ => (None, " "),
                    };
                    let (spans, words, word_bg) = match row.kind {
                        RowKind::Removed => (
                            row.left.and_then(|n| left.get(n - 1)),
                            row.left.and_then(|n| left_words.get(&n)),
                            c.diff_removed_text_bg,
                        ),
                        _ => (
                            row.right.and_then(|n| right.get(n - 1)),
                            row.right.and_then(|n| right_words.get(&n)),
                            c.diff_added_text_bg,
                        ),
                    };
                    div()
                        .flex()
                        .w_full()
                        .when_some(bg, |d, bg| d.bg(bg))
                        .when(at == Some(i), |d| d.border_t_1().border_color(c.focus_border))
                        .child(number(row.left, 44.0, c.text_muted))
                        .child(number(row.right, 44.0, c.text_muted))
                        .child(div().w(px(14.0)).flex_none().text_color(c.text_muted).child(sign))
                        .child(div().whitespace_nowrap().child(code_line(
                            &row.text,
                            spans.map(Vec::as_slice),
                            words.map(Vec::as_slice).unwrap_or_default(),
                            word_bg,
                            cx,
                        )))
                })
                .collect()
        }
    })
    .track_scroll(scroll.clone())
    .font_family(mono)
    .size_full();
    let marks = runs(rows.len(), |i| (rows[i].kind == RowKind::Removed, rows[i].kind == RowKind::Added));
    with_ruler("sc-diff-ruler", list.into_any_element(), rows.len(), marks, scroll, cx)
}

/// One copy alone, as an editor shows a file: its line numbers and its code.
pub fn one_file_view(file: &OneFile, scroll: &UniformListScrollHandle, mono: SharedString, cx: &App) -> AnyElement {
    let (lines, spans) = (file.lines.clone(), file.spans.clone());
    let list = uniform_list("sc-one-file", lines.len(), move |range, _, cx| {
        let c = Theme::get(cx).colors;
        range
            .map(|i| {
                div()
                    .flex()
                    .w_full()
                    .child(number(Some(i + 1), 48.0, c.text_muted))
                    .child(div().pad(px(0.0), px(4.0)).whitespace_nowrap().child(code_line(
                        &lines[i],
                        spans.get(i).map(Vec::as_slice),
                        &[],
                        c.diff_added_text_bg,
                        cx,
                    )))
            })
            .collect()
    })
    .track_scroll(scroll.clone())
    .font_family(mono)
    .size_full();
    div()
        .relative()
        .size_full()
        .child(list)
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

/// The list with VS Code's overview ruler on its right edge (each change in its place along the whole file; a click
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
