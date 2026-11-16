//! `RestoreBackupUseCase` (and the question `ProjectEdits.RestoreBackup` asks first): puts a project back as a backup
//! has it - its files (those added since are deleted, what the backup leaves out - `.git`, `_backup.filter` - isn't
//! touched) and its database, on the server and with the login its web.config names. The backup's database is imported
//! first, under a name of its own, before anything of the site is touched - a failed import leaves the project as it
//! was. Then the files are put back and the imported database swapped in for the site's, which is dropped only then.
//! The site is stopped meanwhile and started again if it ran. Once it has started it runs to the end: a half-restored
//! site is worse than either.
//!
//! Deviation: the C# says "still in use - trying again" on the status line (`IProgressReporter.Progress`), which the
//! port's `Reporter` hasn't got - it is an info line of the Output here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::backups::{Bacpacs, Databases, ProjectBackup, SqlLogin, Stamp};
use crate::iis::{IisManager, states};
use crate::operation::{CANCELLED, CancelToken, Operation};
use crate::project::view::group_thousands;
use crate::records::ProjectRecords;

/// Why DNN Manager would sign in to the site's database server as the user - for `SqlServerAddress.SignInQuestion`.
pub const SIGN_IN_WHY: &str = "Restoring the backup's database";

/// How long the site is given to stop, its worker process to end.
const STOP_WITHIN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreBackupRequest {
    /// The IIS site's name.
    pub site_name: String,
    /// The folder the site serves.
    pub directory: PathBuf,
    /// The backup's site files (`<project>.zip`).
    pub site_zip: PathBuf,
    /// The backup's database (`<project>.bacpac`); None to leave the database as it is.
    pub database: Option<PathBuf>,
    /// The user already agreed to DNN Manager signing in to the site's database server with their Windows account (an
    /// upgrade asks before it starts, then puts a step's backup back): not asked again.
    pub sign_in_agreed: bool,
}

impl RestoreBackupRequest {
    /// Putting `backup` back into the site `site_name` serving `directory` - None when the backup misses a half.
    pub fn from_backup(site_name: &str, directory: &Path, backup: &ProjectBackup) -> Option<Self> {
        Some(Self {
            site_name: site_name.into(),
            directory: directory.into(),
            site_zip: backup.site_zip.clone()?,
            database: Some(backup.database.clone()?),
            sign_in_agreed: false,
        })
    }
}

/// What Restore backup asks before it starts (`Dialogs.ConfirmDanger`), with "Restore backup" and "Cancel".
pub fn confirm_question(site_name: &str, backup: &ProjectBackup) -> String {
    format!(
        "Put '{site_name}' back as it was on {}?\n\nIts files and its database are replaced by the backup's - what changed \
         since is lost. Files added since are deleted; what the backup leaves out (.git, _backup.filter) isn't touched. \
         The database it replaces is dropped once the backup's is in. The site is stopped meanwhile.\n\nBackup: {}",
        backup.created.display(),
        backup.folder.display()
    )
}

/// Asks first; Err(Cancelled.) on a no.
pub fn confirm(site_name: &str, backup: &ProjectBackup, op: &mut Operation) -> Result<(), String> {
    if op.prompt.confirm(&confirm_question(site_name, backup), "Restore backup", "Cancel", true) {
        Ok(())
    } else {
        Err(CANCELLED.into())
    }
}

/// Why an extraction stopped: a file still in use may pass once what holds it lets go - anything else won't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    InUse(String),
    Failed(String),
}

/// The database the site's web.config names, as Restore puts the backup's in (`LocalSqlContainer.DatabaseOf`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTarget {
    /// web.config names no database of its own.
    None,
    /// A LocalDB file in App_Data - left as it is.
    LocalDbFile,
    /// A database on a SQL Server - the local container as its user, any other server as the site signs in.
    /// `sign_in_question`: Windows authentication to a server not on this PC and not the one in Settings → Database
    /// server would hand it the user's sign-in - asked first.
    Server { login: SqlLogin, sign_in_question: Option<String> },
}

/// The site's folder and its database, as Restore needs them (`IProjectFileCopier`, `IFileLockService`,
/// `LocalSqlContainer`, `DnnInstall`).
pub trait RestoreSite: Send + Sync {
    /// Why the zip can't be put back into `directory` - a file outside the folder, through a link or a link itself, or
    /// a zip that can't be read; None when every file of it can.
    fn check_zip(&self, zip: &Path, directory: &Path) -> Option<String>;
    /// What the site's `_backup.filter` leaves out; empty when it has none or it can't be read.
    fn backup_filter(&self, directory: &Path) -> Vec<String>;
    fn extract_zip(
        &self,
        zip: &Path,
        directory: &Path,
        report: &mut dyn crate::operation::Reporter,
    ) -> Result<(), ExtractError>;
    /// Deletes the files added since the backup that aren't in it, but `excluded`; how many.
    fn remove_files_not_in_zip(
        &self,
        zip: &Path,
        directory: &Path,
        excluded: &[String],
        report: &mut dyn crate::operation::Reporter,
    ) -> Result<usize, String>;
    /// Ends what still runs from the site's folder; their file names.
    fn stop_programs_running_from(&self, directory: &Path) -> Vec<String>;
    fn database_target(&self, site_name: &str, directory: &Path) -> RestoreTarget;
    /// The DNN version of the site's files now.
    fn dnn_version(&self, directory: &Path) -> Option<String>;
}

pub struct RestoreBackupDeps<'a> {
    pub iis: &'a dyn IisManager,
    pub site: &'a dyn RestoreSite,
    pub bacpacs: &'a dyn Bacpacs,
    pub databases: &'a dyn Databases,
    pub records: &'a dyn ProjectRecords,
    pub now: Stamp,
    /// Between two tries to put back a file still in use (3 s).
    pub retry_delay: Duration,
}

pub fn restore_backup(deps: &RestoreBackupDeps, req: &RestoreBackupRequest, op: &mut Operation) -> Result<(), String> {
    op.cancel.check()?;
    if !req.site_zip.is_file() {
        return Err(format!("The backup's site files aren't there: {}", req.site_zip.display()));
    }
    if let Some(bacpac) = &req.database
        && !bacpac.is_file()
    {
        return Err(format!("The backup's database isn't there: {}", bacpac.display()));
    }
    if !req.directory.is_dir() {
        return Err(format!("Project folder not found: {}", req.directory.display()));
    }
    // Every file of the backup checked before one is written: one that can't be put back (outside the folder, through
    // a link) would otherwise stop the restore half-way, with the site's files a mix of both.
    if let Some(refused) = deps.site.check_zip(&req.site_zip, &req.directory) {
        return Err(format!("{refused} Nothing was changed."));
    }
    let filtered = deps.site.backup_filter(&req.directory);

    // The backup's database first, beside the site's - the database the site's web.config names, as the site uses it.
    let mut swap: Option<(SqlLogin, String)> = None;
    if let Some(bacpac) = &req.database {
        match deps.site.database_target(&req.site_name, &req.directory) {
            RestoreTarget::None => {
                op.report.warn("The site's web.config names no database of its own - the database is left as it is.")
            }
            RestoreTarget::LocalDbFile => {
                op.report.warn("The site uses a LocalDB file - the database is left as it is.")
            }
            RestoreTarget::Server { login, sign_in_question } => {
                if !req.sign_in_agreed
                    && let Some(question) = sign_in_question
                    && !op.prompt.confirm(&question, &format!("Sign in to {}", login.server), "Don't sign in", false)
                {
                    return Err(format!(
                        "Not signed in to {} with your Windows account - the backup's database goes there. Nothing was changed.",
                        login.server
                    ));
                }
                let incoming = import_aside(deps, &login, bacpac, op)?;
                swap = Some((login, incoming));
            }
        }
    }
    // A cancel can't take a restore back once it changes the site - said, so nobody counts on it.
    op.undo.cannot_undo(format!("the files of '{}' were put back as the backup has them.", req.site_name));

    let site = &req.site_name;
    let was_running =
        deps.iis.site_states().iter().any(|(name, state)| name.eq_ignore_ascii_case(site) && states::is_started(state));
    // Stopped even when it reads so: a worker process still ending holds the site's files (and DNN its lock).
    if was_running {
        op.report.info(&format!("Stopping '{site}' while it is put back…"));
    }
    deps.iis.stop_site_and_wait(site, STOP_WITHIN).map_err(|e| format!("Could not stop the site: {e}"))?;
    let result = put_back(deps, req, &filtered, swap.as_ref(), op);
    if was_running && let Err(e) = deps.iis.start_site(site) {
        op.report.warn(&format!("Could not start '{site}' again: {e}"));
    }
    result
}

/// The files, then the database - not cancelled from here on: the files and the database belong together.
fn put_back(
    deps: &RestoreBackupDeps,
    req: &RestoreBackupRequest,
    filtered: &[String],
    swap: Option<&(SqlLogin, String)>,
    op: &mut Operation,
) -> Result<(), String> {
    op.report.info("Putting the site's files back…");
    stop_site_programs(deps, &req.directory, op);
    if let Err(e) = extract_with_retry(deps, &req.site_zip, &req.directory, op) {
        if let Some((target, incoming)) = swap {
            let _ = deps.databases.drop_database(&target.with_database(incoming));
        }
        return Err(format!(
            "Could not put the site's files back: {e} The site's files are now a mix - some as the backup has them, the \
             rest as they were. Run Restore again once nothing holds them (an editor, a terminal in the folder). The \
             database is left as it was."
        ));
    }
    let excluded: Vec<String> = std::iter::once(".git".to_string()).chain(filtered.iter().cloned()).collect();
    if let Ok(removed) = deps.site.remove_files_not_in_zip(&req.site_zip, &req.directory, &excluded, op.report)
        && removed > 0
    {
        op.report.info(&format!("Deleted {} file(s) added since the backup.", group_thousands(removed as f64, 0)));
    }
    op.report.success("The site's files are as the backup has them.");

    if let Some((target, incoming)) = swap {
        swap_in(deps, target, incoming, op)?;
    }

    if let Some(record) = deps.records.find(&req.site_name)
        && let Some(version) = deps.site.dnn_version(&req.directory)
    {
        deps.records.save(&crate::records::ProjectRecord { dnn_version: Some(version), ..record });
    }
    Ok(())
}

/// What still holds a file of the site once it is stopped: the C# compiler ASP.NET runs from its bin\roslyn keeps
/// running for minutes, and the worker process takes a moment to let go. Tried again for half a minute, the site's
/// own programs ended before each try - but only for a file in use: anything else fails the same way each time, and
/// is said at once.
fn extract_with_retry(
    deps: &RestoreBackupDeps,
    zip: &Path,
    directory: &Path,
    op: &mut Operation,
) -> Result<(), String> {
    let mut attempt = 1;
    loop {
        match deps.site.extract_zip(zip, directory, op.report) {
            Ok(()) => return Ok(()),
            Err(ExtractError::InUse(_)) if attempt < 10 => {
                op.report.info(&format!("A file of the site is still in use - trying again ({attempt} of 9)…"))
            }
            Err(ExtractError::InUse(e) | ExtractError::Failed(e)) => return Err(e),
        }
        attempt += 1;
        std::thread::sleep(deps.retry_delay);
        stop_site_programs(deps, directory, op);
    }
}

fn stop_site_programs(deps: &RestoreBackupDeps, directory: &Path, op: &mut Operation) {
    let mut stopped = deps.site.stop_programs_running_from(directory);
    let mut seen = std::collections::HashSet::new();
    stopped.retain(|s| seen.insert(s.clone()));
    if !stopped.is_empty() {
        op.report.info(&format!("Ended what the site still ran from its folder: {}.", stopped.join(", ")));
    }
}

/// The backup's database imported beside the site's, under a name of its own (returned) - the local container as its
/// user, who may create databases, or any other server as the site signs in. The import never meets the site's
/// database - nor its files: a database renamed keeps its file names, so one imported under the same name would
/// collide with them. A failed import leaves nothing behind.
fn import_aside(
    deps: &RestoreBackupDeps,
    admin: &SqlLogin,
    bacpac: &Path,
    op: &mut Operation,
) -> Result<String, String> {
    let none = CancelToken::new();
    let name = &admin.database;
    op.report.info(&format!("Importing the backup's database beside [{name}] on {}.", admin.server));
    deps.bacpacs.ensure_available(op.report, &none)?;
    let incoming = admin.with_database(&format!("{name}_restore_{}", deps.now.compact()));
    if let Err(e) = deps.bacpacs.import(&incoming, bacpac, op.report, &none) {
        // What the import left half-made goes; the site's own database was never touched.
        if deps.databases.exists(&incoming, &none) == Ok(true) {
            let _ = deps.databases.drop_database(&incoming);
        }
        return Err(format!("Importing the backup's database failed: {e} Nothing was changed."));
    }
    Ok(incoming.database)
}

/// The imported database in place of the site's: the site's renamed aside, the imported one given its name, and the
/// one aside dropped. A rename that fails puts the site's back.
fn swap_in(deps: &RestoreBackupDeps, admin: &SqlLogin, incoming_name: &str, op: &mut Operation) -> Result<(), String> {
    let none = CancelToken::new();
    let name = &admin.database;
    let server = &admin.server;
    let incoming = admin.with_database(incoming_name);
    op.report.info(&format!("Putting database [{name}] back on {server}…"));
    let exists = match deps.databases.exists(admin, &none) {
        Ok(exists) => exists,
        Err(e) => {
            let _ = deps.databases.drop_database(&incoming);
            return Err(format!("Could not reach [{name}] on {server}: {e} The database is left as it was."));
        }
    };
    let aside = format!("{name}_before_restore_{}", deps.now.compact());
    if exists && let Err(e) = deps.databases.rename(admin, &aside) {
        let _ = deps.databases.drop_database(&incoming);
        return Err(format!(
            "Could not set [{name}] aside to put the backup's in: {e} The database is left as it was."
        ));
    }
    if let Err(e) = deps.databases.rename(&incoming, name) {
        if exists && let Err(back) = deps.databases.rename(&admin.with_database(&aside), name) {
            return Err(format!(
                "The backup's database is in as [{incoming_name}] but couldn't be named [{name}]: {e} - and the site's \
                 couldn't be put back from [{aside}]: {back}"
            ));
        }
        let _ = deps.databases.drop_database(&incoming);
        return Err(format!("The backup's database couldn't be named [{name}]: {e} The database is left as it was."));
    }
    if exists && let Err(e) = deps.databases.drop_database(&admin.with_database(&aside)) {
        op.report.warn(&format!(
            "The database it replaced is still there as [{aside}] - drop it once you no longer need it ({e})."
        ));
    }
    op.report.success(&format!("Database [{name}] is as the backup has it."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{Outcome, Reporter};
    use crate::records::InstallMode;
    use crate::use_cases::backups::tests::stamp;
    use crate::use_cases::export_for_deployment::tests::FakeDatabases;
    use crate::use_cases::export_project::tests::FakeBacpacs;
    use crate::use_cases::tests::FakeIis;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeSite {
        refuse: Option<String>,
        in_use_times: usize,
        tries: AtomicUsize,
        fail_extract: bool,
        target: RestoreTarget,
        excluded: Mutex<Vec<String>>,
    }

    impl Default for FakeSite {
        fn default() -> Self {
            Self {
                refuse: None,
                in_use_times: 0,
                tries: AtomicUsize::new(0),
                fail_extract: false,
                target: RestoreTarget::Server {
                    login: SqlLogin::new("localhost,1433", "shop", "sa", "pw"),
                    sign_in_question: None,
                },
                excluded: Mutex::new(vec![]),
            }
        }
    }

    impl RestoreSite for FakeSite {
        fn check_zip(&self, _: &Path, _: &Path) -> Option<String> {
            self.refuse.clone()
        }
        fn backup_filter(&self, _: &Path) -> Vec<String> {
            vec![r"App_Data\Search".into()]
        }
        fn extract_zip(&self, _: &Path, _: &Path, r: &mut dyn Reporter) -> Result<(), ExtractError> {
            let n = self.tries.fetch_add(1, Ordering::SeqCst);
            if n < self.in_use_times {
                return Err(ExtractError::InUse("in use".into()));
            }
            if self.fail_extract {
                return Err(ExtractError::Failed("Access is denied.".into()));
            }
            r.success("Extracted 3 files (0,0 MB).");
            Ok(())
        }
        fn remove_files_not_in_zip(
            &self,
            _: &Path,
            _: &Path,
            ex: &[String],
            _: &mut dyn Reporter,
        ) -> Result<usize, String> {
            *self.excluded.lock().unwrap() = ex.to_vec();
            Ok(1234)
        }
        fn stop_programs_running_from(&self, _: &Path) -> Vec<String> {
            vec!["csc.exe".into(), "csc.exe".into(), "VBCSCompiler.exe".into()]
        }
        fn database_target(&self, _: &str, _: &Path) -> RestoreTarget {
            self.target.clone()
        }
        fn dnn_version(&self, _: &Path) -> Option<String> {
            Some("10.2.5".into())
        }
    }

    #[derive(Default)]
    struct FakeRecords(Mutex<Vec<String>>);

    impl ProjectRecords for FakeRecords {
        fn find(&self, site: &str) -> Option<crate::records::ProjectRecord> {
            Some(crate::records::ProjectRecord {
                site: site.into(),
                install_mode: InstallMode::Automatic,
                created_utc: "2026-01-01T00:00:00.0000000Z".into(),
                dnn_version: Some("9.13.9".into()),
                host_user_name: None,
            })
        }
        fn save(&self, r: &crate::records::ProjectRecord) {
            self.0.lock().unwrap().push(format!("{} {}", r.site, r.dnn_version.clone().unwrap_or_default()));
        }
        fn remove(&self, _: &str) {}
    }

    struct Setup {
        dir: tempfile::TempDir,
        iis: FakeIis,
        site: FakeSite,
        bacpacs: FakeBacpacs,
        databases: FakeDatabases,
        records: FakeRecords,
    }

    impl Setup {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("site")).unwrap();
            std::fs::write(dir.path().join("shop.zip"), "zip").unwrap();
            std::fs::write(dir.path().join("shop.bacpac"), "bacpac").unwrap();
            Self {
                iis: FakeIis::running().with_site("shop", &dir.path().join("site").display().to_string()),
                dir,
                site: FakeSite::default(),
                bacpacs: FakeBacpacs::default(),
                databases: FakeDatabases::default(),
                records: FakeRecords::default(),
            }
        }

        fn request(&self) -> RestoreBackupRequest {
            RestoreBackupRequest {
                site_name: "shop".into(),
                directory: self.dir.path().join("site"),
                site_zip: self.dir.path().join("shop.zip"),
                database: Some(self.dir.path().join("shop.bacpac")),
                sign_in_agreed: false,
            }
        }

        fn run_with(&self, req: &RestoreBackupRequest, answer: &Answer) -> (Outcome, Vec<String>) {
            let deps = RestoreBackupDeps {
                iis: &self.iis,
                site: &self.site,
                bacpacs: &self.bacpacs,
                databases: &self.databases,
                records: &self.records,
                now: stamp("20260928_154210"),
                retry_delay: Duration::ZERO,
            };
            let mut lines = Lines::default();
            let outcome = crate::operation::run(&mut lines, answer, CancelToken::new(), &CancelToken::new(), |op| {
                restore_backup(&deps, req, op)
            });
            (outcome, lines.0)
        }

        fn run(&self) -> (Outcome, Vec<String>) {
            self.run_with(&self.request(), &Answer::yes())
        }

        fn db_calls(&self) -> Vec<String> {
            self.databases.calls.lock().unwrap().clone()
        }
    }

    #[test]
    fn imports_aside_puts_the_files_back_and_swaps_the_database_in() {
        let s = Setup::new();
        let (outcome, lines) = s.run();
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(*s.bacpacs.calls.lock().unwrap(), ["import shop_restore_20260928154210"]);
        assert_eq!(
            s.db_calls(),
            [
                "exists shop",
                "rename shop shop_before_restore_20260928154210",
                "rename shop_restore_20260928154210 shop",
                "drop shop_before_restore_20260928154210"
            ]
        );
        // Stopped, put back, started again because it ran.
        assert_eq!(*s.iis.calls.lock().unwrap(), ["stop shop", "start shop"]);
        assert_eq!(*s.site.excluded.lock().unwrap(), [".git", r"App_Data\Search"]);
        assert_eq!(*s.records.0.lock().unwrap(), ["shop 10.2.5"]);
        assert_eq!(
            lines,
            [
                "Importing the backup's database beside [shop] on localhost,1433.",
                "Stopping 'shop' while it is put back…",
                "Putting the site's files back…",
                "Ended what the site still ran from its folder: csc.exe, VBCSCompiler.exe.",
                "ok Extracted 3 files (0,0 MB).",
                "Deleted 1,234 file(s) added since the backup.",
                "ok The site's files are as the backup has them.",
                "Putting database [shop] back on localhost,1433…",
                "ok Database [shop] is as the backup has it."
            ]
        );
    }

    #[test]
    fn a_failed_import_changes_nothing() {
        let mut s = Setup::new();
        s.bacpacs.fail_import = true;
        let (outcome, _) = s.run();
        assert_eq!(
            outcome,
            Outcome::Failed(
                "Importing the backup's database failed: BACPAC import failed: x Nothing was changed.".into()
            )
        );
        assert_eq!(s.db_calls(), ["exists shop_restore_20260928154210", "drop shop_restore_20260928154210"]);
        assert!(s.iis.calls.lock().unwrap().is_empty(), "the site wasn't stopped");
    }

    #[test]
    fn a_zip_that_cant_be_put_back_and_missing_files_change_nothing() {
        let mut s = Setup::new();
        s.site.refuse = Some("The backup's site files hold a link (x) - DNN Manager doesn't put links back.".into());
        assert_eq!(
            s.run().0,
            Outcome::Failed(
                "The backup's site files hold a link (x) - DNN Manager doesn't put links back. Nothing was changed."
                    .into()
            )
        );
        let s = Setup::new();
        let req = RestoreBackupRequest { database: Some(s.dir.path().join("gone.bacpac")), ..s.request() };
        let (outcome, _) = s.run_with(&req, &Answer::yes());
        assert!(matches!(outcome, Outcome::Failed(e) if e.starts_with("The backup's database isn't there: ")));
        assert!(s.db_calls().is_empty() && s.iis.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn files_in_use_are_tried_again_and_a_failure_drops_the_import() {
        let mut s = Setup::new();
        s.site.in_use_times = 2;
        let (outcome, lines) = s.run();
        assert_eq!(outcome, Outcome::Finished);
        assert!(lines.contains(&"A file of the site is still in use - trying again (1 of 9)…".into()));
        assert!(lines.contains(&"A file of the site is still in use - trying again (2 of 9)…".into()));
        assert_eq!(s.site.tries.load(Ordering::SeqCst), 3);

        let mut s = Setup::new();
        s.site.fail_extract = true;
        let (outcome, lines) = s.run();
        let Outcome::Failed(error) = outcome else { panic!("{outcome:?}") };
        assert!(
            error.starts_with("Could not put the site's files back: Access is denied. The site's files are now a mix")
        );
        assert_eq!(s.db_calls(), ["drop shop_restore_20260928154210"]);
        assert_eq!(*s.iis.calls.lock().unwrap(), ["stop shop", "start shop"], "started again all the same");
        assert!(
            lines.contains(&"warn Can't be put back: the files of 'shop' were put back as the backup has them.".into())
        );

        let mut s = Setup::new();
        s.site.in_use_times = 20;
        let Outcome::Failed(error) = s.run().0 else { panic!() };
        assert!(error.starts_with("Could not put the site's files back: in use "));
        assert_eq!(s.site.tries.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn a_server_elsewhere_is_signed_in_to_only_when_agreed() {
        let mut s = Setup::new();
        s.site.target = RestoreTarget::Server {
            login: SqlLogin::new("sql.corp", "shop", "", ""),
            sign_in_question: Some("Sign in to it as you?".into()),
        };
        let no = Answer::no();
        let (outcome, _) = s.run_with(&s.request(), &no);
        assert_eq!(
            outcome,
            Outcome::Failed(
                "Not signed in to sql.corp with your Windows account - the backup's database goes there. Nothing was changed."
                    .into()
            )
        );
        assert_eq!(*no.1.lock().unwrap(), ["Sign in to it as you?"]);
        let no = Answer::no();
        let req = RestoreBackupRequest { sign_in_agreed: true, ..s.request() };
        assert_eq!(s.run_with(&req, &no).0, Outcome::Finished);
        assert!(no.1.lock().unwrap().is_empty());
    }

    #[test]
    fn no_database_of_its_own_restores_the_files_only() {
        let mut s = Setup::new();
        s.site.target = RestoreTarget::LocalDbFile;
        let (outcome, lines) = s.run();
        assert_eq!(outcome, Outcome::Finished);
        assert_eq!(lines[0], "warn The site uses a LocalDB file - the database is left as it is.");
        assert!(s.db_calls().is_empty() && s.bacpacs.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_question_and_the_request_come_from_the_backup() {
        let backup = ProjectBackup {
            project: "shop".into(),
            folder: r"D:\b\shop_20260928_154210".into(),
            created: stamp("20260928_154210"),
            site_zip: Some(r"D:\b\shop_20260928_154210\shop.zip".into()),
            database: Some(r"D:\b\shop_20260928_154210\shop.bacpac".into()),
            note: None,
        };
        let question = confirm_question("shop", &backup);
        assert!(question.starts_with("Put 'shop' back as it was on 2026-09-28 15:42:10?\n\nIts files"));
        assert!(question.ends_with("\n\nBackup: D:\\b\\shop_20260928_154210"));
        let req = RestoreBackupRequest::from_backup("shop", Path::new(r"C:\DNN\shop"), &backup).unwrap();
        assert_eq!(req.database.as_deref(), backup.database.as_deref());
        let half = ProjectBackup { database: None, ..backup.clone() };
        assert_eq!(RestoreBackupRequest::from_backup("shop", Path::new("x"), &half), None);
        let mut lines = Lines::default();
        let no = Answer::no();
        let mut op = Operation::new(&mut lines, &no, CancelToken::new());
        assert_eq!(confirm("shop", &backup, &mut op).unwrap_err(), CANCELLED);
    }
}
