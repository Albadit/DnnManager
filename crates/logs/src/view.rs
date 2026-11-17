//! What the Logs tab does apart from drawing (`LogsView.xaml.cs`, `LogLine`): its texts, the log picker's headings,
//! which log it opens, a line's colour.

use crate::catalog::SiteLogSource;

/// The site picker's first entry, above the IIS sites by name: DNN Manager's own logs (`AppLogChoice`).
pub const APP_CHOICE: &str = crate::catalog::APP_GROUP;

/// Shown before a log is chosen.
pub const CHOOSE_HINT: &str = "Choose a website and one of its logs above.";

/// How long the view waits for a log's first lines before it says the log is empty (`LogsView.Open`).
pub const EMPTY_HINT_AFTER: std::time::Duration = std::time::Duration::from_millis(800);

/// Shown when nothing came [`EMPTY_HINT_AFTER`] the log was opened.
pub fn empty_hint(source: &SiteLogSource) -> &'static str {
    if source.file_path.is_none() {
        "No events about this site yet - new ones show here as they are logged."
    } else {
        "This log is empty - new lines show here as they are written."
    }
}

/// The log shown, as the window names it: "ceesboer - DNN log - 2026.10.01"; DNN Manager's own logs and the hosts
/// file name themselves.
pub fn current_title(site: &str, source: &SiteLogSource) -> String {
    if source.is_app_log() { source.title.clone() } else { format!("{site} - {}", source.title) }
}

/// One entry of the log picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogMenuItem {
    /// A group's name ("DNN", "IIS", "Windows"): semibold, not selectable.
    Heading(String),
    /// The source at this index, indented under its heading, its description as its tooltip.
    Source(usize),
}

/// The sources under a heading per group, in their order.
pub fn menu(sources: &[SiteLogSource]) -> Vec<LogMenuItem> {
    let mut items = Vec::new();
    let mut group: Option<&str> = None;
    for (index, source) in sources.iter().enumerate() {
        if group != Some(source.group.as_str()) {
            group = Some(&source.group);
            items.push(LogMenuItem::Heading(source.group.clone()));
        }
        items.push(LogMenuItem::Source(index));
    }
    items
}

/// Which source to open: the first `wanted`, else the first log that isn't the hosts file, else the first.
/// - a site chosen in the picker: `|_| false`;
/// - the log shown before a restart: `|s| s.group == group && s.title == title`;
/// - DNN Manager's log by title, or its newest when None: `|s| title.map_or(!s.whole, |t| s.title == t)`.
pub fn choose(sources: &[SiteLogSource], wanted: impl Fn(&SiteLogSource) -> bool) -> Option<usize> {
    sources.iter().position(wanted).or_else(|| sources.iter().position(|s| !s.whole)).or(if sources.is_empty() {
        None
    } else {
        Some(0)
    })
}

/// How a line is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLineLevel {
    Normal,
    Warning,
    Error,
    /// DNN Manager's own notes ("--- the file was emptied …"): muted.
    Note,
}

/// A line as the view keeps it - tabs as four spaces (the text is drawn and selected in columns) - and its colour.
pub fn log_line(text: &str) -> (String, LogLineLevel) {
    let text = if text.contains('\t') { text.replace('\t', "    ") } else { text.to_string() };
    let level = line_level(&text);
    (text, level)
}

/// DNN (log4net) writes "[ERROR]" / "[WARN]", the event logs "  ERROR  " / "  WARNING  ", DNN Manager
/// "[error]" / "[critical]" / "[warning]".
pub fn line_level(text: &str) -> LogLineLevel {
    let has = |s: &str| text.contains(s);
    if text.starts_with("---") {
        LogLineLevel::Note
    } else if has(" ERROR") || has(" FATAL") || has("[ERROR") || has("[FATAL") || has("[error]") || has("[critical]") {
        LogLineLevel::Error
    } else if has(" WARN") || has("[WARN") || has("[warning]") {
        LogLineLevel::Warning
    } else {
        LogLineLevel::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn source(group: &str, title: &str, whole: bool) -> SiteLogSource {
        SiteLogSource {
            group: group.into(),
            title: title.into(),
            description: String::new(),
            file_path: Some(Path::new("x").into()),
            events: None,
            whole,
        }
    }

    #[test]
    fn lines_are_coloured_by_what_their_writer_marks() {
        assert_eq!(line_level("2026-10-01 12:00:00,123 [w3wp][ERROR] DotNetNuke"), LogLineLevel::Error);
        assert_eq!(line_level("2026-10-01 14:03:22  ERROR  ASP.NET (1309)"), LogLineLevel::Error);
        assert_eq!(line_level("14:58:19 [critical] App: Unhandled"), LogLineLevel::Error);
        assert_eq!(line_level("2026-10-01 14:03:22  WARNING  WAS (5011)"), LogLineLevel::Warning);
        assert_eq!(line_level("14:58:19 [warning] IisManager: busy"), LogLineLevel::Warning);
        assert_eq!(line_level("--- changed at 12:00:00; read again ---"), LogLineLevel::Note);
        assert_eq!(line_level("GET /default.aspx 200"), LogLineLevel::Normal);
        assert_eq!(log_line("a\tb"), ("a    b".to_string(), LogLineLevel::Normal));
    }

    #[test]
    fn the_picker_has_a_heading_per_group() {
        let sources = [source("DNN", "a", false), source("DNN", "b", false), source("IIS", "c", false)];
        assert_eq!(
            menu(&sources),
            [
                LogMenuItem::Heading("DNN".into()),
                LogMenuItem::Source(0),
                LogMenuItem::Source(1),
                LogMenuItem::Heading("IIS".into()),
                LogMenuItem::Source(2)
            ]
        );
    }

    #[test]
    fn a_log_not_the_hosts_file_is_opened_unless_asked_for() {
        let sources =
            [source("Windows", "Hosts file", true), source("DNN Manager", "DNN Manager log - 2026-10-04", false)];
        assert_eq!(choose(&sources, |_| false), Some(1));
        assert_eq!(choose(&sources, |s| s.title == "Hosts file"), Some(0));
        assert_eq!(choose(&sources[..1], |_| false), Some(0));
        assert_eq!(choose(&[], |_| true), None);
        assert_eq!(current_title("shop", &sources[1]), "DNN Manager log - 2026-10-04");
        assert_eq!(current_title("shop", &source("DNN", "DNN log - 2026.10.01", false)), "shop - DNN log - 2026.10.01");
        assert_eq!(empty_hint(&sources[1]), "This log is empty - new lines show here as they are written.");
    }
}
