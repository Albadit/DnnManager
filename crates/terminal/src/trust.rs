//! Which shells an Administrator terminal may start - ports the parts of `Infrastructure/Processes/TrustedPrograms.cs`
//! `TerminalService.FindShells` uses (`Resolve` for a full path, `OnMachinePath`, `AdminOnly`, `FinalPath`).
//!
//! Only a program that nobody but administrators can change: one in a folder the signed-in user can write to could be
//! swapped by any program they run, which would then run as Administrator. Not elevated, there is no such boundary,
//! and any program goes.

use std::os::windows::fs::MetadataExt;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// NT SERVICE\TrustedInstaller, owner of Windows' own files and of Program Files.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
// Who may change a program DNN Manager runs elevated: SYSTEM, Administrators, TrustedInstaller - and "the owner of what
// is created here" (CREATOR OWNER), whose rights go to the owner, which is checked on its own.
const TRUSTED_SIDS: [&str; 4] = ["S-1-5-18", "S-1-5-32-544", "S-1-3-0", TRUSTED_INSTALLER];

// Changing a file, or putting another in its place (or a DLL beside it): WriteData, AppendData, WriteExtendedAttributes,
// DeleteSubdirectoriesAndFiles, WriteAttributes, Delete, ChangePermissions, TakeOwnership - and the generic rights
// that include them (.NET's FileSystemRights doesn't show those; an entry granting GENERIC_WRITE counts here).
const CHANGES: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | GENERIC;
const DELETE_OR_TAKE_OVER: u32 = 0x1_0000 | 0x4_0000 | 0x8_0000 | GENERIC;
const DELETE_CHILDREN_OR_TAKE_OVER: u32 = 0x40 | 0x4_0000 | 0x8_0000 | GENERIC;
const GENERIC: u32 = 0x1000_0000 | 0x4000_0000;
const REPARSE_POINT: u32 = 0x400;

/// Whether only administrators can change a program - or that couldn't be found out (a read failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    AdminOnly,
    OthersCanChange,
    Unknown,
}

/// A program DNN Manager may start with its rights: `path`, the file's own path with every link, junction and short
/// name resolved - or None, with `refused` naming the copy found but not trusted and `reason` saying why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: Option<String>,
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
}

/// `file` (a full path) as it may be started with DNN Manager's rights (`elevated`); not elevated, the path itself.
pub fn resolve(file: &str, elevated: bool) -> Resolved {
    if !elevated {
        return Resolved { path: Some(file.to_string()), refused: None, reason: None };
    }
    let refuse =
        |reason: &str| Resolved { path: None, refused: Some(file.to_string()), reason: Some(reason.to_string()) };
    let Some(final_path) = final_path(file) else { return refuse("it can't be opened to check it.") };
    match admin_only(&final_path) {
        Trust::AdminOnly => Resolved { path: Some(final_path), refused: None, reason: None },
        Trust::Unknown => refuse("who may change it couldn't be read - try again in a moment."),
        Trust::OthersCanChange => refuse(
            "programs without administrator rights could change it, and DNN Manager would run it as Administrator.",
        ),
    }
}

/// Where `name` is on the computer's PATH (with PATHEXT's extensions when it has none) - every copy, in PATH's order.
pub fn on_machine_path(name: &str) -> Vec<String> {
    let extensions = if Path::new(name).extension().is_some() {
        vec![String::new()]
    } else {
        crate::environment::machine_path_ext()
    };
    let mut found = Vec::new();
    for folder in crate::environment::machine_path() {
        for extension in &extensions {
            let candidate = crate::environment::join(&folder, &format!("{name}{extension}"));
            if is_fully_qualified(&candidate) && Path::new(&candidate).is_file() {
                found.push(candidate);
            }
        }
    }
    found
}

/// A drive path (`C:\…`) or a UNC path - not one relative to a drive's current folder.
pub fn is_fully_qualified(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
}

/// The path Windows opens for `file` - every link, junction and short name on the way resolved - or None when it
/// can't be opened (or is on a network share).
fn final_path(file: &str) -> Option<String> {
    let path = std::fs::canonicalize(file).ok()?.to_string_lossy().into_owned();
    if path.starts_with("\\\\?\\UNC\\") {
        return None;
    }
    Some(path.strip_prefix("\\\\?\\").unwrap_or(&path).to_string())
}

/// Whether only administrators (and Windows) can change `file`: the file itself, its folder (a file put beside it, a
/// DLL it loads), and every folder above - none a link or junction, none that others may delete, rename or take over.
fn admin_only(file: &str) -> Trust {
    let path = Path::new(file);
    let Ok(metadata) = std::fs::symlink_metadata(path) else { return Trust::Unknown };
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Trust::OthersCanChange;
    }
    let file_rules = match Security::read(file) {
        Ok(rules) => rules,
        Err(trust) => return trust,
    };
    if !file_rules.admin_only(CHANGES, true, false) {
        return Trust::OthersCanChange;
    }
    let Some(folder) = path.parent() else { return Trust::OthersCanChange };
    if is_reparse_point(folder) {
        return Trust::OthersCanChange;
    }
    let mut above_rules = match Security::read(&folder.to_string_lossy()) {
        Ok(rules) => rules,
        Err(trust) => return trust,
    };
    if !above_rules.admin_only(CHANGES, true, false) {
        return Trust::OthersCanChange;
    }
    // The folders above may let others make folders (C:\ does) - not delete, rename or take over one on the way.
    let mut above = folder;
    while let Some(parent) = above.parent() {
        if parent.parent().is_some() && is_reparse_point(parent) {
            return Trust::OthersCanChange;
        }
        let parent_rules = match Security::read(&parent.to_string_lossy()) {
            Ok(rules) => rules,
            Err(trust) => return trust,
        };
        if !above_rules.admin_only(DELETE_OR_TAKE_OVER, false, true)
            || !parent_rules.admin_only(DELETE_CHILDREN_OR_TAKE_OVER, false, true)
        {
            return Trust::OthersCanChange;
        }
        above_rules = parent_rules;
        above = parent;
    }
    Trust::AdminOnly
}

fn is_reparse_point(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_attributes() & REPARSE_POINT != 0).unwrap_or(true)
}

/// The owner and the allowing entries of an object's rules, the SIDs as strings.
struct Security {
    owner: Option<String>,
    // (rights, inherit only, who)
    allowed: Vec<(u32, bool, String)>,
}

impl Security {
    fn read(path: &str) -> Result<Security, Trust> {
        let name = HSTRING::from(path);
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the descriptor GetNamedSecurityInfoW allocates holds the owner and DACL it points to; it is freed
        // with LocalFree once they have been read.
        unsafe {
            let result = GetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            );
            if result != ERROR_SUCCESS {
                // Access denied: not even an administrator may read who can change it - not one to trust.
                return Err(if result.0 == 5 { Trust::OthersCanChange } else { Trust::Unknown });
            }
            let mut security = Security { owner: sid_string(owner), allowed: Vec::new() };
            // No DACL at all: everyone may do anything.
            if dacl.is_null() {
                security.allowed.push((u32::MAX, false, "S-1-1-0".into()));
            } else {
                for index in 0..(*dacl).AceCount as u32 {
                    let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
                    if GetAce(dacl, index, &mut ace).is_err() {
                        continue;
                    }
                    let header = &*(ace as *const ACE_HEADER);
                    // ACCESS_ALLOWED_ACE_TYPE and ACCESS_ALLOWED_CALLBACK_ACE_TYPE: a mask and a SID.
                    if header.AceType != 0 && header.AceType != 9 {
                        continue;
                    }
                    let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
                    let sid = PSID(&allowed.SidStart as *const u32 as *mut _);
                    let inherit_only = header.AceFlags as u32 & INHERIT_ONLY_ACE.0 != 0;
                    security.allowed.push((allowed.Mask, inherit_only, sid_string(sid).unwrap_or_default()));
                }
            }
            LocalFree(Some(HLOCAL(descriptor.0)));
            Ok(security)
        }
    }

    fn admin_only(&self, rights: u32, check_owner: bool, skip_inherit_only: bool) -> bool {
        if check_owner && self.owner.as_deref().is_some_and(|owner| !TRUSTED_SIDS.contains(&owner)) {
            return false;
        }
        // An inherit-only entry is for what is made inside the folder, not for the folder itself.
        !self.allowed.iter().any(|(mask, inherit_only, who)| {
            mask & rights != 0 && !(skip_inherit_only && *inherit_only) && !TRUSTED_SIDS.contains(&who.as_str())
        })
    }
}

fn sid_string(sid: PSID) -> Option<String> {
    if sid.is_invalid() {
        return None;
    }
    let mut text = PWSTR::null();
    // SAFETY: the string ConvertSidToStringSidW allocates is freed with LocalFree after it is copied.
    unsafe {
        ConvertSidToStringSidW(sid, &mut text).ok()?;
        let result = text.to_string().ok();
        LocalFree(Some(HLOCAL(text.0.cast())));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problem_texts() {
        let missing = Resolved { path: None, refused: None, reason: None };
        assert_eq!(missing.problem("PowerShell 7"), "PowerShell 7 isn't installed (or not on the computer's PATH).");
        let refused = Resolved {
            path: None,
            refused: Some("C:\\Users\\me\\pwsh.exe".into()),
            reason: Some(
                "programs without administrator rights could change it, and DNN Manager would run it as Administrator."
                    .into(),
            ),
        };
        assert_eq!(
            refused.problem("PowerShell 7"),
            "DNN Manager doesn't start C:\\Users\\me\\pwsh.exe as Administrator: programs without administrator rights could change it, and DNN Manager would run it as Administrator. Install it for all users (in Program Files) - DNN Manager uses that copy."
        );
    }

    #[test]
    fn not_elevated_any_program_goes() {
        assert_eq!(resolve("C:\\x.exe", false).path.as_deref(), Some("C:\\x.exe"));
    }

    #[test]
    fn windows_own_cmd_is_admin_only_and_a_temp_file_is_not() {
        let system = crate::environment::system_folders().system;
        let cmd = resolve(&crate::environment::join(&system, "cmd.exe"), true);
        assert!(cmd.path.is_some(), "{cmd:?}");
        let temp = std::env::temp_dir().join(format!("dnnm-terminal-trust-{}.exe", std::process::id()));
        std::fs::write(&temp, b"x").unwrap();
        let refused = resolve(&temp.to_string_lossy(), true);
        std::fs::remove_file(&temp).unwrap();
        assert!(refused.path.is_none() && refused.refused.is_some(), "{refused:?}");
    }

    #[test]
    fn fully_qualified_paths() {
        assert!(is_fully_qualified("C:\\x"));
        assert!(is_fully_qualified("\\\\server\\x"));
        assert!(!is_fully_qualified("C:x"));
        assert!(!is_fully_qualified("bin"));
    }
}
