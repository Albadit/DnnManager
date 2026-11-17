//! Set up docker-compose (`Controls/DockerCard.xaml.cs` `SetupCompose_Click`, `UseCases/SetupSqlContainerUseCase.cs`).
//!
//! The app: Settings → Docker container's "Set up docker-compose" runs [`setup_sql_container`] as an operation titled
//! [`SETUP_COMPOSE_TITLE`], then tests the container again. `adopt_password` saves an old volume's sa password as
//! the settings' `sqlServer.saPassword` (encrypted, as every save) and applies it - it is called on the operation's
//! thread; without it the password is only said.

use std::sync::Arc;
use std::time::Instant;

use dnnm_application::operation::Operation;
use dnnm_application::use_cases::hosting::{SiteDatabases, SiteWebConfig};
use dnnm_application::use_cases::sql_container::{self as use_case, ContainerSettings, SqlContainerDeps};

use crate::Services;
use crate::hosting::{Compose, WebConfigFile, hosting_deps};

/// The operation's title.
pub const SETUP_COMPOSE_TITLE: &str = "Set up docker-compose";

/// Saves the sa password an old data volume has as the settings' (Settings → Database server).
pub type AdoptPassword = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// What Set up docker-compose is given: how to take an old volume's sa password into the settings - or None.
#[derive(Clone, Default)]
pub struct SetupSqlContainerRequest {
    pub adopt_password: Option<AdoptPassword>,
}

/// Runs Set up docker-compose (`SetupSqlContainerUseCase`): `docker compose up -d` from the settings' rendered
/// docker-compose.yml (compose project `dnn-mssql`, an old `dnn-shared` container handed over), then waits for the sa
/// login and points sites that reach the container as localhost at 127.0.0.1.
pub fn setup_sql_container(
    services: &Services,
    request: SetupSqlContainerRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let docker = crate::remove_project::docker_options(services)?;
    let settings = ContainerSettings {
        container_name: docker.container_name.clone(),
        container_ip: docker.container_ip.clone(),
        port: docker.default_port,
        volume_name: docker.volume_name.clone(),
        sa_password: docker.sa_password.clone(),
    };
    let sites: Arc<dyn SiteDatabases> = hosting_deps(services)?.sites;
    let web_config: Arc<dyn SiteWebConfig> = Arc::new(WebConfigFile);
    let deps = SqlContainerDeps {
        container: Arc::new(Compose { docker, started: Instant::now() }),
        sites: Some(sites),
        web_config: Some(web_config),
    };
    let adopt = request.adopt_password.as_deref().map(|f| f as &(dyn Fn(&str) -> Result<(), String> + Sync));
    use_case::setup_sql_container(&deps, &settings, adopt, op)
}
