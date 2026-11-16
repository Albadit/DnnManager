//! The site's assemblies (`AssemblyInspector.cs`): every DLL under bin, the folders web.config's probing path adds
//! and the files its codeBase entries point at - read without loading them ([`crate::metadata`]): versions,
//! duplicates, binding redirects that don't match what is there, codeBase files that aren't there, and references to
//! assemblies at a version the site doesn't have, or that are nowhere (the site, the GAC, the .NET Framework).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::metadata::{self, AssemblyVersion};
use crate::web_config::{BindingRedirect, CodeBase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinAssembly {
    /// Relative to the site: `bin\DotNetNuke.dll`.
    pub file: String,
    pub name: String,
    pub version: AssemblyVersion,
    pub file_version: Option<String>,
    pub public_key_token: Option<String>,
    pub modified: SystemTime,
    /// ASP.NET finds it: in bin, the probing path or a codeBase. False in a folder of its own under bin that .NET
    /// doesn't look in (bin\Imageflow without a codeBase, bin\roslyn) - only what uses that folder loads it.
    pub loaded: bool,
}

impl BinAssembly {
    /// The folder it is in, relative to the site: `bin`, `bin\Imageflow`.
    pub fn folder(&self) -> &str {
        self.file.rfind('\\').map_or("", |i| &self.file[..i])
    }
}

/// Something wrong between the site's assemblies - what was found, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyProblem {
    /// "duplicate", "redirect", "codebase", "version" or "missing".
    pub kind: String,
    pub assembly: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssemblyInspection {
    /// By name.
    pub assemblies: Vec<BinAssembly>,
    pub native_files: usize,
    pub problems: Vec<AssemblyProblem>,
    pub newest: Option<BinAssembly>,
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace('/', "\\")
}

fn read(root: &Path, file: &Path, loaded: bool) -> Option<(BinAssembly, Vec<metadata::AssemblyReference>)> {
    let bytes = std::fs::read(file).ok()?;
    let found = metadata::read(&bytes)?;
    let file_version =
        dnnm_infrastructure::dnn_install::file_version(file).map(|(a, b, c, d)| format!("{a}.{b}.{c}.{d}"));
    let assembly = BinAssembly {
        file: relative(root, file),
        name: found.name,
        version: found.version,
        file_version,
        public_key_token: metadata::public_key_token(&found.public_key),
        modified: std::fs::metadata(file).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH),
        loaded,
    };
    Some((assembly, found.references))
}

/// The DLLs in `dir`; with `recurse`, in its folders too - never through a link.
fn dlls(dir: &Path, recurse: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() && !kind.is_symlink() && recurse {
                pending.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
                found.push(path);
            }
        }
    }
    found
}

/// The file a codeBase's href names - relative to the site, or a file: URI; None when it is neither.
fn code_base_path(root: &Path, href: &str) -> Option<PathBuf> {
    let lower = href.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("file:") {
        let path = &href[href.len() - rest.len()..];
        return Some(PathBuf::from(path.trim_start_matches('/').replace('/', "\\")));
    }
    if lower.contains("://") {
        return None;
    }
    std::path::absolute(root.join(href.replace('/', "\\"))).ok()
}

fn in_range(range: &str, version: AssemblyVersion) -> bool {
    let mut parts = range.splitn(2, '-').map(str::trim);
    let Some(low) = parts.next().and_then(AssemblyVersion::parse) else { return false };
    match parts.next() {
        None => version == low,
        Some(high) => AssemblyVersion::parse(high).is_some_and(|high| version >= low && version <= high),
    }
}

/// "8.0.0.0 in bin, 9.0.0.0 in bin\2sxc".
fn versions<'a>(copies: impl Iterator<Item = &'a BinAssembly>) -> String {
    copies.map(|c| format!("{} in {}", c.version, c.folder())).collect::<Vec<_>>().join(", ")
}

/// Where .NET Framework finds what isn't in bin: the GACs, and the framework's own folder.
fn in_framework(name: &str) -> bool {
    let windows = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    let gacs = [
        r"Microsoft.NET\assembly\GAC_MSIL",
        r"Microsoft.NET\assembly\GAC_64",
        r"Microsoft.NET\assembly\GAC_32",
        r"assembly\GAC_MSIL",
        r"assembly\GAC",
    ];
    let frameworks = [r"Microsoft.NET\Framework64\v4.0.30319", r"Microsoft.NET\Framework\v4.0.30319"];
    gacs.iter().any(|g| windows.join(g).join(name).is_dir())
        || frameworks.iter().any(|f| {
            let dir = windows.join(f);
            dir.join(format!("{name}.dll")).is_file() || dir.join("WPF").join(format!("{name}.dll")).is_file()
        })
}

/// Groups in first-seen order, by a case-insensitive key.
fn group_by<'a, T>(items: impl Iterator<Item = &'a T>, key: impl Fn(&T) -> String) -> Vec<(String, Vec<&'a T>)>
where
    T: 'a,
{
    let mut groups: Vec<(String, Vec<&T>)> = Vec::new();
    for item in items {
        let k = key(item);
        match groups.iter_mut().find(|(g, _)| g.eq_ignore_ascii_case(&k)) {
            Some((_, list)) => list.push(item),
            None => groups.push((k, vec![item])),
        }
    }
    groups
}

pub fn inspect(
    root: &Path,
    redirects: &[BindingRedirect],
    code_bases: &[CodeBase],
    probing_path: Option<&str>,
) -> AssemblyInspection {
    let mut folders = vec!["bin".to_string()];
    for extra in probing_path.unwrap_or_default().split(';').map(str::trim).filter(|p| !p.is_empty()) {
        if !folders.iter().any(|f| f.eq_ignore_ascii_case(extra)) {
            folders.push(extra.trim_end_matches(['\\', '/']).to_string());
        }
    }

    let mut assemblies: Vec<BinAssembly> = Vec::new();
    // (index of the assembly referencing, the reference)
    let mut references: Vec<(usize, metadata::AssemblyReference)> = Vec::new();
    let mut native = 0;
    let mut seen = HashSet::new();
    let mut add = |file: &Path, loaded: bool, assemblies: &mut Vec<BinAssembly>| {
        let key = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf()).to_string_lossy().to_lowercase();
        if !seen.insert(key) {
            return;
        }
        match read(root, file, loaded) {
            Some((assembly, refs)) => {
                references.extend(refs.into_iter().map(|r| (assemblies.len(), r)));
                assemblies.push(assembly);
            }
            None => native += 1,
        }
    };

    let mut problems = Vec::new();
    // A codeBase loads that version from its own file - one outside the probing path (bin\2sxc, bin\Imageflow) too.
    let mut code_base_files = HashSet::new();
    for code_base in code_bases {
        match code_base_path(root, &code_base.href).filter(|f| f.is_file()) {
            Some(file) => {
                add(&file, true, &mut assemblies);
                code_base_files.insert(relative(root, &file).to_lowercase());
            }
            None => problems.push(AssemblyProblem {
                kind: "codebase".into(),
                assembly: code_base.name.clone(),
                text: format!(
                    "web.config loads {} {} from {}, which isn't there",
                    code_base.name, code_base.version, code_base.href
                ),
            }),
        }
    }
    for folder in &folders {
        for file in dlls(&root.join(folder), false) {
            add(&file, true, &mut assemblies);
        }
    }
    // The rest of bin: folders of their own - a module's (bin\2sxc, bin\Imageflow), the compiler's (bin\roslyn),
    // native ones (bin\runtimes, bin\x64).
    for file in dlls(&root.join("bin"), true) {
        add(&file, false, &mut assemblies);
    }

    let by_name = group_by(assemblies.iter().filter(|a| a.loaded), |a| a.name.clone());
    let copies_of = |name: &str| by_name.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| c);

    // The same assembly twice: which one loads depends on the probing order - unless web.config's codeBase entries
    // say which version comes from where.
    for (name, copies) in &by_name {
        if copies.iter().filter(|c| !code_base_files.contains(&c.file.to_lowercase())).count() > 1 {
            let list = copies.iter().map(|c| format!("{} ({})", c.file, c.version)).collect::<Vec<_>>().join(", ");
            problems.push(AssemblyProblem {
                kind: "duplicate".into(),
                assembly: name.clone(),
                text: format!("{name} is there {} times: {list}", copies.len()),
            });
        }
    }

    // A redirect to a version bin doesn't have: that assembly can't load.
    for redirect in redirects {
        let (Some(copies), Some(target)) = (copies_of(&redirect.name), AssemblyVersion::parse(&redirect.new_version))
        else {
            continue;
        };
        if copies.iter().any(|c| c.version == target) {
            continue;
        }
        problems.push(AssemblyProblem {
            kind: "redirect".into(),
            assembly: redirect.name.clone(),
            text: format!(
                "web.config redirects {} {} to {}, but the site has {}",
                redirect.name,
                redirect.old_version,
                redirect.new_version,
                versions(copies.iter().copied())
            ),
        });
    }

    // A folder .NET doesn't look in that has the version a reference wants: what web.config lacks, in words.
    let unreachable = |name: &str, version: AssemblyVersion| {
        assemblies
            .iter()
            .find(|a| !a.loaded && a.version == version && a.name.eq_ignore_ascii_case(name))
            .map(|other| format!("{} has {version}, but web.config has no codeBase for it", other.folder()))
    };

    // An assembly in a folder of its own finds what is next to it; and a program's folder (bin\roslyn has csc.exe)
    // loads what its assemblies reference in that program, never in the site.
    let besides: HashSet<(String, String, AssemblyVersion)> = assemblies
        .iter()
        .filter(|a| !a.loaded)
        .map(|a| (a.folder().to_lowercase(), a.name.to_lowercase(), a.version))
        .collect();
    let programs: HashSet<String> = assemblies
        .iter()
        .filter(|a| !a.loaded)
        .map(|a| a.folder().to_lowercase())
        .filter(|f| {
            std::fs::read_dir(root.join(f)).is_ok_and(|entries| {
                entries.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
            })
        })
        .collect();
    references.retain(|(from, r)| {
        let from = &assemblies[*from];
        from.loaded
            || (!besides.contains(&(from.folder().to_lowercase(), r.name.to_lowercase(), r.version))
                && !programs.contains(&from.folder().to_lowercase()))
    });

    // References: to another version of a strong-named assembly with no redirect bridging them, or to one nowhere.
    type Group<'a> = (String, AssemblyVersion, Vec<&'a (usize, metadata::AssemblyReference)>);
    let mut groups: Vec<Group> = Vec::new();
    for reference in &references {
        let (name, version) = (&reference.1.name, reference.1.version);
        match groups.iter_mut().find(|(n, v, _)| n.eq_ignore_ascii_case(name) && *v == version) {
            Some((_, _, list)) => list.push(reference),
            None => groups.push((name.clone(), version, vec![reference])),
        }
    }
    for (name, version, group) in groups {
        let mut from: Vec<&str> = Vec::new();
        for (index, _) in &group {
            let n = assemblies[*index].name.as_str();
            if !from.iter().any(|f| f.eq_ignore_ascii_case(n)) {
                from.push(n);
            }
        }
        let by = from.iter().take(4).copied().collect::<Vec<_>>().join(", ") + if from.len() > 4 { "…" } else { "" };
        match copies_of(&name) {
            Some(copies) => {
                if !group[0].1.strong_named || copies.iter().any(|c| c.version == version) {
                    continue;
                }
                // A redirect covers it: right, or reported above as pointing at a version bin doesn't have.
                if redirects.iter().any(|r| r.name.eq_ignore_ascii_case(&name) && in_range(&r.old_version, version)) {
                    continue;
                }
                let why = unreachable(&name, version).unwrap_or_else(|| "no binding redirect between them".into());
                problems.push(AssemblyProblem {
                    kind: "version".into(),
                    assembly: name.clone(),
                    text: format!(
                        "{by} reference {name} {version}, the site has {} - {why}",
                        versions(copies.iter().copied())
                    ),
                });
            }
            None if !in_framework(&name) => {
                let why =
                    unreachable(&name, version).unwrap_or_else(|| "not in bin, the GAC or the .NET Framework".into());
                problems.push(AssemblyProblem {
                    kind: "missing".into(),
                    assembly: name.clone(),
                    text: format!("{by} reference {name} {version} - {why}"),
                });
            }
            None => {}
        }
    }

    let newest = assemblies.iter().max_by_key(|a| a.modified).cloned();
    assemblies.sort_by_key(|a| a.name.to_lowercase());
    AssemblyInspection { assemblies, native_files: native, problems, newest }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ranges() {
        let v = AssemblyVersion(12, 0, 0, 0);
        assert!(in_range("0.0.0.0-32767.32767.32767.32767", v));
        assert!(in_range("12.0.0.0", v));
        assert!(!in_range("13.0.0.0-13.0.0.0", v));
        assert!(!in_range("x-y", v));
    }

    #[test]
    fn code_base_paths() {
        let root = Path::new(r"C:\DNN\shop");
        assert_eq!(
            code_base_path(root, r"bin\Imageflow\x.dll"),
            Some(PathBuf::from(r"C:\DNN\shop\bin\Imageflow\x.dll"))
        );
        assert_eq!(code_base_path(root, "file:///D:/libs/x.dll"), Some(PathBuf::from(r"D:\libs\x.dll")));
        assert_eq!(code_base_path(root, "http://example.com/x.dll"), None);
    }

    #[test]
    fn a_site_with_framework_copies_finds_duplicates_and_bad_redirects() {
        // Two copies of .NET Framework's System.Xml.dll (an assembly any Windows has) under bin: a duplicate, and a
        // redirect to a version nobody has; a codeBase to nothing. Skipped where the framework isn't installed.
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let source = Path::new(&windows).join(r"Microsoft.NET\Framework64\v4.0.30319\System.Xml.dll");
        if !source.is_file() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(r"bin\Providers")).unwrap();
        std::fs::create_dir_all(root.join(r"bin\Module")).unwrap();
        std::fs::copy(&source, root.join(r"bin\System.Xml.dll")).unwrap();
        std::fs::copy(&source, root.join(r"bin\Providers\System.Xml.dll")).unwrap();
        std::fs::copy(&source, root.join(r"bin\Module\System.Xml.dll")).unwrap();
        std::fs::write(root.join(r"bin\native.dll"), b"MZ not .NET").unwrap();
        let redirects = [BindingRedirect {
            name: "System.Xml".into(),
            old_version: "0.0.0.0-4.0.0.0".into(),
            new_version: "9.0.0.0".into(),
        }];
        let code_bases =
            [CodeBase { name: "Gone".into(), version: "1.0.0.0".into(), href: r"bin\Gone\Gone.dll".into() }];
        let found = inspect(root, &redirects, &code_bases, Some(r"bin\Providers"));
        assert_eq!(found.native_files, 1);
        assert_eq!(found.assemblies.len(), 3);
        let module = found.assemblies.iter().find(|a| a.folder() == r"bin\Module").unwrap();
        assert!(!module.loaded);
        assert_eq!(module.public_key_token.as_deref(), Some("b77a5c561934e089"));
        let kinds: Vec<&str> = found.problems.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds, ["codebase", "duplicate", "redirect"]);
        assert_eq!(found.problems[0].text, r"web.config loads Gone 1.0.0.0 from bin\Gone\Gone.dll, which isn't there");
        assert!(found.problems[1].text.starts_with(r"System.Xml is there 2 times: bin\System.Xml.dll (4.0.0.0)"));
        assert_eq!(
            found.problems[2].text,
            r"web.config redirects System.Xml 0.0.0.0-4.0.0.0 to 9.0.0.0, but the site has 4.0.0.0 in bin, 4.0.0.0 in bin\Providers"
        );
        assert!(found.newest.is_some());
    }
}
