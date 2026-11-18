//! Windows' Open dialog for one kind of file. GPUI 0.2.2's `prompt_for_paths` shows every file (it sets no file
//! types - gpui src/platform/windows/platform.rs:902-945), so a "Site .zip" Browse offered the .bacpac, the .log and
//! the rest of the folder too. This one lists only the files of the type asked for.

use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::core::{HSTRING, PCWSTR};

/// The kind of file an Open dialog lists: its name in the dialog's type box ("Site archive") and its patterns
/// ("*.zip", several as "*.json;*.txt").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileKind {
    pub name: String,
    pub patterns: String,
}

/// Shows the Open dialog over `owner` (a window's HWND, 0 for none) titled `title`, listing only the files of `kind`,
/// and returns the file chosen - None when cancelled. It runs on a thread of its own in a single-threaded COM
/// apartment, as the shell's dialogs need; the caller waits, so call it from a background task, not the UI thread.
pub fn open_file(owner: isize, title: &str, kind: &FileKind) -> Result<Option<PathBuf>, String> {
    let (title, name, patterns) = (HSTRING::from(title), HSTRING::from(kind.name.as_str()), HSTRING::from(kind.patterns.as_str()));
    std::thread::spawn(move || {
        // SAFETY: initialised and uninitialised on this thread, which no one else uses.
        let initialised = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let result = show(owner, &title, &name, &patterns);
        if initialised {
            // SAFETY: matches the CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        result
    })
    .join()
    .map_err(|_| "The Open dialog stopped unexpectedly.".to_string())?
    .map_err(|e| format!("The Open dialog could not be shown: {}", e.message()))
}

fn show(owner: isize, title: &HSTRING, name: &HSTRING, patterns: &HSTRING) -> windows::core::Result<Option<PathBuf>> {
    // SAFETY: COM is initialised on this thread (open_file); the HSTRINGs outlive every call that borrows them, and the
    // path the dialog returns is copied, then freed with CoTaskMemFree as GetDisplayName requires.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)?;
        dialog.SetTitle(title)?;
        dialog.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(patterns.as_ptr()) }])?;
        dialog.SetFileTypeIndex(1)?;
        let owner = (owner != 0).then_some(HWND(owner as *mut _));
        // An error here is Cancel (ERROR_CANCELLED) - or a dialog that couldn't show, which reads the same to the user.
        if dialog.Show(owner).is_err() {
            return Ok(None);
        }
        let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        Ok(text.map(PathBuf::from))
    }
}
