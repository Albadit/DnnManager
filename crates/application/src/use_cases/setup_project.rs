//! New project: `SetupProjectUseCase` (`src/DnnManager.Application/UseCases/SetupProjectUseCase.cs`), with the parts of
//! `LocalSqlContainer`, `AppOptions` and `IisHostNames` it uses. Downloads a DNN release into a folder of its own,
//! creates its IIS website and its database, and - with an automatic install - installs DNN, so the first visit shows
//! the new site instead of DNN's installation wizard. Everything that can be checked is checked before anything is
//! created; a failure or a cancel undoes what was made, except a DNN install that started and failed - that is kept to
//! look into.
//!
//! What it acts on - GitHub, the package, SQL Server, web.config, DNN's installer - is [`SetupSystem`], implemented in
//! `dnnm-wiring`; IIS is [`IisManager`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dnnm_domain::{DnnProject, DnnRelease, DnnReleaseList, validate_project_name};

use crate::iis::{IisManager, IisSiteRuntime};
use crate::operation::{CANCELLED, CancelToken, Operation, Reporter, STEP_LIMIT};
use crate::records::{InstallMode, ProjectRecord, utc_now_round_trip};
use crate::use_cases::provisioner;

/// The set-up's stages, by their short names in the Output tab's stage list (`SetupProjectUseCase.Stage`).
pub mod stage {
    pub const VERSION: &str = "DNN version";
    pub const IIS: &str = "Check IIS";
    pub const DATABASE: &str = "Test database";
    pub const FOLDER: &str = "Create project folder";
    pub const DOWNLOAD: &str = "Download DNN";
    pub const SITE: &str = "Create IIS site";
    pub const CREATE_DATABASE: &str = "Create database";
    pub const CONFIGURE: &str = "Configure DNN";
    pub const INSTALL: &str = "Install DNN";
    pub const HOST: &str = "Create host account";
    pub const START: &str = "Start website";
    pub const VERIFY: &str = "Verify site";
}

/// DNN's extension packages have deep folders: from about 100 characters on some fail to install
/// (`DnnAccountRules.MaxSitePathLength`, `dnnm_dnn::install::MAX_SITE_PATH_LENGTH`).
pub const MAX_SITE_PATH_LENGTH: usize = 100;

/// Where a new site's database lives (`DatabaseKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    /// The local SQL Server container (Docker) from the settings.
    Container,
    /// Any SQL Server or SQL Server Express instance.
    SqlServer,
    /// LocalDB attaching the site's own `App_Data\Database.mdf`.
    LocalDbFile,
}

/// A new site's database and how the site signs in to it (`DatabaseConnection`). No `Debug` prints the password.
#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseConnection {
    pub kind: DatabaseKind,
    pub server: String,
    /// The database's name; the `.mdf` file in App_Data for [`DatabaseKind::LocalDbFile`].
    pub database: String,
    /// Integrated security instead of a SQL Server login.
    pub windows_authentication: bool,
    pub user: String,
    pub password: String,
}

impl DatabaseConnection {
    /// LocalDB's automatic instance - the one every Windows user has.
    pub const LOCAL_DB_SERVER: &str = r"(LocalDB)\MSSQLLocalDB";
    /// The empty database file DNN's install package ships in App_Data.
    pub const LOCAL_DB_FILE_NAME: &str = "Database.mdf";

    /// A SQL Server login's connection.
    pub fn sql_login(kind: DatabaseKind, server: &str, database: &str, user: &str, password: &str) -> Self {
        Self {
            kind,
            server: server.into(),
            database: database.into(),
            windows_authentication: false,
            user: user.into(),
            password: password.into(),
        }
    }

    /// A Windows authentication connection.
    pub fn windows(kind: DatabaseKind, server: &str, database: &str) -> Self {
        Self {
            kind,
            server: server.into(),
            database: database.into(),
            windows_authentication: true,
            user: String::new(),
            password: String::new(),
        }
    }

    pub fn uses_windows_authentication(&self) -> bool {
        self.kind == DatabaseKind::LocalDbFile || self.windows_authentication
    }

    /// "[db] on server (who)" - for messages; never the password (`DatabaseConnection.Describe`).
    pub fn describe(&self) -> String {
        let who = if self.uses_windows_authentication() {
            "Windows authentication".to_string()
        } else {
            format!("user '{}'", self.user)
        };
        if self.kind == DatabaseKind::LocalDbFile {
            format!("App_Data\\{} with {} ({who})", self.database, self.server)
        } else {
            format!("[{}] on {} ({who})", self.database, self.server)
        }
    }
}

impl fmt::Debug for DatabaseConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// The host account and website an automatic install creates (`DnnAccount`). No `Debug` prints the password.
#[derive(Clone, PartialEq, Eq)]
pub struct DnnAccount {
    pub user_name: String,
    pub password: String,
    pub email: String,
    pub website_name: String,
    pub language: String,
    pub template: String,
}

impl fmt::Debug for DnnAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DnnAccount")
            .field("user_name", &self.user_name)
            .field("email", &self.email)
            .field("website_name", &self.website_name)
            .field("language", &self.language)
            .field("template", &self.template)
            .finish_non_exhaustive()
    }
}

/// What New project asks for (`SetupProjectRequest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupProjectRequest {
    pub project_name: String,
    pub release_api_url: String,
    /// The release's tag; None for the latest release.
    pub version: Option<String>,
    /// The host name the site answers on; None for `{project}.{hostname suffix}`.
    pub host_name: Option<String>,
    /// The port the site answers on; None for the settings' site port.
    pub port: Option<u16>,
    pub install_mode: InstallMode,
    /// The host account and website an automatic install creates.
    pub account: Option<DnnAccount>,
    /// The site's database; None for a database named like the project on the local SQL container.
    pub database: Option<DatabaseConnection>,
}

/// The settings the set-up goes by (`AppOptions`): the host name suffix, the site port and the local container.
#[derive(Clone, PartialEq, Eq)]
pub struct SetupOptions {
    pub hostname_suffix: String,
    pub site_port: u16,
    /// `sqlServer.host` - where the container publishes SQL Server.
    pub container_host: String,
    /// `sqlServer.port`.
    pub container_port: u16,
    /// The login on the container: `sqlServer.userName`, or sa.
    pub container_user: String,
    pub container_password: String,
    pub collation: String,
}

impl fmt::Debug for SetupOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupOptions")
            .field("hostname_suffix", &self.hostname_suffix)
            .field("site_port", &self.site_port)
            .field("container_host", &self.container_host)
            .field("container_port", &self.container_port)
            .field("container_user", &self.container_user)
            .field("collation", &self.collation)
            .finish_non_exhaustive()
    }
}

impl SetupOptions {
    /// `{project}.{suffix}` (`AppOptions.HostnameFor`).
    pub fn hostname_for(&self, project: &str) -> String {
        format!("{project}.{}", self.hostname_suffix)
    }

    /// The local database a new project gets: named like the project (`AppOptions.DatabaseNameFor`).
    pub fn database_name_for(&self, project: &str) -> String {
        project.to_string()
    }

    /// The container's SQL Server address, `host,port` (`AppOptions.ServerFor`).
    pub fn container_server(&self) -> String {
        format!("{},{}", self.container_host, self.container_port)
    }

    /// Database `database` on the local container, as its user (`LocalSqlContainer.Connection`).
    pub fn container_connection(&self, database: &str) -> DatabaseConnection {
        DatabaseConnection::sql_login(
            DatabaseKind::Container,
            &self.container_server(),
            database,
            &self.container_user,
            &self.container_password,
        )
    }
}

/// How one check of a database test went (`CheckOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Passed,
    Warning,
    Failed,
}

/// One check of a database test (`DatabaseCheck`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseCheck {
    pub name: String,
    pub outcome: CheckOutcome,
    pub detail: String,
}

/// What the database test checks for a new install (`DatabaseCheckOptions` with `ForNewInstall`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseCheckOptions {
    /// With Windows authentication on SQL Server, the login the site will sign in as.
    pub site_login: Option<String>,
    /// The oldest SQL Server the DNN version supports.
    pub minimum_major_version: i32,
}

/// The site an automatic install talks to (`DnnSiteAddress`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteAddress {
    pub directory: PathBuf,
    pub alias: String,
    pub port: u16,
}

/// What the set-up acts on besides IIS - one method per call of the C#'s services. Err is the message to show;
/// "Cancelled." after a cancel.
pub trait SetupSystem: Send + Sync {
    /// The project of that name: its folder in the projects folder (`IProjectRepository.Build`).
    fn project(&self, name: &str) -> DnnProject;
    /// Everything wrong with the host account, empty when DNN can install it (`DnnAccountRules.Problems`).
    fn account_problems(&self, account: &DnnAccount) -> Vec<String>;
    /// The release of that version, or the latest (`IDnnReleaseService.GetReleaseAsync`).
    fn get_release(&self, api_url: &str, version: Option<&str>, cancel: &CancelToken) -> Result<DnnRelease, String>;
    /// Checks IIS's Windows features and offers to enable the missing ones (`IPrerequisiteChecker`).
    fn ensure_iis_features(&self, op: &mut Operation) -> Result<(), String>;
    fn database_exists(&self, database: &DatabaseConnection, cancel: &CancelToken) -> Result<bool, String>;
    fn drop_database(&self, database: &DatabaseConnection, cancel: &CancelToken) -> Result<(), String>;
    /// Test connection's checks; Err only on a cancel.
    fn check_database(
        &self,
        database: &DatabaseConnection,
        options: &DatabaseCheckOptions,
        cancel: &CancelToken,
    ) -> Result<Vec<DatabaseCheck>, String>;
    /// Downloads (or takes the kept) install package and extracts it (`IDnnPackageInstaller.DownloadAndExtractAsync`).
    fn download_and_extract(
        &self,
        release: &DnnRelease,
        directory: &Path,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// A DNN-tuned `.gitignore` in the new site (`IProjectScaffolder.EnsureGitignore`).
    fn ensure_gitignore(&self, directory: &Path) -> Result<(), String>;
    /// The folder holds a fresh DNN install package (`IDnnInstaller.CheckPackage`).
    fn check_package(&self, directory: &Path) -> Result<(), String>;
    fn prepare_local_db_file(
        &self,
        directory: &Path,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    fn create_database(
        &self,
        database: &DatabaseConnection,
        collation: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// Lets the Windows login `identity` own the database.
    fn grant_site_access(
        &self,
        database: &DatabaseConnection,
        identity: &str,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// The project's own login on the container, owner of `database` only, with a new password: (login, password)
    /// (`LocalSqlContainer.GrantSiteLoginAsync`).
    fn grant_site_login(&self, project: &str, database: &str, cancel: &CancelToken)
    -> Result<(String, String), String>;
    fn drop_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String>;
    /// Writes the site's `SiteSqlServer` connection (`IWebConfigService.WriteDatabaseConnection`).
    fn write_database_connection(&self, web_config: &Path, database: &DatabaseConnection) -> Result<(), String>;
    fn install(
        &self,
        site: &SiteAddress,
        account: &DnnAccount,
        database: &DatabaseConnection,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    fn complete(
        &self,
        site: &SiteAddress,
        account: &DnnAccount,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    fn warm_up(
        &self,
        site: &SiteAddress,
        install_started: SystemTime,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// Removes the install template and blocker - and, once installed, DNN's installer pages.
    fn clean_up(&self, directory: &Path, installed: bool);
    /// The HTTP status `url` answers with (`IHttpConnectivityChecker.CheckAsync`).
    fn http_check(&self, url: &str, timeout_seconds: u64, cancel: &CancelToken) -> Result<u16, String>;
    /// `IProjectRecords.Save` - a failure is the implementation's to log.
    fn save_record(&self, record: &ProjectRecord);
}

/// What the set-up is given: the system, IIS and the settings.
#[derive(Clone)]
pub struct SetupDeps {
    pub system: Arc<dyn SetupSystem>,
    pub iis: Arc<dyn IisManager>,
    pub options: SetupOptions,
}

/// The site among `sites` (by name) that already has an http binding for `host` on `port`; None when none has. The
/// site named `replacing` doesn't count: a new site of that name replaces it (`IisHostNames.SiteUsing`).
pub fn site_using<'a>(
    sites: impl IntoIterator<Item = (&'a str, &'a IisSiteRuntime)>,
    host: &str,
    port: u16,
    replacing: Option<&str>,
) -> Option<String> {
    let name = host.trim().trim_end_matches('.');
    if name.is_empty() {
        return None;
    }
    sites
        .into_iter()
        .filter(|(site, _)| !replacing.is_some_and(|r| r.eq_ignore_ascii_case(site)))
        .find(|(_, runtime)| {
            runtime.bindings.iter().any(|b| {
                b.protocol.eq_ignore_ascii_case("http")
                    && b.port == Some(port)
                    && b.host.trim().trim_end_matches('.').eq_ignore_ascii_case(name)
            })
        })
        .map(|(site, _)| site.to_string())
}

/// DNN 10 needs SQL Server 2017 (14) or later; DNN 9 runs on SQL Server 2012 (11) and later.
pub fn minimum_sql_server_for(release: &DnnRelease) -> i32 {
    let major = release.version.split('.').next().unwrap_or_default().trim_start_matches(['v', 'V']);
    match major.parse::<i32>() {
        Ok(major) if major < 10 => 11,
        _ => 14,
    }
}

/// The text with its first letter in lower case - "something went wrong." when empty.
fn lower(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => "something went wrong.".into(),
        Some(first) => first.to_lowercase().chain(chars).collect(),
    }
}

/// `SetupProjectUseCase.ExecuteAsync`.
pub fn setup_project(deps: &SetupDeps, request: &SetupProjectRequest, op: &mut Operation) -> Result<(), String> {
    validate_project_name(&request.project_name)?;

    let automatic = request.install_mode == InstallMode::Automatic;
    if automatic {
        let Some(account) = &request.account else {
            return Err("An automatic install needs the host account and the website.".into());
        };
        let problems = deps.system.account_problems(account);
        if !problems.is_empty() {
            return Err(problems.join(" "));
        }
    }

    let host_name = match request.host_name.as_deref().map(str::trim) {
        Some(host) if !host.is_empty() => host.to_string(),
        _ => deps.options.hostname_for(&request.project_name),
    };
    let port = request.port.unwrap_or(deps.options.site_port);
    let project = deps.system.project(&request.project_name);
    let directory = project.project_directory.clone();

    let result = run(deps, request, &project, &host_name, port, automatic, op);
    // Whatever happened: the install template holds the host password, and runs DNN's installer for anyone.
    if automatic && directory.is_dir() {
        deps.system.clean_up(&directory, false);
    }
    result
}

fn run(
    deps: &SetupDeps,
    request: &SetupProjectRequest,
    project: &DnnProject,
    host_name: &str,
    port: u16,
    automatic: bool,
    op: &mut Operation,
) -> Result<(), String> {
    let system = &deps.system;
    let iis = &deps.iis;
    let alias = provisioner::alias_for(host_name, port);
    let url = format!("http://{alias}");
    let directory = &project.project_directory;
    // The site's database: the one chosen, or as before a database named like the project on the local container.
    let mut database = request
        .database
        .clone()
        .unwrap_or_else(|| deps.options.container_connection(&deps.options.database_name_for(&project.name)));
    let chosen_database = request.database.is_some();

    // A new project gets a folder of its own - one that is there is never set up over.
    if directory.exists() {
        return Err(format!(
            "A project named '{}' already exists ({}). Choose another name.",
            request.project_name,
            directory.display()
        ));
    }
    // Nor does it take over an IIS site of that name serving another folder: making its site would replace that one.
    if let Some(taken) = provisioner::site_name_taken(iis.as_ref(), &request.project_name, directory) {
        return Err(taken);
    }
    let length = directory.as_os_str().to_string_lossy().encode_utf16().count();
    if automatic && length > MAX_SITE_PATH_LENGTH {
        return Err(format!(
            "The project's folder is too deep for DNN's installer ({length} characters, at most {MAX_SITE_PATH_LENGTH}) - use a shorter projects folder (Settings → Projects)."
        ));
    }

    // The stages, up front: the Output tab shows those still to come - and as skipped those it never gets to.
    if automatic {
        op.report.plan(&[
            stage::VERSION,
            stage::IIS,
            stage::DATABASE,
            stage::FOLDER,
            stage::DOWNLOAD,
            stage::SITE,
            stage::CREATE_DATABASE,
            stage::CONFIGURE,
            stage::INSTALL,
            stage::HOST,
            stage::START,
        ]);
    } else {
        op.report.plan(&[
            stage::VERSION,
            stage::IIS,
            stage::DATABASE,
            stage::FOLDER,
            stage::DOWNLOAD,
            stage::SITE,
            stage::CREATE_DATABASE,
            stage::VERIFY,
        ]);
    }

    op.report.step("DNN version");
    let release = system
        .get_release(&request.release_api_url, request.version.as_deref(), &op.cancel)
        .map_err(|e| if e.is_empty() { "Could not resolve a DNN release.".to_string() } else { e })?;
    op.report.success(&format!("Using DNN {} ({})", release.version, release.download_url));
    op.cancel.check()?;

    op.report.step("Checking IIS");
    let iis_available = iis.is_available();
    if iis_available {
        // Before anything is downloaded: a second site with the same address wouldn't start, and DNN's installer would
        // talk to the first one. New project says so as it is typed; this is for what changed since.
        let sites = iis.site_runtimes().unwrap_or_default();
        let named = sites.iter().map(|(name, site)| (name.as_str(), site));
        if let Some(other) = site_using(named, host_name, port, Some(&project.name)) {
            return Err(format!(
                "{url} is already the address of the IIS site '{other}' - choose another host name or port."
            ));
        }
        // What the user decided about missing features is said in the Output; only a cancel ends the set-up here.
        if let Err(e) = system.ensure_iis_features(op)
            && e == CANCELLED
        {
            return Err(e);
        }
    } else if automatic {
        return Err("An automatic install needs IIS - set it up in Settings → IIS, or choose Manual DNN setup.".into());
    } else {
        op.report.info(
            "IIS not found - skipping website creation. Install IIS and re-run setup to host the site, or use your own web server.",
        );
    }
    op.cancel.check()?;

    op.report.step("Testing database connection");
    let database_ready = check_database(deps, &project.name, &database, &release, automatic, op);
    if let Err(e) = &database_ready {
        if e == CANCELLED {
            return Err(e.clone());
        }
        if automatic || chosen_database {
            return Err(format!("The database isn't ready: {e}"));
        }
    }
    let create_database = matches!(database_ready, Ok(true));
    let database_reachable = database_ready.is_ok();
    if !database_reachable {
        op.report.info(
            "Skipping the database. Start the SQL Server container and set the project up again, or point the site's web.config at your own database.",
        );
    }
    op.cancel.check()?;

    op.report.step("Creating project directory");
    // A cancel takes the new project away again - its folder goes last, once nothing uses it any more.
    op.undo.delete_folder_on_undo(directory);
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    op.report.success(&format!("Project directory ready: {}", directory.display()));
    op.cancel.check()?;

    op.report.step(&format!("Downloading DNN {}", release.version));
    system.download_and_extract(&release, directory, op.report, &op.cancel)?;
    // Drop a DNN-tuned .gitignore next to the freshly extracted site so the project is ready to commit without
    // dragging in runtime data, caches, logs or portal uploads.
    match system.ensure_gitignore(directory) {
        Ok(()) => op.report.info("Project .gitignore ready."),
        Err(e) => op.report.info(&format!("Could not write .gitignore: {e}")),
    }
    if automatic {
        system.check_package(directory)?;
    }
    if database.kind == DatabaseKind::LocalDbFile && database_reachable {
        op.report.info(&format!("Preparing the database file App_Data\\{}…", database.database));
        system.prepare_local_db_file(directory, &database, &op.cancel)?;
    }
    op.cancel.check()?;

    op.report.step("Creating IIS application pool and website");
    let mut site_created = false;
    if iis_available {
        op.report.info(&format!("Creating IIS application pool and website '{}' for {url}…", project.name));
        site_created = provisioner::try_create_site(iis, &project.name, directory, host_name, port, op);
        if site_created
            && database.kind == DatabaseKind::LocalDbFile
            && let Err(e) = iis.enable_user_profile(&project.name)
        {
            op.report.fail(&format!("Could not load the app pool's user profile, which LocalDB needs: {e}"));
        }
    } else {
        op.report.info("Skipped - IIS not available.");
    }
    if automatic && !site_created {
        return Err("The IIS website couldn't be created, so DNN can't be installed - see the messages above.".into());
    }
    op.cancel.check()?;

    op.report.step("Creating database");
    if database_reachable {
        let created = provision_database(deps, project, &database, create_database, site_created, op);
        if let Err(e) = created
            && (automatic || chosen_database || e == CANCELLED)
        {
            return Err(e);
        }
    } else {
        op.report.info("Skipped - the database isn't reachable.");
    }
    op.cancel.check()?;

    if !automatic {
        return finish_manual(deps, project, &release, &database, database_reachable, site_created, &url, op);
    }

    let account = request.account.clone().expect("checked above");
    op.report.step("Configuring DNN");
    // On the local container the site signs in with a login of its own, owner of its database only - not sa.
    if database.kind == DatabaseKind::Container && create_database {
        let (login, password) = system.grant_site_login(&project.name, &database.database, &op.cancel)?;
        let (undo_system, undo_login) = (Arc::clone(system), login.clone());
        op.undo.add(format!("Drop the login {login}"), STEP_LIMIT, move |cancel| {
            undo_system.drop_login(&undo_login, cancel)
        });
        database.user = login;
        database.password = password;
    }
    system
        .write_database_connection(&directory.join("web.config"), &database)
        .map_err(|e| format!("Could not write the site's connection string: {e}"))?;
    op.report.success(&format!("web.config connects to {}.", database.describe()));
    op.cancel.check()?;

    op.report.step("Running DNN installation");
    let site = SiteAddress { directory: directory.clone(), alias: alias.clone(), port };
    let install_started = SystemTime::now();
    if let Err(e) = system.install(&site, &account, &database, op.report, &op.cancel) {
        if e == CANCELLED {
            return Err(e);
        }
        system.clean_up(directory, false);
        // Left to look into: the folder, the site and the database stay, as the message says.
        op.undo.keep();
        return Err(format!(
            "DNN's installation failed: {e} The project is left as it is to look into - remove it and set it up again (DNN can't install twice into the same files and database)."
        ));
    }

    op.report.step("Creating host account");
    // A LocalDB file database is only opened by DNN Manager while the site doesn't use it.
    if database.kind == DatabaseKind::LocalDbFile {
        let _ = iis.stop_site(&project.name);
    }
    let completed = system.complete(&site, &account, &database, &op.cancel);
    // Started again whatever happened: a site left stopped looks broken.
    if database.kind == DatabaseKind::LocalDbFile {
        let _ = iis.start_site(&project.name);
    } else {
        // DNN caches its users - it reads the host account again.
        let _ = iis.recycle_app_pool(&project.name);
    }
    if let Err(e) = completed {
        if e == CANCELLED {
            return Err(e);
        }
        system.clean_up(directory, false);
        op.undo.keep();
        return Err(format!(
            "DNN's installation didn't finish: {e} The project is left as it is to look into - remove it and set it up again."
        ));
    }
    op.report.success(&format!(
        "Host account '{}' ready - it signs in without being asked to change its password.",
        account.user_name
    ));

    op.report.step("Starting website");
    let warm_up = system.warm_up(&site, install_started, op.report, &op.cancel);
    system.clean_up(directory, true);
    system.save_record(&ProjectRecord {
        site: project.name.clone(),
        install_mode: InstallMode::Automatic,
        created_utc: utc_now_round_trip(),
        dnn_version: Some(release.version.clone()),
        host_user_name: Some(account.user_name.clone()),
    });
    // Installed: a site that is slow to answer is still the project.
    op.undo.keep();
    if let Err(e) = warm_up {
        return Err(if e == CANCELLED { e } else { format!("DNN is installed, but {}", lower(&e)) });
    }

    op.report.step("Setup complete");
    op.report.link(&url);
    op.report.success(&format!("DNN installation completed. Open {url} - sign in as '{}'.", account.user_name));
    Ok(())
}

/// Tests the database: reachable, signed in, new enough for `release`, and usable for the new site. A database of that
/// name that already exists can be dropped and created again, after asking. Ok: whether the database still has to be
/// created.
fn check_database(
    deps: &SetupDeps,
    site_name: &str,
    database: &DatabaseConnection,
    release: &DnnRelease,
    automatic: bool,
    op: &mut Operation,
) -> Result<bool, String> {
    let system = &deps.system;
    // With Windows authentication the site signs in as its app pool's identity (IIS APPPOOL\<site> for a new site).
    let options = DatabaseCheckOptions {
        site_login: (database.kind == DatabaseKind::SqlServer && database.uses_windows_authentication())
            .then(|| deps.iis.app_pool_identity(site_name)),
        minimum_major_version: minimum_sql_server_for(release),
    };

    if database.kind != DatabaseKind::LocalDbFile && system.database_exists(database, &op.cancel) == Ok(true) {
        let question = format!(
            "Database [{}] already exists on {}. Drop it and create it again? Everything in it is lost.",
            database.database, database.server
        );
        if op.prompt.confirm(&question, "Drop and recreate", "Keep it", true) {
            op.cancel.check()?;
            system.drop_database(database, &op.cancel)?;
            op.undo.cannot_undo(format!(
                "database [{}] on {} was dropped, as you chose - what was in it is gone.",
                database.database, database.server
            ));
            op.report.info(&format!("Dropped database [{}].", database.database));
        } else if !automatic {
            // As before: a manual install can go into the database that is there.
            op.report.info(&format!("Keeping database [{}] as it is.", database.database));
            return Ok(false);
        }
    }
    op.cancel.check()?;

    let checks = system.check_database(database, &options, &op.cancel)?;
    for check in &checks {
        let line = format!("{}: {}", check.name, check.detail);
        match check.outcome {
            CheckOutcome::Passed => op.report.success(&line),
            CheckOutcome::Warning => op.report.warn(&line),
            CheckOutcome::Failed => op.report.fail(&line),
        }
    }
    let passed = !checks.is_empty() && checks.iter().all(|c| c.outcome != CheckOutcome::Failed);
    if !passed {
        let problem =
            checks.iter().find(|c| c.outcome == CheckOutcome::Failed).map(|c| format!("{}: {}", c.name, c.detail));
        return Err(problem.unwrap_or_else(|| "The database test failed.".into()));
    }
    if database.kind == DatabaseKind::LocalDbFile {
        return Ok(false);
    }
    let still_there = system.database_exists(database, &op.cancel);
    Ok(still_there != Ok(true))
}

/// Creates the database when it isn't there and, with Windows authentication, lets the site's app pool identity own
/// it. A LocalDB file database needs neither: it is the package's own file, attached by the site.
fn provision_database(
    deps: &SetupDeps,
    project: &DnnProject,
    database: &DatabaseConnection,
    create: bool,
    site_created: bool,
    op: &mut Operation,
) -> Result<(), String> {
    if database.kind == DatabaseKind::LocalDbFile {
        op.report.success(&format!(
            "The site's database is App_Data\\{} - LocalDB attaches it when the site first opens it.",
            database.database
        ));
        return Ok(());
    }
    let system = &deps.system;
    if create {
        // Before it is made: a cancel while it is being created drops it too.
        let (undo_system, undo_database) = (Arc::clone(system), database.clone());
        op.undo.add(
            format!("Drop database [{}] on {}", database.database, database.server),
            STEP_LIMIT,
            move |cancel| undo_system.drop_database(&undo_database, cancel),
        );
        op.report.info(&format!("Creating database [{}] on {}…", database.database, database.server));
        let collation = (database.kind == DatabaseKind::Container).then_some(deps.options.collation.as_str());
        if let Err(e) = system.create_database(database, collation, &op.cancel) {
            if e != CANCELLED {
                op.report.fail(&e);
            }
            return Err(e);
        }
        op.report.success(&format!("Database [{}] ready on {}.", database.database, database.server));
    }
    if database.uses_windows_authentication() && site_created {
        let identity = deps.iis.app_pool_identity(&project.name);
        op.report.info(&format!("Giving the site's identity ({identity}) access to the database…"));
        if let Err(e) = system.grant_site_access(database, &identity, &op.cancel) {
            if e != CANCELLED {
                op.report.fail(&e);
            }
            return Err(e);
        }
        op.report.success(&format!("{identity} owns [{}].", database.database));
    }
    Ok(())
}

/// Manual DNN setup: everything is in place for DNN's installation wizard, which the user runs on the first visit.
#[allow(clippy::too_many_arguments)]
fn finish_manual(
    deps: &SetupDeps,
    project: &DnnProject,
    release: &DnnRelease,
    database: &DatabaseConnection,
    database_ready: bool,
    site_created: bool,
    url: &str,
    op: &mut Operation,
) -> Result<(), String> {
    if database_ready {
        let how = if database.kind == DatabaseKind::LocalDbFile {
            format!("the database file {} (SQL Server Express File).", database.database)
        } else {
            let who = if database.uses_windows_authentication() {
                "Windows authentication (integrated security).".to_string()
            } else if database.kind == DatabaseKind::Container {
                "user 'sa' and the SA password from Settings → Database server.".to_string()
            } else {
                format!("user '{}' and its password.", database.user)
            };
            format!("database '{}', {who}", database.database)
        };
        op.report.info(&format!("In DNN's installation wizard, connect to: server '{}', {how}", database.server));
    }

    if site_created {
        op.report.step("Verify site");
        match deps.system.http_check(url, 15, &op.cancel) {
            Ok(status) => op.report.success(&format!("HTTP {status} from {url}")),
            Err(e) if e == CANCELLED => return Err(e),
            Err(e) => op.report.info(&format!("HTTP probe: {e} (expected before install wizard runs)")),
        }
    }

    deps.system.save_record(&ProjectRecord {
        site: project.name.clone(),
        install_mode: InstallMode::Manual,
        created_utc: utc_now_round_trip(),
        dnn_version: Some(release.version.clone()),
        host_user_name: None,
    });
    op.report.step("Setup complete");
    if site_created {
        op.report.link(url);
        op.report.success(&format!("Open {url} to complete the DNN Installation Wizard."));
    } else {
        op.report.success(&format!(
            "DNN files are ready in {}. Point a web server (and database) at them to run the install wizard.",
            project.project_directory.display()
        ));
    }
    Ok(())
}

// ─── What New project looks up while it is filled in ──────────────────────────

/// A project with backups to import from ("From a project backup").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupProject {
    pub project: String,
    /// Its complete backups (site zip and database), newest first.
    pub backups: Vec<BackupChoice>,
}

/// One complete backup: its label (`yyyy-MM-dd HH:mm:ss  (12.3 MB)`) and its two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupChoice {
    pub label: String,
    pub site_zip: PathBuf,
    pub database: PathBuf,
}

/// The fields of the New project page a draft keeps, by their names in the WPF page (`FormDraft`: the named text
/// boxes, radio buttons and lists, in the order they are on the form - never a password).
pub const DRAFT_FIELDS: &[&str] = &[
    "NameBox",
    "HostNameBox",
    "PortBox",
    "FromScratch",
    "FromZip",
    "BackupProjectCombo",
    "ProjectBackupCombo",
    "ZipBox",
    "BackupBox",
    "SourceCombo",
    "VersionCombo",
    "AutomaticInstall",
    "ManualInstall",
    "HostUserBox",
    "HostEmailBox",
    "WebsiteNameBox",
    "LanguageCombo",
    "TemplateCombo",
    "DbContainer",
    "DbSqlServer",
    "DbLocalDb",
    "DbServerBox",
    "DbNameBox",
    "DbWindowsAuth",
    "DbSqlAuth",
    "DbUserBox",
];

/// What the New project page asks of the system - each called off the UI thread but `account_problems`,
/// `generate_password` and `project_exists`, which are quick.
pub trait NewProjectLookups: Send + Sync {
    /// The release sources of the settings (GitHub releases API URLs).
    fn release_sources(&self) -> Vec<String>;
    /// The releases of a source - saved ones when GitHub can't be reached (`saved_at` set).
    fn list_releases(&self, api_url: &str, cancel: &CancelToken) -> Result<DnnReleaseList, String>;
    /// The release's install package is kept, so it needs no download.
    fn is_kept(&self, release: &DnnRelease) -> bool;
    /// Settings → DNN releases → Keep downloaded DNN install packages.
    fn keeps_packages(&self) -> bool;
    fn database_exists(&self, database: &DatabaseConnection, cancel: &CancelToken) -> Result<bool, String>;
    fn account_problems(&self, account: &DnnAccount) -> Vec<String>;
    fn generate_password(&self) -> String;
    /// The project's folder exists already (`IProjectRepository.ProjectExists`).
    fn project_exists(&self, name: &str) -> bool;
    /// The project's folder (`IProjectRepository.Build(name).ProjectDirectory`).
    fn project_directory(&self, name: &str) -> PathBuf;
    /// The projects with a complete backup, by name.
    fn backup_projects(&self) -> Vec<BackupProject>;
    /// The page's saved draft (field name → value), empty when there is none.
    fn load_draft(&self) -> BTreeMap<String, String>;
    fn save_draft(&self, draft: &BTreeMap<String, String>);
    /// The passwords the page starts from: the default host password and the database server login's (Credential
    /// Manager), and the container's sa password of `settings` (decrypted).
    fn secrets(&self, settings: &crate::settings::Settings) -> PageSecrets;
}

/// The passwords the New project page fills in - held in memory only, never in its draft.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PageSecrets {
    pub default_host_password: Option<String>,
    pub database_server_password: Option<String>,
    pub sa_password: String,
}

impl fmt::Debug for PageSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageSecrets { .. }")
    }
}

#[cfg(test)]
mod tests;
