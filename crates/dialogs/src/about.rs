//! Settings → About for the GPUI build (`Pages/AboutInfo.cs`, `Diagnostics/MachineInfo.cs`): which DNN Manager this
//! is, what it runs on and what it works with. Windows, IIS and the .NET Framework are read from the registry with
//! `reg.exe query` (a System32 program, an argument list, a time limit) - off the UI thread, by [`read_machine`].
//!
//! Deviations: the build has no commit or build date stamped in yet, so "Build" and "Built" aren't shown; ".NET" is
//! the UI toolkit and language this build is made with; the libraries are the Rust ones that do the work of
//! Microsoft.Web.Administration (appcmd.exe) and Microsoft.Data.SqlClient (tiberius).

use std::collections::BTreeMap;
use std::time::Duration;

use dnnm_infrastructure::process;
use dnnm_sql::DockerStatus;

/// The dot beside a value (`Health`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    None,
    Ok,
    Warning,
    Unknown,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutRow {
    pub key: &'static str,
    pub value: String,
    pub health: Health,
    /// Said under the value - why GitHub couldn't be reached.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutSection {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub rows: Vec<AboutRow>,
}

impl AboutSection {
    fn new(title: &'static str, subtitle: &'static str) -> Self {
        Self { title, subtitle, rows: Vec::new() }
    }

    fn add(&mut self, key: &'static str, value: impl Into<String>, health: Health, detail: Option<String>) {
        self.rows.push(AboutRow { key, value: value.into(), health, detail });
    }
}

/// The update status as the app's updater has it (`AppUpdater.StatusText`, its state's dot, the problem when GitHub
/// couldn't be reached) - passed in by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub text: String,
    pub health: Health,
    pub problem: Option<String>,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self { text: "not checked yet".into(), health: Health::None, problem: None }
    }
}

/// What the registry says about this PC (`MachineInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineFacts {
    /// "Windows 11 Pro 24H2 (build 26100.4061)".
    pub windows: String,
    /// "IIS 10.0"; None when IIS isn't installed.
    pub iis: Option<String>,
    /// "4.8.1 (release 533320)"; None when not found.
    pub net_framework: Option<String>,
}

pub const REPOSITORY: &str = dnnm_system::updates::REPOSITORY;

/// The values of a `reg query` key: name → data, a `REG_DWORD` as its decimal number.
pub fn parse_reg_values(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter(|l| l.starts_with("    "))
        .filter_map(|line| {
            let mut parts = line.trim_start().splitn(3, "    ");
            let name = parts.next()?.trim().to_string();
            let kind = parts.next()?.trim();
            let data = parts.next().unwrap_or_default().trim();
            let data = match kind {
                "REG_DWORD" | "REG_QWORD" => {
                    u64::from_str_radix(data.trim_start_matches("0x"), 16).map(|n| n.to_string()).ok()?
                }
                _ => data.to_string(),
            };
            Some((name, data))
        })
        .collect()
}

fn reg_query(key: &str) -> BTreeMap<String, String> {
    match process::run(process::system32("reg.exe"), &["query", key], Duration::from_secs(10)) {
        Ok(output) if output.success() => parse_reg_values(&output.stdout),
        _ => BTreeMap::new(),
    }
}

/// `MachineInfo.Windows` from the values of `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
pub fn windows_text(values: &BTreeMap<String, String>) -> String {
    let mut product = values.get("ProductName").cloned().unwrap_or_else(|| "Windows".into());
    let build = values.get("CurrentBuild");
    // Windows 11 still says "Windows 10" in ProductName - its build number tells.
    if build.and_then(|b| b.parse::<u32>().ok()).is_some_and(|n| n >= 22000) {
        product = product.replace("Windows 10", "Windows 11");
    }
    let display = values.get("DisplayVersion").map(|d| format!(" {d}")).unwrap_or_default();
    let ubr = values.get("UBR").map(|u| format!(".{u}")).unwrap_or_default();
    let build = build.map(|b| format!(" (build {b}{ubr})")).unwrap_or_default();
    format!("{product}{display}{build}")
}

/// `MachineInfo.NetFramework` from the v4 Full key's `Release`.
pub fn net_framework_text(release: Option<u32>) -> Option<String> {
    let release = release?;
    let version = match release {
        r if r >= 533320 => "4.8.1",
        r if r >= 528040 => "4.8",
        r if r >= 461808 => "4.7.2",
        r if r >= 461308 => "4.7.1",
        r if r >= 460798 => "4.7",
        r if r >= 394802 => "4.6.2",
        _ => "4.6.1 or older",
    };
    Some(format!("{version} (release {release})"))
}

/// Reads Windows, IIS and the .NET Framework from the registry - a few `reg.exe` calls; not on the UI thread.
pub fn read_machine() -> MachineFacts {
    let windows = reg_query(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let iis = reg_query(r"HKLM\SOFTWARE\Microsoft\InetStp");
    let net = reg_query(r"HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full");
    MachineFacts {
        windows: windows_text(&windows),
        iis: iis
            .get("MajorVersion")
            .map(|major| format!("IIS {major}.{}", iis.get("MinorVersion").map(String::as_str).unwrap_or("0"))),
        net_framework: net_framework_text(net.get("Release").and_then(|r| r.parse().ok())),
    }
}

/// Docker as the Docker container card sees it (`AboutInfo.DockerAsync`).
pub fn docker_text(status: &Result<DockerStatus, String>) -> String {
    match status {
        Ok(d) if !d.desktop_installed => "not installed".into(),
        Ok(d) => {
            let client = d.client_version.clone().unwrap_or_else(|| "installed".into());
            if d.engine_running {
                format!("{client} - engine {} running", d.engine_version.clone().unwrap_or_default())
            } else {
                format!("{client} - engine not running")
            }
        }
        Err(e) => e.clone(),
    }
}

fn architecture() -> String {
    // The OS's, not this process's: an x64 process on ARM64 Windows says so.
    let os = std::env::var("PROCESSOR_ARCHITEW6432")
        .or_else(|_| std::env::var("PROCESSOR_ARCHITECTURE"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let os = match os.as_str() {
        "amd64" => "x64",
        "arm64" => "ARM64",
        "x86" => "x86",
        "" => "unknown",
        other => return format!("{} process on {other} Windows", process_architecture()),
    };
    format!("{} process on {os} Windows", process_architecture())
}

fn process_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => other,
    }
}

/// The sections About shows. `machine` and `docker` are None while they are being read ("checking…").
pub fn sections(update: &UpdateStatus, machine: Option<&MachineFacts>, docker: Option<&str>) -> Vec<AboutSection> {
    let version = env!("CARGO_PKG_VERSION");
    let mut app = AboutSection::new("DNN Manager", "the program");
    app.add("Version", version, Health::None, None);
    if let Some(folder) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.display().to_string())) {
        app.add("Program folder", folder, Health::None, None);
    }
    let channel = if cfg!(debug_assertions) {
        "development (Debug build)"
    } else if version.contains('-') {
        "pre-release"
    } else {
        "stable"
    };
    app.add("Release channel", channel, Health::None, None);
    app.add("Update", update.text.clone(), update.health, update.problem.clone());
    app.add("License", "MIT", Health::None, None);
    app.add("Repository", REPOSITORY, Health::None, None);
    app.add("Documentation", format!("{REPOSITORY}/tree/main/.docs"), Health::None, None);

    let mut runtime = AboutSection::new("Runs on", "this PC");
    runtime.add("UI", "GPUI 0.2 (Rust)", Health::None, None);
    runtime.add("Architecture", architecture(), Health::None, None);
    runtime.add("Windows", machine.map_or_else(|| "checking…".to_string(), |m| m.windows.clone()), Health::None, None);
    let admin = dnnm_infrastructure::windows_security::is_elevated();
    runtime.add(
        "Administrator",
        if admin { "yes - it manages IIS" } else { "no" },
        if admin { Health::None } else { Health::Bad },
        None,
    );

    let mut components = AboutSection::new("Works with", "Windows and its own libraries");
    match machine {
        Some(m) => {
            components.add(
                "IIS",
                m.iis.clone().unwrap_or_else(|| "not installed".into()),
                if m.iis.is_none() { Health::Bad } else { Health::None },
                None,
            );
            components.add(
                ".NET Framework (sites)",
                m.net_framework.clone().unwrap_or_else(|| "not found".into()),
                if m.net_framework.is_none() { Health::Bad } else { Health::None },
                None,
            );
        }
        None => {
            components.add("IIS", "checking…", Health::None, None);
            components.add(".NET Framework (sites)", "checking…", Health::None, None);
        }
    }
    components.add("Docker", docker.unwrap_or("checking…"), Health::None, None);
    components.add("IIS configuration", r"appcmd.exe (%windir%\System32\inetsrv)", Health::None, None);
    components.add("SQL Server client", "tiberius 0.12", Health::None, None);
    vec![app, runtime, components]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reg_query_output() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\r\n    ProductName    REG_SZ    Windows 10 Pro\r\n    CurrentBuild    REG_SZ    26100\r\n    DisplayVersion    REG_SZ    24H2\r\n    UBR    REG_DWORD    0xfdd\r\n";
        let values = parse_reg_values(out);
        assert_eq!(values["UBR"], "4061");
        assert_eq!(windows_text(&values), "Windows 11 Pro 24H2 (build 26100.4061)");
        assert_eq!(windows_text(&BTreeMap::new()), "Windows");
    }

    #[test]
    fn names_the_net_framework_by_its_release() {
        assert_eq!(net_framework_text(Some(533320)).as_deref(), Some("4.8.1 (release 533320)"));
        assert_eq!(net_framework_text(Some(528049)).as_deref(), Some("4.8 (release 528049)"));
        assert_eq!(net_framework_text(Some(1)).as_deref(), Some("4.6.1 or older (release 1)"));
        assert_eq!(net_framework_text(None), None);
    }

    #[test]
    fn says_docker_as_the_card_sees_it() {
        assert_eq!(docker_text(&Ok(DockerStatus::default())), "not installed");
        let running = DockerStatus {
            desktop_installed: true,
            client_version: Some("29.8.1".into()),
            engine_running: true,
            engine_version: Some("29.8.1".into()),
            ..Default::default()
        };
        assert_eq!(docker_text(&Ok(running)), "29.8.1 - engine 29.8.1 running");
        let stopped = DockerStatus { desktop_installed: true, ..Default::default() };
        assert_eq!(docker_text(&Ok(stopped)), "installed - engine not running");
    }

    #[test]
    fn sections_show_checking_until_read() {
        let s = sections(&UpdateStatus::default(), None, None);
        assert_eq!(s.iter().map(|s| s.title).collect::<Vec<_>>(), vec!["DNN Manager", "Runs on", "Works with"]);
        assert!(s[2].rows.iter().any(|r| r.key == "Docker" && r.value == "checking…"));
        assert!(s[0].rows.iter().any(|r| r.key == "License" && r.value == "MIT"));
    }
}
