//! The bottom panel's Find bar (`SearchQuery` in `PanelSearch.cs`, `LogView.Find`): Match Case, Whole Word and Regex
//! over a log's lines.
//!
//! Differences from .NET's regex, which the C# uses: Rust's `regex` has no backtracking, so a pattern can't run away
//! and needs no per-line timeout (the whole pass keeps the C#'s 250 ms budget); look-around and backreferences aren't
//! supported and are reported as the pattern's error; positions are byte offsets in the UTF-8 line, not UTF-16 ones.
//! A whole word is `\b{start-half}…\b{end-half}` - no word character right before or after, as the C#'s
//! `(?<![\p{L}\p{N}_])…(?![\p{L}\p{N}_])`, with Unicode's `\w` as the word characters.

use std::time::{Duration, Instant};

use regex::{Regex, RegexBuilder};

/// A whole search stops after this long: it runs where the UI waits for it.
const PASS_BUDGET: Duration = Duration::from_millis(250);

/// One match: the line's index, and the byte range in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub len: usize,
}

/// What the Find bar looks for.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    regex: Option<Regex>,
    error: Option<String>,
}

/// What a search through every line found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResult {
    /// In reading order.
    pub matches: Vec<Match>,
    /// It took too long and stopped before the end: the matches after where it stopped aren't counted.
    pub stopped: bool,
}

impl SearchQuery {
    pub fn new(text: &str, match_case: bool, whole_word: bool, use_regex: bool) -> Self {
        if text.is_empty() {
            return Self { regex: None, error: None };
        }
        let pattern = if use_regex { text.to_string() } else { regex::escape(text) };
        let pattern = if whole_word { format!(r"\b{{start-half}}(?:{pattern})\b{{end-half}}") } else { pattern };
        match RegexBuilder::new(&pattern).case_insensitive(!match_case).build() {
            Ok(regex) => Self { regex: Some(regex), error: None },
            Err(e) => Self { regex: None, error: Some(e.to_string()) },
        }
    }

    /// Why the regular expression can't be used; None when it can.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Nothing to look for: no text, or a regular expression that isn't one.
    pub fn is_empty(&self) -> bool {
        self.regex.is_none()
    }

    /// Where it is in `line`: (start, length) in bytes of each match, empty ones left out.
    pub fn matches(&self, line: &str) -> Vec<(usize, usize)> {
        let Some(regex) = &self.regex else { return Vec::new() };
        regex.find_iter(line).filter(|m| !m.is_empty()).map(|m| (m.start(), m.len())).collect()
    }

    /// Every match in `lines` (`LogView.Find`): its count is what the bar shows, match `i` the one to mark as current.
    pub fn find<S: AsRef<str>>(&self, lines: &[S]) -> SearchResult {
        let mut result = SearchResult::default();
        if self.is_empty() {
            return result;
        }
        let started = Instant::now();
        for (index, line) in lines.iter().enumerate() {
            if started.elapsed() > PASS_BUDGET {
                result.stopped = true;
                break;
            }
            for (start, len) in self.matches(line.as_ref()) {
                result.matches.push(Match { line: index, start, len });
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_ignores_case_unless_asked() {
        let q = SearchQuery::new("error", false, false, false);
        assert_eq!(q.matches("ERROR and Error"), [(0, 5), (10, 5)]);
        let q = SearchQuery::new("error", true, false, false);
        assert_eq!(q.matches("ERROR and error"), [(10, 5)]);
    }

    #[test]
    fn plain_text_is_not_a_pattern() {
        let q = SearchQuery::new("a.b(", false, false, false);
        assert!(q.error().is_none());
        assert_eq!(q.matches("axb( a.b("), [(5, 4)]);
    }

    #[test]
    fn whole_word_has_no_word_character_on_either_side() {
        let q = SearchQuery::new("log", false, true, false);
        assert_eq!(q.matches("log logs catalog log_x (log) log"), [(0, 3), (24, 3), (29, 3)]);
        // As the C#: a search starting with a non-word character isn't found right after a letter.
        let q = SearchQuery::new("-x", false, true, false);
        assert_eq!(q.matches("a-x a -x"), [(6, 2)]);
    }

    #[test]
    fn regex_and_its_error() {
        let q = SearchQuery::new(r"W3SVC\d+", false, false, true);
        assert_eq!(q.matches("/LM/W3SVC/2 w3svc12"), [(12, 7)]);
        let bad = SearchQuery::new("(", false, false, true);
        assert!(bad.is_empty() && bad.error().is_some());
        assert!(bad.find(&["("]).matches.is_empty());
    }

    #[test]
    fn empty_matches_and_empty_text_find_nothing() {
        assert!(SearchQuery::new("x*", false, false, true).matches("abc").is_empty());
        let q = SearchQuery::new("", false, false, false);
        assert!(q.is_empty() && q.error().is_none());
    }

    #[test]
    fn find_goes_through_every_line_in_reading_order() {
        let q = SearchQuery::new("é", false, false, false);
        let result = q.find(&["aé é", "none", "É"]);
        assert_eq!(
            result.matches,
            [
                Match { line: 0, start: 1, len: 2 },
                Match { line: 0, start: 4, len: 2 },
                Match { line: 2, start: 0, len: 2 }
            ]
        );
        assert!(!result.stopped);
    }
}
