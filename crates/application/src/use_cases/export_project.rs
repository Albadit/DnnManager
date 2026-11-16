//! `ExportProjectUseCase`: a project as the pair "New project → An existing site" imports - a `.zip` of the site's files
//! (without `.git` and what its `_backup.filter` lists) and a `.bacpac` of its database - or just one of the two. The
//! Export menu's backups write them into a dated folder of the project's backups (`backups::new_folder`).

use std::path::{Path, PathBuf};

use super::backups::{Bacpacs, SiteFiles, is_inside};
use crate::operation::Operation;

/// Never part of the site: source control. The site's `_backup.filter` adds its own paths.
const ALWAYS_EXCLUDED: &[&str] = &[".git"];

/// The site's `_backup.filter`, as messages name it.
pub const BACKUP_FILTER: &str = "_backup.filter";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportProjectRequest {
    pub project_name: String,
    /// The folder the site serves (its IIS physical path).
    pub project_directory: PathBuf,
    /// The `.zip` to write the site's files to, or None to leave them out.
    pub zip_path: Option<PathBuf>,
    /// The `.bacpac` to export the site's database to, or None to leave it out.
    pub bacpac_path: Option<PathBuf>,
}

pub struct ExportProjectDeps<'a> {
    pub files: &'a dyn SiteFiles,
    pub bacpacs: &'a dyn Bacpacs,
}

pub fn export_project(deps: &ExportProjectDeps, req: &ExportProjectRequest, op: &mut Operation) -> Result<(), String> {
    let directory = &req.project_directory;
    if !directory.is_dir() {
        return Err(format!("Project folder not found: {}", directory.display()));
    }
    if req.zip_path.is_none() && req.bacpac_path.is_none() {
        return Err("Nothing to export - choose the site files, the database, or both.".into());
    }
    let full = |p: &PathBuf| std::path::absolute(p).unwrap_or_else(|_| p.clone());
    let zip_path = req.zip_path.as_ref().map(full);
    let bacpac_path = req.bacpac_path.as_ref().map(full);
    // Inside the site, the export would be served by IIS and zipped into the next export.
    if [&zip_path, &bacpac_path].into_iter().flatten().any(|p| is_inside(p, directory)) {
        return Err("Choose a location outside the project folder - the export would end up inside the site.".into());
    }
    // A cancel deletes what was written so far, and the backup folder made for it.
    for path in [&zip_path, &bacpac_path].into_iter().flatten() {
        let folder = path.parent().unwrap_or(Path::new(""));
        op.undo.delete_folder_on_undo(folder);
        op.undo.restore_file_on_undo(path);
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }

    if let Some(zip_path) = &zip_path {
        op.report.step("Zipping the website files");
        let filtered = deps
            .files
            .backup_filter(directory)
            .map_err(|e| format!("Could not read {}: {e}", directory.join(BACKUP_FILTER).display()))?;
        if !filtered.is_empty() {
            op.report.info(&format!("Leaving out what {BACKUP_FILTER} lists: {}", filtered.join(", ")));
        }
        let excluded: Vec<String> = ALWAYS_EXCLUDED.iter().map(|s| s.to_string()).chain(filtered).collect();
        if let Err(e) = deps.files.create_zip(directory, zip_path, &excluded, op.report, &op.cancel) {
            remove_if_empty(Some(zip_path));
            return Err(e);
        }
    }

    if let Some(bacpac_path) = &bacpac_path {
        op.report.step("Exporting the database");
        // The database web.config names, as it signs in - Windows authentication too.
        let source = deps.bacpacs.export_source(&req.project_name, directory)?;
        let export = deps
            .bacpacs
            .ensure_available(op.report, &op.cancel)
            .and_then(|()| deps.bacpacs.export(&source, bacpac_path, op.report, &op.cancel));
        if let Err(e) = export {
            remove_if_empty(Some(bacpac_path));
            if op.cancel.is_cancelled() {
                return Err(e);
            }
            let files = zip_path.as_ref().map(|z| format!("The site files are in {}, but ", z.display()));
            return Err(format!("{}exporting database [{}] failed: {e}", files.unwrap_or_default(), source.database));
        }
        op.report.success(&format!("Database [{}] exported to {}", source.database, bacpac_path.display()));
    }

    op.report.step("Export complete");
    if let Some(zip) = &zip_path {
        op.report.success(&format!("Site files: {}", zip.display()));
    }
    if let Some(bacpac) = &bacpac_path {
        op.report.success(&format!("Database:   {}", bacpac.display()));
    }
    if zip_path.is_some() && bacpac_path.is_some() {
        op.report.info("Import both with New project → An existing site.");
    }
    if zip_path.is_some() {
        op.report.warn(
            "The zip's web.config still holds this machine's connection string, password included - Import points it at \
             the new project's own database, but keep it in mind before sharing the zip.",
        );
    }
    Ok(())
}

/// A backup folder left empty by a failed export is removed again.
fn remove_if_empty(file: Option<&PathBuf>) {
    let Some(folder) = file.and_then(|f| f.parent()) else { return };
    if std::fs::read_dir(folder).is_ok_and(|mut entries| entries.next().is_none()) {
        let _ = std::fs::remove_dir(folder);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Reporter};
    use crate::use_cases::backups::SqlLogin;
    use std::sync::Mutex;

    /// Stand-in files: a zip is a file listing what it left out; `fail_zip` fails the zip.
    #[derive(Default)]
    pub struct FakeFiles {
        pub filter: Vec<String>,
        pub fail_filter: bool,
        pub fail_zip: bool,
        pub excluded: Mutex<Vec<String>>,
    }

    impl SiteFiles for FakeFiles {
        fn backup_filter(&self, _: &Path) -> Result<Vec<String>, String> {
            if self.fail_filter { Err("access denied".into()) } else { Ok(self.filter.clone()) }
        }
        fn create_zip(
            &self,
            _: &Path,
            zip: &Path,
            excluded: &[String],
            r: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<(), String> {
            *self.excluded.lock().unwrap() = excluded.to_vec();
            if self.fail_zip {
                return Err("disk full".into());
            }
            r.success("zipped");
            std::fs::write(zip, excluded.join(",")).map_err(|e| e.to_string())
        }
    }

    /// Stand-in SqlPackage: records what it does; `fail_export` fails each export.
    #[derive(Default)]
    pub struct FakeBacpacs {
        pub fail_export: bool,
        pub fail_import: bool,
        pub local_db: bool,
        pub calls: Mutex<Vec<String>>,
    }

    impl Bacpacs for FakeBacpacs {
        fn export_source(&self, project: &str, _: &Path) -> Result<SqlLogin, String> {
            if self.local_db {
                return Err("The site's database is a LocalDB file".into());
            }
            Ok(SqlLogin::new("localhost,1433", project, "sa", "secret"))
        }
        fn ensure_available(&self, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            Ok(())
        }
        fn export(&self, s: &SqlLogin, bacpac: &Path, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("export {}", s.database));
            if self.fail_export {
                return Err("BACPAC export failed: SQL71501".into());
            }
            std::fs::write(bacpac, "bacpac").map_err(|e| e.to_string())
        }
        fn import(&self, t: &SqlLogin, _: &Path, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("import {}", t.database));
            if self.fail_import { Err("BACPAC import failed: x".into()) } else { Ok(()) }
        }
    }

    fn run(files: &FakeFiles, bacpacs: &FakeBacpacs, req: &ExportProjectRequest) -> (Result<(), String>, Vec<String>) {
        let mut lines = Lines::default();
        let answer = Answer::yes();
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        let result = export_project(&ExportProjectDeps { files, bacpacs }, req, &mut op);
        drop(op);
        (result, lines.0)
    }

    fn request(dir: &Path, zip: bool, bacpac: bool) -> ExportProjectRequest {
        let folder = dir.join("backups").join("shop_20260928_154210");
        ExportProjectRequest {
            project_name: "shop".into(),
            project_directory: dir.join("site"),
            zip_path: zip.then(|| folder.join("shop.zip")),
            bacpac_path: bacpac.then(|| folder.join("shop.bacpac")),
        }
    }

    fn temp_site() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("site")).unwrap();
        dir
    }

    #[test]
    fn both_halves_go_into_the_backup_folder() {
        let dir = temp_site();
        let files = FakeFiles { filter: vec![r"App_Data\Search".into()], ..Default::default() };
        let (result, lines) = run(&files, &FakeBacpacs::default(), &request(dir.path(), true, true));
        result.unwrap();
        assert_eq!(*files.excluded.lock().unwrap(), [".git", r"App_Data\Search"]);
        let folder = dir.path().join("backups").join("shop_20260928_154210");
        assert!(folder.join("shop.zip").is_file() && folder.join("shop.bacpac").is_file());
        assert_eq!(lines[0], "## Zipping the website files");
        assert_eq!(lines[1], r"Leaving out what _backup.filter lists: App_Data\Search");
        assert!(lines.contains(&"## Exporting the database".to_string()));
        assert!(lines.contains(&format!("ok Database [shop] exported to {}", folder.join("shop.bacpac").display())));
        assert!(lines.contains(&"Import both with New project → An existing site.".to_string()));
        assert!(lines.last().unwrap().starts_with("warn The zip's web.config still holds"));
    }

    #[test]
    fn refuses_nothing_a_missing_folder_and_a_target_inside_the_site() {
        let dir = temp_site();
        let (files, bacpacs) = (FakeFiles::default(), FakeBacpacs::default());
        let none = request(dir.path(), false, false);
        assert_eq!(
            run(&files, &bacpacs, &none).0.unwrap_err(),
            "Nothing to export - choose the site files, the database, or both."
        );
        let mut inside = request(dir.path(), true, false);
        inside.zip_path = Some(dir.path().join("site").join("b").join("shop.zip"));
        assert!(
            run(&files, &bacpacs, &inside).0.unwrap_err().starts_with("Choose a location outside the project folder")
        );
        let mut missing = request(dir.path(), true, false);
        missing.project_directory = dir.path().join("gone");
        assert!(run(&files, &bacpacs, &missing).0.unwrap_err().starts_with("Project folder not found: "));
        assert!(!dir.path().join("backups").exists());
    }

    #[test]
    fn a_failed_database_export_names_the_zip_and_leaves_no_empty_folder() {
        let dir = temp_site();
        let bacpacs = FakeBacpacs { fail_export: true, ..Default::default() };
        let (result, _) = run(&FakeFiles::default(), &bacpacs, &request(dir.path(), true, true));
        let zip = dir.path().join("backups").join("shop_20260928_154210").join("shop.zip");
        assert_eq!(
            result.unwrap_err(),
            format!(
                "The site files are in {}, but exporting database [shop] failed: BACPAC export failed: SQL71501",
                zip.display()
            )
        );
        let (result, _) = run(&FakeFiles::default(), &bacpacs, &request(dir.path(), false, true));
        assert_eq!(result.unwrap_err(), "exporting database [shop] failed: BACPAC export failed: SQL71501");
        let folder = dir.path().join("backups").join("shop_20260928_154210");
        assert!(folder.join("shop.zip").exists(), "the operation's undo deletes it - not the use case");
    }

    #[test]
    fn a_failed_zip_or_unreadable_filter_stops_it() {
        let dir = temp_site();
        let files = FakeFiles { fail_zip: true, ..Default::default() };
        assert_eq!(run(&files, &FakeBacpacs::default(), &request(dir.path(), true, true)).0.unwrap_err(), "disk full");
        assert!(!dir.path().join("backups").join("shop_20260928_154210").exists(), "an empty folder is removed");
        let files = FakeFiles { fail_filter: true, ..Default::default() };
        let error = run(&files, &FakeBacpacs::default(), &request(dir.path(), true, false)).0.unwrap_err();
        assert!(error.starts_with("Could not read ") && error.ends_with("_backup.filter: access denied"), "{error}");
    }

    #[test]
    fn a_cancel_undoes_the_new_backup_folder() {
        let dir = temp_site();
        let mut lines = Lines::default();
        let cancel = CancelToken::new();
        let req = request(dir.path(), true, false);
        let files = FakeFiles::default();
        let bacpacs = FakeBacpacs::default();
        let c = cancel.clone();
        let outcome = crate::operation::run(&mut lines, &Answer::yes(), cancel, &CancelToken::new(), |op| {
            export_project(&ExportProjectDeps { files: &files, bacpacs: &bacpacs }, &req, op)?;
            c.cancel();
            op.cancel.check()
        });
        assert_eq!(outcome, crate::operation::Outcome::Cancelled);
        assert!(!dir.path().join("backups").join("shop_20260928_154210").exists());
    }
}
