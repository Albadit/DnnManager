//! What Clone, Host existing and Import share, and the traits they see SQL Server, web.config, files and the PC
//! through: `LocalSqlContainer` (`UseCases/Provisioning.cs`), `SiteDatabases.cs`, `LocalDatabaseSwap`
//! (`CloneProjectUseCase.cs`), `HostExistingProjectUseCase.DisableSslAsync`, `IisHostNames.cs`, the names of
//! `ProjectBackups.cs` and the addresses of `AppOptions.cs`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dnnm_domain::{DatabaseConfig, DnnProject};

use crate::iis::{IisManager, IisSiteRuntime};
use crate::operation::{CANCELLED, CancelToken, Operation, Reporter, STEP_LIMIT};

/// A server, database and login as a site's web.config names them (`SiteSqlConnection`) - an empty user is Windows
/// authentication. The password is only held to sign in or to write web.config: `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SiteSql {
    pub server: String,
    pub database: String,
    pub user: String,
    pub password: String,
}

impl SiteSql {
    pub fn new(server: &str, database: &str, user: &str, password: &str) -> Self {
        Self { server: server.into(), database: database.into(), user: user.into(), password: password.into() }
    }
}

impl fmt::Debug for SiteSql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiteSql")
            .field("server", &self.server)
            .field("database", &self.database)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// The settings these use cases read (`AppOptions`): where projects and their backups go, the address a site
/// answers on, and the local SQL Server container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingSettings {
    /// Settings → Projects → folder (`C:\DNN`).
    pub base_directory: PathBuf,
    /// `Documents\DnnManager\backups`.
    pub backups_directory: PathBuf,
    pub hostname_suffix: String,
    pub site_port: u16,
    /// The host DNN Manager reaches the container on (`localhost`).
    pub container_ip: String,
    pub container_port: u16,
    /// The login DNN Manager signs in to the container with (sa by default).
    pub sql_user: String,
    pub collation: String,
}

impl HostingSettings {
    /// `IProjectRepository.Build`: the project's folder and its backups folder.
    pub fn project(&self, name: &str) -> DnnProject {
        DnnProject {
            name: name.to_string(),
            project_directory: self.base_directory.join(name),
            backup_directory: self.backups_directory.join(name),
        }
    }

    /// `{project}.{hostname suffix}`.
    pub fn hostname_for(&self, project: &str) -> String {
        format!("{project}.{}", self.hostname_suffix)
    }

    /// The URL a project's site answers on, with the port when it isn't 80.
    pub fn site_url_for(&self, project: &str) -> String {
        if self.site_port == 80 {
            format!("http://{}", self.hostname_for(project))
        } else {
            format!("http://{}:{}", self.hostname_for(project), self.site_port)
        }
    }

    /// The database a project gets: named like the project.
    pub fn database_name_for(&self, project: &str) -> String {
        project.to_string()
    }

    /// The container's address (`ip,port`) for a published port.
    pub fn server_for(&self, port: u16) -> String {
        format!("{},{port}", self.container_ip)
    }

    /// The local SQL Server's address from the settings.
    pub fn server(&self) -> String {
        self.server_for(self.container_port)
    }

    /// A database in the shared container for `project` (`LocalSqlContainer.DatabaseFor`).
    pub fn database_for(&self, project: &DnnProject, name: &str, port: u16) -> DatabaseConfig {
        DatabaseConfig {
            server: self.server_for(port),
            database_name: name.to_string(),
            collation: self.collation.clone(),
            port,
            backup_directory: project.backup_directory.clone(),
        }
    }
}

/// The HTTPS redirect rules of a web.config (`HttpsRedirectRules`): switched off now, and switched off before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpsRedirectRules {
    pub switched_off: Vec<String>,
    pub already_off: Vec<String>,
}

impl HttpsRedirectRules {
    pub fn all(&self) -> Vec<String> {
        self.switched_off.iter().chain(&self.already_off).cloned().collect()
    }
}

/// The local SQL Server container, signed in to as the settings' user (`ISqlServerService` and the parts of
/// `LocalSqlContainer` that reach the server). A cancel ends a call with `Err("Cancelled.")`.
pub trait LocalSql: Send + Sync {
    /// Signs in to [master] (5 s to connect) and describes the server, e.g. "Microsoft SQL Server 2022 …".
    fn test_login(&self, cancel: &CancelToken) -> Result<String, String>;
    /// `server` names this PC's container publishing `port`, however it is written.
    fn is_local_container(&self, server: &str, port: u16) -> bool;
    fn database_exists(&self, database: &str, cancel: &CancelToken) -> Result<bool, String>;
    fn create_database(&self, database: &DatabaseConfig, cancel: &CancelToken) -> Result<(), String>;
    fn drop_database(&self, database: &str, cancel: &CancelToken) -> Result<(), String>;
    /// Renames the database, closing the connections still open to it.
    fn rename_database(&self, database: &str, new_name: &str, cancel: &CancelToken) -> Result<(), String>;
    /// BACKUP DATABASE in the container, copied out to DNN Manager's private temporary folder: that copy's path.
    fn backup_local(&self, database: &str, file_name: &str, cancel: &CancelToken) -> Result<PathBuf, String>;
    /// RESTORE DATABASE of a .bak into `database`.
    fn restore_local(&self, database: &DatabaseConfig, file: &Path, cancel: &CancelToken) -> Result<(), String>;
    fn remap_portal_aliases(
        &self,
        database: &str,
        hostname_suffix: &str,
        alias: &str,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// Turns DNN's SSL off; how many values changed.
    fn disable_ssl(&self, database: &str, cancel: &CancelToken) -> Result<i64, String>;
    /// The login `login`, owner of `database` only, with a new password (`SqlPasswords.New`) - that password.
    fn grant_site_login(&self, database: &str, login: &str, cancel: &CancelToken) -> Result<String, String>;
    fn drop_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String>;
}

/// Copies of a database made elsewhere: SqlPackage's .bacpac (`IBacpacService`) and BACKUP DATABASE on another
/// server (`IRemoteSqlBackupService`).
pub trait DatabaseCopies: Send + Sync {
    /// Installs SqlPackage on first use.
    fn ensure_sqlpackage(&self, report: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String>;
    fn export_bacpac(
        &self,
        source: &SiteSql,
        file: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// Imports into `database` on the container `server`, as the container's user.
    fn import_bacpac(
        &self,
        server: &str,
        database: &str,
        file: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// BACKUP DATABASE of `source` to `server_path`; the path this PC reads the .bak at.
    fn backup_remote(
        &self,
        source: &SiteSql,
        server_path: &str,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<String, String>;
}

/// A site's web.config (`IWebConfigService`). Each writes the file whole or not at all.
pub trait SiteWebConfig: Send + Sync {
    /// SiteSqlServer with a server and a database - an error says why there is none.
    fn read_site_sql_server(&self, web_config: &Path) -> Result<SiteSql, String>;
    /// SiteSqlServer (connectionStrings and appSettings) pointed at `connection`.
    fn write_site_sql_server(&self, web_config: &Path, connection: &SiteSql) -> Result<(), String>;
    /// Removes system.webServer/rewrite.
    fn remove_rewrite_rules(&self, web_config: &Path) -> Result<(), String>;
    /// Switches off every enabled rule that redirects to https://; none when there is no web.config.
    fn disable_https_redirect_rules(&self, web_config: &Path) -> Result<HttpsRedirectRules, String>;
    /// SiteSqlServer's `localhost,<port>` as `127.0.0.1,<port>`; true when it was changed.
    fn use_loopback_address(&self, web_config: &Path) -> Result<bool, String>;
}

/// Which IIS sites use a database or a login, as their web.config files have it (`SiteDatabases`).
pub trait SiteDatabases: Send + Sync {
    /// The site other than `site` whose web.config names `database` on `server`.
    fn other_site_using(&self, site: &str, server: &str, database: &str) -> Option<String>;
    /// The site other than `site` whose web.config signs in to `server` as `login`.
    fn other_site_signing_in_as(&self, site: &str, server: &str, login: &str) -> Option<String>;
    /// The sites whose web.config reaches `server` as `localhost,<port>`, with that web.config.
    fn reaching_as_localhost(&self, server: &str) -> Vec<(String, PathBuf)>;
}

/// A project's files (`IProjectFileCopier`, `IProjectScaffolder`, `IPrivateTemp`).
pub trait ProjectFiles: Send + Sync {
    fn copy_site(
        &self,
        source: &Path,
        target: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    fn extract_zip(
        &self,
        zip: &Path,
        target: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// Writes DNN Manager's .gitignore unless the folder has one.
    fn ensure_gitignore(&self, directory: &Path) -> Result<(), String>;
    /// A folder only administrators can write.
    fn private_temp(&self) -> Result<PathBuf, String>;
}

/// The PC (`IPrerequisiteChecker`, `IHttpConnectivityChecker`, the clock).
pub trait Machine: Send + Sync {
    /// Checks the IIS features and offers to enable the missing ones.
    fn ensure_iis_features(&self, op: &mut Operation) -> Result<(), String>;
    /// GET `url`: its status code, or why it didn't answer.
    fn check_http(&self, url: &str, timeout_seconds: u64, cancel: &CancelToken) -> Result<u16, String>;
    /// Now in local time as `yyyyMMdd_HHmmss`.
    fn stamp(&self) -> String;
}

/// Everything Clone, Host existing and Import act on.
#[derive(Clone)]
pub struct HostingDeps {
    pub settings: HostingSettings,
    pub iis: Arc<dyn IisManager>,
    pub sql: Arc<dyn LocalSql>,
    pub copies: Arc<dyn DatabaseCopies>,
    pub web_config: Arc<dyn SiteWebConfig>,
    pub sites: Arc<dyn SiteDatabases>,
    pub files: Arc<dyn ProjectFiles>,
    pub machine: Arc<dyn Machine>,
}

/// `error` worded by `wording` - unless it is a cancel, which stays one.
pub(crate) fn worded(error: String, wording: impl FnOnce(String) -> String) -> String {
    if error == CANCELLED { error } else { wording(error) }
}

/// A new backup folder for a backup taken at `stamp` (`ProjectBackups.NewFolder`).
pub fn backup_folder(project: &DnnProject, stamp: &str) -> PathBuf {
    project.backup_directory.join(format!("{}_{stamp}", project.name))
}

/// A database file's name inside a backup folder, for ".bacpac" or ".bak" (`ProjectBackups.DatabaseName`).
pub fn database_file_name(project: &DnnProject, extension: &str) -> String {
    format!("{}{extension}", project.name)
}

/// A file a database can be restored from: a .bacpac, or a .bak that isn't a hand-made copy like web.config.bak
/// (`LocalSqlContainer.IsBackupFile`).
pub fn is_backup_file(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".bacpac") {
        return true;
    }
    let Some(stem) = name.strip_suffix(".bak") else { return false };
    let inner = Path::new(stem).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    !["config", "json", "xml", "txt", "resources", "resx", "js", "css", "aspx", "ascx", "cs", "dll"]
        .contains(&inner.as_str())
}

fn is_bacpac(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bacpac"))
}

/// The login a project's site signs in to the container with (`LocalSqlContainer.SiteLoginFor`).
pub fn site_login_for(project: &str) -> String {
    format!("dnn_{project}")
}

/// The site among `sites` (but `replacing`) that already has an http binding for `host` on `port`
/// (`IisHostNames.SiteUsing`) - IIS wouldn't start a second one.
pub fn site_using(
    sites: &BTreeMap<String, IisSiteRuntime>,
    host: &str,
    port: u16,
    replacing: Option<&str>,
) -> Option<String> {
    let name = host.trim().trim_end_matches('.');
    if name.is_empty() {
        return None;
    }
    sites
        .iter()
        .filter(|(site, _)| !replacing.is_some_and(|r| r.eq_ignore_ascii_case(site)))
        .find(|(_, site)| {
            site.bindings.iter().any(|b| {
                b.protocol.eq_ignore_ascii_case("http")
                    && b.port == Some(port)
                    && b.host.trim().trim_end_matches('.').eq_ignore_ascii_case(name)
            })
        })
        .map(|(site, _)| site.clone())
}

/// Checks that the local SQL Server takes the settings' login, saying so, and returns its port
/// (`LocalSqlContainer.CheckAsync`).
pub fn check_local_sql(deps: &HostingDeps, op: &mut Operation) -> Result<u16, String> {
    let server = deps.settings.server();
    match deps.sql.test_login(&op.cancel) {
        Ok(version) => {
            op.report.success(&format!("Connected to SQL Server at {server} ({version})."));
            Ok(deps.settings.container_port)
        }
        Err(e) if e == CANCELLED => Err(e),
        Err(e) => {
            op.report.fail(&format!("Cannot connect to SQL Server at {server}: {e}"));
            Err(format!(
                "SQL Server at {server} is not reachable - start the SQL Server container (Settings → Docker container → \
                 Set up docker-compose) and check Settings → Database server."
            ))
        }
    }
}

/// The project's own login on the container, owner of `database`, with a new password - how the site signs in with
/// it (`LocalSqlContainer.GrantSiteLoginAsync`). `dnn_shop_2`, … when another site signs in as `dnn_shop`.
pub fn grant_site_login(
    deps: &HostingDeps,
    project: &str,
    database: &DatabaseConfig,
    cancel: &CancelToken,
) -> Result<SiteSql, String> {
    let server = deps.settings.server();
    let mut login = site_login_for(project);
    let mut n = 2;
    while deps.sites.other_site_signing_in_as(project, &server, &login).is_some() {
        login = format!("{}_{n}", site_login_for(project));
        n += 1;
    }
    let password = deps
        .sql
        .grant_site_login(&database.database_name, &login, cancel)
        .map_err(|e| worded(e, |e| format!("Could not make the site's login {login}: {e}")))?;
    Ok(SiteSql::new(&database.server, &database.database_name, &login, &password))
}

/// Notes the undo that drops the login `login` again.
pub(crate) fn drop_login_on_undo(deps: &HostingDeps, login: &str, op: &mut Operation) {
    let (sql, login) = (Arc::clone(&deps.sql), login.to_string());
    op.undo.add(format!("Drop the login {login}"), STEP_LIMIT, move |cancel| sql.drop_login(&login, cancel));
}

/// Notes the undo that drops the database `database` again.
pub(crate) fn drop_database_on_undo(deps: &HostingDeps, database: &str, op: &mut Operation) {
    let (sql, database) = (Arc::clone(&deps.sql), database.to_string());
    op.undo.add(format!("Drop database [{database}]"), STEP_LIMIT, move |cancel| sql.drop_database(&database, cancel));
}

/// Restores `file` into `database`, replacing one of that name: a .bak with RESTORE, a .bacpac with SqlPackage
/// (installed on first use) - `LocalSqlContainer.RestoreAsync`.
pub fn restore_backup(
    deps: &HostingDeps,
    database: &DatabaseConfig,
    file: &Path,
    op: &mut Operation,
) -> Result<(), String> {
    if !is_bacpac(file) {
        return deps.sql.restore_local(database, file, &op.cancel);
    }
    deps.copies.ensure_sqlpackage(op.report, &op.cancel)?;
    // SqlPackage's import always makes a new database.
    if let Ok(true) = deps.sql.database_exists(&database.database_name, &op.cancel) {
        deps.sql.drop_database(&database.database_name, &op.cancel)?;
    }
    deps.copies.import_bacpac(&database.server, &database.database_name, file, op.report, &op.cancel)
}

/// Turns DNN's SSL off in a copy of a live site's database (`HostExistingProjectUseCase.DisableSslAsync`); not fatal.
pub fn disable_ssl(deps: &HostingDeps, database: &str, op: &mut Operation) {
    match deps.sql.disable_ssl(database, &op.cancel) {
        Err(e) => op.report.fail(&format!(
            "Could not turn off DNN's SSL setting: {e}. If the site redirects to https://, switch SSL off in its Site Settings."
        )),
        Ok(changed) if changed > 0 => op.report.warn(&format!(
            "Turned off DNN's SSL in [{database}] ({changed} setting{} and secure pages) - the local site has no https. \
             Switch it back on before this database goes live again.",
            if changed == 1 { "" } else { "s" }
        )),
        Ok(_) => {}
    }
}

/// `incoming` in place of `name` (`LocalDatabaseSwap.SwapInAsync`): that one renamed to `aside`, the new one given its
/// name, the one aside dropped. A rename that fails puts the old one back. `what` names the new one ("the copy").
pub fn swap_in(
    deps: &HostingDeps,
    name: &str,
    incoming: &str,
    aside: &str,
    what: &str,
    op: &mut Operation,
) -> Result<(), String> {
    deps.sql.rename_database(name, aside, &op.cancel).map_err(|e| {
        worded(e, |e| format!("Could not set [{name}] aside to put {what} in: {e} The database is left as it was."))
    })?;
    if let Err(e) = deps.sql.rename_database(incoming, name, &op.cancel) {
        let what = capital(what);
        return Err(match deps.sql.rename_database(aside, name, &CancelToken::new()) {
            Ok(()) => format!("{what} couldn't be named [{name}]: {e} The database is left as it was."),
            Err(back) => format!(
                "{what} couldn't be named [{name}]: {e} - and the database it replaces couldn't be put back from [{aside}]: {back}"
            ),
        });
    }
    op.undo.cannot_undo(format!("database [{name}] that was there before was replaced by {what}, as you chose."));
    if let Err(e) = deps.sql.drop_database(aside, &CancelToken::new()) {
        op.report.warn(&format!(
            "The database it replaced is still there as [{aside}] - drop it once you no longer need it ({e})."
        ));
    }
    op.report.success(&format!("Local database [{name}] ready - it replaced the one that was there."));
    Ok(())
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod fakes {
    //! Stand-ins for every trait, recording what they are asked in one shared journal.

    use super::*;
    use crate::use_cases::tests::FakeIis;
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Mutex;

    pub type Journal = Arc<Mutex<Vec<String>>>;

    fn note(journal: &Journal, line: impl Into<String>) {
        journal.lock().unwrap().push(line.into());
    }

    /// What fails, by the call's name ("restore_local") - its error.
    #[derive(Default)]
    pub struct Failures(pub Mutex<HashMap<String, String>>);

    impl Failures {
        fn check(&self, call: &str) -> Result<(), String> {
            match self.0.lock().unwrap().get(call) {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    pub struct FakeSql {
        pub journal: Journal,
        pub fail: Arc<Failures>,
        pub databases: Mutex<BTreeSet<String>>,
        pub ssl_changed: i64,
        /// Where a container backup is copied out to.
        pub temp: PathBuf,
    }

    impl LocalSql for FakeSql {
        fn test_login(&self, _: &CancelToken) -> Result<String, String> {
            self.fail.check("test_login")?;
            Ok("SQL Server 2022".into())
        }
        fn is_local_container(&self, server: &str, port: u16) -> bool {
            let s = server.to_lowercase();
            (s.starts_with("localhost") || s.starts_with("127.0.0.1")) && s.ends_with(&format!(",{port}"))
        }
        fn database_exists(&self, database: &str, _: &CancelToken) -> Result<bool, String> {
            self.fail.check("database_exists")?;
            Ok(self.databases.lock().unwrap().contains(database))
        }
        fn create_database(&self, database: &DatabaseConfig, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, format!("create [{}]", database.database_name));
            self.fail.check("create_database")?;
            self.databases.lock().unwrap().insert(database.database_name.clone());
            Ok(())
        }
        fn drop_database(&self, database: &str, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, format!("drop [{database}]"));
            self.databases.lock().unwrap().remove(database);
            Ok(())
        }
        fn rename_database(&self, database: &str, new_name: &str, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, format!("rename [{database}] to [{new_name}]"));
            self.fail.check(&format!("rename {database}"))?;
            let mut dbs = self.databases.lock().unwrap();
            dbs.remove(database);
            dbs.insert(new_name.into());
            Ok(())
        }
        fn backup_local(&self, database: &str, file_name: &str, _: &CancelToken) -> Result<PathBuf, String> {
            note(&self.journal, format!("backup local [{database}] {file_name}"));
            self.fail.check("backup_local")?;
            let path = self.temp.join(file_name);
            std::fs::write(&path, "bak").map_err(|e| e.to_string())?;
            Ok(path)
        }
        fn restore_local(&self, database: &DatabaseConfig, file: &Path, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, format!("restore [{}] from {}", database.database_name, file.display()));
            self.fail.check("restore_local")?;
            self.databases.lock().unwrap().insert(database.database_name.clone());
            Ok(())
        }
        fn remap_portal_aliases(
            &self,
            database: &str,
            suffix: &str,
            alias: &str,
            _: &CancelToken,
        ) -> Result<(), String> {
            note(&self.journal, format!("alias [{database}] *.{suffix} -> {alias}"));
            self.fail.check("remap_portal_aliases")
        }
        fn disable_ssl(&self, database: &str, _: &CancelToken) -> Result<i64, String> {
            note(&self.journal, format!("ssl off [{database}]"));
            self.fail.check("disable_ssl")?;
            Ok(self.ssl_changed)
        }
        fn grant_site_login(&self, database: &str, login: &str, _: &CancelToken) -> Result<String, String> {
            note(&self.journal, format!("login {login} owns [{database}]"));
            self.fail.check("grant_site_login")?;
            Ok("Pw-1".into())
        }
        fn drop_login(&self, login: &str, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, format!("drop login {login}"));
            Ok(())
        }
    }

    pub struct FakeCopies {
        pub journal: Journal,
        pub fail: Arc<Failures>,
    }

    impl DatabaseCopies for FakeCopies {
        fn ensure_sqlpackage(&self, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, "sqlpackage");
            self.fail.check("ensure_sqlpackage")
        }
        fn export_bacpac(
            &self,
            source: &SiteSql,
            file: &Path,
            _: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<(), String> {
            note(&self.journal, format!("export [{}] to {}", source.database, file.display()));
            self.fail.check("export_bacpac")?;
            std::fs::write(file, "bacpac").map_err(|e| e.to_string())
        }
        fn import_bacpac(
            &self,
            _: &str,
            database: &str,
            file: &Path,
            _: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<(), String> {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            note(&self.journal, format!("import [{database}] from {name}"));
            self.fail.check("import_bacpac")
        }
        fn backup_remote(
            &self,
            source: &SiteSql,
            path: &str,
            _: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<String, String> {
            note(&self.journal, format!("backup remote [{}] on {}", source.database, source.server));
            self.fail.check("backup_remote")?;
            std::fs::write(path, "bak").map_err(|e| e.to_string())?;
            Ok(path.to_string())
        }
    }

    pub struct FakeWebConfig {
        pub journal: Journal,
        pub fail: Arc<Failures>,
        pub connections: Mutex<HashMap<PathBuf, SiteSql>>,
        pub rules: Mutex<HttpsRedirectRules>,
    }

    impl SiteWebConfig for FakeWebConfig {
        fn read_site_sql_server(&self, web_config: &Path) -> Result<SiteSql, String> {
            self.connections.lock().unwrap().get(web_config).cloned().ok_or_else(|| {
                format!("connectionStrings/add[@name='SiteSqlServer'] not found in {}.", web_config.display())
            })
        }
        fn write_site_sql_server(&self, web_config: &Path, c: &SiteSql) -> Result<(), String> {
            note(&self.journal, format!("web.config -> [{}] on {} as {}", c.database, c.server, c.user));
            self.fail.check("write_site_sql_server")?;
            self.connections.lock().unwrap().insert(web_config.to_path_buf(), c.clone());
            Ok(())
        }
        fn remove_rewrite_rules(&self, _: &Path) -> Result<(), String> {
            note(&self.journal, "remove rewrite");
            Ok(())
        }
        fn disable_https_redirect_rules(&self, _: &Path) -> Result<HttpsRedirectRules, String> {
            note(&self.journal, "disable redirects");
            self.fail.check("disable_https_redirect_rules")?;
            Ok(self.rules.lock().unwrap().clone())
        }
        fn use_loopback_address(&self, web_config: &Path) -> Result<bool, String> {
            note(&self.journal, format!("loopback {}", web_config.display()));
            self.fail.check("use_loopback_address")?;
            Ok(true)
        }
    }

    #[derive(Default)]
    pub struct FakeSites {
        /// (site, database) pairs other sites use; (site, login) pairs they sign in as.
        pub using: Vec<(String, String)>,
        pub signing_in_as: Vec<(String, String)>,
        pub localhost: Vec<(String, PathBuf)>,
    }

    impl SiteDatabases for FakeSites {
        fn other_site_using(&self, site: &str, _: &str, database: &str) -> Option<String> {
            self.using.iter().find(|(s, d)| s != site && d.eq_ignore_ascii_case(database)).map(|(s, _)| s.clone())
        }
        fn other_site_signing_in_as(&self, site: &str, _: &str, login: &str) -> Option<String> {
            self.signing_in_as.iter().find(|(s, l)| s != site && l.eq_ignore_ascii_case(login)).map(|(s, _)| s.clone())
        }
        fn reaching_as_localhost(&self, _: &str) -> Vec<(String, PathBuf)> {
            self.localhost.clone()
        }
    }

    pub struct FakeFiles {
        pub journal: Journal,
        pub fail: Arc<Failures>,
        pub temp: PathBuf,
    }

    impl ProjectFiles for FakeFiles {
        fn copy_site(&self, source: &Path, target: &Path, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, "copy files");
            self.fail.check("copy_site")?;
            for name in ["web.config", "Default.aspx"] {
                if source.join(name).exists() {
                    std::fs::copy(source.join(name), target.join(name)).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        fn extract_zip(&self, zip: &Path, target: &Path, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            note(&self.journal, format!("extract {}", zip.file_name().unwrap().to_string_lossy()));
            std::fs::write(target.join("web.config"), "<configuration/>").map_err(|e| e.to_string())?;
            self.fail.check("extract_zip")
        }
        fn ensure_gitignore(&self, directory: &Path) -> Result<(), String> {
            note(&self.journal, "gitignore");
            std::fs::write(directory.join(".gitignore"), "x").map_err(|e| e.to_string())
        }
        fn private_temp(&self) -> Result<PathBuf, String> {
            Ok(self.temp.clone())
        }
    }

    pub struct FakeMachine {
        pub journal: Journal,
        pub http: Result<u16, String>,
    }

    impl Machine for FakeMachine {
        fn ensure_iis_features(&self, _: &mut Operation) -> Result<(), String> {
            note(&self.journal, "iis features");
            Ok(())
        }
        fn check_http(&self, url: &str, _: u64, _: &CancelToken) -> Result<u16, String> {
            note(&self.journal, format!("GET {url}"));
            self.http.clone()
        }
        fn stamp(&self) -> String {
            "20261116_101500".into()
        }
    }

    /// A world of stand-ins in a temporary folder: projects in `<tmp>\DNN`, backups in `<tmp>\backups`.
    pub struct World {
        pub dir: tempfile::TempDir,
        pub journal: Journal,
        pub fail: Arc<Failures>,
        pub iis: Arc<FakeIis>,
        pub sql: Arc<FakeSql>,
        pub web_config: Arc<FakeWebConfig>,
        pub sites: FakeSites,
        pub http: Result<u16, String>,
    }

    impl World {
        pub fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("DNN")).unwrap();
            let journal: Journal = Arc::default();
            let fail: Arc<Failures> = Arc::default();
            Self {
                sql: Arc::new(FakeSql {
                    journal: journal.clone(),
                    fail: fail.clone(),
                    databases: Mutex::default(),
                    ssl_changed: 0,
                    temp: dir.path().to_path_buf(),
                }),
                web_config: Arc::new(FakeWebConfig {
                    journal: journal.clone(),
                    fail: fail.clone(),
                    connections: Mutex::default(),
                    rules: Mutex::default(),
                }),
                iis: Arc::new(FakeIis::running()),
                sites: FakeSites::default(),
                http: Ok(200),
                dir,
                journal,
                fail,
            }
        }

        pub fn fail(&self, call: &str, error: &str) {
            self.fail.0.lock().unwrap().insert(call.into(), error.into());
        }

        pub fn has_database(&self, name: &str) {
            self.sql.databases.lock().unwrap().insert(name.into());
        }

        pub fn root(&self) -> &Path {
            self.dir.path()
        }

        /// A site folder `name` under the projects folder with a web.config naming `connection`.
        pub fn site(&self, name: &str, connection: Option<SiteSql>) -> PathBuf {
            let folder = self.root().join("DNN").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("web.config"), "<configuration/>").unwrap();
            if let Some(c) = connection {
                self.web_config.connections.lock().unwrap().insert(folder.join("web.config"), c);
            }
            folder
        }

        pub fn deps(&self) -> HostingDeps {
            let journal = self.journal.clone();
            let fail = self.fail.clone();
            HostingDeps {
                settings: HostingSettings {
                    base_directory: self.root().join("DNN"),
                    backups_directory: self.root().join("backups"),
                    hostname_suffix: "dnndev.me".into(),
                    site_port: 80,
                    container_ip: "localhost".into(),
                    container_port: 1433,
                    sql_user: "sa".into(),
                    collation: "Latin1_General_CI_AS".into(),
                },
                iis: self.iis.clone(),
                sql: self.sql.clone(),
                copies: Arc::new(FakeCopies { journal: journal.clone(), fail: fail.clone() }),
                web_config: self.web_config.clone(),
                sites: Arc::new(FakeSites {
                    using: self.sites.using.clone(),
                    signing_in_as: self.sites.signing_in_as.clone(),
                    localhost: self.sites.localhost.clone(),
                }),
                files: Arc::new(FakeFiles { journal: journal.clone(), fail, temp: self.root().to_path_buf() }),
                machine: Arc::new(FakeMachine { journal, http: self.http.clone() }),
            }
        }

        pub fn journal(&self) -> Vec<String> {
            self.journal.lock().unwrap().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::World;
    use super::*;
    use crate::iis::IisBinding;
    use crate::operation::tests::{Answer, Lines};

    fn op_lines<T>(f: impl FnOnce(&mut Operation) -> T) -> (T, Vec<String>) {
        let mut lines = Lines::default();
        let answer = Answer::yes();
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        let result = f(&mut op);
        drop(op);
        (result, lines.0)
    }

    #[test]
    fn hand_made_copies_are_not_database_backups() {
        assert!(is_backup_file(Path::new(r"C:\x\shop.bacpac")));
        assert!(is_backup_file(Path::new(r"C:\x\SHOP.BAK")));
        assert!(!is_backup_file(Path::new(r"C:\x\web.config.bak")));
        assert!(!is_backup_file(Path::new(r"C:\x\shop.zip")));
        assert!(is_backup_file(Path::new(r"C:\x\shop.2026.bak")));
    }

    #[test]
    fn addresses_follow_the_settings() {
        let world = World::new();
        let s = world.deps().settings;
        assert_eq!(s.hostname_for("shop"), "shop.dnndev.me");
        assert_eq!(s.site_url_for("shop"), "http://shop.dnndev.me");
        assert_eq!(HostingSettings { site_port: 8080, ..s.clone() }.site_url_for("shop"), "http://shop.dnndev.me:8080");
        assert_eq!(s.server(), "localhost,1433");
        let p = s.project("shop");
        assert_eq!(backup_folder(&p, "20261116_101500"), s.backups_directory.join("shop").join("shop_20261116_101500"));
        assert_eq!(database_file_name(&p, ".bak"), "shop.bak");
    }

    #[test]
    fn a_second_site_with_the_same_address_is_found() {
        let site = |host: &str, port| IisSiteRuntime {
            id: 1,
            state: "Started".into(),
            app_pool: "x".into(),
            app_pool_state: None,
            worker_process_ids: vec![],
            bindings: vec![IisBinding::parse("http", &format!("*:{port}:{host}"), false)],
            physical_path: PathBuf::new(),
        };
        let sites = BTreeMap::from([("a".to_string(), site("shop.dnndev.me.", 80)), ("b".into(), site("x", 8080))]);
        assert_eq!(site_using(&sites, "SHOP.dnndev.me", 80, None).as_deref(), Some("a"));
        assert_eq!(site_using(&sites, "shop.dnndev.me", 80, Some("A")), None);
        assert_eq!(site_using(&sites, "shop.dnndev.me", 8080, None), None);
        assert_eq!(site_using(&sites, " ", 80, None), None);
    }

    #[test]
    fn a_login_another_site_signs_in_with_is_left_to_it() {
        let mut world = World::new();
        world.sites.signing_in_as = vec![("old".into(), "dnn_shop".into()), ("old2".into(), "dnn_shop_2".into())];
        let deps = world.deps();
        let db = deps.settings.database_for(&deps.settings.project("shop"), "shop", 1433);
        let login = grant_site_login(&deps, "shop", &db, &CancelToken::new()).unwrap();
        assert_eq!(login, SiteSql::new("localhost,1433", "shop", "dnn_shop_3", "Pw-1"));
        assert!(!format!("{login:?}").contains("Pw-1"));
        world.fail("grant_site_login", "denied");
        assert_eq!(
            grant_site_login(&deps, "x", &db, &CancelToken::new()).unwrap_err(),
            "Could not make the site's login dnn_x: denied"
        );
    }

    #[test]
    fn an_unreachable_server_is_said_twice() {
        let world = World::new();
        world.fail("test_login", "timeout");
        let deps = world.deps();
        let (result, lines) = op_lines(|op| check_local_sql(&deps, op));
        assert!(
            result.unwrap_err().starts_with("SQL Server at localhost,1433 is not reachable - start the SQL Server")
        );
        assert_eq!(lines, vec!["! Cannot connect to SQL Server at localhost,1433: timeout"]);
    }

    #[test]
    fn a_swap_puts_the_new_one_in_and_drops_the_old() {
        let world = World::new();
        world.has_database("shop");
        world.has_database("shop_new");
        let deps = world.deps();
        let (result, lines) = op_lines(|op| {
            let r = swap_in(&deps, "shop", "shop_new", "shop_old", "the copy", op);
            assert_eq!(
                op.undo.pending(),
                vec![
                    "Can't be put back: database [shop] that was there before was replaced by the copy, as you chose."
                ]
            );
            r
        });
        result.unwrap();
        assert_eq!(
            world.journal(),
            vec!["rename [shop] to [shop_old]", "rename [shop_new] to [shop]", "drop [shop_old]"]
        );
        assert_eq!(lines, vec!["ok Local database [shop] ready - it replaced the one that was there."]);
    }

    #[test]
    fn a_swap_that_fails_puts_the_old_one_back() {
        let world = World::new();
        world.fail("rename shop_new", "in use");
        let deps = world.deps();
        let (result, _) = op_lines(|op| swap_in(&deps, "shop", "shop_new", "shop_old", "the copy", op));
        assert_eq!(result.unwrap_err(), "The copy couldn't be named [shop]: in use The database is left as it was.");
        assert_eq!(
            world.journal(),
            vec!["rename [shop] to [shop_old]", "rename [shop_new] to [shop]", "rename [shop_old] to [shop]"]
        );
        world.fail("rename shop_old", "gone");
        let (result, _) = op_lines(|op| swap_in(&deps, "shop", "shop_new", "shop_old", "the copy", op));
        assert!(result.unwrap_err().ends_with("couldn't be put back from [shop_old]: gone"));
        world.fail("rename shop", "locked");
        let (result, _) = op_lines(|op| swap_in(&deps, "shop", "shop_new", "shop_old", "the copy", op));
        assert_eq!(
            result.unwrap_err(),
            "Could not set [shop] aside to put the copy in: locked The database is left as it was."
        );
    }

    #[test]
    fn a_bacpac_replaces_through_sqlpackage_and_a_bak_through_restore() {
        let world = World::new();
        world.has_database("shop");
        let deps = world.deps();
        let db = deps.settings.database_for(&deps.settings.project("shop"), "shop", 1433);
        let (result, _) = op_lines(|op| restore_backup(&deps, &db, Path::new(r"C:\b\shop.bacpac"), op));
        result.unwrap();
        let (result, _) = op_lines(|op| restore_backup(&deps, &db, Path::new(r"C:\b\shop.bak"), op));
        result.unwrap();
        assert_eq!(
            world.journal(),
            vec!["sqlpackage", "drop [shop]", "import [shop] from shop.bacpac", r"restore [shop] from C:\b\shop.bak"]
        );
    }

    #[test]
    fn ssl_turned_off_is_a_warning_and_a_failure_is_not_fatal() {
        let world = World::new();
        let deps = world.deps();
        let (_, lines) = op_lines(|op| disable_ssl(&deps, "shop", op));
        assert!(lines.is_empty());
        world.fail("disable_ssl", "no table");
        let (_, lines) = op_lines(|op| disable_ssl(&deps, "shop", op));
        assert_eq!(
            lines,
            vec![
                "! Could not turn off DNN's SSL setting: no table. If the site redirects to https://, switch SSL off in its Site Settings."
            ]
        );
    }
}
