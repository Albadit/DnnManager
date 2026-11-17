//! A file's or folder's owner and access rules - what the C# code reads and writes with `DirectorySecurity` and
//! `FileSystemAccessRule` (`PrivateTemp`, `ProjectsFolderGuard`, `FileLockService`, `TrustedPrograms`).
//!
//! Deviation: .NET's access-control classes have no Rust counterpart. Rules are read with `GetNamedSecurityInfoW` into
//! [`Security`] (SIDs as strings, masks as numbers - the logic over them is plain Rust and tested on values), and
//! written as an SDDL string (`O:…D:P(A;OICI;0x1f01ff;;;S-1-5-18)`) turned into a security descriptor by Windows.

use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
    SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce, GetAclInformation,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation,
    OBJECT_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

use crate::win::{Handle, wide, wide_path};

/// Well-known SIDs the C# code names.
pub mod sids {
    pub const EVERYONE: &str = "S-1-1-0";
    pub const CREATOR_OWNER: &str = "S-1-3-0";
    /// OWNER RIGHTS: with an entry for it, the owner gets what that entry says - not the right to change the rules.
    pub const OWNER_RIGHTS: &str = "S-1-3-4";
    pub const INTERACTIVE: &str = "S-1-5-4";
    pub const AUTHENTICATED_USERS: &str = "S-1-5-11";
    pub const IUSR: &str = "S-1-5-17";
    pub const SYSTEM: &str = "S-1-5-18";
    pub const ADMINISTRATORS: &str = "S-1-5-32-544";
    pub const USERS: &str = "S-1-5-32-545";
    pub const IIS_IUSRS: &str = "S-1-5-32-568";
    /// NT SERVICE\TrustedInstaller, owner of Windows' own files and of Program Files: the same on every Windows.
    pub const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
    /// An app pool's identity (IIS AppPool\name) starts so.
    pub const APP_POOL_PREFIX: &str = "S-1-5-82-";
}

/// `FileSystemRights` values.
pub mod rights {
    pub const WRITE_DATA: u32 = 0x2;
    pub const APPEND_DATA: u32 = 0x4;
    pub const WRITE_EXTENDED_ATTRIBUTES: u32 = 0x10;
    pub const DELETE_SUBDIRECTORIES_AND_FILES: u32 = 0x40;
    pub const WRITE_ATTRIBUTES: u32 = 0x100;
    pub const DELETE: u32 = 0x1_0000;
    pub const CHANGE_PERMISSIONS: u32 = 0x4_0000;
    pub const TAKE_OWNERSHIP: u32 = 0x8_0000;
    pub const SYNCHRONIZE: u32 = 0x10_0000;
    pub const READ_AND_EXECUTE: u32 = 0x12_00A9;
    pub const MODIFY: u32 = 0x13_01BF;
    pub const FULL_CONTROL: u32 = 0x1F_01FF;
}

/// ACE flags, as in an ACE header.
pub mod flags {
    pub const OBJECT_INHERIT: u8 = 0x1;
    pub const CONTAINER_INHERIT: u8 = 0x2;
    pub const NO_PROPAGATE: u8 = 0x4;
    pub const INHERIT_ONLY: u8 = 0x8;
    pub const INHERITED: u8 = 0x10;
}

/// One access rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub allow: bool,
    pub sid: String,
    /// The mask as stored - generic rights (`GA`) included.
    pub mask: u32,
    pub flags: u8,
}

impl Ace {
    /// Allowed in full for this folder, its subfolders and files.
    pub fn full_control(sid: &str) -> Self {
        Self::allow(sid, rights::FULL_CONTROL)
    }

    /// Allowed `mask` for this folder, its subfolders and files.
    pub fn allow(sid: &str, mask: u32) -> Self {
        Self { allow: true, sid: sid.into(), mask, flags: flags::OBJECT_INHERIT | flags::CONTAINER_INHERIT }
    }

    pub fn is_inherited(&self) -> bool {
        self.flags & flags::INHERITED != 0
    }

    pub fn is_inherit_only(&self) -> bool {
        self.flags & flags::INHERIT_ONLY != 0
    }

    /// The rights it gives on a file, generic rights mapped to the file rights they stand for.
    pub fn file_rights(&self) -> u32 {
        const GENERIC_ALL: u32 = 0x1000_0000;
        const GENERIC_EXECUTE: u32 = 0x2000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        let mut mask = self.mask & 0x0FFF_FFFF;
        if self.mask & GENERIC_ALL != 0 {
            mask |= rights::FULL_CONTROL;
        }
        if self.mask & GENERIC_EXECUTE != 0 {
            mask |= 0x12_00A0;
        }
        if self.mask & GENERIC_WRITE != 0 {
            mask |= 0x12_0116;
        }
        if self.mask & GENERIC_READ != 0 {
            mask |= 0x12_0089;
        }
        mask
    }

    fn sddl(&self) -> String {
        let mut text = String::new();
        for (flag, name) in [
            (flags::OBJECT_INHERIT, "OI"),
            (flags::CONTAINER_INHERIT, "CI"),
            (flags::NO_PROPAGATE, "NP"),
            (flags::INHERIT_ONLY, "IO"),
        ] {
            if self.flags & flag != 0 {
                text.push_str(name);
            }
        }
        format!("({};{text};{:#x};;;{})", if self.allow { "A" } else { "D" }, self.mask, self.sid)
    }
}

/// An owner and a DACL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Security {
    pub owner: Option<String>,
    /// Inherits nothing from the folder above (`AreAccessRulesProtected`).
    pub protected: bool,
    pub aces: Vec<Ace>,
}

impl Security {
    /// Inherits nothing; `aces` alone, owned by `owner` when given.
    pub fn protected(owner: Option<&str>, aces: Vec<Ace>) -> Self {
        Self { owner: owner.map(str::to_string), protected: true, aces }
    }

    /// The security descriptor as SDDL: the owner (when set) and the DACL's own rules - inherited ones are Windows' to
    /// add. Deny rules come first, as Windows orders them.
    pub fn to_sddl(&self) -> String {
        let mut text = String::new();
        if let Some(owner) = &self.owner {
            text.push_str(&format!("O:{owner}"));
        }
        text.push_str(if self.protected { "D:P" } else { "D:" });
        let own = self.aces.iter().filter(|a| !a.is_inherited());
        for ace in own.clone().filter(|a| !a.allow).chain(own.filter(|a| a.allow)) {
            text.push_str(&ace.sddl());
        }
        text
    }

    /// What Windows has for `path`.
    pub fn read(path: &Path) -> std::io::Result<Security> {
        let name = wide_path(path);
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: Windows allocates the descriptor; owner and DACL point into it, and it is freed after they are read.
        unsafe {
            let error = GetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            );
            if error.0 != 0 {
                return Err(std::io::Error::from_raw_os_error(error.0 as i32));
            }
            let result = Self::from_descriptor(descriptor, owner, dacl);
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            result
        }
    }

    unsafe fn from_descriptor(
        descriptor: PSECURITY_DESCRIPTOR,
        owner: PSID,
        dacl: *mut ACL,
    ) -> std::io::Result<Security> {
        // SAFETY: the caller passes a live descriptor and the owner and DACL Windows read from it.
        unsafe {
            let mut control = 0u16;
            let mut revision = 0u32;
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision)?;
            let protected = control & SE_DACL_PROTECTED.0 != 0;
            let owner = sid_string(owner);
            let mut aces = Vec::new();
            if dacl.is_null() {
                // No DACL: everyone may do everything.
                aces.push(Ace::full_control(sids::EVERYONE));
            } else {
                let mut info = ACL_SIZE_INFORMATION::default();
                GetAclInformation(
                    dacl,
                    &mut info as *mut _ as *mut c_void,
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )?;
                for index in 0..info.AceCount {
                    let mut ace: *mut c_void = std::ptr::null_mut();
                    GetAce(dacl, index, &mut ace)?;
                    let header = &*(ace as *const ACE_HEADER);
                    // ACCESS_ALLOWED(_CALLBACK)_ACE and ACCESS_DENIED(_CALLBACK)_ACE: header, mask, then the SID.
                    let allow = match header.AceType {
                        0 | 9 => true,
                        1 | 10 => false,
                        _ => continue,
                    };
                    let mask = *(ace.add(4) as *const u32);
                    let sid = PSID(ace.add(8));
                    aces.push(Ace { allow, sid: sid_string(sid).unwrap_or_default(), mask, flags: header.AceFlags });
                }
            }
            Ok(Security { owner, protected, aces })
        }
    }

    /// Gives `path` this owner (when set) and these rules - nothing inherited when protected.
    pub fn apply(&self, path: &Path) -> std::io::Result<()> {
        let name = wide_path(path);
        self.with_descriptor(|descriptor| {
            // SAFETY: owner and DACL point into the descriptor, which lives for the call.
            unsafe {
                let mut owner = PSID::default();
                let mut defaulted = Default::default();
                GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
                let mut present = Default::default();
                let mut dacl: *mut ACL = std::ptr::null_mut();
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)?;
                let mut what: OBJECT_SECURITY_INFORMATION = DACL_SECURITY_INFORMATION;
                what |= if self.protected {
                    PROTECTED_DACL_SECURITY_INFORMATION
                } else {
                    UNPROTECTED_DACL_SECURITY_INFORMATION
                };
                if self.owner.is_some() {
                    what |= OWNER_SECURITY_INFORMATION;
                }
                let error = SetNamedSecurityInfoW(
                    PCWSTR(name.as_ptr()),
                    SE_FILE_OBJECT,
                    what,
                    if self.owner.is_some() { Some(owner) } else { None },
                    None,
                    Some(dacl),
                    None,
                );
                if error.0 != 0 {
                    return Err(std::io::Error::from_raw_os_error(error.0 as i32));
                }
                Ok(())
            }
        })
    }

    /// Makes the folder `path` with these rules from the start (`DirectoryInfo.Create(DirectorySecurity)`). An error
    /// when it is there already.
    pub fn create_dir(&self, path: &Path) -> std::io::Result<()> {
        let name = wide_path(path);
        self.with_descriptor(|descriptor| {
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            // SAFETY: the descriptor lives for the call.
            unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&attributes)) }.map_err(std::io::Error::from)
        })
    }

    fn with_descriptor<T>(
        &self,
        use_it: impl FnOnce(PSECURITY_DESCRIPTOR) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let sddl = wide(&self.to_sddl());
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: Windows allocates the descriptor from the SDDL; it is freed after use.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )?;
            let result = use_it(descriptor);
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            result
        }
    }

    /// Whether an allow rule - its own or inherited - is for one of `sids`.
    pub fn allows_any_of(&self, sids: &[&str]) -> bool {
        self.aces.iter().any(|a| a.allow && sids.contains(&a.sid.as_str()))
    }
}

unsafe fn sid_string(sid: PSID) -> Option<String> {
    if sid.0.is_null() {
        return None;
    }
    let mut text = PWSTR::null();
    // SAFETY: a valid SID from Windows; the string it makes is copied and freed.
    unsafe {
        ConvertSidToStringSidW(sid, &mut text).ok()?;
        let value = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut c_void)));
        value
    }
}

/// The signed-in user's SID (`WindowsIdentity.GetCurrent().User`).
pub fn current_user_sid() -> Option<String> {
    // SAFETY: the token handle is closed by `Handle`; the buffer is the size Windows asks for.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let token = Handle(token);
        let mut size = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        GetTokenInformation(token.0, TokenUser, Some(buffer.as_mut_ptr() as *mut c_void), size, &mut size).ok()?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        sid_string(user.User.Sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_sddl_with_the_owner_and_only_its_own_rules() {
        let mut security = Security::protected(
            Some(sids::ADMINISTRATORS),
            vec![Ace::full_control(sids::SYSTEM), Ace::allow(sids::OWNER_RIGHTS, rights::READ_AND_EXECUTE)],
        );
        security.aces.push(Ace { allow: true, sid: sids::USERS.into(), mask: 1, flags: flags::INHERITED });
        security.aces.push(Ace { allow: false, sid: sids::EVERYONE.into(), mask: rights::DELETE, flags: 0 });
        assert_eq!(
            security.to_sddl(),
            "O:S-1-5-32-544D:P(D;;0x10000;;;S-1-1-0)(A;OICI;0x1f01ff;;;S-1-5-18)(A;OICI;0x1200a9;;;S-1-3-4)"
        );
    }

    #[test]
    fn generic_rights_count_as_the_file_rights_they_stand_for() {
        let ace = Ace { allow: true, sid: sids::CREATOR_OWNER.into(), mask: 0x1000_0000, flags: flags::INHERIT_ONLY };
        assert_eq!(ace.file_rights(), rights::FULL_CONTROL);
        assert!(ace.is_inherit_only());
        let write = Ace { allow: true, sid: sids::USERS.into(), mask: 0x4000_0000, flags: 0 };
        assert_ne!(write.file_rights() & rights::WRITE_DATA, 0);
    }

    #[test]
    fn reads_a_temporary_folder_and_gives_it_rules_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let me = current_user_sid().unwrap();
        assert!(me.starts_with("S-1-5-"));
        let read = Security::read(dir.path()).unwrap();
        assert!(read.owner.is_some());
        assert!(!read.aces.is_empty());

        let folder = dir.path().join("closed");
        let rules = Security::protected(None, vec![Ace::full_control(&me), Ace::full_control(sids::SYSTEM)]);
        rules.create_dir(&folder).unwrap();
        let made = Security::read(&folder).unwrap();
        assert!(made.protected);
        assert!(made.aces.iter().all(|a| a.sid == me || a.sid == sids::SYSTEM), "{made:?}");

        let open =
            Security::protected(None, vec![Ace::full_control(&me), Ace::allow(sids::USERS, rights::READ_AND_EXECUTE)]);
        open.apply(&folder).unwrap();
        assert!(Security::read(&folder).unwrap().allows_any_of(&[sids::USERS]));
    }
}
