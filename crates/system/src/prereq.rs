//! What DNN Manager needs on the PC (`Prereq/WindowsPrerequisiteChecker.cs`): IIS's Windows features - read and
//! enabled with PowerShell's `Get-` / `Enable-WindowsOptionalFeature` (DISM) in one process for all of them - and
//! Docker Desktop: its status from `docker version` / `docker ps`, installed with winget, started through Explorer so it
//! runs as the signed-in user.
//!
//! Deviations, because `dnnm_infrastructure::process::run` takes no environment and gives the output at the end:
//! - The feature names reach the script as single-quoted literals, not through `DNNMANAGER_FEATURES` in its
//!   environment. Only names that pass [`is_iis_feature_name`] (letters, digits, '-', '_', '.') are used, so none can
//!   end the quote.
//! - winget's progress lines are reported when it has ended, not as it runs (a 30-minute limit).
//! - docker and winget run with DNN Manager's environment; the C#'s `ChildEnvironment.Apply` (the computer's PATH, a
//!   private DOCKER_CONFIG) needs a runner that takes an environment.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use dnnm_application::operation::{Operation, Reporter};
use dnnm_infrastructure::process::{self, ProcessOutput};
#[cfg(windows)]
use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_ProgramFiles, FOLDERID_Windows};

use crate::trusted;
#[cfg(windows)]
use crate::win::known_folder;

/// What the IIS feature check says on macOS and Linux.
#[cfg(not(windows))]
const IIS_ONLY_ON_WINDOWS: &str = "IIS and its Windows features exist only on Windows.";

/// An IIS Windows feature Settings → IIS checks (and can enable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisFeature {
    pub name: String,
    pub label: String,
}

/// `IisSettings.RequiredFeatures`' defaults.
pub fn default_features() -> Vec<IisFeature> {
    [
        ("IIS-WebServerRole", "IIS Web Server"),
        ("IIS-WebServer", "World Wide Web Services"),
        ("IIS-ManagementConsole", "IIS Management Console"),
        ("IIS-NetFxExtensibility", ".NET Extensibility 3.5"),
        ("IIS-NetFxExtensibility45", ".NET Extensibility 4.8"),
        ("IIS-ASPNET", "ASP.NET 3.5"),
        ("IIS-ASPNET45", "ASP.NET 4.8"),
        ("IIS-ISAPIExtensions", "ISAPI Extensions"),
        ("IIS-ISAPIFilter", "ISAPI Filters"),
        ("IIS-DefaultDocument", "Default Document"),
        ("IIS-DirectoryBrowsing", "Directory Browsing"),
        ("IIS-HttpErrors", "HTTP Errors"),
        ("IIS-StaticContent", "Static Content"),
        ("IIS-BasicAuthentication", "Basic Authentication"),
        ("IIS-RequestFiltering", "Request Filtering"),
        ("IIS-HostableWebCore", "IIS Hostable Web Core"),
    ]
    .into_iter()
    .map(|(name, label)| IisFeature { name: name.into(), label: label.into() })
    .collect()
}

/// A Windows feature name (`SettingRules.IsIisFeatureName`): letters, digits, '-', '_' and '.', at most 128.
pub fn is_iis_feature_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

const QUERY: &str = "(Get-WindowsOptionalFeature -Online -FeatureName $n -ErrorAction SilentlyContinue).State";
#[cfg(windows)]
const ENABLE: &str = "try { $o = Enable-WindowsOptionalFeature -Online -FeatureName $n -All -NoRestart -ErrorAction Stop; if ($o.RestartNeeded) { 'RESTART' } else { 'OK' } } catch { 'FAIL' }";

/// The script that runs `per_feature` (with the name in `$n`) for every valid name; None when none is valid.
pub fn feature_script(per_feature: &str, features: &[IisFeature]) -> Option<String> {
    let valid: Vec<&str> = features.iter().map(|f| f.name.as_str()).filter(|n| is_iis_feature_name(n)).collect();
    for f in features.iter().filter(|f| !is_iis_feature_name(&f.name)) {
        log::warn!("Skipped the IIS feature '{}': not a Windows feature name", f.name);
    }
    if valid.is_empty() {
        return None;
    }
    let list = valid.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(",");
    Some(format!("foreach ($n in @({list})) {{ $r = {per_feature}; \"$n=$r\" }}"))
}

/// The script's `name=value` lines, by name (any case).
pub fn parse_feature_output(stdout: &str) -> HashMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| {
            let at = line.find('=').filter(|&i| i > 0)?;
            Some((line[..at].trim().to_lowercase(), line[at + 1..].trim().to_string()))
        })
        .collect()
}

#[cfg(not(windows))]
fn run_per_feature(_: &[IisFeature], _: &str, _: Duration) -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(windows)]
fn run_per_feature(features: &[IisFeature], per_feature: &str, limit: Duration) -> HashMap<String, String> {
    let Some(script) = feature_script(per_feature, features) else { return HashMap::new() };
    let powershell = process::system32(r"WindowsPowerShell\v1.0\powershell.exe");
    match process::run(powershell, &["-NoProfile", "-NonInteractive", "-Command", &script], limit) {
        Ok(output) => {
            if !output.success() {
                log::warn!("IIS feature script failed: {}", output.stderr.trim());
            }
            parse_feature_output(&output.stdout)
        }
        Err(e) => {
            log::warn!("IIS feature script failed: {e}");
            HashMap::new()
        }
    }
}

fn is_enabled(states: &HashMap<String, String>, name: &str) -> bool {
    states.get(&name.to_lowercase()).is_some_and(|s| s.eq_ignore_ascii_case("Enabled"))
}

/// Whether each feature is enabled, by name.
pub fn get_iis_feature_states(features: &[IisFeature]) -> BTreeMap<String, bool> {
    let states = run_per_feature(features, QUERY, Duration::from_secs(120));
    features.iter().map(|f| (f.name.clone(), is_enabled(&states, &f.name))).collect()
}

/// Checks the features and offers to enable the missing ones. Missing isn't a failure yet - only what still doesn't
/// work afterwards is, so a stage whose missing features were enabled doesn't end as failed.
#[cfg(not(windows))]
pub fn ensure_iis_features(_: &[IisFeature], _: &mut Operation) -> Result<(), String> {
    Err(IIS_ONLY_ON_WINDOWS.into())
}

/// Checks the features and offers to enable the missing ones (Windows).
#[cfg(windows)]
pub fn ensure_iis_features(features: &[IisFeature], op: &mut Operation) -> Result<(), String> {
    if features.is_empty() {
        op.report.info("No IIS features configured to check.");
        return Ok(());
    }
    let states = run_per_feature(features, QUERY, Duration::from_secs(120));
    let mut missing = Vec::new();
    for f in features {
        if is_enabled(&states, &f.name) {
            op.report.success(&format!("{} ({})", f.label, f.name));
        } else {
            missing.push(f.clone());
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    op.report.info("Missing IIS features:");
    for f in &missing {
        op.report.info(&format!("   {} ({})", f.label, f.name));
    }
    let question = format!("{} IIS feature(s) are missing - see the Output panel. Enable them now?", missing.len());
    if !op.prompt.confirm(&question, "Enable features", "Not now", true) {
        for f in &missing {
            op.report.fail(&format!("Not enabled: {} ({})", f.label, f.name));
        }
        return Err("IIS features missing.".into());
    }
    op.cancel.check()?;
    op.report.info(&format!("Enabling {} feature(s)…", missing.len()));
    let enabled = run_per_feature(&missing, ENABLE, Duration::from_secs(30 * 60));
    report_enabled(op.report, &missing, &enabled);
    Ok(())
}

#[cfg(windows)]
fn report_enabled(report: &mut dyn Reporter, missing: &[IisFeature], enabled: &HashMap<String, String>) {
    let (mut failed, mut restart) = (0, false);
    for f in missing {
        match enabled.get(&f.name.to_lowercase()).map(String::as_str) {
            Some("OK") => report.success(&format!("Enabled {}", f.label)),
            Some("RESTART") => {
                report.success(&format!("Enabled {}", f.label));
                restart = true;
            }
            _ => {
                report.fail(&format!("Could not enable {} ({})", f.label, f.name));
                failed += 1;
            }
        }
    }
    // Said only when Windows says so - not "a reboot may be required" after every change.
    if restart {
        report
            .warn("Windows needs a restart to finish enabling IIS features - restart the PC if the site doesn't open.");
    } else if failed == 0 {
        report.success("IIS features enabled.");
    }
}

// ─── Docker Desktop ─────────────────────────────────────────────────────

/// What `docker version` and `docker ps` say.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerStatus {
    pub installed: bool,
    pub client_version: Option<String>,
    pub running: bool,
    pub server_version: Option<String>,
    pub container_state: Option<String>,
    pub container_status: Option<String>,
}

/// Installed for all users (Program Files), or for this account only (%LOCALAPPDATA%\Programs).
#[cfg(not(windows))]
pub fn docker_desktop_exe() -> PathBuf {
    PathBuf::new()
}

/// Installed for all users (Program Files), or for this account only (%LOCALAPPDATA%\Programs).
#[cfg(windows)]
pub fn docker_desktop_exe() -> PathBuf {
    let all_users = known_folder(&FOLDERID_ProgramFiles)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
        .join(r"Docker\Docker\Docker Desktop.exe");
    let mine = known_folder(&FOLDERID_LocalAppData).map(|l| l.join(r"Programs\DockerDesktop\Docker Desktop.exe"));
    [Some(all_users.clone()), mine].into_iter().flatten().find(|p| p.is_file()).unwrap_or(all_users)
}

fn docker(args: &[&str]) -> Option<ProcessOutput> {
    let program = trusted::resolve("docker").path?;
    // Docker Desktop half started can leave the CLI waiting for ever: a question gets half a minute.
    process::run(program, args, Duration::from_secs(30)).ok()
}

/// `client|server` from `docker version --format` - the client prints even when the engine is down.
pub fn parse_docker_version(stdout: &str) -> (String, String) {
    let mut parts = stdout.trim().split('|');
    let client = parts.next().unwrap_or_default().trim().to_string();
    let server = parts.next().unwrap_or_default().trim().to_string();
    (client, server)
}

/// The first `state|status` line of `docker ps --format`.
pub fn parse_container(stdout: &str) -> (Option<String>, Option<String>) {
    let Some(line) = stdout.lines().map(str::trim).find(|l| !l.is_empty()) else { return (None, None) };
    let mut parts = line.split('|');
    (parts.next().map(str::to_string), parts.next().map(str::to_string))
}

/// Docker's CLI and engine, and the container `container_name`.
pub fn get_docker_status(container_name: &str) -> DockerStatus {
    let version = docker(&["version", "--format", "{{.Client.Version}}|{{.Server.Version}}"]);
    let (client, server) = version.as_ref().map(|v| parse_docker_version(&v.stdout)).unwrap_or_default();
    let installed = !client.is_empty() || docker_desktop_exe().is_file();
    let running = version.as_ref().is_some_and(ProcessOutput::success) && !server.is_empty();
    if !running {
        return DockerStatus {
            installed,
            client_version: (!client.is_empty()).then_some(client),
            ..Default::default()
        };
    }
    let filter = format!("name=^{container_name}$");
    let ps = docker(&["ps", "-a", "--filter", &filter, "--format", "{{.State}}|{{.Status}}"]);
    let (state, status) = ps.filter(ProcessOutput::success).map(|p| parse_container(&p.stdout)).unwrap_or_default();
    DockerStatus {
        installed: true,
        client_version: Some(client),
        running: true,
        server_version: Some(server),
        container_state: state,
        container_status: status,
    }
}

/// Installs Docker Desktop with winget.
pub fn install_docker_desktop(report: &mut dyn Reporter) -> Result<(), String> {
    report.info("Installing Docker Desktop with winget - this downloads about 600 MB and can take several minutes…");
    let instead = "Install Docker Desktop from https://www.docker.com/products/docker-desktop/ instead.";
    let resolved = trusted::resolve("winget");
    let Some(winget) = resolved.path.clone() else { return Err(format!("{} {instead}", resolved.problem("winget"))) };
    let args = [
        "install",
        "--id",
        "Docker.DockerDesktop",
        "-e",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--disable-interactivity",
    ];
    let r = process::run(winget, &args, Duration::from_secs(30 * 60)).map_err(|e| format!("{e} {instead}"))?;
    install_outcome(report, &r)
}

fn install_outcome(report: &mut dyn Reporter, r: &ProcessOutput) -> Result<(), String> {
    if !r.success() {
        let output = format!("{}{}", r.stdout, r.stderr);
        let output = output.trim();
        // winget exits non-zero when the package is already installed and there is no newer version.
        if output.to_lowercase().contains("already installed") {
            report.info("winget says Docker Desktop is already installed - nothing to install.");
            return Ok(());
        }
        let lines: Vec<&str> = output.split('\n').collect();
        let last: Vec<&str> = lines[lines.len().saturating_sub(3)..].iter().map(|l| l.trim()).collect();
        return Err(format!(
            "winget couldn't install Docker Desktop (exit {}): {}",
            r.exit_code.unwrap_or(-1),
            last.join(" ")
        ));
    }
    for line in r.stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        report.info(line);
    }
    report.success(
        "Docker Desktop installed. Start it - the first time it may ask to enable WSL 2 or to sign out and in again.",
    );
    Ok(())
}

/// Starts Docker Desktop through Explorer, so it runs as the signed-in user rather than elevated like DNN Manager.
#[cfg(not(windows))]
pub fn start_docker_desktop() -> Result<(), String> {
    Err("Starting Docker Desktop from DNN Manager works only on Windows - start Docker yourself.".into())
}

/// Starts Docker Desktop through Explorer, so it runs as the signed-in user rather than elevated like DNN Manager.
#[cfg(windows)]
pub fn start_docker_desktop() -> Result<(), String> {
    let exe = docker_desktop_exe();
    if !exe.is_file() {
        return Err(format!("Docker Desktop not found at {}.", exe.display()));
    }
    // Explorer by its full path: a bare name would be looked up in DNN Manager's own folder first.
    let explorer = known_folder(&FOLDERID_Windows).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("explorer.exe");
    let target = exe.display().to_string();
    // Explorer hands the program over and ends; its exit code says nothing.
    process::run(explorer, &[&target], Duration::from_secs(30))
        .map(|_| ())
        .map_err(|e| format!("Could not start Docker Desktop: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Answer, Lines};
    use dnnm_application::operation::CancelToken;

    #[test]
    fn only_windows_feature_names_reach_the_script() {
        assert!(is_iis_feature_name("IIS-ASPNET45"));
        assert!(!is_iis_feature_name("IIS'; Remove-Item C:\\ -Recurse; '"));
        assert!(!is_iis_feature_name(""));
        let features = vec![
            IisFeature { name: "IIS-WebServer".into(), label: "Web".into() },
            IisFeature { name: "bad name'".into(), label: "x".into() },
        ];
        assert_eq!(
            feature_script(QUERY, &features).unwrap(),
            format!("foreach ($n in @('IIS-WebServer')) {{ $r = {QUERY}; \"$n=$r\" }}")
        );
        assert_eq!(feature_script(QUERY, &features[1..]), None);
    }

    #[test]
    fn parses_the_scripts_lines() {
        let states =
            parse_feature_output("IIS-WebServer=Enabled\r\nIIS-ASPNET=Disabled\r\nnoise\r\n=x\r\nIIS-Foo=\r\n");
        assert!(is_enabled(&states, "iis-webserver"));
        assert!(!is_enabled(&states, "IIS-ASPNET"));
        assert!(!is_enabled(&states, "IIS-Foo"));
        assert_eq!(states.len(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn says_a_restart_only_when_windows_says_so() {
        let missing = vec![
            IisFeature { name: "A".into(), label: "Alpha".into() },
            IisFeature { name: "B".into(), label: "Beta".into() },
        ];
        let mut lines = Lines::default();
        report_enabled(&mut lines, &missing, &parse_feature_output("A=OK\nB=RESTART"));
        assert_eq!(
            lines.0,
            vec![
                "ok Enabled Alpha",
                "ok Enabled Beta",
                "warn Windows needs a restart to finish enabling IIS features - restart the PC if the site doesn't open."
            ]
        );
        let mut lines = Lines::default();
        report_enabled(&mut lines, &missing, &parse_feature_output("A=OK\nB=FAIL"));
        assert_eq!(lines.0, vec!["ok Enabled Alpha", "! Could not enable Beta (B)"]);
    }

    #[test]
    fn an_empty_feature_list_checks_nothing() {
        let mut lines = Lines::default();
        let answer = Answer::no();
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        ensure_iis_features(&[], &mut op).unwrap();
        drop(op);
        assert_eq!(lines.0, vec!["No IIS features configured to check."]);
    }

    #[test]
    fn parses_docker_output() {
        assert_eq!(parse_docker_version("27.3.1|27.3.1\n"), ("27.3.1".into(), "27.3.1".into()));
        assert_eq!(parse_docker_version("27.3.1|\n"), ("27.3.1".into(), "".into()));
        assert_eq!(parse_container("\nrunning|Up 2 hours\n"), (Some("running".into()), Some("Up 2 hours".into())));
        assert_eq!(parse_container(""), (None, None));
    }

    #[test]
    fn winget_already_installed_is_fine_and_a_failure_names_the_last_lines() {
        let mut lines = Lines::default();
        let already = ProcessOutput {
            exit_code: Some(-1978335189),
            stdout: "Found an existing package already installed.".into(),
            stderr: String::new(),
        };
        assert!(install_outcome(&mut lines, &already).is_ok());
        let failed = ProcessOutput { exit_code: Some(1), stdout: "a\nb\nc\nd".into(), stderr: String::new() };
        assert_eq!(
            install_outcome(&mut lines, &failed).unwrap_err(),
            "winget couldn't install Docker Desktop (exit 1): b c d"
        );
    }
}
