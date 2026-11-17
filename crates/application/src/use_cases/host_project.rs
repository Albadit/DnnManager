//! Hosting a project folder that already holds a DNN site (`UseCases/HostExistingProjectUseCase.cs`) - its IIS
//! website, its local database, or both - and New project's import (`UseCases/ImportProjectUseCase.cs`): a zipped site
//! extracted into a new project folder, then hosted with its database restored from its backup. The site's files are
//! never downloaded, copied or overwritten by hosting.

use std::path::{Path, PathBuf};

use dnnm_domain::{DnnProject, validate_project_name};

use crate::operation::{CANCELLED, Operation};
use crate::use_cases::hosting::{
    HostingDeps, check_local_sql, disable_ssl, drop_database_on_undo, drop_login_on_undo, grant_site_login,
    is_backup_file, restore_backup, site_using, swap_in, worded,
};
use crate::use_cases::provisioner;

/// What to set up for an existing folder (`HostExistingProjectRequest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExistingRequest {
    /// A folder that already exists under the projects folder.
    pub project_name: String,
    /// Create (or recreate) the IIS website; false for a database-only run.
    pub setup_iis: bool,
    /// A database in the local SQL container, and web.config pointed at it.
    pub setup_database: bool,
    /// A .bacpac or .bak restored into the database; None leaves an existing one as it is and creates a missing one
    /// empty.
    pub backup_file: Option<PathBuf>,
    /// A new project made from a copy of another one (Import): its database is always its own, named after the
    /// project, and web.config is always pointed at it.
    pub own_database: bool,
    /// The host name the site answers on; None for `{project}.{hostname suffix}`.
    pub host_name: Option<String>,
    /// The port; None for the settings' site port.
    pub port: Option<u16>,
}

impl HostExistingRequest {
    /// The IIS website only, as the C# request's defaults.
    pub fn new(project_name: &str) -> Self {
        Self {
            project_name: project_name.to_string(),
            setup_iis: true,
            setup_database: false,
            backup_file: None,
            own_database: false,
            host_name: None,
            port: None,
        }
    }
}

/// A project made from a zipped DNN site and its database backup (`ImportProjectRequest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub project_name: String,
    /// A .zip of the site's files.
    pub zip_path: PathBuf,
    /// The site's database: a .bacpac (or .bak).
    pub backup_file: PathBuf,
    pub host_name: Option<String>,
    pub port: Option<u16>,
}

/// `HostExistingProjectUseCase.ExecuteAsync`.
pub fn host_existing_project(deps: &HostingDeps, req: &HostExistingRequest, op: &mut Operation) -> Result<(), String> {
    let name = req.project_name.as_str();
    validate_project_name(name)?;
    if !req.setup_iis && !req.setup_database {
        return Err("Nothing to set up - choose the IIS website, the database, or both.".into());
    }
    let project = deps.settings.project(name);
    let directory = &project.project_directory;
    if !directory.is_dir() {
        return Err(format!("Project folder not found: {}", directory.display()));
    }
    // A site of this name that serves another folder is another project's - never replaced from here.
    if req.setup_iis
        && let Some(taken) = provisioner::site_name_taken(deps.iis.as_ref(), name, directory)
    {
        return Err(taken);
    }

    // The address the site answers on - the one asked for, or {project}.{suffix} on the settings' port.
    let host_name = match req.host_name.as_deref().map(str::trim) {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => deps.settings.hostname_for(name),
    };
    let port = req.port.unwrap_or(deps.settings.site_port);
    let alias = provisioner::alias_for(&host_name, port);
    // Before anything is changed: a second site with the same address wouldn't start.
    if req.setup_iis
        && deps.iis.is_available()
        && let Some(same) = site_using(&deps.iis.site_runtimes().unwrap_or_default(), &host_name, port, Some(name))
    {
        return Err(format!(
            "http://{alias} is already the address of the IIS site '{same}' - choose another host name or port."
        ));
    }

    let web_config = directory.join("web.config");
    let has_web_config = web_config.is_file();
    op.report
        .info(&format!("Using the existing files in {} - nothing is downloaded or overwritten.", directory.display()));
    if !has_web_config {
        op.report.info("No web.config in the folder - it doesn't look like a DNN site yet. Continuing anyway.");
    }

    let mut site_created = false;
    let mut disabled_rules = Vec::new();
    if req.setup_iis {
        // IIS is the point of this step, so missing features are offered before IIS is checked.
        op.report.step("IIS website");
        if let Err(e) = deps.machine.ensure_iis_features(op)
            && e == CANCELLED
        {
            return Err(e);
        }
        op.cancel.check()?;
        if deps.iis.is_available() {
            if deps.iis.site_states().contains_key(name) {
                op.report.info(&format!("IIS site '{name}' already exists - recreating it."));
            }
            site_created = provisioner::try_create_site(&deps.iis, name, directory, &host_name, port, op);
            disabled_rules = disable_https_redirects(deps, &web_config, op);
        } else {
            op.report.fail("IIS is not available on this machine - enable it in Settings → IIS and retry.");
        }
    }

    if req.setup_database {
        op.report.step("Database");
        if let Err(e) = setup_database(deps, &project, &web_config, has_web_config, req, &alias, op) {
            // Alongside a website the database is a best-effort extra - unless its data was asked for (a backup to
            // restore, Import). On its own it is the whole job.
            if e == CANCELLED || !req.setup_iis || req.backup_file.is_some() || req.own_database {
                return Err(e);
            }
            op.report.fail(&format!("{e} Skipping database setup."));
        }
    }

    if !req.setup_iis {
        op.report.step("Done");
        op.report.success("Database ready.");
        return Ok(());
    }
    if !site_created {
        return Err("The IIS website could not be created.".into());
    }

    let url = format!("http://{alias}");
    op.report.step("Verify");
    match deps.machine.check_http(&url, 15, &op.cancel) {
        Ok(status) => op.report.success(&format!("HTTP {status} from {url}")),
        Err(e) if e == CANCELLED => return Err(e),
        Err(e) => op.report.info(&format!("HTTP probe: {e}")),
    }

    op.report.step("Done");
    op.report.success(&format!("Open {url} to use the site."));
    if !disabled_rules.is_empty() {
        op.report.warn(&production_reminder(&disabled_rules));
    }
    Ok(())
}

/// A production web.config often redirects every request to https://; the local site has no HTTPS, so such rules are
/// switched off - and said loudly, since they must be back on before the site goes to production.
fn disable_https_redirects(deps: &HostingDeps, web_config: &Path, op: &mut Operation) -> Vec<String> {
    // The site's own file: a cancel puts it back as it was.
    if web_config.exists() {
        op.undo.restore_file_on_undo(web_config);
    }
    let rules = match deps.web_config.disable_https_redirect_rules(web_config) {
        Ok(rules) => rules,
        Err(e) => {
            op.report.fail(&format!("Could not check web.config for HTTPS redirects: {e}"));
            return Vec::new();
        }
    };
    if !rules.switched_off.is_empty() {
        op.report.info(&format!(
            "Switched off the HTTPS redirect rule{} {} in web.config - the local site has no HTTPS, so it would \
             redirect to an address that doesn't answer.",
            plural(&rules.switched_off),
            quoted(&rules.switched_off)
        ));
    }
    if !rules.already_off.is_empty() {
        op.report.info(&format!(
            "The HTTPS redirect rule{} {} in web.config {} already switched off for local development.",
            plural(&rules.already_off),
            quoted(&rules.already_off),
            if rules.already_off.len() == 1 { "was" } else { "were" }
        ));
    }
    let all = rules.all();
    if !all.is_empty() {
        op.report.warn(&production_reminder(&all));
    }
    all
}

fn plural(names: &[String]) -> &'static str {
    if names.len() == 1 { "" } else { "s" }
}

fn production_reminder(names: &[String]) -> String {
    format!(
        "Before deploying this site to production, switch {} in web.config back on (remove enabled=\"false\" - look \
         for \"Disabled by DNN Manager\").",
        quoted(names)
    )
}

fn quoted(names: &[String]) -> String {
    names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ")
}

/// Restores the backup into the database when there is one (asking first when the database exists); otherwise creates
/// the database only when it is missing. Existing data is never dropped without a yes. A declined or failed web.config
/// update isn't a failure - the database is ready and how to reach it is said - unless the project must have a
/// database of its own.
fn setup_database(
    deps: &HostingDeps,
    project: &DnnProject,
    web_config: &Path,
    has_web_config: bool,
    req: &HostExistingRequest,
    site_alias: &str,
    op: &mut Operation,
) -> Result<(), String> {
    if let Some(file) = &req.backup_file {
        if !file.is_file() {
            return Err(format!("Backup file not found: {}", file.display()));
        }
        if !is_backup_file(file) {
            return Err(format!("Not a .bacpac or .bak file: {}", file.display()));
        }
    }

    let port = check_local_sql(deps, op)?;

    // web.config already pointing at a database of its own in the local container: kept (made when it's gone), and
    // web.config left alone. Otherwise - a production connection, DNN's LocalDB placeholder, no web.config, a copy that
    // must have its own (Import), or a database another IIS site uses - the project's own database, so two sites copied
    // from the same source never share one.
    let current = if has_web_config { deps.web_config.read_site_sql_server(web_config).ok() } else { None };
    let mut already_local = !req.own_database
        && current.as_ref().is_some_and(|c| !c.database.is_empty() && deps.sql.is_local_container(&c.server, port));
    let own_name = deps.settings.database_name_for(&project.name);
    if already_local
        && let Some(c) = &current
        && let Some(other) = deps.sites.other_site_using(&project.name, &c.server, &c.database)
    {
        op.report.info(&format!(
            "web.config names [{}], the database of the IIS site '{other}' - this project gets a database of its own, \
             [{own_name}].",
            c.database
        ));
        already_local = false;
    }
    let db_name = match (&current, already_local) {
        (Some(c), true) => c.database.clone(),
        _ => own_name,
    };
    let db = deps.settings.database_for(project, &db_name, port);

    let exists = deps
        .sql
        .database_exists(&db.database_name, &op.cancel)
        .map_err(|e| worded(e, |e| format!("Could not check for database [{}]: {e}", db.database_name)))?;

    let mut created = false;
    if let Some(file) = &req.backup_file {
        // Restoring replaces the database, so an existing one is only overwritten on an explicit yes.
        let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let question = format!("Database [{}] already exists - replace it with {file_name}?", db.database_name);
        if exists && !op.prompt.confirm(&question, "Replace database", "Keep existing", true) {
            op.report
                .info(&format!("Kept the existing database [{}] - {file_name} was not restored.", db.database_name));
        } else {
            // Restored beside the database that is there, under a name of its own, and swapped in only once it is
            // complete. One restored into a new database is dropped again by a cancel or a failure.
            let stamp = deps.machine.stamp();
            let mut incoming = db.clone();
            if exists {
                incoming.database_name = format!("{}_restore_{stamp}", db.database_name);
            }
            drop_database_on_undo(deps, &incoming.database_name, op);
            op.report.info(&if exists {
                format!("Restoring {file_name} beside [{}] - it replaces it once it is complete…", db.database_name)
            } else {
                format!("Restoring [{}] from {file_name}…", db.database_name)
            });
            restore_backup(deps, &incoming, file, op).map_err(|e| {
                worded(e, |e| {
                    let left =
                        if exists { format!(" [{}] is left as it was.", db.database_name) } else { String::new() };
                    format!("Restoring {file_name} failed: {e}{left}")
                })
            })?;
            op.report.success(&format!("Database [{}] restored from {file_name}.", incoming.database_name));

            // A backup from another environment carries that site's portal aliases; without one for this address DNN
            // can't match the request. Not fatal - it can be added by hand.
            match deps.sql.remap_portal_aliases(
                &incoming.database_name,
                &deps.settings.hostname_suffix,
                site_alias,
                &op.cancel,
            ) {
                Ok(()) => op.report.success(&format!("PortalAlias set to {site_alias}.")),
                Err(e) => op.report.fail(&format!(
                    "Could not update PortalAlias: {e}. Add '{site_alias}' as a site alias or the site will not load at \
                     that address."
                )),
            }
            disable_ssl(deps, &incoming.database_name, op);

            if exists {
                swap_in(
                    deps,
                    &db.database_name,
                    &incoming.database_name,
                    &format!("{}_before_restore_{stamp}", db.database_name),
                    &format!("the database restored from {file_name}"),
                    op,
                )?;
            }
        }
    } else if exists {
        op.report
            .success(&format!("Database [{}] already exists on {} - keeping its data.", db.database_name, db.server));
    } else {
        drop_database_on_undo(deps, &db.database_name, op);
        deps.sql
            .create_database(&db, &op.cancel)
            .map_err(|e| worded(e, |e| format!("Database creation reported an error: {e}")))?;
        created = true;
        op.report.success(&format!("Created empty database [{}] on {}.", db.database_name, db.server));
    }

    if already_local {
        op.report.info(&format!("web.config already uses [{}] on {} - left unchanged.", db.database_name, db.server));
    } else if has_web_config {
        op.report.info(&match &current {
            Some(c) => format!("web.config currently connects to [{}] on {}.", c.database, c.server),
            None => "web.config has no usable SiteSqlServer connection yet.".to_string(),
        });

        // A copy that must have a database of its own isn't asked: left as it is, it would use the other one.
        let question = format!("Point web.config at [{}] on {}?", db.database_name, db.server);
        if req.own_database || op.prompt.confirm(&question, "Update web.config", "Leave as is", false) {
            // The site signs in with a login of its own, owner of its database only - not the container's sa.
            let login = grant_site_login(deps, &project.name, &db, &op.cancel)?;
            drop_login_on_undo(deps, &login.user, op);
            op.undo.restore_file_on_undo(web_config);
            match deps.web_config.write_site_sql_server(web_config, &login) {
                Ok(()) => op.report.success("web.config updated."),
                Err(e) if req.own_database => {
                    return Err(format!("Could not point web.config at [{}]: {e}", db.database_name));
                }
                Err(e) => op.report.fail(&format!("Could not update web.config: {e}")),
            }
        } else {
            op.report.info(&format!(
                "web.config left unchanged. Connect with: server '{}', database '{}', user '{}' and its password from \
                 Settings → Database server.",
                db.server, db.database_name, deps.settings.sql_user
            ));
        }
    }

    if created {
        op.report.info(
            "The database is empty: open the site to run the DNN install wizard, or put a backup in with Restore backup.",
        );
    }
    Ok(())
}

/// `ImportProjectUseCase.ExecuteAsync`: extracts the zip into a new project folder, then hosts it - the IIS website and
/// the database restored from the backup, a database of its own whatever the zip's web.config names.
pub fn import_project(deps: &HostingDeps, req: &ImportRequest, op: &mut Operation) -> Result<(), String> {
    let name = req.project_name.as_str();
    validate_project_name(name)?;
    if !req.backup_file.is_file() {
        return Err(format!("Backup file not found: {}", req.backup_file.display()));
    }
    if !is_backup_file(&req.backup_file) {
        return Err(format!("Not a .bacpac or .bak file: {}", req.backup_file.display()));
    }
    let project = deps.settings.project(name);
    let directory = &project.project_directory;
    if directory.exists() {
        return Err(format!("The folder {} already exists - choose another name.", directory.display()));
    }
    // Before the zip is extracted: an IIS site of this name serving another folder isn't replaced.
    if let Some(taken) = provisioner::site_name_taken(deps.iis.as_ref(), name, directory) {
        return Err(taken);
    }

    let zip_name = req.zip_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    op.report.step(&format!("Extracting {zip_name}"));
    // A cancel later on (the IIS site, the database) takes the new folder away too - after the rest.
    op.undo.delete_folder_on_undo(directory);
    let extracted = std::fs::create_dir_all(directory)
        .map_err(|e| format!("Extracting the zip failed: {e}"))
        .and_then(|()| deps.files.extract_zip(&req.zip_path, directory, op.report, &op.cancel));
    if let Err(e) = extracted {
        // The folder is new, so a half-extracted one is ours to clean up (on a cancel too).
        let _ = std::fs::remove_dir_all(directory);
        return Err(if e.is_empty() { "Extracting the zip failed.".into() } else { e });
    }

    match deps.files.ensure_gitignore(directory) {
        Ok(()) => op.report.info("Project .gitignore ready."),
        Err(e) => op.report.info(&format!("Could not write .gitignore: {e}")),
    }

    let backup_name = req.backup_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    op.report.info(&format!("Database backup: {backup_name}."));
    let host = HostExistingRequest {
        project_name: name.to_string(),
        setup_iis: true,
        setup_database: true,
        backup_file: Some(req.backup_file.clone()),
        own_database: true,
        host_name: req.host_name.clone(),
        port: req.port,
    };
    host_existing_project(deps, &host, op)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Outcome, run};
    use crate::use_cases::hosting::fakes::World;
    use crate::use_cases::hosting::{HttpsRedirectRules, SiteSql, SiteWebConfig};
    use crate::use_cases::tests::FakeIis;
    use std::sync::Arc;

    fn host(world: &World, req: &HostExistingRequest, answer: &Answer) -> (Outcome, Vec<String>) {
        let deps = world.deps();
        let mut lines = Lines::default();
        let outcome = run(&mut lines, answer, CancelToken::new(), &CancelToken::new(), |op| {
            host_existing_project(&deps, req, op)
        });
        (outcome, lines.0)
    }

    fn import(world: &World, req: &ImportRequest) -> (Outcome, Vec<String>) {
        let deps = world.deps();
        let mut lines = Lines::default();
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            import_project(&deps, req, op)
        });
        (outcome, lines.0)
    }

    fn backup(world: &World, name: &str) -> PathBuf {
        let file = world.root().join(name);
        std::fs::write(&file, "x").unwrap();
        file
    }

    #[test]
    fn nothing_to_set_up_and_a_missing_folder_are_refused() {
        let world = World::new();
        let req = HostExistingRequest { setup_iis: false, ..HostExistingRequest::new("shop") };
        assert_eq!(
            host(&world, &req, &Answer::yes()).0,
            Outcome::Failed("Nothing to set up - choose the IIS website, the database, or both.".into())
        );
        let (outcome, _) = host(&world, &HostExistingRequest::new("shop"), &Answer::yes());
        let Outcome::Failed(e) = outcome else { panic!() };
        assert!(e.starts_with("Project folder not found: "));
    }

    #[test]
    fn the_website_is_made_redirects_switched_off_and_the_site_asked() {
        let world = World::new();
        world.site("shop", None);
        *world.web_config.rules.lock().unwrap() =
            HttpsRedirectRules { switched_off: vec!["Force HTTPS".into()], already_off: vec![] };
        let (outcome, lines) = host(&world, &HostExistingRequest::new("shop"), &Answer::yes());
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(world.journal(), vec!["iis features", "disable redirects", "GET http://shop.dnndev.me"]);
        let reminder = "warn Before deploying this site to production, switch 'Force HTTPS' in web.config back on \
                        (remove enabled=\"false\" - look for \"Disabled by DNN Manager\").";
        assert_eq!(lines.iter().filter(|l| *l == reminder).count(), 2);
        assert!(lines.contains(
            &"Switched off the HTTPS redirect rule 'Force HTTPS' in web.config - the local site has no HTTPS, so it \
              would redirect to an address that doesn't answer."
                .to_string()
        ));
        assert!(lines.contains(&"ok HTTP 200 from http://shop.dnndev.me".to_string()));
        assert_eq!(lines.last().unwrap(), reminder);
    }

    #[test]
    fn an_address_another_site_has_is_refused_before_anything_changes() {
        let mut world = World::new();
        world.site("shop", None);
        let mut iis = FakeIis::running().with_site("other", r"D:\other");
        iis.sites.get_mut().unwrap().get_mut("other").unwrap().bindings =
            vec![crate::iis::IisBinding::parse("http", "*:80:shop.dnndev.me", false)];
        world.iis = Arc::new(iis);
        let (outcome, lines) = host(&world, &HostExistingRequest::new("shop"), &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed(
                "http://shop.dnndev.me is already the address of the IIS site 'other' - choose another host name or port."
                    .into()
            )
        );
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn a_database_beside_a_website_is_best_effort() {
        let world = World::new();
        world.site("shop", None);
        world.fail("test_login", "refused");
        let req = HostExistingRequest { setup_database: true, ..HostExistingRequest::new("shop") };
        let (outcome, lines) = host(&world, &req, &Answer::yes());
        assert_eq!(outcome, Outcome::Finished);
        assert!(lines.iter().any(|l| l.starts_with("! SQL Server at localhost,1433 is not reachable")
            && l.ends_with(" Skipping database setup.")));
        // On its own the database is the whole job.
        let req = HostExistingRequest { setup_iis: false, ..req };
        assert!(matches!(host(&world, &req, &Answer::yes()).0, Outcome::Failed(_)));
    }

    #[test]
    fn a_missing_database_is_made_empty_and_web_config_pointed_at_it_with_a_yes() {
        let world = World::new();
        let folder = world.site("shop", Some(SiteSql::new(r".\SQLEXPRESS", "live", "", "")));
        let req = HostExistingRequest { setup_iis: false, setup_database: true, ..HostExistingRequest::new("shop") };
        let answer = Answer::yes();
        let (outcome, lines) = host(&world, &req, &answer);
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(
            world.journal(),
            vec!["create [shop]", "login dnn_shop owns [shop]", "web.config -> [shop] on localhost,1433 as dnn_shop",]
        );
        assert_eq!(answer.1.lock().unwrap()[0], "Point web.config at [shop] on localhost,1433?");
        assert!(lines.contains(&r"web.config currently connects to [live] on .\SQLEXPRESS.".to_string()));
        assert!(lines.contains(&"The database is empty: open the site to run the DNN install wizard, or put a backup in with Restore backup.".to_string()));
        assert_eq!(world.web_config.read_site_sql_server(&folder.join("web.config")).unwrap().user, "dnn_shop");

        // A no leaves web.config, and says how to connect.
        let world = World::new();
        world.site("shop", Some(SiteSql::new(r".\SQLEXPRESS", "live", "", "")));
        world.has_database("shop");
        let (_, lines) = host(&world, &req, &Answer::no());
        assert!(lines.contains(&"ok Database [shop] already exists on localhost,1433 - keeping its data.".to_string()));
        assert!(
            lines.iter().any(|l| l.starts_with("web.config left unchanged. Connect with: server 'localhost,1433'"))
        );
    }

    #[test]
    fn web_config_already_on_its_own_local_database_is_left_alone_unless_another_site_uses_it() {
        let mut world = World::new();
        world.site("shop", Some(SiteSql::new("localhost,1433", "shopdb", "dnn_shop", "x")));
        world.has_database("shopdb");
        let req = HostExistingRequest { setup_iis: false, setup_database: true, ..HostExistingRequest::new("shop") };
        let (_, lines) = host(&world, &req, &Answer::yes());
        assert!(lines.contains(&"web.config already uses [shopdb] on localhost,1433 - left unchanged.".to_string()));
        world.sites.using = vec![("other".into(), "shopdb".into())];
        let (_, lines) = host(&world, &req, &Answer::yes());
        assert!(lines.contains(
            &"web.config names [shopdb], the database of the IIS site 'other' - this project gets a database of its own, [shop]."
                .to_string()
        ));
    }

    #[test]
    fn a_restore_over_an_existing_database_goes_beside_it_and_is_swapped_in() {
        let world = World::new();
        world.site("shop", None);
        world.has_database("shop");
        let file = backup(&world, "live.bak");
        let req = HostExistingRequest {
            setup_iis: false,
            setup_database: true,
            backup_file: Some(file.clone()),
            ..HostExistingRequest::new("shop")
        };
        let (outcome, lines) = host(&world, &req, &Answer::yes());
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        let journal = world.journal();
        assert_eq!(journal[0], format!("restore [shop_restore_20261116_101500] from {}", file.display()));
        assert_eq!(journal[1], "alias [shop_restore_20261116_101500] *.dnndev.me -> shop.dnndev.me");
        assert_eq!(journal[2], "ssl off [shop_restore_20261116_101500]");
        assert_eq!(journal[3], "rename [shop] to [shop_before_restore_20261116_101500]");
        assert_eq!(journal[4], "rename [shop_restore_20261116_101500] to [shop]");
        assert_eq!(journal[5], "drop [shop_before_restore_20261116_101500]");

        // A failed restore leaves the one that is there.
        let world = World::new();
        world.site("shop", None);
        world.has_database("shop");
        world.fail("restore_local", "bad media");
        let (outcome, _) = host(&world, &req, &Answer::yes());
        assert_eq!(outcome, Outcome::Failed("Restoring live.bak failed: bad media [shop] is left as it was.".into()));
        assert!(world.journal().ends_with(&["drop [shop_restore_20261116_101500]".to_string()]));
    }

    #[test]
    fn a_no_keeps_the_existing_database() {
        let world = World::new();
        world.site("shop", None);
        world.has_database("shop");
        let file = backup(&world, "live.bacpac");
        let req = HostExistingRequest {
            setup_iis: false,
            setup_database: true,
            backup_file: Some(file),
            ..HostExistingRequest::new("shop")
        };
        let answer = Answer::no();
        let (outcome, lines) = host(&world, &req, &answer);
        assert_eq!(outcome, Outcome::Finished);
        assert_eq!(answer.1.lock().unwrap()[0], "Database [shop] already exists - replace it with live.bacpac?");
        assert!(lines.contains(&"Kept the existing database [shop] - live.bacpac was not restored.".to_string()));
        assert!(world.journal().is_empty());
    }

    #[test]
    fn an_import_extracts_then_hosts_with_a_database_of_its_own() {
        let world = World::new();
        let file = backup(&world, "live.bacpac");
        let req = ImportRequest {
            project_name: "shop".into(),
            zip_path: world.root().join("site.zip"),
            backup_file: file,
            host_name: Some("shop.local".into()),
            port: Some(8080),
        };
        let (outcome, lines) = import(&world, &req);
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(
            world.journal(),
            vec![
                "extract site.zip",
                "gitignore",
                "iis features",
                "disable redirects",
                "sqlpackage",
                "import [shop] from live.bacpac",
                "alias [shop] *.dnndev.me -> shop.local:8080",
                "ssl off [shop]",
                "login dnn_shop owns [shop]",
                "web.config -> [shop] on localhost,1433 as dnn_shop",
                "GET http://shop.local:8080",
            ]
        );
        assert_eq!(lines[0], "## Extracting site.zip");
        assert!(lines.contains(&"Database backup: live.bacpac.".to_string()));
        assert!(world.iis.calls.lock().unwrap().contains(&"create shop.local:8080 shop".to_string()));
    }

    #[test]
    fn a_failed_import_undoes_everything_last_first_and_removes_the_folder() {
        let world = World::new();
        let file = backup(&world, "live.bak");
        world.fail("grant_site_login", "denied");
        let req = ImportRequest {
            project_name: "shop".into(),
            zip_path: world.root().join("site.zip"),
            backup_file: file,
            host_name: None,
            port: None,
        };
        let (outcome, lines) = import(&world, &req);
        assert_eq!(outcome, Outcome::Failed("Could not make the site's login dnn_shop: denied".into()));
        let folder = world.root().join("DNN").join("shop");
        let undo: Vec<String> = lines.iter().skip_while(|l| *l != "## Undo").cloned().collect();
        assert_eq!(
            undo,
            vec![
                "## Undo".to_string(),
                "ok Drop database [shop]".into(),
                "ok Put web.config back as it was".into(),
                "ok Remove the IIS site and app pool 'shop'".into(),
                format!("ok Delete {}", folder.display()),
            ]
        );
        assert!(!folder.exists());
    }

    #[test]
    fn an_import_refuses_an_existing_folder_and_a_file_that_isnt_a_backup() {
        let world = World::new();
        world.site("shop", None);
        let req = ImportRequest {
            project_name: "shop".into(),
            zip_path: world.root().join("site.zip"),
            backup_file: backup(&world, "web.config.bak"),
            host_name: None,
            port: None,
        };
        let (outcome, _) = import(&world, &req);
        let Outcome::Failed(e) = outcome else { panic!() };
        assert!(e.starts_with("Not a .bacpac or .bak file: "), "{e}");
        let req = ImportRequest { backup_file: backup(&world, "live.bak"), ..req };
        let (outcome, _) = import(&world, &req);
        let Outcome::Failed(e) = outcome else { panic!() };
        assert!(e.ends_with("already exists - choose another name."), "{e}");
    }

    #[test]
    fn a_failed_extraction_removes_the_half_extracted_folder() {
        let world = World::new();
        world.fail("extract_zip", "Not a valid .zip file: site.zip");
        let req = ImportRequest {
            project_name: "shop".into(),
            zip_path: world.root().join("site.zip"),
            backup_file: backup(&world, "live.bak"),
            host_name: None,
            port: None,
        };
        let (outcome, _) = import(&world, &req);
        assert_eq!(outcome, Outcome::Failed("Not a valid .zip file: site.zip".into()));
        assert!(!world.root().join("DNN").join("shop").exists());
    }
}
