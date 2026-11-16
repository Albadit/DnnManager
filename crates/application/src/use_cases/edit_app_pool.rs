//! `EditAppPoolUseCase` (`UseCases/EditProjectUseCases.cs`): Edit app pool - the settings of the site's own app pool;
//! the site's folder lets a new identity in. Recycles the pool.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::iis::{IisManager, IisPoolSettings};
use crate::operation::{Operation, STEP_LIMIT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditAppPoolRequest {
    pub site_name: String,
    /// The folder the site serves - a new identity gets access to it.
    pub directory: PathBuf,
    pub settings: IisPoolSettings,
}

/// The pool's settings in a line, as the Output says them: ".NET CLR v4.0, Integrated, 64-bit, ApplicationPoolIdentity,
/// idle time-out 20 min, OnDemand."
pub fn describe(settings: &IisPoolSettings) -> String {
    let runtime = if settings.runtime.is_empty() {
        "No managed code".to_string()
    } else {
        format!(".NET CLR {}", settings.runtime)
    };
    let bits = if settings.enable_32bit { "32-bit" } else { "64-bit" };
    let idle = if settings.idle_timeout.is_zero() {
        "none".to_string()
    } else {
        format!("{} min", minutes(settings.idle_timeout))
    };
    format!(
        "{runtime}, {}, {bits}, {}, idle time-out {idle}, {}.",
        settings.pipeline, settings.identity, settings.start_mode
    )
}

/// Whole minutes, rounded half away from zero as .NET's "0" format rounds.
fn minutes(span: Duration) -> u64 {
    (span.as_secs_f64() / 60.0).round() as u64
}

/// `EditAppPoolUseCase.ExecuteAsync`.
pub fn edit_app_pool(iis: &Arc<dyn IisManager>, req: &EditAppPoolRequest, op: &mut Operation) -> Result<(), String> {
    let site_name = req.site_name.as_str();
    let Some(pool) = iis.site_details(site_name).and_then(|d| d.pool) else {
        return Err(format!("IIS site '{site_name}' has no app pool."));
    };
    let before = IisPoolSettings::from_pool(&pool);
    let settings = &req.settings;

    op.report.step(&format!("Changing the app pool '{}'", pool.name));
    {
        let (iis, name, before) = (iis.clone(), site_name.to_string(), before.clone());
        op.undo.add("Put the app pool's settings back", STEP_LIMIT, move |_| {
            iis.set_pool_settings(&name, &before).map_err(|e| e.0)
        });
    }
    iis.set_pool_settings(site_name, settings).map_err(|e| format!("Could not change the app pool: {e}"))?;
    op.report.success(&describe(settings));

    if !settings.identity.eq_ignore_ascii_case(&before.identity) && req.directory.is_dir() {
        let account = iis.app_pool_identity(site_name);
        let directory = req.directory.display();
        match iis.grant_permissions(&req.directory, std::slice::from_ref(&account)) {
            Ok(()) => op.report.success(&format!("{account} may use {directory}.")),
            Err(e) => op.report.warn(&format!("Could not give {account} access to {directory}: {e}")),
        }
        op.report.info(
            "With Windows authentication its database needs that account as a login too - Details → Database → Change connection grants it.",
        );
    }

    let _ = iis.recycle_app_pool(site_name);
    op.report.success("The app pool was recycled - the site starts with the new settings.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Outcome, run};
    use crate::use_cases::rename_project::tests::{Iis, Log, details};

    fn settings(identity: &str, idle_minutes: u64) -> IisPoolSettings {
        IisPoolSettings {
            runtime: String::new(),
            pipeline: "Classic".into(),
            enable_32bit: true,
            identity: identity.into(),
            idle_timeout: Duration::from_secs(idle_minutes * 60),
            start_mode: "AlwaysRunning".into(),
        }
    }

    fn edit(iis: Iis, directory: PathBuf, settings: IisPoolSettings) -> (Outcome, Vec<String>) {
        let iis: Arc<dyn IisManager> = Arc::new(iis);
        let req = EditAppPoolRequest { site_name: "shop".into(), directory, settings };
        let mut lines = Lines::default();
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            edit_app_pool(&iis, &req, op)
        });
        (outcome, lines.0)
    }

    #[test]
    fn changes_grants_a_new_identity_and_recycles() {
        let dir = tempfile::tempdir().unwrap();
        let log: Log = Arc::default();
        let (outcome, lines) = edit(
            Iis::new(&log, Some(details("shop", "Started", vec![]))),
            dir.path().into(),
            settings("NetworkService", 0),
        );
        assert_eq!(outcome, Outcome::Finished);
        let shown = dir.path().display();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "pool shop  NetworkService 0".to_string(),
                format!(r"grant NT AUTHORITY\NETWORK SERVICE for shop {shown}"),
                "recycle shop".into(),
            ]
        );
        assert_eq!(
            lines,
            vec![
                "## Changing the app pool 'shop'".to_string(),
                "ok No managed code, Classic, 32-bit, NetworkService, idle time-out none, AlwaysRunning.".into(),
                format!(r"ok NT AUTHORITY\NETWORK SERVICE for shop may use {shown}."),
                "With Windows authentication its database needs that account as a login too - Details → Database → Change connection grants it.".into(),
                "ok The app pool was recycled - the site starts with the new settings.".into(),
            ]
        );
    }

    #[test]
    fn the_same_identity_or_a_missing_folder_grants_nothing() {
        let log: Log = Arc::default();
        let dir = tempfile::tempdir().unwrap();
        let (_, lines) = edit(
            Iis::new(&log, Some(details("shop", "Started", vec![]))),
            dir.path().into(),
            settings("applicationpoolidentity", 20),
        );
        assert!(
            lines.contains(
                &"ok No managed code, Classic, 32-bit, applicationpoolidentity, idle time-out 20 min, AlwaysRunning."
                    .to_string()
            )
        );
        let missing = dir.path().join("gone");
        edit(Iis::new(&log, Some(details("shop", "Started", vec![]))), missing, settings("LocalSystem", 20));
        assert!(!log.lock().unwrap().iter().any(|l| l.starts_with("grant")));
    }

    #[test]
    fn a_refused_change_fails_and_puts_the_settings_back() {
        let log: Log = Arc::default();
        let mut iis = Iis::new(&log, Some(details("shop", "Started", vec![])));
        iis.fail = vec!["pool"];
        let (outcome, lines) = edit(iis, PathBuf::from(r"C:\nowhere"), settings("LocalSystem", 5));
        assert_eq!(outcome, Outcome::Failed("Could not change the app pool: pool refused".into()));
        assert_eq!(
            *log.lock().unwrap(),
            vec!["pool shop  LocalSystem 300", "pool shop v4.0 ApplicationPoolIdentity 1200"]
        );
        assert!(lines.contains(&"! Put the app pool's settings back: pool refused".to_string()));
    }

    #[test]
    fn a_site_without_a_pool_is_refused() {
        let log: Log = Arc::default();
        let mut site = details("shop", "Started", vec![]);
        site.pool = None;
        let (outcome, _) = edit(Iis::new(&log, Some(site)), PathBuf::new(), settings("LocalSystem", 5));
        assert_eq!(outcome, Outcome::Failed("IIS site 'shop' has no app pool.".into()));
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn describes_runtime_bits_and_rounded_minutes() {
        let mut s = settings("LocalService", 0);
        s.runtime = "v4.0".into();
        s.enable_32bit = false;
        s.idle_timeout = Duration::from_secs(90);
        assert_eq!(describe(&s), ".NET CLR v4.0, Classic, 64-bit, LocalService, idle time-out 2 min, AlwaysRunning.");
    }
}
