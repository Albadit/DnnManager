//! The Source Control page's list as rows, apart from GPUI: the files in sections - Conflicts, Staged Changes (what
//! Commit uploads), Changes (to upload), Server changes (to download) and, when asked for, Unchanged - as VS Code lists
//! them: flat by path (each file with
//! its folder beside it), or as a folder tree (folders first, each with the number of files in it). Filtered by text,
//! folders and sections collapsible. Its own module so the grouping is tested without a window.

use std::collections::{BTreeMap, HashSet};

use dnnm_application::source_control::model::{Change, Entry};

/// A part of the list, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Conflicts,
    /// Changes here put aside for the next commit (VS Code's staging): Commit sends these.
    Staged,
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
            Section::Staged => "Staged Changes",
            Section::Changes => "Changes",
            Section::Server => "Server changes",
            Section::Unchanged => "Unchanged",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Section::Conflicts => "Changed here and on the server since the last sync: compare, then keep one side",
            Section::Staged => "Staged for the next commit: Commit uploads them to the server",
            Section::Changes => "Changed in the project: stage them, then Commit uploads them to the server",
            Section::Server => "Changed on the server, or only there: download them",
            Section::Unchanged => "The same in the project and on the server",
        }
    }
}

/// Whether a change can be staged: a change in the project (one of the Changes section's).
pub fn stageable(change: Change) -> bool {
    Section::of(change) == Section::Changes
}

/// What the list shows: files whose path holds `text` (any case), and the unchanged ones only when `unchanged`; the
/// `staged` paths (of changes in the project) under Staged Changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    pub unchanged: bool,
    pub staged: HashSet<String>,
}

impl Filter {
    /// The section a file is listed in.
    pub fn section(&self, entry: &Entry) -> Section {
        match Section::of(entry.change) {
            Section::Changes if self.staged.contains(&entry.path) => Section::Staged,
            section => section,
        }
    }

    fn keeps(&self, entry: &Entry) -> bool {
        if entry.change == Change::Unchanged && !self.unchanged {
            return false;
        }
        let text = self.text.trim().to_lowercase();
        text.is_empty() || entry.path.to_lowercase().contains(&text)
    }
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

/// The list's rows for `entries` as `filter` and `collapsed` (keys from [`Row::key`]) say: a folder tree when `tree`,
/// else every file on its own row, sorted by path, with its folder beside its name (VS Code's list view).
pub fn rows(entries: &[Entry], filter: &Filter, collapsed: &HashSet<String>, tree: bool) -> Vec<Row> {
    let mut sections: BTreeMap<Section, Node> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate().filter(|(_, e)| filter.keeps(e)) {
        let section = sections.entry(filter.section(entry)).or_default();
        if tree {
            let parts: Vec<&str> = entry.path.split('/').collect();
            insert(section, &parts, i);
        } else {
            section.count += 1;
            section.files.push(i);
        }
    }
    let mut rows = Vec::new();
    for (section, node) in sections {
        let shut = collapsed.contains(&collapse_key(section, ""));
        rows.push(Row::Section { section, count: node.count, collapsed: shut });
        if shut {
            continue;
        }
        if tree {
            walk(entries, section, &node, "", 0, collapsed, &mut rows);
        } else {
            let mut files = node.files;
            files.sort_by_cached_key(|&i| entries[i].path.to_lowercase());
            for i in files {
                let (folder, name) = entries[i].path.rsplit_once('/').unwrap_or(("", &entries[i].path));
                rows.push(Row::File { section, entry: i, name: name.to_string(), folder: folder.to_string(), depth: 0 });
            }
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
                    filter.keeps(e) && filter.section(e) == *section && e.path.to_lowercase().starts_with(&prefix)
                })
                .collect()
        }
        Row::Section { section, .. } => (0..entries.len())
            .filter(|&i| filter.keeps(&entries[i]) && filter.section(&entries[i]) == *section)
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
        let rows = rows(&entries(), &Filter::default(), &HashSet::new(), true);
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
        let rows = rows(&entries(), &Filter::default(), &collapsed, true);
        assert_eq!(
            outline(&rows),
            ["# Conflicts (1)", "# Changes (3)", "Portals/ (2)", "  0/ (2)", "web.config", "# Server changes (1)", "Portals/ (1)", "  1/ (1)", "    a.css"]
        );
        assert!(matches!(rows[3], Row::Folder { collapsed: true, .. }));
    }

    #[test]
    fn the_list_view_is_flat_by_path_with_each_files_folder() {
        let rows = rows(&entries(), &Filter::default(), &HashSet::new(), false);
        assert_eq!(
            outline(&rows),
            ["# Conflicts (1)", "Shop.dll", "# Changes (3)", "logo.png", "skin.css", "web.config", "# Server changes (1)", "a.css"]
        );
        let folders: Vec<&str> =
            rows.iter().filter_map(|r| if let Row::File { folder, .. } = r { Some(folder.as_str()) } else { None }).collect();
        assert_eq!(folders, ["bin", "Portals/0/Images", "Portals/0", "", "Portals/1"]);
    }

    #[test]
    fn the_filter_takes_text_and_unchanged() {
        let text = Filter { text: "PORTALS/0".into(), ..Default::default() };
        for tree in [true, false] {
            assert_eq!(rows(&entries(), &text, &HashSet::new(), tree).iter().filter(|r| matches!(r, Row::File { .. })).count(), 2);
        }
        let all = Filter { unchanged: true, ..Default::default() };
        assert!(outline(&rows(&entries(), &all, &HashSet::new(), false)).contains(&"# Unchanged (1)".to_string()));
    }

    /// Many thousands of files are grouped quickly enough to redraw at once.
    #[test]
    fn twenty_thousand_files_are_grouped_quickly() {
        let e: Vec<Entry> = (0..20_000)
            .map(|i| entry(&format!("Portals/{}/Images/{}/img{i}.png", i % 7, i % 50), if i % 3 == 0 { Change::Added } else { Change::Modified }))
            .collect();
        let started = std::time::Instant::now();
        let shown = rows(&e, &Filter::default(), &HashSet::new(), true);
        assert!(shown.len() > 20_000);
        assert_eq!(rows(&e, &Filter::default(), &HashSet::new(), false).len(), 20_001, "the Changes section and its files");
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[test]
    fn staged_changes_have_their_own_section_and_only_changes_here_are_staged() {
        let e = entries();
        let staged = Filter {
            staged: HashSet::from(["web.config".to_string(), "Portals/1/a.css".to_string(), "gone.txt".to_string()]),
            ..Default::default()
        };
        let rows = rows(&e, &staged, &HashSet::new(), false);
        assert_eq!(
            outline(&rows),
            ["# Conflicts (1)", "Shop.dll", "# Staged Changes (1)", "web.config", "# Changes (2)", "logo.png", "skin.css", "# Server changes (1)", "a.css"],
            "a server change isn't staged, nor a path no longer listed"
        );
        let paths: Vec<&str> = files_in(&rows[2], &e, &staged).iter().map(|&i| e[i].path.as_str()).collect();
        assert_eq!(paths, ["web.config"]);
        assert!(stageable(Change::Deleted) && stageable(Change::Added) && !stageable(Change::Incoming) && !stageable(Change::Conflict));
    }

    #[test]
    fn a_folder_or_section_holds_its_files() {
        let e = entries();
        let filter = Filter::default();
        let rows = rows(&e, &filter, &HashSet::new(), true);
        let portals_0 = rows.iter().find(|r| matches!(r, Row::Folder { path, section: Section::Changes, .. } if path == "Portals/0")).unwrap();
        let paths: Vec<&str> = files_in(portals_0, &e, &filter).iter().map(|&i| e[i].path.as_str()).collect();
        assert_eq!(paths, ["Portals/0/skin.css", "Portals/0/Images/logo.png"]);
        assert_eq!(files_in(&rows[3], &e, &filter).len(), 3, "the Changes section");
    }
}
