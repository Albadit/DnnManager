//! What a site was like before an upgrade step and what became of its content
//! (`src/DnnManager.Application/Upgrades/DnnUpgradeChecks.cs`: the records and `DnnContentComparison`).

use std::collections::{HashMap, HashSet};

use super::facts::{DnnContentCounts, DnnContentItem};

/// An account the checks sign in with after each step - never kept: asked for when the upgrade starts.
#[derive(Clone, PartialEq, Eq)]
pub struct DnnTestAccount {
    pub user_name: String,
    pub password: String,
    pub is_host: bool,
}

impl std::fmt::Debug for DnnTestAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnnTestAccount").field("user_name", &self.user_name).field("is_host", &self.is_host).finish()
    }
}

/// A page the checks open, and what it showed - None when it was fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnPageState {
    pub portal: String,
    pub url: String,
    pub problem: Option<String>,
}

/// What a site was like right before a step - what the checks after it compare with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DnnSiteState {
    pub counts: Option<DnnContentCounts>,
    pub extensions: Vec<String>,
    pub pages: Vec<DnnPageState>,
    pub items: Vec<DnnContentItem>,
}

/// What became of a site's pages, modules and jobs in a step (`DnnContentComparison.Outcome`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentOutcome {
    pub lost: Vec<DnnContentItem>,
    pub removed_with_extension: Vec<DnnContentItem>,
    pub fewer_permissions: Vec<String>,
    pub jobs_gone: Vec<DnnContentItem>,
}

/// Compares a site's pages, modules and jobs one by one: those lost, those DNN's upgrade removed as its own (DNN 10
/// removes the Digital Assets Manager and its Telerik extensions; DNN 8 and 9 replace its Admin and Host pages), and
/// permissions that are fewer on what is still there.
pub fn compare(before: &DnnSiteState, after: &[DnnContentItem], extensions_after: &[String]) -> ContentOutcome {
    let now: HashMap<(&str, i32), &DnnContentItem> = after.iter().map(|i| ((i.kind.as_str(), i.id), i)).collect();
    // What modules can still be made of: the packages, and the module types (DNN's own may have no package).
    let extensions: HashSet<String> = extensions_after
        .iter()
        .cloned()
        .chain(after.iter().filter(|i| i.kind == "type").map(|i| i.name.clone()))
        .map(|e| e.to_lowercase())
        .collect();
    let gone: Vec<&DnnContentItem> =
        before.items.iter().filter(|i| i.kind != "type" && !now.contains_key(&(i.kind.as_str(), i.id))).collect();
    // A module whose extension the upgrade removed went with it; a page went with them when all of its modules did.
    let with_extension: Vec<&DnnContentItem> = gone
        .iter()
        .copied()
        .filter(|i| i.kind == "module" && i.package.as_ref().is_some_and(|p| !extensions.contains(&p.to_lowercase())))
        .collect();
    let pages_with: Vec<&DnnContentItem> = gone
        .iter()
        .copied()
        .filter(|i| {
            let mut modules = before.items.iter().filter(|m| m.kind == "module" && m.page_id == Some(i.id)).peekable();
            i.kind == "page" && modules.peek().is_some() && modules.all(|m| with_extension.contains(&m))
        })
        .collect();
    // DNN's own Admin and Host pages, and the modules on them: DNN 8 moved some into its new admin, DNN 9 put the
    // Persona Bar in their place. A site's own pages aren't under //Admin or //Host.
    let admin_pages: Vec<&DnnContentItem> =
        gone.iter().copied().filter(|i| i.kind == "page" && is_dnn_admin(&i.name)).collect();
    let admin_ids: HashSet<i32> =
        before.items.iter().filter(|i| i.kind == "page" && is_dnn_admin(&i.name)).map(|i| i.id).collect();
    let on_admin_pages: Vec<&DnnContentItem> = gone
        .iter()
        .copied()
        .filter(|i| i.kind == "module" && i.page_id.is_some_and(|p| admin_ids.contains(&p)))
        .collect();
    // DNN's Console module holds nothing of its own - it lists a page's child pages; DNN 9 removes those its site
    // template put on the Activity Feed pages.
    let consoles: Vec<&DnnContentItem> = gone
        .iter()
        .copied()
        .filter(|i| {
            i.kind == "module" && i.package.as_deref().is_some_and(|p| p.eq_ignore_ascii_case("DotNetNuke.Console"))
        })
        .collect();
    let mut expected: Vec<&DnnContentItem> = Vec::new();
    for item in with_extension.into_iter().chain(pages_with).chain(admin_pages).chain(on_admin_pages).chain(consoles) {
        if !expected.contains(&item) {
            expected.push(item);
        }
    }
    let jobs_gone = gone.iter().filter(|i| i.kind == "job").map(|i| (*i).clone()).collect();
    let lost = gone.iter().filter(|i| i.kind != "job" && !expected.contains(i)).map(|i| (*i).clone()).collect();
    let fewer_permissions = before
        .items
        .iter()
        .filter(|i| i.kind != "type")
        .filter_map(|i| {
            let n = now.get(&(i.kind.as_str(), i.id))?;
            (n.permissions < i.permissions)
                .then(|| format!("{} {}: {} → {}", i.kind, i.name, i.permissions, n.permissions))
        })
        .collect();
    ContentOutcome {
        lost,
        removed_with_extension: expected.into_iter().cloned().collect(),
        fewer_permissions,
        jobs_gone,
    }
}

/// A page in DNN's Admin or Host menu, by its path: //Admin, //Admin//LogViewer, //Host//SQL.
fn is_dnn_admin(tab_path: &str) -> bool {
    let lower = tab_path.to_lowercase();
    lower == "//admin" || lower == "//host" || lower.starts_with("//admin//") || lower.starts_with("//host//")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: i32, name: &str, permissions: i32) -> DnnContentItem {
        DnnContentItem { permissions, ..DnnContentItem::new("page", id, name) }
    }

    fn module(id: i32, name: &str, package: &str, page_id: i32, permissions: i32) -> DnnContentItem {
        DnnContentItem {
            package: Some(package.into()),
            page_id: Some(page_id),
            permissions,
            ..DnnContentItem::new("module", id, name)
        }
    }

    fn state(extensions: &[&str], items: Vec<DnnContentItem>) -> DnnSiteState {
        DnnSiteState { extensions: extensions.iter().map(|e| e.to_string()).collect(), items, ..Default::default() }
    }

    fn names(items: &[DnnContentItem]) -> Vec<&str> {
        let mut names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        names.sort();
        names
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn content_is_compared_one_by_one_and_what_dnn_removed_with_its_extension_isnt_lost() {
        // DNN 10 removes the Digital Assets Manager: its module and the page that held only it go - expected.
        let before = state(
            &["DigitalAssetsManagement", "DNN_HTML"],
            vec![
                page(1, "//Home", 3),
                page(2, "//Admin//FileManagement", 2),
                page(3, "//Products", 4),
                module(10, "'Files'", "DigitalAssetsManagement", 2, 0),
                module(11, "'Welcome'", "DNN_HTML", 1, 2),
                module(12, "'Specials'", "DNN_HTML", 3, 0),
                DnnContentItem::new("job", 20, "DotNetNuke.Old.Job"),
            ],
        );
        let after = vec![page(1, "//Home", 3), page(3, "//Products", 1), module(11, "'Welcome'", "DNN_HTML", 1, 2)];
        let outcome = compare(&before, &after, &strings(&["DNN_HTML", "Dnn.ResourceManager"]));
        assert_eq!(names(&outcome.removed_with_extension), vec!["'Files'", "//Admin//FileManagement"]);
        assert_eq!(names(&outcome.lost), vec!["'Specials'"], "A module whose extension is still there was lost.");
        assert_eq!(outcome.fewer_permissions.len(), 1);
        assert!(outcome.fewer_permissions[0].contains("//Products: 4 → 1"));
        assert_eq!(names(&outcome.jobs_gone), vec!["DotNetNuke.Old.Job"]);
    }

    #[test]
    fn dnns_own_admin_and_host_pages_gone_with_an_upgrade_arent_lost_content() {
        let before = state(
            &["DotNetNuke.SQL"],
            vec![
                page(40, "//Host//SQL", 1),
                page(41, "//Admin//LogViewer", 0),
                page(42, "//Administration", 1),
                module(50, "'SQL'", "DotNetNuke.SQL", 40, 0),
                module(51, "'Notes'", "DNN_HTML", 42, 0),
                // 8.0.4 → 9.1.1: the console module on //Admin goes, the page stays.
                page(43, "//Admin", 0),
                module(52, "'Basic Features'", "DotNetNuke.Console", 43, 0),
            ],
        );
        let outcome = compare(
            &before,
            &[page(43, "//Admin", 0)],
            &strings(&["DotNetNuke.SQL", "DNN_HTML", "DotNetNuke.Console"]),
        );
        assert_eq!(
            names(&outcome.removed_with_extension),
            vec!["'Basic Features'", "'SQL'", "//Admin//LogViewer", "//Host//SQL"]
        );
        assert_eq!(names(&outcome.lost), vec!["'Notes'", "//Administration"]);
    }

    #[test]
    fn a_module_whose_module_type_the_upgrade_removed_isnt_lost_content() {
        let before = state(
            &["DNN_HTML"],
            vec![
                DnnContentItem::new("type", 1, "Navigation"),
                DnnContentItem::new("type", 2, "DNN_HTML"),
                module(60, "'Navigation' (#60)", "Navigation", 10, 0),
                module(61, "'Text' (#61)", "DNN_HTML", 10, 0),
                module(62, "'Navigation' (#62)", "DotNetNuke.Console", 11, 0),
            ],
        );
        let outcome = compare(
            &before,
            &[DnnContentItem::new("type", 2, "DNN_HTML")],
            &strings(&["DNN_HTML", "DotNetNuke.Console"]),
        );
        assert_eq!(names(&outcome.removed_with_extension), vec!["'Navigation' (#60)", "'Navigation' (#62)"]);
        assert_eq!(names(&outcome.lost), vec!["'Text' (#61)"], "A module whose type is still there was lost.");
    }
}
