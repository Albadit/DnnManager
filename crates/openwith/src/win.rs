//! Small Windows helpers of this crate.

use std::path::PathBuf;

use windows::Win32::System::Com::{COINIT, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};

/// A special folder's path (`Environment.GetFolderPath`).
pub fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: the returned string is copied out, then freed with CoTaskMemFree as the call requires.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

/// Runs `work` on a thread of its own in the COM apartment `apartment` - the caller's thread (GPUI's background
/// executor) may be in another apartment or none, and isn't ours to change.
pub fn on_com_thread<T: Send + 'static>(apartment: COINIT, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    std::thread::spawn(move || {
        // SAFETY: initialised and uninitialised on this thread, which no one else uses.
        let initialised = unsafe { CoInitializeEx(None, apartment) }.is_ok();
        let result = work();
        if initialised {
            // SAFETY: matches the CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        result
    })
    .join()
    .ok()
}
