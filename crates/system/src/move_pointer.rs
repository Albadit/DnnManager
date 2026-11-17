//! Windows' Move pointer (the four-way arrow, `IDC_SIZEALL`) where GPUI has none: its Windows cursors stop at the
//! arrow, the I-beam, the hand, ↔, ↕ and the crosshair. A subclass in front of GPUI's window procedure answers
//! `WM_SETCURSOR` with the Move pointer while it is on - over a sash corner, or while one is dragged - and leaves every
//! other message, and that one the rest of the time, to GPUI.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, IDC_SIZEALL, LoadCursorW, PostMessageW, SetCursor, WM_MOUSEMOVE, WM_SETCURSOR,
};

/// The Move pointer is wanted now.
static ON: AtomicBool = AtomicBool::new(false);
/// The window it is on.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// This subclass's id on the window.
const SUBCLASS_ID: usize = 0x444E_4D4D; // "DNMM"

/// Puts the subclass on the window (`hwnd`) - once, at the start.
pub fn install(hwnd: isize) {
    WINDOW.store(hwnd, Ordering::Relaxed);
    // SAFETY: the window is the app's own, alive while the subclass is; the procedure is a plain function.
    let _ = unsafe { SetWindowSubclass(HWND(hwnd as *mut _), Some(subclass), SUBCLASS_ID, 0) };
}

/// The Move pointer on or off - shown at once either way: off, Windows is asked for the pointer again, which GPUI
/// answers with its own (the mouse has already moved on, so without that the Move pointer would stay one move longer).
pub fn set(on: bool) {
    if ON.swap(on, Ordering::Relaxed) == on {
        return;
    }
    if on {
        show();
    } else {
        let hwnd = WINDOW.load(Ordering::Relaxed);
        if hwnd != 0 {
            let lparam = (HTCLIENT as isize) | ((WM_MOUSEMOVE as isize) << 16);
            // SAFETY: the app's own window; a WM_SETCURSOR as Windows would send it.
            let _ = unsafe {
                PostMessageW(Some(HWND(hwnd as *mut _)), WM_SETCURSOR, WPARAM(hwnd as usize), LPARAM(lparam))
            };
        }
    }
}

fn show() {
    // SAFETY: a system cursor, loaded shared; SetCursor takes any cursor handle.
    unsafe {
        if let Ok(cursor) = LoadCursorW(None, IDC_SIZEALL) {
            SetCursor(Some(cursor));
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // Over the window's own area only: its frame's resize pointers stay Windows'.
    if message == WM_SETCURSOR && ON.load(Ordering::Relaxed) && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
        show();
        return LRESULT(1);
    }
    // SAFETY: the message as it came, to the next procedure (GPUI's).
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
