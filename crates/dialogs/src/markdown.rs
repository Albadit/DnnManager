//! The Markdown of DNN Manager's release notes as GPUI elements - `Controls/MarkdownDocument.cs`: headings (`#` to
//! `###`), bullet lists (nested by indentation), paragraphs, and in the text `**bold**`, `*italic*`, `` `code` `` and
//! `[links](…)`. Only what the notes use - not a full Markdown reader (no tables: the C# doesn't render them either).
//!
//! Also the guide's and the tour's text with its `**bold**` parts (`GuideDialog.AddText`).

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, FontStyle, FontWeight, Hsla, InteractiveText, Pixels, SharedString, StyledText, TextRun,
    UnderlineStyle, Window, div, font, prelude::*, px,
};

use dnnm_ui::theme::Theme;
use dnnm_ui::{Pad, Tooltip};

/// A piece of a line's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Code(String),
    /// A link's text and where it points, as written (resolved when drawn).
    Link {
        text: String,
        url: String,
    },
}

/// A block of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `#` (1) to `###` (3).
    Heading {
        level: u8,
        text: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    /// A bullet; `depth` 0 is the outer list (a disc), deeper ones are nested (a circle).
    Bullet {
        depth: usize,
        text: Vec<Inline>,
    },
}

/// Reads `markdown` as `MarkdownDocument.Append` does.
pub fn parse(markdown: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    // How many lists are open, outermost first - a bullet indented two more spaces is one level deeper.
    let mut lists = 0usize;
    // Whether the last block is a paragraph (or bullet) the next line continues.
    let mut open_paragraph = false;

    for raw in markdown.replace("\r\n", "\n").split('\n') {
        let line = raw.trim_end();
        if line.is_empty() {
            open_paragraph = false;
            continue;
        }
        if let Some((level, text)) = heading(line) {
            lists = 0;
            blocks.push(Block::Heading { level, text: inlines(text) });
            open_paragraph = false;
            continue;
        }
        if let Some((indent, text)) = bullet(line) {
            let depth = indent / 2;
            if lists > depth + 1 {
                lists = depth + 1;
            }
            if lists < depth + 1 {
                // One level at a time, as the C# opens one list per bullet.
                lists += 1;
            }
            blocks.push(Block::Bullet { depth: lists - 1, text: inlines(text) });
            open_paragraph = true;
            continue;
        }
        // The next line of the paragraph or bullet above, or a paragraph of its own.
        if open_paragraph {
            if let Some(Block::Paragraph(text) | Block::Bullet { text, .. }) = blocks.last_mut() {
                text.push(Inline::Text(" ".into()));
                text.extend(inlines(line.trim()));
            }
            continue;
        }
        lists = 0;
        blocks.push(Block::Paragraph(inlines(line.trim())));
        open_paragraph = true;
    }
    blocks
}

/// `^(#{1,3})\s+(.+)$`
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    let text = rest.trim_start();
    (text.len() < rest.len() && !text.is_empty()).then_some((hashes as u8, text))
}

/// `^(\s*)[-*]\s+(.+)$` - the indentation's length and the text.
fn bullet(line: &str) -> Option<(usize, &str)> {
    let text = line.trim_start();
    let indent = line.len() - text.len();
    let rest = text.strip_prefix('-').or_else(|| text.strip_prefix('*'))?;
    let body = rest.trim_start();
    (body.len() < rest.len() && !body.is_empty()).then_some((indent, body))
}

/// The pieces of a line: the first of `**bold**`, `` `code` ``, `[text](url)`, `*italic*` at each place, as the C#
/// regular expression `Inline()` matches them.
pub fn inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        if let Some((inline, len)) = inline_at(rest) {
            if !plain.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut plain)));
            }
            out.push(inline);
            at += len;
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            plain.push(c);
            at += c.len_utf8();
        }
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

fn inline_at(rest: &str) -> Option<(Inline, usize)> {
    // \*\*(?<bold>.+?)\*\*
    if let Some(after) = rest.strip_prefix("**") {
        let first = after.chars().next().map_or(0, char::len_utf8);
        if first > 0
            && let Some(end) = after[first..].find("**")
        {
            let inner = &after[..first + end];
            return Some((Inline::Bold(inlines(inner)), 2 + inner.len() + 2));
        }
    }
    // `(?<code>[^`]+)`
    if let Some(after) = rest.strip_prefix('`')
        && let Some(end) = after.find('`')
        && end > 0
    {
        return Some((Inline::Code(after[..end].to_string()), end + 2));
    }
    // \[(?<text>[^\]]+)\]\((?<url>[^)\s]+)\)
    if let Some(after) = rest.strip_prefix('[')
        && let Some(close) = after.find(']')
        && close > 0
        && let Some(target) = after[close + 1..].strip_prefix('(')
    {
        let url_len = target.find(|c: char| c == ')' || c.is_whitespace()).unwrap_or(target.len());
        if url_len > 0 && target[url_len..].starts_with(')') {
            let link = Inline::Link { text: after[..close].to_string(), url: target[..url_len].to_string() };
            return Some((link, 1 + close + 2 + url_len + 1));
        }
    }
    // \*(?<italic>[^*\s][^*]*)\*
    if let Some(after) = rest.strip_prefix('*') {
        let first = after.chars().next()?;
        if first != '*'
            && !first.is_whitespace()
            && let Some(end) = after.find('*')
        {
            return Some((Inline::Italic(inlines(&after[..end])), end + 2));
        }
    }
    None
}

/// `text` split at its `**`: every second part is bold (`GuideDialog.AddText`).
pub fn bold_parts(text: &str) -> Vec<(&str, bool)> {
    text.split("**").enumerate().filter(|(_, p)| !p.is_empty()).map(|(i, p)| (p, i % 2 == 1)).collect()
}

/// The guide's and the tour's text: its `**bold**` parts bold, in the font and colour around it.
pub fn bold_text(text: &str) -> StyledText {
    let mut plain = String::new();
    let mut bold = Vec::new();
    for (part, is_bold) in bold_parts(text) {
        if is_bold {
            bold.push((
                plain.len()..plain.len() + part.len(),
                gpui::HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
            ));
        }
        plain.push_str(part);
    }
    StyledText::new(plain).with_highlights(bold)
}

/// What a link click does: gets where the link goes (already resolved).
pub type OnLink = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// A drawn block with the room the C# keeps above and under it - [`stack`] turns those into gaps.
pub struct Piece {
    pub element: AnyElement,
    pub above: Pixels,
    pub under: Pixels,
}

/// Draws `blocks`. `link` turns a link as written into where it goes (`ReleaseNote.LinkTarget`); `on_link` opens it.
/// `first` is whether the blocks start the document - its first heading then has no space above it. Sizes are the
/// C#'s at the default font size 13, scaled with Settings → General's font size. [`stack`] lays the pieces out.
pub fn render(
    id: &str,
    blocks: &[Block],
    first: bool,
    link: &dyn Fn(&str) -> String,
    on_link: &OnLink,
    cx: &App,
) -> Vec<Piece> {
    let theme = Theme::get(cx);
    let k = theme.text.body / px(13.0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let element_id = SharedString::from(format!("{id}-{i}"));
        match &blocks[i] {
            Block::Heading { level, text } => {
                let (size, top, bottom) = match level {
                    1 => (22.0, if first && i == 0 { 0.0 } else { 28.0 }, 6.0),
                    2 => (17.0, 18.0, 6.0),
                    _ => (14.5, 12.0, 4.0),
                };
                out.push(Piece {
                    element: div()
                        .w_full()
                        .text_size(px(size * k))
                        .line_height(px(size * k * 1.4))
                        .child(rich_text(element_id, text, FontWeight::SEMIBOLD, link, on_link, cx))
                        .into_any_element(),
                    above: px(top * k),
                    under: px(bottom * k),
                });
                i += 1;
            }
            Block::Paragraph(text) => {
                out.push(Piece {
                    element: div()
                        .w_full()
                        .child(rich_text(element_id, text, FontWeight::NORMAL, link, on_link, cx))
                        .into_any_element(),
                    above: px(0.0),
                    under: px(8.0 * k),
                });
                i += 1;
            }
            Block::Bullet { .. } => {
                let start = i;
                while i < blocks.len() && matches!(blocks[i], Block::Bullet { .. }) {
                    i += 1;
                }
                out.push(Piece {
                    element: list(id, start, &blocks[start..i], link, on_link, cx).into_any_element(),
                    above: px(2.0 * k),
                    under: px(4.0 * k),
                });
            }
        }
    }
    out
}

/// The pieces one under the other, as the C#'s stacked margins: between two the larger of the room under the first
/// and above the second. Each gap is a flex `gap`: the widest gaps split the pieces into groups, each group stacked
/// the same way with its own (narrower) gaps.
pub fn stack(pieces: Vec<Piece>) -> AnyElement {
    let gaps: Vec<Pixels> =
        pieces.windows(2).map(|w| if w[0].under > w[1].above { w[0].under } else { w[1].above }).collect();
    stack_with_gaps(pieces.into_iter().map(|p| p.element).collect(), gaps)
}

/// `items` one under the other with `gaps[i]` between item `i` and `i + 1`.
pub fn stack_with_gaps(mut items: Vec<AnyElement>, gaps: Vec<Pixels>) -> AnyElement {
    if items.len() <= 1 {
        return items.pop().unwrap_or_else(|| div().into_any_element());
    }
    let widest = gaps.iter().copied().fold(px(0.0), |a, b| if b > a { b } else { a });
    let mut column = div().w_full().flex().flex_col().gap(widest);
    let (mut group, mut group_gaps) = (Vec::new(), Vec::new());
    let mut gaps = gaps.into_iter();
    for item in items {
        group.push(item);
        match gaps.next() {
            Some(gap) if gap == widest => {
                column = column.child(stack_with_gaps(std::mem::take(&mut group), std::mem::take(&mut group_gaps)));
            }
            Some(gap) => group_gaps.push(gap),
            None => {}
        }
    }
    column.child(stack_with_gaps(group, group_gaps)).into_any_element()
}

/// A run of bullets: each indented 20 px a level, a disc for the outer list and a circle for the nested ones.
fn list(id: &str, start: usize, bullets: &[Block], link: &dyn Fn(&str) -> String, on_link: &OnLink, cx: &App) -> Div {
    let theme = Theme::get(cx);
    let k = theme.text.body / px(13.0);
    let color = theme.colors.text_primary;
    div().w_full().flex().flex_col().gap(px(4.0 * k)).children(bullets.iter().enumerate().filter_map(|(n, b)| {
        let Block::Bullet { depth, text } = b else { return None };
        // The level's indent and the disc's column in one, the disc at its right end, 7 before the text.
        let marker = div()
            .flex_none()
            .w(px(20.0 * k * *depth as f32 + 13.0 * k))
            .flex()
            .justify_end()
            .pad(px(8.0 * k), px(0.0))
            .child(
                div()
                    .size(px(5.0 * k))
                    .rounded_full()
                    .border_1()
                    .border_color(color)
                    .when(*depth == 0, |d| d.bg(color)),
            );
        let id = SharedString::from(format!("{id}-{}", start + n));
        Some(
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(px(7.0 * k))
                .child(marker)
                .child(div().flex_1().min_w_0().child(rich_text(id, text, FontWeight::NORMAL, link, on_link, cx))),
        )
    }))
}

/// One piece of text, flattened.
struct Span {
    text: String,
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<String>,
}

fn flatten(inlines: &[Inline], bold: bool, italic: bool, out: &mut Vec<Span>) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push(Span { text: t.clone(), bold, italic, code: false, link: None }),
            Inline::Bold(inner) => flatten(inner, true, italic, out),
            Inline::Italic(inner) => flatten(inner, bold, true, out),
            Inline::Code(t) => out.push(Span { text: t.clone(), bold, italic, code: true, link: None }),
            Inline::Link { text, url } => {
                out.push(Span { text: text.clone(), bold, italic, code: false, link: Some(url.clone()) })
            }
        }
    }
}

/// A block's text with its bold, italic, code (Cascadia Mono) and links (LinkFg, underlined, their target as tooltip).
fn rich_text(
    id: SharedString,
    inlines: &[Inline],
    weight: FontWeight,
    link: &dyn Fn(&str) -> String,
    on_link: &OnLink,
    cx: &App,
) -> AnyElement {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let mut spans = Vec::new();
    flatten(inlines, false, false, &mut spans);
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for span in spans.into_iter().filter(|s| !s.text.is_empty()) {
        let mut f = font(if span.code { theme.mono_font.clone() } else { theme.font.clone() });
        f.weight = if span.bold { FontWeight::BOLD } else { weight };
        if span.italic {
            f.style = FontStyle::Italic;
        }
        let color: Hsla = if span.link.is_some() { c.link_fg } else { c.text_primary };
        let start = text.len();
        text.push_str(&span.text);
        runs.push(TextRun {
            len: span.text.len(),
            font: f,
            color,
            background_color: None,
            underline: span.link.as_ref().map(|_| UnderlineStyle {
                thickness: px(1.0),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: None,
        });
        if let Some(url) = span.link {
            ranges.push(start..text.len());
            targets.push(link(&url));
        }
    }
    let styled = StyledText::new(text).with_runs(runs);
    if ranges.is_empty() {
        return styled.into_any_element();
    }
    let targets = Rc::new(targets);
    let tip_targets = targets.clone();
    let tip_ranges = ranges.clone();
    let on_link = on_link.clone();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, window, cx| {
            if let Some(target) = targets.get(ix) {
                on_link(target, window, cx);
            }
        })
        .tooltip(move |at, window, cx| {
            let ix = tip_ranges.iter().position(|r| r.contains(&at))?;
            Some(Tooltip::text(tip_targets[ix].clone())(window, cx))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn headings_by_level_and_not_without_a_space() {
        assert_eq!(parse("# DNN Manager 1.8.7"), vec![Block::Heading { level: 1, text: vec![t("DNN Manager 1.8.7")] }]);
        assert_eq!(parse("### Sub")[0], Block::Heading { level: 3, text: vec![t("Sub")] });
        assert_eq!(parse("#### Four")[0], Block::Paragraph(vec![t("#### Four")]));
        assert_eq!(parse("#tag")[0], Block::Paragraph(vec![t("#tag")]));
    }

    #[test]
    fn paragraph_lines_join_and_blank_lines_split() {
        let blocks = parse("one\r\ntwo\r\n\r\nthree");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![t("one"), t(" "), t("two")]), Block::Paragraph(vec![t("three")])]
        );
    }

    #[test]
    fn bullets_nest_one_level_at_a_time_and_continue_after_a_blank_line() {
        let blocks = parse("- a\n  - b\n      - c\n- d\n\n- e\n  more");
        let depths: Vec<usize> =
            blocks.iter().map(|b| if let Block::Bullet { depth, .. } = b { *depth } else { 99 }).collect();
        assert_eq!(depths, vec![0, 1, 2, 0, 0]);
        assert_eq!(blocks[4], Block::Bullet { depth: 0, text: vec![t("e"), t(" "), t("more")] });
        // A jump of two levels opens only one.
        let blocks = parse("- a\n    - b");
        assert_eq!(blocks[1], Block::Bullet { depth: 1, text: vec![t("b")] });
    }

    #[test]
    fn inline_bold_code_link_italic() {
        assert_eq!(
            inlines("**Pick** the `host` [CHANGELOG.md](../../CHANGELOG.md#v187) *now*"),
            vec![
                Inline::Bold(vec![t("Pick")]),
                t(" the "),
                Inline::Code("host".into()),
                t(" "),
                Inline::Link { text: "CHANGELOG.md".into(), url: "../../CHANGELOG.md#v187".into() },
                t(" "),
                Inline::Italic(vec![t("now")]),
            ]
        );
        assert_eq!(inlines("**Change host password…**"), vec![Inline::Bold(vec![t("Change host password…")])]);
        assert_eq!(inlines("a * b * c"), vec![t("a * b * c")]);
        assert_eq!(inlines("[x](has space)"), vec![t("[x](has space)")]);
        assert_eq!(
            inlines("**bold *italic* in**"),
            vec![Inline::Bold(vec![t("bold "), Inline::Italic(vec![t("italic")]), t(" in")])]
        );
    }

    #[test]
    fn every_release_note_parses_to_a_heading_first() {
        for note in crate::whats_new::all_notes() {
            let blocks = parse(note.markdown);
            assert!(matches!(blocks.first(), Some(Block::Heading { level: 1, .. })), "{}", note.tag);
        }
    }

    #[test]
    fn bold_parts_alternate() {
        assert_eq!(
            bold_parts("**Projects** lists. **New**"),
            vec![("Projects", true), (" lists. ", false), ("New", true)]
        );
        assert_eq!(bold_parts("plain"), vec![("plain", false)]);
    }
}
