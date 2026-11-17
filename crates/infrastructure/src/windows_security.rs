//! Administrator rights (`AdminElevation`): IIS's configuration is only readable elevated, so DNN Manager relaunches
//! itself with the UAC prompt when started without.

/// The process runs with an elevated token.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: the token handle is closed after use; TOKEN_ELEVATION is the size GetTokenInformation is told.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// Starts this exe again with the UAC prompt ("runas"); false when the user said no or it couldn't start.
#[cfg(windows)]
pub fn relaunch_elevated(args: &[String]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let Ok(exe) = std::env::current_exe() else { return false };
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let exe = wide(exe.as_os_str());
    // Each argument quoted, quotes inside doubled away - the exe is ours, the arguments came from our own command line.
    let joined = args.iter().map(|a| format!("\"{}\"", a.replace('"', ""))).collect::<Vec<_>>().join(" ");
    let parameters = wide(joined.as_ref());
    let verb = wide("runas".as_ref());
    // SAFETY: all strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(exe.as_ptr()),
            PCWSTR(parameters.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value above 32 on success.
    result.0 as usize > 32
}

#[cfg(not(windows))]
pub fn relaunch_elevated(_args: &[String]) -> bool {
    false
}
