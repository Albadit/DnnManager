//! Source Control joined to the real system: the project's folder (`dnnm_infrastructure::source_control`), the
//! servers over SFTP (`dnnm-remote`), the records in `dnnmanager.db`, the passwords in the OS's store
//! (`dnnm_system::secrets`) and the backups folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dnnm_application::operation::{CancelToken, Operation};
use dnnm_application::source_control::model::{Entry, Environment, HistoryLine};
use dnnm_application::source_control::sync::{self, BothCopies, Scan, SyncDeps, TransferItem, TransferResult};
use dnnm_infrastructure::source_control::{ProjectFiles, SourceControlStore};
use dnnm_system::secrets::CredentialStore;

use crate::Services;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Where copies of a project's server files go before they are replaced or deleted:
/// `backups\<project>\server\<environment>\<time>\…`.
pub fn server_backups(project: &str) -> PathBuf {
    crate::backups::backups_root().join(project).join("server")
}

fn store(services: &Services) -> SourceControlStore {
    SourceControlStore::new(services.db.clone())
}

fn deps(services: &Services, project: &str, project_dir: &Path) -> SyncDeps {
    SyncDeps {
        local: Arc::new(ProjectFiles),
        remote: Arc::new(dnnm_remote::Servers::new(Arc::new(|name: &str| CredentialStore.read(name)))),
        state: Arc::new(store(services)),
        project_dir: project_dir.to_path_buf(),
        backups: server_backups(project),
        now,
        retry_pause: Duration::from_secs(2),
    }
}

/// The project's environments.
pub fn environments(services: &Services, project: &str) -> Result<Vec<Environment>, String> {
    store(services).environments(project)
}

/// Saves an environment, and its secret when one is given (`Some("")` takes it away).
pub fn save_environment(services: &Services, env: &Environment, secret: Option<&str>) -> Result<(), String> {
    match secret {
        Some("") => CredentialStore.delete(&env.secret_name())?,
        Some(secret) => CredentialStore.write(&env.secret_name(), secret)?,
        None => {}
    }
    store(services).save_environment(env)
}

/// Whether the environment has a secret saved.
pub fn has_secret(env: &Environment) -> bool {
    CredentialStore.read(&env.secret_name()).is_some_and(|s| !s.is_empty())
}

/// Removes an environment, its records, history and secret.
pub fn delete_environment(services: &Services, env: &Environment) -> Result<(), String> {
    CredentialStore.delete(&env.secret_name())?;
    store(services).delete_environment(&env.id)
}

pub fn history(services: &Services, env: &Environment) -> Result<Vec<HistoryLine>, String> {
    store(services).history(&env.id, 200)
}

/// Compares the project with the environment's server - an operation.
pub fn scan(services: &Services, project_dir: &Path, env: &Environment, op: &mut Operation) -> Result<Scan, String> {
    sync::scan(&deps(services, &env.project, project_dir), env, op)
}

/// Sends files - an operation.
pub fn transfer(
    services: &Services,
    project_dir: &Path,
    env: &Environment,
    items: &[TransferItem],
    previewed: bool,
    op: &mut Operation,
) -> Result<TransferResult, String> {
    sync::transfer(&deps(services, &env.project, project_dir), env, items, previewed, op)
}

/// Both copies of a file, for the diff (off the UI thread, not an operation: nothing changes).
pub fn read_both(
    services: &Services,
    project_dir: &Path,
    env: &Environment,
    path: &str,
    cancel: &CancelToken,
) -> Result<BothCopies, String> {
    sync::read_both(&deps(services, &env.project, project_dir), env, path, cancel)
}

/// The comparison again from the project and the last comparison's server side - no connection (the watcher).
pub fn reclassify_local(services: &Services, project_dir: &Path, env: &Environment, last: &[Entry]) -> Result<Vec<Entry>, String> {
    sync::reclassify_local(&deps(services, &env.project, project_dir), env, last, &CancelToken::new())
}

/// The last comparison again, without connecting (when the page opens on an environment): when the server was
/// listed, and the entries.
pub fn cached(services: &Services, project_dir: &Path, env: &Environment) -> Result<Option<(i64, Vec<Entry>)>, String> {
    sync::cached(&deps(services, &env.project, project_dir), env, &CancelToken::new())
}

/// Starts merging a conflict by hand - an operation; the file's full path.
pub fn start_merge(services: &Services, project_dir: &Path, env: &Environment, path: &str, op: &mut Operation) -> Result<PathBuf, String> {
    sync::start_merge(&deps(services, &env.project, project_dir), env, path, op)
}

/// Opens a merged file in the editor DNN Manager found (VS Code first); else its folder.
pub fn open_for_merge(file: &Path, project_dir: &Path) -> Result<(), String> {
    let ides = dnnm_openwith::locate::installed();
    let ide = ides.iter().find(|i| i.name.contains("Code")).or_else(|| ides.first());
    match ide {
        Some(ide) => dnnm_openwith::open_file_in_ide(ide, file, project_dir),
        None => dnnm_infrastructure::shell::open_folder(file.parent().unwrap_or(project_dir)),
    }
}
