//! The queries the upgrade's analyser and checks run on a site's database (`DnnSiteInspector` and `DnnUpgradeChecks`
//! in `src/DnnManager.Infrastructure/Dnn/`) - a trait here, implemented where SQL Server is reached (another crate).
//! Every table name goes with DNN's object qualifier (`DnnTables.QualifierAsync` / `DnnTables.Name`); a table an older
//! DNN doesn't have counts as none. Err: the database couldn't be reached or a query failed - its message, first line
//! first.

use std::collections::BTreeMap;

use dnnm_infrastructure::web_config::DatabaseConnection;

use crate::upgrades::{DnnContentCounts, DnnContentItem};

/// What SQL Server says about itself and the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFacts {
    /// `SERVERPROPERTY('ProductVersion')`, e.g. 16.0.4135.4.
    pub version: String,
    /// `SERVERPROPERTY('Edition')`.
    pub edition: String,
    /// `DATABASEPROPERTYEX(DB_NAME(), 'Status')` - ONLINE when it is usable.
    pub state: String,
    /// The database's data files' size (`sys.database_files`, type 0).
    pub database_bytes: i64,
}

/// A row of DNN's `Packages` table (`Organization` and `IsSystemPackage` came with DNN 5 and 6: None and false when the
/// database hasn't got them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRow {
    pub name: String,
    /// None when `FriendlyName` is NULL.
    pub friendly_name: Option<String>,
    pub package_type: String,
    pub version: String,
    pub organization: Option<String>,
    pub is_system_package: bool,
}

/// What the checks open: the site's extensions, its portal aliases and a sample of each portal's pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SitePages {
    /// `SELECT Name FROM Packages`.
    pub extensions: Vec<String>,
    /// `(PortalID, HTTPAlias)` ordered by PortalID, IsPrimary DESC, PortalAliasID.
    pub aliases: Vec<(i32, String)>,
    /// Per portal, the first `per_portal` TabIDs: `IsDeleted = 0 AND IsVisible = 1 AND DisableLink = 0 AND
    /// ISNULL(Url, '') = '' AND TabPath NOT LIKE '//Admin%' AND TabPath NOT LIKE '//Host%' ORDER BY Level, TabOrder`.
    pub tabs: BTreeMap<i32, Vec<i32>>,
}

/// The queries on a site's (SQL Server) database.
pub trait SiteSql: Send + Sync {
    fn server_facts(&self, database: &DatabaseConnection) -> Result<ServerFacts, String>;

    /// The highest `Major.Minor.Build` of the `Version` table; None when the database has no DNN tables.
    fn dnn_version(&self, database: &DatabaseConnection) -> Result<Option<String>, String>;

    /// Portals, Users, Roles, UserRoles, Tabs (not deleted), Modules, Tab- + Module- + FolderPermission, Schedule and
    /// Packages counted; None when the database has no DNN tables.
    fn counts(&self, database: &DatabaseConnection) -> Result<Option<DnnContentCounts>, String>;

    /// Every row of `Packages`, ordered by PackageType, Name.
    fn packages(&self, database: &DatabaseConnection) -> Result<Vec<PackageRow>, String>;

    /// `SELECT AssemblyName FROM Assemblies` (none when there is no such table).
    fn registered_assemblies(&self, database: &DatabaseConnection) -> Result<Vec<String>, String>;

    /// The extensions, portal aliases and sampled pages; empty when the database has no DNN tables.
    fn site_pages(&self, database: &DatabaseConnection, per_portal: usize) -> Result<SitePages, String>;

    /// Every page (kind "page": TabID, TabPath or "#id", its TabPermission count), module ("module": ModuleID,
    /// "'title' (#id)", its package - or module type when it has none -, its page and ModulePermission count), module
    /// type ("type": DesktopModuleID, ModuleName) and scheduled job ("job": ScheduleID, TypeFullName or "#id"); empty
    /// when the database has no DNN tables.
    fn content_items(&self, database: &DatabaseConnection) -> Result<Vec<DnnContentItem>, String>;
}
