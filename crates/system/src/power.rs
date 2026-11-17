//! Task Manager's "Efficiency mode" for this process (`Processes/PowerThrottling.cs`): EcoQoS and the Idle priority
//! class - both, as Task Manager sets them, so it shows the process with its leaf. Both make CPU-bound work slower, so
//! they are only for while nobody waits for the app.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, GetLastError};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetPriorityClass, IDLE_PRIORITY_CLASS, PROCESS_CREATION_FLAGS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
    SetPriorityClass, SetProcessInformation,
};

// The priority class before efficiency mode lowered it, to go back to.
static PRIORITY_BEFORE: Mutex<Option<u32>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The process is in efficiency mode now - a time measured meanwhile can be longer than what it measures.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Enters efficiency mode: EcoQoS, timer-resolution requests ignored, and the Idle priority class. False when Windows
/// refused EcoQoS (older than Windows 10 1709); the priority is left alone then.
pub fn try_enter() -> bool {
    // Both in one call: each call replaces the whole setting. A Windows older than 11 doesn't know the timer flag and
    // refuses the call (ERROR_INVALID_PARAMETER) - then without it.
    let both = PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
    // SAFETY: reads this thread's last error right after the call.
    let eco = set(both)
        || (unsafe { GetLastError() } == ERROR_INVALID_PARAMETER && set(PROCESS_POWER_THROTTLING_EXECUTION_SPEED));
    if eco {
        set_idle_priority();
    }
    ACTIVE.store(eco, Ordering::Relaxed);
    eco
}

/// Leaves efficiency mode: the priority class it had before, and the speed left to Windows again. False when Windows
/// refused.
pub fn reset() -> bool {
    restore_priority();
    ACTIVE.store(false, Ordering::Relaxed);
    set(0)
}

fn set_idle_priority() {
    // SAFETY: the current process's pseudo handle.
    unsafe {
        let current = GetPriorityClass(GetCurrentProcess());
        if current == 0 || current == IDLE_PRIORITY_CLASS.0 {
            return;
        }
        // EcoQoS alone still saves most of it when this fails.
        if SetPriorityClass(GetCurrentProcess(), IDLE_PRIORITY_CLASS).is_ok() {
            *PRIORITY_BEFORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(current);
        }
    }
}

fn restore_priority() {
    let mut before = PRIORITY_BEFORE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(class) = *before {
        // SAFETY: the current process's pseudo handle. Tried again with the next reset when it fails.
        if unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_CREATION_FLAGS(class)) }.is_ok() {
            *before = None;
        }
    }
}

fn set(flags: u32) -> bool {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: flags,
        StateMask: flags,
    };
    // SAFETY: the structure and its size, for the current process.
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Changes only this test process's own priority and QoS, and puts them back.
    #[test]
    fn enters_and_leaves_efficiency_mode() {
        // SAFETY: the current process's pseudo handle.
        let priority = || unsafe { GetPriorityClass(GetCurrentProcess()) };
        let before = priority();
        assert!(try_enter());
        assert!(is_active());
        assert_eq!(priority(), IDLE_PRIORITY_CLASS.0);
        assert!(reset());
        assert!(!is_active());
        assert_eq!(priority(), before);
    }
}
