//! Whether a debugger is attached to a site's worker process - the debugger part of `KeepWarmRequester.cs`. A request
//! would stop at its breakpoints, in the middle of someone's debugging session.

#[cfg(windows)]
pub(crate) use imp::attached;

/// Not Windows: no worker processes to debug.
#[cfg(not(windows))]
pub(crate) fn attached(_process_ids: &[u32]) -> bool {
    false
}

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessHandleInformation};
    use windows::Win32::Foundation::{CloseHandle, DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::CheckRemoteDebuggerPresent;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessId, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_DUP_HANDLE,
        PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_WRITE,
    };
    use windows::core::BOOL;

    // The programs that debug a site's worker process: Visual Studio (itself, or its remote debugger), the .NET
    // debugger of VS Code and others, Rider's, dnSpy.
    const DEBUGGER_PROGRAMS: [&str; 6] = ["devenv", "msvsmon", "vsdbg", "vsdbg-ui", "dnSpy", "dnSpy-x86"];
    const RIDER_DEBUGGER_PREFIX: &str = "JetBrains.Debugger.Worker";
    const DEBUGGERS_FRESH_FOR: Duration = Duration::from_secs(5);
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;

    static DEBUGGERS: Mutex<Option<(Instant, Vec<u32>)>> = Mutex::new(None);
    static PROCESS_TYPE: OnceLock<u32> = OnceLock::new();

    /// A handle closed when dropped.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: a handle this module opened and owns.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn open(access: PROCESS_ACCESS_RIGHTS, id: u32) -> Option<Owned> {
        // SAFETY: opening a process by id; the handle is owned and closed by `Owned`.
        unsafe { OpenProcess(access, false, id).ok().map(Owned) }
    }

    /// Whether a debugger is attached to one of `process_ids`: a native one (Windows says so), or a managed one -
    /// Visual Studio's usual attach to w3wp, which Windows doesn't report - recognised by a debugger program that has
    /// the process open with the right to write its memory.
    pub(crate) fn attached(process_ids: &[u32]) -> bool {
        if process_ids.is_empty() {
            return false;
        }
        for &id in process_ids {
            let Some(process) = open(PROCESS_QUERY_INFORMATION, id) else { continue };
            let mut present = BOOL(0);
            // SAFETY: a valid process handle and an out-parameter.
            if unsafe { CheckRemoteDebuggerPresent(process.0, &mut present) }.is_ok() && present.as_bool() {
                return true;
            }
        }
        running_debuggers().into_iter().any(|debugger| holds_for_writing(debugger, process_ids))
    }

    /// The debugger programs running now (their process IDs) - looked for again once the last look is 5 s old.
    pub(crate) fn running_debuggers() -> Vec<u32> {
        if let Ok(guard) = DEBUGGERS.lock()
            && let Some((at, found)) = guard.as_ref()
            && at.elapsed() < DEBUGGERS_FRESH_FOR
        {
            return found.clone();
        }
        let mut found = Vec::new();
        // SAFETY: a process snapshot read with a correctly sized entry; the snapshot handle is closed by `Owned`.
        unsafe {
            if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                let snapshot = Owned(snapshot);
                let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
                let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
                while more {
                    let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                    let file = String::from_utf16_lossy(&entry.szExeFile[..len]);
                    let name = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(&file);
                    if DEBUGGER_PROGRAMS.iter().any(|d| d.eq_ignore_ascii_case(name))
                        || name.to_ascii_lowercase().starts_with(&RIDER_DEBUGGER_PREFIX.to_ascii_lowercase())
                    {
                        found.push(entry.th32ProcessID);
                    }
                    more = Process32NextW(snapshot.0, &mut entry).is_ok();
                }
            }
        }
        if let Ok(mut guard) = DEBUGGERS.lock() {
            *guard = Some((Instant::now(), found.clone()));
        }
        found
    }

    /// Whether process `holder_id` has one of `process_ids` open with the right to write its memory - what a debugger
    /// does with the process it debugs. False when that can't be told.
    pub(crate) fn holds_for_writing(holder_id: u32, process_ids: &[u32]) -> bool {
        let Some(process_type) = process_type_index() else { return false };
        let Some(holder) = open(PROCESS_QUERY_INFORMATION | PROCESS_DUP_HANDLE, holder_id) else { return false };
        for (handle, access, kind) in handles_of(holder.0) {
            if kind != process_type || access & PROCESS_VM_WRITE.0 == 0 {
                continue;
            }
            let mut copy = HANDLE::default();
            // SAFETY: a copy of the holder's handle into this process, only to ask which process it is; closed below.
            unsafe {
                if DuplicateHandle(
                    holder.0,
                    handle,
                    GetCurrentProcess(),
                    &mut copy,
                    PROCESS_QUERY_LIMITED_INFORMATION.0,
                    false,
                    DUPLICATE_HANDLE_OPTIONS(0),
                )
                .is_err()
                {
                    continue;
                }
                let copy = Owned(copy);
                if process_ids.contains(&GetProcessId(copy.0)) {
                    return true;
                }
            }
        }
        false
    }

    // The kernel's number for "a process" among the kinds of handles - read from a handle of our own.
    fn process_type_index() -> Option<u32> {
        if let Some(known) = PROCESS_TYPE.get() {
            return Some(*known);
        }
        // SAFETY: our own process id and pseudo handle.
        let own = open(PROCESS_QUERY_LIMITED_INFORMATION, unsafe { GetCurrentProcessId() })?;
        let found =
            handles_of(unsafe { GetCurrentProcess() }).into_iter().find(|(h, _, _)| *h == own.0).map(|(_, _, t)| t)?;
        Some(*PROCESS_TYPE.get_or_init(|| found))
    }

    /// The handles `process` has open: each one's value, granted access and kind (PROCESS_HANDLE_SNAPSHOT_INFORMATION).
    /// Empty when Windows won't tell.
    fn handles_of(process: HANDLE) -> Vec<(HANDLE, u32, u32)> {
        // NumberOfHandles and Reserved (pointer-sized), then the entries: HandleValue, HandleCount and PointerCount
        // (pointer-sized), GrantedAccess, ObjectTypeIndex, HandleAttributes and Reserved (four bytes each).
        let pointer = size_of::<usize>();
        let header = 2 * pointer;
        let entry_size = 3 * pointer + 16;
        let mut size: usize = 64 * 1024;
        for _ in 0..6 {
            // usize-aligned for the reads below.
            let mut buffer = vec![0usize; size.div_ceil(pointer)];
            let mut needed = 0u32;
            // SAFETY: the buffer is `size` bytes long at least.
            let status = unsafe {
                NtQueryInformationProcess(
                    process,
                    ProcessHandleInformation,
                    buffer.as_mut_ptr().cast(),
                    size as u32,
                    &mut needed,
                )
            };
            if status.0 == STATUS_INFO_LENGTH_MISMATCH || status.0 == STATUS_BUFFER_TOO_SMALL {
                size = (size * 2).max(needed as usize + 64 * 1024);
                continue;
            }
            if status.0 < 0 {
                return Vec::new();
            }
            let bytes = buffer.as_ptr().cast::<u8>();
            let count = buffer[0];
            let mut handles = Vec::with_capacity(count.min(65_536));
            let mut i = 0;
            while i < count && header + (i + 1) * entry_size <= size {
                let at = header + i * entry_size;
                // SAFETY: within the buffer (checked above); unaligned reads of plain integers.
                unsafe {
                    let value = bytes.add(at).cast::<usize>().read_unaligned();
                    let access = bytes.add(at + 3 * pointer).cast::<u32>().read_unaligned();
                    let kind = bytes.add(at + 3 * pointer + 4).cast::<u32>().read_unaligned();
                    handles.push((HANDLE(value as *mut _), access, kind));
                }
                i += 1;
            }
            return handles;
        }
        Vec::new()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::{Command, Stdio};

        #[test]
        fn sees_a_process_held_open_for_writing() {
            // This test process is the "debugger": starting a process leaves it a handle with every right to it.
            let ping = |n: &str| {
                Command::new("ping.exe")
                    .args(["-n", n, "127.0.0.1"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("ping starts")
            };
            let mut held = ping("10");
            let unheld = ping("3");
            let unheld_id = unheld.id();
            // Its handle is closed; the process runs on and ends by itself.
            drop(unheld);
            let own = std::process::id();
            let holds = holds_for_writing(own, &[held.id()]);
            let holds_unheld = holds_for_writing(own, &[unheld_id]);
            let attached_now = attached(&[held.id()]);
            let _ = held.kill();
            let _ = held.wait();
            assert!(holds);
            assert!(!holds_unheld, "Its handle was closed.");
            assert!(!attached_now, "Nothing debugs it - this process isn't a debugger program.");
        }

        #[test]
        fn debugger_programs_are_looked_for_from_a_fresh_list() {
            // Whatever runs on this PC: a list, read again after a while - and this test isn't one of them.
            assert!(!running_debuggers().contains(&std::process::id()));
        }
    }
}
