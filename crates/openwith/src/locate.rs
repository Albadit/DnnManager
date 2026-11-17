//! Ports `Presentation/Services/IdeLocator.cs`: the code editors / IDEs installed on this PC - Visual Studio (through
//! vswhere), VS Code and its forks, JetBrains Rider and IntelliJ IDEA, Sublime Text, Zed, Vim and Neovim, from their
//! default install folders or PATH - and SQL Server Management Studio, for a project's database. Only what's found is
//! offered.
//!
//! The search itself works on a [`Locations`] (the special folders and PATH) and a file check, so it is tested on a
//! folder tree of its own; [`installed`] and [`management_studios`] run it once per run on this PC.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// An editor, IDE or SSMS found on this PC (`Ide`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ide {
    pub name: String,
    pub exe_path: PathBuf,
    /// Opens a project's `.sln` rather than its folder when there is exactly one.
    pub opens_solution: bool,
    /// The product's major version (e.g. 22 for SSMS 22), or 0 when unknown.
    pub major_version: u32,
}

impl Ide {
    fn new(name: &str, exe_path: PathBuf, opens_solution: bool, major_version: u32) -> Self {
        Self { name: name.to_string(), exe_path, opens_solution, major_version }
    }
}

/// Where the search looks: the special folders (`Environment.SpecialFolder`) and DNN Manager's PATH.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    pub local_app_data: PathBuf,
    pub program_files: PathBuf,
    pub program_files_x86: PathBuf,
    /// The PATH environment variable, as `Environment.GetEnvironmentVariable("PATH")` reads it.
    pub path: String,
}

impl Locations {
    /// This PC's.
    pub fn of_this_pc() -> Self {
        use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86};
        Self {
            local_app_data: crate::win::known_folder(&FOLDERID_LocalAppData).unwrap_or_default(),
            program_files: crate::win::known_folder(&FOLDERID_ProgramFiles).unwrap_or_default(),
            program_files_x86: crate::win::known_folder(&FOLDERID_ProgramFilesX86).unwrap_or_default(),
            path: std::env::var("PATH").unwrap_or_default(),
        }
    }

    // Both Program Files folders that are set, once each.
    fn program_files_roots(&self, x86_first: bool) -> Vec<PathBuf> {
        let ordered = if x86_first {
            [&self.program_files_x86, &self.program_files]
        } else {
            [&self.program_files, &self.program_files_x86]
        };
        let mut roots: Vec<PathBuf> = Vec::new();
        for root in ordered {
            if !root.as_os_str().is_empty() && !roots.iter().any(|r| same_path(r, root)) {
                roots.push(root.clone());
            }
        }
        roots
    }
}

const SSMS_PRODUCT: &str = "Microsoft.VisualStudio.Product.Ssms";

/// The installed IDEs - looked up once per run (vswhere takes a moment); the first caller waits for it, the rest share
/// the result. Call it off the UI thread.
pub fn installed() -> &'static [Ide] {
    static INSTALLED: OnceLock<Vec<Ide>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let locations = Locations::of_this_pc();
        let visual_studios = vswhere(&locations, None, "Visual Studio", true);
        find_ides(&locations, visual_studios, &|p| p.is_file())
    })
}

/// The installed SQL Server Management Studio versions, newest first - looked up once per run, like [`installed`].
pub fn management_studios() -> &'static [Ide] {
    static STUDIOS: OnceLock<Vec<Ide>> = OnceLock::new();
    STUDIOS.get_or_init(|| {
        let locations = Locations::of_this_pc();
        let from_vswhere = vswhere(&locations, Some(SSMS_PRODUCT), "SQL Server Management Studio", false);
        find_management_studios(&locations, from_vswhere, &|p| p.is_file())
    })
}

/// Why DNN Manager doesn't start `ssms` (it runs as Administrator: only a copy nobody but administrators can change) -
/// for its menu entry, shown greyed; None when it may. Asked once per copy.
pub fn refusal(ssms: &Ide) -> Option<String> {
    static REFUSALS: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let key = ssms.exe_path.display().to_string().to_lowercase();
    let cache = REFUSALS.get_or_init(Default::default);
    if let Some(known) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return known.clone();
    }
    let resolved = dnnm_system::trusted::resolve(&ssms.exe_path.display().to_string());
    let refusal = resolved.path.is_none().then(|| resolved.problem(&ssms.name));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, refusal.clone());
    refusal
}

/// The project's solution file when its folder holds exactly one (`.sln` or `.slnx`), otherwise None.
pub fn solution_for(project_directory: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(project_directory).ok()?;
    let solutions: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("sln") || e.eq_ignore_ascii_case("slnx"))
        })
        .take(2)
        .collect();
    if solutions.len() == 1 { solutions.into_iter().next() } else { None }
}

/// The editors and IDEs under `locations`, in the menu's order: the Visual Studios vswhere found first.
pub fn find_ides(locations: &Locations, visual_studios: Vec<Ide>, exists: &dyn Fn(&Path) -> bool) -> Vec<Ide> {
    let mut found = visual_studios;
    let local = &locations.local_app_data;
    let program_files = &locations.program_files;
    let programs = local.join("Programs");
    let mut add_first = |name: &str, opens_solution: bool, candidates: Vec<Option<PathBuf>>| {
        if let Some(exe) = candidates.into_iter().flatten().find(|c| exists(c)) {
            found.push(Ide::new(name, exe, opens_solution, 0));
        }
    };

    add_first(
        "Visual Studio Code",
        false,
        vec![
            Some(programs.join("Microsoft VS Code").join("Code.exe")),
            Some(program_files.join("Microsoft VS Code").join("Code.exe")),
            from_path(&locations.path, "code.cmd", "Code.exe", exists),
        ],
    );
    add_first(
        "VS Code Insiders",
        false,
        vec![
            Some(programs.join("Microsoft VS Code Insiders").join("Code - Insiders.exe")),
            Some(program_files.join("Microsoft VS Code Insiders").join("Code - Insiders.exe")),
        ],
    );
    add_first("Cursor", false, vec![Some(programs.join("cursor").join("Cursor.exe"))]);
    add_first("Windsurf", false, vec![Some(programs.join("Windsurf").join("Windsurf.exe"))]);
    add_first("Rider", true, some(jet_brains_candidates(locations, "Rider", "JetBrains Rider*", "rider64.exe")));
    let mut idea = jet_brains_candidates(locations, "IntelliJ IDEA Ultimate", "IntelliJ IDEA*", "idea64.exe");
    idea.extend(jet_brains_candidates(locations, "IntelliJ IDEA Community Edition", "IntelliJ IDEA*", "idea64.exe"));
    add_first("IntelliJ IDEA", false, some(idea));
    add_first(
        "Sublime Text",
        false,
        vec![
            Some(program_files.join("Sublime Text").join("sublime_text.exe")),
            Some(program_files.join("Sublime Text 3").join("sublime_text.exe")),
            on_path(&locations.path, "sublime_text.exe", exists),
        ],
    );
    add_first(
        "Zed",
        false,
        vec![
            Some(programs.join("Zed").join("Zed.exe")),
            Some(program_files.join("Zed").join("Zed.exe")),
            on_path(&locations.path, "zed.exe", exists),
        ],
    );
    // Vim and Neovim: the windowed editor when it's there; otherwise the console one, which opens in a window of its own
    // (DNN Manager has no console) on the project folder.
    let mut vim = some(versioned_candidates(locations, "Vim", "vim*", "gvim.exe"));
    vim.push(on_path(&locations.path, "gvim.exe", exists));
    vim.extend(some(versioned_candidates(locations, "Vim", "vim*", "vim.exe")));
    vim.push(on_path(&locations.path, "vim.exe", exists));
    add_first("Vim", false, vim);
    let neovim = program_files.join("Neovim").join("bin");
    add_first(
        "Neovim",
        false,
        vec![
            Some(neovim.join("nvim-qt.exe")),
            on_path(&locations.path, "nvim-qt.exe", exists),
            Some(neovim.join("nvim.exe")),
            on_path(&locations.path, "nvim.exe", exists),
        ],
    );
    found
}

fn some(paths: Vec<PathBuf>) -> Vec<Option<PathBuf>> {
    paths.into_iter().map(Some).collect()
}

/// SSMS 21 and later are Visual Studio Installer products, so vswhere lists them with their real name (`from_vswhere`);
/// SSMS 18-20 are found by their default install folders. Newest first.
pub fn find_management_studios(
    locations: &Locations,
    from_vswhere: Vec<Ide>,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Ide> {
    let mut found = from_vswhere;
    for root in locations.program_files_roots(true).into_iter().filter(|r| r.is_dir()) {
        for dir in directories(&root, "Microsoft SQL Server Management Studio *") {
            let candidates = [
                dir.join("Common7").join("IDE").join("Ssms.exe"),
                dir.join("Release").join("Common7").join("IDE").join("SSMS.exe"),
            ];
            let Some(exe) = candidates.into_iter().find(|c| exists(c)) else { continue };
            if found.iter().any(|f| same_path(&f.exe_path, &exe)) {
                continue;
            }
            let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = folder.get("Microsoft ".len()..).unwrap_or_default().to_string();
            let major = name.split(' ').next_back().and_then(|m| m.parse().ok()).unwrap_or(0);
            found.push(Ide::new(&name, exe, false, major));
        }
    }
    // Stable, as LINQ's OrderByDescending: vswhere's own order stays among equal versions.
    found.sort_by_key(|f| std::cmp::Reverse(f.major_version));
    found
}

/// Every install in vswhere's JSON output, newest first as vswhere sorts them - those whose `productPath` exists.
/// Unreadable output is none.
pub fn parse_vswhere(
    json: &str,
    fallback_name: &str,
    opens_solution: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Ide> {
    let Ok(serde_json::Value::Array(installs)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    installs
        .iter()
        .filter_map(|vs| {
            let exe = PathBuf::from(vs.get("productPath")?.as_str()?);
            if !exists(&exe) {
                return None;
            }
            let name = vs.get("displayName").and_then(|d| d.as_str()).unwrap_or(fallback_name);
            let major = vs
                .get("installationVersion")
                .and_then(|v| v.as_str())
                .and_then(|v| v.split('.').next()?.parse().ok())
                .unwrap_or(0);
            Some(Ide::new(name, exe, opens_solution, major))
        })
        .collect()
}

/// The arguments vswhere is asked with: every install, prereleases too, newest first, as JSON - Visual Studio itself
/// when `products` is None (vswhere's default: Community, Professional, Enterprise), otherwise that product.
pub fn vswhere_arguments(products: Option<&str>) -> Vec<&str> {
    let mut args = vec!["-all", "-prerelease", "-sort", "-format", "json", "-utf8"];
    if let Some(products) = products {
        args.extend(["-products", products]);
    }
    args
}

// vswhere runs as Administrator, so only a copy only administrators can change, with the safe environment - and never
// for longer than a few seconds (`ElevatedStart.Output`).
fn vswhere(locations: &Locations, products: Option<&str>, fallback_name: &str, opens_solution: bool) -> Vec<Ide> {
    let vswhere = locations.program_files_x86.join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
    if !vswhere.is_file() {
        return Vec::new();
    }
    let program = vswhere.display().to_string();
    let Ok(result) =
        dnnm_sql::programs::Run::new(&program, &vswhere_arguments(products)).timeout(Duration::from_secs(10)).run()
    else {
        return Vec::new();
    };
    parse_vswhere(&result.stdout, fallback_name, opens_solution, &|p| p.is_file())
}

/// The full path of `exe_name` in a folder on PATH, or None. Git for Windows' own tools (its vim, used for commit
/// messages) don't count - that's not an editor anyone installed.
pub fn on_path(path: &str, exe_name: &str, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    path.split(';').filter(|d| !d.is_empty()).find_map(|dir| {
        let exe = Path::new(dir.trim()).join(exe_name);
        let text = exe.display().to_string().to_lowercase();
        if [r"\git\usr\", r"\git\bin\", r"\git\mingw64\"].iter().any(|git| text.contains(git)) {
            return None;
        }
        exists(&exe).then_some(exe)
    })
}

/// The editor exe next to a launcher script on PATH, e.g. `…\Microsoft VS Code\bin\code.cmd` ->
/// `…\Microsoft VS Code\Code.exe` - catches installs outside the default folders.
pub fn from_path(path: &str, launcher: &str, exe_name: &str, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    path.split(';').filter(|d| !d.is_empty()).find_map(|dir| {
        if !exists(&Path::new(dir).join(launcher)) {
            return None;
        }
        let parent = Path::new(dir.trim_end_matches(['\\', '/'])).parent().unwrap_or(Path::new(""));
        let exe = parent.join(exe_name);
        exists(&exe).then_some(exe)
    })
}

// JetBrains Toolbox installs in the user's Programs folder; the standalone installer uses a versioned folder.
fn jet_brains_candidates(
    locations: &Locations,
    toolbox_folder: &str,
    standalone_pattern: &str,
    exe: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![locations.local_app_data.join("Programs").join(toolbox_folder).join("bin").join(exe)];
    let jet_brains = locations.program_files.join("JetBrains");
    candidates.extend(directories(&jet_brains, standalone_pattern).into_iter().map(|d| d.join("bin").join(exe)));
    candidates
}

/// `exe_name` in the versioned folders under `Program Files\<product>` (both Program Files), newest first - e.g.
/// `Vim\vim91\gvim.exe`.
fn versioned_candidates(locations: &Locations, product: &str, version_pattern: &str, exe_name: &str) -> Vec<PathBuf> {
    locations
        .program_files_roots(false)
        .into_iter()
        .flat_map(|root| directories(&root.join(product), version_pattern))
        .map(|d| d.join(exe_name))
        .collect()
}

/// The folders in `parent` whose name matches `pattern` (`*` for any text, case-insensitive), last name first - as
/// `Directory.EnumerateDirectories(…).OrderByDescending(d => d)`.
fn directories(parent: &Path, pattern: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir() && wildcard_match(pattern, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    found.sort_by_key(|d| std::cmp::Reverse(d.display().to_string().to_lowercase()));
    found
}

/// Whether `name` matches `pattern`, `*` standing for any text - case-insensitive, as Windows compares file names.
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut p, mut n) = (0, 0);
    let (mut star, mut star_n) = (None, 0);
    while n < name.len() {
        if p < pattern.len() && pattern[p] != '*' && pattern[p] == name[n] {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            star_n = n;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            star_n += 1;
            n = star_n;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.display().to_string().eq_ignore_ascii_case(&b.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"MZ").unwrap();
    }

    fn tree() -> (tempfile::TempDir, Locations) {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            local_app_data: root.path().join("Local"),
            program_files: root.path().join("Program Files"),
            program_files_x86: root.path().join("Program Files (x86)"),
            path: String::new(),
        };
        (root, locations)
    }

    fn names(ides: &[Ide]) -> Vec<&str> {
        ides.iter().map(|i| i.name.as_str()).collect()
    }

    fn exists(p: &Path) -> bool {
        p.is_file()
    }

    #[test]
    fn nothing_installed_finds_nothing() {
        let (_root, locations) = tree();
        assert!(find_ides(&locations, Vec::new(), &exists).is_empty());
        assert!(find_management_studios(&locations, Vec::new(), &exists).is_empty());
    }

    #[test]
    fn every_editor_is_found_in_its_default_folder_in_the_menus_order() {
        let (_root, mut locations) = tree();
        let local = locations.local_app_data.join("Programs");
        let pf = locations.program_files.clone();
        touch(&local.join(r"Microsoft VS Code\Code.exe"));
        touch(&pf.join(r"Microsoft VS Code Insiders\Code - Insiders.exe"));
        touch(&local.join(r"cursor\Cursor.exe"));
        touch(&local.join(r"Windsurf\Windsurf.exe"));
        touch(&pf.join(r"JetBrains\JetBrains Rider 2024.3\bin\rider64.exe"));
        touch(&local.join(r"IntelliJ IDEA Community Edition\bin\idea64.exe"));
        touch(&pf.join(r"Sublime Text 3\sublime_text.exe"));
        touch(&pf.join(r"Zed\Zed.exe"));
        touch(&locations.program_files_x86.join(r"Vim\vim91\vim.exe"));
        touch(&pf.join(r"Neovim\bin\nvim.exe"));
        locations.path = String::new();
        let vs = vec![Ide::new("Visual Studio Community 2022", pf.join("devenv.exe"), true, 17)];

        let found = find_ides(&locations, vs, &exists);
        assert_eq!(
            names(&found),
            [
                "Visual Studio Community 2022",
                "Visual Studio Code",
                "VS Code Insiders",
                "Cursor",
                "Windsurf",
                "Rider",
                "IntelliJ IDEA",
                "Sublime Text",
                "Zed",
                "Vim",
                "Neovim"
            ]
        );
        assert!(found[5].opens_solution, "Rider opens the solution");
        assert!(!found[1].opens_solution);
        assert!(found[5].exe_path.ends_with(r"JetBrains Rider 2024.3\bin\rider64.exe"));
        assert!(found[9].exe_path.ends_with(r"vim91\vim.exe"));
    }

    #[test]
    fn the_user_install_of_vs_code_comes_before_the_machine_one() {
        let (_root, locations) = tree();
        touch(&locations.local_app_data.join(r"Programs\Microsoft VS Code\Code.exe"));
        touch(&locations.program_files.join(r"Microsoft VS Code\Code.exe"));
        let found = find_ides(&locations, Vec::new(), &exists);
        assert_eq!(found.len(), 1);
        assert!(found[0].exe_path.starts_with(&locations.local_app_data));
    }

    #[test]
    fn vs_code_elsewhere_is_found_through_its_launcher_on_path() {
        let (root, mut locations) = tree();
        let install = root.path().join(r"Tools\VSCode");
        touch(&install.join(r"bin\code.cmd"));
        touch(&install.join("Code.exe"));
        locations.path = format!(r"C:\nowhere;{}\", install.join("bin").display());
        let found = find_ides(&locations, Vec::new(), &exists);
        assert_eq!(names(&found), ["Visual Studio Code"]);
        assert_eq!(found[0].exe_path, install.join("Code.exe"));
    }

    #[test]
    fn gits_own_vim_on_path_is_not_an_editor() {
        let (root, mut locations) = tree();
        let git = root.path().join(r"Git\usr\bin");
        touch(&git.join("vim.exe"));
        locations.path = git.display().to_string();
        assert!(find_ides(&locations, Vec::new(), &exists).is_empty());

        let own = root.path().join(r"Editors");
        touch(&own.join("vim.exe"));
        locations.path = format!("{};{}", git.display(), own.display());
        let found = find_ides(&locations, Vec::new(), &exists);
        assert_eq!(found[0].exe_path, own.join("vim.exe"));
    }

    #[test]
    fn the_windowed_vim_wins_and_the_newest_version_first() {
        let (_root, locations) = tree();
        touch(&locations.program_files.join(r"Vim\vim90\gvim.exe"));
        touch(&locations.program_files.join(r"Vim\vim91\gvim.exe"));
        touch(&locations.program_files.join(r"Vim\vim91\vim.exe"));
        touch(&locations.program_files.join(r"Neovim\bin\nvim-qt.exe"));
        touch(&locations.program_files.join(r"Neovim\bin\nvim.exe"));
        let found = find_ides(&locations, Vec::new(), &exists);
        assert!(found[0].exe_path.ends_with(r"vim91\gvim.exe"));
        assert!(found[1].exe_path.ends_with("nvim-qt.exe"));
    }

    #[test]
    fn intellij_ultimate_comes_before_community() {
        let (_root, locations) = tree();
        touch(&locations.local_app_data.join(r"Programs\IntelliJ IDEA Ultimate\bin\idea64.exe"));
        touch(&locations.local_app_data.join(r"Programs\IntelliJ IDEA Community Edition\bin\idea64.exe"));
        let found = find_ides(&locations, Vec::new(), &exists);
        assert!(found[0].exe_path.to_string_lossy().contains("Ultimate"));
    }

    // vswhere 3.1's output for one Visual Studio 2022 and a preview, trimmed to what is read.
    const VSWHERE: &str = r#"[
  {
    "instanceId": "a1b2c3d4",
    "installationPath": "C:\\Program Files\\Microsoft Visual Studio\\2022\\Preview",
    "installationVersion": "17.14.36015.10",
    "productPath": "C:\\Program Files\\Microsoft Visual Studio\\2022\\Preview\\Common7\\IDE\\devenv.exe",
    "isPrerelease": true,
    "displayName": "Visual Studio Community 2022 Preview",
    "catalog": { "productDisplayVersion": "17.14.0 Preview 7.0" }
  },
  {
    "instanceId": "e5f6a7b8",
    "installationVersion": "17.12.35527.113",
    "productPath": "C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\Common7\\IDE\\devenv.exe",
    "displayName": "Visual Studio Community 2022"
  },
  {
    "instanceId": "gone",
    "installationVersion": "16.11.1.0",
    "productPath": "C:\\Program Files (x86)\\Microsoft Visual Studio\\2019\\Community\\Common7\\IDE\\devenv.exe"
  },
  { "instanceId": "no-product" }
]"#;

    #[test]
    fn vswhere_output_gives_each_install_whose_exe_exists() {
        let exists = |p: &Path| !p.to_string_lossy().contains("2019");
        let found = parse_vswhere(VSWHERE, "Visual Studio", true, &exists);
        assert_eq!(names(&found), ["Visual Studio Community 2022 Preview", "Visual Studio Community 2022"]);
        assert_eq!(found[0].major_version, 17);
        assert!(found[0].opens_solution);

        let all = parse_vswhere(VSWHERE, "Visual Studio", true, &|_| true);
        assert_eq!(all[2].name, "Visual Studio", "no displayName: the fallback name");
        assert_eq!(all[2].major_version, 16);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn vswhere_output_that_is_not_a_list_is_nothing() {
        assert!(parse_vswhere("", "Visual Studio", true, &|_| true).is_empty());
        assert!(parse_vswhere("Error 87: unknown switch", "Visual Studio", true, &|_| true).is_empty());
        assert!(parse_vswhere("{}", "Visual Studio", true, &|_| true).is_empty());
    }

    #[test]
    fn vswhere_is_asked_for_ssms_by_its_product() {
        assert_eq!(vswhere_arguments(None), ["-all", "-prerelease", "-sort", "-format", "json", "-utf8"]);
        assert_eq!(vswhere_arguments(Some(SSMS_PRODUCT))[6..], ["-products", "Microsoft.VisualStudio.Product.Ssms"]);
    }

    #[test]
    fn older_ssms_is_found_by_its_folder_and_sorted_newest_first() {
        let (_root, locations) = tree();
        touch(&locations.program_files_x86.join(r"Microsoft SQL Server Management Studio 19\Common7\IDE\Ssms.exe"));
        touch(&locations.program_files_x86.join(r"Microsoft SQL Server Management Studio 20\Common7\IDE\Ssms.exe"));
        touch(&locations.program_files.join(r"Microsoft SQL Server Management Studio 21\Release\Common7\IDE\SSMS.exe"));
        // No exe inside: not an install.
        std::fs::create_dir_all(locations.program_files.join("Microsoft SQL Server Management Studio 18")).unwrap();
        let ssms22 = Ide::new("SQL Server Management Studio 22", PathBuf::from(r"C:\ssms22\SSMS.exe"), false, 22);

        let found = find_management_studios(&locations, vec![ssms22], &exists);
        assert_eq!(
            names(&found),
            [
                "SQL Server Management Studio 22",
                "SQL Server Management Studio 21",
                "SQL Server Management Studio 20",
                "SQL Server Management Studio 19"
            ]
        );
        assert_eq!(found[1].major_version, 21);
    }

    #[test]
    fn an_ssms_vswhere_listed_is_not_listed_twice() {
        let (_root, locations) = tree();
        let exe =
            locations.program_files.join(r"Microsoft SQL Server Management Studio 21\Release\Common7\IDE\SSMS.exe");
        touch(&exe);
        let listed = Ide::new("SQL Server Management Studio 21", exe, false, 21);
        let found = find_management_studios(&locations, vec![listed], &exists);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn a_single_solution_is_opened_two_are_not() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(solution_for(dir.path()), None);
        touch(&dir.path().join("Site.sln"));
        assert_eq!(solution_for(dir.path()), Some(dir.path().join("Site.sln")));
        touch(&dir.path().join("Other.slnx"));
        assert_eq!(solution_for(dir.path()), None);
        assert_eq!(solution_for(&dir.path().join("missing")), None);
    }

    #[test]
    fn wildcards_match_as_windows_does() {
        assert!(wildcard_match("vim*", "vim91"));
        assert!(wildcard_match("vim*", "VIM"));
        assert!(!wildcard_match("vim*", "gvim"));
        assert!(wildcard_match("JetBrains Rider*", "JetBrains Rider 2024.3"));
        assert!(wildcard_match(
            "Microsoft SQL Server Management Studio *",
            "Microsoft SQL Server Management Studio 20"
        ));
        assert!(!wildcard_match("Microsoft SQL Server Management Studio *", "Microsoft SQL Server Management Studio"));
        assert!(wildcard_match("a*b*c", "aXXbYc"));
    }
}
