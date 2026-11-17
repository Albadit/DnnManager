//! What the pre-upgrade analyser reads about a site (`src/DnnManager.Application/Upgrades/DnnSiteFacts.cs`).

use std::fmt;

use dnnm_infrastructure::web_config::DatabaseKind;

use crate::version::Version;

/// An extension installed in DNN (its `Packages` table): module, skin, provider, library…
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnExtension {
    pub name: String,
    pub friendly_name: String,
    /// DNN's package type: Module, Skin, Container, Provider, Auth_System, Library, …
    pub kind: String,
    pub version: String,
    pub owner: Option<String>,
    /// Part of DNN itself (a system package, or DNN's own organisation) - upgraded with it.
    pub is_dnns: bool,
}

/// An assembly in the site's `bin` that isn't DNN's own, and what it was built against - read from its references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnAssembly {
    pub file: String,
    pub name: String,
    /// The DotNetNuke.dll version it references; None when it doesn't reference DNN.
    pub dnn_version: Option<Version>,
    /// It references Telerik.Web.UI or DNN's Telerik wrappers.
    pub uses_telerik: bool,
    /// An installed extension registered it (DNN's `Assemblies` table) - otherwise it was copied in by hand.
    pub registered: bool,
}

/// A count of what an upgrade must leave as it was, for the checks before and after each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DnnContentCounts {
    pub portals: i32,
    pub users: i32,
    pub roles: i32,
    pub user_roles: i32,
    pub pages: i32,
    pub modules: i32,
    pub permissions: i32,
    pub scheduled_jobs: i32,
    pub extensions: i32,
}

impl DnnContentCounts {
    /// The portals, users, roles and user roles fewer than `before`, as "Users 12 → 11" - what no upgrade may change.
    /// Pages, modules, permissions and jobs are compared one by one: DNN's upgrade removes some of its own.
    pub fn lost(&self, before: &DnnContentCounts) -> Vec<String> {
        let mut lost = Vec::new();
        let mut check = |name: &str, was: i32, now: i32| {
            if now < was {
                lost.push(format!("{name} {was} → {now}"));
            }
        };
        check("Portals", before.portals, self.portals);
        check("Users", before.users, self.users);
        check("Roles", before.roles, self.roles);
        check("User roles", before.user_roles, self.user_roles);
        lost
    }
}

impl fmt::Display for DnnContentCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} portal(s), {} users, {} roles, {} user roles, {} pages, {} modules, {} permissions, {} scheduled jobs, {} extensions",
            self.portals,
            self.users,
            self.roles,
            self.user_roles,
            self.pages,
            self.modules,
            self.permissions,
            self.scheduled_jobs,
            self.extensions
        )
    }
}

/// A page, module, module type or scheduled job of a site - what the checks after a step compare one by one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DnnContentItem {
    /// "page", "module", "type" or "job".
    pub kind: String,
    pub id: i32,
    /// How people know it: the page's path, the module's title and page, the job's type.
    pub name: String,
    /// A module's extension - when the upgrade removed that extension, the module goes with it.
    pub package: Option<String>,
    /// The page a module is on.
    pub page_id: Option<i32>,
    pub permissions: i32,
}

impl DnnContentItem {
    pub fn new(kind: &str, id: i32, name: &str) -> Self {
        Self { kind: kind.into(), id, name: name.into(), package: None, page_id: None, permissions: 0 }
    }
}

/// What the analyser read from a site, its server and this PC. A fact that couldn't be read is None (or its problem
/// is said), and its check comes out Unknown.
#[derive(Debug, Clone, Default)]
pub struct DnnSiteFacts {
    pub site_name: String,
    pub directory: String,
    /// bin\DotNetNuke.dll's version.
    pub files_version: Option<String>,
    /// The newest version in the database's `Version` table.
    pub database_version: Option<String>,
    /// installBlocker.lock is there.
    pub install_blocked: bool,
    /// The v4 Full key's `Release` (528040+ is 4.8).
    pub net_framework_release: Option<i32>,
    pub app_pool_clr: Option<String>,
    pub app_pool_pipeline: Option<String>,
    /// The site has an http binding with a host name - DNN's upgrade is run through it.
    pub has_host_binding: bool,
    pub site_bytes: i64,
    pub database_bytes: Option<i64>,
    /// Free space on the drive the backups are written to.
    pub backup_free_bytes: Option<i64>,
    pub database_kind: Option<DatabaseKind>,
    pub database_name: Option<String>,
    /// Why the database couldn't be read; None when it was.
    pub database_problem: Option<String>,
    pub sql_server_version: Option<String>,
    pub sql_server_edition: Option<String>,
    /// ONLINE when it is usable.
    pub database_state: Option<String>,
    pub counts: Option<DnnContentCounts>,
    pub extensions: Vec<DnnExtension>,
    pub assemblies: Vec<DnnAssembly>,
    pub telerik_in_bin: bool,
    pub has_machine_key: Option<bool>,
    /// The AutoUpgrade appSetting - None when there is none.
    pub auto_upgrade: Option<String>,
    /// appSettings DNN doesn't ship - the site's own, kept by the upgrade.
    pub custom_app_settings: Vec<String>,
    pub connection_strings: Vec<String>,
}

impl DnnSiteFacts {
    /// SQL Server's major version: 16 for 16.0.4135.4.
    pub fn sql_server_major(&self) -> Option<i32> {
        self.sql_server_version.as_deref()?.split('.').next()?.trim().parse().ok()
    }
}

/// "SQL Server 2022" for major 16.
pub fn sql_server_name(major: i32) -> String {
    match major {
        8 => "SQL Server 2000".into(),
        9 => "SQL Server 2005".into(),
        10 => "SQL Server 2008".into(),
        11 => "SQL Server 2012".into(),
        12 => "SQL Server 2014".into(),
        13 => "SQL Server 2016".into(),
        14 => "SQL Server 2017".into(),
        15 => "SQL Server 2019".into(),
        16 => "SQL Server 2022".into(),
        17 => "SQL Server 2025".into(),
        _ => format!("SQL Server (version {major})"),
    }
}

/// ".NET Framework 4.8.1" for the v4 Full key's `Release`.
pub fn net_framework_name(release: i32) -> String {
    let name = match release {
        r if r >= 533320 => ".NET Framework 4.8.1",
        r if r >= 528040 => ".NET Framework 4.8",
        r if r >= 461808 => ".NET Framework 4.7.2",
        r if r >= 461308 => ".NET Framework 4.7.1",
        r if r >= 460798 => ".NET Framework 4.7",
        r if r >= 394802 => ".NET Framework 4.6.2",
        r if r >= 394254 => ".NET Framework 4.6.1",
        r if r >= 393295 => ".NET Framework 4.6",
        r if r >= 379893 => ".NET Framework 4.5.2",
        r if r >= 378675 => ".NET Framework 4.5.1",
        r if r >= 378389 => ".NET Framework 4.5",
        _ => return format!(".NET Framework 4 (release {release})"),
    };
    name.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lose_only_what_no_upgrade_may_change() {
        let before = DnnContentCounts { users: 12, pages: 30, ..Default::default() };
        let after = DnnContentCounts { users: 11, pages: 20, ..Default::default() };
        assert_eq!(after.lost(&before), vec!["Users 12 → 11"]);
        assert_eq!(sql_server_name(16), "SQL Server 2022");
        assert_eq!(net_framework_name(461808), ".NET Framework 4.7.2");
        let facts = DnnSiteFacts { sql_server_version: Some("16.0.4135.4".into()), ..Default::default() };
        assert_eq!(facts.sql_server_major(), Some(16));
    }
}
