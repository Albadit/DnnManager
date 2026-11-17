//! Docker (`Docker/DockerNames.cs`, `Docker/DockerComposeService.cs`, and `GetDockerStatusAsync` of
//! `Prereq/WindowsPrerequisiteChecker.cs`): the container's and volume's names checked where they are used, the
//! shared SQL Server container's docker-compose.yml made from the settings - shown with a placeholder instead of the sa
//! password, run with the real one handed to `docker compose` on standard input so it is never written to a file -
//! and what Docker and the container are doing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::operation::{CancelToken, Reporter};

use crate::model::{DockerOptions, DockerStatus};
use crate::programs::{self, Resolved, Run, is_elevated, known_folders};

/// What Docker itself allows for a container or a volume: a letter or digit, then letters, digits, `_ . -` - so a
/// name can't be read as an option (`--privileged`) or a path (a bind mount).
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    !name.is_empty()
        && name.len() <= 128
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Why `name` (the `what`'s, e.g. "container") can't be used - or None when it can.
pub fn name_refusal(name: &str, what: &str) -> Option<String> {
    (!is_valid_name(name)).then(|| {
        format!(
            "'{name}' can't be the {what}'s name - only letters, digits, '_', '.' and '-', starting with a letter or digit \
             (Settings → Docker container)."
        )
    })
}

// All projects share one SQL container, so there is one compose project name.
const COMPOSE_PROJECT_NAME: &str = "dnn-mssql";
// What the compose project was called before - its container is handed over to the new name.
const OLD_COMPOSE_PROJECT_NAME: &str = "dnn-shared";
const PASSWORD_PLACEHOLDER: &str = "<your-sa-password>";

/// The SQL Server image: one known release (2022 CU27) by its digest - not 2022-latest. Raised by hand.
pub const SQL_SERVER_IMAGE: &str = "mcr.microsoft.com/mssql/server:2022-CU27-ubuntu-22.04@sha256:4402d880dd4c34bfa7d8705e56a86cd6c88da80a1f6bbbe741f999e76264a090";

/// The docker-compose.yml for these settings, with a placeholder instead of the sa password - to show or copy.
pub fn render(docker: &DockerOptions) -> String {
    build(docker, false, os_supports_ipv6())
}

/// Where the container's port is published, as compose's `ports` items: on both loopback addresses when it is reached
/// as localhost (only this PC can reach it), on every interface for another host in the settings.
pub fn published_on(docker: &DockerOptions, ipv6: bool) -> Vec<String> {
    let port = format!("{}:1433", docker.default_port);
    let host = docker.container_ip.trim();
    let local = host.eq_ignore_ascii_case("localhost") || matches!(host, "127.0.0.1" | "." | "::1");
    if !local {
        return vec![port];
    }
    // Without IPv6 on Windows, compose would fail on [::1] - and localhost is 127.0.0.1 only then anyway.
    if ipv6 { vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")] } else { vec![format!("127.0.0.1:{port}")] }
}

fn os_supports_ipv6() -> bool {
    std::net::UdpSocket::bind("[::1]:0").is_ok()
}

fn build(docker: &DockerOptions, with_password: bool, ipv6: bool) -> String {
    // Double-quoted YAML scalars; compose also interpolates "$", so a literal one is written "$$".
    let q = |value: &str| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "$$"));
    let header = if with_password {
        "# Run by DNN Manager (Environment -> Set up docker-compose).".to_string()
    } else {
        format!("# Replace {PASSWORD_PLACEHOLDER} below with the SA password from Settings -> SQL Server, then run:")
    };
    let password = if with_password { q(&docker.sa_password) } else { format!("\"{PASSWORD_PLACEHOLDER}\"") };
    let ports = published_on(docker, ipv6).iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join("\n      - ");
    format!(
        r#"# Shared SQL Server for all DNN Manager projects - one container for every project.
# Made from DNN Manager's settings (Settings -> SQL Server and Docker container).
{header}
#   docker compose -p {COMPOSE_PROJECT_NAME} -f docker-compose.yml up -d
services:
  sqlserver:
    image: {SQL_SERVER_IMAGE}
    container_name: {container}
    hostname: {container}
    environment:
      ACCEPT_EULA: "Y"
      MSSQL_SA_PASSWORD: {password}
      MSSQL_PID: {pid}
      MSSQL_COLLATION: {collation}
    ports:
      - {ports}
    volumes:
      - {volume_mount}
    restart: unless-stopped
    healthcheck:
      # $$ keeps compose from filling it in: the shell in the container reads its own MSSQL_SA_PASSWORD - and hands
      # it to sqlcmd in SQLCMDPASSWORD, not with -P, where the container's process list would show it.
      test: ["CMD-SHELL", "SQLCMDPASSWORD=\"$$MSSQL_SA_PASSWORD\" /opt/mssql-tools18/bin/sqlcmd -S localhost -U sa -C -No -Q \"SELECT 1\" || exit 1"]
      interval: 30s
      timeout: 10s
      retries: 5
      start_period: 60s

volumes:
  {volume}:
    name: {volume}
"#,
        container = q(&docker.container_name),
        pid = q(&docker.mssql_pid),
        collation = q(&docker.collation),
        volume_mount = q(&format!("{}:/var/opt/mssql", docker.volume_name)),
        volume = q(&docker.volume_name),
    )
    .replace('\n', "\r\n")
}

/// The compose plugin docker would run when it is one others could change, else None. The docker DNN Manager runs
/// reads DNN Manager's own configuration folder: it looks for plugins there, then in `%ProgramData%\Docker\cli-plugins`
/// - a folder any user may make - and then in Docker Desktop's (Program Files).
fn untrusted_compose_plugin() -> Option<String> {
    if !is_elevated() {
        return None;
    }
    let config = crate::dirs::docker_config().ok();
    let folders: Vec<PathBuf> = [
        config.map(|c| c.join("cli-plugins")),
        Some(known_folders::program_data().join("Docker").join("cli-plugins")),
        Some(PathBuf::from(known_folders::program_files()).join("Docker").join("cli-plugins")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for folder in folders {
        let plugin = folder.join("docker-compose.exe");
        if !plugin.is_file() {
            continue;
        }
        return match programs::resolve(&plugin.display().to_string()) {
            Resolved::Path(_) => None,
            Resolved::Refused { copy, .. } => Some(copy),
            Resolved::Missing => Some(plugin.display().to_string()),
        };
    }
    None
}

/// Runs `docker compose up -d` with the docker-compose.yml for `docker`, sa password included - creates the
/// container, or updates it after the settings changed. No file is written (`IDockerComposeService.UpAsync`).
pub fn up(docker: &DockerOptions, reporter: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String> {
    if let Some(refused) =
        name_refusal(&docker.container_name, "container").or_else(|| name_refusal(&docker.volume_name, "volume"))
    {
        return Err(refused);
    }
    remove_old_project_container(&docker.container_name, reporter, cancel)?;
    // There already: its databases are kept - and so is the sa password it was first made with.
    let volume = Run::new("docker", &["volume", "inspect", "--format", "{{.Name}}", &docker.volume_name])
        .cancel(cancel)
        .timeout(Duration::from_secs(30))
        .run()?;
    if volume.success() {
        reporter.info(&format!(
            "The data volume '{}' exists already - its databases are kept, and its sa password is the one it was made with.",
            docker.volume_name
        ));
    }

    reporter.info("Running docker compose up -d with the settings (the first run downloads the SQL Server image)…");
    if let Some(plugin) = untrusted_compose_plugin() {
        return Err(format!(
            "Didn't run docker compose: Docker would use {plugin}, which programs without administrator rights could change - \
             and DNN Manager runs it as Administrator. Remove it (Docker Desktop has its own, in Program Files) and try again."
        ));
    }
    let yaml = build(docker, true, os_supports_ipv6());
    // Each line as it comes - the first run pulls well over a GB.
    let shared = std::sync::Mutex::new(&mut *reporter);
    let progress = |line: &str| {
        if !line.trim().is_empty() {
            shared.lock().unwrap_or_else(|e| e.into_inner()).info(line.trim());
        }
    };
    let mut run = Run::new("docker", &["compose", "-p", COMPOSE_PROJECT_NAME, "-f", "-", "up", "-d"])
        .cancel(cancel)
        .stdin_text(&yaml);
    run.on_output = Some(&progress);
    let r = run.run()?;
    if r.success() {
        return Ok(());
    }
    if r.exit_code == -1 {
        let why = r.stderr.trim();
        return Err(if why.is_empty() || why.contains("isn't installed") {
            "Docker is not installed or not on PATH - install Docker Desktop, start it and try again.".into()
        } else {
            why.to_string()
        });
    }
    let error = r.stderr.trim();
    Err(format!("docker compose up failed: {}", if error.is_empty() { r.stdout.trim() } else { error }))
}

/// A container made under the old compose project name would block the new project: removed, so compose makes it
/// again under the new name. The databases are kept: they live in the volume.
fn remove_old_project_container(
    container: &str,
    reporter: &mut dyn Reporter,
    cancel: &CancelToken,
) -> Result<(), String> {
    let inspect =
        Run::new("docker", &["inspect", "-f", "{{ index .Config.Labels \"com.docker.compose.project\" }}", container])
            .cancel(cancel)
            .run()?;
    if !inspect.success() || inspect.stdout.trim() != OLD_COMPOSE_PROJECT_NAME {
        return Ok(());
    }
    reporter.info(&format!(
        "Moving '{container}' from the compose project '{OLD_COMPOSE_PROJECT_NAME}' to '{COMPOSE_PROJECT_NAME}' - \
         the container is made again, the databases stay in the volume…"
    ));
    let removed = Run::new("docker", &["rm", "-f", container]).cancel(cancel).run()?;
    if !removed.success() {
        reporter.warn(&format!("Could not remove the old container: {}", removed.stderr.trim()));
        return Ok(());
    }
    // The old project's network is empty now (best effort - another container may still use it).
    Run::new("docker", &["network", "rm", &format!("{OLD_COMPOSE_PROJECT_NAME}_default")]).cancel(cancel).run()?;
    Ok(())
}

// Installed for all users (Program Files), or for this account only (%LOCALAPPDATA%\Programs).
fn docker_desktop_exe() -> PathBuf {
    let all = PathBuf::from(known_folders::program_files()).join("Docker").join("Docker").join("Docker Desktop.exe");
    let own = std::env::var_os("LOCALAPPDATA")
        .map(|l| Path::new(&l).join("Programs").join("DockerDesktop").join("Docker Desktop.exe"));
    [Some(all.clone()), own].into_iter().flatten().find(|p| p.is_file()).unwrap_or(all)
}

/// Docker Desktop, its engine, and the state of the container `container_name` (`GetDockerStatusAsync`).
pub fn status(container_name: &str, cancel: Option<&CancelToken>) -> Result<DockerStatus, String> {
    // One call answers both "CLI there?" and "engine up?": the client version prints even when the engine is down.
    // Docker Desktop half started can leave the CLI waiting for ever: a question gets half a minute.
    let mut version = Run::new("docker", &["version", "--format", "{{.Client.Version}}|{{.Server.Version}}"])
        .timeout(Duration::from_secs(30));
    version.cancel = cancel;
    let v = version.run()?;
    let (client, server) = parse_version(&v.stdout);
    let installed = !client.is_empty() || docker_desktop_exe().is_file();
    if !(v.success() && !server.is_empty()) {
        return Ok(DockerStatus {
            desktop_installed: installed,
            client_version: (!client.is_empty()).then_some(client),
            ..Default::default()
        });
    }
    if !is_valid_name(container_name) {
        return Err(name_refusal(container_name, "container").unwrap_or_default());
    }
    let filter = format!("name=^{container_name}$");
    let mut ps = Run::new("docker", &["ps", "-a", "--filter", &filter, "--format", "{{.State}}|{{.Status}}"])
        .timeout(Duration::from_secs(30));
    ps.cancel = cancel;
    let c = ps.run()?;
    let (state, status) = if c.success() { parse_container(&c.stdout) } else { (None, None) };
    Ok(DockerStatus {
        desktop_installed: true,
        client_version: Some(client),
        engine_running: true,
        engine_version: Some(server),
        container_state: state,
        container_status: status,
    })
}

fn parse_version(output: &str) -> (String, String) {
    let mut parts = output.trim().split('|');
    let client = parts.next().unwrap_or_default().trim().to_string();
    let server = parts.next().unwrap_or_default().trim().to_string();
    (client, server)
}

fn parse_container(output: &str) -> (Option<String>, Option<String>) {
    let Some(line) = output.lines().map(str::trim).find(|l| !l.is_empty()) else { return (None, None) };
    let mut parts = line.split('|');
    (parts.next().map(str::to_string), parts.next().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_could_be_read_as_options_or_paths_are_refused() {
        for (name, valid) in [
            ("dnn-sqlserver", true),
            ("sql_2022.local", true),
            ("--privileged", false),
            ("-d", false),
            ("C:/data", false),
            ("../etc", false),
            ("a b", false),
            ("", false),
        ] {
            assert_eq!(is_valid_name(name), valid, "{name}");
        }
        assert!(name_refusal("-d", "volume").unwrap().starts_with("'-d' can't be the volume's name"));
    }

    #[test]
    fn the_container_runs_a_pinned_image_and_its_health_check_has_no_password_on_its_command_line() {
        let compose = render(&DockerOptions::default());
        assert!(compose.contains("image: mcr.microsoft.com/mssql/server:2022-CU"));
        assert!(compose.contains("@sha256:"));
        assert!(!compose.contains("2022-latest"));
        assert!(!compose.contains(" -P "));
        assert!(compose.contains(r#"MSSQL_SA_PASSWORD: "<your-sa-password>""#));
    }

    #[test]
    fn the_password_goes_only_into_what_docker_compose_is_handed_and_is_quoted() {
        let docker = DockerOptions { sa_password: "Pa$$\"w\\rd".into(), ..Default::default() };
        assert!(!render(&docker).contains("Pa$$"));
        assert!(build(&docker, true, false).contains(r#"MSSQL_SA_PASSWORD: "Pa$$$$\"w\\rd""#));
    }

    #[test]
    fn a_local_container_is_published_on_both_loopback_addresses() {
        let local = DockerOptions { container_ip: "localhost".into(), default_port: 1433, ..Default::default() };
        assert_eq!(published_on(&local, true), ["127.0.0.1:1433:1433", "[::1]:1433:1433"]);
        assert_eq!(published_on(&local, false), ["127.0.0.1:1433:1433"]);
        let network = DockerOptions { container_ip: "192.168.1.20".into(), default_port: 1433, ..Default::default() };
        assert_eq!(published_on(&network, true), ["1433:1433"]);

        // Each address its own item of the ports list, at the list's indentation.
        let compose = build(&local, false, true);
        let items: Vec<&str> = compose.lines().filter(|l| l.contains(":1433:1433\"")).collect();
        assert_eq!(items.len(), 2, "{compose}");
        assert!(items.iter().all(|i| i.starts_with("      - \"")));
    }

    #[test]
    fn dockers_answers_are_read() {
        assert_eq!(parse_version("28.3.2|28.3.2\n"), ("28.3.2".into(), "28.3.2".into()));
        assert_eq!(parse_version("28.3.2|\n"), ("28.3.2".into(), String::new()));
        assert_eq!(
            parse_container("running|Up 2 hours (healthy)\n"),
            (Some("running".into()), Some("Up 2 hours (healthy)".into()))
        );
        assert_eq!(parse_container("\n"), (None, None));
    }

    /// Read-only: `docker version` and `docker ps -a` - needs Docker Desktop.
    #[test]
    #[ignore = "needs Docker Desktop"]
    fn status_of_the_container() {
        let status = status("dnn-sqlserver", None).unwrap();
        assert!(status.desktop_installed, "{status:?}");
        println!("{status:?}");
    }
}
