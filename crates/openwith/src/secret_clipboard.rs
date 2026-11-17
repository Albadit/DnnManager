//! Ports `Presentation/Services/SecretClipboard.cs`: a password on the clipboard, for the moment it is pasted - kept out
//! of Windows' clipboard history (Win+V) and its cloud clipboard, which would sync it to the user's other devices and
//! Microsoft account, and marked for clipboard managers to leave alone. Taken off the clipboard again after
//! [`CLEAR_AFTER`], or when DNN Manager quits before that ([`clear_if_still_ours`]) - unless something else has been
//! copied since.
//!
//! Deviation: WPF's `DispatcherTimer` becomes a thread that sleeps [`CLEAR_AFTER`]; the clipboard is opened with a
//! message-only window of this crate's own, since there is no WPF window to own it.

use std::sync::Mutex;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{HSTRING, w};

/// How long the password stays on the clipboard.
pub const CLEAR_AFTER: Duration = Duration::from_secs(60);

const CF_UNICODETEXT: u32 = 13;

// The password put on the clipboard last, until it is taken off again - what quitting takes off.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// Puts `secret` on the clipboard as described; false when the clipboard was busy.
pub fn set(secret: &str) -> bool {
    if !put(secret) {
        return false;
    }
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(secret.to_string());
    let secret = secret.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(CLEAR_AFTER);
        clear_if(&secret);
    });
    true
}

/// Takes the password put on the clipboard last off it, when it is still there - as DNN Manager quits: the copy on the
/// clipboard outlives it (it was put there to stay), and the thread that would take it off doesn't.
pub fn clear_if_still_ours() {
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(secret) = pending {
        clear_if(&secret);
    }
}

fn clear_if(secret: &str) {
    {
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if pending.as_deref() == Some(secret) {
            *pending = None;
        }
    }
    // Another program has it open: it stays.
    let Some(_open) = Open::new(None) else { return };
    if text_on_clipboard().as_deref() == Some(secret) {
        // SAFETY: the clipboard is open on this thread.
        let _ = unsafe { EmptyClipboard() };
    }
}

// The clipboard, open on this thread until dropped. Tried a few times: another program may have it for a moment (as
// WPF's Clipboard retries).
struct Open;

impl Open {
    fn new(owner: Option<HWND>) -> Option<Self> {
        for attempt in 0..10 {
            // SAFETY: closed again on drop.
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Open);
            }
            if attempt < 9 {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: opened by this thread in `Open::new`.
        let _ = unsafe { CloseClipboard() };
    }
}

// The clipboard's text, read while it is open.
fn text_on_clipboard() -> Option<String> {
    // SAFETY: the clipboard is open; the global memory is locked while it is read and its size bounds the read.
    unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT).ok()?;
        let data = GetClipboardData(CF_UNICODETEXT).ok()?;
        let global = HGLOBAL(data.0);
        let pointer = GlobalLock(global) as *const u16;
        if pointer.is_null() {
            return None;
        }
        let units = GlobalSize(global) / 2;
        let all = std::slice::from_raw_parts(pointer, units);
        let length = all.iter().position(|&c| c == 0).unwrap_or(units);
        let text = String::from_utf16_lossy(&all[..length]);
        let _ = GlobalUnlock(global);
        Some(text)
    }
}

fn put(secret: &str) -> bool {
    // A window of our own owns what is put there: an unowned clipboard refuses data (SetClipboardData).
    // SAFETY: a message-only window of the system's STATIC class, destroyed below.
    let Ok(window) = (unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }) else {
        return false;
    };
    let done = Open::new(Some(window)).is_some_and(|_open| {
        // SAFETY: the clipboard is open on this thread and owned by `window`.
        unsafe { EmptyClipboard() }.is_ok() && put_text(secret) && put_markers()
    });
    // SAFETY: made above on this thread. The clipboard keeps what was put on it (none of it is rendered on request).
    let _ = unsafe { DestroyWindow(window) };
    done
}

fn put_text(secret: &str) -> bool {
    let mut units: Vec<u16> = secret.encode_utf16().collect();
    units.push(0);
    put_bytes(CF_UNICODETEXT, units.as_ptr().cast(), units.len() * 2)
}

// Windows' own formats for "don't keep this": a DWORD 0 for the history and the cloud, and the mere presence of the
// monitor format for clipboard managers.
fn put_markers() -> bool {
    let zero = 0u32.to_le_bytes();
    ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard", "ExcludeClipboardContentFromMonitorProcessing"]
        .into_iter()
        .all(|name| {
            // SAFETY: a registered name; 0 when it can't be.
            let format = unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) };
            format != 0 && put_bytes(format, zero.as_ptr(), zero.len())
        })
}

fn put_bytes(format: u32, bytes: *const u8, length: usize) -> bool {
    // SAFETY: `length` bytes are copied into memory of that size; once SetClipboardData takes it, the clipboard owns it,
    // otherwise it is freed here.
    unsafe {
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, length) else { return false };
        let target = GlobalLock(global) as *mut u8;
        if target.is_null() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes, target, length);
        let _ = GlobalUnlock(global);
        if SetClipboardData(format, Some(HANDLE(global.0))).is_err() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Changes the user's clipboard: not in a normal test run.
    #[test]
    #[ignore = "puts a value on the user's clipboard"]
    fn the_secret_is_put_on_and_taken_off_while_it_is_still_ours() {
        assert!(set("Not-a-real-secret-1"));
        {
            let _open = Open::new(None).unwrap();
            assert_eq!(text_on_clipboard().as_deref(), Some("Not-a-real-secret-1"));
        }
        clear_if_still_ours();
        let _open = Open::new(None).unwrap();
        assert_eq!(text_on_clipboard(), None);
    }
}
