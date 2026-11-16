//! `EditBindingsUseCase` (`UseCases/EditProjectUseCases.cs`): Edit host names - the site's http bindings as the dialog
//! left them, with DNN's portal aliases following: a changed host name's alias is renamed, a new one's added. Its https
//! and other bindings stay. A LocalDB file database can only be opened while the site doesn't use it, so that site is
//! stopped meanwhile and started again if it ran.

use std::path::PathBuf;
use std::sync::Arc;

use crate::iis::{IisBinding, IisManager, states};
use crate::operation::{CANCELLED, CancelToken, Operation, STEP_LIMIT};
use crate::use_cases::database_connection::{DatabaseConnection, DatabaseKind, SqlAuthentication};
use crate::use_cases::provisioner::alias_for;
use crate::use_cases::rename_project::{SiteDatabases, http_bindings};

/// One of a site's http bindings as Edit host names leaves it (`HttpBindingEdit`): `host` on `port`, and what it was -
/// None for one added - so DNN's portal alias of the old host and port can follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpBindingEdit {
    pub old_host: Option<String>,
    pub old_port: Option<u16>,
    /// Empty: any host name.
    pub host: String,
    pub port: u16,
}

impl HttpBindingEdit {
    /// "host[:port]" as DNN's alias names it, or "*:port" for any host name - the Output's and the toast's form.
    pub fn shown(&self) -> String {
        if self.host.is_empty() { format!("*:{}", self.port) } else { alias_for(&self.host, self.port) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBindingsRequest {
    pub site_name: String,
    /// The folder the site serves - its web.config names the database with DNN's aliases.
    pub directory: PathBuf,
    pub bindings: Vec<HttpBindingEdit>,
}

/// What Edit host names acts on.
#[derive(Clone)]
pub struct EditBindingsDeps {
    pub iis: Arc<dyn IisManager>,
    pub databases: Arc<dyn SiteDatabases>,
}

/// A site's bindings as the dialog shows them (`ProjectEdits.EditBindings`): the http ones with a port, to edit, and
/// the others as text ("https *:443:www.example.com"), kept as they are.
pub fn split_bindings(bindings: &[IisBinding]) -> (Vec<(String, u16)>, Vec<String>) {
    let is_http = |b: &IisBinding| b.protocol.eq_ignore_ascii_case("http") && b.port.is_some();
    let http = bindings.iter().filter(|b| is_http(b)).filter_map(|b| b.port.map(|p| (b.host.clone(), p))).collect();
    let others = bindings
        .iter()
        .filter(|b| !is_http(b))
        .map(|b| {
            format!("{} {}:{}:{}", b.protocol, b.address, b.port.map(|p| p.to_string()).unwrap_or_default(), b.host)
        })
        .collect();
    (http, others)
}

/// `EditBindingsUseCase.ExecuteAsync`.
pub fn edit_bindings(deps: &EditBindingsDeps, req: &EditBindingsRequest, op: &mut Operation) -> Result<(), String> {
    let site_name = req.site_name.as_str();
    if req.bindings.is_empty() {
        return Err("A site needs at least one http binding.".into());
    }
    let Some(site) = deps.iis.site_details(site_name) else {
        return Err(format!("IIS has no site named '{site_name}'."));
    };
    let before = http_bindings(&site.bindings);

    op.report.step("Changing the bindings");
    {
        let (iis, name, before) = (deps.iis.clone(), site_name.to_string(), before.clone());
        op.undo.add("Put the bindings back", STEP_LIMIT, move |_| {
            iis.replace_http_bindings(&name, &before).map_err(|e| e.0)
        });
    }
    let next: Vec<(String, u16)> = req.bindings.iter().map(|b| (b.host.clone(), b.port)).collect();
    deps.iis.replace_http_bindings(site_name, &next).map_err(|e| format!("Could not change the bindings: {e}"))?;
    for b in &req.bindings {
        op.report.success(&format!("http://{}", b.shown()));
    }

    // An alias is a host name with the port when it isn't 80; a binding to any host has none.
    let renamed: Vec<(String, String)> = req
        .bindings
        .iter()
        .filter_map(|b| {
            let (old_host, old_port) = (b.old_host.as_deref()?, b.old_port?);
            (!old_host.is_empty()
                && !b.host.is_empty()
                && (!old_host.eq_ignore_ascii_case(&b.host) || old_port != b.port))
                .then(|| (alias_for(old_host, old_port), alias_for(&b.host, b.port)))
        })
        .collect();
    let added: Vec<String> = req
        .bindings
        .iter()
        .filter(|b| b.old_host.is_none() && !b.host.is_empty())
        .map(|b| alias_for(&b.host, b.port))
        .collect();
    if renamed.is_empty() && added.is_empty() {
        return Ok(());
    }

    op.cancel.check()?;
    op.report.step("DNN's portal aliases");
    let Some(database) = deps.databases.database_of(&req.directory) else {
        op.report.warn(
            "web.config names no database - DNN's portal aliases stay as they are. Add the new host names as site aliases in DNN.",
        );
        return Ok(());
    };
    // The database web.config names - the site's app pool can change it - gets DNN's aliases changed with DNN Manager's
    // rights: asked first when it isn't the project's own, or when signing in to it as you would hand your Windows
    // sign-in to a server elsewhere.
    if let Some(kept) = database_refused(deps, site_name, &database, op) {
        op.report.warn(&format!(
            "{kept} - DNN's portal aliases stay as they are. Add the new host names as site aliases in DNN."
        ));
        return Ok(());
    }
    let local_file = database.kind == DatabaseKind::LocalDbFile;
    let was_running = states::is_started(&site.state);
    if local_file && was_running {
        op.report.info(
            "Stopping the site for a moment - its database file can only be opened while the site doesn't use it…",
        );
        let _ = deps.iis.stop_site(site_name);
    }
    {
        let (databases, db, dir) = (deps.databases.clone(), database.clone(), req.directory.clone());
        let back: Vec<(String, String)> = renamed.iter().map(|(from, to)| (to.clone(), from.clone())).collect();
        op.undo.add("Put DNN's portal aliases back", STEP_LIMIT, move |_| {
            databases.update_portal_aliases(&db, &dir, &back, &[], &CancelToken::new()).map(|_| ())
        });
    }
    let updated = deps.databases.update_portal_aliases(&database, &req.directory, &renamed, &added, &op.cancel);
    // As the C#'s finally: the site runs again whatever became of the aliases.
    if local_file && was_running {
        let _ = deps.iis.start_site(site_name);
    }
    match updated {
        Ok(changed) => op.report.success(&format!("{changed} portal alias(es) changed or added.")),
        Err(e) if e == CANCELLED => return Err(e),
        Err(e) => {
            op.report.warn(&format!("{e} - change the site aliases in DNN (Settings → Site Settings → Site Aliases)."))
        }
    }
    // DNN keeps its aliases in memory: it reads them again after a restart.
    if !local_file {
        let _ = deps.iis.recycle_app_pool(site_name);
    }
    Ok(())
}

/// Why DNN's aliases aren't changed in `database` - the user said no: it isn't the project's own (named like it), or
/// it is reached with their Windows account on a server not on this PC and not the one in Settings; None when they may
/// be (`DatabaseRefusedAsync`).
fn database_refused(
    deps: &EditBindingsDeps,
    site_name: &str,
    database: &DatabaseConnection,
    op: &mut Operation,
) -> Option<String> {
    if database.kind == DatabaseKind::LocalDbFile {
        return None;
    }
    // `AppOptions.DatabaseNameFor`: a project's database is named like it.
    let own = site_name;
    let (name, server) = (&database.database, &database.server);
    if !name.eq_ignore_ascii_case(own)
        && !op.prompt.confirm(
            &format!(
                "The web.config of '{site_name}' names the database [{name}] on {server} - not the project's own, [{own}].\n\nChange DNN's portal aliases in [{name}]?"
            ),
            "Change aliases there",
            "Leave them",
            false,
        )
    {
        return Some(format!("[{name}] isn't the project's own database, and was left as it is"));
    }
    if database.authentication == SqlAuthentication::Windows
        && !deps.databases.may_sign_in_as_user(server)
        && !op.prompt.confirm(
            &deps.databases.sign_in_question(server, "Changing DNN's portal aliases"),
            &format!("Sign in to {server}"),
            "Don't sign in",
            false,
        )
    {
        return Some(format!("Not signed in to {server} with your Windows account"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{Outcome, run};
    use crate::use_cases::rename_project::tests::{Databases, Iis, Log, binding, details, has, sql_database};

    fn edit(old: Option<(&str, u16)>, host: &str, port: u16) -> HttpBindingEdit {
        HttpBindingEdit {
            old_host: old.map(|(h, _)| h.to_string()),
            old_port: old.map(|(_, p)| p),
            host: host.into(),
            port,
        }
    }

    struct Run {
        log: Log,
        outcome: Outcome,
        lines: Vec<String>,
        asked: Vec<String>,
    }

    fn edit_with(
        state: &str,
        database: Option<DatabaseConnection>,
        tune: impl FnOnce(&mut Iis, &mut Databases),
        answer: Answer,
        bindings: Vec<HttpBindingEdit>,
    ) -> Run {
        let log: Log = Arc::default();
        let mut iis = Iis::new(
            &log,
            Some(details("shop", state, vec![binding("http", "shop.me", 80), binding("https", "shop.me", 443)])),
        );
        let mut databases = Databases::new(&log, database);
        tune(&mut iis, &mut databases);
        let deps = EditBindingsDeps { iis: Arc::new(iis), databases: Arc::new(databases) };
        let req = EditBindingsRequest { site_name: "shop".into(), directory: PathBuf::from(r"C:\DNN\shop"), bindings };
        let mut lines = Lines::default();
        let outcome =
            run(&mut lines, &answer, CancelToken::new(), &CancelToken::new(), |op| edit_bindings(&deps, &req, op));
        let asked = answer.1.lock().unwrap().clone();
        Run { log, outcome, lines: lines.0, asked }
    }

    fn logged(run: &Run) -> Vec<String> {
        run.log.lock().unwrap().clone()
    }

    #[test]
    fn rebinds_renames_and_adds_aliases_and_recycles() {
        let r = edit_with(
            "Started",
            Some(sql_database("shop", false)),
            |_, _| {},
            Answer::yes(),
            vec![edit(Some(("shop.me", 80)), "Shop2.me", 8080), edit(None, "new.me", 80), edit(None, "", 81)],
        );
        assert_eq!(r.outcome, Outcome::Finished, "{:#?}", r.lines);
        assert_eq!(
            logged(&r),
            vec![
                "bindings shop Shop2.me:8080,new.me:80,:81",
                "aliases on shop shop.me=>Shop2.me:8080 +new.me",
                "recycle shop",
            ]
        );
        assert_eq!(
            r.lines,
            vec![
                "## Changing the bindings",
                "ok http://Shop2.me:8080",
                "ok http://new.me",
                "ok http://*:81",
                "## DNN's portal aliases",
                "ok 2 portal alias(es) changed or added.",
            ]
        );
        assert!(r.asked.is_empty());
    }

    #[test]
    fn nothing_but_bindings_when_no_alias_changes() {
        let r = edit_with("Started", None, |_, _| {}, Answer::yes(), vec![edit(Some(("shop.me", 80)), "SHOP.me", 80)]);
        assert_eq!(r.outcome, Outcome::Finished);
        assert_eq!(logged(&r), vec!["bindings shop SHOP.me:80"]);
    }

    #[test]
    fn refuses_no_bindings_and_a_missing_site() {
        let r = edit_with("Started", None, |_, _| {}, Answer::yes(), vec![]);
        assert_eq!(r.outcome, Outcome::Failed("A site needs at least one http binding.".into()));
        let r = edit_with(
            "Started",
            None,
            |i, _| *i.details.lock().unwrap() = None,
            Answer::yes(),
            vec![edit(None, "a", 80)],
        );
        assert_eq!(r.outcome, Outcome::Failed("IIS has no site named 'shop'.".into()));
    }

    #[test]
    fn a_refused_binding_change_fails_and_a_later_cancel_puts_both_back() {
        let r =
            edit_with("Started", None, |i, _| i.fail = vec!["bindings"], Answer::yes(), vec![edit(None, "a.me", 80)]);
        assert_eq!(r.outcome, Outcome::Failed("Could not change the bindings: bindings refused".into()));
        assert_eq!(logged(&r), vec!["bindings shop a.me:80", "bindings shop shop.me:80"]);

        let r = edit_with(
            "Started",
            Some(sql_database("shop", false)),
            |_, d| d.fail = vec!["cancel-aliases"],
            Answer::yes(),
            vec![edit(Some(("shop.me", 80)), "b.me", 80)],
        );
        assert_eq!(r.outcome, Outcome::Cancelled);
        assert_eq!(
            logged(&r),
            vec![
                "bindings shop b.me:80",
                "aliases on shop shop.me=>b.me +",
                "aliases on shop b.me=>shop.me +",
                "bindings shop shop.me:80",
            ]
        );
    }

    #[test]
    fn no_database_or_failing_aliases_are_warnings() {
        let r = edit_with("Started", None, |_, _| {}, Answer::yes(), vec![edit(None, "a.me", 80)]);
        assert!(has(
            &r.lines,
            "warn web.config names no database - DNN's portal aliases stay as they are. Add the new host names as site aliases in DNN."
        ));
        let r = edit_with(
            "Started",
            Some(sql_database("shop", false)),
            |_, d| d.fail = vec!["aliases"],
            Answer::yes(),
            vec![edit(None, "a.me", 80)],
        );
        assert_eq!(r.outcome, Outcome::Finished);
        assert!(has(
            &r.lines,
            "warn no PortalAlias table - change the site aliases in DNN (Settings → Site Settings → Site Aliases)."
        ));
    }

    #[test]
    fn asks_before_another_database_and_a_sign_in_elsewhere() {
        let r = edit_with(
            "Started",
            Some(sql_database("dnn9", false)),
            |_, _| {},
            Answer::no(),
            vec![edit(None, "a.me", 80)],
        );
        assert_eq!(
            r.asked,
            vec![
                "The web.config of 'shop' names the database [dnn9] on localhost - not the project's own, [shop].\n\nChange DNN's portal aliases in [dnn9]?"
            ]
        );
        assert!(has(
            &r.lines,
            "warn [dnn9] isn't the project's own database, and was left as it is - DNN's portal aliases stay as they are. Add the new host names as site aliases in DNN."
        ));
        assert!(!logged(&r).iter().any(|l| l.starts_with("aliases")));

        let r = edit_with(
            "Started",
            Some(sql_database("shop", true)),
            |_, d| d.may_sign_in = false,
            Answer::no(),
            vec![edit(None, "a.me", 80)],
        );
        assert_eq!(r.asked, vec!["Changing DNN's portal aliases on localhost?"]);
        assert!(has(
            &r.lines,
            "warn Not signed in to localhost with your Windows account - DNN's portal aliases stay as they are. Add the new host names as site aliases in DNN."
        ));

        let r = edit_with(
            "Started",
            Some(sql_database("other", true)),
            |_, d| d.may_sign_in = false,
            Answer::yes(),
            vec![edit(None, "a.me", 80)],
        );
        assert_eq!(r.asked.len(), 2);
        assert!(logged(&r).iter().any(|l| l.starts_with("aliases")));
    }

    #[test]
    fn a_local_db_file_site_is_stopped_meanwhile_and_not_recycled() {
        let file = DatabaseConnection { kind: DatabaseKind::LocalDbFile, ..sql_database("Database.mdf", true) };
        let r = edit_with("Started", Some(file.clone()), |_, _| {}, Answer::yes(), vec![edit(None, "a.me", 80)]);
        assert_eq!(
            logged(&r),
            vec!["bindings shop a.me:80", "stop shop", "aliases on Database.mdf  +a.me", "start shop"]
        );
        assert!(has(
            &r.lines,
            "Stopping the site for a moment - its database file can only be opened while the site doesn't use it…"
        ));
        let r = edit_with("Stopped", Some(file), |_, _| {}, Answer::yes(), vec![edit(None, "a.me", 80)]);
        assert_eq!(logged(&r), vec!["bindings shop a.me:80", "aliases on Database.mdf  +a.me"]);
    }

    #[test]
    fn splits_bindings_for_the_dialog() {
        let (http, others) = split_bindings(&[
            IisBinding::parse("http", "*:80:a.me", false),
            IisBinding::parse("https", "*:443:a.me", true),
            IisBinding::parse("net.tcp", "808:*", false),
        ]);
        assert_eq!(http, vec![("a.me".to_string(), 80)]);
        assert_eq!(others, vec!["https *:443:a.me", "net.tcp :808:*"]);
    }
}
