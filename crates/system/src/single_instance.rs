//! DNN Manager runs once per Windows session (`Presentation/SingleInstance.cs`, `Presentation/RunningMarker.cs`). A
//! second start finds the running marker mutex, asks the running DNN Manager to show its window through a named event
//! and exits. Setup and the uninstaller ask it to quit through another event before they replace or remove its files -
//! they can't close it themselves, as it runs elevated. The names are the WPF app's: Setup knows them.

use std::ffi::c_void;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForMultipleObjects,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::PCWSTR;

use crate::acl::current_user_sid;
use crate::win::{Handle, wide};

pub const ACTIVATE_EVENT_NAME: &str = "DnnManager.NET.Activate";
/// Keep in step with `QuitEvent` in `src\DnnManager.Installer\DnnManager.iss`.
pub const QUIT_EVENT_NAME: &str = "DnnManager.NET.Quit";
/// Keep in step with `RunningMutex` in `src\DnnManager.Installer\DnnManager.iss`.
pub const RUNNING_MARKER_NAME: &str = "DnnManager.NET.Running";

/// The running marker; keep it alive until the app exits - Windows removes it with the last handle.
pub struct RunningMarker {
    _mutex: Handle,
}

/// Creates the running marker. The flag is true when another DNN Manager made it first. None when it couldn't be
/// created (Setup then just can't tell).
pub fn create_running_marker() -> Option<(RunningMarker, bool)> {
    create_mutex(RUNNING_MARKER_NAME)
}

fn create_mutex(name: &str) -> Option<(RunningMarker, bool)> {
    let name = wide(name);
    // The app runs elevated but Setup doesn't, so everyone may wait on (i.e. see) the mutex: SYNCHRONIZE.
    with_attributes("0x100000", |attributes| {
        // SAFETY: a NUL-terminated name; the attributes live for the call.
        unsafe {
            let handle = CreateMutexW(Some(attributes), false, PCWSTR(name.as_ptr())).ok()?;
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            Some((RunningMarker { _mutex: Handle(handle) }, existed))
        }
    })
}

/// Security attributes letting everyone `everyone_rights` and the signed-in user everything.
fn with_attributes<T>(everyone_rights: &str, use_them: impl FnOnce(&SECURITY_ATTRIBUTES) -> Option<T>) -> Option<T> {
    let me = current_user_sid()?;
    let sddl = wide(&format!("D:(A;;{everyone_rights};;;WD)(A;;GA;;;{me})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: Windows allocates the descriptor; it is freed after use.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .ok()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let result = use_them(&attributes);
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}

/// Whether DNN Manager already runs in this session.
pub fn is_running() -> bool {
    mutex_exists(RUNNING_MARKER_NAME)
}

fn mutex_exists(name: &str) -> bool {
    let name = wide(name);
    // SAFETY: a NUL-terminated name; the handle is closed by `Handle`.
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) } {
        Ok(handle) => {
            drop(Handle(handle));
            true
        }
        // It exists, this start just may not open it.
        Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    }
}

/// True when DNN Manager is already running in this session; it has been asked to show its window, and this start
/// should exit.
pub fn hand_off_to_running() -> bool {
    if !is_running() {
        return false;
    }
    // Still starting up (no event yet), or not allowed: its window shows up by itself.
    signal(ACTIVATE_EVENT_NAME);
    true
}

/// Sets the named event; false when there is none or it may not be set.
fn signal(name: &str) -> bool {
    let name = wide(name);
    // SAFETY: a NUL-terminated name; the handle is closed by `Handle`.
    unsafe {
        let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())) else { return false };
        let event = Handle(event);
        // This start has the foreground (it was just launched) - pass it on, or Windows would only flash the running
        // app's taskbar button.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        SetEvent(event.0).is_ok()
    }
}

/// Listens for later starts and for Setup asking to quit; drop it to stop listening.
pub struct Listener {
    stop: Handle,
    threads: Vec<JoinHandle<()>>,
    // Closed after the threads that wait on them have ended (fields drop after `drop`).
    _events: Vec<Handle>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        // SAFETY: the stop event is ours and open until after the threads are joined.
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

type Action = Box<dyn Fn() + Send>;

/// Runs `on_activate` each time a later start hands off (bring the window to the front) and `on_quit` when Setup asks
/// DNN Manager to quit - on threads of their own: the caller passes them on to the UI thread. An event that can't be
/// made isn't listened to (a second start then just exits; Setup asks to quit DNN Manager by hand).
pub fn listen(on_activate: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) -> Option<Listener> {
    listen_on(ACTIVATE_EVENT_NAME, QUIT_EVENT_NAME, Box::new(on_activate), Box::new(on_quit))
}

fn listen_on(activate: &str, quit: &str, on_activate: Action, on_quit: Action) -> Option<Listener> {
    // SAFETY: an unnamed manual-reset event, closed by `Handle`.
    let stop = Handle(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.ok()?);
    let mut threads = Vec::new();
    let mut events = Vec::new();
    for (name, action) in [(activate, on_activate), (quit, on_quit)] {
        let Some(event) = auto_reset_event(name) else { continue };
        let (event_raw, stop_raw) = (event.0.0 as usize, stop.0.0 as usize);
        events.push(event);
        threads.push(std::thread::spawn(move || {
            let handles = [HANDLE(event_raw as *mut c_void), HANDLE(stop_raw as *mut c_void)];
            loop {
                // SAFETY: both handles stay open until this thread has ended (the listener joins it first).
                let woke = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                if woke != WAIT_OBJECT_0 {
                    break;
                }
                action();
            }
        }));
    }
    Some(Listener { stop, threads, _events: events })
}

fn auto_reset_event(name: &str) -> Option<Handle> {
    let name = wide(name);
    // EVENT_MODIFY_STATE | SYNCHRONIZE for everyone: the running app is elevated, a later start may not be.
    with_attributes("0x100002", |attributes| {
        // SAFETY: a NUL-terminated name; the attributes live for the call.
        unsafe { CreateEventW(Some(attributes), false, false, PCWSTR(name.as_ptr())) }.ok().map(Handle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn a_second_marker_knows_the_first() {
        let name = format!("DnnManager.NET.Tests.Running.{}", std::process::id());
        assert!(!mutex_exists(&name));
        let (first, existed) = create_mutex(&name).unwrap();
        assert!(!existed);
        assert!(mutex_exists(&name));
        let (_second, existed) = create_mutex(&name).unwrap();
        assert!(existed);
        drop(first);
    }

    #[test]
    fn a_signal_runs_the_listeners_action() {
        let id = std::process::id();
        let (activate, quit) =
            (format!("DnnManager.NET.Tests.Activate.{id}"), format!("DnnManager.NET.Tests.Quit.{id}"));
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let on_activate: Action = Box::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let listener = listen_on(&activate, &quit, on_activate, Box::new(|| {})).unwrap();
        assert!(signal(&activate));
        let until = Instant::now() + Duration::from_secs(5);
        while count.load(Ordering::SeqCst) == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        drop(listener);
        assert!(!signal(&format!("DnnManager.NET.Tests.None.{id}")));
    }
}
