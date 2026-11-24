//! Syntax colours for the diff: a small tokenizer per kind of file a DNN site holds - C-like code (C#, VB's look-alike
//! keywords aside, JavaScript, TypeScript, JSON), CSS, markup (HTML, XML, ASP.NET pages and controls, Razor, config,
//! resx) and SQL. Not a parser: keywords, strings, comments, numbers, tags and attributes, with a block comment (or
//! markup comment) carried from line to line. A file of another kind is plain.

use std::ops::Range;

/// What a piece of a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Keyword,
    String,
    Comment,
    Number,
    /// A markup tag's name.
    Tag,
    /// A markup attribute's name, or a CSS property's.
    Attribute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    CLike(&'static [&'static str]),
    Css,
    Markup,
    Sql,
    Plain,
}

const CSHARP: &[&str] = &[
    "abstract", "as", "async", "await", "base", "bool", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "decimal", "default", "delegate", "do", "double", "else", "enum", "event", "explicit", "extern", "false",
    "finally", "float", "for", "foreach", "get", "if", "implicit", "in", "int", "interface", "internal", "is", "lock",
    "long", "namespace", "new", "null", "object", "operator", "out", "override", "params", "private", "protected",
    "public", "readonly", "record", "ref", "return", "sealed", "set", "short", "static", "string", "struct", "switch",
    "this", "throw", "true", "try", "typeof", "uint", "ulong", "using", "var", "virtual", "void", "while", "yield",
];

const JAVASCRIPT: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else",
    "export", "extends", "false", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "interface",
    "let", "new", "null", "of", "return", "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof",
    "undefined", "var", "void", "while", "yield",
];

const JSON: &[&str] = &["true", "false", "null"];

const SQL: &[&str] = &[
    "add", "alter", "and", "as", "begin", "by", "case", "create", "declare", "default", "delete", "desc", "distinct",
    "drop", "else", "end", "exec", "exists", "from", "go", "group", "having", "if", "in", "index", "insert", "into",
    "is", "join", "key", "left", "like", "not", "null", "on", "or", "order", "primary", "procedure", "return", "select",
    "set", "table", "then", "top", "union", "update", "values", "view", "when", "where", "with",
];

/// The language of a file by its extension.
pub fn language_of(path: &str) -> Language {
    let extension = path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    match extension.as_str() {
        "cs" => Language::CLike(CSHARP),
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => Language::CLike(JAVASCRIPT),
        "json" => Language::CLike(JSON),
        "css" | "scss" | "less" => Language::Css,
        "html" | "htm" | "xml" | "config" | "resx" | "ascx" | "aspx" | "master" | "cshtml" | "vbhtml" | "razor" | "svg"
        | "xsl" | "xslt" | "dnn" | "manifest" | "asmx" => Language::Markup,
        "sql" => Language::Sql,
        _ => Language::Plain,
    }
}

/// What is still open at a line's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Open {
    #[default]
    Nothing,
    /// `/* … */`
    BlockComment,
    /// `<!-- … -->`
    MarkupComment,
    /// Inside a tag (`<a href="…"` across lines).
    Tag,
}

/// The coloured pieces of every line of a text, in order (byte ranges within each line).
pub fn highlight(language: Language, lines: &[&str]) -> Vec<Vec<(Range<usize>, Token)>> {
    let mut open = Open::Nothing;
    lines
        .iter()
        .map(|line| {
            let (spans, still) = match language {
                Language::Plain => (Vec::new(), Open::Nothing),
                Language::CLike(keywords) => code(line, keywords, false, open),
                Language::Sql => code(line, SQL, true, open),
                Language::Css => code(line, &[], false, open),
                Language::Markup => markup(line, open),
            };
            open = still;
            spans
        })
        .collect()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$' || c == '@'
}

/// Code: comments, strings, numbers and `keywords` (`any_case` for SQL).
fn code(line: &str, keywords: &[&str], any_case: bool, open: Open) -> (Vec<(Range<usize>, Token)>, Open) {
    let mut spans = Vec::new();
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let end_of = |i: usize| chars.get(i).map_or(line.len(), |(at, _)| *at);
    let mut i = 0;
    if open == Open::BlockComment {
        match line.find("*/") {
            Some(at) => {
                spans.push((0..at + 2, Token::Comment));
                i = chars.iter().position(|(b, _)| *b >= at + 2).unwrap_or(chars.len());
            }
            None => return (vec![(0..line.len(), Token::Comment)], Open::BlockComment),
        }
    }
    while i < chars.len() {
        let (start, c) = chars[i];
        let rest = &line[start..];
        if rest.starts_with("//") || (any_case && rest.starts_with("--")) {
            spans.push((start..line.len(), Token::Comment));
            break;
        }
        if let Some(inside) = rest.strip_prefix("/*") {
            match inside.find("*/") {
                Some(at) => {
                    let end = start + 2 + at + 2;
                    spans.push((start..end, Token::Comment));
                    i = chars.iter().position(|(b, _)| *b >= end).unwrap_or(chars.len());
                    continue;
                }
                None => {
                    spans.push((start..line.len(), Token::Comment));
                    return (spans, Open::BlockComment);
                }
            }
        }
        if c == '"' || c == '\'' || c == '`' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].1 != c {
                if chars[j].1 == '\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = end_of((j + 1).min(chars.len()));
            spans.push((start..end, Token::String));
            i = j + 1;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_word(chars[i - 1].1)) {
            let mut j = i;
            while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '.') {
                j += 1;
            }
            spans.push((start..end_of(j), Token::Number));
            i = j;
            continue;
        }
        if is_word(c) {
            let mut j = i;
            while j < chars.len() && is_word(chars[j].1) {
                j += 1;
            }
            let word = &line[start..end_of(j)];
            let known = if any_case {
                keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
            } else {
                keywords.contains(&word)
            };
            if known {
                spans.push((start..end_of(j), Token::Keyword));
            } else if keywords.is_empty() && chars.get(j).is_some_and(|(_, n)| *n == ':') && line[..start].trim().is_empty() {
                // CSS: a property at the start of its line.
                spans.push((start..end_of(j), Token::Attribute));
            }
            i = j;
            continue;
        }
        if c == '@' || (keywords.is_empty() && c == '#') {
            i += 1;
            continue;
        }
        i += 1;
    }
    (spans, Open::Nothing)
}

/// Markup: comments, tag names, attribute names and their values.
fn markup(line: &str, open: Open) -> (Vec<(Range<usize>, Token)>, Open) {
    let mut spans = Vec::new();
    let mut state = open;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        match state {
            Open::MarkupComment => match line[i..].find("-->") {
                Some(at) => {
                    spans.push((i..i + at + 3, Token::Comment));
                    i += at + 3;
                    state = Open::Nothing;
                }
                None => {
                    spans.push((i..line.len(), Token::Comment));
                    return (spans, Open::MarkupComment);
                }
            },
            Open::Tag => {
                let c = bytes[i];
                if c == b'>' {
                    state = Open::Nothing;
                    i += 1;
                } else if c == b'"' || c == b'\'' {
                    let end = line[i + 1..].find(c as char).map_or(line.len(), |at| i + 1 + at + 1);
                    spans.push((i..end, Token::String));
                    i = end;
                } else if (c as char).is_ascii_alphabetic() || c == b'_' {
                    let end = line[i..]
                        .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.')))
                        .map_or(line.len(), |at| i + at);
                    spans.push((i..end, Token::Attribute));
                    i = end;
                } else {
                    i += 1;
                }
            }
            _ => {
                if line[i..].starts_with("<!--") {
                    state = Open::MarkupComment;
                    continue;
                }
                if bytes[i] == b'<' {
                    let name_start = i + 1 + usize::from(matches!(bytes.get(i + 1), Some(b'/' | b'?' | b'!' | b'%')));
                    let name_end = line[name_start..]
                        .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.')))
                        .map_or(line.len(), |at| name_start + at);
                    if name_end > name_start {
                        spans.push((name_start..name_end, Token::Tag));
                        state = Open::Tag;
                        i = name_end;
                        continue;
                    }
                }
                i += line[i..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    (spans, if state == Open::Tag { Open::Tag } else { Open::Nothing })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(language: Language, text: &str) -> Vec<Vec<(String, Token)>> {
        let lines: Vec<&str> = text.lines().collect();
        highlight(language, &lines)
            .into_iter()
            .zip(&lines)
            .map(|(spans, line)| spans.into_iter().map(|(r, t)| (line[r].to_string(), t)).collect())
            .collect()
    }

    #[test]
    fn languages_come_from_the_extension() {
        assert_eq!(language_of("App_Code/Cart.cs"), Language::CLike(CSHARP));
        assert_eq!(language_of("web.config"), Language::Markup);
        assert_eq!(language_of("View.ascx"), Language::Markup);
        assert_eq!(language_of("skin.CSS"), Language::Css);
        assert_eq!(language_of("logo.png"), Language::Plain);
    }

    #[test]
    fn code_has_keywords_strings_numbers_and_comments() {
        let t = tokens(language_of("a.cs"), "public int x = 42; // the answer\nvar s = \"a \\\" b\";");
        assert_eq!(
            t[0],
            [
                ("public".into(), Token::Keyword),
                ("int".into(), Token::Keyword),
                ("42".into(), Token::Number),
                ("// the answer".into(), Token::Comment)
            ]
        );
        assert_eq!(t[1], [("var".into(), Token::Keyword), ("\"a \\\" b\"".into(), Token::String)]);
    }

    #[test]
    fn a_block_comment_runs_over_lines() {
        let t = tokens(language_of("a.js"), "let a; /* start\nmiddle\nend */ return 1;");
        assert_eq!(t[0].last().unwrap(), &("/* start".to_string(), Token::Comment));
        assert_eq!(t[1], [("middle".into(), Token::Comment)]);
        assert_eq!(t[2][0], ("end */".into(), Token::Comment));
        assert_eq!(t[2][1], ("return".into(), Token::Keyword));
    }

    #[test]
    fn markup_has_tags_attributes_values_and_comments() {
        let t = tokens(Language::Markup, "<add key=\"x\" value='1' /><!-- note\n --> <div class=\"a\"");
        assert_eq!(
            t[0],
            [
                ("add".into(), Token::Tag),
                ("key".into(), Token::Attribute),
                ("\"x\"".into(), Token::String),
                ("value".into(), Token::Attribute),
                ("'1'".into(), Token::String),
                ("<!-- note".into(), Token::Comment),
            ]
        );
        assert_eq!(t[1][0], (" -->".into(), Token::Comment));
        assert_eq!(t[1][1], ("div".into(), Token::Tag));
    }

    #[test]
    fn sql_keywords_in_any_case_and_its_comments() {
        let t = tokens(Language::Sql, "select * FROM Users -- all");
        assert_eq!(t[0][0], ("select".into(), Token::Keyword));
        assert_eq!(t[0][1], ("FROM".into(), Token::Keyword));
        assert_eq!(t[0][2], ("-- all".into(), Token::Comment));
    }

    #[test]
    fn css_properties_and_numbers() {
        let t = tokens(Language::Css, "  color: red; width: 10px; /* x */");
        assert_eq!(t[0][0], ("color".into(), Token::Attribute));
        assert!(t[0].contains(&("10px".to_string(), Token::Number)));
        assert_eq!(t[0].last().unwrap().1, Token::Comment);
    }
}
