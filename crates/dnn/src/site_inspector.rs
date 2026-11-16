//! The pre-upgrade analyser's eyes (`src/DnnManager.Infrastructure/Dnn/DnnSiteInspector.cs`, `IDnnSiteInspector`):
//! reads a DNN site, its database, IIS and this PC into `DnnSiteFacts` - without changing anything. Assemblies are read
//! from their metadata (what they reference), never loaded. The database is read through `SiteSql`; what the C# asks
//! `IIisManager.GetSiteDetails` for comes in as `SiteIis`.

use std::collections::HashSet;
use std::path::Path;

use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind, load_site_xml};

use crate::safe_zip::free_space;
use crate::site_sql::SiteSql;
use crate::upgrades::{DnnAssembly, DnnContentCounts, DnnExtension, DnnSiteFacts};

/// The appSettings DNN's own web.config has (9.x and 10.x) and those its installer adds - the rest are the site's own.
const DNN_APP_SETTINGS: [&str; 24] = [
    "SiteSqlServer",
    "InstallTemplate",
    "AutoUpgrade",
    "UseInstallWizard",
    "InstallMemberRole",
    "ShowMissingKeys",
    "EnableCachePersistence",
    "HostHeader",
    "RemoveAngleBrackets",
    "PersistentCookieTimeout",
    "EnableServicesFrameworkTracing",
    "UpdateServiceUrl",
    "PreserveLoginUrl",
    "loginUrl",
    "ValidationSettings:UnobtrusiveValidationMode",
    "MobileViewSiteCookieName",
    "DisableMobileViewSiteCookieName",
    "AllowDnnImagePlaceholderText",
    "AllowDnnUpgradeUpload",
    "DisableCsp",
    "InstallVersion",
    "InstallationDate",
    "UpdateServiceRedirect",
    "UseDnnImageHandler",
];

/// What a DNN assembly's name starts with - DNN's own, and the libraries it ships.
const SHIPPED_PREFIXES: [&str; 37] = [
    "DotNetNuke",
    "Dnn.",
    "DNN.",
    "Telerik",
    "Microsoft.",
    "System.",
    "Newtonsoft",
    "log4net",
    "Lucene",
    "WebFormsMvp",
    "SharpZipLib",
    "ICSharpCode",
    "CountryListBox",
    "ClientDependency",
    "Moq",
    "Castle",
    "Antlr",
    "WebGrease",
    "Owin",
    "BouncyCastle",
    "MailKit",
    "MimeKit",
    "Ganss",
    "AngleSharp",
    "Lucene.Net",
    "Dapper",
    "PetaPoco",
    "SchwabenCode",
    "netstandard",
    "Nustache",
    "HtmlSanitizer",
    "LiteDB",
    "Ude",
    "WebActivatorEx",
    "Hellang",
    "SolutionFramework",
    "Microsoft",
];

/// What IIS says about the site (`IIisManager.GetSiteDetails`): its root app pool's .NET CLR and pipeline, and whether
/// it has an http binding with a host name and a port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteIis {
    pub app_pool_clr: Option<String>,
    pub app_pool_pipeline: Option<String>,
    pub has_host_binding: bool,
}

/// `IDnnSiteInspector`.
pub struct SiteInspector<S: SiteSql> {
    sql: S,
    /// `AppDataPaths.BackupsDirectory`: where the upgrade's backups are written.
    backups_directory: std::path::PathBuf,
}

fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or_default().trim().to_string()
}

impl<S: SiteSql> SiteInspector<S> {
    pub fn new(sql: S, backups_directory: impl Into<std::path::PathBuf>) -> Self {
        Self { sql, backups_directory: backups_directory.into() }
    }

    /// Everything that can be read about the site `site_name` in `directory`, with its database (None when web.config
    /// names none). `iis`: None when IIS doesn't know the site. Never fails: what can't be read is left out and said.
    pub fn inspect(
        &self,
        site_name: &str,
        directory: &Path,
        database: Option<&DatabaseConnection>,
        iis: Option<&SiteIis>,
    ) -> DnnSiteFacts {
        let mut facts = DnnSiteFacts {
            site_name: site_name.to_string(),
            directory: directory.to_string_lossy().into_owned(),
            files_version: dnnm_infrastructure::dnn_install::dnn_version(directory),
            install_blocked: directory.join("installBlocker.lock").is_file(),
            net_framework_release: net_framework_release(),
            app_pool_clr: iis.and_then(|i| i.app_pool_clr.clone()),
            app_pool_pipeline: iis.and_then(|i| i.app_pool_pipeline.clone()),
            has_host_binding: iis.is_some_and(|i| i.has_host_binding),
            site_bytes: folder_bytes(directory),
            backup_free_bytes: free_space(&self.backups_directory).map(|b| b as i64),
            database_kind: database.map(|d| d.kind),
            database_name: database.map(|d| d.database.clone()),
            ..DnnSiteFacts::default()
        };
        with_web_config(&mut facts, directory);
        let Some(database) = database else { return facts };
        if database.kind == DatabaseKind::LocalDbFile {
            facts.database_problem = Some("A LocalDB file - not read before an upgrade.".into());
            return facts;
        }
        if let Err(problem) = self.read_database(&mut facts, directory, database) {
            facts.database_problem = Some(first_line(&problem));
            facts.assemblies = assemblies(directory, None);
            facts.telerik_in_bin = directory.join("bin").join("Telerik.Web.UI.dll").is_file();
        }
        facts
    }

    fn read_database(
        &self,
        facts: &mut DnnSiteFacts,
        directory: &Path,
        database: &DatabaseConnection,
    ) -> Result<(), String> {
        let server = self.sql.server_facts(database)?;
        facts.sql_server_version = Some(server.version);
        facts.sql_server_edition = Some(server.edition);
        facts.database_state = Some(server.state);
        // Its data files' size, for the room the backups need.
        facts.database_bytes = Some(server.database_bytes);
        let Some(version) = self.sql.dnn_version(database)? else {
            facts.database_problem = Some("The database has no DNN tables.".into());
            return Ok(());
        };
        let extensions = self.sql.packages(database)?.into_iter().map(extension).collect();
        let registered: HashSet<String> = self
            .sql
            .registered_assemblies(database)?
            .into_iter()
            .map(|a| {
                let name = Path::new(&a).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(a);
                name.to_lowercase()
            })
            .collect();
        facts.database_version = Some(version);
        facts.counts = self.sql.counts(database)?;
        facts.extensions = extensions;
        facts.assemblies = assemblies(directory, Some(&registered));
        facts.telerik_in_bin = directory.join("bin").join("Telerik.Web.UI.dll").is_file();
        Ok(())
    }

    /// The counts an upgrade must leave as they were; None when the database can't be read.
    pub fn count(&self, database: &DatabaseConnection) -> Option<DnnContentCounts> {
        self.sql.counts(database).ok().flatten()
    }
}

fn extension(row: crate::site_sql::PackageRow) -> DnnExtension {
    let org = row.organization;
    let is_dnns = row.is_system_package
        || is_dnn_name(&row.name)
        || org.as_deref().is_some_and(|o| {
            let o = o.to_lowercase();
            o.contains("dnn") || o.contains("dotnetnuke")
        });
    DnnExtension {
        friendly_name: row.friendly_name.unwrap_or_else(|| row.name.clone()),
        name: row.name,
        kind: row.package_type,
        version: row.version,
        owner: org,
        is_dnns,
    }
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(prefix))
}

fn is_shipped(assembly: &str) -> bool {
    SHIPPED_PREFIXES.iter().any(|p| starts_with_ignore_case(assembly, p))
}

fn is_dnn_name(package: &str) -> bool {
    starts_with_ignore_case(package, "DotNetNuke") || starts_with_ignore_case(package, "Dnn")
}

/// The assemblies in bin that aren't DNN's or a library DNN ships - each with the DNN version it was built against and
/// whether it uses Telerik - read from their metadata. `registered`: those an extension installed, names in lowercase
/// (None when the database couldn't say - then all count as registered).
pub fn assemblies(directory: &Path, registered: Option<&HashSet<String>>) -> Vec<DnnAssembly> {
    let bin = directory.join("bin");
    let Ok(entries) = std::fs::read_dir(&bin) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .collect();
    files.sort();
    let mut list = Vec::new();
    for file in files {
        // A native DLL, or one in use - not .NET, or not readable now.
        let Some(info) = crate::metadata::read(&file) else { continue };
        if is_shipped(&info.name) {
            continue;
        }
        let mut dnn = None;
        let mut telerik = false;
        for (referenced, version) in &info.references {
            if referenced.eq_ignore_ascii_case("DotNetNuke") {
                dnn = Some(*version);
            }
            if starts_with_ignore_case(referenced, "Telerik.Web.UI")
                || referenced.eq_ignore_ascii_case("DotNetNuke.Web.Deprecated")
                || referenced.eq_ignore_ascii_case("DotNetNuke.Website.Deprecated")
            {
                telerik = true;
            }
        }
        list.push(DnnAssembly {
            file: file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            registered: registered.is_none_or(|r| r.contains(&info.name.to_lowercase())),
            name: info.name,
            dnn_version: dnn,
            uses_telerik: telerik,
        });
    }
    list
}

fn with_web_config(facts: &mut DnnSiteFacts, directory: &Path) {
    let Ok(text) = std::fs::read_to_string(directory.join("web.config")) else { return };
    let Ok(doc) = load_site_xml(text.trim_start_matches('\u{feff}')) else { return };
    let root = doc.root_element();
    fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        node.children().find(|n| n.has_tag_name(name))
    }
    let adds = |section: Option<roxmltree::Node>| -> Vec<(String, Option<String>, Option<String>)> {
        section
            .into_iter()
            .flat_map(|s| s.children().filter(|n| n.has_tag_name("add")))
            .map(|a| {
                (
                    a.attribute("key").unwrap_or_default().to_string(),
                    a.attribute("value").map(str::to_string),
                    a.attribute("name").map(str::to_string),
                )
            })
            .collect()
    };
    let settings = adds(child(root, "appSettings"));
    facts.has_machine_key = Some(child(root, "system.web").and_then(|w| child(w, "machineKey")).is_some());
    facts.auto_upgrade = settings.iter().find(|s| s.0.eq_ignore_ascii_case("AutoUpgrade")).and_then(|s| s.1.clone());
    facts.custom_app_settings = settings
        .iter()
        .map(|s| s.0.clone())
        .filter(|k| {
            !k.is_empty()
                && !DNN_APP_SETTINGS.iter().any(|d| d.eq_ignore_ascii_case(k))
                && !starts_with_ignore_case(k, "aspnet:")
        })
        .collect();
    facts.connection_strings =
        adds(child(root, "connectionStrings")).into_iter().filter_map(|a| a.2).filter(|n| !n.is_empty()).collect();
}

/// The .NET Framework 4.x installed: the v4 Full key's `Release`.
#[cfg(windows)]
fn net_framework_release() -> Option<i32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: a DWORD read into a DWORD of the stated size.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full"),
            w!("Release"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    read.is_ok().then_some(value as i32)
}

#[cfg(not(windows))]
fn net_framework_release() -> Option<i32> {
    None
}

/// The site's files' size, links and junctions not followed, unreadable folders skipped.
fn folder_bytes(directory: &Path) -> i64 {
    fn walk(folder: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
        entries
            .flatten()
            .map(|e| match e.metadata() {
                Ok(m) if m.file_type().is_symlink() || is_reparse(&m) => 0,
                Ok(m) if m.is_dir() => walk(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    }
    walk(directory) as i64
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site_sql::{PackageRow, ServerFacts, SitePages};
    use crate::upgrades::DnnContentItem;

    struct Fake(bool);

    impl SiteSql for Fake {
        fn server_facts(&self, _: &DatabaseConnection) -> Result<ServerFacts, String> {
            if self.0 {
                Ok(ServerFacts {
                    version: "16.0.4135.4".into(),
                    edition: "Developer Edition".into(),
                    state: "ONLINE".into(),
                    database_bytes: 8192,
                })
            } else {
                Err("Login failed for user 'shop'.\r\nmore".into())
            }
        }
        fn dnn_version(&self, _: &DatabaseConnection) -> Result<Option<String>, String> {
            Ok(Some("9.13.9".into()))
        }
        fn counts(&self, _: &DatabaseConnection) -> Result<Option<DnnContentCounts>, String> {
            Ok(Some(DnnContentCounts { portals: 1, ..Default::default() }))
        }
        fn packages(&self, _: &DatabaseConnection) -> Result<Vec<PackageRow>, String> {
            let row = |name: &str, org: Option<&str>, system: bool| PackageRow {
                name: name.into(),
                friendly_name: None,
                package_type: "Module".into(),
                version: "1.0.0".into(),
                organization: org.map(str::to_string),
                is_system_package: system,
            };
            Ok(vec![
                row("DNN_HTML", None, false),
                row("Acme.Blog", Some("Acme"), false),
                row("X", Some("DNN Corp."), false),
                row("Y", None, true),
            ])
        }
        fn registered_assemblies(&self, _: &DatabaseConnection) -> Result<Vec<String>, String> {
            Ok(vec![])
        }
        fn site_pages(&self, _: &DatabaseConnection, _: usize) -> Result<SitePages, String> {
            Ok(SitePages::default())
        }
        fn content_items(&self, _: &DatabaseConnection) -> Result<Vec<DnnContentItem>, String> {
            Ok(vec![])
        }
    }

    fn database() -> DatabaseConnection {
        DatabaseConnection {
            kind: DatabaseKind::SqlServer,
            server: "localhost".into(),
            database: "shop".into(),
            windows_authentication: true,
            user: String::new(),
            password: String::new(),
        }
    }

    #[test]
    fn reads_the_site_its_web_config_and_its_database() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("web.config"),
            "<configuration><appSettings><add key=\"AutoUpgrade\" value=\"false\" /><add key=\"Acme.ApiKey\" value=\"x\" />\
             <add key=\"aspnet:MaxJsonDeserializerMembers\" value=\"1\" /></appSettings>\
             <connectionStrings><add name=\"SiteSqlServer\" connectionString=\"x\" /><add name=\"Reports\" connectionString=\"y\" /></connectionStrings>\
             <system.web><machineKey validationKey=\"x\" /></system.web></configuration>",
        )
        .unwrap();
        std::fs::write(dir.path().join("installBlocker.lock"), "").unwrap();
        let iis = SiteIis {
            app_pool_clr: Some("v4.0".into()),
            app_pool_pipeline: Some("Integrated".into()),
            has_host_binding: true,
        };
        let inspector = SiteInspector::new(Fake(true), dir.path());
        let facts = inspector.inspect("shop", dir.path(), Some(&database()), Some(&iis));
        assert_eq!(facts.auto_upgrade.as_deref(), Some("false"));
        assert_eq!(facts.custom_app_settings, ["Acme.ApiKey"]);
        assert_eq!(facts.connection_strings, ["SiteSqlServer", "Reports"]);
        assert_eq!(facts.has_machine_key, Some(true));
        assert!(facts.install_blocked);
        assert_eq!(facts.database_version.as_deref(), Some("9.13.9"));
        assert_eq!(facts.sql_server_major(), Some(16));
        let dnns: Vec<_> = facts.extensions.iter().map(|e| (e.name.as_str(), e.is_dnns)).collect();
        assert_eq!(dnns, [("DNN_HTML", true), ("Acme.Blog", false), ("X", true), ("Y", true)]);
        assert_eq!(facts.extensions[1].friendly_name, "Acme.Blog");
        assert!(facts.site_bytes > 0);

        let failed = SiteInspector::new(Fake(false), dir.path()).inspect("shop", dir.path(), Some(&database()), None);
        assert_eq!(failed.database_problem.as_deref(), Some("Login failed for user 'shop'."));
        assert!(!failed.has_host_binding);
        let mut local = database();
        local.kind = DatabaseKind::LocalDbFile;
        let local = SiteInspector::new(Fake(true), dir.path()).inspect("shop", dir.path(), Some(&local), None);
        assert_eq!(local.database_problem.as_deref(), Some("A LocalDB file - not read before an upgrade."));
    }

    #[cfg(windows)]
    #[test]
    fn assemblies_outside_dnn_are_read_from_their_metadata() {
        // A .NET assembly every Windows has stands in for an extension's: copied into bin under its own name.
        let Some(windows) = std::env::var_os("SystemRoot") else { return };
        let source = Path::new(&windows).join(r"Microsoft.NET\Framework64\v4.0.30319\Accessibility.dll");
        if !source.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::copy(&source, dir.path().join("bin").join("Accessibility.dll")).unwrap();
        std::fs::write(dir.path().join("bin").join("native.dll"), "MZ not .NET").unwrap();
        let found = assemblies(dir.path(), Some(&HashSet::new()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Accessibility");
        assert!(!found[0].registered && found[0].dnn_version.is_none() && !found[0].uses_telerik);
        assert!(net_framework_release().is_some_and(|r| r >= 378389));
    }
}
