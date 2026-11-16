//! `RemoveProjectUseCase.cs`: removes one or several projects - each with its IIS site and app pool, the pool's user
//! profile, its database (only when it is on this PC, is the project's own and no other site uses it), its keep-warm
//! and project records and, for a project of the projects folder, its folder; its backups only when asked. One
//! question for everything first. None of it can be taken back: each step is noted as such (`CannotUndo`).
//!
//! What the C# reads through `LocalSqlContainer.DatabaseOf`, `DeveloperDb.FromWebConfig`, `SiteDatabases` and
//! `SqlServerAddress.IsOnThisMachine`, and drops with `ISqlServerService` / `IDatabaseProvisioner`, is
//! [`RemoveDatabases`]; the folders (`TryDeleteDirectoryAsync`, `IFileLockService`) are [`ProjectFolders`].
//!
//! Deviation: line breaks in prompts and messages are `\n` (the C#'s `Environment.NewLine` is `\r\n`), which GPUI's
//! text shows as one break.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::iis::IisManager;
use crate::operation::{CANCELLED, CancelToken, Operation};
use crate::records::ProjectRecords;

const NL: &str = "\n";

/// An IIS site to remove, with the folder it serves (`SiteToRemove`). `in_projects_folder`: the folder is one of the
/// projects folder's - a DNN Manager project, whose files are deleted with it. Any other site's files are left where
/// they are: only its IIS site (and the database its web.config names) go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteToRemove {
    pub name: String,
    pub directory: PathBuf,
    pub in_projects_folder: bool,
}

/// What Remove is asked to remove: one row, or the checked rows of the bulk Remove.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoveProjectRequest {
    pub sites: Vec<SiteToRemove>,
}

/// Where a site's database is, as its web.config names it (`DatabaseKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteDatabaseKind {
    /// The local SQL Server container from the settings.
    Container,
    /// Any other SQL Server, on this PC or elsewhere.
    SqlServer,
    /// A LocalDB file in App_Data - it goes with the folder.
    LocalDbFile,
}

/// A site's database as its web.config has it (`DatabaseConnection`). The password is kept to sign in as the site
/// does for the drop, never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct SiteDatabase {
    pub kind: SiteDatabaseKind,
    pub server: String,
    /// The database's name; the .mdf file's for [`SiteDatabaseKind::LocalDbFile`].
    pub database: String,
    pub windows_authentication: bool,
    pub user: String,
    pub password: String,
}

impl fmt::Debug for SiteDatabase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiteDatabase")
            .field("kind", &self.kind)
            .field("server", &self.server)
            .field("database", &self.database)
            .field("windows_authentication", &self.windows_authentication)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// Whether `login` is named as a project's own login on the container (`dnn_shop`) - never the container's own user
/// (`LocalSqlContainer.IsSiteLogin`).
pub fn is_site_login(login: &str) -> bool {
    login.len() >= 4 && login.is_char_boundary(4) && login[..4].eq_ignore_ascii_case("dnn_")
}

/// The databases of sites, and dropping them.
pub trait RemoveDatabases: Send + Sync {
    /// The database the site in `directory` uses, as its web.config has it - `Container` when it is the local
    /// container; None when web.config names none of its own yet (`LocalSqlContainer.DatabaseOf`).
    fn database_of(&self, directory: &Path) -> Option<SiteDatabase>;
    /// The database name (Initial Catalog) of the site's SiteSqlServer connection (`DeveloperDb.FromWebConfig`).
    fn database_name_in_web_config(&self, directory: &Path) -> Option<String>;
    /// The local SQL Server container's address, `host,port` (`LocalSqlContainer.Server`).
    fn container_server(&self) -> String;
    /// `SqlServerAddress.IsOnThisMachine`.
    fn is_on_this_machine(&self, server: &str) -> bool;
    /// The IIS site other than `site` whose web.config names `database` on `server` (`SiteDatabases.OtherSiteUsing`).
    fn other_site_using(&self, site: &str, server: &str, database: &str) -> Option<String>;
    /// The IIS site other than `site` that signs in to `server` as `login` (`SiteDatabases.OtherSiteSigningInAs`).
    fn other_site_signing_in_as(&self, site: &str, server: &str, login: &str) -> Option<String>;
    /// Drops a database on its own server, signed in as the site does (`IDatabaseProvisioner.DropDatabaseAsync`).
    /// "Cancelled." for a cancel.
    fn drop_database(&self, database: &SiteDatabase, cancel: &CancelToken) -> Result<(), String>;
    /// Drops a database on the local container (`ISqlServerService.DropDatabaseAsync`).
    fn drop_container_database(&self, database: &str, cancel: &CancelToken) -> Result<(), String>;
    /// Drops a login on the local container (`ISqlServerService.DropLoginAsync`).
    fn drop_container_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String>;
}

/// A program that keeps a folder from being deleted (`LockingProcess`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locker {
    pub id: u32,
    pub name: String,
    /// `Code.exe`.
    pub exe: String,
    pub reason: String,
    /// Not a service, not Windows itself, not this app.
    pub can_close: bool,
}

impl fmt::Display for Locker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}, pid {}) - {}", self.name, self.exe, self.id, self.reason)
    }
}

/// A project's folders: its backups, deleting, and what holds it open (`IFileLockService`).
pub trait ProjectFolders: Send + Sync {
    /// Where the site's backups are kept (`DnnProject.BackupDirectory`); None for a name that isn't a folder's.
    fn backup_directory(&self, site: &str) -> Option<PathBuf>;
    fn exists(&self, folder: &Path) -> bool;
    /// Deletes the folder with everything in it, read-only files too, never following a link, trying again a few
    /// times while a file is still held (`TryDeleteDirectoryAsync`). Ok(false) when it is still there; Err only for a
    /// cancel.
    fn try_delete(&self, folder: &Path, cancel: &CancelToken) -> Result<bool, String>;
    fn find_lockers(&self, folder: &Path) -> Vec<Locker>;
    /// Closes the programs; the ones still running.
    fn close(&self, lockers: &[Locker]) -> Vec<Locker>;
    /// Has Windows delete what is left at the next restart.
    fn schedule_delete_on_restart(&self, folder: &Path) -> Result<(), String>;
}

/// The sites kept warm (`IKeepWarmRecords`): a removed site's goes with it.
pub trait KeepWarmList: Send + Sync {
    fn remove(&self, site: &str);
}

/// What Remove acts on.
pub struct RemoveDeps<'a> {
    pub iis: &'a dyn IisManager,
    pub databases: &'a dyn RemoveDatabases,
    pub folders: &'a dyn ProjectFolders,
    pub records: &'a dyn ProjectRecords,
    pub keep_warm: &'a dyn KeepWarmList,
}

struct Project<'s> {
    name: &'s str,
    directory: &'s Path,
    backups: Option<PathBuf>,
    delete_files: bool,
}

impl Project<'_> {
    fn has_backups(&self, deps: &RemoveDeps) -> bool {
        self.backups.as_deref().is_some_and(|b| deps.folders.exists(b))
    }
}

/// Removes the sites - each with its database - after asking once for all of them, saying what goes: the IIS site,
/// the folder and the database. Goes on past a project that fails; fails when any did.
pub fn remove_project(deps: &RemoveDeps, request: &RemoveProjectRequest, op: &mut Operation) -> Result<(), String> {
    let sites = &request.sites;
    if sites.is_empty() {
        return Ok(());
    }
    let single = sites.len() == 1;
    let projects: Vec<Project> = sites
        .iter()
        .map(|s| Project {
            name: &s.name,
            directory: &s.directory,
            backups: deps.folders.backup_directory(&s.name),
            delete_files: s.in_projects_folder,
        })
        .collect();

    // One question for everything: a project goes with its database - what is left of it would only be in the way.
    // Backups live outside the project folder, so removing the project keeps them.
    let question = if single { single_question(deps, &projects[0]) } else { bulk_question(deps, &projects) };
    let yes = if single { "Remove project".to_string() } else { format!("Remove {} projects", projects.len()) };
    if !op.prompt.confirm(&question, &yes, "Cancel", true) {
        return Err(CANCELLED.into());
    }

    // Backups outlive the project unless asked otherwise. Kept is the answer that loses nothing.
    let with_backups: Vec<&Project> = projects.iter().filter(|p| p.has_backups(deps)).collect();
    let delete_backups = !with_backups.is_empty() && {
        let first = if single {
            let p = with_backups[0];
            let folder = p.backups.as_deref().map(|b| b.display().to_string()).unwrap_or_default();
            format!("Also delete the backups of '{}' in {folder}?", p.name)
        } else {
            let names: Vec<String> = with_backups.iter().map(|p| format!("'{}'", p.name)).collect();
            format!("Also delete the backups of {}?", names.join(", "))
        };
        op.prompt.confirm(
            &format!(
                "{first}{NL}{NL}Each holds a copy of the site and its database. Deleted, they can't be brought back."
            ),
            "Delete backups",
            "Keep backups",
            true,
        )
    };

    let mut failed: Vec<&str> = Vec::new();
    for project in &projects {
        op.cancel.check()?;
        if !single {
            op.report.step(&format!("Removing '{}'", project.name));
        }
        let result = remove_one(deps, project, op);
        if op.cancel.is_cancelled() || result.as_ref().err().map(String::as_str) == Some(CANCELLED) {
            return Err(CANCELLED.into());
        }
        if result.is_ok()
            && delete_backups
            && let Some(backups) = project.backups.as_deref()
            && deps.folders.exists(backups)
        {
            op.undo.cannot_undo(format!("the backups of '{}' were deleted.", project.name));
            if deps.folders.try_delete(backups, &op.cancel)? {
                op.report.success(&format!("Deleted its backups ({}).", backups.display()));
            } else {
                op.report
                    .warn(&format!("Could not delete its backups in {} - delete them yourself.", backups.display()));
            }
        }
        if let Err(error) = result {
            if single {
                return Err(error);
            }
            op.report.fail(&format!("'{}': {error}", project.name));
            failed.push(project.name);
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(format!("{} of {} projects weren't removed completely: {}.", failed.len(), projects.len(), failed.join(", ")))
}

fn single_question(deps: &RemoveDeps, project: &Project) -> String {
    let dir = project.directory.display();
    let mut goes = vec!["its IIS site and app pool".to_string()];
    if project.delete_files {
        goes.push(format!("its folder {dir}"));
    }
    if let Some(database) = database_text(deps, project) {
        goes.push(format!("its database {database}"));
    }
    let mut kept = Vec::new();
    if !project.delete_files {
        kept.push(format!("Its files in {dir} are kept - they are outside the projects folder."));
    }
    if let Some(remote) = elsewhere(deps, project) {
        kept.push(format!(
            "Its database [{}] on {} is kept - it is on another server.",
            remote.database, remote.server
        ));
    }
    if let Some((database, site)) = shared_database(deps, project) {
        kept.push(format!("Its database [{database}] is kept - the IIS site '{site}' uses it too."));
    }
    if project.has_backups(deps)
        && let Some(backups) = &project.backups
    {
        kept.push(format!("Its backups in {} are kept.", backups.display()));
    }
    let kept = if kept.is_empty() { String::new() } else { format!("{NL}{NL}{}", kept.join(" ")) };
    format!("Remove project '{}' permanently?{NL}{NL}Deleted: {}.{kept}", project.name, join(&goes))
}

fn bulk_question(deps: &RemoveDeps, projects: &[Project]) -> String {
    let list: Vec<String> = projects
        .iter()
        .map(|p| {
            let mut parts = Vec::new();
            if let Some(database) = database_text(deps, p) {
                parts.push(format!("database {database}"));
            }
            if !p.delete_files {
                parts.push("its files outside the projects folder are kept".to_string());
            }
            if let Some(remote) = elsewhere(deps, p) {
                parts.push(format!("its database [{}] on {} is kept - another server", remote.database, remote.server));
            }
            if let Some((database, site)) = shared_database(deps, p) {
                parts.push(format!("its database [{database}] is kept - '{site}' uses it too"));
            }
            if parts.is_empty() { format!("• {}", p.name) } else { format!("• {}  ({})", p.name, parts.join("; ")) }
        })
        .collect();
    let keeps = if projects.iter().any(|p| p.has_backups(deps)) {
        format!("{NL}{NL}Their backups are kept.")
    } else {
        String::new()
    };
    format!(
        "Remove these {} projects permanently - each with its IIS site, folder and database?{NL}{NL}{}{keeps}",
        projects.len(),
        list.join(NL)
    )
}

/// "a", "a and b", "a, b and c".
fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "[shop] on localhost,1433", or "(the file App_Data\Database.mdf)"; None when it has none DNN Manager would drop.
fn database_text(deps: &RemoveDeps, project: &Project) -> Option<String> {
    let database = deps.databases.database_of(project.directory);
    if let Some(d) = &database {
        if d.kind == SiteDatabaseKind::LocalDbFile {
            return project.delete_files.then(|| format!(r"(the file App_Data\{})", d.database));
        }
        if d.kind == SiteDatabaseKind::SqlServer && !deps.databases.is_on_this_machine(&d.server) {
            return None;
        }
    }
    if shared_database(deps, project).is_some() {
        return None;
    }
    if let Some(d) = database {
        return Some(format!("[{}] on {}", d.database, d.server));
    }
    database_name_of(deps, project).map(|name| format!("[{name}] on the local SQL container"))
}

/// The database the site uses when it is on another SQL Server than this PC's: it isn't dropped with the project
/// (others may use it), only named as kept.
fn elsewhere(deps: &RemoveDeps, project: &Project) -> Option<SiteDatabase> {
    deps.databases
        .database_of(project.directory)
        .filter(|d| d.kind == SiteDatabaseKind::SqlServer && !deps.databases.is_on_this_machine(&d.server))
}

/// The database DNN Manager would drop with the project, and the other IIS site whose web.config names it too - then
/// it is that site's as well, and is kept.
fn shared_database(deps: &RemoveDeps, project: &Project) -> Option<(String, String)> {
    let database = deps.databases.database_of(project.directory);
    let (server, name) = match database {
        Some(d) if d.kind == SiteDatabaseKind::LocalDbFile => return None,
        Some(d) => (d.server, d.database),
        None => (deps.databases.container_server(), database_name_of(deps, project)?),
    };
    deps.databases.other_site_using(project.name, &server, &name).map(|other| (name, other))
}

/// The database to drop on the local container when web.config names none of its own: the one its web.config's
/// SiteSqlServer names - or, for a project of the projects folder, the one named like it.
fn database_name_of(deps: &RemoveDeps, project: &Project) -> Option<String> {
    deps.databases
        .database_name_in_web_config(project.directory)
        .filter(|n| !n.trim().is_empty())
        .or_else(|| project.delete_files.then(|| project.name.to_string()))
}

/// Whether `database` - named by the site's web.config, which its app pool can change - is the project's own (named
/// like it), and so may be dropped with it. Another name is asked about first, naming both.
fn is_own_database(project: &Project, database: &str, server: &str, op: &mut Operation) -> bool {
    let own = project.name;
    if database.eq_ignore_ascii_case(own) {
        return true;
    }
    op.prompt.confirm(
        &format!(
            "The web.config of '{own}' names the database [{database}] on {server} - not the project's own, [{own}].{NL}{NL}\
             Drop [{database}] with the project anyway? Another site or program may use it; dropped, it can't be brought back."
        ),
        &format!("Drop [{database}]"),
        "Keep it",
        true,
    )
}

fn remove_one(deps: &RemoveDeps, project: &Project, op: &mut Operation) -> Result<(), String> {
    let name = project.name;
    op.report.step("Step 1: Remove IIS site & pool");
    if let Err(e) = deps.iis.remove_site(name) {
        op.report.fail(&format!(
            "Could not remove the IIS site/pool: {e}. Its worker process may still be holding the project files, so the \
             folder can't be deleted yet. Ensure the app is running as Administrator and try again."
        ));
        return Err(e.0);
    }
    op.report.success(&format!("IIS site and app pool '{name}' removed (if they existed)."));
    // Removing can't be taken back - a cancel says what is already gone.
    op.undo.cannot_undo(format!("the IIS site and app pool '{name}' were removed."));
    // Whether it was kept warm goes with its site - a new site of the same name starts cold.
    deps.keep_warm.remove(name);

    // The app pool's identity got a Windows user profile at C:\Users\<project>; the worker exited during
    // remove_site, so the profile is unloaded and removable.
    op.report.step("Step 2: Remove app pool user profile");
    match deps.iis.remove_app_pool_profile(name) {
        Ok(()) => op.report.success(&format!(r"Removed app pool profile (C:\Users\{name}) if present.")),
        Err(e) => op.report.info(&format!("App pool profile cleanup skipped: {e}")),
    }
    op.cancel.check()?;

    // The database goes with the project - unless another site uses it too. One that couldn't be dropped is named in
    // the result: the project's folder (and web.config, which says which database it was) goes all the same.
    op.report.step("Step 3: Drop project database");
    let leftover = drop_database(deps, project, op)?;

    // What DNN Manager remembered about it (how DNN was installed) goes with the site.
    deps.records.remove(name);
    let done = |leftover: Option<String>| match leftover {
        None => Ok(()),
        Some(left) => Err(format!("'{name}' is removed, but its database {left}")),
    };
    if !project.delete_files {
        op.report
            .info(&format!("{} is outside the projects folder - its files are kept.", project.directory.display()));
        op.report.step("Removal complete");
        return done(leftover);
    }

    op.report.step("Step 4: Delete project directory");
    op.undo.cannot_undo(format!("whatever was already deleted from {}.", project.directory.display()));
    delete_project_folder(deps, project.directory, op)?;

    op.report.step("Removal complete");
    done(leftover)
}

/// Step 3: the database, where it is - or why it is kept. Ok(Some(what is left)) when a drop failed; Err only for a
/// cancel.
fn drop_database(deps: &RemoveDeps, project: &Project, op: &mut Operation) -> Result<Option<String>, String> {
    let databases = deps.databases;
    let database = databases.database_of(project.directory);
    let db_name = database.as_ref().map(|d| d.database.clone()).or_else(|| database_name_of(deps, project));
    if let Some(d) = &database
        && d.kind == SiteDatabaseKind::LocalDbFile
    {
        op.report.info(&if project.delete_files {
            format!(r"Its database is the file App_Data\{} - it goes with the project's folder.", d.database)
        } else {
            format!(r"Its database is the file App_Data\{} - it stays with its files.", d.database)
        });
        return Ok(None);
    }
    if let Some(d) = &database
        && d.kind == SiteDatabaseKind::SqlServer
        && !databases.is_on_this_machine(&d.server)
    {
        op.report.info(&format!(
            "Database [{}] is on {}, another server - it is kept. Drop it there if it should go.",
            d.database, d.server
        ));
        return Ok(None);
    }
    if let Some((shared, site)) = shared_database(deps, project) {
        op.report.info(&format!("Database [{shared}] is kept - the IIS site '{site}' uses it too."));
        return Ok(None);
    }
    if let Some(name) = &db_name {
        let server = database.as_ref().map_or_else(|| databases.container_server(), |d| d.server.clone());
        if !is_own_database(project, name, &server, op) {
            op.report.info(&format!(
                "Database [{name}] is kept - it isn't the project's own ([{}]). Drop it yourself if it should go.",
                project.name
            ));
            return Ok(None);
        }
    }
    if let Some(d) = &database
        && d.kind == SiteDatabaseKind::SqlServer
    {
        // Not on the container but on this PC: dropped on its own server, signed in as the site does.
        return match databases.drop_database(d, &op.cancel) {
            Ok(()) => {
                op.undo.cannot_undo(format!("database [{}] on {} was dropped.", d.database, d.server));
                op.report.success(&format!("Database [{}] dropped on {} (if it existed).", d.database, d.server));
                Ok(None)
            }
            Err(e) if e == CANCELLED => Err(e),
            Err(e) => {
                op.report.fail(&e);
                Ok(Some(format!("[{}] on {} couldn't be dropped ({e}) - drop it yourself.", d.database, d.server)))
            }
        };
    }
    let Some(name) = db_name else {
        op.report.info("No database named in its web.config - none dropped.");
        return Ok(None);
    };
    match databases.drop_container_database(&name, &op.cancel) {
        Ok(()) => {
            op.undo.cannot_undo(format!("database [{name}] was dropped."));
            op.report.success(&format!("Database [{name}] dropped (if it existed)."));
            // The site's own login on the container goes with its database - never the container's own user, nor
            // one another site signs in with.
            if let Some(login) = database.as_ref().map(|d| d.user.as_str()).filter(|u| !u.is_empty())
                && is_site_login(login)
                && databases.other_site_signing_in_as(project.name, &databases.container_server(), login).is_none()
            {
                match databases.drop_container_login(login, &op.cancel) {
                    Ok(()) => op.report.success(&format!("Its login {login} dropped.")),
                    Err(e) if e == CANCELLED => return Err(e),
                    Err(e) => op.report.warn(&format!("Could not drop its login {login}: {e}")),
                }
            }
            Ok(None)
        }
        Err(e) if e == CANCELLED => Err(e),
        Err(e) => {
            op.report.fail(&format!("Could not drop database [{name}]: {e}"));
            Ok(Some(format!(
                "[{name}] on {} couldn't be dropped ({e}) - drop it yourself.",
                databases.container_server()
            )))
        }
    }
}

/// Deletes the project folder. When files are still in use, finds the programs holding them, closes them after asking
/// and tries again. Whatever still can't be deleted is left for Windows to delete at the next restart.
fn delete_project_folder(deps: &RemoveDeps, folder: &Path, op: &mut Operation) -> Result<(), String> {
    let folders = deps.folders;
    let shown = folder.display();
    if !folders.exists(folder) {
        op.report.info(&format!("{shown} doesn't exist - nothing to delete."));
        return Ok(());
    }
    if folders.try_delete(folder, &op.cancel)? {
        op.report.success(&format!("Deleted {shown}"));
        return Ok(());
    }

    op.report.info("Some files are still in use - looking for the programs holding them…");
    let lockers = folders.find_lockers(folder);
    for locker in lockers.iter().filter(|l| !l.can_close) {
        op.report.info(&format!("{locker} - DNN Manager doesn't close this one."));
    }
    let closable: Vec<Locker> = lockers.into_iter().filter(|l| l.can_close).collect();
    if closable.is_empty() {
        op.report.info(
            "No program that DNN Manager can close holds the folder - it may be antivirus or Windows Search, which let \
             go after a moment.",
        );
    } else {
        for locker in &closable {
            op.report.info(&format!("In use by {locker}"));
        }
        let names: Vec<String> = closable.iter().map(|l| format!("• {} ({})", l.name, l.exe)).collect();
        // Closing loses what isn't saved in them: keeping them is the default.
        if op.prompt.confirm(
            &format!(
                "These programs are using {shown}:{NL}{NL}{}{NL}{NL}Close them and delete the folder? Unsaved work in them is lost.",
                names.join(NL)
            ),
            "Close and delete",
            "Don't close",
            true,
        ) {
            let still_running = folders.close(&closable);
            for locker in closable.iter().filter(|l| !still_running.contains(l)) {
                op.report.success(&format!("Closed {} ({}).", locker.name, locker.exe));
            }
            for locker in &still_running {
                op.report.fail(&format!("Could not close {} ({}).", locker.name, locker.exe));
            }
        } else {
            op.report.info("Left the programs open.");
        }
    }

    if folders.try_delete(folder, &op.cancel)? {
        op.report.success(&format!("Deleted {shown}"));
        return Ok(());
    }
    // Still locked: Windows deletes what's left at the next restart, before anything can open it again.
    match folders.schedule_delete_on_restart(folder) {
        Ok(()) => {
            op.report
                .warn(&format!("{shown} is still partly in use - what's left of it is deleted when Windows restarts."));
            Ok(())
        }
        Err(e) => Err(format!(
            "Could not delete {shown}: a file is still in use, and scheduling it for deletion at restart failed ({e}). \
             Close what uses it and remove the project again."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::Lines;
    use crate::records::ProjectRecord;
    use crate::use_cases::tests::FakeIis;
    use std::collections::{HashSet, VecDeque};
    use std::sync::Mutex;

    /// Answers the questions in order (no once they run out), recording them with their yes button.
    #[derive(Default)]
    struct Script {
        answers: Mutex<VecDeque<bool>>,
        asked: Mutex<Vec<(String, String)>>,
    }

    impl Script {
        fn answers(a: &[bool]) -> Self {
            Self { answers: Mutex::new(a.iter().copied().collect()), ..Default::default() }
        }
        fn asked(&self) -> Vec<(String, String)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl crate::operation::Prompt for Script {
        fn confirm(&self, q: &str, yes: &str, _: &str, danger: bool) -> bool {
            assert!(danger, "every question of Remove is a dangerous one");
            self.asked.lock().unwrap().push((q.into(), yes.into()));
            self.answers.lock().unwrap().pop_front().unwrap_or(false)
        }
        fn choose(&self, _: &str, _: &[&str]) -> Option<usize> {
            None
        }
    }

    fn container(db: &str, user: &str) -> SiteDatabase {
        SiteDatabase {
            kind: SiteDatabaseKind::Container,
            server: "localhost,1433".into(),
            database: db.into(),
            windows_authentication: false,
            user: user.into(),
            password: "secret".into(),
        }
    }

    #[derive(Default)]
    struct Dbs {
        of: Vec<(&'static str, SiteDatabase)>,
        web_config_names: Vec<(&'static str, &'static str)>,
        used_by: Vec<(&'static str, &'static str)>,
        login_used_by: Option<&'static str>,
        failing: Vec<&'static str>,
        calls: Mutex<Vec<String>>,
    }

    impl RemoveDatabases for Dbs {
        fn database_of(&self, directory: &Path) -> Option<SiteDatabase> {
            self.of.iter().find(|(d, _)| Path::new(d) == directory).map(|(_, db)| db.clone())
        }
        fn database_name_in_web_config(&self, directory: &Path) -> Option<String> {
            self.web_config_names.iter().find(|(d, _)| Path::new(d) == directory).map(|(_, n)| n.to_string())
        }
        fn container_server(&self) -> String {
            "localhost,1433".into()
        }
        fn is_on_this_machine(&self, server: &str) -> bool {
            server.starts_with("localhost") || server.starts_with('.')
        }
        fn other_site_using(&self, site: &str, _: &str, database: &str) -> Option<String> {
            self.used_by.iter().find(|(db, other)| *db == database && *other != site).map(|(_, o)| o.to_string())
        }
        fn other_site_signing_in_as(&self, _: &str, _: &str, _: &str) -> Option<String> {
            self.login_used_by.map(str::to_string)
        }
        fn drop_database(&self, database: &SiteDatabase, _: &CancelToken) -> Result<(), String> {
            self.call(format!("drop {} on {}", database.database, database.server), &database.database)
        }
        fn drop_container_database(&self, database: &str, _: &CancelToken) -> Result<(), String> {
            self.call(format!("drop {database}"), database)
        }
        fn drop_container_login(&self, login: &str, _: &CancelToken) -> Result<(), String> {
            self.call(format!("drop login {login}"), login)
        }
    }

    impl Dbs {
        fn call(&self, what: String, name: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(what);
            if self.failing.contains(&name) { Err("server not reachable".into()) } else { Ok(()) }
        }
    }

    #[derive(Default)]
    struct Folders {
        existing: Mutex<HashSet<PathBuf>>,
        /// What each delete attempt returns, in order; true once they run out.
        deletes: Mutex<VecDeque<bool>>,
        lockers: Vec<Locker>,
        not_closing: Vec<u32>,
        schedule_fails: bool,
        calls: Mutex<Vec<String>>,
    }

    impl Folders {
        fn with(paths: &[&str]) -> Self {
            Self { existing: Mutex::new(paths.iter().map(PathBuf::from).collect()), ..Default::default() }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProjectFolders for Folders {
        fn backup_directory(&self, site: &str) -> Option<PathBuf> {
            Some(PathBuf::from(format!(r"D:\backups\{site}")))
        }
        fn exists(&self, folder: &Path) -> bool {
            self.existing.lock().unwrap().contains(folder)
        }
        fn try_delete(&self, folder: &Path, _: &CancelToken) -> Result<bool, String> {
            self.calls.lock().unwrap().push(format!("delete {}", folder.display()));
            let done = self.deletes.lock().unwrap().pop_front().unwrap_or(true);
            if done {
                self.existing.lock().unwrap().remove(folder);
            }
            Ok(done)
        }
        fn find_lockers(&self, _: &Path) -> Vec<Locker> {
            self.lockers.clone()
        }
        fn close(&self, lockers: &[Locker]) -> Vec<Locker> {
            self.calls.lock().unwrap().push(format!("close {}", lockers.len()));
            lockers.iter().filter(|l| self.not_closing.contains(&l.id)).cloned().collect()
        }
        fn schedule_delete_on_restart(&self, folder: &Path) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("schedule {}", folder.display()));
            if self.schedule_fails { Err("access denied".into()) } else { Ok(()) }
        }
    }

    #[derive(Default)]
    struct Removed(Mutex<Vec<String>>);

    impl KeepWarmList for Removed {
        fn remove(&self, site: &str) {
            self.0.lock().unwrap().push(format!("warm {site}"));
        }
    }

    impl ProjectRecords for Removed {
        fn find(&self, _: &str) -> Option<ProjectRecord> {
            None
        }
        fn save(&self, _: &ProjectRecord) {}
        fn remove(&self, site: &str) {
            self.0.lock().unwrap().push(format!("record {site}"));
        }
    }

    fn site(name: &str, in_projects_folder: bool) -> SiteToRemove {
        SiteToRemove { name: name.into(), directory: PathBuf::from(format!(r"C:\DNN\{name}")), in_projects_folder }
    }

    struct Run {
        result: Result<(), String>,
        lines: Vec<String>,
        pending: Vec<String>,
    }

    fn run(
        iis: &FakeIis,
        dbs: &Dbs,
        folders: &Folders,
        gone: &Removed,
        prompt: &Script,
        sites: Vec<SiteToRemove>,
    ) -> Run {
        let deps = RemoveDeps { iis, databases: dbs, folders, records: gone, keep_warm: gone };
        let mut lines = Lines::default();
        let mut op = Operation::new(&mut lines, prompt, CancelToken::new());
        let result = remove_project(&deps, &RemoveProjectRequest { sites }, &mut op);
        let pending = op.undo.pending();
        drop(op);
        Run { result, lines: lines.0, pending }
    }

    #[test]
    fn a_no_removes_nothing() {
        let (iis, dbs, folders, gone) = (FakeIis::running(), Dbs::default(), Folders::default(), Removed::default());
        let prompt = Script::answers(&[false]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        assert_eq!(r.result.unwrap_err(), CANCELLED);
        assert!(iis.calls.lock().unwrap().is_empty() && folders.calls().is_empty() && r.lines.is_empty());
        assert_eq!(
            prompt.asked(),
            vec![(
                "Remove project 'shop' permanently?\n\nDeleted: its IIS site and app pool, its folder C:\\DNN\\shop and \
                 its database [shop] on the local SQL container."
                    .to_string(),
                "Remove project".to_string()
            )]
        );
    }

    #[test]
    fn removes_site_profile_database_login_records_and_folder() {
        let iis = FakeIis::running();
        let dbs = Dbs { of: vec![(r"C:\DNN\shop", container("shop", "dnn_shop"))], ..Default::default() };
        let folders = Folders::with(&[r"C:\DNN\shop"]);
        let gone = Removed::default();
        let prompt = Script::answers(&[true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        r.result.unwrap();
        assert_eq!(prompt.asked()[0].0.lines().nth(2).unwrap(), {
            "Deleted: its IIS site and app pool, its folder C:\\DNN\\shop and its database [shop] on localhost,1433."
        });
        assert_eq!(*iis.calls.lock().unwrap(), vec!["remove shop", "remove profile shop"]);
        assert_eq!(*dbs.calls.lock().unwrap(), vec!["drop shop", "drop login dnn_shop"]);
        assert_eq!(*gone.0.lock().unwrap(), vec!["warm shop", "record shop"]);
        assert_eq!(folders.calls(), vec![r"delete C:\DNN\shop"]);
        assert_eq!(
            r.lines,
            vec![
                "## Step 1: Remove IIS site & pool",
                "ok IIS site and app pool 'shop' removed (if they existed).",
                "## Step 2: Remove app pool user profile",
                r"ok Removed app pool profile (C:\Users\shop) if present.",
                "## Step 3: Drop project database",
                "ok Database [shop] dropped (if it existed).",
                "ok Its login dnn_shop dropped.",
                "## Step 4: Delete project directory",
                r"ok Deleted C:\DNN\shop",
                "## Removal complete",
            ]
        );
        // Every step is gone for good: an undo only says so.
        assert_eq!(
            r.pending,
            vec![
                "Can't be put back: the IIS site and app pool 'shop' were removed.",
                "Can't be put back: database [shop] was dropped.",
                r"Can't be put back: whatever was already deleted from C:\DNN\shop.",
            ]
        );
    }

    #[test]
    fn a_database_another_site_uses_is_kept_and_so_is_its_login() {
        let iis = FakeIis::running();
        let dbs = Dbs {
            of: vec![(r"C:\DNN\shop", container("shared", "dnn_shop"))],
            used_by: vec![("shared", "blog")],
            ..Default::default()
        };
        let (folders, gone, prompt) = (Folders::default(), Removed::default(), Script::answers(&[true]));
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        r.result.unwrap();
        assert!(dbs.calls.lock().unwrap().is_empty());
        assert!(prompt.asked()[0].0.ends_with("Its database [shared] is kept - the IIS site 'blog' uses it too."));
        assert!(!prompt.asked()[0].0.contains("its database [shared]"), "not named as deleted");
        assert!(r.lines.contains(&"Database [shared] is kept - the IIS site 'blog' uses it too.".to_string()));
        assert!(r.lines.contains(&r"C:\DNN\shop doesn't exist - nothing to delete.".to_string()));
    }

    #[test]
    fn a_database_on_another_server_or_not_the_projects_own_is_kept() {
        let iis = FakeIis::running();
        let remote = SiteDatabase {
            kind: SiteDatabaseKind::SqlServer,
            server: "db.example.com".into(),
            ..container("shop", "u")
        };
        let dbs =
            Dbs { of: vec![(r"C:\DNN\shop", remote), (r"C:\DNN\blog", container("other", ""))], ..Default::default() };
        let (folders, gone) = (Folders::default(), Removed::default());
        // Yes to Remove, no to dropping [other].
        let prompt = Script::answers(&[true, false]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true), site("blog", true)]);
        r.result.unwrap();
        assert!(dbs.calls.lock().unwrap().is_empty());
        let asked = prompt.asked();
        assert_eq!(
            asked[0].0,
            "Remove these 2 projects permanently - each with its IIS site, folder and database?\n\n\
             • shop  (its database [shop] on db.example.com is kept - another server)\n\
             • blog  (database [other] on localhost,1433)"
        );
        assert_eq!(asked[0].1, "Remove 2 projects");
        assert_eq!(asked[1].1, "Drop [other]");
        assert!(asked[1].0.starts_with(
            "The web.config of 'blog' names the database [other] on localhost,1433 - not the project's own, [blog]."
        ));
        assert!(r.lines.contains(
            &"Database [shop] is on db.example.com, another server - it is kept. Drop it there if it should go.".into()
        ));
        assert!(r.lines.contains(
            &"Database [other] is kept - it isn't the project's own ([blog]). Drop it yourself if it should go.".into()
        ));
    }

    #[test]
    fn a_local_sql_server_database_is_dropped_on_its_server_and_a_shared_login_is_kept() {
        let iis = FakeIis::running();
        let local =
            SiteDatabase { kind: SiteDatabaseKind::SqlServer, server: r".\SQLEXPRESS".into(), ..container("shop", "") };
        let dbs = Dbs { of: vec![(r"C:\DNN\shop", local)], ..Default::default() };
        let (folders, gone, prompt) = (Folders::default(), Removed::default(), Script::answers(&[true]));
        run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]).result.unwrap();
        assert_eq!(*dbs.calls.lock().unwrap(), vec![r"drop shop on .\SQLEXPRESS"]);

        let dbs = Dbs {
            of: vec![(r"C:\DNN\shop", container("shop", "dnn_old"))],
            login_used_by: Some("blog"),
            ..Default::default()
        };
        let prompt = Script::answers(&[true]);
        run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]).result.unwrap();
        assert_eq!(*dbs.calls.lock().unwrap(), vec!["drop shop"], "another site signs in as dnn_old");
    }

    #[test]
    fn a_site_outside_the_projects_folder_keeps_its_files_and_unnamed_database() {
        let iis = FakeIis::running();
        let (dbs, folders, gone) = (Dbs::default(), Folders::with(&[r"C:\DNN\shop"]), Removed::default());
        let prompt = Script::answers(&[true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", false)]);
        r.result.unwrap();
        assert_eq!(
            prompt.asked()[0].0,
            "Remove project 'shop' permanently?\n\nDeleted: its IIS site and app pool.\n\n\
             Its files in C:\\DNN\\shop are kept - they are outside the projects folder."
        );
        assert!(folders.calls().is_empty() && dbs.calls.lock().unwrap().is_empty());
        assert_eq!(
            r.lines[5..],
            [
                "No database named in its web.config - none dropped.",
                r"C:\DNN\shop is outside the projects folder - its files are kept.",
                "## Removal complete"
            ]
        );
    }

    fn locker(id: u32, name: &str, can_close: bool) -> Locker {
        Locker {
            id,
            name: name.into(),
            exe: format!("{name}.exe"),
            reason: "has files in the folder open".into(),
            can_close,
        }
    }

    #[test]
    fn locked_files_close_their_programs_on_consent_and_delete_again() {
        let iis = FakeIis::running();
        let folders = Folders {
            deletes: Mutex::new([false].into()),
            lockers: vec![locker(7, "Code", true), locker(4, "MsMpEng", false)],
            ..Folders::with(&[r"C:\DNN\shop"])
        };
        let (dbs, gone) = (Dbs::default(), Removed::default());
        let prompt = Script::answers(&[true, true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        r.result.unwrap();
        assert_eq!(folders.calls(), vec![r"delete C:\DNN\shop", "close 1", r"delete C:\DNN\shop"]);
        let asked = prompt.asked();
        assert_eq!(
            asked[1],
            (
                "These programs are using C:\\DNN\\shop:\n\n• Code (Code.exe)\n\nClose them and delete the folder? \
                 Unsaved work in them is lost."
                    .to_string(),
                "Close and delete".to_string()
            )
        );
        assert!(r.lines.contains(
            &"MsMpEng (MsMpEng.exe, pid 4) - has files in the folder open - DNN Manager doesn't close this one.".into()
        ));
        assert!(r.lines.contains(&"In use by Code (Code.exe, pid 7) - has files in the folder open".into()));
        assert!(r.lines.contains(&"ok Closed Code (Code.exe).".into()));
    }

    #[test]
    fn locked_files_left_open_are_deleted_at_restart_or_fail() {
        let iis = FakeIis::running();
        let (dbs, gone) = (Dbs::default(), Removed::default());
        let folders = Folders {
            deletes: Mutex::new([false, false].into()),
            lockers: vec![locker(7, "Code", true)],
            ..Folders::with(&[r"C:\DNN\shop"])
        };
        // Remove, but don't close Code.
        let prompt = Script::answers(&[true, false]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        r.result.unwrap();
        assert!(r.lines.contains(&"Left the programs open.".into()));
        assert!(r.lines.contains(
            &r"warn C:\DNN\shop is still partly in use - what's left of it is deleted when Windows restarts.".into()
        ));
        assert_eq!(folders.calls().last().unwrap(), r"schedule C:\DNN\shop");

        let folders = Folders {
            deletes: Mutex::new([false, false].into()),
            schedule_fails: true,
            ..Folders::with(&[r"C:\DNN\shop"])
        };
        let prompt = Script::answers(&[true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        assert_eq!(
            r.result.unwrap_err(),
            r"Could not delete C:\DNN\shop: a file is still in use, and scheduling it for deletion at restart failed (access denied). Close what uses it and remove the project again."
        );
        assert!(!r.lines.contains(&"## Removal complete".into()));
    }

    #[test]
    fn goes_on_past_a_project_that_fails_and_names_the_failed() {
        let iis = FakeIis { failing: vec!["a"], ..FakeIis::running() };
        let dbs = Dbs { failing: vec!["c"], ..Default::default() };
        let (folders, gone) = (Folders::default(), Removed::default());
        let prompt = Script::answers(&[true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("a", true), site("b", true), site("c", true)]);
        assert_eq!(r.result.unwrap_err(), "2 of 3 projects weren't removed completely: a, c.");
        assert!(r.lines.contains(&"## Removing 'b'".into()));
        assert!(r.lines.contains(&"! 'a': access denied".into()));
        assert!(r.lines.contains(&"! Could not drop database [c]: server not reachable".into()));
        assert!(r.lines.contains(
            &"! 'c': 'c' is removed, but its database [c] on localhost,1433 couldn't be dropped (server not reachable) - drop it yourself.".into()
        ));
        // c's folder goes all the same; a's isn't touched.
        assert_eq!(*gone.0.lock().unwrap(), vec!["warm b", "record b", "warm c", "record c"]);
    }

    #[test]
    fn a_single_failure_is_its_own_message() {
        let iis = FakeIis { failing: vec!["shop"], ..FakeIis::running() };
        let (dbs, folders, gone) = (Dbs::default(), Folders::default(), Removed::default());
        let prompt = Script::answers(&[true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        assert_eq!(r.result.unwrap_err(), "access denied");
        assert!(r.lines[1].starts_with("! Could not remove the IIS site/pool: access denied. Its worker process"));
        assert!(gone.0.lock().unwrap().is_empty() && r.pending.is_empty());
    }

    #[test]
    fn backups_are_kept_unless_asked_and_then_deleted() {
        let iis = FakeIis::running();
        let (dbs, gone) = (Dbs::default(), Removed::default());
        let folders = Folders::with(&[r"D:\backups\shop"]);
        let prompt = Script::answers(&[true, false, true]);
        run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]).result.unwrap();
        let asked = prompt.asked();
        assert!(asked[0].0.ends_with(r"Its backups in D:\backups\shop are kept."));
        assert_eq!(
            asked[1],
            (
                "Also delete the backups of 'shop' in D:\\backups\\shop?\n\nEach holds a copy of the site and its \
                 database. Deleted, they can't be brought back."
                    .to_string(),
                "Delete backups".to_string()
            )
        );
        assert!(folders.exists(Path::new(r"D:\backups\shop")), "a no keeps them");

        let prompt = Script::answers(&[true, true, true]);
        let r = run(&iis, &dbs, &folders, &gone, &prompt, vec![site("shop", true)]);
        r.result.unwrap();
        assert!(r.lines.contains(&r"ok Deleted its backups (D:\backups\shop).".into()));
        assert!(r.pending.contains(&"Can't be put back: the backups of 'shop' were deleted.".into()));
    }

    #[test]
    fn site_logins_are_the_dnn_ones() {
        assert!(is_site_login("dnn_shop") && is_site_login("DNN_x"));
        assert!(!is_site_login("sa") && !is_site_login("dnn") && !is_site_login("dnné"));
        assert_eq!(join(&["a".into(), "b".into(), "c".into()]), "a, b and c");
    }
}
