//! `ExportForDeploymentUseCase` (and `ExportForDeploymentRequest`, and the validation of `DeploymentExportDialog`): a
//! package for the live server in one folder - the site's files as `<project>.zip` without what only matters on this
//! PC (source control, editor folders, logs, caches, the search index), its web.config made ready for the server (the
//! live connection string or a placeholder, debug off, the HTTPS rules DNN Manager switched off back on); the database
//! as `<project>.bacpac` with every portal's local aliases (this PC's addresses) left out and its live primary alias
//! primary again, the live domains typed as portal 0's aliases and DNN's SSL setting to match; and DEPLOY.txt
//! saying what to do with them. The local site and its database are not changed: the database is changed in a
//! temporary copy (exported, imported under another name on the same server, changed, exported again, dropped).
//!
//! Deviation: the C# reporter ties each step to a stage of its plan and sets the Output's context line; the port's
//! `Reporter` has neither, so the plan is announced and the steps follow it by their titles.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::backups::{Bacpacs, Databases, SiteFiles, SqlLogin, Stamp, is_same_or_inside};
use super::portal_aliases;
use crate::operation::{Operation, STEP_LIMIT};

/// What the live site doesn't need: made on this PC, or made again by DNN.
pub const LOCAL_ONLY: &[&str] = &[
    ".git",
    ".vs",
    ".vscode",
    ".idea",
    "node_modules",
    r"Portals\_default\Logs",
    r"Portals\_default\Cache",
    r"App_Data\Search",
    r"App_Data\ClientDependency",
];

/// Written into web.config when no live connection string is given - to fill in on the server.
pub const PLACEHOLDER: &str =
    "Data Source=YOUR-SQL-SERVER;Initial Catalog=YOUR-DATABASE;User ID=YOUR-USER;Password=YOUR-PASSWORD";

/// The stages of the Output's plan.
pub const STAGES: [&str; 4] = ["Site files", "web.config", "Database", "Instructions"];

/// What Export for deployment makes, as its dialog asks for it.
#[derive(Clone, PartialEq, Eq)]
pub struct ExportForDeploymentRequest {
    pub project_name: String,
    /// The folder the site serves (its IIS physical path).
    pub project_directory: PathBuf,
    /// The folder the package is written to: `<project>.zip`, `<project>.bacpac` and DEPLOY.txt.
    pub output_folder: PathBuf,
    /// The live site's domains, e.g. `www.example.com` - the first is the primary portal alias. Empty: the database's
    /// aliases are left as they are.
    pub domains: Vec<String>,
    /// The live site uses HTTPS: DNN's SSL setting is switched on and the HTTPS rules DNN Manager switched off are
    /// back on.
    pub https: bool,
    /// The live database's connection string for web.config; None writes a placeholder to fill in on the server.
    pub connection_string: Option<String>,
    /// The database as a .bacpac too; false: only the site's files.
    pub include_database: bool,
}

impl std::fmt::Debug for ExportForDeploymentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The connection string can hold the live server's password.
        f.debug_struct("ExportForDeploymentRequest")
            .field("project_name", &self.project_name)
            .field("output_folder", &self.output_folder)
            .field("domains", &self.domains)
            .field("https", &self.https)
            .field("include_database", &self.include_database)
            .finish_non_exhaustive()
    }
}

impl ExportForDeploymentRequest {
    fn live_connection_string(&self) -> Option<&str> {
        self.connection_string.as_deref().filter(|c| !c.is_empty())
    }
}

/// What making the zip's web.config ready for the server did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedWebConfig {
    /// `compilation debug` was set to false.
    pub debug_off: bool,
    /// The HTTPS redirect rules DNN Manager had switched off that are on again.
    pub rules_enabled: Vec<String>,
}

/// The zip's web.config - and the files its connectionStrings and appSettings come from (configSource) - changed for
/// the server and put back into the zip (`PrepareWebConfig`).
pub trait DeployWebConfig: Send + Sync {
    /// Writes `connection_string` as SiteSqlServer, switches debug compilation off and, with `https`, the HTTPS
    /// redirect rules DNN Manager switched off back on. Err: "The site has no web.config.", "Could not set the
    /// connection string: …", or why the zip couldn't be changed.
    fn prepare(&self, zip: &Path, connection_string: &str, https: bool) -> Result<PreparedWebConfig, String>;
}

pub struct ExportForDeploymentDeps<'a> {
    pub files: &'a dyn SiteFiles,
    pub web_config: &'a dyn DeployWebConfig,
    pub bacpacs: &'a dyn Bacpacs,
    pub databases: Arc<dyn Databases>,
    /// Not %TEMP%: the database's copy is read back from it - a folder only administrators can change.
    pub temp_folder: PathBuf,
    pub now: Stamp,
    /// Settings → host-name suffix (`dnndev.me`): portal aliases under it are this PC's, left out of the package.
    pub hostname_suffix: String,
    /// The site's own host names here (its IIS bindings') - aliases on them, or under them, are left out too.
    pub local_hosts: Vec<String>,
}

pub fn export_for_deployment(
    deps: &ExportForDeploymentDeps,
    req: &ExportForDeploymentRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let name = &req.project_name;
    let directory = &req.project_directory;
    if !directory.is_dir() {
        return Err(format!("Project folder not found: {}", directory.display()));
    }
    let folder = std::path::absolute(&req.output_folder).unwrap_or_else(|_| req.output_folder.clone());
    if is_same_or_inside(&folder, directory) {
        return Err("Choose a folder outside the project's folder - the package would end up inside the site.".into());
    }

    let zip_path = folder.join(format!("{name}.zip"));
    let bacpac_path = folder.join(format!("{name}.bacpac"));
    let instructions = folder.join("DEPLOY.txt");
    op.undo.delete_folder_on_undo(&folder);
    for file in [&zip_path, &bacpac_path, &instructions] {
        op.undo.restore_file_on_undo(file);
    }
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;

    op.report.plan(&STAGES);
    op.report.step("Zipping the site's files");
    // An unreadable filter leaves out only what is always left out.
    let filtered = deps.files.backup_filter(directory).unwrap_or_default();
    let excluded: Vec<String> = LOCAL_ONLY.iter().map(|s| s.to_string()).chain(filtered).collect();
    op.report.info(&format!("Leaving out: {}", excluded.join(", ")));
    deps.files.create_zip(directory, &zip_path, &excluded, op.report, &op.cancel)?;

    op.report.step("Making web.config ready for the server");
    let live = req.live_connection_string().map(str::trim).unwrap_or(PLACEHOLDER);
    let prepared = deps.web_config.prepare(&zip_path, live, req.https)?;
    op.report.success(if req.live_connection_string().is_some() {
        "SiteSqlServer is the live connection string."
    } else {
        "SiteSqlServer is a placeholder - fill it in on the server (DEPLOY.txt)."
    });
    if prepared.debug_off {
        op.report.success("Debug compilation is off.");
    }
    if req.https && !prepared.rules_enabled.is_empty() {
        op.report.success(&format!("Switched back on: {}", prepared.rules_enabled.join(", ")));
    }

    op.report.step("Exporting the database");
    if req.include_database {
        if let Err(e) = export_database(deps, &bacpac_path, req, op) {
            if op.cancel.is_cancelled() {
                return Err(e);
            }
            return Err(format!(
                "The site's files are in {}, but the database couldn't be exported: {e}",
                zip_path.display()
            ));
        }
    } else {
        op.report.info("Left out, as chosen.");
    }

    op.report.step("Writing DEPLOY.txt");
    std::fs::write(&instructions, instructions_text(name, req, deps.now)).map_err(|e| e.to_string())?;
    op.report.success(&format!("Package: {}", folder.display()));
    // What it holds is the live site's: said, so it doesn't stay around forgotten.
    let has_connection = req.live_connection_string().is_some();
    if req.include_database || has_connection {
        op.report.warn(&format!(
            "The package holds {}{}{} - delete it once it is deployed (Troubleshoot → Clean up data → Deployment packages).",
            if req.include_database { "the whole database (its users too)" } else { "" },
            if req.include_database && has_connection { " and " } else { "" },
            if has_connection { "the live connection string, password included" } else { "" },
        ));
    }
    Ok(())
}

/// The site's database as a .bacpac. With local portal aliases to leave out, live domains or HTTPS, a temporary copy
/// on the same server gets them first: exported, imported under another name, changed, exported again - then dropped.
fn export_database(
    deps: &ExportForDeploymentDeps,
    bacpac_path: &Path,
    req: &ExportForDeploymentRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let source = deps.bacpacs.export_source(&req.project_name, &req.project_directory)?;
    deps.bacpacs.ensure_available(op.report, &op.cancel)?;
    if req.domains.is_empty() && !req.https && !has_local_aliases(deps, &source, op) {
        return deps.bacpacs.export(&source, bacpac_path, op.report, &op.cancel);
    }

    let copy_name = format!("{}_deploy_{}", source.database, deps.now.compact());
    let copy = source.with_database(&copy_name);
    let temp_bacpac = deps.temp_folder.join(format!("{copy_name}.bacpac"));
    let (undo_databases, undo_copy) = (Arc::clone(&deps.databases), copy.clone());
    op.undo.add(format!("Drop the temporary database [{copy_name}]"), STEP_LIMIT, move |_| {
        undo_databases.drop_database(&undo_copy)
    });
    let result = change_copy(deps, &source, &copy, &temp_bacpac, bacpac_path, req, op);
    if let Err(e) = deps.databases.drop_database(&copy) {
        op.report.warn(&format!("The temporary database [{copy_name}] is still there: {e}"));
    }
    let _ = std::fs::remove_file(&temp_bacpac);
    result
}

fn change_copy(
    deps: &ExportForDeploymentDeps,
    source: &SqlLogin,
    copy: &SqlLogin,
    temp_bacpac: &Path,
    bacpac_path: &Path,
    req: &ExportForDeploymentRequest,
    op: &mut Operation,
) -> Result<(), String> {
    op.report.info(&format!(
        "Copying [{}] to [{}] on {} - the site's own database isn't changed…",
        source.database, copy.database, source.server
    ));
    deps.bacpacs.export(source, temp_bacpac, op.report, &op.cancel)?;
    deps.bacpacs.import(copy, temp_bacpac, op.report, &op.cancel)?;
    live_aliases(deps, copy, req, op)?;
    if let Some(primary) = req.domains.first() {
        deps.databases.replace_portal_aliases(copy, &req.domains, &op.cancel)?;
        op.report.success(&format!("Portal aliases: {} ({primary} primary).", req.domains.join(", ")));
    }
    match deps.databases.set_ssl(copy, req.https, &op.cancel) {
        Ok(()) => op.report.success(&format!("DNN's SSL setting is {}.", if req.https { "on" } else { "off" })),
        Err(e) => op.report.warn(&e),
    }
    deps.bacpacs.export(copy, bacpac_path, op.report, &op.cancel)
}

/// Whether the database has aliases of this PC's, or noted live primaries, to take out - read in the site's own
/// database, which isn't changed. One that can't be read: no.
fn has_local_aliases(deps: &ExportForDeploymentDeps, source: &SqlLogin, op: &mut Operation) -> bool {
    match deps.databases.portal_aliases(source, &op.cancel) {
        Ok((rows, noted)) => {
            let plan = portal_aliases::delocalize(&rows, &noted, &deps.hostname_suffix, &deps.local_hosts);
            portal_aliases::changes(&rows, &noted, &plan)
        }
        Err(e) => {
            op.report.warn(&format!("The portal aliases couldn't be read, so they are exported as they are: {e}"));
            false
        }
    }
}

/// In the copy: every portal's local aliases out, its live primary alias primary again - so it works on the server
/// as it is. A portal with no live alias is said; portal 0's are the domains typed, when there are some.
fn live_aliases(
    deps: &ExportForDeploymentDeps,
    copy: &SqlLogin,
    req: &ExportForDeploymentRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let (rows, noted) = deps.databases.portal_aliases(copy, &op.cancel)?;
    let plan = portal_aliases::delocalize(&rows, &noted, &deps.hostname_suffix, &deps.local_hosts);
    if !portal_aliases::changes(&rows, &noted, &plan) {
        return Ok(());
    }
    deps.databases.apply_live_aliases(copy, &plan, &op.cancel)?;
    for d in &plan {
        let typed = d.portal == 0 && !req.domains.is_empty();
        match d.primary_alias(&rows) {
            _ if typed => {}
            Some(alias) if !d.remove.is_empty() => op.report.success(&format!(
                "Portal {}: {alias} primary - {} local alias{} left out.",
                d.portal,
                d.remove.len(),
                if d.remove.len() == 1 { "" } else { "es" }
            )),
            Some(_) => {}
            None => op.report.warn(&format!(
                "Portal {} has only this PC's addresses - add its live domain in DNN on the server (Settings → Site \
                 Settings → Site Aliases).",
                d.portal
            )),
        }
    }
    Ok(())
}

/// DEPLOY.txt: what to do with the package on the server.
pub fn instructions_text(name: &str, req: &ExportForDeploymentRequest, now: Stamp) -> String {
    let domains = if req.domains.is_empty() { "your domain".to_string() } else { req.domains.join(", ") };
    let scheme = if req.https { "https" } else { "http" };
    let mut lines = vec![
        format!("Deploying {name} - made by DNN Manager on {}", now.display_minutes()),
        String::new(),
        format!(
            "1. Unzip {name}.zip into the website's folder on the server (IIS, or Azure App Service's site\\wwwroot)."
        ),
        "   Give the app pool's identity modify rights on the folder - DNN writes to App_Data and Portals.".into(),
    ];
    if req.include_database {
        lines.push(format!(
            "2. Import {name}.bacpac into the live SQL Server: SQL Server Management Studio → Databases → Import Data-tier"
        ));
        lines.push(format!(
            "   Application, or: sqlpackage /Action:Import /SourceFile:{name}.bacpac /TargetConnectionString:\"...\""
        ));
    } else {
        lines.push("2. Point the site at its live database (the database wasn't part of this package).".into());
    }
    lines.push(if req.live_connection_string().is_some() {
        "3. web.config already has the live connection string (SiteSqlServer) - check it.".into()
    } else {
        "3. In web.config, replace the placeholder connection string SiteSqlServer (connectionStrings and appSettings)\r\n   with the live database's.".into()
    });
    lines.push(format!(
        "4. Bind the IIS site to {domains}{}.",
        if req.https { " - with a certificate on an https binding" } else { "" }
    ));
    lines.push(if req.domains.is_empty() {
        "   DNN's portal aliases are the live site's - this PC's addresses were left out. A portal made here needs its live domain in DNN (Settings → Site Settings → Site Aliases).".into()
    } else {
        format!("   DNN's portal aliases are already {domains}.")
    });
    lines.push(format!(
        "5. Open {scheme}://{}/ and sign in as a host.",
        req.domains.first().map_or("your-domain", String::as_str)
    ));
    lines.push(String::new());
    lines.push(format!(
        "web.config has debug compilation off{}",
        if req.https { " and the HTTPS redirect rules DNN Manager had switched off back on." } else { "." }
    ));
    lines.join("\r\n") + "\r\n"
}

/// The live domains as typed (`DeploymentExportDialog.Domains`) - comma-, semicolon- or space-separated, lower case,
/// each once, in their order (the first is the primary alias) - and what is wrong with the first that isn't a host
/// name, with an optional port.
pub fn parse_domains(text: &str) -> (Vec<String>, Option<String>) {
    let mut domains: Vec<String> = Vec::new();
    let mut problem = None;
    for entry in text.split([',', ';', ' ', '\t']).map(str::trim).filter(|e| !e.is_empty()) {
        if !is_host_name(entry) {
            problem.get_or_insert_with(|| {
                format!(
                    "'{entry}' isn't a host name - enter it like www.example.com or example.com:8080, without http:// or a path."
                )
            });
            continue;
        }
        let domain = entry.to_lowercase();
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    (domains, problem)
}

/// A DNS host name, optionally followed by `:port` (`Uri.CheckHostName(host) == Dns`): labels of letters, digits,
/// `-` and `_`, none empty or longer than 63 - not an IP address.
pub fn is_host_name(text: &str) -> bool {
    let (host, port) = match text.rfind(':') {
        Some(colon) => (&text[..colon], Some(&text[colon + 1..])),
        None => (text, None),
    };
    if let Some(port) = port
        && !port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p))
    {
        return false;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > 255 || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// What's wrong with the folder the package goes to, or None when it can go there (`DeploymentExportDialog`).
pub fn folder_problem(folder: &str, site_directory: &Path) -> Option<String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Some("Choose the folder the package goes to.".into());
    }
    if !is_fully_qualified(folder) || folder.contains(['<', '>', '"', '|', '?', '*']) {
        return Some(r"Enter the folder's full path, e.g. C:\Deploy.".into());
    }
    if site_directory.as_os_str().is_empty() {
        return None;
    }
    // The site's folder itself, or any folder in it: the package would be part of the site it packs.
    is_same_or_inside(Path::new(folder), site_directory).then(|| "Choose a folder outside the site's folder.".into())
}

/// `Path.IsPathFullyQualified`: a drive with a separator (`C:\`) or a UNC path (`\\server\share`).
fn is_fully_qualified(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || (b.len() >= 2 && (b[0] == b'\\' || b[0] == b'/') && (b[1] == b'\\' || b[1] == b'/'))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Outcome};
    use crate::use_cases::backups::tests::stamp;
    use crate::use_cases::export_project::tests::{FakeBacpacs, FakeFiles};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeWebConfig {
        written: Mutex<Option<(String, bool)>>,
        missing: bool,
    }

    impl DeployWebConfig for FakeWebConfig {
        fn prepare(&self, _: &Path, connection_string: &str, https: bool) -> Result<PreparedWebConfig, String> {
            if self.missing {
                return Err("The site has no web.config.".into());
            }
            *self.written.lock().unwrap() = Some((connection_string.into(), https));
            Ok(PreparedWebConfig { debug_off: true, rules_enabled: vec!["HTTP to HTTPS".into()] })
        }
    }

    #[derive(Default)]
    pub struct FakeDatabases {
        pub calls: Mutex<Vec<String>>,
        pub fail_ssl: bool,
        pub fail_drop: bool,
        /// The aliases the database has.
        pub aliases: Vec<portal_aliases::AliasRow>,
        pub noted: std::collections::HashMap<i32, String>,
    }

    impl Databases for FakeDatabases {
        fn exists(&self, d: &SqlLogin, _: &CancelToken) -> Result<bool, String> {
            self.calls.lock().unwrap().push(format!("exists {}", d.database));
            Ok(true)
        }
        fn drop_database(&self, d: &SqlLogin) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("drop {}", d.database));
            if self.fail_drop { Err("in use".into()) } else { Ok(()) }
        }
        fn rename(&self, d: &SqlLogin, new_name: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("rename {} {new_name}", d.database));
            Ok(())
        }
        fn replace_portal_aliases(&self, d: &SqlLogin, a: &[String], _: &CancelToken) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("aliases {} {}", d.database, a.join("+")));
            Ok(())
        }
        fn set_ssl(&self, d: &SqlLogin, on: bool, _: &CancelToken) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("ssl {} {on}", d.database));
            if self.fail_ssl { Err("The database has no DNN portal settings.".into()) } else { Ok(()) }
        }
        fn portal_aliases(&self, _: &SqlLogin, _: &CancelToken) -> Result<super::super::backups::PortalAliases, String> {
            Ok((self.aliases.clone(), self.noted.clone()))
        }
        fn apply_live_aliases(&self, d: &SqlLogin, plan: &[portal_aliases::Delocalized], _: &CancelToken) -> Result<(), String> {
            let removed: Vec<String> = plan.iter().flat_map(|p| &p.remove).map(|id| id.to_string()).collect();
            let primary: Vec<String> = plan.iter().filter_map(|p| p.primary).map(|id| id.to_string()).collect();
            self.calls.lock().unwrap().push(format!("live aliases {} -{} +{}", d.database, removed.join(","), primary.join(",")));
            Ok(())
        }
    }

    struct Setup {
        dir: tempfile::TempDir,
        files: FakeFiles,
        web_config: FakeWebConfig,
        bacpacs: FakeBacpacs,
        databases: Arc<FakeDatabases>,
    }

    impl Setup {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("site")).unwrap();
            std::fs::create_dir(dir.path().join("temp")).unwrap();
            Self {
                dir,
                files: FakeFiles::default(),
                web_config: FakeWebConfig::default(),
                bacpacs: FakeBacpacs::default(),
                databases: Arc::new(FakeDatabases::default()),
            }
        }

        fn request(&self) -> ExportForDeploymentRequest {
            ExportForDeploymentRequest {
                project_name: "shop".into(),
                project_directory: self.dir.path().join("site"),
                output_folder: self.dir.path().join("deployments").join("shop_20260928_154210"),
                domains: vec!["www.example.com".into(), "example.com".into()],
                https: true,
                connection_string: None,
                include_database: true,
            }
        }

        fn run(&self, req: &ExportForDeploymentRequest) -> (Outcome, Vec<String>) {
            let deps = ExportForDeploymentDeps {
                files: &self.files,
                web_config: &self.web_config,
                bacpacs: &self.bacpacs,
                databases: self.databases.clone(),
                temp_folder: self.dir.path().join("temp"),
                now: stamp("20260928_154210"),
                hostname_suffix: "dnndev.me".into(),
                local_hosts: vec!["shop.dnndev.me".into()],
            };
            let mut lines = Lines::default();
            let outcome =
                crate::operation::run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
                    export_for_deployment(&deps, req, op)
                });
            (outcome, lines.0)
        }
    }

    #[test]
    fn makes_the_package_from_a_changed_copy_of_the_database() {
        let s = Setup::new();
        let req = s.request();
        let (outcome, lines) = s.run(&req);
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(*s.files.excluded.lock().unwrap(), LOCAL_ONLY);
        assert_eq!(*s.web_config.written.lock().unwrap(), Some((PLACEHOLDER.to_string(), true)));
        assert_eq!(
            *s.bacpacs.calls.lock().unwrap(),
            ["export shop", "import shop_deploy_20260928154210", "export shop_deploy_20260928154210"]
        );
        assert_eq!(
            *s.databases.calls.lock().unwrap(),
            [
                "aliases shop_deploy_20260928154210 www.example.com+example.com",
                "ssl shop_deploy_20260928154210 true",
                "drop shop_deploy_20260928154210"
            ]
        );
        let folder = &req.output_folder;
        let deploy = std::fs::read_to_string(folder.join("DEPLOY.txt")).unwrap();
        assert!(deploy.starts_with("Deploying shop - made by DNN Manager on 2026-09-28 15:42\r\n"));
        assert!(deploy.contains("5. Open https://www.example.com/ and sign in as a host."));
        assert!(lines.contains(&"ok SiteSqlServer is a placeholder - fill it in on the server (DEPLOY.txt).".into()));
        assert!(lines.contains(&"ok Switched back on: HTTP to HTTPS".into()));
        assert!(lines.contains(&"ok Portal aliases: www.example.com, example.com (www.example.com primary).".into()));
        assert_eq!(
            lines.last().unwrap(),
            "warn The package holds the whole database (its users too) - delete it once it is deployed (Troubleshoot → Clean up data → Deployment packages)."
        );
    }

    #[test]
    fn without_domains_or_https_the_database_is_exported_as_it_is() {
        let s = Setup::new();
        let req = ExportForDeploymentRequest {
            domains: vec![],
            https: false,
            connection_string: Some(" Server=live;Database=x;User ID=u;Password=p ".into()),
            ..s.request()
        };
        let (outcome, lines) = s.run(&req);
        assert_eq!(outcome, Outcome::Finished);
        assert_eq!(*s.bacpacs.calls.lock().unwrap(), ["export shop"]);
        assert!(s.databases.calls.lock().unwrap().is_empty());
        assert_eq!(
            s.web_config.written.lock().unwrap().as_ref().unwrap().0,
            "Server=live;Database=x;User ID=u;Password=p"
        );
        assert!(lines.contains(&"ok SiteSqlServer is the live connection string.".into()));
        assert!(!lines.iter().any(|l| l.starts_with("ok Switched back on")));
        assert!(
            lines
                .last()
                .unwrap()
                .contains("the whole database (its users too) and the live connection string, password included")
        );
    }

    #[test]
    fn this_pcs_aliases_are_left_out_and_the_live_primary_ones_are_back() {
        let row = |id: i64, portal: i32, alias: &str, primary: bool| portal_aliases::AliasRow {
            id,
            portal,
            alias: alias.into(),
            primary,
        };
        let databases = FakeDatabases {
            aliases: vec![
                row(1, 0, "shop.nl", false),
                row(2, 0, "shop.dnndev.me", true),
                row(3, 1, "outlet.nl", false),
                row(4, 1, "outlet.shop.dnndev.me", true),
                row(5, 2, "made-here.shop.dnndev.me", true),
            ],
            noted: [(0, "shop.nl".to_string()), (1, "outlet.nl".to_string())].into(),
            ..FakeDatabases::default()
        };
        let s = Setup { databases: Arc::new(databases), ..Setup::new() };
        // No domains typed, no HTTPS: a copy all the same, as the aliases change.
        let req = ExportForDeploymentRequest { domains: vec![], https: false, ..s.request() };
        let (outcome, lines) = s.run(&req);
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(
            *s.databases.calls.lock().unwrap(),
            [
                "live aliases shop_deploy_20260928154210 -2,4,5 +1,3",
                "ssl shop_deploy_20260928154210 false",
                "drop shop_deploy_20260928154210"
            ]
        );
        assert!(lines.contains(&"ok Portal 0: shop.nl primary - 1 local alias left out.".into()), "{lines:#?}");
        assert!(lines.contains(&"ok Portal 1: outlet.nl primary - 1 local alias left out.".into()));
        assert!(lines.iter().any(|l| l.starts_with("warn Portal 2 has only this PC's addresses")));
    }

    #[test]
    fn left_out_database_and_a_failed_export() {
        let s = Setup::new();
        let req = ExportForDeploymentRequest { include_database: false, ..s.request() };
        let (outcome, lines) = s.run(&req);
        assert_eq!(outcome, Outcome::Finished);
        assert!(lines.contains(&"Left out, as chosen.".into()));
        assert!(!lines.iter().any(|l| l.starts_with("warn The package holds")));
        let deploy = std::fs::read_to_string(req.output_folder.join("DEPLOY.txt")).unwrap();
        assert!(deploy.contains("2. Point the site at its live database"));

        let mut s = Setup::new();
        s.bacpacs.fail_import = true;
        let req = s.request();
        let (outcome, lines) = s.run(&req);
        let zip = req.output_folder.join("shop.zip");
        assert_eq!(
            outcome,
            Outcome::Failed(format!(
                "The site's files are in {}, but the database couldn't be exported: BACPAC import failed: x",
                zip.display()
            ))
        );
        // Dropped once as the step ends, once more by the undo - a drop of what isn't there is fine.
        assert_eq!(s.databases.calls.lock().unwrap().iter().filter(|c| c.starts_with("drop")).count(), 2);
        assert!(lines.contains(&"ok Drop the temporary database [shop_deploy_20260928154210]".into()));
        assert!(!req.output_folder.exists(), "the undo deletes the package folder");
    }

    #[test]
    fn refuses_a_folder_in_the_site_and_a_site_without_web_config() {
        let mut s = Setup::new();
        let req = ExportForDeploymentRequest { output_folder: s.dir.path().join("site"), ..s.request() };
        assert!(
            matches!(s.run(&req).0, Outcome::Failed(e) if e.starts_with("Choose a folder outside the project's folder"))
        );
        s.web_config.missing = true;
        assert_eq!(s.run(&s.request()).0, Outcome::Failed("The site has no web.config.".into()));
    }

    #[test]
    fn domains_are_validated_as_the_dialog_does() {
        let (domains, problem) = parse_domains("WWW.Example.com, example.com;www.example.com  shop.dnndev.me:8080");
        assert_eq!(domains, ["www.example.com", "example.com", "shop.dnndev.me:8080"]);
        assert_eq!(problem, None);
        let (domains, problem) = parse_domains("https://x.com example.com 1.2.3.4 a:0");
        assert_eq!(domains, ["example.com"]);
        assert_eq!(
            problem.unwrap(),
            "'https://x.com' isn't a host name - enter it like www.example.com or example.com:8080, without http:// or a path."
        );
        assert!(!is_host_name("1.2.3.4") && !is_host_name("x.com/path") && !is_host_name("-x.com"));
        assert_eq!(parse_domains("  "), (vec![], None));
    }

    #[test]
    fn the_folder_is_checked_as_the_dialog_does() {
        let site = Path::new(r"C:\DNN\shop");
        assert_eq!(folder_problem(" ", site).unwrap(), "Choose the folder the package goes to.");
        assert_eq!(folder_problem("deploy", site).unwrap(), r"Enter the folder's full path, e.g. C:\Deploy.");
        assert_eq!(folder_problem(r"C:\a|b", site).unwrap(), r"Enter the folder's full path, e.g. C:\Deploy.");
        assert_eq!(folder_problem(r"c:\dnn\SHOP\out", site).unwrap(), "Choose a folder outside the site's folder.");
        assert_eq!(folder_problem(r"C:\DNN\shop", site).unwrap(), "Choose a folder outside the site's folder.");
        assert_eq!(folder_problem(r"C:\DNN\shop-deploy", site), None);
        assert_eq!(folder_problem(r"\\server\share\x", site), None);
    }
}
