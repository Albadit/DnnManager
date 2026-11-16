//! Starting docker, sqlcmd's SqlLocalDB, dotnet and SqlPackage as the C# does (`Processes/TrustedPrograms.cs`,
//! `ChildEnvironment.cs`, `ProcessRunner.cs`): DNN Manager runs as Administrator, so a program is only started from a
//! copy that nobody but administrators can change - a bare name looked up on the computer's PATH, never the user's -
//! with an argument list (never a shell), without a window, in an environment with the variables that make programs
//! load other code dropped, and ended with its child processes on a cancel or when its time is up.
//!
//! Not elevated (the tests), there is no such boundary, and any program goes - as in the C#.
//!
//! Deviation: the C# runs docker as the signed-in user when its only copy is one installed for this account
//! (`UserProcess`); this port refuses that copy with the C#'s message instead. Programs are also not put in one job
//! for the whole of DNN Manager: each run has a job of its own, which ends it (and what it started) on a cancel, when
//! its time is up, and when DNN Manager ends while it runs.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dnnm_application::operation::{CANCELLED, CancelToken};

/// What a program wrote and how it ended (`ProcessResult`); exit code -1 when it couldn't be started or its time was up.
#[derive(Debug, Clone, Default)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ProcessResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    fn failed(stderr: String) -> Self {
        Self { exit_code: -1, stdout: String::new(), stderr }
    }
}

/// What goes to a program's standard input.
pub enum Input<'a> {
    None,
    Text(&'a str),
    Bytes(&'a mut (dyn Read + Send)),
}

/// One run of a program (`ProcessRunner.RunAsync`'s parameters).
pub struct Run<'a> {
    pub program: &'a str,
    pub args: Vec<String>,
    /// Set after the environment is made safe - the only way a secret (SQLCMDPASSWORD) reaches a program.
    pub env: Vec<(String, String)>,
    pub input: Input<'a>,
    /// The standard output as it is (bytes, e.g. a file) - instead of [`ProcessResult::stdout`].
    pub output: Option<&'a mut (dyn Write + Send)>,
    /// Called with each stdout / stderr line as it arrives.
    pub on_output: Option<&'a (dyn Fn(&str) + Sync)>,
    /// Ended, with what it started, when it runs longer: a failed run (exit code -1).
    pub timeout: Option<Duration>,
    pub cancel: Option<&'a CancelToken>,
    /// SqlLocalDB starts the LocalDB instance sites use: what it starts outlives it.
    pub keep_children: bool,
}

impl<'a> Run<'a> {
    pub fn new(program: &'a str, args: &[&str]) -> Self {
        Self {
            program,
            args: args.iter().map(|a| a.to_string()).collect(),
            env: Vec::new(),
            input: Input::None,
            output: None,
            on_output: None,
            timeout: None,
            cancel: None,
            keep_children: false,
        }
    }

    pub fn cancel(mut self, cancel: &'a CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    pub fn stdin_text(mut self, text: &'a str) -> Self {
        self.input = Input::Text(text);
        self
    }

    /// Runs it. Err only for a cancel ("Cancelled."); a program that can't be started is a failed result, as in the C#.
    pub fn run(self) -> Result<ProcessResult, String> {
        run(self)
    }
}

fn run(mut r: Run<'_>) -> Result<ProcessResult, String> {
    let program = match resolve(r.program) {
        Resolved::Path(path) => path,
        Resolved::Refused { copy, reason } => {
            return Ok(ProcessResult::failed(format!(
                "DNN Manager doesn't start {copy} as Administrator: {reason} Install it for all users (in Program Files) - DNN Manager uses that copy."
            )));
        }
        Resolved::Missing => {
            return Ok(ProcessResult::failed(format!(
                "Could not start '{}': it isn't installed (or not on PATH).",
                r.program
            )));
        }
    };
    let mut command = Command::new(&program);
    command.args(&r.args).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.stdin(if matches!(r.input, Input::None) { Stdio::null() } else { Stdio::piped() });
    if is_elevated() {
        for (name, value) in
            harden(std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))))
        {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
    }
    for (name, value) in &r.env {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Ok(ProcessResult::failed(format!("Could not start '{}': {e}", r.program))),
    };
    let job = job::Job::for_child(&child, !r.keep_children);

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let stdin = child.stdin.take();
    let on_output = r.on_output;
    let started = Instant::now();
    let mut outcome = Ok(());
    let (out_text, err_text) = std::thread::scope(|scope| {
        let input = std::mem::replace(&mut r.input, Input::None);
        if let Some(mut stdin) = stdin {
            scope.spawn(move || {
                let _ = match input {
                    Input::Text(text) => stdin.write_all(text.as_bytes()),
                    Input::Bytes(reader) => std::io::copy(reader, &mut stdin).map(|_| ()),
                    Input::None => Ok(()),
                };
                // Dropped: closed, so the program sees the end of its input.
            });
        }
        let output = r.output.take();
        let out = scope.spawn(move || match output {
            Some(writer) => {
                let mut stdout = stdout;
                let _ = std::io::copy(&mut stdout, writer);
                String::new()
            }
            None => read_lines(stdout, on_output),
        });
        let err = scope.spawn(move || read_lines(stderr, on_output));

        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(_) => break,
            }
            if r.cancel.is_some_and(CancelToken::is_cancelled) {
                outcome = Err(CANCELLED.to_string());
            } else if r.timeout.is_some_and(|t| started.elapsed() > t) {
                outcome = Err(format!(
                    "'{}' didn't finish within {} seconds.",
                    r.program,
                    r.timeout.map_or(0, |t| t.as_secs_f64().round() as u64)
                ));
            }
            if outcome.is_err() {
                // The whole tree: docker starts helpers, which would otherwise keep the pipes (and the wait) open.
                job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        (out.join().unwrap_or_default(), err.join().unwrap_or_default())
    });
    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
    drop(job);
    match outcome {
        Ok(()) => Ok(ProcessResult { exit_code: code, stdout: out_text, stderr: err_text }),
        Err(e) if e == CANCELLED => Err(e),
        Err(timed_out) => Ok(ProcessResult { exit_code: -1, stdout: out_text, stderr: timed_out }),
    }
}

fn read_lines(stream: impl Read, on_output: Option<&(dyn Fn(&str) + Sync)>) -> String {
    let mut text = String::new();
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
        let decoded = String::from_utf8_lossy(&line);
        let trimmed = decoded.trim_end_matches(['\r', '\n']);
        if let Some(callback) = on_output {
            callback(trimmed);
        }
        text.push_str(trimmed);
        text.push_str("\r\n");
        line.clear();
    }
    text
}

/// DNN Manager runs with administrator rights - what it starts runs with them too.
pub fn is_elevated() -> bool {
    dnnm_infrastructure::windows_security::is_elevated()
}

// ─── Which copy may be started (TrustedPrograms) ───────────────────────────

/// A program DNN Manager may start with its rights - the file's own path, links resolved - or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Path(PathBuf),
    /// A copy that was found but isn't trusted, and why - for a message.
    Refused {
        copy: String,
        reason: String,
    },
    Missing,
}

/// `name` (`docker`, `dotnet`, or a full path) as it may be started: a full path is checked as it is, a bare name
/// looked up on the computer's PATH - its first copy only administrators can change. Not elevated: the name itself.
pub fn resolve(name: &str) -> Resolved {
    if !is_elevated() {
        return Resolved::Path(PathBuf::from(name));
    }
    if Path::new(name).is_absolute() {
        return check(Path::new(name));
    }
    let mut refused = None;
    for candidate in on_machine_path(name) {
        match check(&candidate) {
            Resolved::Path(path) => return Resolved::Path(path),
            other => {
                refused.get_or_insert(other);
            }
        }
    }
    if let Some(refused) = refused {
        return refused;
    }
    // Only on the user's own PATH (installed for this account): there, but never started.
    match on_user_path(name) {
        Some(own) => Resolved::Refused {
            copy: own.display().to_string(),
            reason: "it is installed for your account only - programs without administrator rights could change it, and DNN Manager would run it as Administrator.".into(),
        },
        None => Resolved::Missing,
    }
}

fn check(file: &Path) -> Resolved {
    let Some(final_path) = trust::final_path(file) else {
        return Resolved::Refused {
            copy: file.display().to_string(),
            reason: "it can't be opened to check it.".into(),
        };
    };
    match trust::admin_only(&final_path) {
        trust::Trust::AdminOnly => Resolved::Path(final_path),
        trust::Trust::Unknown => Resolved::Refused {
            copy: file.display().to_string(),
            reason: "who may change it couldn't be read - try again in a moment.".into(),
        },
        trust::Trust::OthersCanChange => Resolved::Refused {
            copy: file.display().to_string(),
            reason:
                "programs without administrator rights could change it, and DNN Manager would run it as Administrator."
                    .into(),
        },
    }
}

fn candidates(folders: &str, name: &str, path_ext: &str) -> Vec<PathBuf> {
    let extensions: Vec<&str> = if Path::new(name).extension().is_some() {
        vec![""]
    } else {
        path_ext.split(';').filter(|e| !e.is_empty()).collect()
    };
    let mut found = Vec::new();
    for folder in folders.split(';').map(str::trim).filter(|f| !f.is_empty()) {
        for extension in &extensions {
            let candidate = Path::new(folder).join(format!("{name}{extension}"));
            if candidate.is_absolute() && candidate.is_file() {
                found.push(candidate);
            }
        }
    }
    found
}

/// Where `name` is on the computer's PATH, every copy in PATH's order.
pub fn on_machine_path(name: &str) -> Vec<PathBuf> {
    candidates(&machine_path(), name, &machine_path_ext())
}

fn on_user_path(name: &str) -> Option<PathBuf> {
    let path = registry::user_value("Path")?;
    let expanded = registry::expand_with_environment(&path);
    candidates(&expanded, name, &machine_path_ext()).into_iter().next()
}

// ─── The environment of a program started elevated (ChildEnvironment) ──────

// .NET's runtime, host and SDK settings (startup hooks, profilers, diagnostic ports), NuGet's, docker's and sqlcmd's.
const DROPPED_PREFIXES: [&str; 7] = ["DOTNET_", "COMPlus_", "COR_", "CORECLR_", "NUGET_", "DOCKER_", "SQLCMD"];

/// What to change in `environment` (name → Some(value) to set, None to remove) so a program started with
/// administrator rights loads no code the user's environment points it at (`ChildEnvironment.Harden`).
pub fn harden(environment: impl Iterator<Item = (String, String)>) -> Vec<(String, Option<String>)> {
    let mut changes: Vec<(String, Option<String>)> = environment
        .filter(|(name, _)| {
            DROPPED_PREFIXES.iter().any(|p| name.len() >= p.len() && name[..p.len()].eq_ignore_ascii_case(p))
        })
        .map(|(name, _)| (name, None))
        .collect();
    for (name, folder) in system_folders() {
        if !folder.is_empty() {
            changes.push((name.to_string(), Some(folder)));
        }
    }
    changes.push(("PATH".into(), Some(machine_path())));
    changes.push(("PATHEXT".into(), Some(machine_path_ext())));
    changes.push(("DOTNET_EnableDiagnostics".into(), Some("0".into())));
    let docker_config =
        crate::dirs::docker_config().unwrap_or_else(|_| system32_dir().join("config").join("DnnManager-docker"));
    changes.push(("DOCKER_CONFIG".into(), Some(docker_config.display().to_string())));
    changes
}

/// The computer's PATH from the registry, its %variables% filled in only from Windows' own folders and the computer's
/// variables - never the user's. A folder with a variable it can't fill in so is left out.
pub fn machine_path() -> String {
    registry::machine_value("Path")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .filter_map(|entry| expand(entry, 0))
        .collect::<Vec<_>>()
        .join(";")
}

pub fn machine_path_ext() -> String {
    registry::machine_value("PATHEXT")
        .filter(|e| !e.is_empty())
        .and_then(|e| expand(&e, 0))
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
}

fn expand(value: &str, depth: u32) -> Option<String> {
    let folders: HashMap<String, String> =
        system_folders().into_iter().filter(|(_, f)| !f.is_empty()).map(|(n, f)| (n.to_ascii_lowercase(), f)).collect();
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%').filter(|&e| e > 0 && !after[..e].contains(';')) else {
            break;
        };
        out.push_str(&rest[..start]);
        let name = &after[..end];
        if let Some(folder) = folders.get(&name.to_ascii_lowercase()) {
            out.push_str(folder);
        } else {
            let inner =
                (depth < 3).then(|| registry::machine_value(name).and_then(|m| expand(&m, depth + 1))).flatten()?;
            out.push_str(&inner);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn system32_dir() -> PathBuf {
    known_folders::windows().join("System32")
}

fn system_folders() -> Vec<(&'static str, String)> {
    let windows = known_folders::windows().display().to_string();
    let program_files = known_folders::program_files();
    let common = known_folders::common_files();
    let program_data = known_folders::program_data().display().to_string();
    let drive = windows.get(..2).unwrap_or_default().to_string();
    vec![
        ("SystemRoot", windows.clone()),
        ("windir", windows.clone()),
        ("SystemDrive", drive),
        ("ComSpec", system32_dir().join("cmd.exe").display().to_string()),
        ("ProgramFiles", program_files.clone()),
        ("ProgramW6432", program_files),
        ("ProgramFiles(x86)", known_folders::program_files_x86()),
        ("CommonProgramFiles", common.clone()),
        ("CommonProgramW6432", common),
        ("CommonProgramFiles(x86)", known_folders::common_files_x86()),
        ("ProgramData", program_data.clone()),
        ("ALLUSERSPROFILE", program_data),
    ]
}

/// Windows' own folders, from Windows rather than the user's environment.
pub mod known_folders {
    use std::path::PathBuf;

    #[cfg(windows)]
    fn known(id: &windows::core::GUID) -> Option<PathBuf> {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
        unsafe {
            let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let text = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as *const _));
            text.map(PathBuf::from)
        }
    }

    #[cfg(windows)]
    pub fn windows() -> PathBuf {
        known(&windows::Win32::UI::Shell::FOLDERID_Windows).unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }
    #[cfg(windows)]
    pub fn program_files() -> String {
        known(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles).map(|p| p.display().to_string()).unwrap_or_default()
    }
    #[cfg(windows)]
    pub fn program_files_x86() -> String {
        known(&windows::Win32::UI::Shell::FOLDERID_ProgramFilesX86).map(|p| p.display().to_string()).unwrap_or_default()
    }
    #[cfg(windows)]
    pub fn common_files() -> String {
        known(&windows::Win32::UI::Shell::FOLDERID_ProgramFilesCommon)
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    pub fn common_files_x86() -> String {
        known(&windows::Win32::UI::Shell::FOLDERID_ProgramFilesCommonX86)
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    pub fn program_data() -> PathBuf {
        known(&windows::Win32::UI::Shell::FOLDERID_ProgramData).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
    }

    #[cfg(not(windows))]
    pub fn windows() -> PathBuf {
        PathBuf::from("/")
    }
    #[cfg(not(windows))]
    pub fn program_files() -> String {
        String::new()
    }
    #[cfg(not(windows))]
    pub fn program_files_x86() -> String {
        String::new()
    }
    #[cfg(not(windows))]
    pub fn common_files() -> String {
        String::new()
    }
    #[cfg(not(windows))]
    pub fn common_files_x86() -> String {
        String::new()
    }
    #[cfg(not(windows))]
    pub fn program_data() -> PathBuf {
        std::env::temp_dir()
    }
}

/// Reading the registry's environment and LocalDB keys as written - %variables% not filled in from anyone's
/// environment.
pub mod registry {
    #[cfg(windows)]
    fn read(root: windows::Win32::System::Registry::HKEY, key: &str, name: &str) -> Option<String> {
        use windows::Win32::System::Registry::{RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
        use windows::core::HSTRING;
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut size = 0u32;
        unsafe {
            RegGetValueW(root, &key, &name, flags, None, None, Some(&mut size)).ok().ok()?;
            let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
            RegGetValueW(root, &key, &name, flags, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size))
                .ok()
                .ok()?;
            let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..length]))
        }
    }

    const MACHINE_ENVIRONMENT: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

    /// A variable of the computer's (HKLM), as written.
    #[cfg(windows)]
    pub fn machine_value(name: &str) -> Option<String> {
        read(windows::Win32::System::Registry::HKEY_LOCAL_MACHINE, MACHINE_ENVIRONMENT, name)
    }

    /// A variable of the user's (HKCU) - only to name a copy in a message.
    #[cfg(windows)]
    pub fn user_value(name: &str) -> Option<String> {
        read(windows::Win32::System::Registry::HKEY_CURRENT_USER, "Environment", name)
    }

    /// A string value under HKLM.
    #[cfg(windows)]
    pub fn local_machine(key: &str, name: &str) -> Option<String> {
        read(windows::Win32::System::Registry::HKEY_LOCAL_MACHINE, key, name)
    }

    /// The names of the keys under an HKLM key.
    #[cfg(windows)]
    pub fn local_machine_subkeys(key: &str) -> Vec<String> {
        use windows::Win32::System::Registry::{
            HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
        };
        use windows::core::{HSTRING, PWSTR};
        let mut names = Vec::new();
        unsafe {
            let mut handle = HKEY::default();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, &HSTRING::from(key), None, KEY_READ, &mut handle).is_err() {
                return names;
            }
            for index in 0.. {
                let mut buffer = [0u16; 256];
                let mut length = buffer.len() as u32;
                if RegEnumKeyExW(handle, index, Some(PWSTR(buffer.as_mut_ptr())), &mut length, None, None, None, None)
                    .is_err()
                {
                    break;
                }
                names.push(String::from_utf16_lossy(&buffer[..length as usize]));
            }
            let _ = RegCloseKey(handle);
        }
        names
    }

    /// `text`'s %variables% from this process's environment (the user's PATH, only to name a copy in a message).
    pub fn expand_with_environment(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find('%') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('%') else { break };
            out.push_str(&rest[..start]);
            match std::env::var(&after[..end]) {
                Ok(value) => out.push_str(&value),
                Err(_) => out.push_str(&rest[start..start + end + 2]),
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    #[cfg(not(windows))]
    pub fn machine_value(_name: &str) -> Option<String> {
        None
    }
    #[cfg(not(windows))]
    pub fn user_value(_name: &str) -> Option<String> {
        None
    }
    #[cfg(not(windows))]
    pub fn local_machine(_key: &str, _name: &str) -> Option<String> {
        None
    }
    #[cfg(not(windows))]
    pub fn local_machine_subkeys(_key: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Who may change a file or folder: the owner and the DACL, read with Windows' own API.
pub mod trust {
    use std::path::{Path, PathBuf};

    /// Whether only administrators can change a program - or that couldn't be found out (a read failed).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Trust {
        AdminOnly,
        OthersCanChange,
        Unknown,
    }

    /// NT SERVICE\TrustedInstaller, owner of Windows' own files and of Program Files.
    pub const TRUSTED_INSTALLER_SID: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
    // SYSTEM, Administrators, CREATOR OWNER (whose rights go to the owner, checked on its own), TrustedInstaller.
    const TRUSTED_SIDS: [&str; 4] = ["S-1-5-18", "S-1-5-32-544", "S-1-3-0", TRUSTED_INSTALLER_SID];

    pub fn is_trusted_sid(sid: &str) -> bool {
        TRUSTED_SIDS.contains(&sid)
    }

    // FileSystemRights: changing a file, or putting another in its place (or a DLL beside it).
    const WRITE_DATA: u32 = 0x2;
    const APPEND_DATA: u32 = 0x4;
    const WRITE_EXTENDED_ATTRIBUTES: u32 = 0x10;
    const DELETE_SUBDIRECTORIES_AND_FILES: u32 = 0x40;
    const WRITE_ATTRIBUTES: u32 = 0x100;
    const DELETE: u32 = 0x1_0000;
    const CHANGE_PERMISSIONS: u32 = 0x4_0000;
    const TAKE_OWNERSHIP: u32 = 0x8_0000;
    pub const CHANGES: u32 = WRITE_DATA
        | APPEND_DATA
        | DELETE
        | DELETE_SUBDIRECTORIES_AND_FILES
        | CHANGE_PERMISSIONS
        | TAKE_OWNERSHIP
        | WRITE_ATTRIBUTES
        | WRITE_EXTENDED_ATTRIBUTES;
    const GENERIC_ALL: u32 = 0x1000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;

    /// An access rule: who, what (a FileSystemRights mask, generic rights read as every change), whether it is for
    /// what is made inside only.
    #[derive(Debug, Clone)]
    pub struct Rule {
        pub sid: String,
        pub mask: u32,
        pub inherit_only: bool,
    }

    /// A file's or folder's owner and its allow rules (None: no DACL - everyone may do anything).
    #[derive(Debug, Clone)]
    pub struct Security {
        pub owner: Option<String>,
        pub allows: Option<Vec<Rule>>,
    }

    /// Whether only trusted accounts may do `rights` by `security` (and own it, with `check_owner`).
    pub fn admin_only_rules(security: &Security, rights: u32, check_owner: bool, skip_inherit_only: bool) -> bool {
        if check_owner && security.owner.as_deref().is_some_and(|o| !is_trusted_sid(o)) {
            return false;
        }
        let Some(rules) = &security.allows else { return false };
        !rules.iter().any(|r| {
            let mask = if r.mask & (GENERIC_ALL | GENERIC_WRITE) != 0 { r.mask | CHANGES } else { r.mask };
            mask & rights != 0 && !(skip_inherit_only && r.inherit_only) && !is_trusted_sid(&r.sid)
        })
    }

    /// Whether only administrators (and Windows) can change `file`: the file, its folder, and every folder above -
    /// none a link or junction, none that others may delete, rename or take over.
    pub fn admin_only(file: &Path) -> Trust {
        let attributes = |p: &Path| std::fs::symlink_metadata(p).map(|m| is_reparse_point(&m));
        match attributes(file) {
            Ok(false) => {}
            Ok(true) => return Trust::OthersCanChange,
            Err(e) => return io_trust(&e),
        }
        let file_rules = match read_security(file) {
            Ok(s) => s,
            Err(t) => return t,
        };
        if !admin_only_rules(&file_rules, CHANGES, true, false) {
            return Trust::OthersCanChange;
        }
        let Some(folder) = file.parent() else { return Trust::OthersCanChange };
        folder_admin_only(folder)
    }

    /// Whether only administrators can change `folder` and the folders above it (what a program in it, or a folder
    /// DNN Manager keeps its own files in, needs).
    pub fn folder_admin_only(folder: &Path) -> Trust {
        let attributes = |p: &Path| std::fs::symlink_metadata(p).map(|m| is_reparse_point(&m));
        match attributes(folder) {
            Ok(false) => {}
            Ok(true) => return Trust::OthersCanChange,
            Err(e) => return io_trust(&e),
        }
        let mut above_rules = match read_security(folder) {
            Ok(s) => s,
            Err(t) => return t,
        };
        if !admin_only_rules(&above_rules, CHANGES, true, false) {
            return Trust::OthersCanChange;
        }
        let mut above = folder.to_path_buf();
        while let Some(parent) = above.parent().map(Path::to_path_buf) {
            if parent.parent().is_some() && attributes(&parent).unwrap_or(true) {
                return Trust::OthersCanChange;
            }
            let parent_rules = match read_security(&parent) {
                Ok(s) => s,
                Err(t) => return t,
            };
            if !admin_only_rules(&above_rules, DELETE | CHANGE_PERMISSIONS | TAKE_OWNERSHIP, false, true)
                || !admin_only_rules(
                    &parent_rules,
                    DELETE_SUBDIRECTORIES_AND_FILES | CHANGE_PERMISSIONS | TAKE_OWNERSHIP,
                    false,
                    true,
                )
            {
                return Trust::OthersCanChange;
            }
            above_rules = parent_rules;
            above = parent;
        }
        Trust::AdminOnly
    }

    fn io_trust(e: &std::io::Error) -> Trust {
        if e.kind() == std::io::ErrorKind::PermissionDenied { Trust::OthersCanChange } else { Trust::Unknown }
    }

    #[cfg(windows)]
    fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }

    #[cfg(not(windows))]
    fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
        metadata.file_type().is_symlink()
    }

    /// The owner and allow rules of `path`; Err with how to take a failure (not readable even by an administrator:
    /// not to be trusted; anything else may pass).
    #[cfg(windows)]
    pub fn read_security(path: &Path) -> Result<Security, Trust> {
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
        use windows::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        };
        use windows::core::HSTRING;

        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let result = unsafe {
            GetNamedSecurityInfoW(
                &HSTRING::from(path.as_os_str()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            )
        };
        if result.is_err() {
            return Err(if result == ERROR_ACCESS_DENIED { Trust::OthersCanChange } else { Trust::Unknown });
        }
        let security = unsafe {
            let owner = sid_string(owner);
            let allows = if dacl.is_null() {
                None
            } else {
                let mut rules = Vec::new();
                for index in 0..u32::from((*dacl).AceCount) {
                    let mut ace = std::ptr::null_mut();
                    if GetAce(dacl, index, &mut ace).is_err() {
                        continue;
                    }
                    let header = &*(ace as *const ACE_HEADER);
                    // Allowed, allowed-object, allowed-callback and allowed-callback-object: every kind that grants.
                    if !matches!(header.AceType, 0 | 5 | 9 | 11) {
                        continue;
                    }
                    let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
                    // An object ACE keeps its SID further in; only the plain kinds are read for their SID, the others
                    // count as anyone's.
                    let sid = if matches!(header.AceType, 0 | 9) {
                        sid_string(PSID(&allowed.SidStart as *const u32 as *mut _)).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    rules.push(Rule {
                        sid,
                        mask: allowed.Mask,
                        inherit_only: u32::from(header.AceFlags) & INHERIT_ONLY_ACE.0 != 0,
                    });
                }
                Some(rules)
            };
            Security { owner, allows }
        };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        Ok(security)
    }

    #[cfg(windows)]
    unsafe fn sid_string(sid: windows::Win32::Security::PSID) -> Option<String> {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows::core::PWSTR;
        if sid.is_invalid() {
            return None;
        }
        let mut text = PWSTR::null();
        unsafe {
            ConvertSidToStringSidW(sid, &mut text).ok()?;
            let value = text.to_string().ok();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            value
        }
    }

    #[cfg(not(windows))]
    pub fn read_security(_path: &Path) -> Result<Security, Trust> {
        Err(Trust::Unknown)
    }

    /// The path Windows opens for `file` - every link, junction and short name resolved - or None.
    pub fn final_path(file: &Path) -> Option<PathBuf> {
        let path = std::fs::canonicalize(file).ok()?;
        let text = path.display().to_string();
        if text.starts_with(r"\\?\UNC\") {
            return None;
        }
        Some(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)))
    }
}

/// A job object per run: ends the program and what it started on a cancel or a time-out, and - with
/// `kill_on_close` - when DNN Manager ends while it runs.
mod job {
    #[cfg(windows)]
    pub struct Job(Option<windows::Win32::Foundation::HANDLE>);

    #[cfg(windows)]
    impl Job {
        pub fn for_child(child: &std::process::Child, kill_on_close: bool) -> Self {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::Foundation::{CloseHandle, HANDLE};
            use windows::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
            };
            unsafe {
                let Ok(job) = CreateJobObjectW(None, None) else { return Self(None) };
                if kill_on_close {
                    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                    let _ = SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                        size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                }
                if AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())).is_err() {
                    let _ = CloseHandle(job);
                    return Self(None);
                }
                Self(Some(job))
            }
        }

        pub fn terminate(&self) {
            if let Some(job) = self.0 {
                unsafe {
                    let _ = windows::Win32::System::JobObjects::TerminateJobObject(job, 1);
                }
            }
        }
    }

    #[cfg(windows)]
    impl Drop for Job {
        fn drop(&mut self) {
            if let Some(job) = self.0.take() {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(job);
                }
            }
        }
    }

    #[cfg(not(windows))]
    pub struct Job;

    #[cfg(not(windows))]
    impl Job {
        pub fn for_child(_child: &std::process::Child, _kill_on_close: bool) -> Self {
            Self
        }
        pub fn terminate(&self) {}
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn cmd() -> String {
        system32_dir().join("cmd.exe").display().to_string()
    }

    #[test]
    fn runs_a_program_with_its_input_environment_and_output() {
        let r = Run::new(&cmd(), &["/c", "echo %DNNM_TEST%"]).env("DNNM_TEST", "hello").run().unwrap();
        assert!(r.success());
        assert_eq!(r.stdout.trim(), "hello");

        let more = system32_dir().join("more.com").display().to_string();
        let r = Run::new(&more, &[]).stdin_text("from stdin\r\n").run().unwrap();
        assert_eq!(r.stdout.trim(), "from stdin");
    }

    #[test]
    fn a_program_past_its_time_is_ended_and_a_cancel_is_cancelled() {
        let ping = system32_dir().join("PING.EXE").display().to_string();
        let r = Run::new(&ping, &["-n", "30", "127.0.0.1"]).timeout(Duration::from_millis(300)).run().unwrap();
        assert_eq!(r.exit_code, -1);
        assert!(r.stderr.contains("didn't finish within"), "{}", r.stderr);

        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(Run::new(&ping, &["-n", "30", "127.0.0.1"]).cancel(&cancel).run().unwrap_err(), CANCELLED);
    }

    #[test]
    fn a_missing_program_is_a_failed_run() {
        let r = Run::new(r"C:\no\such\program.exe", &[]).run().unwrap();
        assert_eq!(r.exit_code, -1);
        assert!(r.stderr.contains("Could not start"));
    }

    #[test]
    fn windows_own_programs_are_admin_only_and_a_temp_file_is_not() {
        assert_eq!(trust::admin_only(&system32_dir().join("cmd.exe")), trust::Trust::AdminOnly);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.exe");
        std::fs::write(&file, b"x").unwrap();
        assert_ne!(trust::admin_only(&file), trust::Trust::AdminOnly);
    }

    #[test]
    fn the_hardened_environment_drops_code_loading_variables_and_uses_the_machine_path() {
        let changes = harden(
            [
                ("DOTNET_STARTUP_HOOKS".to_string(), "x".to_string()),
                ("SQLCMDPASSWORD".into(), "p".into()),
                ("HOME".into(), "h".into()),
            ]
            .into_iter(),
        );
        let get = |name: &str| changes.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        assert_eq!(get("DOTNET_STARTUP_HOOKS"), Some(None));
        assert_eq!(get("SQLCMDPASSWORD"), Some(None));
        assert_eq!(get("HOME"), None);
        assert_eq!(get("DOTNET_EnableDiagnostics"), Some(Some("0".into())));
        let path = get("PATH").flatten().unwrap();
        assert!(path.to_lowercase().contains(r"\windows\system32"), "{path}");
        assert!(!path.contains('%'), "{path}");
    }

    #[test]
    fn rules_trust_only_administrators() {
        use trust::{CHANGES, Rule, Security, admin_only_rules};
        let rule = |sid: &str, mask, inherit_only| Rule { sid: sid.into(), mask, inherit_only };
        let admins =
            Security { owner: Some("S-1-5-32-544".into()), allows: Some(vec![rule("S-1-5-32-544", 0x1F01FF, false)]) };
        assert!(admin_only_rules(&admins, CHANGES, true, false));
        let users =
            Security { owner: Some("S-1-5-32-544".into()), allows: Some(vec![rule("S-1-5-32-545", 0x2, false)]) };
        assert!(!admin_only_rules(&users, CHANGES, true, false));
        let inherit_only = Security { owner: None, allows: Some(vec![rule("S-1-5-11", 0x4000_0000, true)]) };
        assert!(admin_only_rules(&inherit_only, CHANGES, false, true));
        assert!(!admin_only_rules(&inherit_only, CHANGES, false, false), "generic write counts as a change");
        let no_dacl = Security { owner: Some("S-1-5-18".into()), allows: None };
        assert!(!admin_only_rules(&no_dacl, CHANGES, true, false));
        let users_own = Security { owner: Some("S-1-5-21-1-2-3-1001".into()), allows: Some(vec![]) };
        assert!(!admin_only_rules(&users_own, CHANGES, true, false));
    }
}
