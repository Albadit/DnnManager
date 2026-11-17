//! Restore backup ▸ <backup> (`ProjectEdits.RestoreBackup`, `RestoreBackupUseCase`, its `CheckZip`): asks first, then
//! puts the site's files and its database back as the backup has them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::operation::{CancelToken, Operation, Reporter};
use dnnm_application::use_cases::backups::ProjectBackup;
use dnnm_application::use_cases::database_connection::DatabaseKind;
use dnnm_application::use_cases::restore_backup::{
    self as use_case, ExtractError, RestoreBackupDeps, RestoreBackupRequest, RestoreSite, RestoreTarget, SIGN_IN_WHY,
};
use dnnm_infrastructure::app_db::ProjectTable;
use dnnm_sql::SqlServerAddress;
use dnnm_system::files::{backup_filter, copier, file_locks, safe_path, safe_zip};

use crate::Services;
use crate::backups::{SqlDatabases, SqlPackage, SqlSettings, site_login};

/// One item of Restore backup ▸: the site, its folder and the (complete) backup chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreChoice {
    pub site_name: String,
    pub directory: PathBuf,
    pub backup: ProjectBackup,
}

/// The operation's title: `Restore 'shop' from 2026-09-28 15:42`.
pub fn restore_title(site: &str, backup: &ProjectBackup) -> String {
    format!("Restore '{site}' from {}", backup.created.display_minutes())
}

/// Its success toast.
pub fn restore_done(site: &str, backup: &ProjectBackup) -> String {
    format!("'{site}' is back as it was on {}.", backup.created.display_minutes())
}

/// Asks (a danger confirmation: "Restore backup" / "Cancel"), then restores. Only complete backups are offered
/// ([`crate::backups::restorable_backups`]).
pub fn restore_backup(services: &Services, choice: RestoreChoice, op: &mut Operation) -> Result<(), String> {
    let request = RestoreBackupRequest::from_backup(&choice.site_name, &choice.directory, &choice.backup)
        .ok_or_else(|| format!("The backup in {} has no site files or no database.", choice.backup.folder.display()))?;
    use_case::confirm(&choice.site_name, &choice.backup, op)?;
    restore_backup_request(services, request, op)
}

/// Restores without asking - for a flow that asked already (an upgrade putting a step's backup back).
pub fn restore_backup_request(
    services: &Services,
    request: RestoreBackupRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let sql = SqlSettings::new(services);
    let site = SiteFolder(SqlSettings::new(services));
    let deps = RestoreBackupDeps {
        iis: services.iis.as_ref(),
        site: &site,
        bacpacs: &SqlPackage(sql),
        databases: &SqlDatabases,
        records: &ProjectTable(services.db.clone()),
        now: crate::backups::now(),
        retry_delay: Duration::from_secs(3),
    };
    use_case::restore_backup(&deps, &request, op)
}

/// The site's folder and database through the system, SQL and infrastructure crates.
struct SiteFolder(SqlSettings);

impl RestoreSite for SiteFolder {
    fn check_zip(&self, zip: &Path, directory: &Path) -> Option<String> {
        check_zip(zip, directory)
    }

    fn backup_filter(&self, directory: &Path) -> Vec<String> {
        backup_filter::read(directory)
    }

    fn extract_zip(&self, zip: &Path, directory: &Path, report: &mut dyn Reporter) -> Result<(), ExtractError> {
        copier::extract_zip(zip, directory, report, &mut |_| {}, &CancelToken::new()).map_err(|e| match e {
            copier::ExtractError::InUse(m) => ExtractError::InUse(m),
            copier::ExtractError::Failed(m) => ExtractError::Failed(m),
        })
    }

    fn remove_files_not_in_zip(
        &self,
        zip: &Path,
        directory: &Path,
        excluded: &[String],
        report: &mut dyn Reporter,
    ) -> Result<usize, String> {
        copier::remove_files_not_in_zip(zip, directory, excluded, report, &CancelToken::new())
    }

    fn stop_programs_running_from(&self, directory: &Path) -> Vec<String> {
        file_locks::stop_programs_running_from(directory)
    }

    fn database_target(&self, _site_name: &str, directory: &Path) -> RestoreTarget {
        let Some(database) = self.0.database_of(directory) else { return RestoreTarget::None };
        match database.kind {
            DatabaseKind::LocalDbFile => RestoreTarget::LocalDbFile,
            // The container's user may create databases; the site's own login may not.
            DatabaseKind::Container => match self.0.container_login(&database.database) {
                Ok(login) => RestoreTarget::Server { login, sign_in_question: None },
                // Without the sa password the site's own login is tried - and says why it can't.
                Err(_) => RestoreTarget::Server { login: site_login(&database), sign_in_question: None },
            },
            DatabaseKind::SqlServer => {
                let login = site_login(&database);
                let ask = login.uses_windows_authentication()
                    && !SqlServerAddress::may_sign_in_as_user(&login.server, &self.0.database_server());
                let sign_in_question = ask.then(|| SqlServerAddress::sign_in_question(&login.server, SIGN_IN_WHY));
                RestoreTarget::Server { login, sign_in_question }
            }
        }
    }

    fn dnn_version(&self, directory: &Path) -> Option<String> {
        dnnm_infrastructure::dnn_install::dnn_version(directory)
    }
}

/// Why `zip` can't be put back into `directory` - or None when every file in it can: none outside the folder ("..",
/// a full path, a drive), none through a link or junction in the folder, none that is itself a link (a zip made on
/// Linux can hold one). The same files as the extraction writes: those under the zip's site root (its shallowest
/// folder with a web.config).
pub(crate) fn check_zip(zip: &Path, directory: &Path) -> Option<String> {
    let invalid = || format!("The backup's site files aren't a valid zip: {}", zip.display());
    let file = match std::fs::File::open(zip) {
        Ok(file) => file,
        Err(e) => return Some(format!("Could not read the backup's site files: {e}")),
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else { return Some(invalid()) };
    let mut files = Vec::new();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index_raw(i) else { return Some(invalid()) };
        let Ok(raw) = entry.name() else { return Some(invalid()) };
        let raw = raw.to_string();
        if !safe_zip::is_folder(&raw) {
            files.push((raw, entry.unix_mode()));
        }
    }
    if files.is_empty() {
        return Some("The backup's site files are empty.".into());
    }
    let root = files
        .iter()
        .map(|(raw, _)| safe_zip::name(raw))
        .filter(|n| safe_zip::file_name(n).eq_ignore_ascii_case("web.config"))
        .min_by_key(|n| n.matches('/').count())
        .map(|n| n[..n.len() - "web.config".len()].to_string())
        .unwrap_or_default();
    for (raw, mode) in &files {
        let name = safe_zip::name(raw);
        if name.len() < root.len()
            || !name.is_char_boundary(root.len())
            || !name[..root.len()].eq_ignore_ascii_case(&root)
        {
            continue;
        }
        // The Unix file type in the upper bits: 0xA000 is a symbolic link.
        if mode.is_some_and(|m| m & 0xF000 == 0xA000) {
            return Some(format!("The backup's site files hold a link ({raw}) - DNN Manager doesn't put links back."));
        }
        if safe_path::under(directory, &name[root.len()..], None).is_none() {
            return Some(format!(
                "The backup's site files hold a file that would land outside the site's folder, or go through a link or junction in it: {raw}"
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(path: &Path, files: &[&str]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for name in files {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(b"x").unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_zip_is_checked_before_a_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir(&site).unwrap();
        let zip = dir.path().join("shop.zip");
        zip_with(&zip, &["wwwroot/web.config", "wwwroot/bin/x.dll", "readme.txt"]);
        assert_eq!(check_zip(&zip, &site), None, "what is outside the site root isn't put back");
        zip_with(&zip, &["web.config", "../evil.dll"]);
        assert!(check_zip(&zip, &site).unwrap().ends_with("go through a link or junction in it: ../evil.dll"));
        zip_with(&zip, &[]);
        assert_eq!(check_zip(&zip, &site).unwrap(), "The backup's site files are empty.");
        std::fs::write(&zip, "not a zip").unwrap();
        assert!(check_zip(&zip, &site).unwrap().starts_with("The backup's site files aren't a valid zip: "));
    }

    #[test]
    fn a_link_in_the_zip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("shop.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&zip).unwrap());
        writer.start_file("web.config", zip::write::SimpleFileOptions::default()).unwrap();
        writer.add_symlink("bin", "/etc", zip::write::SimpleFileOptions::default()).unwrap();
        writer.finish().unwrap();
        assert_eq!(
            check_zip(&zip, dir.path()).unwrap(),
            "The backup's site files hold a link (bin) - DNN Manager doesn't put links back."
        );
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn a_real_backup_round_trip_puts_the_files_back() {
        // Zip a site, change it, then extract and remove what came after - in temporary folders only.
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join("bin")).unwrap();
        std::fs::write(site.join("web.config"), "<configuration />").unwrap();
        std::fs::write(site.join("bin").join("x.dll"), "v1").unwrap();
        let zip = dir.path().join("shop.zip");
        let mut lines = crate::backups::test_support::Lines::default();
        copier::create_zip(&site, &zip, &[], &mut lines, &mut |_| {}, &CancelToken::new()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(site.join("bin").join("x.dll"), "v2").unwrap();
        std::fs::write(site.join("added.txt"), "new").unwrap();
        let folder = SiteFolder(SqlSettings { docker: Err("not used".into()), settings: Default::default() });
        assert_eq!(folder.check_zip(&zip, &site), None);
        folder.extract_zip(&zip, &site, &mut lines).unwrap();
        let removed = folder.remove_files_not_in_zip(&zip, &site, &[".git".into()], &mut lines).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(std::fs::read_to_string(site.join("bin").join("x.dll")).unwrap(), "v1");
        assert!(!site.join("added.txt").exists());
    }

    #[test]
    fn title_and_toast_name_the_backup() {
        let backup = ProjectBackup {
            project: "shop".into(),
            folder: PathBuf::from(r"D:\b\shop_20260928_154210"),
            created: dnnm_application::use_cases::backups::Stamp::from_folder_name("x_20260928_154210").unwrap(),
            site_zip: None,
            database: None,
            note: None,
        };
        assert_eq!(restore_title("shop", &backup), "Restore 'shop' from 2026-09-28 15:42");
        assert_eq!(restore_done("shop", &backup), "'shop' is back as it was on 2026-09-28 15:42.");
    }
}
