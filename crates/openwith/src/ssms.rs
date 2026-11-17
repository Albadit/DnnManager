//! SQL Server Management Studio: starting it on a database (`IdeLocator.OpenDatabase`, `StartManagementStudio`,
//! `FindRunning`) and with DNN Manager's rights only from a copy nobody but administrators can change
//! (`Infrastructure/Processes/ElevatedStart.cs`'s `Start`).
//!
//! SSMS starts with DNN Manager's administrator rights - Windows authentication signs in as an administrator then - so
//! only a copy that only administrators can change; [`crate::locate::refusal`] says beforehand when it isn't one.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use dnnm_sql::model::SiteSqlConnection;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};

use crate::locate::Ide;

/// The command line switches that open `database` in `ssms`: server, database and login filled in. SSMS takes no
/// password on its command line, so it asks for one (and can remember it); no user means Windows authentication.
/// `trust_server_certificate`: trust the server's certificate without validating it - for the local container, whose
/// certificate is self-signed. SSMS 20+ encrypts by default and refuses such a certificate otherwise.
pub fn open_database_arguments(
    ssms: &Ide,
    database: &SiteSqlConnection,
    trust_server_certificate: bool,
    display_name: &str,
) -> Vec<String> {
    // SSMS 21+ switches: -S -d -U -A -C -N -i -dn -nosplash -log; no user = Windows authentication.
    // SSMS 18-20 have no -C / -dn and take -E for Windows authentication.
    let modern = ssms.major_version >= 21;
    let mut args: Vec<String> = vec!["-S".into(), database.server.clone()];
    if !database.database.is_empty() {
        args.extend(["-d".into(), database.database.clone()]);
    }
    if !database.user.is_empty() {
        args.extend(["-U".into(), database.user.clone()]);
    } else if !modern {
        args.push("-E".into());
    }
    if modern {
        if trust_server_certificate {
            args.push("-C".into());
        }
        args.extend(["-dn".into(), display_name.to_string()]);
    }
    args.push("-nosplash".into());
    args
}

/// Opens `database` in `ssms` with [`open_database_arguments`].
pub fn open_database(
    ssms: &Ide,
    database: &SiteSqlConnection,
    trust_server_certificate: bool,
    display_name: &str,
) -> Result<SsmsProcess, String> {
    let args = open_database_arguments(ssms, database, trust_server_certificate, display_name);
    start_elevated(&ssms.exe_path, &args.iter().map(String::as_str).collect::<Vec<_>>(), None)
}

/// Starts SSMS with no connection switches, so it just shows its Connect dialog (any connection switch makes it connect
/// at once - without a password, failing with an error first).
pub fn start_management_studio(ssms: &Ide) -> Result<SsmsProcess, String> {
    start_elevated(&ssms.exe_path, &["-nosplash"], None)
}

/// A process of SSMS: its id, a way to tell whether it has ended, and its main window when it has one.
pub struct SsmsProcess {
    pub id: u32,
    exit: ExitWatch,
    pub main_window: Option<isize>,
}

impl SsmsProcess {
    fn open(id: u32, main_window: Option<isize>) -> Self {
        // SAFETY: the handle, when there is one, is owned and closed when the last watch goes.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, id) }
            .ok()
            .map(|h| Arc::new(unsafe { OwnedHandle::from_raw_handle(h.0) }));
        Self { id, exit: ExitWatch(handle), main_window }
    }

    /// Whether it has ended (`Process.HasExited`).
    pub fn has_exited(&self) -> bool {
        self.exit.has_exited()
    }

    pub(crate) fn watch(&self) -> ExitWatch {
        self.exit.clone()
    }
}

/// Tells whether a process has ended - from any thread. A process that couldn't be opened counts as ended.
#[derive(Clone)]
pub struct ExitWatch(Option<Arc<OwnedHandle>>);

impl ExitWatch {
    pub fn has_exited(&self) -> bool {
        let Some(handle) = &self.0 else { return true };
        // SAFETY: a valid process handle; a zero wait only asks.
        unsafe { WaitForSingleObject(HANDLE(handle.as_raw_handle()), 0) == WAIT_OBJECT_0 }
    }
}

/// The one way to start a program with DNN Manager's administrator rights (`ElevatedStart.Start`): only a copy nobody
/// but administrators can change, started by its own path, each argument passed as it is - never a command line put
/// together - and, elevated, with the environment of `dnnm_sql::programs::harden` (none of the user's variables that
/// make a program load other code). Err says why it may not be started (or isn't there), or why Windows couldn't.
pub fn start_elevated(
    program: &Path,
    arguments: &[&str],
    working_directory: Option<&Path>,
) -> Result<SsmsProcess, String> {
    let name = program.display().to_string();
    let resolved = dnnm_system::trusted::resolve(&name);
    let Some(path) = resolved.path.clone() else { return Err(resolved.problem(&name)) };
    let mut command = Command::new(path);
    command.args(arguments);
    if let Some(directory) = working_directory {
        command.current_dir(directory);
    }
    if dnnm_system::trusted::is_elevated() {
        let environment = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
        for (variable, value) in dnnm_sql::programs::harden(environment) {
            match value {
                Some(value) => command.env(variable, value),
                None => command.env_remove(variable),
            };
        }
    }
    let child = command.spawn().map_err(|e| format!("{name} didn't start: {e}"))?;
    Ok(SsmsProcess::open(child.id(), None))
}

/// A running instance of exactly this SSMS (same exe) with a main window, or None.
pub fn find_running(ssms: &Ide) -> Option<SsmsProcess> {
    struct Search {
        exe: String,
        found: Option<(u32, isize)>,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Search` passed to EnumWindows below, alive for the whole enumeration.
        let search = unsafe { &mut *(data.0 as *mut Search) };
        // A main window: a visible top-level window no other window owns (as .NET's MainWindowHandle).
        // SAFETY: plain queries of a window handle EnumWindows hands out.
        let main = unsafe { IsWindowVisible(window).as_bool() && GetWindow(window, GW_OWNER).is_err() };
        if !main {
            return BOOL(1);
        }
        let mut id = 0u32;
        // SAFETY: as above.
        unsafe { GetWindowThreadProcessId(window, Some(&mut id)) };
        if id != 0 && image_path(id).is_some_and(|p| p.eq_ignore_ascii_case(&search.exe)) {
            search.found = Some((id, window.0 as isize));
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut search = Search { exe: ssms.exe_path.display().to_string(), found: None };
    // SAFETY: the callback only uses `search`, which outlives the call. EnumWindows reports stopping early as an error.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.found.map(|(id, window)| SsmsProcess::open(id, Some(window)))
}

// The full path of a process's program, or None when it can't be asked (ended, no access).
fn image_path(id: u32) -> Option<String> {
    // SAFETY: the handle is closed below; the buffer's size is what the call is told.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id).ok()?;
        let mut buffer = [0u16; 32768];
        let mut size = buffer.len() as u32;
        let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size);
        let _ = CloseHandle(process);
        read.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ssms(major: u32) -> Ide {
        Ide {
            name: format!("SQL Server Management Studio {major}"),
            exe_path: PathBuf::from(r"C:\ssms\SSMS.exe"),
            opens_solution: false,
            major_version: major,
        }
    }

    #[test]
    fn ssms_21_gets_the_name_and_the_certificate_switch() {
        let db = SiteSqlConnection::new("localhost,1433", "shop", "sa", "Secret-1");
        let args = open_database_arguments(&ssms(21), &db, true, "shop - shop");
        assert_eq!(args, ["-S", "localhost,1433", "-d", "shop", "-U", "sa", "-C", "-dn", "shop - shop", "-nosplash"]);
        assert!(!args.iter().any(|a| a.contains("Secret")), "never the password");
    }

    #[test]
    fn ssms_21_with_windows_authentication_has_no_user_switch() {
        let db = SiteSqlConnection::new(r".\SQLEXPRESS", "", "", "");
        let args = open_database_arguments(&ssms(22), &db, false, "Local SQL Server");
        assert_eq!(args, ["-S", r".\SQLEXPRESS", "-dn", "Local SQL Server", "-nosplash"]);
    }

    #[test]
    fn older_ssms_takes_e_for_windows_authentication_and_no_name() {
        let db = SiteSqlConnection::new("db.example.com", "shop", "", "");
        assert_eq!(
            open_database_arguments(&ssms(20), &db, true, "x"),
            ["-S", "db.example.com", "-d", "shop", "-E", "-nosplash"]
        );
        let login = SiteSqlConnection::new("db", "shop", "web", "pw");
        assert_eq!(
            open_database_arguments(&ssms(0), &login, true, "x"),
            ["-S", "db", "-d", "shop", "-U", "web", "-nosplash"]
        );
    }

    #[test]
    fn a_missing_program_is_not_started() {
        let err = start_elevated(Path::new(r"C:\no\such\Ssms.exe"), &[], None).err().unwrap();
        assert!(!err.is_empty());
    }
}
