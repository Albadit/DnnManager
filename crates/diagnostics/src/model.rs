//! What the Details view shows, as plain data (`Inspector.cs`: `Health`, `InspectorRow`, `InspectorTable`,
//! `InspectorSection`): tabs of sections, each section rows of a label and a value, a table, or a note. The app draws
//! it - see the crate's documentation for how.

/// How a value or a row stands: nothing to say, as it should be, worth a look, wrong - or not known (a gray dot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Health {
    #[default]
    None,
    Ok,
    Warning,
    Bad,
    Unknown,
}

/// A label and what was detected for it - with, when there is more to say, a line under it (`detail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub health: Health,
    pub detail: Option<String>,
}

impl Row {
    /// The address to open when the value is one (http…); the WPF app shows it as a link.
    pub fn link(&self) -> Option<&str> {
        is_address(&self.value).then_some(self.value.as_str())
    }
}

/// A row of a table: a dot in front for its health; `detail` is the tooltip of its last cell. A cell that is an
/// address ([`is_address`]) is a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    pub cells: Vec<String>,
    pub health: Health,
    pub detail: Option<String>,
}

impl TableRow {
    pub fn new(cells: Vec<String>, health: Health, detail: Option<String>) -> Self {
        Self { cells, health, detail }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<TableRow>,
}

impl Table {
    pub fn new(columns: &[&str], rows: Vec<TableRow>) -> Self {
        Self { columns: columns.iter().map(|c| c.to_string()).collect(), rows }
    }
}

/// A group of what was detected - from one place (`source`, shown muted on the right of the title): rows, a table,
/// or a note, in that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub source: Option<String>,
    pub rows: Vec<Row>,
    pub table: Option<Table>,
    pub note: Option<String>,
}

impl Section {
    pub fn new(title: &str, source: Option<&str>) -> Self {
        Self { title: title.into(), source: source.map(str::to_string), rows: Vec::new(), table: None, note: None }
    }

    /// A row - none when there is no value (`InspectorSection.Add` skips a null).
    pub fn add(&mut self, label: &str, value: Option<String>, health: Health, detail: Option<String>) -> &mut Self {
        if let Some(value) = value {
            self.rows.push(Row { label: label.into(), value, health, detail });
        }
        self
    }

    /// A plain row: a value with nothing to say about it.
    pub fn text(&mut self, label: &str, value: impl Into<String>) -> &mut Self {
        self.add(label, Some(value.into()), Health::None, None)
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// The tabs of the Details view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabKind {
    General,
    Iis,
    Dnn,
    Database,
    Advanced,
}

impl TabKind {
    pub const ALL: [TabKind; 5] = [TabKind::General, TabKind::Iis, TabKind::Dnn, TabKind::Database, TabKind::Advanced];

    pub fn title(self) -> &'static str {
        match self {
            TabKind::General => "General",
            TabKind::Iis => "IIS",
            TabKind::Dnn => "DNN",
            TabKind::Database => "Database",
            TabKind::Advanced => "Advanced",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub kind: TabKind,
    pub sections: Vec<Section>,
}

/// What every tab shows while the site is read for the first time (`ProjectView.ShowReading`).
pub fn reading() -> Vec<Section> {
    vec![
        Section::new("Reading…", None)
            .with_note("IIS, the site's folder, web.config, bin and its database are being read."),
    ]
}

/// An address the user can open in the browser.
pub fn is_address(text: &str) -> bool {
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_value_is_no_row_and_addresses_are_links() {
        let mut section = Section::new("Site", Some("IIS"));
        section.add("Preload", None, Health::Ok, None).text("Name", "shop");
        assert_eq!(section.rows.len(), 1);
        assert_eq!(section.rows[0].link(), None);
        section.text("Site", "HTTPS://shop.me");
        assert_eq!(section.rows[1].link(), Some("HTTPS://shop.me"));
        assert!(!is_address("ftp://x"));
    }
}
