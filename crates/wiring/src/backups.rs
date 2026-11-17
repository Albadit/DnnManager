//! A project's backups for the app - the Export and Restore backup menus' texts (`ProjectMenu.cs`, `ProjectEdits.cs`),
//! where backups and deployment packages go (`AppDataPaths`), the backups Restore offers (`ProjectBackups.List`), the
//! cleanup by `backups.keepDays` (`AppDataCleaner.DeleteExpired`, run at start by `Program.DeleteExpiredBackups`) -
//! and what the backup use cases share: the site's files, SqlPackage and the databases.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dnnm_application::operation::{CancelToken, Reporter};
use dnnm_application::project::view::byte_size;
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::backups::{self, Bacpacs, Databases, ProjectBackup, SiteFiles, SqlLogin, Stamp};
use dnnm_application::use_cases::database_connection::{DatabaseConnection, DatabaseKind};
use dnnm_sql::provisioner::DatabaseProvisioner;
use dnnm_sql::{DockerOptions, SiteSqlConnection, sqlpackage};
use dnnm_system::files::{backup_filter, copier, safe_path};

use crate::Services;

/// The project menu's texts for backups, word for word.
pub mod texts {
    /// The submenu (`Export ▸`).
    pub const EXPORT: &str = "Export";
    pub const FOR_DEPLOYMENT: &str = "For deployment…";
    pub const FOR_DEPLOYMENT_TOOLTIP: &str = "A package for the live server: the site's files with web.config ready for it, the database with the live domain, and DEPLOY.txt.";
    pub const BACKUP_BOTH: &str = "Site and database  (.zip + .bacpac)";
    pub const BACKUP_SITE: &str = "Site files  (.zip)";
    pub const BACKUP_DATABASE: &str = "Database  (.bacpac)";
    /// Enabled while the project's backups folder is there.
    pub const OPEN_BACKUPS_FOLDER: &str = "Open backups folder";
    /// The submenu (`Restore backup ▸`): enabled while the site's folder is there; its items are read as it opens.
    pub const RESTORE_BACKUP: &str = "Restore backup";
    pub const RESTORE_BACKUP_TOOLTIP: &str = "Puts the site and its database back from a backup.";
    /// While the backups are read.
    pub const READING: &str = "Reading…";
    /// The only (disabled) item when there is no complete backup.
    pub const NO_BACKUP: &str = "No backup with the site and its database yet";
    /// The action of the toasts of a backup and a deployment package.
    pub const OPEN_FOLDER: &str = "Open folder";

    /// A backup's operation title.
    pub fn backup_title(site: &str) -> String {
        format!("Back up '{site}'")
    }

    /// A backup's success toast (with [`OPEN_FOLDER`], which opens `folder`).
    pub fn backup_done(site: &str, folder: &std::path::Path) -> String {
        format!("'{site}' is backed up in {}.", folder.display())
    }

    pub fn deployment_title(site: &str) -> String {
        format!("Export '{site}' for deployment")
    }

    /// Its success toast (with [`OPEN_FOLDER`], which opens the request's output folder).
    pub fn deployment_done(site: &str) -> String {
        format!("The deployment package of '{site}' is ready.")
    }
}

/// What the Export menu backs up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    Both,
    Site,
    Database,
}

impl BackupKind {
    pub const ALL: [BackupKind; 3] = [BackupKind::Both, BackupKind::Site, BackupKind::Database];

    /// Its item in the Export menu.
    pub fn label(self) -> &'static str {
        match self {
            BackupKind::Both => texts::BACKUP_BOTH,
            BackupKind::Site => texts::BACKUP_SITE,
            BackupKind::Database => texts::BACKUP_DATABASE,
        }
    }
}

/// `Documents\DnnManager\backups`.
pub fn backups_root() -> PathBuf {
    dnnm_infrastructure::paths::app_data_directory().join("backups")
}

/// `Documents\DnnManager\deployments`.
pub fn deployments_root() -> PathBuf {
    dnnm_infrastructure::paths::app_data_directory().join("deployments")
}

/// The site's backups folder - what Open backups folder opens (`DnnProject.BackupDirectory`).
pub fn backups_folder(site: &str) -> PathBuf {
    backups_root().join(site)
}

/// Now on this PC's clock.
pub fn now() -> Stamp {
    stamp_of(SystemTime::now())
}

fn stamp_of(at: SystemTime) -> Stamp {
    let t = dnnm_diagnostics::format::LocalDateTime::from_system_time(at);
    Stamp {
        year: t.year,
        month: t.month as u8,
        day: t.day as u8,
        hour: t.hour as u8,
        minute: t.minute as u8,
        second: t.second as u8,
    }
}

/// Every dated backup of the site, newest first. Reads the disk: off the UI thread.
pub fn list_backups(site: &str) -> Vec<ProjectBackup> {
    backups::list(&backups_folder(site), site, &stamp_of)
}

/// What Restore backup offers: the complete backups (the site's files and its database), newest first - read when
/// the submenu opens. None of them: one disabled [`texts::NO_BACKUP`].
pub fn restorable_backups(site: &str) -> Vec<ProjectBackup> {
    list_backups(site).into_iter().filter(ProjectBackup::is_complete).collect()
}

/// What [`delete_expired_backups`] did: each folder deleted, what that freed, and how many files couldn't be deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpiredBackups {
    pub deleted: Vec<PathBuf>,
    pub freed_bytes: u64,
    pub skipped: usize,
    pub keep_days: u32,
}

impl ExpiredBackups {
    /// The Output's info line - None when nothing was deleted.
    pub fn info(&self) -> Option<String> {
        if self.deleted.is_empty() {
            return None;
        }
        let names: Vec<String> =
            self.deleted.iter().map(|f| f.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        Some(format!(
            "Deleted {} backup(s) and deployment package(s) older than {} days ({}, Settings → Projects → Backups): {}.",
            self.deleted.len(),
            self.keep_days,
            byte_size(self.freed_bytes as f64),
            names.join(", ")
        ))
    }

    /// The Output's warning - None when every file went.
    pub fn warning(&self) -> Option<String> {
        (self.skipped > 0).then(|| {
            format!(
                "{} file(s) of backups older than {} days couldn't be deleted (in use) - they are tried again at the next start.",
                self.skipped, self.keep_days
            )
        })
    }
}

/// Setting `backups.keepDays` (0, the default, keeps them for good).
pub fn keep_days(settings: &Settings) -> u32 {
    settings.get("backups.keepDays").and_then(|d| d.trim().parse::<u32>().ok()).unwrap_or(0).min(3650)
}

/// At start, off the UI thread: the project backups (`backups\<project>\<project>_<date>\`) and deployment packages
/// (`deployments\<project>_<date>\`) made more than `backups.keepDays` days ago are deleted - nothing when it is 0. A
/// folder's age is the date in its name, or when it was last written to. Never through a junction or link.
pub fn delete_expired_backups(settings: &Settings) -> ExpiredBackups {
    delete_expired(&backups_root(), &deployments_root(), keep_days(settings), now())
}

pub(crate) fn delete_expired(
    backups_root: &Path,
    deployments_root: &Path,
    keep_days: u32,
    now: Stamp,
) -> ExpiredBackups {
    let mut result = ExpiredBackups { keep_days, ..Default::default() };
    if keep_days == 0 {
        return result;
    }
    let before = now.minus_days(keep_days);
    let mut expired = Vec::new();
    if usable(backups_root) {
        for project in safe_folders(backups_root) {
            expired.extend(safe_folders(&project).into_iter().filter(|f| made_at(f) < before));
        }
    }
    if usable(deployments_root) {
        expired.extend(safe_folders(deployments_root).into_iter().filter(|f| made_at(f) < before));
    }
    for folder in expired {
        let (freed, left) = delete_files(&folder);
        result.freed_bytes += freed;
        result.skipped += left;
        delete_empty_folders(&folder);
        if left == 0 {
            result.deleted.push(folder);
        }
    }
    // A project with no backup left keeps no empty folder.
    if usable(backups_root) {
        for project in safe_folders(backups_root) {
            delete_empty_folders(&project);
        }
    }
    result
}

fn usable(folder: &Path) -> bool {
    folder.is_dir() && !safe_path::is_link(folder)
}

/// The folders in `folder`, but junctions and links; none when it can't be read.
fn safe_folders(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| !safe_path::is_link(p) && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .collect()
}

/// When a backup or package folder was made: the date in its name, or when it was last written to.
fn made_at(folder: &Path) -> Stamp {
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    Stamp::from_folder_name(&name).unwrap_or_else(|| {
        std::fs::metadata(folder).and_then(|m| m.modified()).map(stamp_of).unwrap_or(Stamp {
            year: 9999,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
        })
    })
}

/// Deletes every file under `folder` - never through a link, which is deleted itself: what that freed, and how many
/// couldn't be deleted (in use, or not allowed).
fn delete_files(folder: &Path) -> (u64, usize) {
    let (mut freed, mut skipped) = (0, 0);
    let Ok(entries) = std::fs::read_dir(folder) else { return (0, 0) };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.is_dir() && !safe_path::is_link(&path) {
            let (f, s) = delete_files(&path);
            freed += f;
            skipped += s;
            continue;
        }
        if safe_path::is_link(&path) {
            // The link, not what it points to.
            if std::fs::remove_dir(&path).or_else(|_| std::fs::remove_file(&path)).is_err() {
                skipped += 1;
            }
            continue;
        }
        let mut permissions = meta.permissions();
        if permissions.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&path, permissions);
        }
        match std::fs::remove_file(&path) {
            Ok(()) => freed += meta.len(),
            Err(_) => skipped += 1,
        }
    }
    (freed, skipped)
}

/// Deletes `folder` and the folders in it that are empty, deepest first.
fn delete_empty_folders(folder: &Path) {
    if safe_path::is_link(folder) {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                delete_empty_folders(&path);
            }
        }
    }
    let _ = std::fs::remove_dir(folder);
}

// ─── What the use cases share ───────────────────────────────────────────

/// The site's files through `dnnm-system`'s copier.
pub(crate) struct CopierFiles;

impl SiteFiles for CopierFiles {
    fn backup_filter(&self, site: &Path) -> Result<Vec<String>, String> {
        match std::fs::read_to_string(site.join(backup_filter::FILE_NAME)) {
            Ok(text) => Ok(backup_filter::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn create_zip(
        &self,
        site: &Path,
        zip: &Path,
        excluded: &[String],
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        // The status line's progress has no place on the port's Reporter (see copier.rs).
        copier::create_zip(site, zip, excluded, report, &mut |_| {}, cancel)
    }
}

/// The local SQL container's settings and Settings → Database server, as `LocalSqlContainer` reads them.
pub(crate) struct SqlSettings {
    pub docker: Result<DockerOptions, String>,
    pub settings: Settings,
}

impl SqlSettings {
    pub fn new(services: &Services) -> Self {
        Self { docker: crate::remove_project::docker_options(services), settings: services.settings.clone() }
    }

    fn docker(&self) -> Result<&DockerOptions, String> {
        self.docker.as_ref().map_err(Clone::clone)
    }

    /// The local SQL Server's address (`ip,port`).
    fn container_server(&self) -> String {
        let host = self.settings.get("sqlServer.host").map(str::trim).filter(|h| !h.is_empty()).unwrap_or("localhost");
        let port = self.settings.get("sqlServer.port").and_then(|p| p.trim().parse::<u16>().ok()).unwrap_or(1433);
        format!("{host},{port}")
    }

    /// The database the site uses, as its web.config has it (`LocalSqlContainer.DatabaseOf`).
    pub fn database_of(&self, site: &Path) -> Option<DatabaseConnection> {
        crate::change_database_connection::database_of(&self.settings, site)
    }

    /// Settings → Database server, for whether DNN Manager may sign in to a server as the user.
    pub fn database_server(&self) -> dnnm_sql::DatabaseServerOptions {
        dnnm_sql::DatabaseServerOptions {
            kind: self.settings.get("sqlServer.type").unwrap_or("container").trim().to_string(),
            server: self.settings.get("sqlServer.server").unwrap_or_default().trim().to_string(),
        }
    }

    /// `database` on the local container, as its user (`LocalSqlContainer.Connection`).
    pub fn container_login(&self, database: &str) -> Result<SqlLogin, String> {
        let docker = self.docker()?;
        Ok(SqlLogin::new(&self.container_server(), database, &docker.sql_user, &docker.sa_password))
    }
}

/// How to reach `database` - as its login, or Windows authentication (as you) (`LocalSqlContainer.SiteConnection`).
pub(crate) fn site_login(database: &DatabaseConnection) -> SqlLogin {
    if database.uses_windows_authentication() {
        SqlLogin::new(&database.server, &database.database, "", "")
    } else {
        SqlLogin::new(&database.server, &database.database, &database.user, &database.password)
    }
}

/// SqlPackage through `dnnm-sql`.
pub(crate) struct SqlPackage(pub SqlSettings);

impl Bacpacs for SqlPackage {
    fn export_source(&self, project: &str, site: &Path) -> Result<SqlLogin, String> {
        match self.0.database_of(site) {
            Some(file) if file.kind == DatabaseKind::LocalDbFile => Err(format!(
                "The site's database is a LocalDB file (App_Data\\{}), which can't be exported as .bacpac - move it to SQL Server first (Details → Database → Change connection).",
                file.database
            )),
            // SqlPackage puts the login of every user in the database into the .bacpac, and fails (SQL71501) on one it
            // can't see. The container's user sees them all.
            Some(container) if container.kind == DatabaseKind::Container => {
                self.0.container_login(&container.database).map(|login| login.with_database(&container.database))
            }
            Some(database) => Ok(site_login(&database)),
            // Without a database of its own: its database on the local SQL Server (`ConnectionOf`).
            None => self.0.container_login(project),
        }
    }

    fn ensure_available(&self, report: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String> {
        sqlpackage::ensure_available(report, cancel)
    }

    fn export(
        &self,
        source: &SqlLogin,
        bacpac: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let source = SiteSqlConnection::new(&source.server, &source.database, &source.user, &source.password);
        sqlpackage::export(&source, bacpac, report, cancel)
    }

    fn import(
        &self,
        target: &SqlLogin,
        bacpac: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        sqlpackage::import(&target.server, &target.user, &target.password, &target.database, bacpac, report, cancel)
    }
}

/// The SQL crate's connection for a login: SQL Server authentication with a user, Windows without one.
fn connection(login: &SqlLogin) -> dnnm_sql::DatabaseConnection {
    let authentication = if login.uses_windows_authentication() {
        dnnm_sql::SqlAuthentication::Windows
    } else {
        dnnm_sql::SqlAuthentication::Sql
    };
    dnnm_sql::DatabaseConnection::new(dnnm_sql::DatabaseKind::SqlServer, &login.server, &login.database, authentication)
        .with_login(&login.user, &login.password)
}

/// Whole databases through `dnnm-sql`'s provisioner.
pub(crate) struct SqlDatabases;

impl Databases for SqlDatabases {
    fn exists(&self, database: &SqlLogin, cancel: &CancelToken) -> Result<bool, String> {
        DatabaseProvisioner.database_exists(&connection(database), Some(cancel))
    }

    fn drop_database(&self, database: &SqlLogin) -> Result<(), String> {
        DatabaseProvisioner.drop_database(&connection(database), None)
    }

    fn rename(&self, database: &SqlLogin, new_name: &str) -> Result<(), String> {
        DatabaseProvisioner.rename_database(&connection(database), new_name, None)
    }

    fn replace_portal_aliases(
        &self,
        database: &SqlLogin,
        aliases: &[String],
        cancel: &CancelToken,
    ) -> Result<(), String> {
        DatabaseProvisioner.replace_portal_aliases(&connection(database), aliases, Some(cancel))
    }

    fn set_ssl(&self, database: &SqlLogin, on: bool, cancel: &CancelToken) -> Result<(), String> {
        DatabaseProvisioner.set_ssl(&connection(database), on, Some(cancel))
    }
}

/// A reporter and a prompt for the backup use cases' tests here (all Windows-only).
#[cfg(all(test, windows))]
pub(crate) mod test_support {
    use dnnm_application::operation::{Prompt, Reporter};

    #[derive(Default)]
    pub struct Lines(pub Vec<String>);

    impl Reporter for Lines {
        fn step(&mut self, t: &str) {
            self.0.push(format!("## {t}"));
        }
        fn info(&mut self, t: &str) {
            self.0.push(t.into());
        }
        fn success(&mut self, t: &str) {
            self.0.push(format!("ok {t}"));
        }
        fn warn(&mut self, t: &str) {
            self.0.push(format!("warn {t}"));
        }
        fn fail(&mut self, t: &str) {
            self.0.push(format!("! {t}"));
        }
    }

    pub struct Yes;

    impl Prompt for Yes {
        fn confirm(&self, _: &str, _: &str, _: &str, _: bool) -> bool {
            true
        }
        fn choose(&self, _: &str, _: &[&str]) -> Option<usize> {
            Some(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(text: &str) -> Stamp {
        Stamp::from_folder_name(&format!("x_{text}")).unwrap()
    }

    #[test]
    fn expired_backups_and_packages_go_the_rest_stays() {
        let dir = tempfile::tempdir().unwrap();
        let (backups, deployments) = (dir.path().join("backups"), dir.path().join("deployments"));
        let make = |folder: PathBuf| {
            std::fs::create_dir_all(folder.join("sub")).unwrap();
            std::fs::write(folder.join("shop.zip"), "12345").unwrap();
            std::fs::write(folder.join("sub").join("x.txt"), "1").unwrap();
        };
        make(backups.join("shop").join("shop_20260101_090000"));
        make(backups.join("shop").join("shop_20261110_090000"));
        make(backups.join("old").join("old_20250101_090000"));
        make(deployments.join("shop_20260201_120000"));
        make(deployments.join("shop_20261115_120000"));

        let now = stamp("20261116_120000");
        assert_eq!(delete_expired(&backups, &deployments, 0, now), ExpiredBackups::default());
        let result = delete_expired(&backups, &deployments, 30, now);
        let mut names: Vec<_> =
            result.deleted.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["old_20250101_090000", "shop_20260101_090000", "shop_20260201_120000"]);
        assert_eq!((result.freed_bytes, result.skipped), (18, 0));
        assert!(backups.join("shop").join("shop_20261110_090000").join("shop.zip").exists());
        assert!(deployments.join("shop_20261115_120000").exists());
        assert!(!backups.join("old").exists(), "a project with no backup left keeps no empty folder");
        assert_eq!(result.warning(), None);
        let info = result.info().unwrap();
        assert!(info.starts_with(
            "Deleted 3 backup(s) and deployment package(s) older than 30 days (18 B, Settings → Projects → Backups): "
        ));
    }

    #[test]
    fn keep_days_reads_the_setting() {
        let mut settings = Settings::default();
        assert_eq!(keep_days(&settings), 0);
        settings.set("backups.keepDays", "30");
        assert_eq!(keep_days(&settings), 30);
        settings.set("backups.keepDays", "-1");
        assert_eq!(keep_days(&settings), 0);
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn the_menu_texts_are_the_wpf_apps() {
        assert_eq!(
            BackupKind::ALL.map(BackupKind::label),
            ["Site and database  (.zip + .bacpac)", "Site files  (.zip)", "Database  (.bacpac)"]
        );
        assert_eq!(texts::backup_title("shop"), "Back up 'shop'");
        assert_eq!(texts::backup_done("shop", Path::new(r"C:\b")), r"'shop' is backed up in C:\b.");
        assert!(backups_folder("shop").ends_with(r"DnnManager\backups\shop"));
    }

    #[test]
    fn the_backup_filter_is_read_or_said_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(CopierFiles.backup_filter(dir.path()).unwrap(), Vec::<String>::new());
        std::fs::write(dir.path().join("_backup.filter"), "\\site\\wwwroot\\App_Data\\Search\r\n\r\nPortals/0/Cache")
            .unwrap();
        assert_eq!(CopierFiles.backup_filter(dir.path()).unwrap(), [r"App_Data\Search", r"Portals\0\Cache"]);
        std::fs::remove_file(dir.path().join("_backup.filter")).unwrap();
        std::fs::create_dir(dir.path().join("_backup.filter")).unwrap();
        assert!(CopierFiles.backup_filter(dir.path()).is_err(), "a folder of that name can't be read");
    }
}
