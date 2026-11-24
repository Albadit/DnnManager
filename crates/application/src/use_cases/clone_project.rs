//! "Clone…" (`UseCases/CloneProjectUseCase.cs`): a copy of a project - its files, its database copied into one of its
//! own on the local SQL Server, and an IIS website of its own. Everything that can be checked is checked before a file
//! is copied: the name, the source's database, the local SQL Server, and - with a yes - a database of the clone's name
//! that is already there. That one is only replaced once the copy is in: the copy is seeded under a name of its own
//! and swapped in at the end.
//!
//! Deviation: the C#'s `reporter.Context` (the SQL Server and the site's host name next to the operation's title) has
//! no counterpart in the port's `Reporter` and is left out.

use std::path::{Path, PathBuf};

use dnnm_domain::{DatabaseConfig, DnnProject, validate_project_name};

use crate::operation::{CANCELLED, Operation};
use crate::use_cases::hosting::{
    HostingDeps, SiteSql, backup_folder, bind_portals, check_local_sql, database_file_name, disable_ssl,
    drop_database_on_undo, drop_login_on_undo, grant_site_login, localize_portals, swap_in, worded,
};
use crate::use_cases::portal_aliases::Localized;
use crate::use_cases::provisioner;

/// What to clone, and into what (`CloneProjectRequest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRequest {
    pub target_project_name: String,
    /// The folder holding the DNN site to copy.
    pub source_directory: PathBuf,
    /// Where the source's SQL Server writes the .bak (must be readable from this PC too).
    pub source_backup_server_path: String,
    pub create_iis_site: bool,
    /// Copy (overwrite) the website files; false keeps the files already in the target folder.
    pub copy_files: bool,
    /// Back the source's database up and seed the clone's; false leaves the database alone.
    pub seed_database: bool,
}

impl CloneRequest {
    /// Everything: files, database and IIS site.
    pub fn new(target: &str, source_directory: &Path, source_backup_server_path: &str) -> Self {
        Self {
            target_project_name: target.to_string(),
            source_directory: source_directory.to_path_buf(),
            source_backup_server_path: source_backup_server_path.to_string(),
            create_iis_site: true,
            copy_files: true,
            seed_database: true,
        }
    }
}

/// The clone's stages, by their short names in the Output's stage list.
mod stage {
    pub const PREPARE: &str = "Prepare target";
    pub const COPY: &str = "Copy website files";
    pub const KEEP: &str = "Keep website files";
    pub const CHECK_SQL: &str = "Check local SQL Server";
    pub const READ_CONNECTION: &str = "Read SiteSqlServer";
    pub const BACKUP: &str = "Back up source DB";
    pub const SEED: &str = "Seed database";
    pub const ALIAS: &str = "Update PortalAlias";
    pub const WEB_CONFIG: &str = "Rewrite web.config";
    pub const IIS: &str = "Create IIS site";
}

/// Where the copy of the source's database is kept, and how old backups go: it holds all of the source's data.
pub fn kept_copy_note(file: &Path) -> String {
    format!(
        "A copy of the source's database is kept with the project's backups: {}. It holds all of the source's data - \
         Settings → Projects → Backups (Keep backups and deployment packages) can delete old backups by themselves; \
         until that is set they are kept for good.",
        file.display()
    )
}

/// `source`: the source's database as its web.config has it. `target`: the clone's, by its final name. `replaces`: a
/// database of that name is there already - the copy goes in beside it and replaces it at the end.
struct DatabasePlan {
    source: SiteSql,
    target: DatabaseConfig,
    replaces: bool,
    source_is_azure: bool,
}

/// `CloneProjectUseCase.ExecuteAsync`.
pub fn clone_project(deps: &HostingDeps, req: &CloneRequest, op: &mut Operation) -> Result<(), String> {
    let name = req.target_project_name.as_str();
    validate_project_name(name)?;
    let project = deps.settings.project(name);
    let hostname = deps.settings.hostname_for(name);
    // A site of this name that serves another folder is another project's: making the clone's would replace it.
    if req.create_iis_site
        && let Some(taken) = provisioner::site_name_taken(deps.iis.as_ref(), name, &project.project_directory)
    {
        return Err(taken);
    }

    // The stages, up front: the Output shows those still to come.
    let mut plan = vec![stage::PREPARE];
    if req.seed_database {
        plan.extend([stage::READ_CONNECTION, stage::CHECK_SQL]);
    }
    plan.push(if req.copy_files { stage::COPY } else { stage::KEEP });
    if req.seed_database {
        plan.extend([stage::BACKUP, stage::SEED, stage::ALIAS, stage::WEB_CONFIG]);
    }
    if req.create_iis_site {
        plan.push(stage::IIS);
    }
    op.report.plan(&plan);

    op.report.step(&format!("Prepare target project '{name}'"));

    // Before a file is copied: the source's database, the local SQL Server, and the clone's own database.
    let database = if req.seed_database { Some(prepare_database(deps, req, &project, op)?) } else { None };
    op.cancel.check()?;

    // A cancel or a failure takes a new target folder away again; files copied over an existing one can't be.
    let directory = &project.project_directory;
    let new_folder = !directory.exists();
    op.undo.delete_folder_on_undo(directory);
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;

    if req.copy_files {
        if !new_folder {
            op.undo.cannot_undo(format!("the website files copied over the ones in {}.", directory.display()));
        }
        op.report.step("Copy website files");
        deps.files.copy_site(&req.source_directory, directory, op.report, &op.cancel)?;
    } else {
        op.report.step("Keep the existing website files");
        op.report.info("Skipped file copy - using the files already in the target folder.");
    }

    // The IIS URL Rewrite section: production-only rules (HTTPS redirect, request blocking) that need the URL Rewrite
    // module, which is usually absent locally - IIS would answer 500.19. DNN doesn't need them for local development.
    let web_config = directory.join("web.config");
    if web_config.exists() {
        op.undo.restore_file_on_undo(&web_config);
        if deps.web_config.remove_rewrite_rules(&web_config).is_ok() {
            op.report.info("Removed URL Rewrite rules (not needed locally).");
        }
    }

    // A DNN-tuned .gitignore, so the clone is ready to commit - the source's own is kept.
    op.undo.restore_file_on_undo(&directory.join(".gitignore"));
    match deps.files.ensure_gitignore(directory) {
        Ok(()) => op.report.info("Project .gitignore ready."),
        Err(e) => op.report.info(&format!("Could not write .gitignore: {e}")),
    }

    let mut kept_copy = None;
    let mut portals = Vec::new();
    match database {
        None => {
            op.report.info("Skipping database - website files only.");
            // Its web.config is the source's: the copy would work in the source's database, which may be a live one.
            if web_config.exists()
                && let Ok(c) = deps.web_config.read_site_sql_server(&web_config)
                && !c.database.is_empty()
            {
                op.report.warn(&format!(
                    "web.config still connects to [{}] on {} - the source's database. The copy changes that database's \
                     data; point it at a database of its own (Details → Database) before using it.",
                    c.database, c.server
                ));
            }
        }
        Some(plan) => {
            (kept_copy, portals) = seed(deps, req, &project, &plan, op)?;
            op.cancel.check()?;

            op.report.step("Rewrite web.config to use local database");
            // The copy signs in with a login of its own, owner of its database only - not the container's sa.
            let login = grant_site_login(deps, name, &plan.target, &op.cancel)?;
            drop_login_on_undo(deps, &login.user, op);
            op.undo.restore_file_on_undo(&web_config);
            deps.web_config.write_site_sql_server(&web_config, &login)?;
            op.report.success("web.config updated.");
        }
    }

    // An IIS site - skipped (not fatal) when IIS is absent or creating it fails.
    let mut site_created = false;
    if req.create_iis_site && deps.iis.is_available() {
        op.cancel.check()?;
        op.report.step("Create IIS site");
        site_created = provisioner::try_create_site(&deps.iis, name, directory, &hostname, deps.settings.site_port, op);
        if site_created {
            bind_portals(deps, name, &portals, op);
        }
    } else if req.create_iis_site {
        op.report.info("Skipped IIS site - IIS not available.");
    }

    op.report.step("Clone complete");
    if let Some(kept) = kept_copy {
        op.report.info(&kept_copy_note(&kept));
    }
    if site_created {
        op.report.link(&deps.settings.site_url_for(name));
    } else {
        op.report.success(&format!(
            "Cloned files are ready in {}. Point a web server (and database) at them to use the site.",
            directory.display()
        ));
    }
    Ok(())
}

/// Everything about the database that can be known before a file is copied: the source's connection, SqlPackage for
/// an Azure source, the local SQL Server, and whether a database of the clone's name may be replaced - asked, and
/// refused when another site uses it. A no ends the clone as cancelled.
fn prepare_database(
    deps: &HostingDeps,
    req: &CloneRequest,
    project: &DnnProject,
    op: &mut Operation,
) -> Result<DatabasePlan, String> {
    op.report.step("Read SiteSqlServer from the source's web.config");
    let source_config = req.source_directory.join("web.config");
    let source = if source_config.exists() {
        deps.web_config.read_site_sql_server(&source_config)?
    } else {
        return Err(format!("The source has no web.config ({}).", source_config.display()));
    };
    if source.database.is_empty() {
        return Err("Could not read the SiteSqlServer connection from the source's web.config.".into());
    }
    op.report.success(&format!(
        "Source DB: [{}] on {} ({})",
        source.database,
        source.server,
        if source.user.is_empty() { "Windows authentication".to_string() } else { format!("user: {}", source.user) }
    ));

    // Azure SQL Database can't make a .bak: it is cloned through a BACPAC (SqlPackage export and import) - SqlPackage
    // is provisioned now, before anything else.
    let source_is_azure = source.server.to_lowercase().contains("database.windows.net");
    if source_is_azure {
        deps.copies.ensure_sqlpackage(op.report, &op.cancel)?;
    }

    op.report.step("Check local SQL Server");
    let port = check_local_sql(deps, op)
        .map_err(|e| worded(e, |e| format!("{e} Nothing was copied - or clone the website files only.")))?;

    let name = &req.target_project_name;
    let target = deps.settings.database_for(project, &deps.settings.database_name_for(name), port);
    if deps.sql.is_local_container(&source.server, port) && source.database.eq_ignore_ascii_case(&target.database_name)
    {
        return Err(format!("The source already uses [{}] - the clone needs a name of its own.", target.database_name));
    }
    let exists = deps
        .sql
        .database_exists(&target.database_name, &op.cancel)
        .map_err(|e| worded(e, |e| format!("Could not check for database [{}]: {e}", target.database_name)))?;
    if exists {
        if let Some(other) = deps.sites.other_site_using(name, &target.server, &target.database_name) {
            return Err(format!(
                "[{}] is the database of the IIS site '{other}' - choose another name for the clone.",
                target.database_name
            ));
        }
        let question = format!(
            "Database [{}] already exists on {}. Replace it with a copy of [{}]? Everything in it now is lost.",
            target.database_name, target.server, source.database
        );
        if !op.prompt.confirm(&question, "Replace database", "Keep it", true) {
            return Err(CANCELLED.into());
        }
    }
    Ok(DatabasePlan { source, target, replaces: exists, source_is_azure })
}

/// Copies the source's database into the clone's: a BACPAC export and import from Azure, a backup and restore from
/// anywhere else - under a name of its own when a database of the clone's name is there, swapped in only once it is
/// complete, with its portal aliases and SSL set for the local site. The copy kept in the project's backups, and each
/// portal's local address.
fn seed(
    deps: &HostingDeps,
    req: &CloneRequest,
    project: &DnnProject,
    plan: &DatabasePlan,
    op: &mut Operation,
) -> Result<(Option<PathBuf>, Vec<Localized>), String> {
    let stamp = deps.machine.stamp();
    let name = plan.target.database_name.clone();
    let mut incoming = plan.target.clone();
    if plan.replaces {
        incoming.database_name = format!("{name}_clone_{stamp}");
    }
    // The copy - dropped again by a cancel or a failure; the database it replaces is untouched until the end.
    drop_database_on_undo(deps, &incoming.database_name, op);

    let folder = backup_folder(project, &stamp);
    op.undo.delete_folder_on_undo(&folder);
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;

    let mut kept = None;
    if plan.source_is_azure {
        op.report.step("Export the source database (BACPAC)");
        let temp =
            deps.files.private_temp()?.join(format!("dnnmanager_clone_{}_{stamp}.bacpac", req.target_project_name));
        let result = (|| {
            deps.copies.export_bacpac(&plan.source, &temp, op.report, &op.cancel)?;
            // Kept with the project's backups, so the copy can be made again from the same data.
            let cached = folder.join(database_file_name(project, ".bacpac"));
            match std::fs::copy(&temp, &cached) {
                Ok(_) => {
                    op.report.info(&format!("Copy of the source's database kept at {}", cached.display()));
                    kept = Some(cached);
                }
                Err(e) => op.report.info(&format!("Could not keep a copy of the BACPAC in the project's backups: {e}")),
            }
            op.report.step(&format!("Seed [{name}] from the BACPAC"));
            deps.copies.import_bacpac(&incoming.server, &incoming.database_name, &temp, op.report, &op.cancel)
        })();
        // The source's whole database - never left behind, whatever happened.
        let _ = std::fs::remove_file(&temp);
        result?;
    } else {
        deps.sql.create_database(&incoming, &op.cancel)?;

        // From the local SQL container the backup goes through the container, not a Windows path it can't see.
        op.report.step("Back up source database");
        let source_bak = if deps.sql.is_local_container(&plan.source.server, plan.target.port) {
            op.report.info("Source DB is on the local SQL container - using container backup path.");
            let file_name = Path::new(&req.source_backup_server_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = deps.sql.backup_local(&plan.source.database, &file_name, &op.cancel).map_err(|e| {
                if e.is_empty() { "Source backup on the local SQL container failed.".to_string() } else { e }
            })?;
            op.report.success(&format!("Source backup written to {}", path.display()));
            path
        } else {
            let path = deps
                .copies
                .backup_remote(&plan.source, &req.source_backup_server_path, op.report, &op.cancel)
                .map_err(|e| if e.is_empty() { "Source backup failed.".to_string() } else { e })?;
            PathBuf::from(path)
        };

        let project_bak = folder.join(database_file_name(project, ".bak"));
        let copied = std::fs::copy(&source_bak, &project_bak);
        let _ = std::fs::remove_file(&source_bak);
        copied.map_err(|e| format!("{}: {e}", project_bak.display()))?;
        op.report.info(&format!("Copy of the source's database kept at {}", project_bak.display()));
        kept = Some(project_bak.clone());

        op.report.step(&format!("Seed [{name}] from clone backup"));
        deps.sql.restore_local(&incoming, &project_bak, &op.cancel)?;
    }
    op.report.success(&format!("Database copied into [{}].", incoming.database_name));
    op.cancel.check()?;

    // So the clone answers at its own address instead of the source's - every portal.
    op.report.step("Update PortalAlias to match new hostname");
    let alias = provisioner::alias_for(&deps.settings.hostname_for(&req.target_project_name), deps.settings.site_port);
    let portals = localize_portals(deps, &incoming.database_name, &alias, op)?;
    disable_ssl(deps, &incoming.database_name, op);

    if plan.replaces {
        swap_in(deps, &name, &incoming.database_name, &format!("{name}_before_clone_{stamp}"), "the copy", op)?;
    } else {
        op.report.success(&format!("Local database [{name}] ready."));
    }
    Ok((kept, portals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Outcome, run};
    use crate::use_cases::hosting::fakes::World;
    use crate::use_cases::tests::FakeIis;
    use std::sync::Arc;

    fn source(world: &World, server: &str) -> PathBuf {
        world.site("shop", Some(SiteSql::new(server, "shop", "dnn_shop", "x")))
    }

    fn request(world: &World, source: &Path) -> CloneRequest {
        let bak = world.root().join("server_shop.bak");
        CloneRequest::new("shop_copy", source, &bak.display().to_string())
    }

    fn clone(world: &World, req: &CloneRequest, answer: &Answer) -> (Outcome, Vec<String>) {
        let deps = world.deps();
        let mut lines = Lines::default();
        let outcome =
            run(&mut lines, answer, CancelToken::new(), &CancelToken::new(), |op| clone_project(&deps, req, op));
        (outcome, lines.0)
    }

    #[test]
    fn a_clone_from_another_server_backs_up_restores_and_hosts_in_order() {
        let world = World::new();
        let src = source(&world, "sql.example.com");
        let (outcome, lines) = clone(&world, &request(&world, &src), &Answer::yes());
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        let bak =
            world.root().join("backups").join("shop_copy").join("shop_copy_20261116_101500").join("shop_copy.bak");
        assert_eq!(
            world.journal(),
            vec![
                "copy files".to_string(),
                "remove rewrite".into(),
                "gitignore".into(),
                "create [shop_copy]".into(),
                "backup remote [shop] on sql.example.com".into(),
                format!("restore [shop_copy] from {}", bak.display()),
                "alias [shop_copy] *.dnndev.me -> shop_copy.dnndev.me".into(),
                "ssl off [shop_copy]".into(),
                "login dnn_shop_copy owns [shop_copy]".into(),
                "web.config -> [shop_copy] on localhost,1433 as dnn_shop_copy".into(),
            ]
        );
        assert!(bak.exists() && !world.root().join("server_shop.bak").exists(), "kept, and the server's copy gone");
        let steps: Vec<&String> = lines.iter().filter(|l| l.starts_with("## ")).collect();
        assert_eq!(
            steps,
            vec![
                "## Prepare target project 'shop_copy'",
                "## Read SiteSqlServer from the source's web.config",
                "## Check local SQL Server",
                "## Copy website files",
                "## Back up source database",
                "## Seed [shop_copy] from clone backup",
                "## Update PortalAlias to match new hostname",
                "## Rewrite web.config to use local database",
                "## Create IIS site",
                "## Clone complete",
            ]
        );
        assert!(lines.contains(&"ok Source DB: [shop] on sql.example.com (user: dnn_shop)".to_string()));
        assert!(lines.contains(&kept_copy_note(&bak)));
        assert!(world.iis.calls.lock().unwrap().contains(&"create shop_copy.dnndev.me:80 shop_copy".to_string()));
    }

    #[test]
    fn a_failure_after_the_copy_undoes_last_step_first() {
        let world = World::new();
        let src = source(&world, "sql.example.com");
        world.fail("write_site_sql_server", "read-only");
        let (outcome, lines) = clone(&world, &request(&world, &src), &Answer::yes());
        assert_eq!(outcome, Outcome::Failed("read-only".into()));
        let undo: Vec<String> = lines.iter().skip_while(|l| *l != "## Undo").cloned().collect();
        let target = world.root().join("DNN").join("shop_copy");
        let backups = world.root().join("backups").join("shop_copy").join("shop_copy_20261116_101500");
        assert_eq!(
            undo,
            vec![
                "## Undo".to_string(),
                "ok Drop the login dnn_shop_copy".into(),
                format!("ok Delete {}", backups.display()),
                "ok Drop database [shop_copy]".into(),
                format!("ok Delete {}", target.join(".gitignore").display()),
                "ok Put web.config back as it was".into(),
                format!("ok Delete {}", target.display()),
            ]
        );
        assert!(!target.exists() && !backups.exists());
        assert!(world.journal().ends_with(&["drop login dnn_shop_copy".to_string(), "drop [shop_copy]".into()]));
    }

    #[test]
    fn a_database_of_that_name_is_replaced_only_with_a_yes_and_only_at_the_end() {
        let world = World::new();
        world.has_database("shop_copy");
        let src = source(&world, "sql.example.com");
        let no = Answer::no();
        let (outcome, lines) = clone(&world, &request(&world, &src), &no);
        assert_eq!(outcome, Outcome::Cancelled);
        assert!(lines.iter().all(|l| !l.starts_with("## Copy")));
        assert_eq!(
            no.1.lock().unwrap()[0],
            "Database [shop_copy] already exists on localhost,1433. Replace it with a copy of [shop]? Everything in it now is lost."
        );
        assert!(world.journal().is_empty());

        let (outcome, _) = clone(&world, &request(&world, &src), &Answer::yes());
        assert_eq!(outcome, Outcome::Finished);
        let journal = world.journal();
        assert_eq!(journal[3], "create [shop_copy_clone_20261116_101500]");
        assert!(journal.contains(&"rename [shop_copy] to [shop_copy_before_clone_20261116_101500]".to_string()));
        assert!(journal.contains(&"rename [shop_copy_clone_20261116_101500] to [shop_copy]".to_string()));
        assert!(journal.contains(&"drop [shop_copy_before_clone_20261116_101500]".to_string()));
    }

    #[test]
    fn a_database_another_site_uses_is_never_replaced() {
        let mut world = World::new();
        world.has_database("shop_copy");
        world.sites.using = vec![("other".into(), "shop_copy".into())];
        let src = source(&world, "sql.example.com");
        let (outcome, _) = clone(&world, &request(&world, &src), &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed(
                "[shop_copy] is the database of the IIS site 'other' - choose another name for the clone.".into()
            )
        );
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn a_source_in_the_container_is_backed_up_there_and_one_with_the_clones_name_is_refused() {
        let world = World::new();
        let src = source(&world, "localhost,1433");
        let req = CloneRequest { source_backup_server_path: r"C:\x\clone.bak".into(), ..request(&world, &src) };
        let deps = world.deps();
        let mut lines = Lines::default();
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            clone_project(&deps, &req, op)
        });
        assert_eq!(outcome, Outcome::Finished, "{:#?}", lines.0);
        assert!(!world.root().join("clone.bak").exists(), "the copy out of the container is moved on");
        assert!(world.journal().contains(&"backup local [shop] clone.bak".to_string()));
        assert!(
            lines.0.contains(&"Source DB is on the local SQL container - using container backup path.".to_string())
        );

        let req = CloneRequest { target_project_name: "shop".into(), ..request(&world, &src) };
        let (outcome, _) = clone(&world, &req, &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed("The source already uses [shop] - the clone needs a name of its own.".into())
        );
    }

    #[test]
    fn an_azure_source_goes_through_a_bacpac_that_is_never_left_behind() {
        let world = World::new();
        let src = source(&world, "tcp:shop.database.windows.net,1433");
        let (outcome, lines) = clone(&world, &request(&world, &src), &Answer::yes());
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        let journal = world.journal();
        assert_eq!(journal[0], "sqlpackage", "provisioned before a file is copied");
        assert_eq!(journal[1], "copy files");
        assert!(journal[4].starts_with("export [shop] to "));
        assert_eq!(journal[5], "import [shop_copy] from dnnmanager_clone_shop_copy_20261116_101500.bacpac");
        assert!(!world.root().join("dnnmanager_clone_shop_copy_20261116_101500.bacpac").exists());
        assert!(lines.contains(&"## Seed [shop_copy] from the BACPAC".to_string()));
        let cached =
            world.root().join("backups").join("shop_copy").join("shop_copy_20261116_101500").join("shop_copy.bacpac");
        assert!(cached.exists());
    }

    #[test]
    fn files_only_warns_that_the_copy_uses_the_sources_database() {
        let world = World::new();
        let src = source(&world, "sql.example.com");
        std::fs::write(src.join("Default.aspx"), "x").unwrap();
        let req = CloneRequest { seed_database: false, create_iis_site: false, ..request(&world, &src) };
        // The copy's web.config is the source's: the stand-in reads the same connection there.
        let copy_config = world.root().join("DNN").join("shop_copy").join("web.config");
        world
            .web_config
            .connections
            .lock()
            .unwrap()
            .insert(copy_config, SiteSql::new("sql.example.com", "shop", "u", "p"));
        let (outcome, lines) = clone(&world, &req, &Answer::yes());
        assert_eq!(outcome, Outcome::Finished);
        assert!(lines.contains(&"Skipping database - website files only.".to_string()));
        assert!(lines.iter().any(|l| l.starts_with("warn web.config still connects to [shop] on sql.example.com")));
        assert!(lines.last().unwrap().starts_with("ok Cloned files are ready in "));
    }

    #[test]
    fn a_site_of_that_name_serving_another_folder_is_never_taken() {
        let mut world = World::new();
        world.iis = Arc::new(FakeIis::running().with_site("shop_copy", r"D:\elsewhere"));
        let src = source(&world, "sql.example.com");
        let (outcome, lines) = clone(&world, &request(&world, &src), &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed(
                r"IIS already has a site named 'shop_copy', serving D:\elsewhere - choose another name.".into()
            )
        );
        assert_eq!(lines.len(), 1);
        assert!(world.journal().is_empty());
    }

    #[test]
    fn an_unreachable_local_server_stops_before_a_file_is_copied() {
        let world = World::new();
        world.fail("test_login", "refused");
        let src = source(&world, "sql.example.com");
        let (outcome, _) = clone(&world, &request(&world, &src), &Answer::yes());
        let Outcome::Failed(e) = outcome else { panic!() };
        assert!(e.ends_with("Nothing was copied - or clone the website files only."), "{e}");
        assert!(!world.root().join("DNN").join("shop_copy").exists());
    }
}
