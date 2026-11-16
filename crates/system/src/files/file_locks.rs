//! What keeps a folder from being deleted (`Files/FileLockService.cs`), found two ways:
//! - the Restart Manager (what installers use to find "files in use") for processes with files open, and
//! - each process's current directory, read from its PEB - a terminal `cd`'d into the folder holds no file open, yet
//!   Windows won't delete a directory that is some process's working folder.
//!
//! The lockers are closed only on the user's consent ([`close`]); what can't be closed can be deleted at the next
//! restart ([`schedule_delete_on_restart`]).

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::os::windows::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows::Win32::Foundation::{ERROR_MORE_DATA, FILETIME, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmCritical, RmEndSession, RmGetList, RmRegisterResources, RmService,
    RmStartSession,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, IsWow64Process, OpenProcess, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, PROCESS_VM_READ, TerminateProcess,
    WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use super::safe_path::{self, LINK_HINT};
use crate::acl::{Ace, Security, sids};
use crate::trusted::is_reparse_point;
use crate::win::{Handle, ProcessEntry, exe_path, processes, version_string, wide_path};

/// A process that keeps a folder from being deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockingProcess {
    pub id: u32,
    /// The app's name: Restart Manager's, its exe's description, or its process name.
    pub name: String,
    /// `Code.exe`.
    pub exe: String,
    /// "has files in the folder open", "its working folder is inside the project".
    pub reason: String,
    /// Not a service, not Windows itself, not this app, not session 0.
    pub can_close: bool,
}

// Never closed: Windows itself, the shell, security software and this app.
const PROTECTED: [&str; 23] = [
    "System",
    "Idle",
    "Registry",
    "smss",
    "csrss",
    "wininit",
    "winlogon",
    "services",
    "lsass",
    "svchost",
    "dwm",
    "fontdrvhost",
    "explorer",
    "sihost",
    "ctfmon",
    "MsMpEng",
    "NisSrv",
    "SearchIndexer",
    "SearchProtocolHost",
    "SearchFilterHost",
    "audiodg",
    "spoolsv",
    "conhost",
];

// Restart Manager takes the files to check; past this many, the first ones are enough to find the culprits.
const MAX_FILES_TO_CHECK: usize = 20_000;

/// The processes that keep `directory` from being deleted, by name.
pub fn find_lockers(directory: &Path) -> Vec<LockingProcess> {
    let root = safe_path::full(directory).unwrap_or_else(|| directory.to_path_buf());
    let list = processes();
    let mut found: BTreeMap<u32, LockingProcess> = BTreeMap::new();
    for (pid, app_name, is_service) in processes_with_files_open(&root) {
        add(&mut found, &list, pid, Some(app_name), "has files in the folder open", is_service);
    }
    for pid in processes_working_in(&root, &list) {
        if !found.contains_key(&pid) {
            add(&mut found, &list, pid, None, "its working folder is inside the project", false);
        }
    }
    let mut lockers: Vec<LockingProcess> = found.into_values().collect();
    lockers.sort_by_key(|p| p.name.to_lowercase());
    lockers
}

/// The exe name without `.exe` (`Process.ProcessName`).
fn process_name(exe: &str) -> String {
    exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(exe).to_string()
}

/// Records `pid` - as the app that owns it - with a readable name.
fn add(
    found: &mut BTreeMap<u32, LockingProcess>,
    list: &[ProcessEntry],
    pid: u32,
    app_name: Option<String>,
    reason: &str,
    is_service: bool,
) {
    let Some(process) = list.iter().find(|p| p.pid == pid) else { return }; // exited meanwhile
    // Report a helper (VS Code, Chrome, … run many processes of the same exe) as its topmost same-named ancestor:
    // closing that closes the app, closing a helper only restarts it.
    let owner = owner_of(process, list);
    if found.contains_key(&owner.pid) {
        return;
    }
    let name = process_name(&owner.exe);
    let session = session_of(owner.pid);
    let can_close = !is_service
        && owner.pid != std::process::id()
        && session.is_some_and(|s| s != 0)
        && !PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(&name));
    let display = match app_name {
        Some(app) if !app.is_empty() && owner.pid == process.pid => app,
        _ => friendly_name(owner.pid, &name),
    };
    found.insert(
        owner.pid,
        LockingProcess { id: owner.pid, name: display, exe: format!("{name}.exe"), reason: reason.into(), can_close },
    );
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: a value to fill.
    unsafe { ProcessIdToSessionId(pid, &mut session) }.ok().map(|()| session)
}

/// The topmost ancestor of `process` running the same exe (or the process itself).
fn owner_of<'a>(process: &'a ProcessEntry, list: &'a [ProcessEntry]) -> &'a ProcessEntry {
    let mut current = process;
    for _ in 0..16 {
        let Some(parent) = list.iter().find(|p| p.pid == current.parent && p.pid != current.pid) else { break };
        // A reused pid, not the real parent, started after its "child".
        if !parent.exe.eq_ignore_ascii_case(&current.exe) || start_time(parent.pid) > start_time(current.pid) {
            break;
        }
        current = parent;
    }
    current
}

fn start_time(pid: u32) -> u64 {
    // SAFETY: the handle is closed by `Handle`; the times are values to fill.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return 0 };
        let handle = Handle(handle);
        let (mut created, mut exited, mut kernel, mut user) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        if GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user).is_err() {
            return 0;
        }
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
    }
}

fn friendly_name(pid: u32, process_name: &str) -> String {
    exe_path(pid).and_then(|exe| version_string(&exe, "FileDescription")).unwrap_or_else(|| process_name.to_string())
}

/// Closes the lockers that may be closed - politely first (the app can save its state), then by force with its
/// children, whose handles are the lock too. Returns the ones still running.
pub fn close(lockers: &[LockingProcess]) -> Vec<LockingProcess> {
    let mut still_running = Vec::new();
    for locker in lockers.iter().filter(|p| p.can_close) {
        if close_main_window(locker.id) && exited(locker.id, Duration::from_secs(5)) {
            continue;
        }
        kill_tree(locker.id);
        if !exited(locker.id, Duration::from_secs(5)) {
            log::warn!("Could not close {} ({})", locker.name, locker.id);
            still_running.push(locker.clone());
        }
    }
    // Windows releases a closed process's handles a moment after it exits.
    std::thread::sleep(Duration::from_millis(500));
    still_running
}

/// Asks the process's main window (visible, without an owner) to close; false when it has none.
fn close_main_window(pid: u32) -> bool {
    struct Search {
        pid: u32,
        window: Option<HWND>,
    }
    unsafe extern "system" fn each(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Search` passed to EnumWindows, alive for the call.
        unsafe {
            let search = &mut *(data.0 as *mut Search);
            let mut owner_pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut owner_pid));
            if owner_pid == search.pid
                && IsWindowVisible(window).as_bool()
                && GetWindow(window, GW_OWNER).map_or(true, |o| o.is_invalid())
            {
                search.window = Some(window);
                return false.into();
            }
            true.into()
        }
    }
    let mut search = Search { pid, window: None };
    // SAFETY: the callback only reads windows and writes into `search`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize));
        match search.window {
            Some(window) => PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok(),
            None => false,
        }
    }
}

/// Whether the process has ended within `timeout` (also when it is already gone).
fn exited(pid: u32, timeout: Duration) -> bool {
    // SAFETY: the handle is closed by `Handle`.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
        let handle = Handle(handle);
        WaitForSingleObject(handle.0, timeout.as_millis() as u32) == WAIT_OBJECT_0
    }
}

/// Ends `pid` and every process it started, children first.
pub(crate) fn kill_tree(pid: u32) {
    let list = processes();
    let mut order = vec![pid];
    let mut i = 0;
    while i < order.len() {
        let parent = order[i];
        let children: Vec<u32> = list
            .iter()
            .filter(|p| p.parent == parent && p.pid != parent && !order.contains(&p.pid))
            .map(|p| p.pid)
            .collect();
        order.extend(children);
        i += 1;
    }
    for pid in order.into_iter().rev() {
        // SAFETY: the handle is closed by `Handle`.
        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let handle = Handle(handle);
                let _ = TerminateProcess(handle.0, 1);
            }
        }
    }
}

/// Ends every program running from an exe inside `directory` (a site's own tools) - returns their file names.
pub fn stop_programs_running_from(directory: &Path) -> Vec<String> {
    let mut stopped = Vec::new();
    for process in processes() {
        if process.pid == std::process::id() {
            continue;
        }
        let Some(exe) = exe_path(process.pid) else { continue };
        if !safe_path::is_inside(&exe, directory) {
            continue;
        }
        kill_tree(process.pid);
        exited(process.pid, Duration::from_secs(5));
        stopped.push(exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    }
    // Windows releases an ended process's handles a moment after it exits.
    if !stopped.is_empty() {
        std::thread::sleep(Duration::from_millis(500));
    }
    stopped
}

/// Has Windows delete `directory` with everything in it as it next starts. Each folder is first made Administrators'
/// and SYSTEM's only: Windows deletes by path with SYSTEM's rights, so nothing without administrator rights may put a
/// junction on the way in between. A link already in it is deleted itself, never followed.
pub fn schedule_delete_on_restart(directory: &Path) -> Result<(), String> {
    if !directory.is_dir() {
        return Ok(());
    }
    if safe_path::is_link(directory) {
        return Err(format!(
            "{} is a link - DNN Manager doesn't delete through those.\r\n{LINK_HINT}",
            directory.display()
        ));
    }
    let root = safe_path::full(directory).unwrap_or_else(|| directory.to_path_buf());
    let mut files = Vec::new();
    let mut folders = Vec::new();
    collect(&root, &mut files, &mut folders).map_err(|e| e.to_string())?;
    folders.sort_by_key(|f| std::cmp::Reverse(f.as_os_str().len()));
    folders.push(root);

    // Pending deletes run in order: every file first, then the folders deepest first so each is empty.
    let mut failed = 0;
    let mut last_error = 0;
    for path in files.iter().chain(folders.iter()) {
        let name = wide_path(path);
        // SAFETY: a NUL-terminated path; no new name means "delete".
        if let Err(e) = unsafe { MoveFileExW(PCWSTR(name.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT) } {
            failed += 1;
            last_error = e.code().0 & 0xFFFF;
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} item(s) couldn't be scheduled for deletion (error {last_error})."))
    }
}

fn administrators_only() -> Security {
    Security::protected(
        Some(sids::ADMINISTRATORS),
        vec![Ace::full_control(sids::ADMINISTRATORS), Ace::full_control(sids::SYSTEM)],
    )
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>, folders: &mut Vec<PathBuf>) -> std::io::Result<()> {
    administrators_only().apply(dir)?;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
        if meta.is_dir() || (is_reparse_point(&meta) && meta.file_type().is_symlink_dir()) {
            if !is_reparse_point(&meta) {
                collect(&entry.path(), files, folders)?;
            }
            folders.push(entry.path());
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

// ─── Restart Manager: processes with files open ──────────────────────────

fn processes_with_files_open(root: &Path) -> Vec<(u32, String, bool)> {
    // Only the files still there after the failed delete - those are the locked ones (and their neighbours).
    let files = files_under(root, MAX_FILES_TO_CHECK);
    if files.is_empty() {
        return Vec::new();
    }
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: the session is ended after use; every name passed lives until RmRegisterResources returns.
    unsafe {
        if RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())).0 != 0 {
            return Vec::new();
        }
        let result = (|| {
            for chunk in files.chunks(1000) {
                let names: Vec<Vec<u16>> = chunk.iter().map(|f| wide_path(f)).collect();
                let pointers: Vec<PCWSTR> = names.iter().map(|n| PCWSTR(n.as_ptr())).collect();
                if RmRegisterResources(session, Some(&pointers), None, None).0 != 0 {
                    return Vec::new();
                }
            }
            let (mut needed, mut count, mut reasons) = (0u32, 0u32, 0u32);
            let first = RmGetList(session, &mut needed, &mut count, None, &mut reasons);
            // Nobody has them open, or the list can't be read.
            if first.0 != ERROR_MORE_DATA.0 {
                return Vec::new();
            }
            let mut infos = vec![RM_PROCESS_INFO::default(); needed as usize];
            count = needed;
            if RmGetList(session, &mut needed, &mut count, Some(infos.as_mut_ptr()), &mut reasons).0 != 0 {
                return Vec::new();
            }
            infos
                .iter()
                .take(count as usize)
                .map(|i| {
                    let service = i.ApplicationType == RmService || i.ApplicationType == RmCritical;
                    (i.Process.dwProcessId, crate::win::from_wide(&i.strAppName), service)
                })
                .collect()
        })();
        let _ = RmEndSession(session);
        result
    }
}

fn files_under(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
            if is_reparse_point(&meta) {
                continue;
            }
            if meta.is_dir() {
                folders.push(entry.path());
            } else {
                files.push(entry.path());
                if files.len() >= limit {
                    return files;
                }
            }
        }
    }
    files
}

// ─── Current directories ─────────────────────────────────────────────────

fn processes_working_in(root: &Path, list: &[ProcessEntry]) -> Vec<u32> {
    list.iter()
        .filter(|p| current_directory_of(p.pid).is_some_and(|cwd| safe_path::is_same_or_inside(&cwd, root)))
        .map(|p| p.pid)
        .collect()
}

/// A 64-bit process's current directory: PEB → ProcessParameters → CurrentDirectory.DosPath. None for processes that
/// can't be opened (protected, other users) and for 32-bit ones, which keep it elsewhere.
fn current_directory_of(pid: u32) -> Option<PathBuf> {
    // SAFETY: the handle is closed by `Handle`; every read is of the size of the buffer it fills.
    unsafe {
        let handle = Handle(OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?);
        let mut wow64 = BOOL::default();
        if IsWow64Process(handle.0, &mut wow64).is_ok() && wow64.as_bool() {
            return None;
        }
        let mut info = PROCESS_BASIC_INFORMATION::default();
        let mut length = 0u32;
        let status = NtQueryInformationProcess(
            handle.0,
            ProcessBasicInformation,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut length,
        );
        if status.is_err() || info.PebBaseAddress.is_null() {
            return None;
        }
        let read = |address: usize, size: usize| -> Option<Vec<u8>> {
            let mut buffer = vec![0u8; size];
            let mut done = 0usize;
            ReadProcessMemory(
                handle.0,
                address as *const c_void,
                buffer.as_mut_ptr() as *mut c_void,
                size,
                Some(&mut done),
            )
            .ok()?;
            (done == size).then_some(buffer)
        };
        let parameters = u64::from_le_bytes(read(info.PebBaseAddress as usize + 0x20, 8)?.try_into().ok()?); // PEB.ProcessParameters
        if parameters == 0 {
            return None;
        }
        let header = read(parameters as usize + 0x38, 16)?; // CurrentDirectory.DosPath
        let length = u16::from_le_bytes([header[0], header[1]]) as usize;
        let buffer = u64::from_le_bytes(header[8..16].try_into().ok()?);
        if length == 0 || buffer == 0 {
            return None;
        }
        let text = read(buffer as usize, length)?;
        let units: Vec<u16> = text.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        Some(PathBuf::from(String::from_utf16_lossy(&units)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_process_with_a_file_open_and_one_working_in_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("site");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("locked.txt");
        std::fs::write(&file, "x").unwrap();

        // A ping that runs a while, started in the folder: its working folder is inside it.
        let mut child = std::process::Command::new(dnnm_infrastructure::process::system32("PING.EXE"))
            .args(["-n", "30", "127.0.0.1"])
            .current_dir(&folder)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let lockers = find_lockers(&folder);
        let _ = child.kill();
        let _ = child.wait();

        let ping = lockers.iter().find(|l| l.id == child.id()).expect("the ping is found");
        assert_eq!(ping.exe.to_lowercase(), "ping.exe");
        assert_eq!(ping.reason, "its working folder is inside the project");
        assert!(ping.can_close);
    }

    #[test]
    fn closes_a_locker_by_force_when_it_has_no_window() {
        let mut child = std::process::Command::new(dnnm_infrastructure::process::system32("PING.EXE"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let locker = LockingProcess {
            id: child.id(),
            name: "ping".into(),
            exe: "PING.EXE".into(),
            reason: String::new(),
            can_close: true,
        };
        assert!(close(&[locker]).is_empty());
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn this_process_and_windows_own_are_never_closed() {
        let list = processes();
        let mut found = BTreeMap::new();
        add(&mut found, &list, std::process::id(), None, "x", false);
        assert!(!found.values().next().unwrap().can_close);
        assert_eq!(process_name("Code.exe"), "Code");
        assert!(find_lockers(&std::env::temp_dir().join("no-such-folder-dnnm")).is_empty());
    }
}
