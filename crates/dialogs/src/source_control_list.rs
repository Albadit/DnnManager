//! The Source Control page's list as rows, apart from GPUI: the files in sections - Conflicts, Changes (to upload),
//! Server changes (to download) and, when asked for, Unchanged - each a folder tree (folders first, each with the
//! number of files in it), filtered by text and change, folders and sections collapsible. Its own module so the
//! grouping is tested without a window.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use dnnm_application::source_control::model::{Change, Entry};

/// A part of the list, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Conflicts,
    /// What differs here: to upload (or, for a deletion here, to delete on the server).
    Changes,
    /// What differs on the server: to download.
    Server,
    Unchanged,
}

impl Section {
    pub fn of(change: Change) -> Section {
        match change {
            Change::Conflict => Section::Conflicts,
            Change::Modified | Change::Added | Change::Deleted | Change::DeletedOnServer => Section::Changes,
            Change::Incoming | Change::RemoteOnly => Section::Server,
            Change::Unchanged => Section::Unchanged,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Section::Conflicts => "Conflicts",
            Section::Changes => "Changes",
            Section::Server => "Server changes",
            Section::Unchanged => "Unchanged",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Section::Conflicts => "Changed here and on the server since the last sync: compare, then keep one side",
            Section::Changes => "Changed in the project: upload them to the server",
            Section::Server => "Changed on the server, or only there: download them",
            Section::Unchanged => "The same in the project and on the server",
        }
    }
}

/// What the list shows: files whose path holds `text` (any case), of the changes in `changes` (all when empty),
/// and the unchanged ones only when `unchanged`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only this part of the project (a module's or a skin's folder, from [`dnn_scopes`]); None: all of it.
    pub scope: Option<String>,
    pub text: String,
    pub changes: BTreeSet<Change>,
    pub unchanged: bool,
}

impl Filter {
    fn keeps(&self, entry: &Entry) -> bool {
        if let Some(scope) = &self.scope
            && !entry.path.to_lowercase().starts_with(&format!("{}/", scope.to_lowercase()))
        {
            return false;
        }
        if entry.change == Change::Unchanged && !self.unchanged {
            return false;
        }
        if !self.changes.is_empty() && entry.change != Change::Unchanged && !self.changes.contains(&entry.change) {
            return false;
        }
        let text = self.text.trim().to_lowercase();
        text.is_empty() || entry.path.to_lowercase().contains(&text)
    }
}

/// A part of a DNN site that can be deployed on its own: its name in the picker and its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub label: String,
    pub folder: String,
}

/// The modules, skins and containers among `entries`: `DesktopModules/<module>`, and `Portals/<portal>/Skins/<skin>`
/// or `…/Containers/<container>` (the shared ones in `_default`), sorted by their names.
pub fn dnn_scopes(entries: &[Entry]) -> Vec<Scope> {
    let mut found: BTreeMap<String, Scope> = BTreeMap::new();
    for entry in entries {
        let parts: Vec<&str> = entry.path.split('/').collect();
        let scope = match parts.as_slice() {
            [first, module, _, ..] if first.eq_ignore_ascii_case("DesktopModules") => {
                Some((format!("Module: {module}"), format!("{first}/{module}")))
            }
            [first, portal, kind, name, _, ..]
                if first.eq_ignore_ascii_case("Portals")
                    && (kind.eq_ignore_ascii_case("Skins") || kind.eq_ignore_ascii_case("Containers")) =>
            {
                let what = if kind.eq_ignore_ascii_case("Skins") { "Skin" } else { "Container" };
                let whose = if portal.eq_ignore_ascii_case("_default") { String::new() } else { format!(" (portal {portal})") };
                Some((format!("{what}: {name}{whose}"), format!("{first}/{portal}/{kind}/{name}")))
            }
            _ => None,
        };
        if let Some((label, folder)) = scope {
            found.entry(label.to_lowercase()).or_insert(Scope { label, folder });
        }
    }
    found.into_values().collect()
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Section { section: Section, count: usize, collapsed: bool },
    Folder { section: Section, path: String, name: String, depth: usize, count: usize, collapsed: bool },
    /// `entry` is the file's place in the entries given.
    File { section: Section, entry: usize, name: String, folder: String, depth: usize },
}

impl Row {
    /// The key a collapsed section or folder is kept under.
    pub fn key(&self) -> Option<String> {
        match self {
            Row::Section { section, .. } => Some(collapse_key(*section, "")),
            Row::Folder { section, path, .. } => Some(collapse_key(*section, path)),
            Row::File { .. } => None,
        }
    }
}

pub fn collapse_key(section: Section, path: &str) -> String {
    format!("{section:?}/{path}")
}

#[derive(Default)]
struct Node {
    /// Folders by their lower-case name: their name as written, and what is in them.
    folders: BTreeMap<String, (String, Node)>,
    files: Vec<usize>,
    count: usize,
}

fn insert(node: &mut Node, parts: &[&str], entry: usize) {
    node.count += 1;
    match parts {
        [] | [_] => node.files.push(entry),
        [folder, rest @ ..] => {
            let (_, child) = node.folders.entry(folder.to_lowercase()).or_insert_with(|| (folder.to_string(), Node::default()));
            insert(child, rest, entry);
        }
    }
}

/// The list's rows for `entries` as `filter` and `collapsed` (keys from [`Row::key`]) say.
pub fn rows(entries: &[Entry], filter: &Filter, collapsed: &HashSet<String>) -> Vec<Row> {
    let mut sections: BTreeMap<Section, Node> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate().filter(|(_, e)| filter.keeps(e)) {
        let parts: Vec<&str> = entry.path.split('/').collect();
        insert(sections.entry(Section::of(entry.change)).or_default(), &parts, i);
    }
    let mut rows = Vec::new();
    for (section, node) in sections {
        let shut = collapsed.contains(&collapse_key(section, ""));
        rows.push(Row::Section { section, count: node.count, collapsed: shut });
        if !shut {
            walk(entries, section, &node, "", 0, collapsed, &mut rows);
        }
    }
    rows
}

fn walk(
    entries: &[Entry],
    section: Section,
    node: &Node,
    path: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    rows: &mut Vec<Row>,
) {
    for (name, child) in node.folders.values() {
        let folder = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
        let shut = collapsed.contains(&collapse_key(section, &folder));
        rows.push(Row::Folder {
            section,
            path: folder.clone(),
            name: name.clone(),
            depth,
            count: child.count,
            collapsed: shut,
        });
        if !shut {
            walk(entries, section, child, &folder, depth + 1, collapsed, rows);
        }
    }
    let mut files = node.files.clone();
    files.sort_by_key(|&i| entries[i].path.to_lowercase());
    for i in files {
        let name = entries[i].path.rsplit('/').next().unwrap_or_default().to_string();
        rows.push(Row::File { section, entry: i, name, folder: path.to_string(), depth });
    }
}

/// The entries of the files in a row: a file itself, every file in a folder or a section (as shown - the filter
/// applies, collapsing doesn't).
pub fn files_in(row: &Row, entries: &[Entry], filter: &Filter) -> Vec<usize> {
    match row {
        Row::File { entry, .. } => vec![*entry],
        Row::Folder { section, path, .. } => {
            let prefix = format!("{}/", path.to_lowercase());
            (0..entries.len())
                .filter(|&i| {
                    let e = &entries[i];
                    filter.keeps(e) && Section::of(e.change) == *section && e.path.to_lowercase().starts_with(&prefix)
                })
                .collect()
        }
        Row::Section { section, .. } => (0..entries.len())
            .filter(|&i| filter.keeps(&entries[i]) && Section::of(entries[i].change) == *section)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dnnm_application::source_control::model::FileMeta;

    fn entry(path: &str, change: Change) -> Entry {
        Entry { path: path.into(), local: Some(FileMeta::new(1, 1)), remote: None, change }
    }

    fn entries() -> Vec<Entry> {
        vec![
            entry("web.config", Change::Modified),
            entry("Portals/0/skin.css", Change::Modified),
            entry("Portals/0/Images/logo.png", Change::Added),
            entry("Portals/1/a.css", Change::Incoming),
            entry("bin/Shop.dll", Change::Conflict),
            entry("same.txt", Change::Unchanged),
        ]
    }

    fn outline(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Section { section, count, .. } => format!("# {} ({count})", section.title()),
                Row::Folder { name, depth, count, .. } => format!("{}{name}/ ({count})", "  ".repeat(*depth)),
                Row::File { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
            })
            .collect()
    }

    #[test]
    fn files_are_in_sections_as_folder_trees_folders_first() {
        let rows = rows(&entries(), &Filter::default(), &HashSet::new());
        assert_eq!(
            outline(&rows),
            [
                "# Conflicts (1)",
                "bin/ (1)",
                "  Shop.dll",
                "# Changes (3)",
                "Portals/ (2)",
                "  0/ (2)",
                "    Images/ (1)",
                "      logo.png",
                "    skin.css",
                "web.config",
                "# Server changes (1)",
                "Portals/ (1)",
                "  1/ (1)",
                "    a.css",
            ]
        );
    }

    #[test]
    fn collapsing_hides_what_is_inside() {
        let collapsed = HashSet::from([collapse_key(Section::Changes, "Portals/0"), collapse_key(Section::Conflicts, "")]);
        let rows = rows(&entries(), &Filter::default(), &collapsed);
        assert_eq!(
            outline(&rows),
            ["# Conflicts (1)", "# Changes (3)", "Portals/ (2)", "  0/ (2)", "web.config", "# Server changes (1)", "Portals/ (1)", "  1/ (1)", "    a.css"]
        );
        assert!(matches!(rows[3], Row::Folder { collapsed: true, .. }));
    }

    #[test]
    fn the_filter_takes_text_changes_and_unchanged() {
        let text = Filter { text: "PORTALS/0".into(), ..Default::default() };
        assert_eq!(rows(&entries(), &text, &HashSet::new()).iter().filter(|r| matches!(r, Row::File { .. })).count(), 2);
        let added = Filter { changes: BTreeSet::from([Change::Added]), ..Default::default() };
        let files: Vec<String> = rows(&entries(), &added, &HashSet::new())
            .into_iter()
            .filter_map(|r| if let Row::File { name, .. } = r { Some(name) } else { None })
            .collect();
        assert_eq!(files, ["logo.png"]);
        let all = Filter { unchanged: true, ..Default::default() };
        assert!(outline(&rows(&entries(), &all, &HashSet::new())).contains(&"# Unchanged (1)".to_string()));
    }

    #[test]
    fn the_dnn_parts_of_a_site_are_scopes() {
        let e = vec![
            entry("DesktopModules/Shop/View.ascx", Change::Modified),
            entry("DesktopModules/Shop/App_LocalResources/View.ascx.resx", Change::Modified),
            entry("Portals/_default/Skins/Xcillion/home.ascx", Change::Added),
            entry("Portals/0/Containers/Boxed/Title.ascx", Change::Added),
            entry("DesktopModules/readme.txt", Change::Added),
            entry("web.config", Change::Modified),
        ];
        let scopes = dnn_scopes(&e);
        let labels: Vec<&str> = scopes.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Container: Boxed (portal 0)", "Module: Shop", "Skin: Xcillion"]);
        let shop = Filter { scope: Some(scopes[1].folder.clone()), ..Default::default() };
        let files = rows(&e, &shop, &HashSet::new()).into_iter().filter(|r| matches!(r, Row::File { .. })).count();
        assert_eq!(files, 2, "only the module's files");
    }

    /// Many thousands of files are grouped quickly enough to redraw at once.
    #[test]
    fn twenty_thousand_files_are_grouped_quickly() {
        let e: Vec<Entry> = (0..20_000)
            .map(|i| entry(&format!("Portals/{}/Images/{}/img{i}.png", i % 7, i % 50), if i % 3 == 0 { Change::Added } else { Change::Modified }))
            .collect();
        let started = std::time::Instant::now();
        let shown = rows(&e, &Filter::default(), &HashSet::new());
        let scopes = dnn_scopes(&e);
        assert!(shown.len() > 20_000 && scopes.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[test]
    fn a_folder_or_section_holds_its_files() {
        let e = entries();
        let filter = Filter::default();
        let rows = rows(&e, &filter, &HashSet::new());
        let portals_0 = rows.iter().find(|r| matches!(r, Row::Folder { path, section: Section::Changes, .. } if path == "Portals/0")).unwrap();
        let paths: Vec<&str> = files_in(portals_0, &e, &filter).iter().map(|&i| e[i].path.as_str()).collect();
        assert_eq!(paths, ["Portals/0/skin.css", "Portals/0/Images/logo.png"]);
        assert_eq!(files_in(&rows[3], &e, &filter).len(), 3, "the Changes section");
    }
}
