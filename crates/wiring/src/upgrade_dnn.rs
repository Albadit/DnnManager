//! Upgrade DNN joined to what it acts on (`UpgradeDnnUseCase`'s collaborators in the C# app's DI): the site inspector,
//! the analyser, the checks, DNN's installer and its packages from `dnnm-dnn`; SQL Server through `dnnm-sql` (the
//! [`SiteSql`] and [`InstallerSql`] queries of `DnnSiteInspector`, `DnnUpgradeChecks` and `DnnInstaller`); each step's
//! backup and its way back through Export and Restore backup. The texts of its menu item, button and operation
//! (`ProjectMenu.cs`, `ProjectView.xaml`, `ProjectEdits.UpgradeDnn`, `MainWindow.Commands.cs`), word for word.
//!
//! The use case reads the site's database from its web.config (the C#'s `LocalSqlContainer.DatabaseOf` signs a
//! container database in as the container's user; the site's own login, db_owner of its database, reads the same).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use dnnm_application::operation::{CANCELLED, CancelToken, Operation, Prompt, Reporter};
use dnnm_application::records::ProjectRecords;
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::backups::Stamp;
use dnnm_application::use_cases::export_project::ExportProjectRequest;
use dnnm_application::use_cases::upgrade_dnn::{
    self as use_case, BeforeNotes, Diagnosis, DnnNumber, Finding, PlanStep, Severity, SiteAddress, SiteDatabase,
    StepMethod, StepRestore, UpgradeDnnDeps, UpgradeDnnRequest, UpgradePlan, UpgradeSystem,
};
use dnnm_dnn::install::DnnSiteAddress;
use dnnm_dnn::installer::{DbVersion, HostPasswordOutcome, Installer, InstallerSql, VersionOrder};
use dnnm_dnn::packages::{PackageInstaller, PackageOptions};
use dnnm_dnn::releases::{GitHubHttp, ReleaseService, ReleaseStore};
use dnnm_dnn::site_inspector::{SiteIis, SiteInspector};
use dnnm_dnn::site_sql::{PackageRow, ServerFacts, SitePages, SiteSql};
use dnnm_dnn::upgrade_checks::UpgradeChecks;
use dnnm_dnn::upgrades::{
    self, DnnContentCounts, DnnContentItem, DnnSiteState, DnnUpgradeMethod, DnnUpgradePlan, UpgradeFinding,
    UpgradeFindingSeverity, UpgradeStop, upgrade_stop_version,
};
use dnnm_dnn::version::Version;
use dnnm_domain::{DnnRelease, DnnReleaseList};
use dnnm_infrastructure::app_db::ProjectTable;
use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind, read_database_connection};
use dnnm_sql::client::{Conn, SqlError, Value, block_on, values};
use dnnm_sql::connection_strings::{for_app, for_server};
use dnnm_sql::sql_text::dnn_tables;

use crate::Services;

/// The menu item, button and command palette's texts, word for word.
pub mod texts {
    /// The project menu's item, the Details' button and the command (`project.upgradeDnn`).
    pub const UPGRADE_DNN: &str = "Upgrade DNN…";
    pub const COMMAND: &str = "project.upgradeDnn";
    /// The command palette's search words.
    pub const COMMAND_KEYWORDS: &str = "update version release newer platform backup";
    /// The Details' button's tooltip (`ProjectView.xaml`).
    pub const BUTTON_TOOLTIP: &str = "To a newer DNN, with a backup before each step";
    // The project header's More button mentions it: "More actions: logs, clear cache, export, backups, upgrade".

    /// The project menu's tooltip (`ProjectMenu.UpgradeItem`): `dnn` as the row shows it.
    pub fn menu_tooltip(dnn: &str) -> String {
        format!("From DNN {dnn}, with a backup before each step.")
    }

    /// The operation's title (`ProjectEdits.UpgradeDnn`): `version` as the dialog chose it (its release's version).
    pub fn operation_title(site: &str, version: &str) -> String {
        format!("Upgrade DNN of '{site}' to {version}")
    }

    /// The toast once it has finished.
    pub fn done(site: &str, version: &str) -> String {
        format!("'{site}' runs DNN {version} - the backup of every step stays in its backups folder.")
    }
}

/// `projects.dnnReleaseSources`'s defaults (`UserSettings.DnnReleaseSources`).
const DEFAULT_RELEASE_SOURCES: [&str; 2] = [
    "https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases",
    "https://api.github.com/repos/DNN-Connect/Dnn.Platform/releases",
];

/// The repositories the dialog offers (`IDnnReleaseService.KnownReleaseApis`).
pub fn release_apis(settings: &Settings) -> Vec<String> {
    settings.list("projects.dnnReleaseSources").unwrap_or_else(|| DEFAULT_RELEASE_SOURCES.map(String::from).to_vec())
}

/// The path's edits saved in Settings (`projects.dnnUpgradeStops`) - none when they break the rules: a saved value a
/// rule refuses is reset at the next start.
pub fn upgrade_stops(settings: &Settings) -> Vec<UpgradeStop> {
    const KEY: &str = "projects.dnnUpgradeStops";
    let rows = settings.rows();
    let count: usize = rows.get(KEY).and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    let stops: Vec<UpgradeStop> = (0..count)
        .map(|i| {
            let field = |name: &str| rows.get(&format!("{KEY}[{i}].{name}")).cloned().unwrap_or_default();
            UpgradeStop {
                version: field("version"),
                net_framework_release: field("netFrameworkRelease").trim().parse().unwrap_or(0),
                sql_server_major: field("sqlServerMajor").trim().parse().unwrap_or(0),
                note: field("note"),
                replaces_listed: field("replacesListed").trim().eq_ignore_ascii_case("true"),
                removed: field("removed").trim().eq_ignore_ascii_case("true"),
            }
        })
        .collect();
    if upgrades::upgrade_stops_problem(&stops).is_some() { Vec::new() } else { stops }
}

/// The releases of `api`, highest version first - for the dialog's versions (`DnnReleaseCatalog.GetAsync`; the app
/// keeps what it got). Reaches GitHub: off the UI thread.
pub fn list_releases(services: &Services, api: &str) -> Result<DnnReleaseList, String> {
    release_service(services).list_releases(api, &CancelToken::new())
}

/// The plan the dialog shows (`UpgradeDnnUseCase.PlanAsync`): `target` as a release's version. Reads the site, its
/// database and this PC, and may ask to sign in to the database's server: off the UI thread.
pub fn plan_upgrade(
    services: &Services,
    site: &str,
    directory: &Path,
    target: &str,
    prompt: &dyn Prompt,
) -> Result<UpgradePlan, String> {
    let Some(target) = DnnNumber::of(target) else { return Err(format!("'{target}' isn't a DNN version.")) };
    use_case::plan(&deps(services), site, directory, target, prompt)
}

/// The upgrade, run as an operation titled [`texts::operation_title`].
pub fn upgrade_dnn(services: &Services, request: UpgradeDnnRequest, op: &mut Operation) -> Result<(), String> {
    use_case::upgrade_dnn(&deps(services), &request, op)
}

fn deps(services: &Services) -> UpgradeDnnDeps<DnnUpgrades> {
    UpgradeDnnDeps {
        iis: Arc::clone(&services.iis),
        records: Arc::new(ProjectTable(services.db.clone())) as Arc<dyn ProjectRecords>,
        system: Arc::new(DnnUpgrades { services: services.clone(), stops: upgrade_stops(&services.settings) }),
    }
}

fn release_service(services: &Services) -> ReleaseService {
    ReleaseService::new(GitHubHttp::default(), ReleaseStore::new(services.db.path()), release_apis(&services.settings))
}

/// What the upgrade acts on, from the `dnnm-dnn` and `dnnm-sql` crates and the backup use cases.
pub struct DnnUpgrades {
    services: Services,
    stops: Vec<UpgradeStop>,
}

fn version_of(number: DnnNumber) -> Version {
    Version::new(number.major, number.minor, number.build)
}

fn number_of(version: &Version) -> DnnNumber {
    DnnNumber::new(version.major, version.minor, version.build_or_zero())
}

fn finding(f: &UpgradeFinding) -> Finding {
    Finding {
        severity: match f.severity {
            UpgradeFindingSeverity::Compatible => Severity::Compatible,
            UpgradeFindingSeverity::Warning => Severity::Warning,
            UpgradeFindingSeverity::Blocking => Severity::Blocking,
            UpgradeFindingSeverity::Unknown => Severity::Unknown,
        },
        area: f.area.clone(),
        title: f.title.clone(),
        detail: f.detail.clone(),
        fix: f.fix.clone(),
    }
}

/// `dnnm-dnn`'s plan as the use case and the dialog see it.
pub fn plan_of(plan: &DnnUpgradePlan) -> UpgradePlan {
    UpgradePlan {
        current: number_of(&plan.current),
        target: number_of(&plan.target),
        steps: plan
            .steps
            .iter()
            .map(|s| PlanStep {
                from: number_of(&s.step.from),
                to: number_of(&s.step.to),
                method: match s.step.method {
                    DnnUpgradeMethod::UpgradePackage => StepMethod::UpgradePackage,
                    DnnUpgradeMethod::LocalUpgrade => StepMethod::LocalUpgrade,
                    DnnUpgradeMethod::Manual => StepMethod::Manual,
                },
                checks: s.checks.iter().map(finding).collect(),
            })
            .collect(),
        site: plan.site.iter().map(finding).collect(),
        description: plan.describe(),
    }
}

/// Whether `version` is on the path because of an edit in Settings.
fn added(stops: &[UpgradeStop], version: DnnNumber) -> bool {
    stops.iter().any(|s| !s.removed && upgrade_stop_version(&s.version) == Some(version_of(version)))
}

fn utc(at: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(at)
}

fn address(site: &SiteAddress) -> DnnSiteAddress {
    DnnSiteAddress { directory: site.directory.clone(), alias: site.alias.clone(), port: site.port }
}

fn web_config_database(directory: &Path) -> Result<DatabaseConnection, String> {
    read_database_connection(&directory.join("web.config"))
}

/// The settings' database server (`DatabaseServerOptions`), for whom DNN Manager may sign in as the user.
fn database_server(settings: &Settings) -> dnnm_sql::model::DatabaseServerOptions {
    dnnm_sql::model::DatabaseServerOptions {
        kind: settings.get("sqlServer.type").unwrap_or("container").to_string(),
        server: settings.get("sqlServer.server").unwrap_or_default().to_string(),
    }
}

impl UpgradeSystem for DnnUpgrades {
    type Before = DnnSiteState;

    fn database(&self, _: &str, directory: &Path) -> Option<SiteDatabase> {
        let database = web_config_database(directory).ok()?;
        Some(SiteDatabase {
            server: database.server.clone(),
            windows_authentication: database.windows_authentication,
            local_db_file: database.kind == DatabaseKind::LocalDbFile,
        })
    }

    fn may_sign_in_as_user(&self, server: &str) -> bool {
        dnnm_sql::SqlServerAddress::may_sign_in_as_user(server, &database_server(&self.services.settings))
    }

    fn sign_in_question(&self, server: &str, why: &str) -> String {
        dnnm_sql::SqlServerAddress::sign_in_question(server, why)
    }

    fn plan(&self, site_name: &str, directory: &Path, target: DnnNumber) -> UpgradePlan {
        let database = web_config_database(directory).ok();
        let iis = self.services.iis.site_details(site_name).map(|details| SiteIis {
            app_pool_clr: details.pool.as_ref().map(|p| p.runtime.clone()),
            app_pool_pipeline: details.pool.as_ref().map(|p| p.pipeline.clone()),
            has_host_binding: use_case::address_of(&details, directory).is_some(),
        });
        let inspector = SiteInspector::new(SqlSite, crate::backups::backups_root());
        let facts = inspector.inspect(site_name, directory, database.as_ref(), iis.as_ref());
        plan_of(&upgrades::plan(&facts, version_of(target), &self.stops))
    }

    fn added_in_settings(&self, version: DnnNumber) -> bool {
        added(&self.stops, version)
    }

    fn list_releases(&self, api_url: &str, cancel: &CancelToken) -> Result<Vec<DnnRelease>, String> {
        release_service(&self.services).list_releases(api_url, cancel).map(|list| list.releases)
    }

    fn get_release(&self, api_url: &str, version: &str, cancel: &CancelToken) -> Result<DnnRelease, String> {
        release_service(&self.services).get_release(api_url, Some(version), cancel)
    }

    fn backup_directory(&self, site_name: &str) -> PathBuf {
        crate::backups::backups_folder(site_name)
    }

    fn now(&self) -> Stamp {
        crate::backups::now()
    }

    fn export(&self, request: &ExportProjectRequest, op: &mut Operation) -> Result<(), String> {
        crate::export_project::export_project(&self.services, request.clone(), op)
    }

    fn restore(&self, request: &StepRestore, op: &mut Operation) -> Result<(), String> {
        put_back(&self.services, request, op)
    }

    fn capture(&self, site: &SiteAddress, cancel: &CancelToken) -> Result<DnnSiteState, String> {
        let database = web_config_database(&site.directory)?;
        UpgradeChecks::new(SqlSite).capture(&address(site), &database, cancel)
    }

    fn notes_of(&self, before: &DnnSiteState) -> BeforeNotes {
        BeforeNotes {
            counts: before.counts.map(|c| c.to_string()),
            recorded: before.items.iter().filter(|i| i.kind != "type").count(),
            extensions: before.extensions.clone(),
            pages: before.pages.iter().map(|p| (p.url.clone(), p.problem.clone())).collect(),
        }
    }

    fn validate(
        &self,
        site: &SiteAddress,
        before: &DnnSiteState,
        expected: DnnNumber,
        since: SystemTime,
        app_pool: &str,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<Vec<Finding>, String> {
        let database = web_config_database(&site.directory)?;
        let found = UpgradeChecks::new(SqlSite).validate(
            &address(site),
            &database,
            before,
            version_of(expected),
            utc(since),
            &[],
            app_pool,
            reporter,
            cancel,
        )?;
        Ok(found.iter().map(finding).collect())
    }

    fn collect_diagnostics(
        &self,
        directory: &Path,
        app_pool: &str,
        since: SystemTime,
        folder: &Path,
    ) -> Result<Vec<String>, String> {
        UpgradeChecks::new(SqlSite).collect_diagnostics(directory, app_pool, utc(since), folder, &CancelToken::new())
    }

    fn explain(&self, error: &str, evidence: &[String]) -> Diagnosis {
        let diagnosis = upgrades::explain(error, evidence);
        Diagnosis { cause: diagnosis.cause, fixes: diagnosis.fixes }
    }

    fn extract(
        &self,
        release: &DnnRelease,
        directory: &Path,
        local_upgrade: bool,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let private_temp = dnnm_system::files::private_temp::path().map_err(|e| e.to_string())?;
        let packages = PackageInstaller::new(PackageOptions {
            keep_packages: self
                .services
                .settings
                .get("projects.keepDnnPackages")
                .is_some_and(|v| v.eq_ignore_ascii_case("true")),
            packages_directory: dnnm_infrastructure::paths::app_data_directory().join("packages"),
            private_temp,
        });
        // The port's reporter has no transient status line: the packages' progress isn't shown.
        let mut progress = |_: &str| {};
        if local_upgrade {
            packages.extract_local_upgrade(release, directory, reporter, &mut progress, cancel)
        } else {
            packages.extract_upgrade(release, directory, reporter, &mut progress, cancel)
        }
    }

    fn upgrade(
        &self,
        site: &SiteAddress,
        output: &Path,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let database = web_config_database(&site.directory)?;
        let mut progress = |_: &str| {};
        Installer::new(SqlSite).upgrade(
            &address(site),
            &database.password,
            reporter,
            &mut progress,
            cancel,
            Some(output),
        )
    }

    fn warm_up(
        &self,
        site: &SiteAddress,
        since: SystemTime,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let mut progress = |_: &str| {};
        Installer::new(SqlSite).warm_up(&address(site), utc(since), reporter, &mut progress, cancel)
    }

    fn clean_up(&self, directory: &Path) {
        Installer::new(SqlSite).clean_up(directory, true);
    }
}

/// A step's backup put back (`RestoreBackupUseCase`, the user's agreement to sign in given before the upgrade began).
fn put_back(services: &Services, request: &StepRestore, op: &mut Operation) -> Result<(), String> {
    let request = dnnm_application::use_cases::restore_backup::RestoreBackupRequest {
        site_name: request.site_name.clone(),
        directory: request.directory.clone(),
        site_zip: request.site_zip.clone(),
        database: request.database.clone(),
        sign_in_agreed: true,
    };
    crate::restore_backup::restore_backup_request(services, request, op)
}

// ─── SQL Server ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The queries the upgrade's analyser, checks and DNN's installer run on a site's database, through `dnnm-sql`
/// (`DnnSiteInspector`, `DnnUpgradeChecks`, `DnnInstaller`). Values go as parameters; DNN's table names with the
/// object qualifier found from its tables, which is letters, digits and `_` only.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlSite;

fn sql_connection(c: &DatabaseConnection) -> dnnm_sql::model::DatabaseConnection {
    use dnnm_sql::model::{DatabaseConnection as Sql, DatabaseKind as Kind, SqlAuthentication as Auth};
    let kind = if c.kind == DatabaseKind::LocalDbFile { Kind::LocalDbFile } else { Kind::SqlServer };
    let authentication = if c.windows_authentication { Auth::Windows } else { Auth::Sql };
    Sql::new(kind, &c.server, &c.database, authentication).with_login(&c.user, &c.password)
}

fn error(e: SqlError) -> String {
    if e.cancelled { CANCELLED.to_string() } else { e.message }
}

/// Runs `work` on the database, opened as DNN Manager's own connection within `timeout` seconds.
fn on_database<T, F>(
    database: &DatabaseConnection,
    timeout: u64,
    cancel: Option<&CancelToken>,
    work: impl FnOnce(Conn) -> F + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
    F: Future<Output = Result<T, SqlError>> + Send + 'static,
{
    let settings = for_app(&sql_connection(database), None, timeout, None);
    block_on(cancel, async move { work(Conn::open(&settings).await?).await }).map_err(error)
}

/// Runs `work` on the site's database - a LocalDB file through DNN Manager's own instance (`WithSiteDatabaseAsync`).
fn on_site_database<T, F>(
    site: &Path,
    database: &DatabaseConnection,
    cancel: &CancelToken,
    work: impl FnOnce(Conn) -> F + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
    F: Future<Output = Result<T, SqlError>> + Send + 'static,
{
    if database.kind == DatabaseKind::LocalDbFile {
        return dnnm_sql::localdb::with_database(site, &sql_connection(database), Some(cancel), work).map_err(error);
    }
    on_database(database, 30, Some(cancel), work)
}

/// The qualifier of DNN's table `table` ("" or "dnn_"); None when the database has no such table (`DnnTables`).
async fn qualifier(conn: &mut Conn, table: &str) -> Result<Option<String>, SqlError> {
    Ok(conn
        .scalar(dnn_tables::QUALIFIER_SQL, &[table.into()])
        .await?
        .into_text()
        .filter(|q| dnn_tables::is_qualifier(q)))
}

fn text(value: &Value) -> String {
    value.as_text().map(str::to_string).unwrap_or_default()
}

fn int(value: &Value) -> i32 {
    value.as_i64().unwrap_or(0) as i32
}

const HIGHEST_VERSION: &str =
    "SELECT TOP 1 CONCAT(Major, '.', Minor, '.', Build) FROM {} ORDER BY Major DESC, Minor DESC, Build DESC";

async fn dnn_version(conn: &mut Conn) -> Result<Option<String>, SqlError> {
    let Some(q) = qualifier(conn, "Version").await? else { return Ok(None) };
    let sql = HIGHEST_VERSION.replace("{}", &dnn_tables::name(&q, "Version"));
    Ok(conn.scalar(&sql, &[]).await?.into_text())
}

async fn counts(conn: &mut Conn, q: &str) -> Result<DnnContentCounts, SqlError> {
    async fn count(conn: &mut Conn, q: &str, table: &str, filter: &str) -> Result<i32, SqlError> {
        // A table an older DNN doesn't have counts as none.
        let name = dnn_tables::name(q, table);
        let sql = format!("IF OBJECT_ID('{name}') IS NULL SELECT 0 ELSE SELECT COUNT(*) FROM {name} {filter}");
        Ok(int(&conn.scalar(&sql, &[]).await?))
    }
    Ok(DnnContentCounts {
        portals: count(conn, q, "Portals", "").await?,
        users: count(conn, q, "Users", "").await?,
        roles: count(conn, q, "Roles", "").await?,
        user_roles: count(conn, q, "UserRoles", "").await?,
        pages: count(conn, q, "Tabs", "WHERE IsDeleted = 0").await?,
        modules: count(conn, q, "Modules", "").await?,
        permissions: count(conn, q, "TabPermission", "").await?
            + count(conn, q, "ModulePermission", "").await?
            + count(conn, q, "FolderPermission", "").await?,
        scheduled_jobs: count(conn, q, "Schedule", "").await?,
        extensions: count(conn, q, "Packages", "").await?,
    })
}

impl SiteSql for SqlSite {
    fn server_facts(&self, database: &DatabaseConnection) -> Result<ServerFacts, String> {
        on_database(database, 15, None, |mut conn| async move {
            let rows = conn
                .rows(
                    "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(64)), CAST(SERVERPROPERTY('Edition') AS nvarchar(128)), \
                     CAST(DATABASEPROPERTYEX(DB_NAME(), 'Status') AS nvarchar(64))",
                    &[],
                )
                .await?;
            let row = rows.first().map(values).ok_or_else(|| SqlError::new("No answer to SERVERPROPERTY."))?;
            // Its data files' size, for the room the backups need.
            let bytes = conn
                .scalar(
                    "SELECT CAST(ISNULL(SUM(CAST(size AS bigint)), 0) * 8192 AS bigint) FROM sys.database_files WHERE type = 0",
                    &[],
                )
                .await?;
            Ok(ServerFacts {
                version: text(&row[0]),
                edition: text(&row[1]),
                state: text(&row[2]),
                database_bytes: bytes.as_i64().unwrap_or(0),
            })
        })
    }

    fn dnn_version(&self, database: &DatabaseConnection) -> Result<Option<String>, String> {
        on_database(database, 20, None, |mut conn| async move { dnn_version(&mut conn).await })
    }

    fn counts(&self, database: &DatabaseConnection) -> Result<Option<DnnContentCounts>, String> {
        on_database(database, 15, None, |mut conn| async move {
            match qualifier(&mut conn, "Version").await? {
                Some(q) => Ok(Some(counts(&mut conn, &q).await?)),
                None => Ok(None),
            }
        })
    }

    fn packages(&self, database: &DatabaseConnection) -> Result<Vec<PackageRow>, String> {
        on_database(database, 15, None, |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(Vec::new()) };
            // Organization and IsSystemPackage came with DNN 5 / 6 - read what this database has.
            let columns: Vec<String> = conn
                .rows(
                    "SELECT name FROM sys.columns WHERE object_id = OBJECT_ID(@P1)",
                    &[format!("dbo.{q}Packages").as_str().into()],
                )
                .await?
                .iter()
                .map(|r| text(&values(r)[0]).to_lowercase())
                .collect();
            let owner = if columns.iter().any(|c| c == "organization") { "Organization" } else { "NULL" };
            let system =
                if columns.iter().any(|c| c == "issystempackage") { "IsSystemPackage" } else { "CAST(0 AS bit)" };
            let sql = format!(
                "SELECT Name, FriendlyName, PackageType, Version, {owner}, {system} FROM {} ORDER BY PackageType, Name",
                dnn_tables::name(&q, "Packages")
            );
            Ok(conn
                .rows(&sql, &[])
                .await?
                .iter()
                .map(|r| {
                    let cells = values(r);
                    PackageRow {
                        name: text(&cells[0]),
                        friendly_name: cells[1].as_text().map(str::to_string),
                        package_type: text(&cells[2]),
                        version: text(&cells[3]),
                        organization: cells[4].as_text().map(str::to_string),
                        is_system_package: cells[5].as_i64() == Some(1),
                    }
                })
                .collect())
        })
    }

    fn registered_assemblies(&self, database: &DatabaseConnection) -> Result<Vec<String>, String> {
        on_database(database, 15, None, |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(Vec::new()) };
            let name = dnn_tables::name(&q, "Assemblies");
            let sql = format!("IF OBJECT_ID('{name}') IS NOT NULL SELECT AssemblyName FROM {name}");
            Ok(conn.rows(&sql, &[]).await?.iter().map(|r| text(&values(r)[0])).collect())
        })
    }

    fn site_pages(&self, database: &DatabaseConnection, per_portal: usize) -> Result<SitePages, String> {
        on_database(database, 20, None, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(SitePages::default()) };
            let extensions = conn
                .rows(&format!("SELECT Name FROM {}", dnn_tables::name(&q, "Packages")), &[])
                .await?
                .iter()
                .map(|r| text(&values(r)[0]))
                .collect();
            let aliases: Vec<(i32, String)> = conn
                .rows(
                    &format!(
                        "SELECT PortalID, HTTPAlias FROM {} ORDER BY PortalID, IsPrimary DESC, PortalAliasID",
                        dnn_tables::name(&q, "PortalAlias")
                    ),
                    &[],
                )
                .await?
                .iter()
                .map(|r| {
                    let cells = values(r);
                    (int(&cells[0]), text(&cells[1]))
                })
                .collect();
            let mut portals: Vec<i32> = aliases.iter().map(|a| a.0).collect();
            portals.dedup();
            let mut tabs = std::collections::BTreeMap::new();
            for portal in portals {
                // A portal's number from the database, written as a number.
                let sql = format!(
                    "SELECT TOP {per_portal} TabID FROM {} WHERE PortalID = {portal} AND IsDeleted = 0 AND IsVisible = 1 \
                     AND DisableLink = 0 AND ISNULL(Url, '') = '' AND TabPath NOT LIKE '//Admin%' AND TabPath NOT LIKE '//Host%' ORDER BY Level, TabOrder",
                    dnn_tables::name(&q, "Tabs")
                );
                let ids: Vec<i32> = conn.rows(&sql, &[]).await?.iter().map(|r| int(&values(r)[0])).collect();
                tabs.insert(portal, ids);
            }
            Ok(SitePages { extensions, aliases, tabs })
        })
    }

    fn content_items(&self, database: &DatabaseConnection) -> Result<Vec<DnnContentItem>, String> {
        on_database(database, 30, None, |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(Vec::new()) };
            let t = |table: &str| dnn_tables::name(&q, table);
            let limit = std::time::Duration::from_secs(120);
            let mut items = Vec::new();
            let sql = format!(
                "SELECT t.TabID, t.TabPath, (SELECT COUNT(*) FROM {} p WHERE p.TabID = t.TabID) FROM {} t WHERE t.IsDeleted = 0",
                t("TabPermission"),
                t("Tabs")
            );
            for r in conn.rows_within(&sql, &[], limit).await? {
                let c = values(&r);
                let id = int(&c[0]);
                let name = c[1].as_text().map(str::to_string).unwrap_or_else(|| format!("#{id}"));
                items.push(DnnContentItem { permissions: int(&c[2]), ..DnnContentItem::new("page", id, &name) });
            }
            // A module's extension: its package - or, for DNN's own modules that have none (DNN 8's 'Navigation'), its
            // module type, listed as a "type" item of its own so the comparison sees it gone with the upgrade.
            let sql = format!(
                "SELECT m.ModuleID, ISNULL(MIN(tm.ModuleTitle), ''), MIN(tm.TabID), ISNULL(MIN(pk.Name), MIN(dm.ModuleName)), \
                 (SELECT COUNT(*) FROM {} mp WHERE mp.ModuleID = m.ModuleID) \
                 FROM {} m JOIN {} md ON md.ModuleDefID = m.ModuleDefID \
                 JOIN {} dm ON dm.DesktopModuleID = md.DesktopModuleID LEFT JOIN {} pk ON pk.PackageID = dm.PackageID \
                 LEFT JOIN {} tm ON tm.ModuleID = m.ModuleID AND tm.IsDeleted = 0 WHERE m.IsDeleted = 0 GROUP BY m.ModuleID",
                t("ModulePermission"),
                t("Modules"),
                t("ModuleDefinitions"),
                t("DesktopModules"),
                t("Packages"),
                t("TabModules")
            );
            for r in conn.rows_within(&sql, &[], limit).await? {
                let c = values(&r);
                let id = int(&c[0]);
                items.push(DnnContentItem {
                    package: c[3].as_text().map(str::to_string),
                    page_id: c[2].as_i64().map(|p| p as i32),
                    permissions: int(&c[4]),
                    ..DnnContentItem::new("module", id, &format!("'{}' (#{id})", text(&c[1])))
                });
            }
            for r in conn
                .rows_within(&format!("SELECT DesktopModuleID, ModuleName FROM {}", t("DesktopModules")), &[], limit)
                .await?
            {
                let c = values(&r);
                items.push(DnnContentItem::new("type", int(&c[0]), &text(&c[1])));
            }
            for r in
                conn.rows_within(&format!("SELECT ScheduleID, TypeFullName FROM {}", t("Schedule")), &[], limit).await?
            {
                let c = values(&r);
                let id = int(&c[0]);
                let name = c[1].as_text().map(str::to_string).unwrap_or_else(|| format!("#{id}"));
                items.push(DnnContentItem::new("job", id, &name));
            }
            Ok(items)
        })
    }
}

impl InstallerSql for SqlSite {
    fn raise_compatibility_level(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        // The shipped file is at level 100 (SQL Server 2008); DNN 10's HTML module needs 130+ (STRING_SPLIT).
        on_site_database(site, database, cancel, |mut conn| async move {
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
        database: &DatabaseConnection,
        order: VersionOrder,
        cancel: &CancelToken,
    ) -> Result<DbVersion, String> {
        on_site_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(DbVersion::NoDnnTables) };
            let order = match order {
                VersionOrder::Highest => "Major DESC, Minor DESC, Build DESC",
                VersionOrder::LastWritten => "VersionId DESC",
            };
            let sql = format!(
                "SELECT TOP 1 CONCAT(Major, '.', Minor, '.', Build) FROM {} ORDER BY {order}",
                dnn_tables::name(&q, "Version")
            );
            Ok(DbVersion::At(conn.scalar(&sql, &[]).await?.into_text()))
        })
    }

    fn portal_alias_count(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        alias: &str,
        cancel: &CancelToken,
    ) -> Result<i64, String> {
        let alias = alias.to_string();
        on_site_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(0) };
            let sql = format!(
                "SELECT COUNT(*) FROM {} WHERE PortalID = 0 AND HTTPAlias = @P1",
                dnn_tables::name(&q, "PortalAlias")
            );
            Ok(conn.scalar(&sql, &[alias.as_str().into()]).await?.as_i64().unwrap_or(0))
        })
    }

    fn finish_host(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        user: &str,
        cancel: &CancelToken,
    ) -> Result<i64, String> {
        let user = user.to_string();
        on_site_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(0) };
            let sql = format!(
                "UPDATE {} SET UpdatePassword = 0 WHERE IsSuperUser = 1 AND Username = @P1; SELECT @@ROWCOUNT; \
                 UPDATE {} SET IsSecure = 0 WHERE PortalID = 0 AND IsSecure = 1;",
                dnn_tables::name(&q, "Users"),
                dnn_tables::name(&q, "Tabs")
            );
            Ok(conn.scalar(&sql, &[user.as_str().into()]).await?.as_i64().unwrap_or(0))
        })
    }

    fn set_host_password(
        &self,
        site: &Path,
        database: &DatabaseConnection,
        application_name: &str,
        user: &str,
        hash: &str,
        salt: &str,
        cancel: &CancelToken,
    ) -> Result<HostPasswordOutcome, String> {
        let (application, user, hash, salt) =
            (application_name.to_string(), user.to_string(), hash.to_string(), salt.to_string());
        on_site_database(site, database, cancel, move |mut conn| async move {
            let Some(q) = qualifier(&mut conn, "Version").await? else { return Ok(HostPasswordOutcome::NoDnnTables) };
            let users = dnn_tables::name(&q, "Users");
            let hosts = conn
                .scalar(
                    &format!("SELECT COUNT(*) FROM {users} WHERE Username = @P1 AND IsSuperUser = 1"),
                    &[user.as_str().into()],
                )
                .await?;
            if hosts.as_i64().unwrap_or(0) == 0 {
                return Ok(HostPasswordOutcome::NotAHost);
            }
            // In one transaction: an error before COMMIT leaves the connection - and with it the transaction - to be
            // closed, which rolls it back.
            conn.batch("SET XACT_ABORT ON; BEGIN TRANSACTION;").await?;
            let returned = conn
                .scalar(
                    "DECLARE @r int; DECLARE @now datetime = GETUTCDATE(); EXEC @r = dbo.aspnet_Membership_SetPassword                      @ApplicationName = @P1, @UserName = @P2, @NewPassword = @P3, @PasswordSalt = @P4, @CurrentTimeUtc = @now,                      @PasswordFormat = 1; SELECT @r",
                    &[application.as_str().into(), user.as_str().into(), hash.as_str().into(), salt.as_str().into()],
                )
                .await?;
            if returned.as_i64().is_some_and(|code| code != 0) {
                conn.batch("ROLLBACK TRANSACTION;").await?;
                return Ok(HostPasswordOutcome::NoMembershipUser);
            }
            conn.execute(
                "EXEC dbo.aspnet_Membership_UnlockUser @ApplicationName = @P1, @UserName = @P2",
                &[application.as_str().into(), user.as_str().into()],
            )
            .await?;
            conn.execute(
                &format!("UPDATE {users} SET UpdatePassword = 0 WHERE Username = @P1 AND IsSuperUser = 1"),
                &[user.as_str().into()],
            )
            .await?;
            conn.batch("COMMIT TRANSACTION;").await?;
            Ok(HostPasswordOutcome::Changed)
        })
    }

    fn try_connect(&self, database: &DatabaseConnection) -> Result<(), String> {
        // The site's own connection string, from here - not kept open: a session of the site's login would stop
        // Remove from dropping that login (tiberius has no pool; the connection closes when dropped).
        let user = if database.windows_authentication { "" } else { database.user.as_str() };
        let settings = for_server(&database.server, Some(&database.database), user, Some(&database.password), 10);
        block_on(None, async move { Conn::open(&settings).await.map(drop) }).map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dnnm_dnn::upgrades::{DnnUpgradeStep, PlannedStep};

    #[test]
    fn texts_word_for_word() {
        assert_eq!(texts::menu_tooltip("9.13.9"), "From DNN 9.13.9, with a backup before each step.");
        assert_eq!(texts::operation_title("shop", "10.3.3"), "Upgrade DNN of 'shop' to 10.3.3");
        assert_eq!(
            texts::done("shop", "10.3.3"),
            "'shop' runs DNN 10.3.3 - the backup of every step stays in its backups folder."
        );
    }

    #[test]
    fn the_saved_path_edits_are_read_and_refused_ones_dropped() {
        let settings = Settings::from_rows(
            [
                ("projects.dnnUpgradeStops", "1"),
                ("projects.dnnUpgradeStops[0].version", "10.4.2"),
                ("projects.dnnUpgradeStops[0].netFrameworkRelease", "528040"),
                ("projects.dnnUpgradeStops[0].sqlServerMajor", "14"),
                ("projects.dnnUpgradeStops[0].note", "Mine"),
                ("projects.dnnUpgradeStops[0].replacesListed", "false"),
                ("projects.dnnUpgradeStops[0].removed", "False"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string())),
        );
        let stops = upgrade_stops(&settings);
        assert_eq!(stops.len(), 1);
        assert_eq!(
            (stops[0].version.as_str(), stops[0].net_framework_release, stops[0].sql_server_major),
            ("10.4.2", 528040, 14)
        );
        assert!(added(&stops, DnnNumber::new(10, 4, 2)));
        assert!(!added(&stops, DnnNumber::new(10, 3, 3)));

        let mut bad = settings;
        bad.set("projects.dnnUpgradeStops[0].version", "10.4");
        assert!(upgrade_stops(&bad).is_empty());
        assert_eq!(release_apis(&Settings::default()).len(), 2);
    }

    #[test]
    fn dnns_plan_becomes_the_use_cases() {
        let step = DnnUpgradeStep {
            from: Version::new(10, 2, 5),
            to: Version::new(10, 3, 3),
            method: DnnUpgradeMethod::LocalUpgrade,
            ..DnnUpgradeStep::default()
        };
        let plan = DnnUpgradePlan {
            current: Version::new4(10, 2, 5, 0),
            target: Version::new(10, 3, 3),
            steps: vec![PlannedStep {
                step,
                checks: vec![
                    UpgradeFinding::new(UpgradeFindingSeverity::Blocking, "SQL Server", "Too old")
                        .with_fix("Upgrade it"),
                ],
            }],
            site: vec![],
        };
        let ours = plan_of(&plan);
        assert_eq!(ours.current, DnnNumber::new(10, 2, 5));
        assert_eq!(ours.steps[0].to_string(), "10.02.05 → 10.03.03");
        assert_eq!(ours.steps[0].method, StepMethod::LocalUpgrade);
        assert!(!ours.can_start());
        assert_eq!(ours.description, plan.describe());
        assert_eq!(ours.blocking()[0].fix.as_deref(), Some("Upgrade it"));
    }

    #[test]
    fn a_connection_keeps_its_kind_and_login() {
        let site = DatabaseConnection {
            kind: DatabaseKind::SqlServer,
            server: "localhost,1433".into(),
            database: "shop".into(),
            windows_authentication: false,
            user: "shop".into(),
            password: "secret".into(),
        };
        let sql = sql_connection(&site);
        assert_eq!((sql.server.as_str(), sql.database.as_str(), sql.user.as_str()), ("localhost,1433", "shop", "shop"));
        assert!(!sql.uses_windows_authentication());
        let file = DatabaseConnection { kind: DatabaseKind::LocalDbFile, windows_authentication: true, ..site };
        assert_eq!(sql_connection(&file).kind, dnnm_sql::model::DatabaseKind::LocalDbFile);
    }
}
