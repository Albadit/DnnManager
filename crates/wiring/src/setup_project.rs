//! New project joined to what it acts on: [`setup_project`] runs `SetupProjectUseCase` with GitHub's releases, the
//! DNN package and installer (`dnnm-dnn`), SQL Server and the container (`dnnm-sql`), IIS's Windows features
//! (`dnnm-system`) and the site's web.config; [`PageLookups`] is what the New project page asks while it is filled in
//! (`SetupPage.xaml.cs`'s services), and [`load_form_draft`] / [`save_form_draft`] keep its draft in the `state` table
//! as the WPF app's `FormDraft` does.
//!
//! Written here because no other crate has them yet: DNN's `InstallerSql` over `dnnm-sql` ([`SiteSql`]),
//! `ProjectScaffolder.EnsureGitignore`, `SqlPasswords.New` and `LocalSqlContainer.FreeLoginFor`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dnnm_application::iis::IisManager;
use dnnm_application::operation::{CANCELLED, CancelToken, Operation, Reporter};
use dnnm_application::records::{ProjectRecord, ProjectRecords};
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::setup_project::{
    self as use_case, BackupChoice, BackupProject, CheckOutcome, DatabaseCheck, DatabaseCheckOptions,
    DatabaseConnection, DatabaseKind, DnnAccount, NewProjectLookups, PageSecrets, SetupDeps, SetupOptions,
    SetupProjectRequest, SetupSystem, SiteAddress,
};
use dnnm_dnn::install::{self as dnn_install, DnnSiteAddress};
use dnnm_dnn::installer::{DbVersion, HostPasswordOutcome, Installer, InstallerSql, VersionOrder};
use dnnm_dnn::packages::{PackageInstaller, PackageOptions};
use dnnm_dnn::releases::{GitHubHttp, ReleaseService, ReleaseStore};
use dnnm_domain::{DnnProject, DnnRelease, DnnReleaseList};
use dnnm_infrastructure::app_db::{AppDatabase, ProjectTable};
use dnnm_infrastructure::paths::app_data_directory;
use dnnm_infrastructure::web_config;
use dnnm_sql::client::{Conn, SqlError, block_on};
use dnnm_sql::connection_strings::{for_app, for_site};
use dnnm_sql::container::SqlServerService;
use dnnm_sql::provisioner::DatabaseProvisioner;
use dnnm_sql::sql_text::dnn_tables;
use dnnm_sql::{SqlAuthentication, SqlServerAddress};

use crate::Services;

/// The release sources when the settings have none (`UserSettings.Projects.DnnReleaseSources`).
pub const DEFAULT_RELEASE_SOURCES: [&str; 2] = [
    "https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases",
    "https://api.github.com/repos/DNN-Connect/Dnn.Platform/releases",
];

/// The operation's title: `Create 'shop'` (`SetupPage.Run_Click`).
pub fn setup_title(project: &str) -> String {
    format!("Create '{project}'")
}

/// The toast when it is done (`SetupPage.Run_Click`).
pub fn setup_done(request: &SetupProjectRequest) -> String {
    match &request.account {
        Some(account) if request.install_mode == dnnm_application::records::InstallMode::Automatic => {
            format!("'{}' is ready - DNN is installed. Sign in as '{}'.", request.project_name, account.user_name)
        }
        _ => format!("'{}' is set up - open it to run DNN's installation wizard.", request.project_name),
    }
}

/// New project: `SetupProjectUseCase` over the real system.
pub fn setup_project(services: &Services, request: SetupProjectRequest, op: &mut Operation) -> Result<(), String> {
    let options = setup_options(services)?;
    let system: Arc<dyn SetupSystem> = Arc::new(WiredSetup::new(services));
    let deps = SetupDeps { system, iis: Arc::clone(&services.iis), options };
    use_case::setup_project(&deps, &request, op)
}

/// The settings the set-up goes by. Err when the sa password can't be decrypted.
pub fn setup_options(services: &Services) -> Result<SetupOptions, String> {
    let docker = crate::remove_project::docker_options(services)?;
    let settings = &services.settings;
    Ok(SetupOptions {
        hostname_suffix: settings.get("projects.hostnameSuffix").unwrap_or("dnndev.me").trim().to_string(),
        site_port: settings.get("projects.sitePort").and_then(|p| p.trim().parse().ok()).unwrap_or(80),
        container_host: docker.container_ip,
        container_port: docker.default_port,
        container_user: docker.sql_user,
        container_password: docker.sa_password,
        collation: docker.collation,
    })
}

/// The release sources of the settings, or the defaults.
fn release_sources(settings: &Settings) -> Vec<String> {
    settings
        .list("projects.dnnReleaseSources")
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_SOURCES.iter().map(|s| s.to_string()).collect())
}

fn keeps_packages(settings: &Settings) -> bool {
    settings.get("projects.keepDnnPackages").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

fn release_service(services: &Services) -> ReleaseService {
    ReleaseService::new(
        GitHubHttp::default(),
        ReleaseStore::new(services.db.path()),
        release_sources(&services.settings),
    )
}

/// The package installer: kept packages in `Documents\DnnManager\packages`, downloads in the private temp folder.
fn package_installer(settings: &Settings) -> Result<PackageInstaller, String> {
    let private_temp = dnnm_system::files::private_temp::path()
        .map_err(|e| format!("Could not prepare DNN Manager's private temp folder: {e}"))?;
    Ok(PackageInstaller::new(PackageOptions {
        keep_packages: keeps_packages(settings),
        packages_directory: app_data_directory().join("packages"),
        private_temp,
    }))
}

fn project_of(settings: &Settings, name: &str) -> DnnProject {
    DnnProject {
        name: name.to_string(),
        project_directory: PathBuf::from(settings.base_directory()).join(name),
        backup_directory: app_data_directory().join("backups").join(name),
    }
}

fn to_dnn_account(account: &DnnAccount) -> dnn_install::DnnAccount {
    dnn_install::DnnAccount {
        user_name: account.user_name.clone(),
        password: account.password.clone(),
        email: account.email.clone(),
        website_name: account.website_name.clone(),
        language: account.language.clone(),
        template: account.template.clone(),
    }
}

/// The connection as `dnnm-sql` has it.
fn to_sql(c: &DatabaseConnection) -> dnnm_sql::DatabaseConnection {
    let kind = match c.kind {
        DatabaseKind::Container => dnnm_sql::DatabaseKind::Container,
        DatabaseKind::SqlServer => dnnm_sql::DatabaseKind::SqlServer,
        DatabaseKind::LocalDbFile => dnnm_sql::DatabaseKind::LocalDbFile,
    };
    let authentication =
        if c.uses_windows_authentication() { SqlAuthentication::Windows } else { SqlAuthentication::Sql };
    dnnm_sql::DatabaseConnection::new(kind, &c.server, &c.database, authentication).with_login(&c.user, &c.password)
}

/// The connection as web.config (and DNN's installer) has it: the container is a SQL Server.
fn to_site(c: &DatabaseConnection) -> web_config::DatabaseConnection {
    web_config::DatabaseConnection {
        kind: if c.kind == DatabaseKind::LocalDbFile {
            web_config::DatabaseKind::LocalDbFile
        } else {
            web_config::DatabaseKind::SqlServer
        },
        server: c.server.clone(),
        database: c.database.clone(),
        windows_authentication: c.uses_windows_authentication(),
        user: c.user.clone(),
        password: c.password.clone(),
    }
}

/// `web_config::DatabaseConnection` back to `dnnm-sql`'s, to open it.
fn site_to_sql(c: &web_config::DatabaseConnection) -> dnnm_sql::DatabaseConnection {
    let kind = match c.kind {
        web_config::DatabaseKind::LocalDbFile => dnnm_sql::DatabaseKind::LocalDbFile,
        web_config::DatabaseKind::SqlServer => dnnm_sql::DatabaseKind::SqlServer,
    };
    let authentication = if c.windows_authentication { SqlAuthentication::Windows } else { SqlAuthentication::Sql };
    dnnm_sql::DatabaseConnection::new(kind, &c.server, &c.database, authentication).with_login(&c.user, &c.password)
}

fn address(site: &SiteAddress) -> DnnSiteAddress {
    DnnSiteAddress { directory: site.directory.clone(), alias: site.alias.clone(), port: site.port }
}

/// A cancel's "Cancelled.", or the error's message.
fn message(e: SqlError) -> String {
    if e.cancelled { CANCELLED.to_string() } else { e.message }
}

// ─── SqlPasswords.New and LocalSqlContainer's site login ─────────────────────

/// 24 random characters with upper and lower case, digits and symbols - what SQL Server's password policy asks - and
/// nothing a connection string, docker-compose.yml or a command line would need to quote (`SqlPasswords.New`).
pub fn new_sql_password() -> String {
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const DIGITS: &[u8] = b"23456789";
    const SYMBOLS: &[u8] = b"-_.!";
    let all = [UPPER, LOWER, DIGITS, SYMBOLS].concat();
    let mut chars = vec![
        UPPER[random_below(UPPER.len())],
        LOWER[random_below(LOWER.len())],
        DIGITS[random_below(DIGITS.len())],
        SYMBOLS[random_below(SYMBOLS.len())],
    ];
    while chars.len() < 24 {
        chars.push(all[random_below(all.len())]);
    }
    // The four kinds anywhere in it, not always first.
    for i in (1..chars.len()).rev() {
        chars.swap(i, random_below(i + 1));
    }
    String::from_utf8(chars).expect("ASCII")
}

/// A uniform random number below `n` from the system's cryptographic generator (`RandomNumberGenerator.GetInt32`).
fn random_below(n: usize) -> usize {
    let n = n as u64;
    let zone = u64::MAX - u64::MAX % n;
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the system's random numbers");
        let value = u64::from_le_bytes(bytes);
        if value < zone {
            return (value % n) as usize;
        }
    }
}

/// The SQL login a project's site signs in to the container with: `dnn_<project>` (`LocalSqlContainer.SiteLoginFor`).
pub fn site_login_for(project: &str) -> String {
    format!("dnn_{project}")
}

/// The IIS site other than `site` whose web.config signs in to `server` as `login` (`SiteDatabases.OtherSiteSigningInAs`).
fn other_site_signing_in_as(iis: &dyn IisManager, site: &str, server: &str, login: &str) -> Option<String> {
    if login.is_empty() {
        return None;
    }
    let address = SqlServerAddress::parse(server);
    let sites = iis.site_runtimes().ok()?;
    sites.into_iter().filter(|(name, _)| !name.eq_ignore_ascii_case(site)).find_map(|(name, runtime)| {
        let config = web_config::read_database_connection(&runtime.physical_path.join("web.config")).ok()?;
        (config.user.eq_ignore_ascii_case(login) && address.same_server_as(&SqlServerAddress::parse(&config.server)))
            .then_some(name)
    })
}

// ─── ProjectScaffolder ───────────────────────────────────────────────────────

/// The DNN-tuned `.gitignore` written into every new site (`ProjectScaffolder.GitignoreTemplate`).
const GITIGNORE: &[&str] = &[
    "# ---------------------------------------------------------------------",
    "# .gitignore for a DNN (DotNetNuke) site, written by DNN Manager.",
    "# Tracks the site's code, themes and configuration; leaves out build",
    "# output, runtime data (caches, search index, logs), database files and",
    "# portal uploads. A site keeps its own .gitignore once it has one.",
    "# ---------------------------------------------------------------------",
    "",
    "# ===== IDEs =====",
    ".vs/",
    ".vscode/",
    ".idea/",
    "*.suo",
    "*.user",
    "*.userosscache",
    "*.userprefs",
    "*.sln.docstates",
    "*.DotSettings.user",
    "_ReSharper*/",
    "*.[Rr]e[Ss]harper",
    "",
    "# ===== Build output of module/theme source projects =====",
    "# The site's own bin/ stays tracked: it holds the assemblies DNN runs on,",
    "# native libraries in bin/x64 and bin/x86 included.",
    "obj/",
    "**/[Bb]in/[Dd]ebug/",
    "**/[Bb]in/[Rr]elease/",
    "/build/",
    "/bld/",
    "/artifacts/",
    "packages/",
    "*.nupkg",
    "*.snupkg",
    "project.lock.json",
    "project.fragment.lock.json",
    "*.tmp",
    "*.tmp_proj",
    "*.cache",
    "",
    "# ===== Node / front-end tooling =====",
    "node_modules/",
    "npm-debug.log*",
    "yarn-debug.log*",
    "yarn-error.log*",
    "",
    "# ===== OS =====",
    "Thumbs.db",
    "ehthumbs.db",
    "Desktop.ini",
    "$RECYCLE.BIN/",
    ".DS_Store",
    "",
    "# ---------------------------------------------------------------------",
    "# DNN specific",
    "# ---------------------------------------------------------------------",
    "",
    "# ----- Logs (log4net writes Portals/_default/Logs/*.log.resources) -----",
    "*.log",
    "*.log.resources",
    "*.svclog",
    "[Ll]og/",
    "[Ll]ogs/",
    "",
    "# ----- Runtime data in App_Data -----",
    "App_Data/Search/",
    "App_Data/_imagecache/",
    "App_Data/_ipcount/",
    "App_Data/ClientDependency/",
    "App_Data/ExportImport/",
    "App_Data/ExtensionPackages/",
    "App_Data/FipsCompilanceAssemblies/",
    "App_Data/Upgrade/",
    "App_Data/Backup_*/",
    "",
    "# ----- File-based cache -----",
    "Portals/_default/Cache/",
    "",
    "# ----- Database files and backups (they hold the site's data) -----",
    "*.mdf",
    "*.ldf",
    "*.bak",
    "*.bacpac",
    "*.orig",
    "",
    "# ----- Install leftovers -----",
    "# Install/*.resources and Install/<type>/*.resources are DNN's install",
    "# template and bundled packages: they stay tracked.",
    "Install/Temp/",
    "",
    "# ----- Config backups made by upgrades -----",
    "Config/Backup_*/",
    "web.config.bak",
    "web_*.config",
    "*.config.bak",
    "",
    "# ----- Portal folders: uploads stay out, portal themes stay in -----",
    "# Portals/_default holds core themes and templates and stays tracked.",
    "Portals/[0-9]*/*",
    "!Portals/[0-9]*/Skins/",
    "!Portals/[0-9]*/Containers/",
    "!Portals/[0-9]*/portal.css",
    "",
    "# ----- Visual Studio web publishing -----",
    "PublishProfiles/",
    "*.pubxml",
    "*.pubxml.user",
    "*.publishproj",
    "*.azurePubxml",
    "",
    "# ----- Local overrides -----",
    "*.local",
    "*.local.config",
];

/// Writes the `.gitignore` - never over one the site has (`ProjectScaffolder.EnsureGitignore`).
pub fn ensure_gitignore(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let path = directory.join(".gitignore");
    if path.exists() || web_config::has_link(directory, &path) {
        return Ok(());
    }
    std::fs::write(&path, GITIGNORE.join("\r\n")).map_err(|e| {
        log::warn!("Could not write .gitignore to {}: {e}", directory.display());
        e.to_string()
    })
}

// ─── DNN's installer's SQL ───────────────────────────────────────────────────

/// What DNN's installer needs of the site's database (`DnnInstaller`'s queries), over `dnnm-sql`: a LocalDB file
/// through DNN Manager's own instance (`LocalDbFiles.WithDatabaseAsync`), any other database with the site's login.
pub struct SiteSql;

impl SiteSql {
    fn with_database<T: Send + 'static, F: Future<Output = Result<T, SqlError>> + Send + 'static>(
        site: &Path,
        database: &web_config::DatabaseConnection,
        cancel: &CancelToken,
        work: impl FnOnce(Conn) -> F + Send + 'static,
    ) -> Result<T, String> {
        let connection = site_to_sql(database);
        if database.kind == web_config::DatabaseKind::LocalDbFile {
            return dnnm_sql::localdb::with_database(site, &connection, Some(cancel), work).map_err(message);
        }
        let settings = for_app(&connection, None, 30, None);
        block_on(Some(cancel), async move { work(Conn::open(&settings).await?).await }).map_err(message)
    }
}

/// The qualifier of DNN's table `table`, None when the database has no such table (`DnnTables.QualifierAsync`).
async fn qualifier(conn: &mut Conn, table: &str) -> Result<Option<String>, SqlError> {
    Ok(conn
        .scalar(dnn_tables::QUALIFIER_SQL, &[table.into()])
        .await?
        .into_text()
        .filter(|q| dnn_tables::is_qualifier(q)))
}

impl InstallerSql for SiteSql {
    fn raise_compatibility_level(
        &self,
        site: &Path,
        database: &web_config::DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        Self::with_database(site, database, cancel, |mut conn| async move {
            conn.execute(
                "DECLARE @level int = (SELECT compatibility_level FROM sys.databases WHERE name = 'model'); \
                 IF (SELECT compatibility_level FROM sys.databases WHERE database_id = DB_ID()) < @level BEGIN \
                 DECLARE @sql nvarchar(200) = N'ALTER DATABASE CURRENT SET COMPATIBILITY_LEVEL = ' + CAST(@level AS nvarchar(10)); EXEC (@sql); END",
                &[],
            )
            .await
            .map(|_| ())
        })
    }

    fn version(
        &self,
        site: &Path,
        database: &web_config::DatabaseConnection,
        order: VersionOrder,
        cancel: &CancelToken,
    ) -> Result<DbVersion, String> {
        Self::with_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(DbVersion::NoDnnTables) };
            let order = match order {
                VersionOrder::Highest => "Major DESC, Minor DESC, Build DESC",
                VersionOrder::LastWritten => "VersionId DESC",
            };
            let sql =
                format!("SELECT TOP 1 CONCAT(Major, '.', Minor, '.', Build) FROM dbo.[{q}Version] ORDER BY {order}");
            Ok(DbVersion::At(conn.scalar(&sql, &[]).await?.into_text()))
        })
    }

    fn portal_alias_count(
        &self,
        site: &Path,
        database: &web_config::DatabaseConnection,
        alias: &str,
        cancel: &CancelToken,
    ) -> Result<i64, String> {
        let alias = alias.to_string();
        Self::with_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "PortalAlias").await? else { return Ok(0) };
            let sql = format!("SELECT COUNT(*) FROM dbo.[{q}PortalAlias] WHERE PortalID = 0 AND HTTPAlias = @P1");
            Ok(conn.scalar(&sql, &[alias.as_str().into()]).await?.as_i64().unwrap_or(0))
        })
    }

    fn finish_host(
        &self,
        site: &Path,
        database: &web_config::DatabaseConnection,
        user: &str,
        cancel: &CancelToken,
    ) -> Result<i64, String> {
        let user = user.to_string();
        Self::with_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Users").await? else { return Ok(0) };
            // Install.aspx makes the host change its password at the first sign-in; the wizard doesn't. Same for the
            // pages it marks secure.
            let sql = format!(
                "UPDATE dbo.[{q}Users] SET UpdatePassword = 0 WHERE IsSuperUser = 1 AND Username = @P1; SELECT @@ROWCOUNT; \
                 UPDATE dbo.[{q}Tabs] SET IsSecure = 0 WHERE PortalID = 0 AND IsSecure = 1;"
            );
            Ok(conn.scalar(&sql, &[user.as_str().into()]).await?.as_i64().unwrap_or(0))
        })
    }

    fn set_host_password(
        &self,
        _site: &Path,
        _database: &web_config::DatabaseConnection,
        _application_name: &str,
        _user: &str,
        _hash: &str,
        _salt: &str,
        _cancel: &CancelToken,
    ) -> Result<HostPasswordOutcome, String> {
        // New project never changes a host password - Change host password has its own wiring.
        Err("Setting a host password isn't part of New project.".into())
    }

    fn try_connect(&self, database: &web_config::DatabaseConnection) -> Result<(), String> {
        let settings = for_app(&site_to_sql(database), None, 10, None);
        block_on(None, async move { Conn::open(&settings).await.map(|_| ()) }).map_err(message)
    }
}

// ─── The set-up's system ─────────────────────────────────────────────────────

/// `SetupSystem` over the real system.
struct WiredSetup {
    settings: Settings,
    iis: Arc<dyn IisManager>,
    db: AppDatabase,
    releases: ReleaseService,
    packages: Result<PackageInstaller, String>,
    installer: Installer<SiteSql>,
    docker: Result<dnnm_sql::DockerOptions, String>,
}

impl WiredSetup {
    fn new(services: &Services) -> Self {
        Self {
            settings: services.settings.clone(),
            iis: Arc::clone(&services.iis),
            db: services.db.clone(),
            releases: release_service(services),
            packages: package_installer(&services.settings),
            installer: Installer::new(SiteSql),
            docker: crate::remove_project::docker_options(services),
        }
    }

    fn container(&self) -> Result<SqlServerService, String> {
        self.docker.clone().map(SqlServerService::new)
    }
}

impl SetupSystem for WiredSetup {
    fn project(&self, name: &str) -> DnnProject {
        project_of(&self.settings, name)
    }

    fn account_problems(&self, account: &DnnAccount) -> Vec<String> {
        dnn_install::account_problems(&to_dnn_account(account))
    }

    fn get_release(&self, api_url: &str, version: Option<&str>, cancel: &CancelToken) -> Result<DnnRelease, String> {
        self.releases.get_release(api_url, version, cancel)
    }

    fn ensure_iis_features(&self, op: &mut Operation) -> Result<(), String> {
        dnnm_system::prereq::ensure_iis_features(&iis_features(&self.settings), op)
    }

    fn database_exists(&self, database: &DatabaseConnection, cancel: &CancelToken) -> Result<bool, String> {
        DatabaseProvisioner.database_exists(&to_sql(database), Some(cancel))
    }

    fn drop_database(&self, database: &DatabaseConnection, cancel: &CancelToken) -> Result<(), String> {
        DatabaseProvisioner.drop_database(&to_sql(database), Some(cancel))
    }

    fn check_database(
        &self,
        database: &DatabaseConnection,
        options: &DatabaseCheckOptions,
        cancel: &CancelToken,
    ) -> Result<Vec<DatabaseCheck>, String> {
        let options = dnnm_sql::DatabaseCheckOptions {
            for_new_install: true,
            site_login: options.site_login.clone(),
            minimum_major_version: options.minimum_major_version,
            server_only: false,
        };
        let report = DatabaseProvisioner.check(&to_sql(database), &options, Some(cancel))?;
        Ok(report
            .checks
            .into_iter()
            .map(|c| DatabaseCheck {
                name: c.name,
                outcome: match c.outcome {
                    dnnm_sql::CheckOutcome::Passed => CheckOutcome::Passed,
                    dnnm_sql::CheckOutcome::Warning => CheckOutcome::Warning,
                    dnnm_sql::CheckOutcome::Failed => CheckOutcome::Failed,
                },
                detail: c.detail,
            })
            .collect())
    }

    fn download_and_extract(
        &self,
        release: &DnnRelease,
        directory: &Path,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let packages = self.packages.as_ref().map_err(Clone::clone)?;
        packages.download_and_extract(release, directory, reporter, &mut |_| {}, cancel)
    }

    fn ensure_gitignore(&self, directory: &Path) -> Result<(), String> {
        ensure_gitignore(directory)
    }

    fn check_package(&self, directory: &Path) -> Result<(), String> {
        self.installer.check_package(directory)
    }

    fn prepare_local_db_file(
        &self,
        directory: &Path,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        self.installer.prepare_local_db_file(directory, &to_site(database), cancel)
    }

    fn create_database(
        &self,
        database: &DatabaseConnection,
        collation: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        DatabaseProvisioner.create_database(&to_sql(database), collation, Some(cancel))
    }

    fn grant_site_access(
        &self,
        database: &DatabaseConnection,
        identity: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        DatabaseProvisioner.grant_site_access(&to_sql(database), identity, Some(cancel))
    }

    fn grant_site_login(
        &self,
        project: &str,
        database: &str,
        cancel: &CancelToken,
    ) -> Result<(String, String), String> {
        let container = self.container()?;
        let server = format!("{},{}", container.docker.container_ip, container.docker.default_port);
        // dnn_shop, or dnn_shop_2, … when another site signs in with it: a renamed project keeps its login, and a new
        // password for it - or the undo's drop - would lock that site out.
        let mut login = site_login_for(project);
        let mut n = 2;
        while other_site_signing_in_as(self.iis.as_ref(), project, &server, &login).is_some() {
            login = format!("{}_{n}", site_login_for(project));
            n += 1;
        }
        let password = new_sql_password();
        match container.grant_site_login(database, &login, &password, cancel) {
            Ok(()) => Ok((login, password)),
            Err(e) if e == CANCELLED => Err(e),
            Err(e) => Err(format!("Could not make the site's login {login}: {e}")),
        }
    }

    fn drop_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String> {
        self.container()?.drop_login(login, cancel)
    }

    fn write_database_connection(&self, web_config: &Path, database: &DatabaseConnection) -> Result<(), String> {
        crate::change_database_connection::write_site_sql_server(web_config, &for_site(&to_sql(database)))
    }

    fn install(
        &self,
        site: &SiteAddress,
        account: &DnnAccount,
        database: &DatabaseConnection,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let account = to_dnn_account(account);
        self.installer.install(&address(site), &account, &to_site(database), reporter, &mut |_| {}, cancel)
    }

    fn complete(
        &self,
        site: &SiteAddress,
        account: &DnnAccount,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        self.installer.complete(&address(site), &to_dnn_account(account), &to_site(database), cancel)
    }

    fn warm_up(
        &self,
        site: &SiteAddress,
        install_started: SystemTime,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let started = chrono::DateTime::<chrono::Utc>::from(install_started);
        self.installer.warm_up(&address(site), started, reporter, &mut |_| {}, cancel)
    }

    fn clean_up(&self, directory: &Path, installed: bool) {
        self.installer.clean_up(directory, installed);
    }

    fn http_check(&self, url: &str, timeout_seconds: u64, cancel: &CancelToken) -> Result<u16, String> {
        dnnm_dnn::connectivity::check(url, timeout_seconds, cancel)
    }

    fn save_record(&self, record: &ProjectRecord) {
        ProjectTable(self.db.clone()).save(record);
    }
}

/// `iis.requiredFeatures` of the settings - each item's `name` and `label` row - or the defaults.
fn iis_features(settings: &Settings) -> Vec<dnnm_system::prereq::IisFeature> {
    let key = "iis.requiredFeatures";
    let count: usize = settings.rows().get(key).and_then(|c| c.parse().ok()).unwrap_or(0);
    let listed: Vec<_> = (0..count)
        .filter_map(|i| {
            let name = settings.rows().get(&format!("{key}[{i}].name"))?.clone();
            let label = settings.rows().get(&format!("{key}[{i}].label")).cloned().unwrap_or_else(|| name.clone());
            Some(dnnm_system::prereq::IisFeature { name, label })
        })
        .collect();
    if settings.rows().contains_key(key) { listed } else { dnnm_system::prereq::default_features() }
}

// ─── What the New project page asks ──────────────────────────────────────────

/// [`NewProjectLookups`] over the real system - build it with [`page_lookups`] and give it to the page.
pub struct PageLookups {
    settings: Settings,
    db: AppDatabase,
    releases: ReleaseService,
    packages: Result<PackageInstaller, String>,
}

/// What the New project page asks of the system, for the settings and database in `services`.
pub fn page_lookups(services: &Services) -> PageLookups {
    PageLookups {
        settings: services.settings.clone(),
        db: services.db.clone(),
        releases: release_service(services),
        packages: package_installer(&services.settings),
    }
}

impl NewProjectLookups for PageLookups {
    fn release_sources(&self) -> Vec<String> {
        self.releases.known_release_apis().to_vec()
    }

    fn list_releases(&self, api_url: &str, cancel: &CancelToken) -> Result<DnnReleaseList, String> {
        self.releases.list_releases(api_url, cancel)
    }

    fn is_kept(&self, release: &DnnRelease) -> bool {
        self.packages.as_ref().is_ok_and(|p| p.is_kept(release))
    }

    fn keeps_packages(&self) -> bool {
        keeps_packages(&self.settings)
    }

    fn database_exists(&self, database: &DatabaseConnection, cancel: &CancelToken) -> Result<bool, String> {
        DatabaseProvisioner.database_exists(&to_sql(database), Some(cancel))
    }

    fn account_problems(&self, account: &DnnAccount) -> Vec<String> {
        dnn_install::account_problems(&to_dnn_account(account))
    }

    fn generate_password(&self) -> String {
        // DnnAccountRules.GeneratePassword's default length.
        dnn_install::generate_password(16)
    }

    fn project_exists(&self, name: &str) -> bool {
        project_of(&self.settings, name).project_directory.is_dir()
    }

    fn project_directory(&self, name: &str) -> PathBuf {
        project_of(&self.settings, name).project_directory
    }

    fn backup_projects(&self) -> Vec<BackupProject> {
        backup_projects(&app_data_directory().join("backups"))
    }

    fn load_draft(&self) -> BTreeMap<String, String> {
        load_form_draft(&self.db)
    }

    fn save_draft(&self, draft: &BTreeMap<String, String>) {
        save_form_draft(&self.db, draft);
    }

    fn secrets(&self, settings: &Settings) -> PageSecrets {
        use dnnm_system::secrets::{CredentialStore, names, unprotect};
        let sa = settings.get(dnnm_system::secrets::SA_PASSWORD_KEY).unwrap_or_default();
        PageSecrets {
            default_host_password: CredentialStore.read(names::DEFAULT_HOST_PASSWORD),
            database_server_password: CredentialStore.read(names::DATABASE_SERVER_PASSWORD),
            // One that can't be decrypted is the settings' error, said where they are: the page starts without it.
            sa_password: unprotect(sa).unwrap_or_default(),
        }
    }
}

/// The projects in `backups` (one folder each) with a complete backup, by name (`ListProjectsWithBackups` and
/// `ProjectBackups.List`): its zip and database file, newest first, labelled `yyyy-MM-dd HH:mm:ss  (12.3 MB)`.
pub fn backup_projects(backups: &Path) -> Vec<BackupProject> {
    let Ok(entries) = std::fs::read_dir(backups) else { return Vec::new() };
    let mut projects: Vec<BackupProject> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let project = e.file_name().to_string_lossy().into_owned();
            let backups = complete_backups(&e.path());
            (!backups.is_empty()).then_some(BackupProject { project, backups })
        })
        .collect();
    projects.sort_by_key(|p| p.project.to_lowercase());
    projects
}

/// The complete backups in a project's backup folder, newest first.
fn complete_backups(folder: &Path) -> Vec<BackupChoice> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut found: Vec<(String, BackupChoice)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(e.path())
                .ok()?
                .flatten()
                .filter(|f| f.file_type().is_ok_and(|t| t.is_file()))
                .map(|f| f.path())
                .collect();
            files.sort();
            let zip = files.iter().find(|f| has_extension(f, "zip"))?.clone();
            let database = files.iter().filter(|f| is_backup_file(f)).min_by_key(|f| !has_extension(f, "bacpac"))?;
            let name = e.file_name().to_string_lossy().into_owned();
            // "<project>_20260928_154210" - the folder's date, in local time, for anything named by hand.
            let created = match stamp_of(&name) {
                Some(stamp) => stamp_label(&stamp),
                None => {
                    let made = e.metadata().and_then(|m| m.created()).ok()?;
                    chrono::DateTime::<chrono::Local>::from(made).format("%Y-%m-%d %H:%M:%S").to_string()
                }
            };
            let size = (file_size(&zip) + file_size(database)) as f64 / 1024.0 / 1024.0;
            let label = format!("{created}  ({size:.1} MB)");
            Some((created, BackupChoice { label, site_zip: zip, database: database.clone() }))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, choice)| choice).collect()
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

/// A `.bacpac`, or a `.bak` that isn't a copied file such as `web.config.bak` (`LocalSqlContainer.IsBackupFile`).
pub fn is_backup_file(path: &Path) -> bool {
    if has_extension(path, "bacpac") {
        return true;
    }
    if !has_extension(path, "bak") {
        return false;
    }
    const COPIED: [&str; 12] =
        ["config", "json", "xml", "txt", "resources", "resx", "js", "css", "aspx", "ascx", "cs", "dll"];
    let inner = path.file_stem().map(Path::new).and_then(Path::extension).map(|e| e.to_string_lossy().to_lowercase());
    !inner.is_some_and(|e| COPIED.contains(&e.as_str()))
}

/// The `yyyyMMdd_HHmmss` at the end of a backup folder's name.
fn stamp_of(name: &str) -> Option<String> {
    let stamp = name.get(name.len().checked_sub(15)?..)?;
    let ok = stamp.len() == 15
        && name.len() > 15
        && stamp.char_indices().all(|(i, c)| if i == 8 { c == '_' } else { c.is_ascii_digit() });
    ok.then(|| stamp.to_string())
}

/// `yyyyMMdd_HHmmss` as `yyyy-MM-dd HH:mm:ss`.
fn stamp_label(stamp: &str) -> String {
    format!(
        "{}-{}-{} {}:{}:{}",
        &stamp[0..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    )
}

// ─── The page's draft (`FormDraft`, the `forms` state area) ──────────────────

/// The `state` area the WPF app keeps form drafts in (`FormsState.Area`).
pub const FORMS_AREA: &str = "forms";
/// The New project page's name among the drafts.
pub const SETUP_PAGE: &str = "Setup";

fn draft_key(field: &str) -> String {
    format!("drafts{{{SETUP_PAGE}}}{{{field}}}")
}

/// The New project page's saved draft - field name → value - as the WPF app saves it: rows
/// `drafts{Setup}{NameBox}` of the `forms` area. Empty when there is none or it can't be read.
pub fn load_form_draft(db: &AppDatabase) -> BTreeMap<String, String> {
    use_case::DRAFT_FIELDS
        .iter()
        .filter_map(|field| Some((field.to_string(), db.state(FORMS_AREA, &draft_key(field)).ok()??)))
        .collect()
}

/// Saves the page's draft in place of the one before; passwords are never in it. The Settings page's draft in the same
/// area stays as it is. A failure is logged: a draft is a convenience.
pub fn save_form_draft(db: &AppDatabase, draft: &BTreeMap<String, String>) {
    let save = || -> Result<(), String> {
        let page_key = format!("drafts{{{SETUP_PAGE}}}");
        let had_page = db.state(FORMS_AREA, &page_key)?.is_some();
        let mut kept = 0;
        for field in use_case::DRAFT_FIELDS {
            match draft.get(*field) {
                Some(value) => {
                    db.set_state(FORMS_AREA, &draft_key(field), value)?;
                    kept += 1;
                }
                None => db.delete_state(FORMS_AREA, &draft_key(field))?,
            }
        }
        db.set_state(FORMS_AREA, &page_key, &kept.to_string())?;
        if !had_page {
            let pages: usize = db.state(FORMS_AREA, "drafts")?.and_then(|n| n.parse().ok()).unwrap_or(0);
            db.set_state(FORMS_AREA, "drafts", &(pages + 1).to_string())?;
        }
        Ok(())
    };
    if let Err(e) = save() {
        log::warn!("Could not save the New project page's draft: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_passwords_have_every_kind_and_nothing_to_quote() {
        for _ in 0..50 {
            let p = new_sql_password();
            assert_eq!(p.len(), 24);
            assert!(p.chars().any(|c| c.is_ascii_uppercase()) && p.chars().any(|c| c.is_ascii_lowercase()));
            assert!(p.chars().any(|c| c.is_ascii_digit()) && p.chars().any(|c| "-_.!".contains(c)));
            assert!(!p.chars().any(|c| ";='\"$ ".contains(c)));
        }
        assert_ne!(new_sql_password(), new_sql_password());
    }

    #[test]
    fn the_draft_round_trips_through_the_forms_area_and_keeps_other_pages() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        db.set_state(FORMS_AREA, "drafts", "1").unwrap();
        db.set_state(FORMS_AREA, "drafts{Settings}", "1").unwrap();
        db.set_state(FORMS_AREA, "drafts{Settings}{PortBox}", "8080").unwrap();
        assert!(load_form_draft(&db).is_empty());

        let mut draft = BTreeMap::new();
        draft.insert("NameBox".to_string(), "shop".to_string());
        draft.insert("FromScratch".to_string(), "true".to_string());
        draft.insert("HostPasswordBox".to_string(), "never kept".to_string());
        save_form_draft(&db, &draft);
        assert_eq!(db.state(FORMS_AREA, "drafts{Setup}{NameBox}").unwrap().as_deref(), Some("shop"));
        assert_eq!(db.state(FORMS_AREA, "drafts{Setup}").unwrap().as_deref(), Some("2"));
        assert_eq!(db.state(FORMS_AREA, "drafts").unwrap().as_deref(), Some("2"));
        assert_eq!(db.state(FORMS_AREA, "drafts{Setup}{HostPasswordBox}").unwrap(), None);
        assert_eq!(db.state(FORMS_AREA, "drafts{Settings}{PortBox}").unwrap().as_deref(), Some("8080"));
        draft.remove("HostPasswordBox");
        assert_eq!(load_form_draft(&db), draft);

        draft.remove("FromScratch");
        save_form_draft(&db, &draft);
        assert_eq!(db.state(FORMS_AREA, "drafts{Setup}{FromScratch}").unwrap(), None);
        assert_eq!(db.state(FORMS_AREA, "drafts").unwrap().as_deref(), Some("2"), "the page is counted once");
    }

    #[test]
    fn backups_list_complete_ones_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let make = |folder: &str, files: &[&str]| {
            let path = dir.path().join("shop").join(folder);
            std::fs::create_dir_all(&path).unwrap();
            for f in files {
                std::fs::write(path.join(f), vec![0u8; 1024 * 1024]).unwrap();
            }
        };
        make("shop_20260101_101010", &["shop.zip", "shop.bacpac"]);
        make("shop_20261103_090807", &["shop.zip", "shop.bak", "shop.bacpac"]);
        make("shop_20261104_000000", &["shop.zip", "web.config.bak"]);
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        let projects = backup_projects(dir.path());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project, "shop");
        let labels: Vec<&str> = projects[0].backups.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, vec!["2026-11-03 09:08:07  (2.0 MB)", "2026-01-01 10:10:10  (2.0 MB)"]);
        assert!(projects[0].backups[0].database.ends_with("shop.bacpac"));
        assert!(is_backup_file(Path::new("x.bak")) && !is_backup_file(Path::new("web.config.bak")));
    }

    #[test]
    fn the_gitignore_is_written_once_with_crlf() {
        let dir = tempfile::tempdir().unwrap();
        ensure_gitignore(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(
            text.starts_with("# ----") && text.contains("\r\nApp_Data/Search/\r\n") && text.ends_with("*.local.config")
        );
        std::fs::write(dir.path().join(".gitignore"), "mine").unwrap();
        ensure_gitignore(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(), "mine");
    }

    #[test]
    fn features_come_from_the_settings_or_the_defaults() {
        assert_eq!(iis_features(&Settings::default()).len(), dnnm_system::prereq::default_features().len());
        let s = Settings::from_rows([
            ("iis.requiredFeatures".to_string(), "1".to_string()),
            ("iis.requiredFeatures[0].name".to_string(), "IIS-ASPNET45".to_string()),
            ("iis.requiredFeatures[0].label".to_string(), "ASP.NET 4.8".to_string()),
        ]);
        let features = iis_features(&s);
        assert_eq!((features[0].name.as_str(), features[0].label.as_str()), ("IIS-ASPNET45", "ASP.NET 4.8"));
    }
}
