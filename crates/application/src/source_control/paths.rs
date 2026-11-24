//! Paths in Source Control: the ignore and protect rules (simple globs), and where a project file goes on the server
//! (the mappings). A project path is relative and `/`-separated whatever the OS (`Portals/0/skin.css`); compared
//! without regard to case, as Windows and IIS treat them.

use super::model::{Environment, Mapping};

/// What is never compared or sent, whatever the environment: made on this PC or by DNN at run time - editor and
/// source-control folders, packages, logs, caches, the search index, the local database, temporary files.
pub const DEFAULT_IGNORE: &[&str] = &[
    ".git/",
    ".svn/",
    ".vs/",
    ".vscode/",
    ".idea/",
    "node_modules/",
    "App_Data/",
    "Portals/_default/Logs/",
    "Portals/_default/Cache/",
    "Portals/*/Cache/",
    "*.tmp",
    "*.log",
    "*.bak",
    "*.swp",
    "~$*",
    "Thumbs.db",
    ".DS_Store",
    "desktop.ini",
];

/// What a Production server's copy is asked about before it is replaced or deleted: its configuration, and what
/// the live site's users and DNN made there (uploads, the portal's own files).
pub const DEFAULT_PROTECTED: &[&str] = &["web.config", "*.config", "Portals/", "App_Data/", "bin/"];

/// Glob rules as `.gitignore` writes them, simplified: `*` any characters within a name, `**` any folders, `?` one
/// character; a pattern with no `/` but a trailing one matches a name anywhere (`node_modules/`), one with a `/`
/// in it is from the project's root (`Portals/_default/Logs/`); a trailing `/` matches a folder and everything in it.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    patterns: Vec<Pattern>,
}

#[derive(Debug, Clone)]
struct Pattern {
    /// Its segments, lower-case.
    parts: Vec<String>,
    /// Matches a folder: the path's start, not the whole of it.
    folder: bool,
    /// No `/` inside: matches at any depth.
    anywhere: bool,
}

impl IgnoreRules {
    pub fn new<S: AsRef<str>>(patterns: impl IntoIterator<Item = S>) -> Self {
        let patterns = patterns
            .into_iter()
            .filter_map(|p| {
                let p = p.as_ref().trim().replace('\\', "/");
                if p.is_empty() || p.starts_with('#') {
                    return None;
                }
                let folder = p.ends_with('/');
                let body = p.trim_matches('/');
                if body.is_empty() {
                    return None;
                }
                // A `/` before the end (a leading one too: `/robots.txt`) ties it to the root.
                let anywhere = !p.trim_end_matches('/').contains('/');
                Some(Pattern { parts: body.split('/').map(str::to_lowercase).collect(), folder, anywhere })
            })
            .collect();
        Self { patterns }
    }

    /// The environment's ignore rules on top of [`DEFAULT_IGNORE`].
    pub fn for_environment(env: &Environment) -> Self {
        Self::new(DEFAULT_IGNORE.iter().map(|s| s.to_string()).chain(env.ignore.iter().cloned()))
    }

    /// The server's protected paths: [`DEFAULT_PROTECTED`] and its own - on every server, whatever kind it was saved
    /// as (the kind isn't asked any more; any server may be the live one).
    pub fn protected(env: &Environment) -> Self {
        Self::new(DEFAULT_PROTECTED.iter().map(|s| s.to_string()).chain(env.protected.iter().cloned()))
    }

    /// Whether a whole folder (a project path) is matched - so a scan doesn't go into it at all.
    pub fn matches_folder(&self, folder: &str) -> bool {
        let segments: Vec<String> = folder.split('/').filter(|s| !s.is_empty()).map(str::to_lowercase).collect();
        !segments.is_empty()
            && self.patterns.iter().any(|p| {
                if p.anywhere {
                    segments.iter().any(|s| glob(&p.parts[0], s))
                } else {
                    (1..=segments.len()).any(|end| parts_match(&p.parts, &segments[..end]))
                }
            })
    }

    /// Whether `path` (a file's project path) is matched - itself, or a folder it is in.
    pub fn matches(&self, path: &str) -> bool {
        let segments: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(str::to_lowercase).collect();
        self.patterns.iter().any(|p| p.matches(&segments))
    }
}

impl Pattern {
    fn matches(&self, segments: &[String]) -> bool {
        if self.anywhere {
            // A name at any depth: a folder pattern a folder the file is in; else the file or any folder it is in.
            let names = if self.folder { &segments[..segments.len().saturating_sub(1)] } else { segments };
            return names.iter().any(|s| glob(&self.parts[0], s));
        }
        // From the root: the path itself or a folder it is in - a folder pattern only a folder.
        (1..=segments.len()).any(|end| (!self.folder || end < segments.len()) && parts_match(&self.parts, &segments[..end]))
    }
}

/// Pattern segments against path segments, `**` standing for any number of folders.
fn parts_match(parts: &[String], segments: &[String]) -> bool {
    match parts.split_first() {
        None => segments.is_empty(),
        Some((first, rest)) if first == "**" => (0..=segments.len()).any(|skip| parts_match(rest, &segments[skip..])),
        Some((first, rest)) => {
            segments.split_first().is_some_and(|(s, tail)| glob(first, s) && parts_match(rest, tail))
        }
    }
}

/// `*` and `?` within one name.
fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A project path made safe: `/`-separated, no empty, `.` or `..` parts - None for one that would leave its folder.
pub fn clean(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// Where a project file goes on the server: the mapping that holds it (the longest local folder) and the full
/// server path. None when no mapping holds it.
pub fn remote_path(mappings: &[Mapping], path: &str) -> Option<(usize, String)> {
    let lower = path.to_lowercase();
    let (index, mapping) = mappings
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            let local = m.local.trim_matches('/').to_lowercase();
            local.is_empty() || lower == local || lower.starts_with(&format!("{local}/"))
        })
        .max_by_key(|(_, m)| m.local.trim_matches('/').len())?;
    let local = mapping.local.trim_matches('/');
    let rest = if local.is_empty() { path } else { path[local.len()..].trim_start_matches('/') };
    Some((index, join_remote(&mapping.remote, rest)))
}

/// A server folder and a relative path, joined with `/`.
pub fn join_remote(folder: &str, rest: &str) -> String {
    let folder = folder.trim_end_matches('/');
    if rest.is_empty() {
        return if folder.is_empty() { "/".into() } else { folder.into() };
    }
    format!("{folder}/{}", rest.trim_start_matches('/'))
}

/// The project path of a file found under a mapping's server folder (`relative`, `/`-separated).
pub fn project_path(mapping: &Mapping, relative: &str) -> String {
    let local = mapping.local.trim_matches('/');
    if local.is_empty() { relative.to_string() } else { format!("{local}/{}", relative.trim_start_matches('/')) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_control::model::EnvironmentKind;

    #[test]
    fn globs_match_within_a_name() {
        assert!(glob("*.log", "error.log"));
        assert!(glob("~$*", "~$report.docx"));
        assert!(glob("a?c", "abc"));
        assert!(!glob("*.log", "error.log.txt"));
        assert!(glob("*", ""));
        assert!(glob("*b*", "abc"));
    }

    #[test]
    fn the_default_rules_leave_out_what_the_server_doesnt_need() {
        let rules = IgnoreRules::new(DEFAULT_IGNORE);
        for path in [
            ".git/config",
            "node_modules/x/index.js",
            "DesktopModules/Shop/node_modules/a.js",
            "App_Data/Database.mdf",
            "Portals/_default/Logs/2026.log",
            "Portals/3/Cache/a.resources",
            "Portals/0/notes.tmp",
            "Thumbs.db",
            "Portals/0/.DS_Store",
        ] {
            assert!(rules.matches(path), "{path}");
        }
        for path in ["web.config", "Portals/0/skin.css", "bin/DotNetNuke.dll", "DesktopModules/Shop/View.ascx", "Logs.aspx"] {
            assert!(!rules.matches(path), "{path}");
        }
    }

    #[test]
    fn a_pattern_with_a_slash_is_from_the_root_and_case_doesnt_matter() {
        let rules = IgnoreRules::new(["Portals/0/Images/", "/robots.txt", "DesktopModules/**/*.cs"]);
        assert!(rules.matches("portals/0/images/logo.png"));
        assert!(!rules.matches("Portals/1/Images/logo.png"));
        assert!(rules.matches("robots.txt"));
        assert!(!rules.matches("Portals/robots.txt"));
        assert!(rules.matches("DesktopModules/Shop/Code/Cart.cs"));
        assert!(rules.matches("DesktopModules/Cart.cs"), "** may be no folder at all");
        assert!(!rules.matches("App_Code/Cart.cs"));
        assert!(!IgnoreRules::new(["", "# a comment", "/"]).matches("anything"));
        let defaults = IgnoreRules::new(DEFAULT_IGNORE);
        assert!(defaults.matches_folder("DesktopModules/Shop/node_modules"));
        assert!(defaults.matches_folder("App_Data"));
        assert!(!defaults.matches_folder("Portals/0"));
        assert!(!defaults.matches_folder(""));
    }

    #[test]
    fn production_protects_its_configuration_and_uploads() {
        let mut env = super::super::model::tests::environment();
        let protected = IgnoreRules::protected(&env);
        assert!(protected.matches("web.config"));
        assert!(protected.matches("Portals/0/Images/logo.png"));
        assert!(protected.matches("bin/Shop.dll"));
        assert!(!protected.matches("DesktopModules/Shop/View.ascx"));
        env.kind = EnvironmentKind::Test;
        assert!(IgnoreRules::protected(&env).matches("web.config"), "on every server, whatever its kind");
        env.protected = vec!["robots.txt".into()];
        assert!(IgnoreRules::protected(&env).matches("robots.txt") && IgnoreRules::protected(&env).matches("web.config"));
    }

    #[test]
    fn a_path_is_mapped_by_its_longest_local_folder() {
        let mappings = vec![
            Mapping { local: String::new(), remote: "/var/www/shop".into() },
            Mapping { local: "DesktopModules/Shop".into(), remote: "/srv/shop-module/".into() },
        ];
        assert_eq!(remote_path(&mappings, "web.config"), Some((0, "/var/www/shop/web.config".into())));
        assert_eq!(
            remote_path(&mappings, "desktopmodules/shop/View.ascx"),
            Some((1, "/srv/shop-module/View.ascx".into()))
        );
        assert_eq!(remote_path(&mappings, "DesktopModules/ShopX/a.js").unwrap().0, 0, "a folder, not a prefix");
        let only = vec![Mapping { local: "Portals/_default/Skins/Mine".into(), remote: "/skins".into() }];
        assert_eq!(remote_path(&only, "web.config"), None, "outside every mapping: not deployed");
        assert_eq!(project_path(&only[0], "home.ascx"), "Portals/_default/Skins/Mine/home.ascx");
        assert_eq!(join_remote("/", "a"), "/a");
        assert_eq!(join_remote("", ""), "/");
    }

    #[test]
    fn a_path_that_climbs_out_is_refused() {
        assert_eq!(clean(r"Portals\0\a.css").as_deref(), Some("Portals/0/a.css"));
        assert_eq!(clean("./a//b/").as_deref(), Some("a/b"));
        assert_eq!(clean("a/../../etc/passwd"), None);
    }
}
