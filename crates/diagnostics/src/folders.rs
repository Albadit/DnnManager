//! DNN's folders against the account its app pool runs as (`FolderInspector.cs`): whether each is there, read-only,
//! and whether the account may write to it by the folder's ACL. The account's own entries count, and those of the
//! groups every app pool identity is in (IIS_IUSRS, Users, Authenticated Users, Everyone); a deny wins. That is what
//! the ACL grants - not a sign-in as the account.

use std::path::Path;

/// One of the folders DNN writes to or needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderCheck {
    /// "(site root)", "/bin", "/Portals/_default".
    pub folder: String,
    pub exists: bool,
    pub read_only: bool,
    /// By the folder's ACL; None when the account isn't known or the ACL can't be read.
    pub writable: Option<bool>,
    /// Which entry gives (or denies) the write: the account itself, IIS_IUSRS, Users…
    pub via: Option<String>,
    pub required: bool,
    pub needs_write: bool,
}

/// DNN's folders: what it needs - and where it writes (uploads, logs, installs, the cache).
const DNN_FOLDERS: [(&str, bool, bool); 7] = [
    (".", true, true),
    ("bin", true, true),
    ("App_Data", true, true),
    ("Portals", true, true),
    (r"Portals\_default", true, true),
    ("DesktopModules", true, true),
    ("Providers", true, false),
];

/// The groups every app pool identity is in, by SID: IIS_IUSRS, Users, Authenticated Users, Everyone.
const APP_POOL_GROUPS: [&str; 4] = ["S-1-5-32-568", "S-1-5-32-545", "S-1-5-11", "S-1-1-0"];

const WRITE_DATA: u32 = 0x2;
const APPEND_DATA: u32 = 0x4;
const FULL_CONTROL: u32 = 0x1F_01FF;
const MODIFY: u32 = 0x3_01BF;

pub fn inspect(root: &Path, account: Option<&str>) -> Vec<FolderCheck> {
    let account_sid = account.and_then(account_sid);
    DNN_FOLDERS
        .iter()
        .map(|&(folder, required, needs_write)| {
            let path = if folder == "." { root.to_path_buf() } else { root.join(folder) };
            let name =
                if folder == "." { "(site root)".to_string() } else { format!("/{}", folder.replace('\\', "/")) };
            let Some(meta) = std::fs::metadata(&path).ok().filter(|m| m.is_dir()) else {
                return FolderCheck {
                    folder: name,
                    exists: false,
                    read_only: false,
                    writable: None,
                    via: None,
                    required,
                    needs_write,
                };
            };
            let (writable, via) = match (account, &account_sid) {
                (None, _) => (None, None),
                (Some(_), Some(sid)) => can_write(&path, sid),
                (Some(account), None) => {
                    (None, Some(format!("permissions can't be read: {account} isn't a known account")))
                }
            };
            FolderCheck {
                folder: name,
                exists: true,
                read_only: meta.permissions().readonly(),
                writable,
                via,
                required,
                needs_write,
            }
        })
        .collect()
}

/// The SID of an app pool's account: an app pool identity's is computed, the service accounts' are fixed, any
/// other is looked up.
fn account_sid(account: &str) -> Option<String> {
    const PREFIX: &str = r"IIS APPPOOL\";
    let upper = account.to_ascii_uppercase();
    if let Some(pool) = upper.strip_prefix(PREFIX).map(|_| &account[PREFIX.len()..]) {
        return Some(dnnm_infrastructure::iis_admin::app_pool_sid(pool));
    }
    match upper.as_str() {
        r"NT AUTHORITY\NETWORK SERVICE" => Some("S-1-5-20".into()),
        r"NT AUTHORITY\LOCAL SERVICE" => Some("S-1-5-19".into()),
        r"NT AUTHORITY\SYSTEM" => Some("S-1-5-18".into()),
        _ => lookup_account(account),
    }
}

/// "BUILTIN\IIS_IUSRS" → "IIS_IUSRS".
fn short(identity: &str) -> String {
    identity.strip_prefix(r"BUILTIN\").unwrap_or(identity).to_string()
}

fn rights(mask: u32) -> &'static str {
    if mask & FULL_CONTROL == FULL_CONTROL {
        "Full control"
    } else if mask & MODIFY == MODIFY {
        "Modify"
    } else {
        "Write"
    }
}

/// An entry of a folder's ACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEntry {
    pub sid: String,
    pub name: String,
    pub allow: bool,
    pub mask: u32,
    pub inherited: bool,
}

/// What the entries say of `sid` (and the app pool groups) writing: a deny wins, then the first allow.
pub fn decide(entries: &[AccessEntry], sid: &str) -> (Option<bool>, Option<String>) {
    let relevant: Vec<&AccessEntry> = entries
        .iter()
        .filter(|e| e.sid.eq_ignore_ascii_case(sid) || APP_POOL_GROUPS.contains(&e.sid.as_str()))
        .filter(|e| e.mask & (WRITE_DATA | APPEND_DATA) != 0)
        .collect();
    if let Some(deny) = relevant.iter().find(|e| !e.allow) {
        return (Some(false), Some(format!("denied for {}", short(&deny.name))));
    }
    if let Some(allow) = relevant.iter().find(|e| e.allow) {
        let inherited = if allow.inherited { ", inherited" } else { "" };
        return (Some(true), Some(format!("{} - {}{inherited}", short(&allow.name), rights(allow.mask))));
    }
    (Some(false), Some("no entry gives it write access".into()))
}

fn can_write(dir: &Path, sid: &str) -> (Option<bool>, Option<String>) {
    match access_entries(dir) {
        Ok(entries) => decide(&entries, sid),
        Err(e) => (None, Some(format!("permissions can't be read: {e}"))),
    }
}

#[cfg(windows)]
fn sid_text(sid: windows::Win32::Security::PSID) -> Option<String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::core::PWSTR;
    let mut text = PWSTR::null();
    // SAFETY: the string Windows allocates is copied, then freed.
    unsafe {
        ConvertSidToStringSidW(sid, &mut text).ok()?;
        let value = text.to_string().ok();
        LocalFree(Some(HLOCAL(text.0 as _)));
        value
    }
}

#[cfg(windows)]
fn sid_name(sid: windows::Win32::Security::PSID) -> Option<String> {
    use windows::Win32::Security::{LookupAccountSidW, SID_NAME_USE};
    use windows::core::{PCWSTR, PWSTR};
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    // SAFETY: both buffers' lengths are passed with them.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            sid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        )
        .ok()?;
    }
    let name = String::from_utf16_lossy(&name[..name_len as usize]);
    let domain = String::from_utf16_lossy(&domain[..domain_len as usize]);
    Some(if domain.is_empty() { name } else { format!(r"{domain}\{name}") })
}

#[cfg(windows)]
fn lookup_account(account: &str) -> Option<String> {
    use windows::Win32::Security::{LookupAccountNameW, PSID, SID_NAME_USE};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    let mut sid = vec![0u8; 256];
    let mut domain = [0u16; 256];
    let (mut sid_len, mut domain_len) = (sid.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    // SAFETY: both buffers' lengths are passed with them.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &HSTRING::from(account),
            Some(PSID(sid.as_mut_ptr() as _)),
            &mut sid_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        )
        .ok()?;
    }
    sid_text(PSID(sid.as_mut_ptr() as _))
}

#[cfg(windows)]
fn access_entries(dir: &Path) -> Result<Vec<AccessEntry>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
        GetAclInformation, PSECURITY_DESCRIPTOR, PSID,
    };
    use windows::core::HSTRING;

    const ACCESS_ALLOWED: u8 = 0;
    const ACCESS_DENIED: u8 = 1;
    const INHERITED_ACE: u8 = 0x10;

    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: Windows allocates the descriptor (freed below) and points the DACL into it; every ACE is read within
    // the ACL Windows describes.
    unsafe {
        let error = GetNamedSecurityInfoW(
            &HSTRING::from(dir.as_os_str()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        );
        if error.is_err() {
            return Err(error.to_hresult().message());
        }
        let mut entries = Vec::new();
        if !dacl.is_null() {
            let mut size = ACL_SIZE_INFORMATION::default();
            let info = GetAclInformation(
                dacl,
                &mut size as *mut _ as *mut _,
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            );
            if info.is_ok() {
                for i in 0..size.AceCount {
                    let mut ace = std::ptr::null_mut();
                    if GetAce(dacl, i, &mut ace).is_err() {
                        continue;
                    }
                    let header = &*(ace as *const ACE_HEADER);
                    if header.AceType != ACCESS_ALLOWED && header.AceType != ACCESS_DENIED {
                        continue;
                    }
                    // ACCESS_ALLOWED_ACE and ACCESS_DENIED_ACE: header, mask, then the SID.
                    let mask = *((ace as *const u8).add(4) as *const u32);
                    let sid = PSID((ace as *mut u8).add(8) as _);
                    let Some(text) = sid_text(sid) else { continue };
                    entries.push(AccessEntry {
                        name: sid_name(sid).unwrap_or_else(|| text.clone()),
                        sid: text,
                        allow: header.AceType == ACCESS_ALLOWED,
                        mask,
                        inherited: header.AceFlags & INHERITED_ACE != 0,
                    });
                }
            }
        }
        LocalFree(Some(HLOCAL(descriptor.0)));
        Ok(entries)
    }
}

#[cfg(not(windows))]
fn lookup_account(_account: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn access_entries(_dir: &Path) -> Result<Vec<AccessEntry>, String> {
    Err("not on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sid: &str, name: &str, allow: bool, mask: u32, inherited: bool) -> AccessEntry {
        AccessEntry { sid: sid.into(), name: name.into(), allow, mask, inherited }
    }

    #[test]
    fn a_deny_wins_then_the_first_allow_names_how() {
        let pool = "S-1-5-82-1";
        let entries = [
            entry("S-1-5-32-545", r"BUILTIN\Users", true, 0x1200A9, true),
            entry("S-1-5-32-568", r"BUILTIN\IIS_IUSRS", true, MODIFY, true),
            entry(pool, r"IIS APPPOOL\shop", true, FULL_CONTROL, false),
        ];
        assert_eq!(decide(&entries, pool), (Some(true), Some("IIS_IUSRS - Modify, inherited".into())));
        let denied = [entries[2].clone(), entry(pool, r"IIS APPPOOL\shop", false, WRITE_DATA, false)];
        assert_eq!(decide(&denied, pool), (Some(false), Some(r"denied for IIS APPPOOL\shop".into())));
        assert_eq!(decide(&entries[..1], pool), (Some(false), Some("no entry gives it write access".into())));
    }

    #[test]
    fn checks_the_dnn_folders_of_a_site() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(r"Portals\_default")).unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        let found = inspect(dir.path(), None);
        let names: Vec<&str> = found.iter().map(|f| f.folder.as_str()).collect();
        assert_eq!(
            names,
            ["(site root)", "/bin", "/App_Data", "/Portals", "/Portals/_default", "/DesktopModules", "/Providers"]
        );
        assert!(found[1].exists && !found[2].exists && found[2].required);
        assert_eq!(found[1].writable, None);
        // As the account running the tests, by name: its folders are its own - written to through some entry.
        let me = std::env::var("USERNAME").unwrap_or_default();
        let checked = inspect(dir.path(), Some(&me));
        assert!(checked[0].via.is_some(), "{:?}", checked[0]);
        assert_eq!(
            account_sid(r"IIS APPPOOL\DefaultAppPool").unwrap(),
            dnnm_infrastructure::iis_admin::app_pool_sid("DefaultAppPool")
        );
        assert_eq!(account_sid(r"nt authority\network service").as_deref(), Some("S-1-5-20"));
    }
}
