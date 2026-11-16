//! The token of an unelevated terminal - ports `UnelevatedToken` in `Infrastructure/Terminal/PseudoConsole.cs`.
//!
//! DNN Manager's own token with what makes it Administrator taken out, as Windows' User Account Control makes the
//! signed-in user's everyday token from the elevated one. The Administrators group only denies (it never grants), every
//! privilege but "bypass traverse checking" is gone, it runs at Medium integrity (it can't open or send input to DNN
//! Manager's own windows and processes), and what it makes belongs to the user. A restricted version of the caller's
//! own token is one Windows lets a process start another with, without the "replace a process token" privilege an
//! administrator doesn't have - a token of another process (Explorer's) would need it.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::{GENERIC_ALL, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::{
    ACL, ACL_REVISION, AddAccessAllowedAce, CheckTokenMembership, CreateRestrictedToken, CreateWellKnownSid,
    DISABLE_MAX_PRIVILEGE, DuplicateToken, GetLengthSid, GetTokenInformation, InitializeAcl, PSID,
    SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, SecurityIdentification, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DEFAULT_DACL, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_OWNER, TOKEN_QUERY,
    TOKEN_USER, TokenDefaultDacl, TokenIntegrityLevel, TokenOwner, TokenUser, WELL_KNOWN_SID_TYPE,
    WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{BOOL, w};

/// A terminal without administrator rights couldn't be started - the message says why. Nothing was started in its
/// place: an Administrator terminal is asked for on its own.
pub fn unavailable(reason: impl std::fmt::Display) -> String {
    format!("{UNAVAILABLE}{reason}")
}

const UNAVAILABLE: &str = "A terminal without administrator rights couldn't be started: ";

/// The message is [`unavailable`]'s - the rights couldn't be dropped.
pub fn is_unavailable(message: &str) -> bool {
    message.starts_with(UNAVAILABLE)
}

fn handle(owned: &OwnedHandle) -> HANDLE {
    HANDLE(owned.as_raw_handle())
}

/// The token - or the message saying why there is none (see [`unavailable`]).
pub fn create() -> Result<OwnedHandle, String> {
    // SAFETY: every handle is owned (closed on drop); every buffer outlives the call it is passed to, and is as large
    // as the call is told.
    unsafe {
        let mut raw = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut raw,
        )
        .map_err(|e| unavailable(format!("DNN Manager's own sign-in couldn't be read ({}).", e.message())))?;
        let own = OwnedHandle::from_raw_handle(raw.0);

        let mut administrators = well_known(WinBuiltinAdministratorsSid)?;
        let disable = [SID_AND_ATTRIBUTES { Sid: PSID(administrators.as_mut_ptr().cast()), Attributes: 0 }];
        let mut restricted = HANDLE::default();
        CreateRestrictedToken(handle(&own), DISABLE_MAX_PRIVILEGE, Some(&disable), None, None, &mut restricted)
            .map_err(|e| {
                unavailable(format!("a token without administrator rights couldn't be made ({}).", e.message()))
            })?;
        let restricted = OwnedHandle::from_raw_handle(restricted.0);

        let mut user = token_user(&own)?;
        set_medium_integrity(&restricted)?;
        set_owner(&restricted, &mut user)?;
        set_default_dacl(&restricted, &mut user)?;
        if is_administrator(&restricted) {
            return Err(unavailable("the token made for it still had administrator rights."));
        }
        Ok(restricted)
    }
}

fn well_known(kind: WELL_KNOWN_SID_TYPE) -> Result<Vec<u8>, String> {
    let mut sid = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = SECURITY_MAX_SID_SIZE;
    // SAFETY: the buffer is SECURITY_MAX_SID_SIZE bytes, as the call is told.
    unsafe { CreateWellKnownSid(kind, None, Some(PSID(sid.as_mut_ptr().cast())), &mut size) }
        .map_err(|e| unavailable(format!("a group couldn't be named ({}).", e.message())))?;
    sid.truncate(size as usize);
    Ok(sid)
}

/// The SID of the token's user, copied out of the token's information.
fn token_user(token: &OwnedHandle) -> Result<Vec<u8>, String> {
    let fail =
        |e: windows::core::Error| unavailable(format!("DNN Manager's own sign-in couldn't be read ({}).", e.message()));
    // SAFETY: the first call only asks for the size; the second gets a buffer of that size. The SID is copied out
    // before the buffer goes.
    unsafe {
        let mut size = 0u32;
        let _ = GetTokenInformation(handle(token), TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        GetTokenInformation(handle(token), TokenUser, Some(buffer.as_mut_ptr().cast()), size, &mut size)
            .map_err(fail)?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let length = GetLengthSid(user.User.Sid) as usize;
        Ok(std::slice::from_raw_parts(user.User.Sid.0 as *const u8, length).to_vec())
    }
}

/// Whether `token` counts as a member of Administrators (a deny-only group doesn't).
pub fn is_administrator(token: &OwnedHandle) -> bool {
    let Ok(mut administrators) = well_known(WinBuiltinAdministratorsSid) else { return true };
    // SAFETY: CheckTokenMembership needs an impersonation token: a duplicate, closed on drop.
    unsafe {
        let mut duplicate = HANDLE::default();
        if DuplicateToken(handle(token), SecurityIdentification, &mut duplicate).is_err() {
            return true;
        }
        let duplicate = OwnedHandle::from_raw_handle(duplicate.0);
        let mut member = BOOL(1);
        CheckTokenMembership(Some(handle(&duplicate)), PSID(administrators.as_mut_ptr().cast()), &mut member).is_err()
            || member.as_bool()
    }
}

/// DNN Manager runs with administrator rights - what it starts runs with them too. (`TrustedPrograms.IsElevated`:
/// the process token counts as a member of Administrators.)
pub fn is_elevated() -> bool {
    let Ok(mut administrators) = well_known(WinBuiltinAdministratorsSid) else { return false };
    let mut member = BOOL(0);
    // SAFETY: no token: the thread's or process's own; the SID buffer outlives the call.
    unsafe { CheckTokenMembership(None, PSID(administrators.as_mut_ptr().cast()), &mut member) }.is_ok()
        && member.as_bool()
}

// Medium (S-1-16-8192): what the user's own programs run at - below DNN Manager's High.
fn set_medium_integrity(token: &OwnedHandle) -> Result<(), String> {
    // SAFETY: the SID from ConvertStringSidToSidW is freed with LocalFree after use.
    unsafe {
        let mut medium = PSID::default();
        ConvertStringSidToSidW(w!("S-1-16-8192"), &mut medium)
            .map_err(|e| unavailable(format!("its integrity couldn't be lowered ({}).", e.message())))?;
        let mut label = TOKEN_MANDATORY_LABEL::default();
        label.Label.Sid = medium;
        label.Label.Attributes = SE_GROUP_INTEGRITY as u32;
        let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + GetLengthSid(medium);
        let result = SetTokenInformation(handle(token), TokenIntegrityLevel, &label as *const _ as *const _, size);
        LocalFree(Some(HLOCAL(medium.0)));
        result.map_err(|e| unavailable(format!("its integrity couldn't be lowered ({}).", e.message())))
    }
}

// What it makes is the user's: the Administrators group (the elevated token's owner) only denies in it.
fn set_owner(token: &OwnedHandle, user: &mut [u8]) -> Result<(), String> {
    let owner = TOKEN_OWNER { Owner: PSID(user.as_mut_ptr().cast()) };
    // SAFETY: the owner SID outlives the call.
    unsafe {
        SetTokenInformation(
            handle(token),
            TokenOwner,
            &owner as *const _ as *const _,
            std::mem::size_of::<TOKEN_OWNER>() as u32,
        )
    }
    .map_err(|e| unavailable(format!("its owner couldn't be set ({}).", e.message())))
}

// The rights on what it makes without rules of its own (its processes, pipes): the user's, SYSTEM's and Administrators'.
fn set_default_dacl(token: &OwnedHandle, user: &mut [u8]) -> Result<(), String> {
    let fail = |e: windows::core::Error| unavailable(format!("its default rights couldn't be set ({}).", e.message()));
    let mut system = well_known(WinLocalSystemSid)?;
    let mut administrators = well_known(WinBuiltinAdministratorsSid)?;
    // ACL header + 3 ACCESS_ALLOWED_ACEs (each a header, a mask and a SID, the SID's first DWORD counted in the ACE).
    let size = std::mem::size_of::<ACL>() + 3 * (8 + SECURITY_MAX_SID_SIZE as usize);
    let mut acl = vec![0u32; size.div_ceil(4)];
    let acl = acl.as_mut_ptr() as *mut ACL;
    // SAFETY: the ACL buffer is `size` bytes, aligned to a DWORD, and outlives SetTokenInformation.
    unsafe {
        InitializeAcl(acl, size as u32, ACL_REVISION).map_err(fail)?;
        for sid in [user, &mut system[..], &mut administrators[..]] {
            AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_ALL.0, PSID(sid.as_mut_ptr().cast())).map_err(fail)?;
        }
        let dacl = TOKEN_DEFAULT_DACL { DefaultDacl: acl };
        SetTokenInformation(
            handle(token),
            TokenDefaultDacl,
            &dacl as *const _ as *const _,
            std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
        )
        .map_err(fail)
    }
}
