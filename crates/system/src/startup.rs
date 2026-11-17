//! "Start DNN Manager when you sign in" (`Startup/StartupTask.cs`) and what its launcher starts it with
//! (`Startup/LaunchEnvironment.cs`).
//!
//! A scheduled task starts DNN Manager at the user's logon with the highest privileges - a Run key or the Startup
//! folder would ask for Administrator rights with a UAC prompt at every sign-in. Only for a DNN Manager that nobody but
//! administrators can change (installed for all users, in Program Files): the task would start whatever another program
//! put in its place, with administrator rights and without asking. And only at sign-in - not on demand.
//!
//! The launcher (`DnnManager-launcher.exe`, beside an installed WPF `DnnManager.exe`) drops the .NET variables that
//! would load a profiler into the elevated app. A Rust exe has no CoreCLR to read them; the launcher's rules are ported
//! because the task finds and starts the launcher of an installed DNN Manager either way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_infrastructure::process::{self, ProcessOutput};

use crate::files::private_temp;
use crate::trusted::{Trust, admin_only, is_elevated};

// ─── LaunchEnvironment ──────────────────────────────────────────────────

/// The launcher's file name, beside [`APP_FILE_NAME`] in an installed DNN Manager.
pub const LAUNCHER_FILE_NAME: &str = "DnnManager-launcher.exe";

/// The program the launcher starts.
pub const APP_FILE_NAME: &str = "DnnManager.exe";

// Every variable starting so is .NET's: the runtime's, the host's, and the profiling API's.
const DROPPED_PREFIXES: [&str; 4] = ["DOTNET_", "COMPlus_", "COR_", "CORECLR_"];

/// Whether the launcher leaves `name` out.
pub fn is_dropped(name: &str) -> bool {
    DROPPED_PREFIXES.iter().any(|p| name.len() >= p.len() && name[..p.len()].eq_ignore_ascii_case(p))
}

/// `environment` without .NET's variables, and with its diagnostics switched off (`DOTNET_EnableDiagnostics=0`) - set,
/// not only left out. Names compare without case, as Windows' do; the result is keyed by the lower-case name and keeps
/// each variable's own spelling.
pub fn clean(environment: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, (String, String)> {
    let mut clean = BTreeMap::new();
    for (name, value) in environment {
        if !name.is_empty() && !is_dropped(&name) {
            clean.insert(name.to_lowercase(), (name, value));
        }
    }
    clean.insert("dotnet_enablediagnostics".into(), ("DOTNET_EnableDiagnostics".into(), "0".into()));
    clean
}

/// The launcher beside `app_exe` (DNN Manager's exe), or None when there is none - a portable DNN Manager, or a
/// development build, has none and is started as it is.
pub fn launcher_beside(app_exe: Option<&Path>) -> Option<PathBuf> {
    let exe = app_exe?;
    if !exe.file_name()?.to_string_lossy().eq_ignore_ascii_case(APP_FILE_NAME) {
        return None;
    }
    let launcher = exe.parent()?.join(LAUNCHER_FILE_NAME);
    launcher.is_file().then_some(launcher)
}

// ─── StartupTask ────────────────────────────────────────────────────────

pub const TASK_NAME: &str = "DNN Manager";

const LIMIT: Duration = Duration::from_secs(30);

fn schtasks() -> PathBuf {
    process::system32("schtasks.exe")
}

/// The DNN Manager exe the task starts, or None when there is no task (Some("") when it can't be read). A task that
/// starts the launcher counts as starting the DnnManager.exe beside it.
pub fn get_target() -> Option<PathBuf> {
    let command = get_command()?;
    Some(if command.as_os_str().is_empty() { command } else { app_of(&command) })
}

/// The exe in the task's action as it is written, or None when there is no task.
fn get_command() -> Option<PathBuf> {
    let query = process::run(schtasks(), &["/Query", "/TN", TASK_NAME, "/XML"], LIMIT).ok()?;
    if !query.success() {
        return None;
    }
    Some(command_in(&query.stdout).map(PathBuf::from).unwrap_or_default())
}

/// `<Command>C:\…\DnnManager-launcher.exe</Command>` in a task's XML, decoded - None when there is none.
pub fn command_in(xml: &str) -> Option<String> {
    // schtasks may write UTF-16: read as UTF-8, every other byte is a NUL.
    let xml = xml.replace('\0', "");
    let lower = xml.to_ascii_lowercase();
    let start = lower.find("<command>")? + "<command>".len();
    let end = start + lower[start..].find("</command>")?;
    Some(xml_decode(&xml[start..end]).trim().trim_matches('"').to_string())
}

fn xml_decode(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(end) = after.find(';') else {
            out.push_str(after);
            return out;
        };
        let entity = &after[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `SecurityElement.Escape`.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The DnnManager.exe a task's command starts: the one beside the launcher, or the command itself.
pub fn app_of(command: &Path) -> PathBuf {
    let is_launcher = command.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(LAUNCHER_FILE_NAME));
    if is_launcher { command.parent().unwrap_or(Path::new("")).join(APP_FILE_NAME) } else { command.to_path_buf() }
}

/// What the task runs for `exe_path`: the launcher beside it when there is one, else the exe itself.
pub fn command_for(exe_path: &Path) -> PathBuf {
    launcher_beside(Some(exe_path)).unwrap_or_else(|| exe_path.to_path_buf())
}

/// Why `exe_path` can't be started by the task, or None when it can.
pub fn refusal(exe_path: &Path) -> Option<String> {
    if !is_elevated() {
        return None;
    }
    refusal_for(exe_path, trust_of(&[exe_path, &command_for(exe_path)]))
}

/// Whether only administrators can change what the task runs - each of `files` (DnnManager.exe and its launcher).
fn trust_of(files: &[&Path]) -> Trust {
    let trust: Vec<Trust> = files.iter().map(|f| admin_only(f)).collect();
    if trust.contains(&Trust::OthersCanChange) {
        Trust::OthersCanChange
    } else if trust.contains(&Trust::Unknown) {
        Trust::Unknown
    } else {
        Trust::AdminOnly
    }
}

pub fn refusal_for(exe_path: &Path, trust: Trust) -> Option<String> {
    let exe = exe_path.display();
    match trust {
        Trust::AdminOnly => None,
        Trust::Unknown => Some(format!(
            "Who may change {exe} couldn't be read just now (a file in use?) - the task starts it with administrator rights, so it is set up only once that is known. Try again in a moment."
        )),
        Trust::OthersCanChange => Some(format!(
            "{exe} is in a folder that programs without administrator rights can change, and the task would start whatever is put there with administrator rights, without asking. Install DNN Manager for all users (its Setup does, in Program Files) to start it when you sign in."
        )),
    }
}

/// The task's definition: at `user`'s logon, with the highest rights, no time limit, also on battery, never a second
/// instance, not on demand.
pub fn task_xml(user: &str, command: &Path) -> String {
    let user = xml_escape(user);
    let working = command.parent().map(|p| p.display().to_string()).unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts DNN Manager when {user} signs in (Settings - General in DNN Manager).</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <AllowStartOnDemand>false</AllowStartOnDemand>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>"#,
        xml_escape(&command.display().to_string()),
        xml_escape(&working)
    )
}

/// The signed-in user as `DOMAIN\user` (`WindowsIdentity.GetCurrent().Name`).
pub fn current_user_name() -> Option<String> {
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
    let mut buffer = vec![0u16; 512];
    let mut size = buffer.len() as u32;
    // SAFETY: the buffer's length is passed in and the result's read back.
    let ok = unsafe { GetUserNameExW(NameSamCompatible, Some(windows::core::PWSTR(buffer.as_mut_ptr())), &mut size) };
    ok.then(|| String::from_utf16_lossy(&buffer[..size as usize]))
}

/// Registers the task (replacing one that is there) to start `exe_path` when this user signs in - through the launcher
/// beside it, when there is one.
pub fn enable(exe_path: &Path) -> Result<(), String> {
    if let Some(refusal) = refusal(exe_path) {
        return Err(refusal);
    }
    let command = command_for(exe_path);
    let user = current_user_name().ok_or("The signed-in user's name can't be read.")?;
    let xml = task_xml(&user, &command);
    // Not %TEMP%: schtasks reads it back, and the task it makes runs with the highest rights at every sign-in.
    let folder = private_temp::path().map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let file = folder.join(format!("dnnmanager-startup-{}-{nonce:x}.xml", std::process::id()));
    // UTF-16 with its byte order mark, as the XML says.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let result = std::fs::write(&file, bytes).map_err(|e| e.to_string()).and_then(|()| {
        let path = file.display().to_string();
        let create = process::run(schtasks(), &["/Create", "/TN", TASK_NAME, "/XML", &path, "/F"], LIMIT)?;
        if create.success() { Ok(()) } else { Err(message(&create)) }
    });
    let _ = std::fs::remove_file(&file);
    result
}

/// Removes a task that starts a DNN Manager others could change - with the reason, or None when there is none such. A
/// safe task that starts DnnManager.exe itself is made again to start the launcher beside it, without a word.
pub fn remove_if_unsafe() -> Option<String> {
    if !is_elevated() {
        return None;
    }
    let command = get_command().filter(|c| !c.as_os_str().is_empty())?;
    let target = app_of(&command);
    // Removed only when others can change it for sure: a read that failed is asked once more.
    let mut trust = trust_of(&[&target, &command]);
    if trust == Trust::Unknown {
        std::thread::sleep(Duration::from_secs(2));
        trust = trust_of(&[&target, &command]);
    }
    match trust {
        Trust::Unknown => None,
        Trust::AdminOnly => {
            if !command_for(&target).as_os_str().eq_ignore_ascii_case(command.as_os_str()) {
                let _ = enable(&target);
            }
            None
        }
        Trust::OthersCanChange => {
            let refusal = refusal_for(&target, trust).unwrap_or_default();
            Some(match disable() {
                Ok(()) => format!("\"Start DNN Manager when you sign in\" is switched off: {refusal}"),
                Err(e) => format!(
                    "\"Start DNN Manager when you sign in\" should be switched off - {refusal} Removing its scheduled task failed: {e}"
                ),
            })
        }
    }
}

/// Removes the task; fine when there is none.
pub fn disable() -> Result<(), String> {
    if get_target().is_none() {
        return Ok(());
    }
    let delete = process::run(schtasks(), &["/Delete", "/TN", TASK_NAME, "/F"], LIMIT)?;
    if delete.success() { Ok(()) } else { Err(message(&delete)) }
}

fn message(output: &ProcessOutput) -> String {
    let text = if output.stderr.trim().is_empty() { &output.stdout } else { &output.stderr };
    let text = text.replace('\0', "");
    let text = text.trim();
    if text.is_empty() {
        format!("schtasks failed (exit code {}).", output.exit_code.unwrap_or(-1))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_every_dotnet_variable_and_switches_diagnostics_off() {
        let env = clean([
            ("Path".to_string(), r"C:\Windows".to_string()),
            ("CORECLR_PROFILER".to_string(), "{x}".to_string()),
            ("dotnet_StartupHooks".to_string(), "x.dll".to_string()),
            ("COMPlus_EnableDiagnostics".to_string(), "1".to_string()),
            ("COR_ENABLE_PROFILING".to_string(), "1".to_string()),
        ]);
        let names: Vec<&str> = env.values().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["DOTNET_EnableDiagnostics", "Path"]);
        assert_eq!(env["dotnet_enablediagnostics"].1, "0");
        assert!(is_dropped("DOTNET_x") && !is_dropped("DOT"));
    }

    #[test]
    fn the_launcher_counts_only_beside_dnn_manager_exe() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(APP_FILE_NAME);
        assert_eq!(launcher_beside(Some(&exe)), None);
        std::fs::write(dir.path().join(LAUNCHER_FILE_NAME), "MZ").unwrap();
        assert_eq!(launcher_beside(Some(&exe)), Some(dir.path().join(LAUNCHER_FILE_NAME)));
        assert_eq!(launcher_beside(Some(&dir.path().join("other.exe"))), None);
        assert_eq!(command_for(&exe), dir.path().join(LAUNCHER_FILE_NAME));
        assert_eq!(app_of(&dir.path().join(LAUNCHER_FILE_NAME)), exe);
        assert_eq!(app_of(&exe), exe);
    }

    #[test]
    fn reads_the_command_from_the_tasks_xml_also_in_utf16() {
        let xml = task_xml(r"PC\Ann & Bob", Path::new(r"C:\Program Files\DNN Manager\DnnManager-launcher.exe"));
        assert!(xml.contains(r"<UserId>PC\Ann &amp; Bob</UserId>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains(r"<WorkingDirectory>C:\Program Files\DNN Manager</WorkingDirectory>"));
        assert_eq!(command_in(&xml).as_deref(), Some(r"C:\Program Files\DNN Manager\DnnManager-launcher.exe"));
        let utf16: String = xml.chars().flat_map(|c| [c, '\0']).collect();
        assert_eq!(command_in(&utf16).as_deref(), Some(r"C:\Program Files\DNN Manager\DnnManager-launcher.exe"));
        assert_eq!(command_in("<Task/>"), None);
        assert_eq!(xml_decode("&quot;C:\\a&#x26;b&#38;c&quot;"), "\"C:\\a&b&c\"");
    }

    #[test]
    fn refuses_a_dnn_manager_others_can_change() {
        let exe = Path::new(r"C:\Users\x\AppData\Local\Programs\DNN Manager\DnnManager.exe");
        assert!(
            refusal_for(exe, Trust::OthersCanChange).unwrap().starts_with(&format!("{} is in a folder", exe.display()))
        );
        assert!(refusal_for(exe, Trust::Unknown).unwrap().contains("couldn't be read just now"));
        assert_eq!(refusal_for(exe, Trust::AdminOnly), None);
        assert!(current_user_name().unwrap().contains('\\'));
    }
}
