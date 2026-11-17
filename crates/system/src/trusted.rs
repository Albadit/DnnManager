//! Which programs DNN Manager - running as Administrator - may start with its rights: only one that nobody but
//! administrators can change (`Processes/TrustedPrograms.cs`), looked up on the computer's PATH, never the user's
//! (`Processes/ChildEnvironment.cs`: `MachinePath`, `MachinePathExt`). Not elevated (the tests), there is no such
//! boundary, and any program goes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesCommon, FOLDERID_ProgramFilesCommonX86,
    FOLDERID_ProgramFilesX86, FOLDERID_System, FOLDERID_Windows,
};

use crate::acl::{Security, rights, sids};
use crate::win::{known_folder, registry_string};

/// DNN Manager runs with administrator rights - what it starts runs with them too.
pub fn is_elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(dnnm_infrastructure::windows_security::is_elevated)
}

/// Whether only administrators can change a program - or that couldn't be found out (a read failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    AdminOnly,
    OthersCanChange,
    Unknown,
}

/// A program DNN Manager may start with its rights: `path`, the file's own path with every link, junction and short
/// name resolved - or None, with `refused` naming a copy that was found but not trusted and `reason` saying why.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolved {
    pub path: Option<PathBuf>,
    pub refused: Option<String>,
    pub reason: Option<String>,
}

impl Resolved {
    /// Why it isn't started, as a sentence for the user - also when it isn't there at all.
    pub fn problem(&self, name: &str) -> String {
        match &self.refused {
            None => format!("{name} isn't installed (or not on the computer's PATH)."),
            Some(refused) => format!(
                "DNN Manager doesn't start {refused} as Administrator: {} Install it for all users (in Program Files) - DNN Manager uses that copy.",
                self.reason.as_deref().unwrap_or_default()
            ),
        }
    }

    fn refused(file: impl Into<String>, reason: &str) -> Self {
        Self { path: None, refused: Some(file.into()), reason: Some(reason.into()) }
    }
}

// Who may change a program DNN Manager runs elevated: SYSTEM, Administrators, TrustedInstaller - and "the owner of what
// is created here" (CREATOR OWNER), whose rights go to the owner, which is checked on its own.
const TRUSTED_SIDS: [&str; 4] = [sids::SYSTEM, sids::ADMINISTRATORS, sids::CREATOR_OWNER, sids::TRUSTED_INSTALLER];

// Changing a file, or putting another in its place (or a DLL beside it).
const CHANGES: u32 = rights::WRITE_DATA
    | rights::APPEND_DATA
    | rights::DELETE
    | rights::DELETE_SUBDIRECTORIES_AND_FILES
    | rights::CHANGE_PERMISSIONS
    | rights::TAKE_OWNERSHIP
    | rights::WRITE_ATTRIBUTES
    | rights::WRITE_EXTENDED_ATTRIBUTES;

/// The program `name` (`docker`, `winget`, or a full path) as it may be started with DNN Manager's rights: a full path
/// is checked as it is, a bare name looked up on the computer's PATH - its first copy that only administrators can
/// change. Not elevated, there is no boundary: the name itself.
pub fn resolve(name: &str) -> Resolved {
    if !is_elevated() {
        return Resolved { path: Some(PathBuf::from(name)), ..Default::default() };
    }
    if Path::new(name).is_absolute() {
        return check(Path::new(name));
    }
    if name.eq_ignore_ascii_case("winget") || name.eq_ignore_ascii_case("winget.exe") {
        return winget();
    }
    let mut refused = None;
    for candidate in on_machine_path(name) {
        let checked = check(&candidate);
        if checked.path.is_some() {
            return checked;
        }
        refused.get_or_insert(checked);
    }
    // Only on the user's own PATH (installed for this account): there, but never started - said as such.
    refused.unwrap_or_else(|| match on_user_path(name) {
        Some(own) => Resolved::refused(
            own.display().to_string(),
            "it is installed for your account only - programs without administrator rights could change it, and DNN Manager would run it as Administrator.",
        ),
        None => Resolved::default(),
    })
}

fn extensions(name: &str) -> Vec<String> {
    if Path::new(name).extension().is_some() {
        vec![String::new()]
    } else {
        machine_path_ext().split(';').filter(|e| !e.is_empty()).map(str::to_string).collect()
    }
}

/// Where `name` is on the computer's PATH (with PATHEXT's extensions when it has none) - every copy, in PATH's order.
pub fn on_machine_path(name: &str) -> Vec<PathBuf> {
    let exts = extensions(name);
    let mut found = Vec::new();
    for folder in machine_path().split(';').map(str::trim).filter(|f| !f.is_empty()) {
        for ext in &exts {
            let candidate = Path::new(folder).join(format!("{name}{ext}"));
            if candidate.is_absolute() && candidate.is_file() {
                found.push(candidate);
            }
        }
    }
    found
}

// The first copy of `name` on the user's own PATH - only to name it in a message; never started.
fn on_user_path(name: &str) -> Option<PathBuf> {
    let path = registry_string(HKEY_CURRENT_USER, "Environment", "Path")?;
    let exts = extensions(name);
    path.split(';').map(str::trim).filter(|f| !f.is_empty()).find_map(|folder| {
        let folder = expand_user(folder);
        exts.iter().map(|ext| Path::new(&folder).join(format!("{name}{ext}"))).find(|c| c.is_absolute() && c.is_file())
    })
}

// Only to name a file in a message: the user's own variables are fine there.
fn expand_user(text: &str) -> String {
    let mut out = text.to_string();
    for (name, value) in std::env::vars() {
        let pattern = format!("%{name}%");
        if let Some(at) = out.to_ascii_lowercase().find(&pattern.to_ascii_lowercase()) {
            out.replace_range(at..at + pattern.len(), &value);
        }
    }
    out
}

const MACHINE_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

/// The computer's PATH, from the registry: its %variables% filled in with the folders of Windows and Program Files and
/// the computer's own variables - never the user's. A folder with a variable it can't fill in so is left out.
pub fn machine_path() -> String {
    let path = registry_string(HKEY_LOCAL_MACHINE, MACHINE_KEY, "Path").unwrap_or_default();
    let folders = system_folders();
    path.split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .filter_map(|e| expand(e, 0, &folders, &machine_value))
        .collect::<Vec<_>>()
        .join(";")
}

/// The computer's PATHEXT.
pub fn machine_path_ext() -> String {
    registry_string(HKEY_LOCAL_MACHINE, MACHINE_KEY, "PATHEXT")
        .filter(|e| !e.is_empty())
        .and_then(|e| expand(&e, 0, &system_folders(), &machine_value))
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string())
}

fn machine_value(name: &str) -> Option<String> {
    registry_string(HKEY_LOCAL_MACHINE, MACHINE_KEY, name)
}

/// The folders of Windows, Program Files and ProgramData by their variable names (`ChildEnvironment.SystemFolders`),
/// names in lower case.
pub fn system_folders() -> HashMap<String, String> {
    let folder = |id| known_folder(id).map(|p| p.display().to_string()).unwrap_or_default();
    let windows = folder(&FOLDERID_Windows);
    let program_files = folder(&FOLDERID_ProgramFiles);
    let common = folder(&FOLDERID_ProgramFilesCommon);
    let program_data = folder(&FOLDERID_ProgramData);
    let drive = windows.get(..2).unwrap_or_default().to_string();
    let system = folder(&FOLDERID_System);
    [
        ("SystemRoot", windows.clone()),
        ("windir", windows),
        ("SystemDrive", drive),
        ("ComSpec", format!(r"{system}\cmd.exe")),
        ("ProgramFiles", program_files.clone()),
        ("ProgramW6432", program_files),
        ("ProgramFiles(x86)", folder(&FOLDERID_ProgramFilesX86)),
        ("CommonProgramFiles", common.clone()),
        ("CommonProgramW6432", common),
        ("CommonProgramFiles(x86)", folder(&FOLDERID_ProgramFilesCommonX86)),
        ("ProgramData", program_data.clone()),
        ("ALLUSERSPROFILE", program_data),
    ]
    .into_iter()
    .filter(|(_, f)| !f.is_empty())
    .map(|(n, f)| (n.to_ascii_lowercase(), f))
    .collect()
}

/// `value` with its `%variables%` filled in from `folders` or the computer's own (`machine`, three deep) - None when
/// one can't be filled in so.
pub fn expand(
    value: &str,
    depth: u32,
    folders: &HashMap<String, String>,
    machine: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 && !after[..end].contains(';') => {
                out.push_str(&rest[..start]);
                let name = &after[..end];
                if let Some(folder) = folders.get(&name.to_ascii_lowercase()) {
                    out.push_str(folder);
                } else if depth < 3
                    && let Some(inner) = machine(name).and_then(|m| expand(&m, depth + 1, folders, machine))
                {
                    out.push_str(&inner);
                } else {
                    return None;
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Some(out)
}

/// `file` checked: its own path when only administrators can change it.
fn check(file: &Path) -> Resolved {
    let display = file.display().to_string();
    let Some(final_path) = final_path(file) else {
        return Resolved::refused(display, "it can't be opened to check it.");
    };
    match admin_only(&final_path) {
        Trust::AdminOnly => Resolved { path: Some(final_path), ..Default::default() },
        Trust::Unknown => Resolved::refused(display, "who may change it couldn't be read - try again in a moment."),
        Trust::OthersCanChange => Resolved::refused(
            display,
            "programs without administrator rights could change it, and DNN Manager would run it as Administrator.",
        ),
    }
}

/// The path Windows opens for `file` - every link, junction and short name on the way resolved - or None when it
/// can't be opened (or is on a share).
pub fn final_path(file: &Path) -> Option<PathBuf> {
    let path = std::fs::canonicalize(file).ok()?;
    let text = path.to_str()?;
    if text.starts_with(r"\\?\UNC\") {
        return None;
    }
    Some(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text)))
}

/// Whether only administrators (and Windows) can change `file` - false also when that couldn't be read.
pub fn is_admin_only(file: &Path) -> bool {
    admin_only(file) == Trust::AdminOnly
}

/// Whether only administrators (and Windows) can change `file`: the file itself, its folder, and every folder above -
/// none a link or junction, none that others may delete, rename or take over. [`Trust::Unknown`] when a read failed for
/// a reason that may pass (a file in use).
pub fn admin_only(file: &Path) -> Trust {
    match admin_only_checked(file) {
        Ok(trust) => trust,
        // Not even an administrator may read who can change it: not one to trust.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Trust::OthersCanChange,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Trust::OthersCanChange,
        // In use, a network hiccup: unknown, not "others can change it".
        Err(_) => Trust::Unknown,
    }
}

fn admin_only_checked(file: &Path) -> std::io::Result<Trust> {
    let meta = std::fs::symlink_metadata(file)?;
    if !meta.is_file() || is_reparse_point(&meta) {
        return Ok(Trust::OthersCanChange);
    }
    if !rules_admin_only(&Security::read(file)?, CHANGES, true, false) {
        return Ok(Trust::OthersCanChange);
    }
    let Some(folder) = file.parent() else { return Ok(Trust::OthersCanChange) };
    if is_reparse_point(&std::fs::symlink_metadata(folder)?) {
        return Ok(Trust::OthersCanChange);
    }
    let mut above_rules = Security::read(folder)?;
    if !rules_admin_only(&above_rules, CHANGES, true, false) {
        return Ok(Trust::OthersCanChange);
    }
    // The folders above may let others make folders (C:\ does) - not delete, rename or take over one on the way.
    let mut above = folder;
    while let Some(parent) = above.parent() {
        if parent.parent().is_some() && is_reparse_point(&std::fs::symlink_metadata(parent)?) {
            return Ok(Trust::OthersCanChange);
        }
        let parent_rules = Security::read(parent)?;
        if !rules_admin_only(
            &above_rules,
            rights::DELETE | rights::CHANGE_PERMISSIONS | rights::TAKE_OWNERSHIP,
            false,
            true,
        ) || !rules_admin_only(
            &parent_rules,
            rights::DELETE_SUBDIRECTORIES_AND_FILES | rights::CHANGE_PERMISSIONS | rights::TAKE_OWNERSHIP,
            false,
            true,
        ) {
            return Ok(Trust::OthersCanChange);
        }
        above_rules = parent_rules;
        above = parent;
    }
    Ok(Trust::AdminOnly)
}

/// Whether only the trusted SIDs may do any of `mask` under `security` - and own it, when `check_owner`.
pub fn rules_admin_only(security: &Security, mask: u32, check_owner: bool, skip_inherit_only: bool) -> bool {
    if check_owner && security.owner.as_deref().is_some_and(|o| !TRUSTED_SIDS.contains(&o)) {
        return false;
    }
    // An inherit-only entry is for what is made inside the folder, not for the folder itself.
    !security.aces.iter().any(|a| {
        a.allow
            && a.file_rights() & mask != 0
            && !(skip_inherit_only && a.is_inherit_only())
            && !TRUSTED_SIDS.contains(&a.sid.as_str())
    })
}

pub(crate) fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

// winget is the App Installer package's: in the user's WindowsApps it is a link anyone of theirs could change; in
// Program Files\WindowsApps it is TrustedInstaller's.
fn winget() -> Resolved {
    if let Some(program_files) = known_folder(&FOLDERID_ProgramFiles)
        && let Ok(entries) = std::fs::read_dir(program_files.join("WindowsApps"))
    {
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                name.starts_with("microsoft.desktopappinstaller_")
                    && (name.ends_with("_x64__8wekyb3d8bbwe") || name.ends_with("_neutral__8wekyb3d8bbwe"))
            })
            .map(|e| e.path().join("winget.exe"))
            .filter(|p| p.is_file())
            .collect();
        candidates.sort_by_key(|p| std::cmp::Reverse(p.display().to_string().to_ascii_lowercase()));
        if let Some(found) = candidates.iter().map(|p| check(p)).find(|r| r.path.is_some()) {
            return found;
        }
    }
    Resolved::refused(
        "winget (the App Installer)",
        "no copy of it only administrators can change was found in Program Files\\WindowsApps.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::{Ace, flags};

    #[test]
    fn a_windows_program_is_only_administrators() {
        let cmd = dnnm_infrastructure::process::system32("cmd.exe");
        assert_eq!(admin_only(&cmd), Trust::AdminOnly);
        assert_eq!(final_path(&cmd).unwrap().file_name().unwrap().to_ascii_lowercase(), "cmd.exe");
    }

    #[test]
    fn a_file_in_a_temporary_folder_can_be_changed_by_others() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tool.exe");
        std::fs::write(&file, b"MZ").unwrap();
        // The user's own profile: the user (not an administrator group) may change it.
        assert_eq!(admin_only(&file), Trust::OthersCanChange);
        assert_eq!(admin_only(&dir.path().join("missing.exe")), Trust::OthersCanChange);
    }

    #[test]
    fn rules_count_only_the_trusted_sids() {
        let mut rules = Security::protected(
            Some(sids::TRUSTED_INSTALLER),
            vec![Ace::full_control(sids::SYSTEM), Ace::allow(sids::USERS, rights::READ_AND_EXECUTE)],
        );
        assert!(rules_admin_only(&rules, CHANGES, true, false));
        rules.aces.push(Ace { allow: true, sid: sids::USERS.into(), mask: 0x4000_0000, flags: flags::INHERIT_ONLY });
        assert!(!rules_admin_only(&rules, CHANGES, true, false), "generic write counts");
        assert!(rules_admin_only(&rules, CHANGES, true, true), "unless inherit-only entries are skipped");
        rules.owner = Some("S-1-5-21-1-2-3-1001".into());
        assert!(!rules_admin_only(&rules, CHANGES, true, true), "a user owns it");
        assert!(rules_admin_only(&rules, CHANGES, false, true));
    }

    #[test]
    fn expands_only_system_folders_and_the_computers_variables() {
        let folders = HashMap::from([("systemroot".to_string(), r"C:\Windows".to_string())]);
        let machine = |name: &str| (name == "JAVA").then(|| r"%SystemRoot%\java".to_string());
        assert_eq!(expand(r"%SystemRoot%\System32", 0, &folders, &machine).as_deref(), Some(r"C:\Windows\System32"));
        assert_eq!(expand(r"%JAVA%\bin", 0, &folders, &machine).as_deref(), Some(r"C:\Windows\java\bin"));
        assert_eq!(expand(r"%USERPROFILE%\bin", 0, &folders, &machine), None, "the user's own: left out");
        assert_eq!(expand("100%", 0, &folders, &machine).as_deref(), Some("100%"));
    }

    #[test]
    fn the_machine_path_has_system32() {
        let system32 = dnnm_infrastructure::process::system32("").display().to_string().to_ascii_lowercase();
        let system32 = system32.trim_end_matches('\\');
        assert!(machine_path().to_ascii_lowercase().split(';').any(|f| f.trim_end_matches('\\') == system32));
        assert!(machine_path_ext().to_ascii_uppercase().contains(".EXE"));
        assert!(!on_machine_path("cmd").is_empty());
    }

    #[test]
    fn says_why_a_program_isnt_started() {
        assert_eq!(Resolved::default().problem("docker"), "docker isn't installed (or not on the computer's PATH).");
        let refused = Resolved::refused(r"C:\Users\x\docker.exe", "it can't be opened to check it.");
        assert_eq!(
            refused.problem("docker"),
            r"DNN Manager doesn't start C:\Users\x\docker.exe as Administrator: it can't be opened to check it. Install it for all users (in Program Files) - DNN Manager uses that copy."
        );
    }
}
