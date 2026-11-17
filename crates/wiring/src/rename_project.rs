//! Rename project joined to IIS, SQL Server, web.config, the projects folder and `dnnmanager.db`
//! (`RenameProjectUseCase.cs`, `ProjectEdits.Rename`), with the texts the app offers it with (`ProjectMenu.cs`,
//! `ProjectView.xaml`, `MainWindow.Commands.cs`).
//!
//! Also what Edit host names shares: the site's database through `dnnm-sql` and web.config ([`SqlSiteDatabases`]),
//! with Change connection's `database_of`, `to_sql` and `write_site_sql_server`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dnnm_application::iis::IisBinding;
use dnnm_application::operation::{CancelToken, Operation};
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::database_connection::DatabaseConnection;
use dnnm_application::use_cases::rename_project::{
    self as use_case, KeepWarmTable, ProjectFolders, RenameDeps, RenameDialogFacts, RenameProjectRequest, SiteDatabases,
};
use dnnm_infrastructure::app_db::ProjectTable;
use dnnm_keepwarm::KeepWarmRecords;
use dnnm_sql::SqlServerAddress;
use dnnm_sql::connection_strings;
use dnnm_sql::model::DatabaseServerOptions;
use dnnm_sql::provisioner::DatabaseProvisioner;

use crate::Services;
use crate::change_database_connection::{database_of, to_sql, write_site_sql_server};

/// The command palette's entry (`MainWindow.Commands.cs`).
pub const COMMAND_ID: &str = "project.rename";
pub const COMMAND: &str = "Rename project…";
pub const COMMAND_KEYWORDS: &str = "name folder iis site app pool";
/// The project's menu (`ProjectMenu.cs`).
pub const MENU_ITEM: &str = "Rename…";
/// The Details' General tab (`ProjectView.xaml`).
pub const BUTTON: &str = "Rename…";
pub const BUTTON_TOOLTIP: &str =
    "Rename the project: its folder, IIS site and app pool - and its host name and database if you like";

/// The operation's title.
pub fn title(name: &str, new_name: &str) -> String {
    format!("Rename '{name}' → '{new_name}'")
}

/// The toast once it is done.
pub fn done_toast(name: &str, new_name: &str) -> String {
    format!("'{name}' is '{new_name}' now.")
}

/// `ProjectEdits.CanEdit`: not while another operation runs, and only for a site whose folder is there - for Rename,
/// Edit host names and Edit app pool alike. Reads the disk: call it where a folder check is fine (a menu opening).
pub fn can_edit(busy: bool, folder: &Path) -> bool {
    !busy && folder.is_dir()
}

/// The host name suffix of Settings → Projects (`AppOptions.HostnameSuffix`).
pub(crate) fn hostname_suffix(services: &Services) -> String {
    services.settings.get("projects.hostnameSuffix").unwrap_or("dnndev.me").to_string()
}

/// What the Rename dialog shows for the project `name` serving `directory` with `bindings` (`ProjectEdits.Rename`).
/// Reads IIS and the site's web.config: call it off the UI thread.
pub fn dialog_facts(services: &Services, name: &str, directory: &Path, bindings: &[IisBinding]) -> RenameDialogFacts {
    let sites = services.iis.site_states().into_keys().collect();
    let database = SqlSiteDatabases::new(services).database_of(directory);
    RenameDialogFacts::new(name, sites, bindings, &hostname_suffix(services), database.as_ref())
}

/// Runs Rename project.
pub fn rename_project(services: &Services, request: RenameProjectRequest, op: &mut Operation) -> Result<(), String> {
    let deps = RenameDeps {
        iis: services.iis.clone(),
        folders: Arc::new(FileProjectFolders { base: PathBuf::from(services.settings.base_directory()) }),
        databases: Arc::new(SqlSiteDatabases::new(services)),
        records: Arc::new(ProjectTable(services.db.clone())),
        keep_warm: Arc::new(KeepWarm(KeepWarmRecords::new(services.db.path()))),
        hostname_suffix: hostname_suffix(services),
    };
    use_case::rename_project(&deps, &request, op)
}

/// The projects folder of the settings (`FileSystemProjectRepository`).
struct FileProjectFolders {
    base: PathBuf,
}

impl ProjectFolders for FileProjectFolders {
    fn project_directory(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn exists(&self, folder: &Path) -> bool {
        folder.is_dir()
    }

    fn move_folder(&self, from: &Path, to: &Path) -> Result<(), String> {
        // Directory.Move: refused when `to` exists; a link at `from` is moved as the link, never followed.
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(format!("{} exists already.", to.display()));
        }
        std::fs::rename(from, to).map_err(|e| e.to_string())
    }
}

/// The `keep_warm` table.
struct KeepWarm(KeepWarmRecords);

impl KeepWarmTable for KeepWarm {
    fn find(&self, site: &str) -> Option<String> {
        self.0.find(site)
    }

    fn save(&self, site: &str) {
        self.0.save(site, true);
    }

    fn remove(&self, site: &str) {
        self.0.remove(site);
    }
}

/// A site's database through `dnnm-sql`, and web.config written as the site reads it.
pub(crate) struct SqlSiteDatabases {
    /// For which server is the local container.
    settings: Settings,
    /// Settings → Database server: whether signing in to a server as the user needs asking.
    chosen: DatabaseServerOptions,
}

impl SqlSiteDatabases {
    pub(crate) fn new(services: &Services) -> Self {
        // As the Projects table's database check reads them (`DatabaseServerOptions`).
        let chosen = DatabaseServerOptions {
            kind: services.settings.get("sqlServer.type").unwrap_or("container").to_string(),
            server: services.settings.get("sqlServer.server").unwrap_or_default().to_string(),
        };
        Self { settings: services.settings.clone(), chosen }
    }
}

impl SiteDatabases for SqlSiteDatabases {
    fn database_of(&self, directory: &Path) -> Option<DatabaseConnection> {
        database_of(&self.settings, directory)
    }

    fn rename(&self, database: &DatabaseConnection, new_name: &str, cancel: &CancelToken) -> Result<(), String> {
        DatabaseProvisioner.rename_database(&to_sql(database), new_name, Some(cancel))
    }

    fn write_connection(&self, web_config: &Path, database: &DatabaseConnection) -> Result<(), String> {
        write_site_sql_server(web_config, &connection_strings::for_site(&to_sql(database)))
    }

    fn grant_site_access(
        &self,
        database: &DatabaseConnection,
        login: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        DatabaseProvisioner.grant_site_access(&to_sql(database), login, Some(cancel))
    }

    fn update_portal_aliases(
        &self,
        database: &DatabaseConnection,
        site_directory: &Path,
        renamed: &[(String, String)],
        added: &[String],
        cancel: &CancelToken,
    ) -> Result<i64, String> {
        DatabaseProvisioner.update_portal_aliases(&to_sql(database), site_directory, renamed, added, Some(cancel))
    }

    fn may_sign_in_as_user(&self, server: &str) -> bool {
        SqlServerAddress::may_sign_in_as_user(server, &self.chosen)
    }

    fn sign_in_question(&self, server: &str, why: &str) -> String {
        SqlServerAddress::sign_in_question(server, why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_move_but_never_onto_one_that_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let folders = FileProjectFolders { base: dir.path().to_path_buf() };
        let (a, b) = (folders.project_directory("a"), folders.project_directory("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        assert!(folders.move_folder(&a, &b).unwrap_err().ends_with("exists already."));
        std::fs::remove_dir(&b).unwrap();
        folders.move_folder(&a, &b).unwrap();
        assert!(folders.exists(&b) && !folders.exists(&a));
    }

    #[test]
    fn texts() {
        assert_eq!(title("shop", "store"), "Rename 'shop' → 'store'");
        assert_eq!(done_toast("shop", "store"), "'shop' is 'store' now.");
        assert!(!can_edit(true, Path::new(".")));
        assert!(can_edit(false, &std::env::temp_dir()));
    }
}
