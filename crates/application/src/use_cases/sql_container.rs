//! Set up docker-compose (`UseCases/SetupSqlContainerUseCase.cs`): the shared SQL Server in Docker from the settings -
//! `docker compose up -d` with their docker-compose.yml (no file is written; the compose project is `dnn-mssql`, and a
//! container of the old project `dnn-shared` is handed over) - then waiting until SQL Server takes the sa login. A data
//! volume made by DNN Manager 1.7.1 or older keeps that version's default sa password: it is found, and taken into the
//! settings when the caller can save it - the container is then made again with it.
//!
//! Deviation: the C#'s in-place progress line ("Waiting for SQL Server to accept the sa login… 12s", every 3 s) has no
//! counterpart in the port's `Reporter`; it is an info line every 30 s instead.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::operation::{CancelToken, Operation, Reporter};
use crate::use_cases::hosting::{SiteDatabases, SiteWebConfig};

/// A fresh container needs a while to make its system databases before the first login works.
pub const READY_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const RETRY_EVERY: Duration = Duration::from_secs(3);
const PROGRESS_EVERY: Duration = Duration::from_secs(30);

/// The sa password DNN Manager 1.7.1 and older gave every container - a volume made then still has it.
pub const LEGACY_SA_PASSWORD: &str = "Admin@123";

/// The container's settings this needs (`DockerOptions`); the rest of them the wiring keeps.
#[derive(Clone, PartialEq, Eq)]
pub struct ContainerSettings {
    pub container_name: String,
    pub container_ip: String,
    pub port: u16,
    pub volume_name: String,
    pub sa_password: String,
}

impl std::fmt::Debug for ContainerSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerSettings")
            .field("container_name", &self.container_name)
            .field("container_ip", &self.container_ip)
            .field("port", &self.port)
            .field("volume_name", &self.volume_name)
            .finish_non_exhaustive()
    }
}

/// Docker and SQL Server as this sees them (`IDockerComposeService`, `ISqlConnectionTester`), and the clock.
pub trait SqlContainer: Send + Sync {
    /// `docker compose -p dnn-mssql up -d` with the docker-compose.yml for the settings and `sa_password`.
    fn up(&self, sa_password: &str, report: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String>;
    /// Signs in to [master] at `server` as sa with `password` (5 s to connect); describes the server.
    fn test_sa(&self, server: &str, password: &str, cancel: &CancelToken) -> Result<String, String>;
    /// Time since an arbitrary start.
    fn elapsed(&self) -> Duration;
    /// Waits, ending early with `Err("Cancelled.")` on a cancel.
    fn wait(&self, how_long: Duration, cancel: &CancelToken) -> Result<(), String>;
}

/// What Set up docker-compose acts on; without `sites` and `web_config` the sites' web.config files aren't looked at.
#[derive(Clone)]
pub struct SqlContainerDeps {
    pub container: Arc<dyn SqlContainer>,
    pub sites: Option<Arc<dyn SiteDatabases>>,
    pub web_config: Option<Arc<dyn SiteWebConfig>>,
}

/// Saves the sa password the data volume turned out to have as the settings' - or None: it is only said.
pub type AdoptPassword<'a> = Option<&'a (dyn Fn(&str) -> Result<(), String> + Sync)>;

/// `SetupSqlContainerUseCase.ExecuteAsync`.
pub fn setup_sql_container(
    deps: &SqlContainerDeps,
    settings: &ContainerSettings,
    adopt_password: AdoptPassword,
    op: &mut Operation,
) -> Result<(), String> {
    let (result, volume_password) = up_and_wait(deps, settings, op);
    let Some(volume_password) = volume_password else { return reachable(deps, result, settings, op) };

    let volume = &settings.volume_name;
    if adopt_password.is_none_or(|adopt| adopt(volume_password).is_err()) {
        return Err(format!(
            "SQL Server is up, but its data volume '{volume}' still has DNN Manager's old default sa password \
             ({volume_password}). Put it in Settings → Database server, Save, and set it up again."
        ));
    }
    op.report.warn(&format!(
        "The data volume '{volume}' still has DNN Manager's old default sa password - Settings → Database server uses \
         it now. Change it there and in SQL Server if this PC is on a network you don't trust."
    ));
    // Made again with it: the container's health check signs in with the password it is given.
    let again = ContainerSettings { sa_password: volume_password.to_string(), ..settings.clone() };
    let (result, _) = up_and_wait(deps, &again, op);
    reachable(deps, result, settings, op)
}

/// Once the container answers: the sites whose web.config reaches it as `localhost,<port>` are pointed at
/// `127.0.0.1,<port>` - it is published on this PC's loopback only, and a site's SqlClient goes to localhost by this
/// computer's name and network address, where it doesn't listen.
fn reachable(
    deps: &SqlContainerDeps,
    result: Result<(), String>,
    settings: &ContainerSettings,
    op: &mut Operation,
) -> Result<(), String> {
    result?;
    let (Some(sites), Some(web_config)) = (&deps.sites, &deps.web_config) else { return Ok(()) };
    let port = settings.port;
    let found: Vec<(String, PathBuf)> = sites.reaching_as_localhost(&format!("{},{port}", settings.container_ip));
    for (site, file) in found {
        match web_config.use_loopback_address(&file) {
            Ok(true) => op.report.info(&format!(
                "Site '{site}' connects to 127.0.0.1,{port} now - its SqlClient didn't reach the container as localhost."
            )),
            Ok(false) => {}
            Err(e) => op.report.warn(&format!(
                "Site '{site}' still connects as localhost,{port} and may not reach the container: {e}"
            )),
        }
    }
    Ok(())
}

/// Up, then waiting for the sa login. The second value: SQL Server refused the settings' password and took the old
/// default instead - said at once, not after minutes of waiting for a password that will never work.
fn up_and_wait(
    deps: &SqlContainerDeps,
    settings: &ContainerSettings,
    op: &mut Operation,
) -> (Result<(), String>, Option<&'static str>) {
    op.report.step("Starting the SQL Server container");
    if let Err(e) = deps.container.up(&settings.sa_password, op.report, &op.cancel) {
        return (Err(e), None);
    }
    op.report.success(&format!("Container '{}' is up.", settings.container_name));

    let server = format!("{},{}", settings.container_ip, settings.port);
    op.report.step(&format!("Waiting for SQL Server at {server}"));
    // sa, whatever Settings → Database server → User says: a new container has no other login yet.
    let started = deps.container.elapsed();
    let mut last_error: Option<String> = None;
    let mut legacy_tried = false;
    let mut last_progress = Duration::ZERO;
    while deps.container.elapsed() - started < READY_TIMEOUT {
        match deps.container.test_sa(&server, &settings.sa_password, &op.cancel) {
            Ok(version) => {
                op.report.success(&format!("SQL Server is ready - {version}."));
                return (Ok(()), None);
            }
            Err(e) if e == crate::operation::CANCELLED => return (Err(e), None),
            Err(e) => last_error = Some(e),
        }
        if !legacy_tried && is_login_failure(last_error.as_deref()) && settings.sa_password != LEGACY_SA_PASSWORD {
            legacy_tried = true;
            if deps.container.test_sa(&server, LEGACY_SA_PASSWORD, &op.cancel).is_ok() {
                return (Ok(()), Some(LEGACY_SA_PASSWORD));
            }
        }
        let waited = deps.container.elapsed() - started;
        if waited - last_progress >= PROGRESS_EVERY {
            last_progress = waited;
            op.report.info(&format!("Waiting for SQL Server to accept the sa login… {}s", waited.as_secs()));
        }
        if let Err(e) = deps.container.wait(RETRY_EVERY, &op.cancel) {
            return (Err(e), None);
        }
    }

    // An existing data volume keeps the sa password it was first made with, whatever the settings say.
    let volume = &settings.volume_name;
    let error = if is_login_failure(last_error.as_deref()) {
        format!(
            "SQL Server at {server} is up, but doesn't take the sa password in Settings → Database server: the data \
             volume '{volume}' keeps the sa password it was first created with. Put that one in Settings → Database \
             server and Save, then Set up docker-compose again."
        )
    } else {
        format!(
            "SQL Server at {server} did not accept the sa login within {} minutes: {} If the volume '{volume}' already \
             existed, its sa password is the one it was first created with.",
            READY_TIMEOUT.as_secs() / 60,
            last_error.unwrap_or_default()
        )
    };
    (Err(error), None)
}

/// SQL Server answered, and refused the login (18456) - not "not there yet".
fn is_login_failure(error: Option<&str>) -> bool {
    error.is_some_and(|e| e.to_lowercase().contains("login failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{Outcome, run};
    use crate::use_cases::hosting::fakes::{FakeSites, World};
    use std::sync::Mutex;

    /// Docker and SQL Server: the passwords SQL Server takes, after how long, and a clock that moves when waited.
    struct FakeContainer {
        takes: Option<&'static str>,
        ready_after: Duration,
        up_error: Option<String>,
        now: Mutex<Duration>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeContainer {
        fn new(takes: Option<&'static str>, ready_after: Duration) -> Self {
            Self { takes, ready_after, up_error: None, now: Mutex::default(), calls: Mutex::default() }
        }
    }

    impl SqlContainer for FakeContainer {
        fn up(&self, sa_password: &str, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("up with {sa_password}"));
            self.up_error.clone().map_or(Ok(()), Err)
        }
        fn test_sa(&self, _: &str, password: &str, _: &CancelToken) -> Result<String, String> {
            if *self.now.lock().unwrap() < self.ready_after {
                return Err("A network-related error occurred".into());
            }
            if Some(password) == self.takes {
                Ok("SQL Server 2022".into())
            } else {
                Err("Login failed for user 'sa'.".into())
            }
        }
        fn elapsed(&self) -> Duration {
            *self.now.lock().unwrap()
        }
        fn wait(&self, how_long: Duration, _: &CancelToken) -> Result<(), String> {
            *self.now.lock().unwrap() += how_long;
            Ok(())
        }
    }

    fn settings() -> ContainerSettings {
        ContainerSettings {
            container_name: "dnn-sqlserver".into(),
            container_ip: "localhost".into(),
            port: 1433,
            volume_name: "dnn_sqlserver_data".into(),
            sa_password: "New-Pw-1".into(),
        }
    }

    fn set_up(deps: &SqlContainerDeps, adopt: AdoptPassword) -> (Outcome, Vec<String>) {
        let mut lines = Lines::default();
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            setup_sql_container(deps, &settings(), adopt, op)
        });
        (outcome, lines.0)
    }

    fn deps(container: FakeContainer) -> (SqlContainerDeps, Arc<FakeContainer>) {
        let container = Arc::new(container);
        (SqlContainerDeps { container: container.clone(), sites: None, web_config: None }, container)
    }

    #[test]
    fn up_then_ready_once_the_login_works() {
        let (deps, container) = deps(FakeContainer::new(Some("New-Pw-1"), Duration::from_secs(40)));
        let (outcome, lines) = set_up(&deps, None);
        assert_eq!(outcome, Outcome::Finished);
        assert_eq!(*container.calls.lock().unwrap(), vec!["up with New-Pw-1"]);
        assert_eq!(
            lines,
            vec![
                "## Starting the SQL Server container",
                "ok Container 'dnn-sqlserver' is up.",
                "## Waiting for SQL Server at localhost,1433",
                "Waiting for SQL Server to accept the sa login… 30s",
                "ok SQL Server is ready - SQL Server 2022.",
            ]
        );
    }

    #[test]
    fn an_old_volumes_password_is_adopted_and_the_container_made_again_with_it() {
        let (deps, container) = deps(FakeContainer::new(Some(LEGACY_SA_PASSWORD), Duration::ZERO));
        let adopted = Mutex::new(None);
        let adopt = |p: &str| {
            *adopted.lock().unwrap() = Some(p.to_string());
            Ok(())
        };
        let (outcome, lines) = set_up(&deps, Some(&adopt));
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(adopted.lock().unwrap().as_deref(), Some(LEGACY_SA_PASSWORD));
        assert_eq!(*container.calls.lock().unwrap(), vec!["up with New-Pw-1", "up with Admin@123"]);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("warn The data volume 'dnn_sqlserver_data' still has DNN Manager's old"))
        );

        // Without a way to save it, it is said.
        let (deps, _) = self::deps(FakeContainer::new(Some(LEGACY_SA_PASSWORD), Duration::ZERO));
        let (outcome, _) = set_up(&deps, None);
        assert_eq!(
            outcome,
            Outcome::Failed(
                "SQL Server is up, but its data volume 'dnn_sqlserver_data' still has DNN Manager's old default sa \
                 password (Admin@123). Put it in Settings → Database server, Save, and set it up again."
                    .into()
            )
        );
    }

    #[test]
    fn a_password_never_taken_and_a_server_never_ready_are_told_apart() {
        let (deps, _) = deps(FakeContainer::new(Some("other"), Duration::ZERO));
        let Outcome::Failed(e) = set_up(&deps, None).0 else { panic!() };
        assert!(e.starts_with("SQL Server at localhost,1433 is up, but doesn't take the sa password"), "{e}");
        let (deps, _) = self::deps(FakeContainer::new(None, Duration::from_secs(3600)));
        let Outcome::Failed(e) = set_up(&deps, None).0 else { panic!() };
        assert_eq!(
            e,
            "SQL Server at localhost,1433 did not accept the sa login within 3 minutes: A network-related error \
             occurred If the volume 'dnn_sqlserver_data' already existed, its sa password is the one it was first \
             created with."
        );
    }

    #[test]
    fn a_failed_up_is_the_error_and_nothing_is_waited_for() {
        let mut container = FakeContainer::new(Some("New-Pw-1"), Duration::ZERO);
        container.up_error = Some("docker compose up failed: no space".into());
        let (deps, _) = deps(container);
        let (outcome, lines) = set_up(&deps, None);
        assert_eq!(outcome, Outcome::Failed("docker compose up failed: no space".into()));
        assert_eq!(lines, vec!["## Starting the SQL Server container", "! docker compose up failed: no space"]);
    }

    #[test]
    fn sites_reaching_it_as_localhost_are_pointed_at_the_loopback_address() {
        let world = World::new();
        let sites = FakeSites {
            localhost: vec![("shop".into(), PathBuf::from(r"C:\DNN\shop\web.config"))],
            ..Default::default()
        };
        let deps = SqlContainerDeps {
            container: Arc::new(FakeContainer::new(Some("New-Pw-1"), Duration::ZERO)),
            sites: Some(Arc::new(sites)),
            web_config: Some(world.web_config.clone()),
        };
        let (outcome, lines) = set_up(&deps, None);
        assert_eq!(outcome, Outcome::Finished);
        assert_eq!(
            lines.last().unwrap(),
            "Site 'shop' connects to 127.0.0.1,1433 now - its SqlClient didn't reach the container as localhost."
        );
        assert_eq!(world.journal(), vec![r"loopback C:\DNN\shop\web.config"]);
    }
}
