//! Change connection (`UseCases/EditProjectUseCases.cs`, `ChangeDatabaseConnectionUseCase`): points web.config's
//! SiteSqlServer at another database, server or sign-in. Nothing in the databases is moved - the new one should
//! already hold the site. With Windows authentication the site's app pool gets a login, with a LocalDB file its user
//! profile is loaded. Restarts the site.

use std::path::{Path, PathBuf};

use super::database_connection::{CheckOutcome, DatabaseCheckReport, DatabaseConnection, DatabaseKind};
use crate::iis::IisManager;
use crate::operation::Operation;

/// What Change connection is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeConnectionRequest {
    pub site_name: String,
    /// The site's folder - where its web.config is.
    pub directory: PathBuf,
    pub connection: DatabaseConnection,
}

/// The databases and web.config, as Change connection needs them (`IDatabaseProvisioner`, `IWebConfigService`).
pub trait ConnectionChanger: Send + Sync {
    /// Test connection's checks (not for a new install); `site_login`: the Windows login the site will sign in as,
    /// checked too. Err only when cancelled.
    fn check(
        &self,
        connection: &DatabaseConnection,
        site_login: Option<&str>,
        cancel: &crate::operation::CancelToken,
    ) -> Result<DatabaseCheckReport, String>;

    /// Writes `connection` as web.config's SiteSqlServer (and appSettings' SiteSqlServer when it has one).
    fn write_connection(&self, web_config: &Path, connection: &DatabaseConnection) -> Result<(), String>;

    /// Lets the Windows login `account` own the database. The error is the message to show.
    fn grant_site_access(
        &self,
        connection: &DatabaseConnection,
        account: &str,
        cancel: &crate::operation::CancelToken,
    ) -> Result<(), String>;
}

pub fn change_database_connection(
    iis: &dyn IisManager,
    databases: &dyn ConnectionChanger,
    request: &ChangeConnectionRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let (site, connection) = (request.site_name.as_str(), &request.connection);
    let web_config = request.directory.join("web.config");
    if !web_config.is_file() {
        return Err(format!("The site has no web.config: {}", web_config.display()));
    }

    op.report.step("Checking the database");
    let account = iis.app_pool_identity(site);
    let site_login = (connection.uses_windows_authentication() && connection.kind != DatabaseKind::LocalDbFile)
        .then_some(account.as_str());
    let report = databases.check(connection, site_login, &op.cancel)?;
    for check in &report.checks {
        let line = format!("{}: {}", check.name, check.detail);
        match check.outcome {
            CheckOutcome::Passed => op.report.success(&line),
            CheckOutcome::Warning => op.report.warn(&line),
            CheckOutcome::Failed => op.report.fail(&line),
        }
    }
    let passed = report.passed();
    if !passed {
        op.report.warn("Saved anyway - the site shows an error until the database answers.");
    }
    op.cancel.check()?;

    op.report.step("Pointing web.config at it");
    op.undo.restore_file_on_undo(&web_config);
    databases.write_connection(&web_config, connection).map_err(|e| format!("Could not write web.config: {e}"))?;
    op.report.success(&format!("SiteSqlServer: {}", connection.describe()));

    if connection.kind == DatabaseKind::LocalDbFile {
        if let Err(e) = iis.enable_user_profile(site) {
            op.report.warn(&format!("Could not load the app pool's user profile, which LocalDB needs: {e}"));
        }
    } else if connection.uses_windows_authentication() && passed {
        match databases.grant_site_access(connection, &account, &op.cancel) {
            Ok(()) => op.report.success(&format!("{account} may use [{}].", connection.database)),
            Err(e) => op.report.warn(&e),
        }
    }

    // As the C#: a recycle that fails leaves the old worker running until it recycles by itself.
    let _ = iis.recycle_app_pool(site);
    op.report.success("The site was restarted with the new connection.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Undo};
    use crate::use_cases::database_connection::{DatabaseCheck, SqlAuthentication};
    use crate::use_cases::tests::FakeIis;

    /// A stand-in for SQL Server and web.config: answers checks with `outcome`, records what it is asked.
    struct FakeDatabases {
        outcome: CheckOutcome,
        write_fails: bool,
        grant_fails: bool,
        calls: Mutex<Vec<String>>,
    }

    impl FakeDatabases {
        fn new(outcome: CheckOutcome) -> Self {
            Self { outcome, write_fails: false, grant_fails: false, calls: Mutex::new(vec![]) }
        }
    }

    impl ConnectionChanger for FakeDatabases {
        fn check(
            &self,
            c: &DatabaseConnection,
            site_login: Option<&str>,
            _: &CancelToken,
        ) -> Result<DatabaseCheckReport, String> {
            self.calls.lock().unwrap().push(format!("check {} as {site_login:?}", c.database));
            let checks = vec![
                DatabaseCheck { name: "Server".into(), outcome: CheckOutcome::Passed, detail: "answers.".into() },
                DatabaseCheck { name: "Database".into(), outcome: self.outcome, detail: "found.".into() },
            ];
            Ok(DatabaseCheckReport { checks })
        }
        fn write_connection(&self, web_config: &Path, c: &DatabaseConnection) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("write {}", c.database));
            if self.write_fails {
                return Err("access denied".into());
            }
            std::fs::write(web_config, "after").map_err(|e| e.to_string())
        }
        fn grant_site_access(&self, c: &DatabaseConnection, account: &str, _: &CancelToken) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("grant {account} {}", c.database));
            if self.grant_fails {
                Err(format!("Could not give {account} access to [{}]: no", c.database))
            } else {
                Ok(())
            }
        }
    }

    fn site() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("web.config"), "before").unwrap();
        dir
    }

    fn run(
        dir: &Path,
        connection: DatabaseConnection,
        databases: &FakeDatabases,
        iis: &FakeIis,
    ) -> (Result<(), String>, Vec<String>, Undo) {
        let mut lines = Lines::default();
        let answer = Answer::yes();
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        let request = ChangeConnectionRequest { site_name: "shop".into(), directory: dir.to_path_buf(), connection };
        let result = change_database_connection(iis, databases, &request, &mut op);
        let undo = std::mem::take(&mut op.undo);
        drop(op);
        (result, lines.0, undo)
    }

    fn windows(database: &str) -> DatabaseConnection {
        DatabaseConnection::new(DatabaseKind::SqlServer, r".\SQLEXPRESS", database, SqlAuthentication::Windows)
    }

    #[test]
    fn checks_writes_grants_and_restarts() {
        let dir = site();
        let (iis, databases) = (FakeIis::running(), FakeDatabases::new(CheckOutcome::Passed));
        let (result, lines, mut undo) = run(dir.path(), windows("shop2"), &databases, &iis);
        result.unwrap();
        assert_eq!(
            *databases.calls.lock().unwrap(),
            vec![r#"check shop2 as Some("IIS APPPOOL\\shop")"#, "write shop2", r"grant IIS APPPOOL\shop shop2"]
        );
        assert_eq!(
            lines,
            vec![
                "## Checking the database",
                "ok Server: answers.",
                "ok Database: found.",
                "## Pointing web.config at it",
                r"ok SiteSqlServer: [shop2] on .\SQLEXPRESS (Windows authentication)",
                r"ok IIS APPPOOL\shop may use [shop2].",
                "ok The site was restarted with the new connection."
            ]
        );
        assert_eq!(*iis.calls.lock().unwrap(), vec!["recycle shop"]);
        // A failure later on puts web.config back as it was.
        assert!(undo.run(&mut Lines::default(), &CancelToken::new()));
        assert_eq!(std::fs::read_to_string(dir.path().join("web.config")).unwrap(), "before");
    }

    #[test]
    fn a_failed_test_is_saved_anyway_without_a_login() {
        let dir = site();
        let (iis, databases) = (FakeIis::running(), FakeDatabases::new(CheckOutcome::Failed));
        let (result, lines, _) = run(dir.path(), windows("gone"), &databases, &iis);
        result.unwrap();
        assert!(lines.contains(&"! Database: found.".to_string()));
        assert!(lines.contains(&"warn Saved anyway - the site shows an error until the database answers.".to_string()));
        assert!(!databases.calls.lock().unwrap().iter().any(|c| c.starts_with("grant")));
    }

    #[test]
    fn a_sql_login_gets_no_site_login_and_a_refused_grant_is_a_warning() {
        let dir = site();
        let (iis, databases) = (FakeIis::running(), FakeDatabases::new(CheckOutcome::Passed));
        let sql =
            DatabaseConnection::new(DatabaseKind::SqlServer, "db", "shop", SqlAuthentication::Sql).with_login("u", "p");
        run(dir.path(), sql, &databases, &iis).0.unwrap();
        assert_eq!(databases.calls.lock().unwrap()[0], "check shop as None");
        assert_eq!(databases.calls.lock().unwrap().len(), 2);

        let databases = FakeDatabases { grant_fails: true, ..FakeDatabases::new(CheckOutcome::Warning) };
        let (result, lines, _) = run(dir.path(), windows("shop"), &databases, &iis);
        result.unwrap();
        assert!(lines.contains(&r"warn Could not give IIS APPPOOL\shop access to [shop]: no".to_string()));
    }

    #[test]
    fn a_local_db_file_loads_the_profile_and_signs_in_as_nobody_else() {
        let dir = site();
        let (iis, databases) = (FakeIis::running(), FakeDatabases::new(CheckOutcome::Passed));
        let file = DatabaseConnection::new(
            DatabaseKind::LocalDbFile,
            DatabaseConnection::LOCAL_DB_SERVER,
            "Database.mdf",
            SqlAuthentication::Windows,
        );
        run(dir.path(), file, &databases, &iis).0.unwrap();
        assert_eq!(databases.calls.lock().unwrap()[0], "check Database.mdf as None");
        assert_eq!(*iis.calls.lock().unwrap(), vec!["profile shop", "recycle shop"]);
    }

    #[test]
    fn no_web_config_or_a_failed_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (iis, databases) = (FakeIis::running(), FakeDatabases::new(CheckOutcome::Passed));
        let (result, _, _) = run(dir.path(), windows("shop"), &databases, &iis);
        assert!(result.unwrap_err().starts_with("The site has no web.config: "));
        assert!(databases.calls.lock().unwrap().is_empty());

        let dir = site();
        let databases = FakeDatabases { write_fails: true, ..FakeDatabases::new(CheckOutcome::Passed) };
        let (result, _, _) = run(dir.path(), windows("shop"), &databases, &iis);
        assert_eq!(result.unwrap_err(), "Could not write web.config: access denied");
        assert!(iis.calls.lock().unwrap().is_empty());
    }
}
