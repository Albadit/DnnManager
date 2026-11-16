//! Remove (`RemoveProjectUseCase` with `LocalSqlContainer.DatabaseOf`, `SiteDatabases`, `DeveloperDb`,
//! `FileLockService`, `KeepWarmRecords`, `ProjectRecords`) joined to the SQL, files, keep-warm and IIS crates; and the
//! WPF texts the app shows around it (`ProjectsPage.xaml`, `ProjectMenu.cs`, `ServerStore.RemoveAsync`,
//! `Onboarding.cs`).
//!
//! Deviations: `DeveloperDb.FromWebConfig` reads the Initial Catalog of any SiteSqlServer connection; here it is the
//! database of a SQL Server connection only - a LocalDB-file connection string with an Initial Catalog too (DNN's
//! shipped one has none) gives no name. A site name that isn't a single folder name (`..`) has no backups folder,
//! so Remove can never delete outside `Documents\DnnManager\backups`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::iis::IisManager;
use dnnm_application::operation::{CANCELLED, CancelToken, Operation};
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::remove_project::{
    self as use_case, KeepWarmList, Locker, ProjectFolders, RemoveDatabases, RemoveDeps, SiteDatabase, SiteDatabaseKind,
};
pub use dnnm_application::use_cases::remove_project::{RemoveProjectRequest, SiteToRemove};
use dnnm_infrastructure::app_db::ProjectTable;
use dnnm_infrastructure::web_config::{self, DatabaseKind as WebConfigKind};
use dnnm_keepwarm::records::KeepWarmRecords;
use dnnm_sql::container::SqlServerService;
use dnnm_sql::provisioner::DatabaseProvisioner;
use dnnm_sql::{DatabaseConnection, DatabaseKind, DockerOptions, SqlAuthentication, SqlServerAddress};
use dnnm_system::files::{file_locks, safe_path};

use crate::Services;

/// The row's menu item (`ProjectMenu`), enabled while the row is idle.
pub const REMOVE_MENU_ITEM: &str = "Remove…";
/// The row's Remove button (the bin), enabled while the row is idle; Del on a selected row does the same.
pub const REMOVE_ROW_TOOLTIP: &str =
    "Remove - delete the website, its folder and its database from this computer; asks first (Del)";
/// The bulk bar's Remove button: its accessible name and tooltip. Enabled when every checked row is idle.
pub const REMOVE_BULK_NAME: &str = "Remove the selected websites";
pub const REMOVE_BULK_TOOLTIP: &str =
    "Remove the selected websites - their folders and databases are deleted; asks first";
/// The guide's (Onboarding) entry for the Remove button.
pub const REMOVE_GUIDE_TITLE: &str = "Remove";
pub const REMOVE_GUIDE_TEXT: &str = "Deletes the website from this computer: its website in IIS, its folder and its database. Its backups are kept. Asks first, naming what goes.";
/// What each removed row says once the operation has started (after the question was answered yes).
pub const REMOVING: &str = "Removing…";

/// The operation's title: `Remove 'shop'`, `Remove 3 projects`.
pub fn remove_title(names: &[String]) -> String {
    match names {
        [one] => format!("Remove '{one}'"),
        _ => format!("Remove {} projects", names.len()),
    }
}

/// Removes the projects of `request` (one row or the checked rows), asking first.
pub fn remove_project(services: &Services, request: RemoveProjectRequest, op: &mut Operation) -> Result<(), String> {
    let databases = SqlDatabases { iis: services.iis.as_ref(), docker: docker_options(services) };
    let folders = SiteFolders { backups_root: dnnm_infrastructure::paths::app_data_directory().join("backups") };
    let records = ProjectTable(services.db.clone());
    let keep_warm = KeepWarm(KeepWarmRecords::new(services.db.path()));
    let deps = RemoveDeps {
        iis: services.iis.as_ref(),
        databases: &databases,
        folders: &folders,
        records: &records,
        keep_warm: &keep_warm,
    };
    use_case::remove_project(&deps, &request, op)
}

/// The container's settings (`AppOptions.Docker` from `UserSettings`); the sa password decrypted. Err when it can't
/// be - said only when the container is needed.
pub(crate) fn docker_options(services: &Services) -> Result<DockerOptions, String> {
    let settings: &Settings = &services.settings;
    let defaults = DockerOptions::default();
    let text = |key: &str, fallback: &str| {
        settings.get(key).map(str::trim).filter(|v| !v.is_empty()).unwrap_or(fallback).to_string()
    };
    let sa_password = settings.get(dnnm_system::secrets::SA_PASSWORD_KEY).unwrap_or_default();
    let sa_password = dnnm_system::secrets::unprotect(sa_password).map_err(|_| {
        format!("The settings in {} have an SA password that can't be decrypted.", services.db.path().display())
    })?;
    Ok(DockerOptions {
        container_name: text("docker.containerName", &defaults.container_name),
        container_ip: text("sqlServer.host", &defaults.container_ip),
        volume_name: text("docker.volumeName", &defaults.volume_name),
        sql_user: text("sqlServer.userName", "sa"),
        sa_password,
        default_port: settings
            .get("sqlServer.port")
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(defaults.default_port),
        collation: text("docker.collation", &defaults.collation),
        mssql_pid: text("docker.edition", &defaults.mssql_pid),
    })
}

/// `LocalSqlContainer`, `SiteDatabases`, `ISqlServerService` and `IDatabaseProvisioner` for Remove.
struct SqlDatabases<'a> {
    iis: &'a dyn IisManager,
    docker: Result<DockerOptions, String>,
}

impl SqlDatabases<'_> {
    fn docker(&self) -> Result<&DockerOptions, String> {
        self.docker.as_ref().map_err(Clone::clone)
    }

    /// The database connection of every IIS site but `site`, as its web.config has it - none when IIS can't be read.
    fn other_sites(&self, site: &str) -> Vec<(String, web_config::DatabaseConnection)> {
        let Ok(sites) = self.iis.site_runtimes() else { return Vec::new() };
        sites
            .into_iter()
            .filter(|(name, s)| !name.eq_ignore_ascii_case(site) && !s.physical_path.as_os_str().is_empty())
            .filter_map(|(name, s)| {
                let path = s.physical_path.join("web.config");
                if !path.is_file() {
                    return None;
                }
                web_config::read_database_connection(&path)
                    .ok()
                    .filter(|c| c.kind != WebConfigKind::LocalDbFile)
                    .map(|c| (name, c))
            })
            .collect()
    }
}

fn cancelled_or(e: String, cancel: &CancelToken) -> String {
    if cancel.is_cancelled() { CANCELLED.into() } else { e }
}

impl RemoveDatabases for SqlDatabases<'_> {
    fn database_of(&self, directory: &Path) -> Option<SiteDatabase> {
        let c = web_config::read_database_connection(&directory.join("web.config")).ok()?;
        // DNN's shipped connection (.\SQLExpress with a User Instance) is what a site has until it is installed.
        if c.kind == WebConfigKind::LocalDbFile && !web_config::is_local_db(&c.server) {
            return None;
        }
        let kind = match c.kind {
            WebConfigKind::LocalDbFile => SiteDatabaseKind::LocalDbFile,
            WebConfigKind::SqlServer
                if !c.windows_authentication
                    && self.docker.as_ref().is_ok_and(|d| {
                        SqlServerAddress::parse(&c.server).is_container(&d.container_ip, d.default_port)
                    }) =>
            {
                SiteDatabaseKind::Container
            }
            WebConfigKind::SqlServer => SiteDatabaseKind::SqlServer,
        };
        Some(SiteDatabase {
            kind,
            server: c.server.clone(),
            database: c.database.clone(),
            windows_authentication: c.windows_authentication,
            user: c.user.clone(),
            password: c.password.clone(),
        })
    }

    fn database_name_in_web_config(&self, directory: &Path) -> Option<String> {
        web_config::read_database_connection(&directory.join("web.config"))
            .ok()
            .filter(|c| c.kind == WebConfigKind::SqlServer && !c.database.trim().is_empty())
            .map(|c| c.database.clone())
    }

    fn container_server(&self) -> String {
        let defaults = DockerOptions::default();
        let d = self.docker.as_ref().unwrap_or(&defaults);
        format!("{},{}", d.container_ip, d.default_port)
    }

    fn is_on_this_machine(&self, server: &str) -> bool {
        SqlServerAddress::is_on_this_machine(server)
    }

    fn other_site_using(&self, site: &str, server: &str, database: &str) -> Option<String> {
        if database.is_empty() {
            return None;
        }
        let address = SqlServerAddress::parse(server);
        self.other_sites(site)
            .into_iter()
            .find(|(_, c)| {
                c.database.eq_ignore_ascii_case(database) && address.same_server_as(&SqlServerAddress::parse(&c.server))
            })
            .map(|(name, _)| name)
    }

    fn other_site_signing_in_as(&self, site: &str, server: &str, login: &str) -> Option<String> {
        if login.is_empty() {
            return None;
        }
        let address = SqlServerAddress::parse(server);
        self.other_sites(site)
            .into_iter()
            .find(|(_, c)| {
                c.user.eq_ignore_ascii_case(login) && address.same_server_as(&SqlServerAddress::parse(&c.server))
            })
            .map(|(name, _)| name)
    }

    fn drop_database(&self, database: &SiteDatabase, cancel: &CancelToken) -> Result<(), String> {
        let authentication =
            if database.windows_authentication { SqlAuthentication::Windows } else { SqlAuthentication::Sql };
        let connection =
            DatabaseConnection::new(DatabaseKind::SqlServer, &database.server, &database.database, authentication)
                .with_login(&database.user, &database.password);
        DatabaseProvisioner.drop_database(&connection, Some(cancel)).map_err(|e| cancelled_or(e, cancel))
    }

    fn drop_container_database(&self, database: &str, cancel: &CancelToken) -> Result<(), String> {
        SqlServerService::new(self.docker()?.clone())
            .drop_database(database, cancel)
            .map_err(|e| cancelled_or(e, cancel))
    }

    fn drop_container_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String> {
        SqlServerService::new(self.docker()?.clone()).drop_login(login, cancel).map_err(|e| cancelled_or(e, cancel))
    }
}

/// How many times a folder delete is tried, and how long it waits in between (`TryDeleteDirectoryAsync`): DNN ships
/// read-only files, and the IIS worker may release its handles a beat after its pool reports Stopped.
const DELETE_ATTEMPTS: u32 = 8;

fn delete_wait(attempt: u32) -> Duration {
    Duration::from_millis(u64::from((500 * attempt).min(2000)))
}

/// The project's folders: `Documents\DnnManager\backups\<site>`, deletes and `FileLockService`.
struct SiteFolders {
    backups_root: PathBuf,
}

/// A site name that is one folder's name - never `..`, a drive or a path.
fn is_folder_name(name: &str) -> bool {
    !name.trim().is_empty() && name != "." && name != ".." && !name.contains(['\\', '/', ':', '\0'])
}

impl ProjectFolders for SiteFolders {
    fn backup_directory(&self, site: &str) -> Option<PathBuf> {
        let folder = self.backups_root.join(site);
        (is_folder_name(site) && safe_path::is_inside(&folder, &self.backups_root)).then_some(folder)
    }

    fn exists(&self, folder: &Path) -> bool {
        folder.is_dir()
    }

    fn try_delete(&self, folder: &Path, cancel: &CancelToken) -> Result<bool, String> {
        try_delete_folder(folder, cancel)
    }

    fn find_lockers(&self, folder: &Path) -> Vec<Locker> {
        file_locks::find_lockers(folder)
            .into_iter()
            .map(|l| Locker { id: l.id, name: l.name, exe: l.exe, reason: l.reason, can_close: l.can_close })
            .collect()
    }

    fn close(&self, lockers: &[Locker]) -> Vec<Locker> {
        let processes: Vec<file_locks::LockingProcess> = lockers
            .iter()
            .map(|l| file_locks::LockingProcess {
                id: l.id,
                name: l.name.clone(),
                exe: l.exe.clone(),
                reason: l.reason.clone(),
                can_close: l.can_close,
            })
            .collect();
        let still: Vec<u32> = file_locks::close(&processes).into_iter().map(|p| p.id).collect();
        lockers.iter().filter(|l| still.contains(&l.id)).cloned().collect()
    }

    fn schedule_delete_on_restart(&self, folder: &Path) -> Result<(), String> {
        file_locks::schedule_delete_on_restart(folder)
    }
}

/// Deletes `folder` (read-only files too; a link in it is only unlinked), trying again with a growing wait while a
/// file is still held. Ok(false) when it couldn't; Err only for a cancel.
fn try_delete_folder(folder: &Path, cancel: &CancelToken) -> Result<bool, String> {
    for attempt in 1.. {
        cancel.check()?;
        match safe_path::delete_tree(folder) {
            Ok(()) if std::fs::symlink_metadata(folder).is_err() => return Ok(true),
            _ if attempt >= DELETE_ATTEMPTS => return Ok(false),
            _ => {}
        }
        let until = std::time::Instant::now() + delete_wait(attempt);
        while std::time::Instant::now() < until {
            cancel.check()?;
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(false)
}

/// `IKeepWarmRecords` over `dnnmanager.db`'s `keep_warm` table.
struct KeepWarm(KeepWarmRecords);

impl KeepWarmList for KeepWarm {
    fn remove(&self, site: &str) {
        self.0.remove(site);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(remove_title(&["shop".into()]), "Remove 'shop'");
        assert_eq!(remove_title(&["a".into(), "b".into()]), "Remove 2 projects");
    }

    #[test]
    fn backups_are_only_ever_inside_the_backups_folder() {
        let folders = SiteFolders { backups_root: PathBuf::from(r"C:\Users\x\Documents\DnnManager\backups") };
        assert_eq!(
            folders.backup_directory("shop"),
            Some(PathBuf::from(r"C:\Users\x\Documents\DnnManager\backups\shop"))
        );
        for bad in ["..", ".", "", r"..\..", "a/b", r"C:\x", " "] {
            assert_eq!(folders.backup_directory(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_folder_is_deleted_read_only_files_and_all_and_a_cancel_stops_the_retries() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("site");
        std::fs::create_dir_all(folder.join("bin")).unwrap();
        let file = folder.join("bin").join("x.dll");
        std::fs::write(&file, "x").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert_eq!(try_delete_folder(&folder, &CancelToken::new()), Ok(true));
        assert!(!folder.exists() && dir.path().exists());

        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(try_delete_folder(dir.path(), &cancel), Err(CANCELLED.into()));
        assert_eq!(delete_wait(1), Duration::from_millis(500));
        assert_eq!(delete_wait(8), Duration::from_secs(2));
    }

    #[test]
    fn the_database_is_read_from_web_config_and_the_container_is_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let config = |cs: &str| {
            std::fs::write(
                dir.path().join("web.config"),
                format!(r#"<configuration><connectionStrings><add name="SiteSqlServer" connectionString="{cs}" /></connectionStrings></configuration>"#),
            )
            .unwrap()
        };
        let iis: std::sync::Arc<dyn IisManager> = std::sync::Arc::new(NoIis);
        let databases = SqlDatabases {
            iis: iis.as_ref(),
            docker: Ok(DockerOptions { container_ip: "localhost".into(), default_port: 1433, ..Default::default() }),
        };
        config("Data Source=localhost,1433;Initial Catalog=shop;User ID=dnn_shop;Password=p");
        let db = databases.database_of(dir.path()).unwrap();
        assert_eq!(
            (db.kind, db.database.as_str(), db.user.as_str()),
            (SiteDatabaseKind::Container, "shop", "dnn_shop")
        );
        assert_eq!(databases.database_name_in_web_config(dir.path()).as_deref(), Some("shop"));
        assert_eq!(databases.container_server(), "localhost,1433");

        config(r"Data Source=.\SQLEXPRESS;Initial Catalog=shop;Integrated Security=True");
        assert_eq!(databases.database_of(dir.path()).unwrap().kind, SiteDatabaseKind::SqlServer);

        config(
            r"Data Source=.\SQLExpress;Integrated Security=True;User Instance=True;AttachDBFilename=|DataDirectory|Database.mdf;",
        );
        assert_eq!(databases.database_of(dir.path()), None, "DNN's shipped connection names no database of its own");
        assert_eq!(databases.database_name_in_web_config(dir.path()), None);
    }

    /// An IIS without sites - nothing here asks it to change anything.
    struct NoIis;

    impl IisManager for NoIis {
        fn is_available(&self) -> bool {
            false
        }
        fn server_state(&self) -> dnnm_application::iis::IisServerState {
            dnnm_application::iis::IisServerState::NotInstalled
        }
        fn site_runtimes(
            &self,
        ) -> Result<
            std::collections::BTreeMap<String, dnnm_application::iis::IisSiteRuntime>,
            dnnm_application::iis::IisError,
        > {
            Ok(Default::default())
        }
        fn start_site(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn stop_site(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn restart_site(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn recycle_app_pool(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn control_server(
            &self,
            _: dnnm_application::iis::IisServerAction,
        ) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn create_site(&self, _: &str, _: &Path, _: &str, _: u16) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn remove_site(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn log_directory(&self, _: &str) -> Option<PathBuf> {
            None
        }
        fn app_pool_identity(&self, site: &str) -> String {
            format!(r"IIS APPPOOL\{site}")
        }
        fn enable_user_profile(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn site_traffic(&self) -> std::collections::HashMap<String, dnnm_application::project::SiteTraffic> {
            Default::default()
        }
        fn requests_served(&self) -> std::collections::HashMap<String, u64> {
            Default::default()
        }
        fn grant_permissions(&self, _: &Path, _: &[String]) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
        fn remove_app_pool_profile(&self, _: &str) -> Result<(), dnnm_application::iis::IisError> {
            unreachable!()
        }
    }
}
