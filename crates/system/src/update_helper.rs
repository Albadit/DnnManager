//! The update's second half, in a process of its own (`Updates/UpdateHelper.cs`): a copy of the old DNN Manager's exe
//! started with [`ARGUMENT`] and an [`UpdatePlan`], from a folder of its own - so nothing it runs from is replaced. It
//! waits for DNN Manager to close, installs the new version (Setup silently, or the portable exe swapped with a
//! backup), and starts DNN Manager again - the new version, or the old one when the update failed.
//!
//! What differs for the Rust exe from the WPF installer flow:
//! - The Rust build is one exe with nothing beside it, so [`UpdateKind::Portable`] is its flow: the new exe is copied
//!   next to the old one (`.new`), swapped in with `ReplaceFileW` (the old one kept as `.old`), and put back when the
//!   new one closes with an error within 15 seconds. Shortcuts and the sign-in task keep working - the name stays.
//! - The Setup flow (Inno Setup's `/SILENT … /ALLUSERS|/CURRENTUSER /LOG=`) is ported as it is, for an installation
//!   made by the WPF Setup; there is no Setup of the Rust build yet.
//! - The release's asset names are the WPF app's ([`crate::updates::AppRelease::asset_for`]); a Rust release needs
//!   assets of its own name before the update offers it.
//! - The C# starts Setup and the new version with `ChildEnvironment.ForSelf` (the .NET variables dropped); here the
//!   .NET variables are dropped too ([`crate::startup::is_dropped`]) - the Rust exe doesn't read them, Setup's child
//!   processes might.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use dnnm_application::project::view::group_thousands;
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Storage::FileSystem::{
    REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::core::PCWSTR;

use crate::updates::{UpdateKind, sha256_file};
use crate::win::{Handle, wide_path};

pub const ARGUMENT: &str = "--apply-update";

/// How long a new portable exe must keep running before its backup is deleted.
pub const WATCH: Duration = Duration::from_secs(15);

/// How long DNN Manager may take to close.
pub const CLOSING: Duration = Duration::from_secs(120);

/// What the helper is to do - written by the running DNN Manager before it closes. The WPF app's JSON names, so a plan
/// either app wrote reads in the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdatePlan {
    pub kind: UpdateKind,
    /// The downloaded, checked Setup or portable exe.
    pub package: PathBuf,
    /// The package's SHA-256 as GitHub lists it - checked again right before it is used.
    pub package_sha256: String,
    pub package_size: u64,
    /// The exe that is replaced (portable) or installed over (Setup) - and started again afterwards.
    pub app_exe: PathBuf,
    #[serde(default)]
    pub all_users: bool,
    /// The DNN Manager that closes for the update - nothing is touched until it has.
    pub wait_for_process_id: u32,
    pub from_version: String,
    pub to_version: String,
    pub log_file: PathBuf,
    /// Where the helper writes the [`UpdateResult`] the started DNN Manager reads.
    pub result_file: PathBuf,
    /// When the update fails: where its log and Setup's are kept (the user's logs folder).
    #[serde(default)]
    pub failed_log_file: Option<PathBuf>,
}

impl UpdatePlan {
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }

    pub fn read(path: &Path) -> std::io::Result<Self> {
        serde_json::from_str(&std::fs::read_to_string(path)?).map_err(std::io::Error::other)
    }
}

/// How the update went - read by the DNN Manager the helper starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateResult {
    pub installed: bool,
    pub message: String,
    pub log_file: Option<PathBuf>,
}

impl UpdateResult {
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }

    /// The result in `path`; None when there is none (the helper didn't get that far) or it can't be read.
    pub fn try_read(path: Option<&Path>) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path?).ok()?).ok()
    }
}

pub fn is_helper(args: &[String]) -> bool {
    args.len() >= 2 && args[0] == ARGUMENT
}

/// Starts DNN Manager's helper: a copy of `current_exe` in `folder` (DNN Manager's private temporary folder, which only
/// administrators can change), run with [`ARGUMENT`] and the plan written beside it.
pub fn start_helper(plan: &UpdatePlan, current_exe: &Path, folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let helper = folder.join("DnnManager-update.exe");
    std::fs::copy(current_exe, &helper)?;
    let plan_file = folder.join("update-plan.json");
    plan.write(&plan_file)?;
    let mut command = std::process::Command::new(&helper);
    command.arg(ARGUMENT).arg(&plan_file).current_dir(folder);
    drop_dotnet_variables(&mut command);
    command.spawn().map(|_| ())
}

fn drop_dotnet_variables(command: &mut std::process::Command) {
    for (name, _) in std::env::vars_os() {
        if crate::startup::is_dropped(&name.to_string_lossy()) {
            command.env_remove(&name);
        }
    }
}

/// A process the helper started.
pub trait Started {
    /// Its exit code once it has ended within `timeout`; None while it runs.
    fn wait(&mut self, timeout: Duration) -> Option<i32>;
}

/// Starts a process (Setup, DNN Manager) - replaced in tests.
pub trait Starter {
    fn start(&self, program: &Path, args: &[String], directory: &Path) -> std::io::Result<Box<dyn Started>>;
}

/// Starts processes as they are, with a window when they have one.
pub struct SystemStarter;

struct Child(std::process::Child);

impl Started for Child {
    fn wait(&mut self, timeout: Duration) -> Option<i32> {
        let until = std::time::Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.0.try_wait() {
                return Some(status.code().unwrap_or(-1));
            }
            if std::time::Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Starter for SystemStarter {
    fn start(&self, program: &Path, args: &[String], directory: &Path) -> std::io::Result<Box<dyn Started>> {
        let mut command = std::process::Command::new(program);
        command.args(args).current_dir(directory);
        drop_dotnet_variables(&mut command);
        Ok(Box::new(Child(command.spawn()?)))
    }
}

/// Runs the plan in `args[1]`. Never panics, never shows anything: the result is in the plan's result file and log.
pub fn run(args: &[String]) -> i32 {
    let Some(plan) = args.get(1).and_then(|p| UpdatePlan::read(Path::new(p)).ok()) else { return 2 };
    run_plan(&plan, &SystemStarter, WATCH, CLOSING)
}

/// Runs `plan`: waits up to `closing` for DNN Manager to close, installs, starts it again, and - portable - watches the
/// new version for `watch`.
pub fn run_plan(plan: &UpdatePlan, starter: &dyn Starter, watch: Duration, closing: Duration) -> i32 {
    let log = HelperLog(plan.log_file.clone());
    log.write(&format!(
        "Updating DNN Manager {} to {} ({:?}): {}",
        plan.from_version,
        plan.to_version,
        plan.kind,
        plan.app_exe.display()
    ));
    if !wait_for_exit(plan.wait_for_process_id, closing) {
        // It still runs: nothing was changed, and nothing is started - it is open.
        finish(
            plan,
            &log,
            false,
            &format!("DNN Manager didn't close within {} minutes - nothing was changed.", minutes(closing)),
        );
        return 1;
    }
    log.write("DNN Manager has closed.");

    let backup = backup_of(plan);
    // A backup left by an earlier update isn't this one's: deleted before anything changes.
    if plan.kind == UpdateKind::Portable
        && backup.exists()
        && let Err(e) = std::fs::remove_file(&backup)
    {
        finish(
            plan,
            &log,
            false,
            &format!("{}, left by an earlier update, can't be deleted ({e}) - nothing was changed.", backup.display()),
        );
        launch(plan, starter, &log);
        return 1;
    }

    // Checked again now, and held open so it can't be changed while it is copied or run.
    let mut package = match open_checked(plan) {
        Ok(file) => file,
        Err(problem) => {
            finish(
                plan,
                &log,
                false,
                &format!("The downloaded update can't be used: {problem} - nothing was changed."),
            );
            launch(plan, starter, &log);
            return 1;
        }
    };
    let failure = match plan.kind {
        UpdateKind::Portable => replace_portable(plan, &mut package, &log),
        UpdateKind::Installer => run_setup(plan, starter, &log),
        UpdateKind::Msi => run_msi(plan, starter, &log),
    };
    drop(package);
    if let Some(failure) = failure {
        finish(plan, &log, false, &failure);
        launch(plan, starter, &log);
        return 1;
    }

    finish(plan, &log, true, &format!("DNN Manager {} was installed.", plan.to_version));
    let started = launch(plan, starter, &log);
    if plan.kind == UpdateKind::Portable {
        if let Some(code) = started.and_then(|mut s| s.wait(watch)).filter(|&c| c != 0) {
            log.write(&format!(
                "The new version closed right after starting (exit code {code}) - putting {} back.",
                plan.from_version
            ));
            restore_portable(plan, &log);
            finish(
                plan,
                &log,
                false,
                &format!(
                    "DNN Manager {} closed right after it started (exit code {code}) - {} was put back.",
                    plan.to_version, plan.from_version
                ),
            );
            launch(plan, starter, &log);
            return 1;
        }
        try_delete(&backup, &log);
    }
    log.write("Done.");
    0
}

fn minutes(d: Duration) -> String {
    let m = d.as_secs_f64() / 60.0;
    if m.fract() == 0.0 { format!("{m:.0}") } else { format!("{m:.1}") }
}

/// The package opened so that nobody can write to it while it is open - once its size and SHA-256 are the release's.
pub fn open_checked(plan: &UpdatePlan) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&plan.package)
        .map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length != plan.package_size {
        return Err(format!(
            "it is {} bytes, GitHub lists {}",
            group_thousands(length as f64, 0),
            group_thousands(plan.package_size as f64, 0)
        ));
    }
    if plan.package_sha256.is_empty() {
        return Err("GitHub lists no SHA-256 for it".into());
    }
    if !sha256_file(&mut file).map_err(|e| e.to_string())?.eq_ignore_ascii_case(&plan.package_sha256) {
        return Err("its SHA-256 isn't the one GitHub lists".into());
    }
    file.rewind().map_err(|e| e.to_string())?;
    Ok(file)
}

/// Whether the process has ended within `timeout` (also when it is already gone).
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    // SAFETY: the handle is closed by `Handle`.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
        let handle = Handle(handle);
        WaitForSingleObject(handle.0, timeout.as_millis().min(u32::MAX as u128 - 1) as u32) == WAIT_OBJECT_0
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn staged_of(plan: &UpdatePlan) -> PathBuf {
    with_suffix(&plan.app_exe, ".new")
}

fn backup_of(plan: &UpdatePlan) -> PathBuf {
    with_suffix(&plan.app_exe, ".old")
}

/// Puts the new exe in the old one's place, under the old one's name, keeping the old one as a backup. None when it
/// worked, else why not - the old exe is then in place.
fn replace_portable(plan: &UpdatePlan, package: &mut std::fs::File, log: &HelperLog) -> Option<String> {
    let staged = staged_of(plan);
    let backup = backup_of(plan);
    // Next to the exe, so the swap is a rename on one volume - copied from the checked file, still held open.
    let copied = std::fs::File::create(&staged).and_then(|mut target| {
        std::io::copy(package, &mut target)?;
        target.flush()
    });
    if let Err(e) = copied {
        try_delete(&staged, log);
        return Some(format!("{} couldn't be replaced: {e}", plan.app_exe.display()));
    }
    let mut last = String::new();
    // The closed process's file, or an antivirus scan, can hold it for a moment.
    for _ in 0..20 {
        let (replaced, replacement, backup_name) = (wide_path(&plan.app_exe), wide_path(&staged), wide_path(&backup));
        // SAFETY: NUL-terminated paths that live for the call.
        let result = unsafe {
            ReplaceFileW(
                PCWSTR(replaced.as_ptr()),
                PCWSTR(replacement.as_ptr()),
                PCWSTR(backup_name.as_ptr()),
                REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
                None,
                None,
            )
        };
        match result {
            Ok(()) => {
                log.write(&format!(
                    "Replaced {} (the old one is {} until the new one runs).",
                    plan.app_exe.display(),
                    backup.display()
                ));
                return None;
            }
            Err(e) => {
                last = e.message().trim().to_string();
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    log.write(&format!("Couldn't replace {}: {last}", plan.app_exe.display()));
    restore_portable(plan, log);
    try_delete(&staged, log);
    Some(format!("{} couldn't be replaced: {last}", plan.app_exe.display()))
}

/// The old exe back in its place, when the swap didn't finish or the new one didn't run.
fn restore_portable(plan: &UpdatePlan, log: &HelperLog) {
    let backup = backup_of(plan);
    if !backup.exists() {
        return;
    }
    match std::fs::rename(&backup, &plan.app_exe) {
        Ok(()) => log.write(&format!("Put {} back: {}.", plan.from_version, plan.app_exe.display())),
        Err(e) => log.write(&format!("Couldn't put {} back: {e}", backup.display())),
    }
}

/// Setup's log, beside the helper's.
fn setup_log_of(plan: &UpdatePlan) -> PathBuf {
    plan.log_file.with_extension("setup.log")
}

/// Inno Setup's switches: no questions, its progress window, no reboot, into the installation it updates.
pub fn setup_arguments(all_users: bool, setup_log: &Path) -> Vec<String> {
    [
        "/SILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/NOCANCEL",
        "/SP-",
        if all_users { "/ALLUSERS" } else { "/CURRENTUSER" },
    ]
    .into_iter()
    .map(str::to_string)
    .chain(std::iter::once(format!("/LOG={}", setup_log.display())))
    .collect()
}

/// How often the helper notes in its log that Setup is still running.
const SETUP_NOTICE: Duration = Duration::from_secs(30 * 60);

/// Runs Setup over the installation and waits for it as long as it runs: DNN Manager is started only once Setup has
/// ended. None when it worked.
fn run_setup(plan: &UpdatePlan, starter: &dyn Starter, log: &HelperLog) -> Option<String> {
    let setup_log = setup_log_of(plan);
    let args = setup_arguments(plan.all_users, &setup_log);
    log.write(&format!("Running {} {}", plan.package.display(), args.join(" ")));
    let directory = plan.package.parent().unwrap_or(Path::new("."));
    let Ok(mut setup) = starter.start(&plan.package, &args, directory) else {
        return Some("Setup couldn't be started.".into());
    };
    let mut waited = 0u64;
    let code = loop {
        if let Some(code) = setup.wait(SETUP_NOTICE) {
            break code;
        }
        waited += SETUP_NOTICE.as_secs() / 60;
        log.write(&format!(
            "Setup is still running after {waited} minutes - waiting for it; DNN Manager starts once it has ended (see {}).",
            setup_log.display()
        ));
    };
    log.write(&format!("Setup ended with exit code {code}."));
    match code {
        0 => None,
        2 | 5 => Some("Setup was cancelled.".into()),
        7 => Some(format!("Setup couldn't prepare the installation - see {}.", setup_log.display())),
        _ => Some(format!("Setup failed (exit code {code}) - see {}.", setup_log.display())),
    }
}

/// msiexec's switches: install over the old version with its progress bar and no questions, no reboot, a full log.
pub fn msi_arguments(package: &Path, msi_log: &Path) -> Vec<String> {
    vec![
        "/i".into(),
        package.display().to_string(),
        "/passive".into(),
        "/norestart".into(),
        "/l*v".into(),
        msi_log.display().to_string(),
    ]
}

/// Installs the .msi over the installed DNN Manager (its major upgrade removes the old version) and waits for it.
/// None when it worked.
fn run_msi(plan: &UpdatePlan, starter: &dyn Starter, log: &HelperLog) -> Option<String> {
    let msi_log = setup_log_of(plan);
    let args = msi_arguments(&plan.package, &msi_log);
    let msiexec = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join(r"System32\msiexec.exe");
    log.write(&format!("Running {} {}", msiexec.display(), args.join(" ")));
    let directory = plan.package.parent().unwrap_or(Path::new("."));
    let Ok(mut msi) = starter.start(&msiexec, &args, directory) else {
        return Some("The Windows Installer (msiexec) couldn't be started.".into());
    };
    let mut waited = 0u64;
    let code = loop {
        if let Some(code) = msi.wait(SETUP_NOTICE) {
            break code;
        }
        waited += SETUP_NOTICE.as_secs() / 60;
        log.write(&format!("The installer is still running after {waited} minutes - waiting for it ({}).", msi_log.display()));
    };
    log.write(&format!("The installer ended with exit code {code}."));
    msi_failure(code, &msi_log)
}

/// What msiexec's exit code means; None when it installed (3010 / 1641: installed, Windows wants a restart).
pub fn msi_failure(code: i32, msi_log: &Path) -> Option<String> {
    match code {
        0 | 3010 | 1641 => None,
        1602 => Some("The installation was cancelled.".into()),
        1618 => Some("Another installation is running - try the update again once it has finished.".into()),
        1925 | 1730 => Some("The installation needs administrator rights.".into()),
        _ => Some(format!("The installation failed (exit code {code}) - see {}.", msi_log.display())),
    }
}

fn launch(plan: &UpdatePlan, starter: &dyn Starter, log: &HelperLog) -> Option<Box<dyn Started>> {
    let directory = plan.app_exe.parent().unwrap_or(Path::new("."));
    match starter.start(&plan.app_exe, &[], directory) {
        Ok(started) => {
            log.write(&format!("Started {}.", plan.app_exe.display()));
            Some(started)
        }
        Err(e) => {
            log.write(&format!("Couldn't start {}: {e}", plan.app_exe.display()));
            None
        }
    }
}

fn finish(plan: &UpdatePlan, log: &HelperLog, installed: bool, message: &str) {
    log.write(message);
    // A failed update's evidence outlives the update's folder: "Show log" opens the kept copy.
    let mut log_file = plan.log_file.clone();
    if !installed && let Some(kept) = &plan.failed_log_file {
        match keep_failed_log(&plan.log_file, Some(&setup_log_of(plan)), kept) {
            Ok(()) => log_file = kept.clone(),
            Err(problem) => log.write(&format!("Couldn't keep the log as {}: {problem}", kept.display())),
        }
    }
    let result = UpdateResult { installed, message: message.into(), log_file: Some(log_file) };
    if let Err(e) = result.write(&plan.result_file) {
        log.write(&format!("Couldn't write {}: {e}", plan.result_file.display()));
    }
}

/// How many failed updates' logs are kept in the logs folder - the newest.
pub const FAILED_LOGS_KEPT: usize = 2;

/// Copies a failed update's log, and Setup's when there is one, into `destination`, and deletes all but the newest
/// [`FAILED_LOGS_KEPT`] such files beside it. A file already there is deleted and the copy made new - never written
/// through whatever was there (a link removed is only the link).
pub fn keep_failed_log(update_log: &Path, setup_log: Option<&Path>, destination: &Path) -> Result<(), String> {
    if !update_log.is_file() && !setup_log.is_some_and(Path::is_file) {
        return Err("there is no log to keep".into());
    }
    let folder = destination.parent().ok_or("the destination has no folder")?;
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        if std::fs::symlink_metadata(destination).is_ok() {
            std::fs::remove_file(destination)?;
        }
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(destination)?;
        write!(out, "# DNN Manager update that failed\r\n")?;
        for (title, file) in [("The update helper's log", Some(update_log)), ("Setup's log", setup_log)] {
            let Some(file) = file.filter(|f| f.is_file()) else { continue };
            write!(out, "\r\n## {title} ({})\r\n\r\n", file.display())?;
            let mut text = Vec::new();
            std::fs::File::open(file)?.read_to_end(&mut text)?;
            out.write_all(&text)?;
        }
        Ok(())
    };
    write().map_err(|e| e.to_string())?;
    let mut kept: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(folder)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.starts_with("update-failed-") && name.ends_with(".log")
        })
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH), e.path()))
        .collect();
    kept.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    for (_, old) in kept.into_iter().skip(FAILED_LOGS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// The file a failed update to `to_version` keeps its logs in, in `logs_directory`.
pub fn failed_log_path(logs_directory: &Path, to_version: &str) -> PathBuf {
    logs_directory.join(format!("update-failed-{to_version}.log"))
}

fn try_delete(path: &Path, log: &HelperLog) {
    if path.exists()
        && let Err(e) = std::fs::remove_file(path)
    {
        log.write(&format!("Couldn't delete {}: {e}", path.display()));
    }
}

struct HelperLog(PathBuf);

impl HelperLog {
    fn write(&self, line: &str) {
        let stamp = crate::win::local_parts(SystemTime::now())
            .map(|(y, mo, d, h, mi, s)| format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"))
            .unwrap_or_default();
        // Nowhere to say it when this fails.
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&self.0) {
            let _ = write!(file, "{stamp} {line}\r\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records what is started; each start ends with the next exit code (None: still running).
    struct Fake {
        started: RefCell<Vec<(PathBuf, Vec<String>)>>,
        exits: RefCell<Vec<Option<i32>>>,
    }

    struct Ended(Option<i32>);

    impl Started for Ended {
        fn wait(&mut self, _: Duration) -> Option<i32> {
            self.0
        }
    }

    impl Starter for Fake {
        fn start(&self, program: &Path, args: &[String], _: &Path) -> std::io::Result<Box<dyn Started>> {
            self.started.borrow_mut().push((program.to_path_buf(), args.to_vec()));
            let exits = &mut self.exits.borrow_mut();
            let code = if exits.is_empty() { None } else { exits.remove(0) };
            Ok(Box::new(Ended(code)))
        }
    }

    fn fake(exits: Vec<Option<i32>>) -> Fake {
        Fake { started: RefCell::new(vec![]), exits: RefCell::new(exits) }
    }

    fn plan(dir: &Path, new_bytes: &[u8]) -> UpdatePlan {
        let package = dir.join("DnnManager_Portable-1.9.0-x64.exe");
        std::fs::write(&package, new_bytes).unwrap();
        let app_exe = dir.join(r"app\DnnManager.exe");
        std::fs::create_dir_all(app_exe.parent().unwrap()).unwrap();
        std::fs::write(&app_exe, b"old").unwrap();
        UpdatePlan {
            kind: UpdateKind::Portable,
            package_sha256: sha256_file(&mut &new_bytes[..]).unwrap(),
            package_size: new_bytes.len() as u64,
            package,
            app_exe,
            all_users: false,
            // A pid that isn't running: "already closed".
            wait_for_process_id: u32::MAX - 1,
            from_version: "1.8.7".into(),
            to_version: "1.9.0".into(),
            log_file: dir.join("update.log"),
            result_file: dir.join("result.json"),
            failed_log_file: Some(dir.join(r"logs\update-failed-1.9.0.log")),
        }
    }

    #[test]
    fn swaps_the_portable_exe_and_deletes_the_backup_once_the_new_one_runs() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan(dir.path(), b"new version");
        let starter = fake(vec![None]);
        assert_eq!(run_plan(&plan, &starter, Duration::ZERO, Duration::from_secs(1)), 0);
        assert_eq!(std::fs::read(&plan.app_exe).unwrap(), b"new version");
        assert!(!backup_of(&plan).exists());
        assert!(!staged_of(&plan).exists());
        let result = UpdateResult::try_read(Some(&plan.result_file)).unwrap();
        assert!(result.installed);
        assert_eq!(result.message, "DNN Manager 1.9.0 was installed.");
        assert_eq!(starter.started.borrow()[0].0, plan.app_exe);
    }

    #[test]
    fn puts_the_old_exe_back_when_the_new_one_closes_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan(dir.path(), b"broken");
        let starter = fake(vec![Some(3), None]);
        assert_eq!(run_plan(&plan, &starter, Duration::ZERO, Duration::from_secs(1)), 1);
        assert_eq!(std::fs::read(&plan.app_exe).unwrap(), b"old");
        let result = UpdateResult::try_read(Some(&plan.result_file)).unwrap();
        assert_eq!(
            result.message,
            "DNN Manager 1.9.0 closed right after it started (exit code 3) - 1.8.7 was put back."
        );
        assert_eq!(result.log_file.as_deref(), plan.failed_log_file.as_deref(), "the kept log is the one to show");
        assert!(
            std::fs::read_to_string(plan.failed_log_file.as_ref().unwrap())
                .unwrap()
                .starts_with("# DNN Manager update that failed")
        );
        assert_eq!(starter.started.borrow().len(), 2, "the old one is started again");
    }

    #[test]
    fn a_package_that_isnt_the_releases_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = plan(dir.path(), b"new");
        plan.package_sha256 = "00".into();
        let starter = fake(vec![]);
        assert_eq!(run_plan(&plan, &starter, Duration::ZERO, Duration::from_secs(1)), 1);
        assert_eq!(std::fs::read(&plan.app_exe).unwrap(), b"old");
        let result = UpdateResult::try_read(Some(&plan.result_file)).unwrap();
        assert_eq!(
            result.message,
            "The downloaded update can't be used: its SHA-256 isn't the one GitHub lists - nothing was changed."
        );
    }

    #[test]
    fn runs_setup_with_inno_setups_switches_and_reads_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = plan(dir.path(), b"setup");
        plan.kind = UpdateKind::Installer;
        plan.all_users = true;
        let starter = fake(vec![Some(5), None]);
        assert_eq!(run_plan(&plan, &starter, Duration::ZERO, Duration::from_secs(1)), 1);
        let started = starter.started.borrow();
        assert_eq!(started[0].0, plan.package);
        assert_eq!(
            &started[0].1[..6],
            ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOCANCEL", "/SP-", "/ALLUSERS"]
        );
        assert!(started[0].1[6].starts_with("/LOG=") && started[0].1[6].ends_with("update.setup.log"));
        assert_eq!(UpdateResult::try_read(Some(&plan.result_file)).unwrap().message, "Setup was cancelled.");
    }

    #[test]
    fn installs_the_msi_with_msiexec_and_starts_dnn_manager_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = plan(dir.path(), b"msi");
        plan.kind = UpdateKind::Msi;
        let starter = fake(vec![Some(3010), None]);
        assert_eq!(run_plan(&plan, &starter, Duration::ZERO, Duration::from_secs(1)), 0);
        let started = starter.started.borrow();
        assert!(started[0].0.ends_with(r"System32\msiexec.exe"));
        assert_eq!(started[0].1[..4], ["/i".to_string(), plan.package.display().to_string(), "/passive".into(), "/norestart".into()]);
        assert_eq!(started[0].1[4], "/l*v");
        assert_eq!(started[1].0, plan.app_exe, "DNN Manager is started again");
        assert_eq!(std::fs::read(&plan.app_exe).unwrap(), b"old", "the installer replaces it, not the helper");
        assert!(UpdateResult::try_read(Some(&plan.result_file)).unwrap().installed);
        assert_eq!(msi_failure(1602, Path::new("m.log")).unwrap(), "The installation was cancelled.");
        assert!(msi_failure(1603, Path::new("m.log")).unwrap().starts_with("The installation failed (exit code 1603)"));
    }

    #[test]
    fn the_plan_reads_the_wpf_apps_json() {
        let json = r#"{ "Kind": "Portable", "Package": "C:\\t\\p.exe", "PackageSha256": "ab", "PackageSize": 2,
            "AppExe": "C:\\a\\DnnManager.exe", "AllUsers": false, "WaitForProcessId": 42, "FromVersion": "1.8.6",
            "ToVersion": "1.8.7", "LogFile": "C:\\t\\u.log", "ResultFile": "C:\\t\\r.json", "FailedLogFile": null }"#;
        let plan: UpdatePlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.kind, UpdateKind::Portable);
        assert_eq!(plan.wait_for_process_id, 42);
        assert!(serde_json::to_string(&plan).unwrap().contains("\"PackageSha256\":\"ab\""));
        assert!(is_helper(&[ARGUMENT.into(), "plan.json".into()]));
        assert!(!is_helper(&[ARGUMENT.into()]));
        assert_eq!(UpdateResult::try_read(None), None);
        assert_eq!(failed_log_path(Path::new(r"C:\logs"), "1.9.0"), PathBuf::from(r"C:\logs\update-failed-1.9.0.log"));
    }

    #[test]
    fn keeps_only_the_newest_failed_logs() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("u.log");
        std::fs::write(&log, "helper").unwrap();
        for v in ["1.0.0", "1.1.0", "1.2.0"] {
            keep_failed_log(&log, None, &failed_log_path(&dir.path().join("logs"), v)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = std::fs::read_dir(dir.path().join("logs")).unwrap().count();
        assert_eq!(left, FAILED_LOGS_KEPT);
        assert_eq!(
            keep_failed_log(&dir.path().join("none.log"), None, &dir.path().join("x.log")).unwrap_err(),
            "there is no log to keep"
        );
    }
}
