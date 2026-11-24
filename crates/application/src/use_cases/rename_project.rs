//! `RenameProjectUseCase.cs` (and the `problem` check of `Pages/Projects/ProjectEdits.cs`): renames a project - its IIS
//! site and app pool, its folder when it is the project's own in the projects folder, its records (how it was
//! installed, keep warm) - and, when asked, its host name (with DNN's portal aliases) and its database. The site is
//! stopped meanwhile and started again if it ran. A step that fails takes back the ones before it. Its backups stay
//! under the old name: they are copies of the project as it was.
//!
//! Also how the use cases that edit a project act on its database ([`SiteDatabases`]) - shared with Edit host names.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dnnm_domain::validate_project_name;

use crate::iis::{IisBinding, IisManager, states};
use crate::operation::{CANCELLED, CancelToken, FILE_STEP_LIMIT, Operation, STEP_LIMIT};
use crate::records::ProjectRecords;
use crate::use_cases::database_connection::{DatabaseConnection, DatabaseKind};
use crate::use_cases::provisioner::alias_for;

/// A site's database, as renaming a project and editing its host names act on it (`LocalSqlContainer.DatabaseOf`,
/// `IDatabaseProvisioner`, `IWebConfigService.WriteDatabaseConnection`, `SqlServerAddress`). A cancelled call ends with
/// [`CANCELLED`].
pub trait SiteDatabases: Send + Sync {
    /// The database the web.config in `directory` names; None when it names none of its own yet.
    fn database_of(&self, directory: &Path) -> Option<DatabaseConnection>;
    /// Renames the database to `new_name`, closing the connections still open to it.
    fn rename(&self, database: &DatabaseConnection, new_name: &str, cancel: &CancelToken) -> Result<(), String>;
    /// Points web.config's SiteSqlServer at `database`.
    fn write_connection(&self, web_config: &Path, database: &DatabaseConnection) -> Result<(), String>;
    /// Lets the Windows login (`IIS APPPOOL\site`) own the database.
    fn grant_site_access(&self, database: &DatabaseConnection, login: &str, cancel: &CancelToken)
    -> Result<(), String>;
    /// Every alias `from` becomes `to`; each of `added` becomes an alias of portal 0. How many changed.
    fn update_portal_aliases(
        &self,
        database: &DatabaseConnection,
        site_directory: &Path,
        renamed: &[(String, String)],
        added: &[String],
        cancel: &CancelToken,
    ) -> Result<i64, String>;
    /// Whether DNN Manager may sign in to `server` with the user's Windows account without asking.
    fn may_sign_in_as_user(&self, server: &str) -> bool;
    /// The question asked before it does when it may not (`SqlServerAddress.SignInQuestion`).
    fn sign_in_question(&self, server: &str, why: &str) -> String;
}

/// The projects folder (`IProjectRepository.Build(name).ProjectDirectory`) and moving a project's folder.
pub trait ProjectFolders: Send + Sync {
    /// The folder a project of this name has in the projects folder.
    fn project_directory(&self, name: &str) -> PathBuf;
    fn exists(&self, folder: &Path) -> bool;
    /// `Directory.Move`.
    fn move_folder(&self, from: &Path, to: &Path) -> Result<(), String>;
}

/// The sites kept warm (`IKeepWarmRecords`): the name as recorded, whatever its case.
pub trait KeepWarmTable: Send + Sync {
    fn find(&self, site: &str) -> Option<String>;
    /// Switches `site` on.
    fn save(&self, site: &str);
    fn remove(&self, site: &str);
}

/// What Rename project acts on.
#[derive(Clone)]
pub struct RenameDeps {
    pub iis: Arc<dyn IisManager>,
    pub folders: Arc<dyn ProjectFolders>,
    pub databases: Arc<dyn SiteDatabases>,
    pub records: Arc<dyn ProjectRecords>,
    pub keep_warm: Arc<dyn KeepWarmTable>,
    /// Settings → Projects' host name suffix: a project `x` answers on `x.<suffix>` (`AppOptions.HostnameFor`).
    pub hostname_suffix: String,
}

/// `RenameProjectRequest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameProjectRequest {
    /// The IIS site's name now.
    pub site_name: String,
    /// The folder the site serves.
    pub directory: PathBuf,
    pub new_name: String,
    /// Its http bindings' host name `<old>.<suffix>` becomes `<new>.<suffix>`, and DNN's portal aliases with it.
    pub rename_host: bool,
    /// Its database (not a LocalDB file) is renamed to the new name, and web.config follows.
    pub rename_database: bool,
}

/// What the Rename dialog shows (`ProjectEdits.Rename` gathers it): the sites IIS has, for the name check; the host
/// name to offer to change - when the site answers on `<name>.<suffix>`; the database to offer to rename - when it is
/// named like the project and isn't a LocalDB file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameDialogFacts {
    pub sites: Vec<String>,
    pub current_host: Option<String>,
    pub hostname_suffix: String,
    pub database: Option<String>,
}

impl RenameDialogFacts {
    /// From what IIS and web.config say about the project `name`.
    pub fn new(
        name: &str,
        sites: Vec<String>,
        bindings: &[IisBinding],
        hostname_suffix: &str,
        database: Option<&DatabaseConnection>,
    ) -> Self {
        let host = hostname_for(name, hostname_suffix);
        let current_host = bindings.iter().any(|b| b.host.eq_ignore_ascii_case(&host)).then_some(host);
        let database = database
            .filter(|d| d.kind != DatabaseKind::LocalDbFile && d.database.eq_ignore_ascii_case(name))
            .map(|d| d.database.clone());
        Self { sites, current_host, hostname_suffix: hostname_suffix.to_string(), database }
    }
}

/// `AppOptions.HostnameFor`: `<name>.<suffix>`.
pub fn hostname_for(name: &str, suffix: &str) -> String {
    format!("{name}.{suffix}")
}

/// What is wrong with `candidate` as the new name of `current` (`ProjectEdits.Rename`'s `Problem`); None when it is
/// fine. IIS's site names are compared ignoring case, as IIS does.
pub fn name_problem(candidate: &str, current: &str, sites: &[String]) -> Option<String> {
    if let Err(e) = validate_project_name(candidate) {
        return Some(e);
    }
    (!candidate.eq_ignore_ascii_case(current) && sites.iter().any(|s| s.eq_ignore_ascii_case(candidate)))
        .then(|| format!("IIS already has a site named '{candidate}'."))
}

/// The same database under another name (`database with { Database = … }`).
fn named(database: &DatabaseConnection, name: &str) -> DatabaseConnection {
    DatabaseConnection { database: name.to_string(), ..database.clone() }
}

/// `Path.GetFullPath(…).TrimEnd('\\')`.
fn full(path: &Path) -> PathBuf {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    PathBuf::from(full.to_string_lossy().trim_end_matches('\\').to_string())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// The http bindings that have a port, as (host, port).
pub(crate) fn http_bindings(bindings: &[crate::iis::IisBindingDetails]) -> Vec<(String, u16)> {
    bindings
        .iter()
        .filter(|b| b.protocol.eq_ignore_ascii_case("http"))
        .filter_map(|b| b.port.map(|p| (b.host.clone(), p)))
        .collect()
}

/// `RenameProjectUseCase.ExecuteAsync`. A failure is returned for the runner, which takes back what was noted - the
/// C# runs its undo itself under the step "Taking the rename back"; here it is the runner's "Undo".
pub fn rename_project(deps: &RenameDeps, req: &RenameProjectRequest, op: &mut Operation) -> Result<(), String> {
    let old_name = req.site_name.as_str();
    let new_name = req.new_name.trim().to_string();
    validate_project_name(&new_name)?;
    if new_name == old_name {
        return Ok(());
    }
    let same_ignoring_case = new_name.eq_ignore_ascii_case(old_name);
    if !same_ignoring_case && deps.iis.site_states().keys().any(|k| k.eq_ignore_ascii_case(&new_name)) {
        return Err(format!("IIS already has a site named '{new_name}'."));
    }
    let Some(site) = deps.iis.site_details(old_name) else {
        return Err(format!("IIS has no site named '{old_name}'."));
    };

    // The project's own folder in the projects folder is renamed with it; a folder elsewhere stays where it is.
    let old_directory = full(&req.directory);
    let own_folder = same_path(&old_directory, &full(&deps.folders.project_directory(old_name)));
    let new_directory =
        if own_folder { full(&deps.folders.project_directory(&new_name)) } else { old_directory.clone() };
    if own_folder && !same_ignoring_case && deps.folders.exists(&new_directory) {
        return Err(format!("The folder {} exists already - choose another name.", new_directory.display()));
    }

    let database = deps.databases.database_of(&old_directory);
    let rename_database = req.rename_database
        && database
            .as_ref()
            .is_some_and(|d| d.kind != DatabaseKind::LocalDbFile && !d.database.eq_ignore_ascii_case(&new_name));
    let was_running = states::is_started(&site.state);

    // The C#'s reporter.Context("old → new") has no counterpart here: the operation's title already says it.
    op.report.plan(&["IIS site", "Folder", "Host name", "Database", "Records", "Start"]);

    op.cancel.check()?;
    op.report.step(&format!("Renaming the IIS site '{old_name}' to '{new_name}'"));
    {
        let (iis, new, old, dir) = (deps.iis.clone(), new_name.clone(), old_name.to_string(), old_directory.clone());
        op.undo.add(format!("Rename the IIS site back to '{old_name}'"), STEP_LIMIT, move |_| {
            iis.rename_site(&new, &old, &dir).map_err(|e| e.0)
        });
    }
    deps.iis
        .rename_site(old_name, &new_name, &new_directory)
        .map_err(|e| format!("Could not rename the IIS site: {e}"))?;
    op.report.success(&format!("IIS site and app pool are '{new_name}' (stopped for the rename)."));

    op.cancel.check()?;
    op.report.step("Renaming the folder");
    if own_folder && !same_ignoring_case {
        let (folders, from, to) = (deps.folders.clone(), new_directory.clone(), old_directory.clone());
        op.undo.add(format!("Move the folder back to {}", old_directory.display()), FILE_STEP_LIMIT, move |_| {
            folders.move_folder(&from, &to)
        });
        if let Err(e) = deps.folders.move_folder(&old_directory, &new_directory) {
            return Err(format!(
                "Could not rename {} to {}: {e} Close what has files of the project open (an editor, a terminal) and try again.",
                old_directory.display(),
                new_directory.display()
            ));
        }
        op.report.success(&format!("Folder: {}", new_directory.display()));
    } else if own_folder {
        op.report.info("The folder keeps its name - only its capitals differ.");
    } else {
        op.report.info(&format!("The folder stays where it is: {}", old_directory.display()));
    }
    // The renamed app pool runs as IIS APPPOOL\<new name> - a new Windows identity.
    let identity = format!(r"IIS APPPOOL\{new_name}");
    if let Err(e) = deps.iis.grant_permissions(&new_directory, std::slice::from_ref(&identity)) {
        op.report.warn(&format!("Could not give {identity} access to {}: {e}", new_directory.display()));
    }

    op.cancel.check()?;
    op.report.step("Host name");
    if req.rename_host {
        rename_host(deps, &site.bindings, old_name, &new_name, &new_directory, database.as_ref(), op)?;
    } else {
        op.report.info("Kept as it is.");
    }

    op.cancel.check()?;
    op.report.step("Database");
    let mut site_database = database.clone();
    match &database {
        Some(db) if rename_database => {
            let web_config = new_directory.join("web.config");
            op.undo.restore_file_on_undo(&web_config);
            let (databases, renamed, old_db) = (deps.databases.clone(), named(db, &new_name), db.database.clone());
            op.undo.add(format!("Rename the database back to [{}]", db.database), STEP_LIMIT, move |_| {
                databases.rename(&renamed, &old_db, &CancelToken::new())
            });
            deps.databases.rename(db, &new_name, &op.cancel)?;
            let named = named(db, &new_name);
            if let Err(e) = deps.databases.write_connection(&web_config, &named) {
                return Err(format!("The database is [{new_name}], but web.config couldn't be updated: {e}"));
            }
            op.report.success(&format!("Database [{}] is [{new_name}]; web.config points at it.", db.database));
            site_database = Some(named);
        }
        Some(db) => op.report.info(&format!("Kept as it is: [{}].", db.database)),
        None => op.report.info("web.config names no database of its own."),
    }
    // Signing in as the app pool, the site has a new Windows login now.
    if let Some(db) = site_database.as_ref().filter(|d| d.kind != DatabaseKind::LocalDbFile)
        && db.uses_windows_authentication()
    {
        match deps.databases.grant_site_access(db, &identity, &op.cancel) {
            Ok(()) => op.report.success(&format!("{identity} may use [{}].", db.database)),
            Err(e) if e == CANCELLED => return Err(e),
            Err(e) => op.report.warn(&e),
        }
    }

    op.cancel.check()?;
    op.report.step("Records");
    move_records(deps, old_name, &new_name, op);
    op.report.success("How it was installed and its keep-warm choice go with the new name.");

    op.report.step(&if was_running { format!("Starting '{new_name}'") } else { "Start".to_string() });
    if was_running {
        match deps.iis.start_site(&new_name) {
            Ok(()) => op.report.success(&format!("'{new_name}' runs.")),
            Err(e) => op.report.warn(&format!("'{new_name}' couldn't be started: {e}")),
        }
    } else {
        op.report.info("It was stopped - it stays stopped.");
    }
    // The old app pool identity's Windows profile is left over - nothing runs as it any more.
    let _ = deps.iis.remove_app_pool_profile(old_name);
    Ok(())
}

/// The http bindings on `<old>.<suffix>` become `<new>.<suffix>`, and DNN's portal aliases follow (`RenameHostAsync`).
fn rename_host(
    deps: &RenameDeps,
    bindings: &[crate::iis::IisBindingDetails],
    old_name: &str,
    new_name: &str,
    directory: &Path,
    database: Option<&DatabaseConnection>,
    op: &mut Operation,
) -> Result<(), String> {
    let (old_host, new_host) =
        (hostname_for(old_name, &deps.hostname_suffix), hostname_for(new_name, &deps.hostname_suffix));
    let http = http_bindings(bindings);
    if !http.iter().any(|(h, _)| h.eq_ignore_ascii_case(&old_host)) {
        op.report.info(&format!("No binding on {old_host} - nothing to change."));
        return Ok(());
    }
    // The site's own host name, and the other portals' under it (`flip.<old>.dnndev.me`).
    let moved = |h: &str| -> Option<String> {
        let lower = h.to_ascii_lowercase();
        if lower == old_host.to_ascii_lowercase() {
            Some(new_host.clone())
        } else {
            lower.strip_suffix(&format!(".{}", old_host.to_ascii_lowercase())).map(|sub| format!("{sub}.{new_host}"))
        }
    };
    let next: Vec<(String, u16)> =
        http.iter().map(|(h, p)| (moved(h).unwrap_or_else(|| h.clone()), *p)).collect();
    {
        let (iis, site, before) = (deps.iis.clone(), new_name.to_string(), http.clone());
        op.undo.add("Put the bindings back", STEP_LIMIT, move |_| {
            iis.replace_http_bindings(&site, &before).map_err(|e| e.0)
        });
    }
    deps.iis.replace_http_bindings(new_name, &next).map_err(|e| format!("Could not change the bindings: {e}"))?;
    op.report.success(&format!("Bound to {new_host} instead of {old_host}."));

    let Some(database) = database else {
        op.report.warn(&format!("web.config names no database - DNN's portal alias {old_host} stays as it is."));
        return Ok(());
    };
    let aliases: Vec<(String, String)> = http
        .iter()
        .filter_map(|(h, p)| moved(h).map(|to| (alias_for(h, *p), alias_for(&to, *p))))
        .collect();
    {
        let (databases, db, dir) = (deps.databases.clone(), database.clone(), directory.to_path_buf());
        let back: Vec<(String, String)> = aliases.iter().map(|(from, to)| (to.clone(), from.clone())).collect();
        op.undo.add("Put DNN's portal aliases back", STEP_LIMIT, move |_| {
            databases.update_portal_aliases(&db, &dir, &back, &[], &CancelToken::new()).map(|_| ())
        });
    }
    match deps.databases.update_portal_aliases(database, directory, &aliases, &[], &op.cancel) {
        Ok(changed) => op.report.success(&format!("DNN's portal alias is {new_host} ({changed} changed).")),
        Err(e) if e == CANCELLED => return Err(e),
        Err(e) => op
            .report
            .warn(&format!("{e} - add {new_host} as a site alias in DNN (Settings → Site Settings → Site Aliases).")),
    }
    Ok(())
}

fn move_records(deps: &RenameDeps, old_name: &str, new_name: &str, op: &mut Operation) {
    let renamed = !old_name.eq_ignore_ascii_case(new_name);
    if let Some(record) = deps.records.find(old_name) {
        let (records, new, before) = (deps.records.clone(), new_name.to_string(), record.clone());
        op.undo.add("Put the project record back", STEP_LIMIT, move |_| {
            records.remove(&new);
            records.save(&before);
            Ok(())
        });
        deps.records.save(&crate::records::ProjectRecord { site: new_name.to_string(), ..record });
        if renamed {
            deps.records.remove(old_name);
        }
    }
    if let Some(warm) = deps.keep_warm.find(old_name) {
        let (keep_warm, new) = (deps.keep_warm.clone(), new_name.to_string());
        op.undo.add("Put the keep-warm record back", STEP_LIMIT, move |_| {
            keep_warm.remove(&new);
            keep_warm.save(&warm);
            Ok(())
        });
        deps.keep_warm.save(new_name);
        if renamed {
            deps.keep_warm.remove(old_name);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::iis::{
        IisBindingDetails, IisError, IisPoolDetails, IisPoolSettings, IisServerAction, IisServerState, IisSiteDetails,
        IisSiteRuntime,
    };
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{Outcome, run};
    use crate::records::{InstallMode, ProjectRecord};
    use crate::use_cases::database_connection::SqlAuthentication;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;
    use std::time::Duration;

    pub type Log = Arc<Mutex<Vec<String>>>;

    fn push(log: &Log, line: String) {
        log.lock().unwrap().push(line);
    }

    /// A stand-in IIS for the edit use cases: one site, its details, every change written to `log`; `fail` names the
    /// calls that fail ("rename", "bindings", "pool", "grant", "start").
    pub struct Iis {
        pub log: Log,
        pub details: Mutex<Option<IisSiteDetails>>,
        pub sites: Vec<String>,
        pub fail: Vec<&'static str>,
    }

    impl Iis {
        pub fn new(log: &Log, details: Option<IisSiteDetails>) -> Self {
            Self { log: log.clone(), details: Mutex::new(details), sites: vec!["shop".into()], fail: vec![] }
        }

        fn call(&self, kind: &str, line: String) -> Result<(), IisError> {
            push(&self.log, line);
            if self.fail.contains(&kind) { Err(IisError(format!("{kind} refused"))) } else { Ok(()) }
        }
    }

    impl IisManager for Iis {
        fn is_available(&self) -> bool {
            true
        }
        fn server_state(&self) -> IisServerState {
            IisServerState::Running
        }
        fn site_runtimes(&self) -> Result<BTreeMap<String, IisSiteRuntime>, IisError> {
            Ok(BTreeMap::new())
        }
        fn site_states(&self) -> BTreeMap<String, String> {
            self.sites.iter().map(|s| (s.clone(), "Started".to_string())).collect()
        }
        fn start_site(&self, site: &str) -> Result<(), IisError> {
            self.call("start", format!("start {site}"))
        }
        fn stop_site(&self, site: &str) -> Result<(), IisError> {
            self.call("stop", format!("stop {site}"))
        }
        fn restart_site(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn recycle_app_pool(&self, site: &str) -> Result<(), IisError> {
            self.call("recycle", format!("recycle {site}"))
        }
        fn control_server(&self, _: IisServerAction) -> Result<(), IisError> {
            Ok(())
        }
        fn create_site(&self, _: &str, _: &Path, _: &str, _: u16) -> Result<(), IisError> {
            Ok(())
        }
        fn remove_site(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn log_directory(&self, _: &str) -> Option<PathBuf> {
            None
        }
        fn app_pool_identity(&self, site: &str) -> String {
            format!(r"NT AUTHORITY\NETWORK SERVICE for {site}")
        }
        fn enable_user_profile(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn site_traffic(&self) -> HashMap<String, crate::project::SiteTraffic> {
            HashMap::new()
        }
        fn requests_served(&self) -> HashMap<String, u64> {
            HashMap::new()
        }
        fn site_details(&self, site: &str) -> Option<IisSiteDetails> {
            self.details.lock().unwrap().clone().filter(|d| d.name.eq_ignore_ascii_case(site))
        }
        fn replace_http_bindings(&self, site: &str, bindings: &[(String, u16)]) -> Result<(), IisError> {
            let list: Vec<String> = bindings.iter().map(|(h, p)| format!("{h}:{p}")).collect();
            self.call("bindings", format!("bindings {site} {}", list.join(",")))
        }
        fn set_pool_settings(&self, site: &str, s: &IisPoolSettings) -> Result<(), IisError> {
            self.call("pool", format!("pool {site} {} {} {}", s.runtime, s.identity, s.idle_timeout.as_secs()))
        }
        fn rename_site(&self, site: &str, new_name: &str, path: &Path) -> Result<(), IisError> {
            self.call("rename", format!("rename {site} -> {new_name} at {}", path.display()))
        }
        fn grant_permissions(&self, path: &Path, identities: &[String]) -> Result<(), IisError> {
            self.call("grant", format!("grant {} {}", identities.join("+"), path.display()))
        }
        fn remove_app_pool_profile(&self, pool: &str) -> Result<(), IisError> {
            self.call("profile", format!("remove profile {pool}"))
        }
    }

    pub fn binding(protocol: &str, host: &str, port: u16) -> IisBindingDetails {
        IisBindingDetails::parse(protocol, &format!("*:{port}:{host}"), false)
    }

    pub fn pool(name: &str) -> IisPoolDetails {
        IisPoolDetails {
            name: name.into(),
            state: None,
            runtime: "v4.0".into(),
            pipeline: "Integrated".into(),
            identity_type: "ApplicationPoolIdentity".into(),
            account: String::new(),
            start_mode: "OnDemand".into(),
            idle_timeout: Duration::from_secs(20 * 60),
            idle_timeout_action: None,
            recycle_interval: Duration::ZERO,
            recycle_at: vec![],
            private_memory_limit_kb: 0,
            virtual_memory_limit_kb: 0,
            queue_length: 1000,
            enable_32bit: false,
            rapid_fail_protection: true,
            rapid_fail_max_crashes: 5,
            rapid_fail_interval: Duration::from_secs(300),
            max_processes: 1,
            load_user_profile: true,
            worker_process_ids: vec![],
        }
    }

    pub fn details(name: &str, state: &str, bindings: Vec<IisBindingDetails>) -> IisSiteDetails {
        IisSiteDetails {
            id: 1,
            name: name.into(),
            state: state.into(),
            physical_path: PathBuf::from(format!(r"C:\DNN\{name}")),
            config_path: String::new(),
            server_auto_start: true,
            preload_enabled: None,
            bindings,
            pool: Some(pool(name)),
        }
    }

    pub fn sql_database(name: &str, windows: bool) -> DatabaseConnection {
        DatabaseConnection {
            kind: DatabaseKind::SqlServer,
            server: "localhost".into(),
            database: name.into(),
            authentication: if windows { SqlAuthentication::Windows } else { SqlAuthentication::Sql },
            user: if windows { String::new() } else { "sa".into() },
            password: if windows { String::new() } else { "secret".into() },
        }
    }

    /// A stand-in for the databases; `fail` names the calls that fail ("rename", "write", "grant", "aliases") and
    /// "cancel-aliases" makes the alias update end as cancelled.
    pub struct Databases {
        pub log: Log,
        pub database: Option<DatabaseConnection>,
        pub fail: Vec<&'static str>,
        pub may_sign_in: bool,
    }

    impl Databases {
        pub fn new(log: &Log, database: Option<DatabaseConnection>) -> Self {
            Self { log: log.clone(), database, fail: vec![], may_sign_in: true }
        }

        fn call(&self, kind: &str, line: String) -> Result<(), String> {
            push(&self.log, line);
            if self.fail.contains(&kind) { Err(format!("{kind} failed")) } else { Ok(()) }
        }
    }

    impl SiteDatabases for Databases {
        fn database_of(&self, _: &Path) -> Option<DatabaseConnection> {
            self.database.clone()
        }
        fn rename(&self, database: &DatabaseConnection, new_name: &str, _: &CancelToken) -> Result<(), String> {
            self.call("rename", format!("rename db {} -> {new_name}", database.database))
        }
        fn write_connection(&self, _: &Path, database: &DatabaseConnection) -> Result<(), String> {
            self.call("write", format!("web.config -> {}", database.database))
        }
        fn grant_site_access(&self, database: &DatabaseConnection, login: &str, _: &CancelToken) -> Result<(), String> {
            self.call("grant", format!("login {login} on {}", database.database))
        }
        fn update_portal_aliases(
            &self,
            database: &DatabaseConnection,
            _: &Path,
            renamed: &[(String, String)],
            added: &[String],
            _: &CancelToken,
        ) -> Result<i64, String> {
            let list: Vec<String> = renamed.iter().map(|(f, t)| format!("{f}=>{t}")).collect();
            push(&self.log, format!("aliases on {} {} +{}", database.database, list.join(","), added.join(",")));
            if self.fail.contains(&"cancel-aliases") {
                return Err(CANCELLED.into());
            }
            if self.fail.contains(&"aliases") {
                return Err("no PortalAlias table".into());
            }
            Ok((renamed.len() + added.len()) as i64)
        }
        fn may_sign_in_as_user(&self, _: &str) -> bool {
            self.may_sign_in
        }
        fn sign_in_question(&self, server: &str, why: &str) -> String {
            format!("{why} on {server}?")
        }
    }

    /// The projects folder is a temporary folder: the undo of web.config acts on real paths.
    struct Folders {
        log: Log,
        base: PathBuf,
        existing: Vec<PathBuf>,
        fail_move: bool,
    }

    impl ProjectFolders for Folders {
        fn project_directory(&self, name: &str) -> PathBuf {
            self.base.join(name)
        }
        fn exists(&self, folder: &Path) -> bool {
            self.existing.iter().any(|e| same_path(e, folder))
        }
        fn move_folder(&self, from: &Path, to: &Path) -> Result<(), String> {
            push(&self.log, format!("move {} -> {}", from.display(), to.display()));
            if self.fail_move { Err("The process cannot access the file.".into()) } else { Ok(()) }
        }
    }

    struct Records(Log, Mutex<Vec<ProjectRecord>>);

    impl ProjectRecords for Records {
        fn find(&self, site: &str) -> Option<ProjectRecord> {
            self.1.lock().unwrap().iter().find(|r| r.site.eq_ignore_ascii_case(site)).cloned()
        }
        fn save(&self, record: &ProjectRecord) {
            push(&self.0, format!("record save {}", record.site));
            let mut all = self.1.lock().unwrap();
            all.retain(|r| r.site != record.site);
            all.push(record.clone());
        }
        fn remove(&self, site: &str) {
            push(&self.0, format!("record remove {site}"));
            self.1.lock().unwrap().retain(|r| r.site != site);
        }
    }

    struct Warm(Log, Mutex<Vec<String>>);

    impl KeepWarmTable for Warm {
        fn find(&self, site: &str) -> Option<String> {
            self.1.lock().unwrap().iter().find(|s| s.eq_ignore_ascii_case(site)).cloned()
        }
        fn save(&self, site: &str) {
            push(&self.0, format!("warm save {site}"));
            self.1.lock().unwrap().push(site.into());
        }
        fn remove(&self, site: &str) {
            push(&self.0, format!("warm remove {site}"));
            self.1.lock().unwrap().retain(|s| s != site);
        }
    }

    struct Setup {
        _dir: tempfile::TempDir,
        base: PathBuf,
        log: Log,
        iis: Iis,
        databases: Databases,
        folders: Folders,
    }

    impl Setup {
        fn new(state: &str, database: Option<DatabaseConnection>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let base = full(dir.path());
            let log: Log = Arc::default();
            let bindings = vec![
                binding("http", "shop.dnndev.me", 80),
                binding("http", "flip.shop.dnndev.me", 80),
                binding("http", "", 8080),
                binding("https", "shop.dnndev.me", 443),
            ];
            Self {
                iis: Iis::new(&log, Some(details("shop", state, bindings))),
                databases: Databases::new(&log, database),
                folders: Folders { log: log.clone(), base: base.clone(), existing: vec![], fail_move: false },
                log,
                base,
                _dir: dir,
            }
        }

        /// A path in the projects folder, as the log writes it.
        fn at(&self, name: &str) -> String {
            self.base.join(name).display().to_string()
        }

        fn request(&self, new_name: &str, host: bool, database: bool) -> RenameProjectRequest {
            RenameProjectRequest {
                site_name: "shop".into(),
                directory: PathBuf::from(format!("{}\\", self.at("shop"))),
                new_name: new_name.into(),
                rename_host: host,
                rename_database: database,
            }
        }

        fn deps(&mut self) -> RenameDeps {
            let record = ProjectRecord {
                site: "shop".into(),
                install_mode: InstallMode::Automatic,
                created_utc: "2026-11-13T14:44:35.5371802Z".into(),
                dnn_version: None,
                host_user_name: None,
            };
            let log = self.log.clone();
            let iis = std::mem::replace(&mut self.iis, Iis::new(&log, None));
            let databases = std::mem::replace(&mut self.databases, Databases::new(&log, None));
            let folders = std::mem::replace(
                &mut self.folders,
                Folders { log: log.clone(), base: self.base.clone(), existing: vec![], fail_move: false },
            );
            RenameDeps {
                iis: Arc::new(iis),
                folders: Arc::new(folders),
                databases: Arc::new(databases),
                records: Arc::new(Records(log.clone(), Mutex::new(vec![record]))),
                keep_warm: Arc::new(Warm(log, Mutex::new(vec!["shop".into()]))),
                hostname_suffix: "dnndev.me".into(),
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    fn rename(deps: &RenameDeps, req: &RenameProjectRequest) -> (Outcome, Vec<String>) {
        let mut lines = Lines::default();
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            rename_project(deps, req, op)
        });
        (outcome, lines.0)
    }

    pub fn has(lines: &[String], line: &str) -> bool {
        lines.iter().any(|l| l == line)
    }

    #[test]
    fn renames_site_folder_host_database_and_records_and_starts_it_again() {
        let mut s = Setup::new("Started", Some(sql_database("shop", true)));
        let deps = s.deps();
        let (outcome, lines) = rename(&deps, &s.request(" store ", true, true));
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        let (shop, store) = (s.at("shop"), s.at("store"));
        assert_eq!(
            s.log(),
            vec![
                format!("rename shop -> store at {store}"),
                format!("move {shop} -> {store}"),
                format!(r"grant IIS APPPOOL\store {store}"),
                "bindings store store.dnndev.me:80,flip.store.dnndev.me:80,:8080".into(),
                "aliases on shop shop.dnndev.me=>store.dnndev.me,flip.shop.dnndev.me=>flip.store.dnndev.me +".into(),
                "rename db shop -> store".into(),
                "web.config -> store".into(),
                r"login IIS APPPOOL\store on store".into(),
                "record save store".into(),
                "record remove shop".into(),
                "warm save store".into(),
                "warm remove shop".into(),
                "start store".into(),
                "remove profile shop".into(),
            ]
        );
        assert!(has(&lines, "ok IIS site and app pool are 'store' (stopped for the rename)."));
        assert!(has(&lines, "ok Bound to store.dnndev.me instead of shop.dnndev.me."));
        assert!(has(&lines, "ok DNN's portal alias is store.dnndev.me (2 changed)."));
        assert!(has(&lines, "ok Database [shop] is [store]; web.config points at it."));
        assert!(has(&lines, r"ok IIS APPPOOL\store may use [store]."));
        assert!(has(&lines, "ok How it was installed and its keep-warm choice go with the new name."));
        assert!(has(&lines, "## Starting 'store'") && has(&lines, "ok 'store' runs."));
    }

    #[test]
    fn a_failed_step_takes_back_the_ones_before_it_last_first() {
        let mut s = Setup::new("Started", Some(sql_database("shop", false)));
        s.databases.fail = vec!["write"];
        let deps = s.deps();
        let (outcome, lines) = rename(&deps, &s.request("store", true, true));
        assert_eq!(
            outcome,
            Outcome::Failed("The database is [store], but web.config couldn't be updated: write failed".into())
        );
        let log = s.log();
        let undo = &log[log.iter().position(|l| l == "web.config -> store").unwrap() + 1..];
        assert_eq!(
            undo,
            [
                "rename db store -> shop".to_string(),
                "aliases on shop store.dnndev.me=>shop.dnndev.me,flip.store.dnndev.me=>flip.shop.dnndev.me +".into(),
                "bindings store shop.dnndev.me:80,flip.shop.dnndev.me:80,:8080".into(),
                format!("move {} -> {}", s.at("store"), s.at("shop")),
                format!("rename store -> shop at {}", s.at("shop")),
            ]
        );
        assert!(!log.iter().any(|l| l.starts_with("start") || l.starts_with("record")));
        // web.config wasn't there before: its undo deletes it, after the database is renamed back.
        let undo_lines = &lines[lines.iter().position(|l| l == "## Undo").unwrap() + 1..];
        assert_eq!(undo_lines[0], "ok Rename the database back to [shop]");
        assert!(undo_lines[1].starts_with("ok Delete ") && undo_lines[1].ends_with("web.config"));
        assert_eq!(undo_lines[2], "ok Put DNN's portal aliases back");
    }

    #[test]
    fn a_folder_that_cannot_move_puts_the_site_back() {
        let mut s = Setup::new("Stopped", None);
        s.folders.fail_move = true;
        let deps = s.deps();
        let (outcome, _) = rename(&deps, &s.request("store", false, false));
        let (shop, store) = (s.at("shop"), s.at("store"));
        assert_eq!(
            outcome,
            Outcome::Failed(format!(
                "Could not rename {shop} to {store}: The process cannot access the file. Close what has files of the project open (an editor, a terminal) and try again."
            ))
        );
        assert_eq!(
            s.log(),
            vec![
                format!("rename shop -> store at {store}"),
                format!("move {shop} -> {store}"),
                format!("move {store} -> {shop}"),
                format!("rename store -> shop at {shop}"),
            ]
        );
    }

    #[test]
    fn a_failed_iis_rename_is_said_so() {
        let mut s = Setup::new("Started", None);
        s.iis.fail = vec!["rename"];
        let deps = s.deps();
        let (outcome, _) = rename(&deps, &s.request("store", false, false));
        assert_eq!(outcome, Outcome::Failed("Could not rename the IIS site: rename refused".into()));
    }

    #[test]
    fn refuses_bad_taken_and_missing_names_and_ignores_the_same_name() {
        let mut s = Setup::new("Started", None);
        s.iis.sites = vec!["shop".into(), "Store".into()];
        let deps = s.deps();
        let failed = |name: &str| rename(&deps, &s.request(name, false, false)).0;
        assert_eq!(failed("store"), Outcome::Failed("IIS already has a site named 'store'.".into()));
        assert_eq!(failed("con"), Outcome::Failed("'con' uses the reserved Windows device name 'con'.".into()));
        assert_eq!(failed("shop"), Outcome::Finished);
        assert!(s.log().is_empty());

        let mut s = Setup::new("Started", None);
        *s.iis.details.lock().unwrap() = None;
        let deps = s.deps();
        assert_eq!(
            rename(&deps, &s.request("other", false, false)).0,
            Outcome::Failed("IIS has no site named 'shop'.".into())
        );
    }

    #[test]
    fn refuses_a_folder_that_is_there_already() {
        let mut s = Setup::new("Started", None);
        s.folders.existing = vec![PathBuf::from(s.at("store"))];
        let deps = s.deps();
        assert_eq!(
            rename(&deps, &s.request("store", false, false)).0,
            Outcome::Failed(format!("The folder {} exists already - choose another name.", s.at("store")))
        );
        assert!(s.log().is_empty());
    }

    #[test]
    fn only_capitals_and_a_folder_elsewhere_keep_the_folder() {
        let mut s = Setup::new("Stopped", Some(sql_database("shop", false)));
        let deps = s.deps();
        let (outcome, lines) = rename(&deps, &s.request("Shop", false, true));
        assert_eq!(outcome, Outcome::Finished);
        assert!(has(&lines, "The folder keeps its name - only its capitals differ."));
        // The database is named like it already, ignoring case: it isn't renamed.
        assert!(has(&lines, "Kept as it is: [shop]."));
        assert!(has(&lines, "## Start") && has(&lines, "It was stopped - it stays stopped."));
        let log = s.log();
        assert!(!log.iter().any(|l| l.starts_with("move") || l.starts_with("rename db") || l.starts_with("start")));
        // The records follow the new capitals without being removed under the old name.
        assert!(log.contains(&"record save Shop".to_string()) && !log.contains(&"record remove shop".to_string()));

        let mut s = Setup::new("Stopped", None);
        let deps = s.deps();
        let elsewhere = s.base.join("work").join("shop");
        let req = RenameProjectRequest { directory: elsewhere.clone(), ..s.request("store", false, false) };
        let (_, lines) = rename(&deps, &req);
        assert!(has(&lines, &format!("The folder stays where it is: {}", elsewhere.display())));
        assert_eq!(s.log()[0], format!("rename shop -> store at {}", elsewhere.display()));
        assert!(has(&lines, "web.config names no database of its own."));
    }

    #[test]
    fn host_name_without_a_binding_or_database_and_failing_aliases() {
        let mut s = Setup::new("Stopped", None);
        let deps = s.deps();
        let (_, lines) = rename(&deps, &s.request("store", true, false));
        assert!(has(&lines, "warn web.config names no database - DNN's portal alias shop.dnndev.me stays as it is."));

        let mut s = Setup::new("Stopped", Some(sql_database("other", false)));
        s.databases.fail = vec!["aliases"];
        let deps = s.deps();
        let (outcome, lines) = rename(&deps, &s.request("store", true, false));
        assert_eq!(outcome, Outcome::Finished);
        assert!(has(
            &lines,
            "warn no PortalAlias table - add store.dnndev.me as a site alias in DNN (Settings → Site Settings → Site Aliases)."
        ));

        let mut s = Setup::new("Stopped", None);
        *s.iis.details.lock().unwrap() = Some(details("shop", "Stopped", vec![binding("http", "", 80)]));
        let deps = s.deps();
        let (_, lines) = rename(&deps, &s.request("store", true, false));
        assert!(has(&lines, "No binding on shop.dnndev.me - nothing to change."));
    }

    #[test]
    fn a_cancelled_alias_update_ends_the_rename_and_undoes_it() {
        let mut s = Setup::new("Started", Some(sql_database("shop", false)));
        s.databases.fail = vec!["cancel-aliases"];
        let deps = s.deps();
        let (outcome, _) = rename(&deps, &s.request("store", true, true));
        assert_eq!(outcome, Outcome::Cancelled);
        let log = s.log();
        assert!(log.last().unwrap().starts_with("rename store -> shop"), "{log:#?}");
        assert!(!log.iter().any(|l| l.starts_with("rename db")));
    }

    #[test]
    fn a_failed_start_or_login_is_a_warning() {
        let mut s = Setup::new("Started", Some(sql_database("shop", true)));
        s.iis.fail = vec!["start", "grant"];
        s.databases.fail = vec!["grant"];
        let deps = s.deps();
        let (outcome, lines) = rename(&deps, &s.request("store", false, false));
        assert_eq!(outcome, Outcome::Finished);
        assert!(has(
            &lines,
            &format!(r"warn Could not give IIS APPPOOL\store access to {}: grant refused", s.at("store"))
        ));
        assert!(has(&lines, "warn grant failed"));
        assert!(has(&lines, "warn 'store' couldn't be started: start refused"));
    }

    #[test]
    fn dialog_facts_and_name_problems() {
        let bindings = vec![IisBinding::parse("http", "*:80:Shop.dnndev.me", false)];
        let facts = RenameDialogFacts::new(
            "shop",
            vec!["shop".into()],
            &bindings,
            "dnndev.me",
            Some(&sql_database("SHOP", true)),
        );
        assert_eq!(facts.current_host.as_deref(), Some("shop.dnndev.me"));
        assert_eq!(facts.database.as_deref(), Some("SHOP"));
        let file = DatabaseConnection { kind: DatabaseKind::LocalDbFile, ..sql_database("shop", true) };
        let facts = RenameDialogFacts::new("shop", vec![], &[], "dnndev.me", Some(&file));
        assert_eq!((facts.current_host, facts.database), (None, None));
        let other = RenameDialogFacts::new("shop", vec![], &[], "x", Some(&sql_database("dnn", true)));
        assert_eq!(other.database, None);

        let sites = vec!["shop".to_string(), "Store".to_string()];
        assert_eq!(name_problem("store", "shop", &sites).as_deref(), Some("IIS already has a site named 'store'."));
        assert_eq!(name_problem("SHOP", "shop", &sites), None);
        assert_eq!(name_problem("new", "shop", &sites), None);
        assert!(
            name_problem("a b", "shop", &sites)
                .unwrap()
                .starts_with("Project name contains an unsupported character ' '.")
        );
    }

    #[test]
    fn the_password_stays_out_of_debug() {
        assert!(!format!("{:?}", sql_database("shop", false)).contains("secret"));
    }
}
