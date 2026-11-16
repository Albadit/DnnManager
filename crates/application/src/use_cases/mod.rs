//! Use cases: what changes the system. Each runs inside an [`Operation`] - it reports its stages, notes how to undo
//! each step and asks through its prompt - and never touches the UI.

use crate::iis::{IisManager, IisServerState};
use crate::operation::Operation;
pub mod clone_project;
pub mod host_project;
pub mod hosting;
pub mod sql_container;
pub mod upgrade_dnn;
pub mod edit_app_pool;
pub mod edit_bindings;
pub mod rename_project;
pub mod backups;
pub mod export_for_deployment;
pub mod export_project;
pub mod restore_backup;
pub mod clear_site_cache;
pub mod remove_project;
pub mod setup_project;
pub mod change_database_connection;
pub mod change_host_password;
pub mod database_connection;

/// What Start, Stop and Restart do to a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteAction {
    Start,
    Stop,
    Restart,
}

impl SiteAction {
    /// The operation's title: `Start 'dnn10'`, `Stop 3 sites`.
    pub fn title(self, names: &[String]) -> String {
        let action = match self {
            SiteAction::Start => "Start",
            SiteAction::Stop => "Stop",
            SiteAction::Restart => "Restart",
        };
        match names {
            [one] => format!("{action} '{one}'"),
            _ => format!("{action} {} sites", names.len()),
        }
    }

    /// What the row says while it runs.
    pub fn pending(self) -> &'static str {
        match self {
            SiteAction::Start => "Starting…",
            SiteAction::Stop => "Stopping…",
            SiteAction::Restart => "Restarting…",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            SiteAction::Start => "start",
            SiteAction::Stop => "stop",
            SiteAction::Restart => "restart",
        }
    }
}

/// `ControlSitesUseCase`: acts on every site, going on past one that fails; fails when any did.
pub fn control_sites(
    iis: &dyn IisManager,
    names: &[String],
    action: SiteAction,
    op: &mut Operation,
) -> Result<(), String> {
    match iis.server_state() {
        IisServerState::NotInstalled => return Err("IIS isn't installed - set it up in Settings → IIS.".into()),
        IisServerState::Stopped => return Err("IIS is stopped - start it first (bottom-left of the window).".into()),
        _ => {}
    }
    let single = names.len() == 1;
    let mut failed: Vec<(&str, String)> = Vec::new();
    for name in names {
        op.cancel.check()?;
        op.report.step(name);
        let result = match action {
            SiteAction::Start => iis.start_site(name),
            SiteAction::Stop => iis.stop_site(name),
            SiteAction::Restart => iis.restart_site(name),
        };
        match result {
            Ok(()) => op.report.info(&match action {
                SiteAction::Start => format!("Started '{name}'."),
                SiteAction::Stop => format!("Stopped '{name}'."),
                SiteAction::Restart => format!("Restarted '{name}' (its app pool recycled)."),
            }),
            Err(e) => {
                if !single {
                    op.report.fail(&format!("Could not {} '{name}': {e}", action.verb()));
                }
                failed.push((name, e.0));
            }
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    if single {
        return Err(failed.remove(0).1);
    }
    let done = match action {
        SiteAction::Start => "started",
        SiteAction::Stop => "stopped",
        SiteAction::Restart => "restarted",
    };
    let list = failed.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
    Err(format!("{} of {} could not be {done}: {list}.", failed.len(), names.len()))
}

/// `IisServerUseCase`: Start, Stop or Restart IIS with `iisreset`; Stop and Restart ask first - every website on the
/// PC goes down.
pub fn iis_server(iis: &dyn IisManager, action: crate::iis::IisServerAction, op: &mut Operation) -> Result<(), String> {
    use crate::iis::IisServerAction;
    if !iis.is_available() {
        return Err("IIS isn't installed or its configuration can't be read - check Settings → IIS.".into());
    }
    let question = match action {
        IisServerAction::Stop => {
            Some(("Stop IIS? Every website on this machine stops until IIS is started again.", "Stop IIS"))
        }
        IisServerAction::Restart => {
            Some(("Restart IIS? Every website on this machine stops for a few seconds.", "Restart IIS"))
        }
        IisServerAction::Start => None,
    };
    if let Some((question, yes)) = question
        && !op.prompt.confirm(question, yes, "Cancel", false)
    {
        return Err(crate::operation::CANCELLED.into());
    }
    let (step, done) = match action {
        IisServerAction::Start => ("Starting IIS (iisreset /start)", "IIS started."),
        IisServerAction::Stop => ("Stopping IIS (iisreset /stop)", "IIS stopped."),
        IisServerAction::Restart => ("Restarting IIS (iisreset)", "IIS restarted."),
    };
    op.report.step(step);
    iis.control_server(action).map_err(|e| e.0)?;
    op.report.success(done);
    Ok(())
}

/// `IisSiteProvisioner`: creates a project's IIS website - shared by setup, clone and import so all three bind,
/// permission and start the site the same way.
pub mod provisioner {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use crate::iis::IisManager;
    use crate::operation::{Operation, STEP_LIMIT};

    /// The folder of the IIS site named `site_name` when that site serves another folder than `directory` - a site
    /// this project must not take over. None when there is no such site, or it serves that folder.
    pub fn site_serving_another_folder(iis: &dyn IisManager, site_name: &str, directory: &Path) -> Option<PathBuf> {
        let sites = iis.site_runtimes().ok()?;
        let (_, site) = sites.iter().find(|(name, _)| name.eq_ignore_ascii_case(site_name))?;
        if same_folder(&site.physical_path, directory) { None } else { Some(site.physical_path.clone()) }
    }

    /// Why `site_name` can't be this project's site, for a message; None when it can.
    pub fn site_name_taken(iis: &dyn IisManager, site_name: &str, directory: &Path) -> Option<String> {
        site_serving_another_folder(iis, site_name, directory).map(|other| {
            let other = other.display().to_string();
            let other = if other.is_empty() { "another folder".to_string() } else { other };
            format!("IIS already has a site named '{site_name}', serving {other} - choose another name.")
        })
    }

    fn same_folder(a: &Path, b: &Path) -> bool {
        fn normal(path: &Path) -> String {
            let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            full.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
        }
        !a.as_os_str().is_empty() && !b.as_os_str().is_empty() && normal(a) == normal(b)
    }

    /// DNN's portal alias for a host name and port: the port only when it isn't 80 (`DnnSiteAddress.AliasFor`).
    pub fn alias_for(host: &str, port: u16) -> String {
        if port == 80 { host.to_string() } else { format!("{host}:{port}") }
    }

    /// Creates (or recreates) the site `name` and its app pool for the folder `directory`, bound to `host` on `port`,
    /// grants the IIS identities access to the folder and starts the site. Failures are reported and return false - a
    /// missing website is never fatal to the calling flow.
    pub fn try_create_site(
        iis: &Arc<dyn IisManager>,
        name: &str,
        directory: &Path,
        host: &str,
        port: u16,
        op: &mut Operation,
    ) -> bool {
        // Never another site's: one of this name serving another folder stays as it is.
        if let Some(taken) = site_name_taken(iis.as_ref(), name, directory) {
            op.report.fail(&taken);
            return false;
        }

        // A cancel takes a new site away again; one that was there is replaced, and can't be brought back as it was.
        if iis.site_states().contains_key(name) {
            op.undo.cannot_undo(format!("the IIS site '{name}' that was there before was replaced by a new one."));
        } else {
            let (undo_iis, undo_name) = (Arc::clone(iis), name.to_string());
            op.undo.add(format!("Remove the IIS site and app pool '{name}'"), STEP_LIMIT, move |_| {
                undo_iis.remove_site(&undo_name).map_err(|e| e.0)?;
                // What is left of the pool's profile is only a folder in C:\Users - not worth failing the undo for.
                let _ = undo_iis.remove_app_pool_profile(&undo_name);
                Ok(())
            });
        }

        // create_site tears down any existing site/pool of this name itself (waiting for its worker to exit), so
        // callers must not remove it first - that just repeats the whole teardown.
        if let Err(e) = iis.create_site(name, directory, host, port) {
            op.report.fail(&format!("IIS site creation failed: {e}. Continuing without a website."));
            return false;
        }

        let identities = ["IIS_IUSRS".to_string(), "IUSR".to_string(), format!(r"IIS APPPOOL\{name}")];
        if let Err(e) = iis.grant_permissions(directory, &identities) {
            op.report.fail(&format!(
                "Could not grant IIS access to {}: {e}. The site may return 401/500 errors until the folder permissions are fixed.",
                directory.display()
            ));
        }

        // As the C#: a site that doesn't start is still made - its Start button says why.
        let _ = iis.start_site(name);
        op.report.success(&format!("IIS site '{name}' bound to http://{}", alias_for(host, port)));
        true
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::iis::{IisError, IisServerAction, IisSiteRuntime};
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Operation};

    fn with_op<T>(f: impl FnOnce(&mut Operation) -> T) -> (T, Vec<String>) {
        let mut lines = Lines::default();
        let answer = Answer::yes();
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        let result = f(&mut op);
        drop(op);
        (result, lines.0)
    }
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A stand-in IIS: records what it is asked, fails for the names in `failing`.
    pub struct FakeIis {
        pub state: IisServerState,
        pub failing: Vec<&'static str>,
        pub calls: Mutex<Vec<String>>,
        pub sites: Mutex<BTreeMap<String, IisSiteRuntime>>,
    }

    impl FakeIis {
        pub fn running() -> Self {
            Self {
                state: IisServerState::Running,
                failing: vec![],
                calls: Mutex::new(vec![]),
                sites: Mutex::new(BTreeMap::new()),
            }
        }

        pub fn with_site(self, name: &str, folder: &str) -> Self {
            let site = IisSiteRuntime {
                id: 1,
                state: "Started".into(),
                app_pool: name.into(),
                app_pool_state: None,
                worker_process_ids: vec![],
                bindings: vec![],
                physical_path: folder.into(),
            };
            self.sites.lock().unwrap().insert(name.into(), site);
            self
        }

        fn call(&self, what: &str, site: &str) -> Result<(), IisError> {
            self.calls.lock().unwrap().push(format!("{what} {site}"));
            if self.failing.contains(&site) { Err(IisError("access denied".into())) } else { Ok(()) }
        }
    }

    impl IisManager for FakeIis {
        fn is_available(&self) -> bool {
            true
        }
        fn server_state(&self) -> IisServerState {
            self.state
        }
        fn site_runtimes(&self) -> Result<BTreeMap<String, IisSiteRuntime>, IisError> {
            Ok(self.sites.lock().unwrap().clone())
        }
        fn start_site(&self, site: &str) -> Result<(), IisError> {
            self.call("start", site)
        }
        fn stop_site(&self, site: &str) -> Result<(), IisError> {
            self.call("stop", site)
        }
        fn restart_site(&self, site: &str) -> Result<(), IisError> {
            self.call("restart", site)
        }
        fn recycle_app_pool(&self, site: &str) -> Result<(), IisError> {
            self.call("recycle", site)
        }
        fn control_server(&self, _: IisServerAction) -> Result<(), IisError> {
            Ok(())
        }
        fn create_site(&self, site: &str, _: &std::path::Path, host: &str, port: u16) -> Result<(), IisError> {
            self.call(&format!("create {host}:{port}"), site)
        }
        fn remove_site(&self, site: &str) -> Result<(), IisError> {
            self.call("remove", site)
        }
        fn log_directory(&self, _: &str) -> Option<std::path::PathBuf> {
            None
        }
        fn app_pool_identity(&self, site: &str) -> String {
            format!(r"IIS APPPOOL\{site}")
        }
        fn enable_user_profile(&self, site: &str) -> Result<(), IisError> {
            self.call("profile", site)
        }
        fn site_traffic(&self) -> std::collections::HashMap<String, crate::project::SiteTraffic> {
            Default::default()
        }
        fn requests_served(&self) -> std::collections::HashMap<String, u64> {
            Default::default()
        }
        fn grant_permissions(&self, path: &std::path::Path, identities: &[String]) -> Result<(), IisError> {
            self.call(&format!("grant {}", identities.join("+")), &path.display().to_string())
        }
        fn remove_app_pool_profile(&self, pool: &str) -> Result<(), IisError> {
            self.call("remove profile", pool)
        }
    }

    #[test]
    fn provisioner_creates_grants_and_starts_and_notes_the_undo() {
        use std::sync::Arc;
        let fake = Arc::new(FakeIis::running());
        let iis: Arc<dyn IisManager> = fake.clone();
        let (made, lines) = with_op(|op| {
            let made = provisioner::try_create_site(&iis, "shop", "C:\\DNN\\shop".as_ref(), "shop.dnndev.me", 8080, op);
            assert_eq!(op.undo.pending(), vec!["Remove the IIS site and app pool 'shop'"]);
            made
        });
        assert!(made);
        assert_eq!(
            *fake.calls.lock().unwrap(),
            vec!["create shop.dnndev.me:8080 shop", r"grant IIS_IUSRS+IUSR+IIS APPPOOL\shop C:\DNN\shop", "start shop"]
        );
        assert_eq!(lines, vec!["ok IIS site 'shop' bound to http://shop.dnndev.me:8080"]);
    }

    #[test]
    fn provisioner_never_takes_another_folders_site() {
        use std::sync::Arc;
        let fake = Arc::new(FakeIis::running().with_site("Shop", r"D:\other"));
        let iis: Arc<dyn IisManager> = fake.clone();
        let (made, lines) =
            with_op(|op| provisioner::try_create_site(&iis, "shop", r"C:\DNN\shop".as_ref(), "shop.me", 80, op));
        assert!(!made);
        assert_eq!(lines, vec![r"! IIS already has a site named 'shop', serving D:\other - choose another name."]);
        assert!(fake.calls.lock().unwrap().is_empty());
        // The same folder is this project's own site: replaced, which can't be undone.
        let fake = Arc::new(FakeIis::running().with_site("shop", r"C:\DNN\shop\"));
        let iis: Arc<dyn IisManager> = fake.clone();
        let (_, _) = with_op(|op| {
            assert!(provisioner::try_create_site(&iis, "shop", r"C:\DNN\shop".as_ref(), "shop.me", 80, op));
            assert_eq!(
                op.undo.pending(),
                vec!["Can't be put back: the IIS site 'shop' that was there before was replaced by a new one."]
            );
        });
    }

    #[test]
    fn provisioner_reports_a_failed_create() {
        use std::sync::Arc;
        let iis: Arc<dyn IisManager> = Arc::new(FakeIis { failing: vec!["shop"], ..FakeIis::running() });
        let (made, lines) =
            with_op(|op| provisioner::try_create_site(&iis, "shop", r"C:\DNN\shop".as_ref(), "shop.me", 80, op));
        assert!(!made);
        assert_eq!(lines, vec!["! IIS site creation failed: access denied. Continuing without a website."]);
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn titles() {
        assert_eq!(SiteAction::Start.title(&names(&["dnn10"])), "Start 'dnn10'");
        assert_eq!(SiteAction::Stop.title(&names(&["a", "b", "c"])), "Stop 3 sites");
    }

    #[test]
    fn starts_each_site_and_reports_it() {
        let iis = FakeIis::running();
        let (result, lines) = with_op(|op| control_sites(&iis, &names(&["a", "b"]), SiteAction::Start, op));
        result.unwrap();
        assert_eq!(*iis.calls.lock().unwrap(), vec!["start a", "start b"]);
        assert_eq!(lines, vec!["## a", "Started 'a'.", "## b", "Started 'b'."]);
    }

    #[test]
    fn goes_on_past_a_failure_and_names_the_failed() {
        let iis = FakeIis { failing: vec!["b"], ..FakeIis::running() };
        let (result, lines) = with_op(|op| control_sites(&iis, &names(&["a", "b", "c"]), SiteAction::Stop, op));
        assert_eq!(result.unwrap_err(), "1 of 3 could not be stopped: b.");
        assert!(lines.contains(&"! Could not stop 'b': access denied".to_string()));
        assert_eq!(iis.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_single_failure_is_its_own_message() {
        let iis = FakeIis { failing: vec!["a"], ..FakeIis::running() };
        let (result, _) = with_op(|op| control_sites(&iis, &names(&["a"]), SiteAction::Restart, op));
        assert_eq!(result.unwrap_err(), "access denied");
    }

    #[test]
    fn refuses_when_iis_is_stopped_or_missing() {
        let stopped = FakeIis { state: IisServerState::Stopped, ..FakeIis::running() };
        let (result, _) = with_op(|op| control_sites(&stopped, &names(&["a"]), SiteAction::Start, op));
        assert!(result.unwrap_err().starts_with("IIS is stopped"));
        assert!(stopped.calls.lock().unwrap().is_empty());
        let missing = FakeIis { state: IisServerState::NotInstalled, ..FakeIis::running() };
        assert!(with_op(|op| control_sites(&missing, &names(&["a"]), SiteAction::Start, op)).0.is_err());
    }

    #[test]
    fn iis_stop_asks_first_and_a_no_does_nothing() {
        use crate::iis::IisServerAction;
        let iis = FakeIis::running();
        let mut lines = Lines::default();
        let no = Answer::no();
        let mut op = Operation::new(&mut lines, &no, CancelToken::new());
        assert_eq!(iis_server(&iis, IisServerAction::Stop, &mut op).unwrap_err(), crate::operation::CANCELLED);
        drop(op);
        assert_eq!(
            no.1.lock().unwrap()[0],
            "Stop IIS? Every website on this machine stops until IIS is started again."
        );
        assert!(lines.0.is_empty());
        let (result, lines) = with_op(|op| iis_server(&iis, IisServerAction::Start, op));
        result.unwrap();
        assert_eq!(lines, vec!["## Starting IIS (iisreset /start)", "ok IIS started."]);
    }
}
