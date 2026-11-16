//! Windows' redirection trust for this process (`Files/RedirectionTrust.cs`): a junction that a program without
//! administrator rights made is no longer followed by DNN Manager. Opening a path through one fails with "untrusted
//! mount point"; a junction an administrator made still works. Other programs, IIS among them, aren't affected.

use std::ffi::c_void;

use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{PROCESS_MITIGATION_POLICY, SetProcessMitigationPolicy};
use windows::core::PCWSTR;

use crate::win::wide;

/// Turns it on; false where Windows doesn't have it (an older Windows without its recent updates).
pub fn enforce() -> bool {
    // ProcessRedirectionTrustPolicy, PROCESS_MITIGATION_REDIRECTION_TRUST_POLICY.EnforceRedirectionTrust.
    const PROCESS_REDIRECTION_TRUST_POLICY: i32 = 16;
    let flags: u32 = 0x1;
    // SAFETY: a 4-byte policy structure, its size passed with it.
    unsafe {
        SetProcessMitigationPolicy(
            PROCESS_MITIGATION_POLICY(PROCESS_REDIRECTION_TRUST_POLICY),
            &flags as *const u32 as *const c_void,
            std::mem::size_of::<u32>(),
        )
        .is_ok()
    }
}

/// Windows' Developer Mode is on (`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock`,
/// `AllowDevelopmentWithoutDevLicense` = 1): any program may then make symbolic links - which redirection trust (it
/// covers junctions) doesn't stop. DNN Manager's own link checks still hold; this is for a start-up notice.
pub fn developer_mode_allows_links() -> bool {
    let key = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock");
    let name = wide("AllowDevelopmentWithoutDevLicense");
    let mut value = 0u32;
    let mut size = 4u32;
    // SAFETY: a DWORD to fill, its size passed with it.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    read.is_ok() && value != 0
}

#[cfg(test)]
mod tests {
    #[test]
    fn reading_developer_mode_never_fails() {
        let _ = super::developer_mode_allows_links();
    }
}
