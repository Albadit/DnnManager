//! The side-by-side diff of a file: the server's copy on the left, the project's on the right, line by line
//! (`similar`'s Myers diff), with where each change starts for going from one to the next. A binary file has none.

use similar::{DiffOp, TextDiff};

/// A line on one side: its number (from 1) and its text, without the line break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub number: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Same,
    /// Only on the left (the server's): removed here.
    Removed,
    /// Only on the right (the project's): added here.
    Added,
    /// Both, different.
    Changed,
}

/// One row of the side-by-side view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub left: Option<Line>,
    pub right: Option<Line>,
    pub kind: RowKind,
}

/// The whole diff: its rows, and the row each change starts at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideBySide {
    pub rows: Vec<Row>,
    pub changes: Vec<usize>,
}

impl SideBySide {
    /// The change after `row` (from the first when None), wrapping round.
    pub fn next_change(&self, row: Option<usize>) -> Option<usize> {
        let after = |&&c: &&usize| row.is_none_or(|r| c > r);
        self.changes.iter().find(after).or(self.changes.first()).copied()
    }

    /// The change before `row` (from the last when None), wrapping round.
    pub fn previous_change(&self, row: Option<usize>) -> Option<usize> {
        let before = |&&c: &&usize| row.is_none_or(|r| c < r);
        self.changes.iter().rev().find(before).or(self.changes.last()).copied()
    }

    /// Lines added and removed, as a "+3 -1" summary counts them (a changed row is one of each).
    pub fn added_removed(&self) -> (usize, usize) {
        self.rows.iter().fold((0, 0), |(a, r), row| match row.kind {
            RowKind::Same => (a, r),
            RowKind::Added => (a + 1, r),
            RowKind::Removed => (a, r + 1),
            RowKind::Changed => (a + 1, r + 1),
        })
    }
}

/// One row of the inline view: a line of either side, with its number on each side it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub text: String,
    /// Same, Removed (the server's) or Added (the project's) - never Changed: a changed line is its two lines.
    pub kind: RowKind,
}

/// The diff as one column: unchanged lines once, a change's server lines before its project lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inline {
    pub rows: Vec<InlineRow>,
    /// The row each change starts at.
    pub changes: Vec<usize>,
}

impl SideBySide {
    /// The same diff as one column ([`Inline`]).
    pub fn inline(&self) -> Inline {
        let mut out = Inline::default();
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let flush = |out: &mut Inline, removed: &mut Vec<InlineRow>, added: &mut Vec<InlineRow>| {
            if !removed.is_empty() || !added.is_empty() {
                out.changes.push(out.rows.len());
                out.rows.append(removed);
                out.rows.append(added);
            }
        };
        for row in &self.rows {
            match row.kind {
                RowKind::Same => {
                    flush(&mut out, &mut removed, &mut added);
                    let (left, right) = (row.left.as_ref(), row.right.as_ref());
                    out.rows.push(InlineRow {
                        left: left.map(|l| l.number),
                        right: right.map(|r| r.number),
                        text: left.or(right).map(|l| l.text.clone()).unwrap_or_default(),
                        kind: RowKind::Same,
                    });
                }
                _ => {
                    if let Some(l) = &row.left {
                        removed.push(InlineRow { left: Some(l.number), right: None, text: l.text.clone(), kind: RowKind::Removed });
                    }
                    if let Some(r) = &row.right {
                        added.push(InlineRow { left: None, right: Some(r.number), text: r.text.clone(), kind: RowKind::Added });
                    }
                }
            }
        }
        flush(&mut out, &mut removed, &mut added);
        out
    }
}

/// The markers a merge leaves in a file, as git writes them.
pub const MARKER_SERVER: &str = "<<<<<<< ";
pub const MARKER_SPLIT: &str = "=======";
pub const MARKER_PROJECT: &str = ">>>>>>> ";

/// Both copies in one text to merge by hand: what they share once, and each place they differ as the server's lines
/// and the project's between git's markers (`<<<<<<< server`, `=======`, `>>>>>>> project`). The project's line
/// breaks (`\r\n` or `\n`) are kept.
pub fn merge_markers(server: &str, project: &str, server_name: &str) -> String {
    let sample = if project.is_empty() { server } else { project };
    let newline = if sample.contains("\r\n") { "\r\n" } else { "\n" };
    let diff = TextDiff::from_lines(server, project);
    let (old, new) = (diff.old_slices(), diff.new_slices());
    let line = |s: &str| s.trim_end_matches(['\n', '\r']).to_string();
    let mut out: Vec<String> = Vec::new();
    for op in diff.ops() {
        let (old_range, new_range) = match *op {
            DiffOp::Equal { new_index, len, .. } => {
                out.extend((new_index..new_index + len).map(|i| line(new[i])));
                continue;
            }
            DiffOp::Delete { old_index, old_len, new_index } => (old_index..old_index + old_len, new_index..new_index),
            DiffOp::Insert { old_index, new_index, new_len } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                (old_index..old_index + old_len, new_index..new_index + new_len)
            }
        };
        out.push(format!("{MARKER_SERVER}{server_name}"));
        out.extend(old_range.map(|i| line(old[i])));
        out.push(MARKER_SPLIT.into());
        out.extend(new_range.map(|i| line(new[i])));
        out.push(format!("{MARKER_PROJECT}project"));
    }
    let mut text = out.join(newline);
    if sample.ends_with('\n') {
        text.push_str(newline);
    }
    text
}

/// Whether a text still has a merge's markers: a line starting `<<<<<<< ` or `>>>>>>> `, or one that is `=======`.
pub fn has_markers(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_end();
        l.starts_with(MARKER_SERVER) || l.starts_with(MARKER_PROJECT) || l == MARKER_SPLIT
    })
}

/// Whether `bytes` are text to show: UTF-8 (a byte-order mark allowed) with no NUL in the first 8 KB.
pub fn is_text(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    !head.contains(&0) && std::str::from_utf8(strip_bom(bytes)).is_ok()
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// The text of a file's bytes; None for a binary one.
pub fn text_of(bytes: &[u8]) -> Option<String> {
    is_text(bytes).then(|| String::from_utf8_lossy(strip_bom(bytes)).into_owned())
}

/// The file extensions shown as text without looking at the bytes first (the rest are shown when they are text).
pub const TEXT_EXTENSIONS: &[&str] = &[
    "cs", "cshtml", "razor", "vb", "vbhtml", "ascx", "aspx", "asmx", "ashx", "master", "js", "mjs", "ts", "tsx", "jsx",
    "json", "css", "scss", "less", "html", "htm", "xml", "xsl", "xslt", "config", "resx", "txt", "md", "yml", "yaml",
    "svg", "sql", "ps1", "bat", "cmd", "sh", "ini", "dnn", "manifest", "template", "tokens",
];

/// The side-by-side rows of `left` (the server's text) against `right` (the project's).
pub fn side_by_side(left: &str, right: &str) -> SideBySide {
    let diff = TextDiff::from_lines(left, right);
    let (old, new): (Vec<&str>, Vec<&str>) = (diff.old_slices().to_vec(), diff.new_slices().to_vec());
    let line = |slices: &[&str], i: usize| Line { number: i + 1, text: slices[i].trim_end_matches(['\n', '\r']).into() };
    let mut out = SideBySide::default();
    for op in diff.ops() {
        if !matches!(op, DiffOp::Equal { .. }) {
            out.changes.push(out.rows.len());
        }
        match *op {
            DiffOp::Equal { old_index, new_index, len } => {
                for k in 0..len {
                    out.rows.push(Row {
                        left: Some(line(&old, old_index + k)),
                        right: Some(line(&new, new_index + k)),
                        kind: RowKind::Same,
                    });
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                for k in 0..old_len {
                    out.rows.push(Row { left: Some(line(&old, old_index + k)), right: None, kind: RowKind::Removed });
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                for k in 0..new_len {
                    out.rows.push(Row { left: None, right: Some(line(&new, new_index + k)), kind: RowKind::Added });
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                // Paired line by line; what is left over on the longer side is removed or added.
                for k in 0..old_len.max(new_len) {
                    let (l, r) = ((k < old_len).then(|| line(&old, old_index + k)), (k < new_len).then(|| line(&new, new_index + k)));
                    let kind = match (&l, &r) {
                        (Some(_), Some(_)) => RowKind::Changed,
                        (Some(_), None) => RowKind::Removed,
                        _ => RowKind::Added,
                    };
                    out.rows.push(Row { left: l, right: r, kind });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(d: &SideBySide) -> Vec<RowKind> {
        d.rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn rows_pair_the_two_sides() {
        let d = side_by_side("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        assert_eq!(kinds(&d), [RowKind::Same, RowKind::Changed, RowKind::Same, RowKind::Same, RowKind::Added]);
        assert_eq!(d.rows[1].left.as_ref().unwrap().text, "b");
        assert_eq!(d.rows[1].right.as_ref().unwrap().text, "B");
        assert_eq!(d.rows[4].right.as_ref().unwrap().number, 5);
        assert_eq!(d.changes, [1, 4]);
        assert_eq!(d.added_removed(), (2, 1));
    }

    #[test]
    fn removed_lines_are_on_the_left_only_and_windows_line_breaks_go() {
        let d = side_by_side("a\r\nx\r\nb\r\n", "a\r\nb\r\n");
        assert_eq!(kinds(&d), [RowKind::Same, RowKind::Removed, RowKind::Same]);
        assert_eq!(d.rows[1].left.as_ref().unwrap().text, "x");
        assert!(d.rows[1].right.is_none());
    }

    #[test]
    fn going_from_change_to_change_wraps_round() {
        let d = side_by_side("1\n2\n3\n4\n5\n", "1\nX\n3\n4\nY\n");
        assert_eq!(d.changes, [1, 4]);
        assert_eq!(d.next_change(None), Some(1));
        assert_eq!(d.next_change(Some(1)), Some(4));
        assert_eq!(d.next_change(Some(4)), Some(1));
        assert_eq!(d.previous_change(Some(4)), Some(1));
        assert_eq!(d.previous_change(Some(1)), Some(4));
        assert_eq!(side_by_side("same\n", "same\n").next_change(None), None);
    }

    #[test]
    fn inline_shows_a_change_as_its_server_lines_then_its_project_lines() {
        let inline = side_by_side("a
b
c
d
", "a
B
B2
d
").inline();
        let rows: Vec<(Option<usize>, Option<usize>, &str, RowKind)> =
            inline.rows.iter().map(|r| (r.left, r.right, r.text.as_str(), r.kind)).collect();
        assert_eq!(
            rows,
            [
                (Some(1), Some(1), "a", RowKind::Same),
                (Some(2), None, "b", RowKind::Removed),
                (Some(3), None, "c", RowKind::Removed),
                (None, Some(2), "B", RowKind::Added),
                (None, Some(3), "B2", RowKind::Added),
                (Some(4), Some(4), "d", RowKind::Same),
            ]
        );
        assert_eq!(inline.changes, [1]);
        assert!(side_by_side("x
", "x
").inline().changes.is_empty());
    }

    #[test]
    fn a_merge_marks_each_difference_with_both_sides() {
        let merged = merge_markers("a\nserver\nc\nold\n", "a\nmine\nc\n", "Production");
        assert_eq!(
            merged,
            "a\n<<<<<<< Production\nserver\n=======\nmine\n>>>>>>> project\nc\n<<<<<<< Production\nold\n=======\n>>>>>>> project\n"
        );
        assert!(has_markers(&merged));
        assert!(!has_markers("a\nb ======= c\n"), "only a line of its own counts");
        let crlf = merge_markers("x\r\n", "y\r\n", "Test");
        assert_eq!(crlf, "<<<<<<< Test\r\nx\r\n=======\r\ny\r\n>>>>>>> project\r\n", "the project's line breaks kept");
        assert_eq!(merge_markers("same\n", "same\n", "T"), "same\n");
    }

    #[test]
    fn binary_files_are_told_from_text() {
        assert!(is_text(b"<div>hello</div>"));
        assert!(is_text(&[0xEF, 0xBB, 0xBF, b'a']));
        assert!(!is_text(&[0x89, b'P', b'N', b'G', 0, 1]));
        assert!(!is_text(&[0xFF, 0xFE, 0xFD]));
        assert_eq!(text_of(&[0xEF, 0xBB, 0xBF, b'a']).as_deref(), Some("a"));
        assert_eq!(text_of(&[0]), None);
    }
}
