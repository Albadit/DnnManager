//! Ports `Presentation/Unelevated.cs`: starts a program as the signed-in user - not with DNN Manager's administrator
//! rights: handed to the desktop's shell (Explorer, which runs as the user), as Windows' own "run as the user" does. For
//! programs DNN Manager opens for the user (an editor), which have no need of its rights - and which, installed in the
//! user's own folders, any program of theirs could have changed.
//!
//! Unlike the terminal's restricted token (`dnnm-terminal`'s `token.rs`), which keeps DNN Manager's sign-in session and
//! profile, this is the user's own everyday Explorer starting the program - as the C# does.

use std::path::Path;

use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, IDispatch, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows, SID_STopLevelBrowser, SVGIO_BACKGROUND,
    SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
};
use windows::core::{BSTR, Interface};

/// Starts `exe` with `arguments` (quoted where needed) in `working_directory`. False when the shell isn't there to start
/// it (no Explorer). Runs its COM calls on a thread of its own; it returns once the shell has the request.
pub fn start_unelevated(exe: &Path, arguments: &[&str], working_directory: Option<&Path>) -> bool {
    let exe = exe.display().to_string();
    let arguments = arguments.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
    let directory = working_directory.map(|d| d.display().to_string()).unwrap_or_default();
    crate::win::on_com_thread(COINIT_APARTMENTTHREADED, move || {
        let Some(shell) = desktop_shell() else { return false };
        // SAFETY: COM calls on this thread's apartment with values that outlive them.
        unsafe {
            shell
                .ShellExecute(
                    &BSTR::from(exe.as_str()),
                    &VARIANT::from(arguments.as_str()),
                    &VARIANT::from(directory.as_str()),
                    &VARIANT::from("open"),
                    &VARIANT::from(1i32), // SW_SHOWNORMAL
                )
                .is_ok()
        }
    })
    .unwrap_or(false)
}

/// The desktop's Shell.Application (IShellDispatch2), from the shell window of the desktop - Explorer's.
fn desktop_shell() -> Option<IShellDispatch2> {
    // SAFETY: COM calls on the caller's apartment; every interface is reference counted and released on drop.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        let location = VARIANT::from(0i32); // CSIDL_DESKTOP
        let root = VARIANT::default();
        let mut hwnd = 0i32;
        let desktop: IDispatch =
            windows.FindWindowSW(&location, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH).ok()?;
        let provider: IServiceProvider = desktop.cast().ok()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser).ok()?;
        let view = browser.QueryActiveShellView().ok()?;
        let folder_view: IShellFolderViewDual = view.GetItemObject(SVGIO_BACKGROUND).ok()?;
        folder_view.Application().ok()?.cast().ok()
    }
}

/// A command line argument as the program reads it back (CommandLineToArgvW's rules): in quotes when it has a space or
/// a quote, with backslashes before a quote - or before the closing one - doubled.
pub fn quote(argument: &str) -> String {
    if !argument.is_empty() && !argument.chars().any(|c| c.is_whitespace() || c == '"') {
        return argument.to_string();
    }
    let mut text = String::from("\"");
    let mut slashes = 0;
    for c in argument.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            text.push_str(&"\\".repeat(slashes * 2 + 1));
        } else {
            text.push_str(&"\\".repeat(slashes));
        }
        text.push(c);
        slashes = 0;
    }
    text.push_str(&"\\".repeat(slashes * 2));
    text.push('"');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arguments_stay_as_they_are() {
        assert_eq!(quote(r"C:\Sites\shop"), r"C:\Sites\shop");
        assert_eq!(quote("-nosplash"), "-nosplash");
    }

    #[test]
    fn arguments_with_spaces_or_quotes_are_quoted_as_windows_reads_them_back() {
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote(r"C:\My Sites\shop"), r#""C:\My Sites\shop""#);
        // A trailing backslash would escape the closing quote: doubled.
        assert_eq!(quote(r"C:\My Sites\"), r#""C:\My Sites\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r#"a\"b c"#), r#""a\\\"b c""#);
    }

    // Starts Notepad through Explorer: a program on the desktop, so not in a normal test run.
    #[test]
    #[ignore = "starts Notepad on the desktop"]
    fn starts_a_program_through_the_desktop_shell() {
        assert!(start_unelevated(Path::new(r"C:\Windows\System32\notepad.exe"), &[], None));
    }
}
