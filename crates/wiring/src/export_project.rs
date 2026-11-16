//! Export ▸ Site and database / Site files / Database (`ProjectMenu.Backup`): a dated backup into the project's backups
//! folder, `Documents\DnnManager\backups\<project>\<project>_<yyyyMMdd_HHmmss>\` with `<project>.zip` and / or
//! `<project>.bacpac` (`ExportProjectUseCase`).

use std::path::{Path, PathBuf};

use dnnm_application::operation::Operation;
use dnnm_application::use_cases::backups::{self, Stamp};
use dnnm_application::use_cases::export_project::{self as use_case, ExportProjectDeps, ExportProjectRequest};

use crate::Services;
use crate::backups::{BackupKind, CopierFiles, SqlPackage, SqlSettings, backups_folder};

/// The request for a backup of `kind` taken `now`, and the folder it goes to (for the toast's Open folder).
pub fn backup_request(site: &str, directory: &Path, kind: BackupKind, now: Stamp) -> (ExportProjectRequest, PathBuf) {
    let folder = backups::new_folder(&backups_folder(site), site, now);
    let request = ExportProjectRequest {
        project_name: site.to_string(),
        project_directory: directory.to_path_buf(),
        zip_path: (kind != BackupKind::Database).then(|| folder.join(backups::site_zip_name(site))),
        bacpac_path: (kind != BackupKind::Site).then(|| folder.join(backups::database_name(site, ".bacpac"))),
    };
    (request, folder)
}

/// Runs the backup; the app titles it [`crate::backups::texts::backup_title`] and shows
/// [`crate::backups::texts::backup_done`] when it is done.
pub fn export_project(services: &Services, request: ExportProjectRequest, op: &mut Operation) -> Result<(), String> {
    let bacpacs = SqlPackage(SqlSettings::new(services));
    use_case::export_project(&ExportProjectDeps { files: &CopierFiles, bacpacs: &bacpacs }, &request, op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_asks_for_its_halves_in_a_dated_folder() {
        let now = Stamp::from_folder_name("x_20260928_154210").unwrap();
        let (both, folder) = backup_request("shop", Path::new(r"C:\DNN\shop"), BackupKind::Both, now);
        assert!(folder.ends_with(r"backups\shop\shop_20260928_154210"));
        assert_eq!(both.zip_path, Some(folder.join("shop.zip")));
        assert_eq!(both.bacpac_path, Some(folder.join("shop.bacpac")));
        let (site, _) = backup_request("shop", Path::new(r"C:\DNN\shop"), BackupKind::Site, now);
        assert!(site.zip_path.is_some() && site.bacpac_path.is_none());
        let (database, _) = backup_request("shop", Path::new(r"C:\DNN\shop"), BackupKind::Database, now);
        assert!(database.zip_path.is_none() && database.bacpac_path.is_some());
    }

    #[test]
    fn a_real_zip_leaves_out_git_and_the_filter() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        for file in
            ["web.config", r"bin\DotNetNuke.dll", r".git\HEAD", r"App_Data\Search\index.dat", r"App_Data\keep.txt"]
        {
            let path = site.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        std::fs::write(site.join("_backup.filter"), r"\site\wwwroot\App_Data\Search").unwrap();
        let zip = dir.path().join("backups").join("shop_20260928_154210").join("shop.zip");
        let request = ExportProjectRequest {
            project_name: "shop".into(),
            project_directory: site,
            zip_path: Some(zip.clone()),
            bacpac_path: None,
        };
        let bacpacs = SqlPackage(SqlSettings { docker: Err("not used".into()), settings: Default::default() });
        let mut lines = crate::backups::test_support::Lines::default();
        let prompt = crate::backups::test_support::Yes;
        let mut op = Operation::new(&mut lines, &prompt, Default::default());
        use_case::export_project(&ExportProjectDeps { files: &CopierFiles, bacpacs: &bacpacs }, &request, &mut op)
            .unwrap();
        drop(op);
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        let mut names: Vec<String> =
            (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().replace('\\', "/")).collect();
        names.sort();
        assert_eq!(names, ["App_Data/keep.txt", "_backup.filter", "bin/DotNetNuke.dll", "web.config"]);
        assert!(
            lines.0.contains(&r"Leaving out what _backup.filter lists: App_Data\Search".to_string()),
            "{:?}",
            lines.0
        );
    }
}
