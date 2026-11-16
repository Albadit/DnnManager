//! Change host password (`UseCases/ChangeHostPasswordUseCase.cs`): sets a new password for one of a DNN site's host
//! accounts, the way DNN's membership provider stores passwords, then restarts the site (DNN caches its users). A
//! LocalDB file database can only be opened while the site doesn't use it, so that site is stopped meanwhile - and
//! started again only if it ran.

use std::fmt;
use std::path::{Path, PathBuf};

use super::database_connection::{DatabaseConnection, DatabaseKind};
use crate::iis::{IisManager, states};
use crate::operation::{CancelToken, Operation};

/// Which host account of which site gets which password.
#[derive(Clone, PartialEq, Eq)]
pub struct HostPasswordRequest {
    pub site_name: String,
    pub directory: PathBuf,
    pub user_name: String,
    pub new_password: String,
}

impl fmt::Debug for HostPasswordRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostPasswordRequest")
            .field("site_name", &self.site_name)
            .field("directory", &self.directory)
            .field("user_name", &self.user_name)
            .finish_non_exhaustive()
    }
}

/// DNN and the site's database, as Change host password needs them (`DnnAccountRules`, `LocalSqlContainer`,
/// `IDnnInstaller`).
pub trait HostPasswordChanger: Send + Sync {
    /// `DnnAccountRules.PasswordProblem`: why DNN wouldn't take `password`; None when it would.
    fn password_problem(&self, password: &str) -> Option<String>;

    /// The database the site's web.config names (`LocalSqlContainer.DatabaseOf`); None when it has none of its own yet.
    fn database_of(&self, site_name: &str, directory: &Path) -> Option<DatabaseConnection>;

    /// Sets the password as DNN's membership provider stores it (`IDnnInstaller.ChangeHostPasswordAsync`). The error
    /// is the message to show - never with the password in it.
    fn change_host_password(
        &self,
        directory: &Path,
        database: &DatabaseConnection,
        user_name: &str,
        new_password: &str,
        cancel: &CancelToken,
    ) -> Result<(), String>;
}

pub fn change_host_password(
    iis: &dyn IisManager,
    dnn: &dyn HostPasswordChanger,
    request: &HostPasswordRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let (site, user) = (request.site_name.as_str(), request.user_name.as_str());
    if let Some(problem) = dnn.password_problem(&request.new_password) {
        return Err(problem);
    }
    let Some(database) = dnn.database_of(site, &request.directory) else {
        return Err("The site's web.config doesn't name its database yet - install DNN first.".into());
    };

    op.report.step(&format!("Changing the password of host '{user}'"));
    let local_file = database.kind == DatabaseKind::LocalDbFile;
    let was_running = iis
        .site_states()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(site))
        .is_some_and(|(_, state)| states::is_started(state));
    if local_file && was_running {
        op.report.info(
            "Stopping the site for a moment - its database file can only be opened while the site doesn't use it…",
        );
        let _ = iis.stop_site(site);
    }

    let changed = dnn.change_host_password(&request.directory, &database, user, &request.new_password, &op.cancel);
    if local_file && was_running {
        let _ = iis.start_site(site);
    }
    changed?;

    // DNN keeps its users in memory: it reads the account again after a restart.
    if !local_file {
        let _ = iis.recycle_app_pool(site);
    }
    op.report.success(&format!("Host '{user}' signs in with the new password - it is kept nowhere else."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::use_cases::database_connection::SqlAuthentication;
    use crate::use_cases::tests::FakeIis;

    struct FakeDnn {
        database: Option<DatabaseConnection>,
        fails: bool,
        calls: Mutex<Vec<String>>,
    }

    impl FakeDnn {
        fn on(kind: DatabaseKind) -> Self {
            let database = DatabaseConnection::new(kind, "db", "shop", SqlAuthentication::Windows);
            Self { database: Some(database), fails: false, calls: Mutex::new(vec![]) }
        }
    }

    impl HostPasswordChanger for FakeDnn {
        fn password_problem(&self, password: &str) -> Option<String> {
            (password.len() < 7).then(|| "The password needs at least 7 characters.".into())
        }
        fn database_of(&self, _: &str, _: &Path) -> Option<DatabaseConnection> {
            self.database.clone()
        }
        fn change_host_password(
            &self,
            _: &Path,
            _: &DatabaseConnection,
            user: &str,
            password: &str,
            _: &CancelToken,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("set {user} {password}"));
            if self.fails { Err(format!("'{user}' isn't a host account of this site.")) } else { Ok(()) }
        }
    }

    fn run(iis: &FakeIis, dnn: &FakeDnn, password: &str) -> (Result<(), String>, Vec<String>) {
        let mut lines = Lines::default();
        let answer = Answer::yes();
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        let request = HostPasswordRequest {
            site_name: "shop".into(),
            directory: PathBuf::from(r"C:\DNN\shop"),
            user_name: "host".into(),
            new_password: password.into(),
        };
        let result = change_host_password(iis, dnn, &request, &mut op);
        drop(op);
        (result, lines.0)
    }

    #[test]
    fn sets_the_password_and_recycles_the_site() {
        let (iis, dnn) = (FakeIis::running().with_site("shop", r"C:\DNN\shop"), FakeDnn::on(DatabaseKind::SqlServer));
        let (result, lines) = run(&iis, &dnn, "Secret-123");
        result.unwrap();
        assert_eq!(*dnn.calls.lock().unwrap(), vec!["set host Secret-123"]);
        assert_eq!(*iis.calls.lock().unwrap(), vec!["recycle shop"]);
        assert_eq!(
            lines,
            vec![
                "## Changing the password of host 'host'",
                "ok Host 'host' signs in with the new password - it is kept nowhere else."
            ]
        );
    }

    #[test]
    fn a_local_db_file_stops_a_running_site_meanwhile_even_when_it_fails() {
        let iis = FakeIis::running().with_site("Shop", r"C:\DNN\shop");
        let dnn = FakeDnn { fails: true, ..FakeDnn::on(DatabaseKind::LocalDbFile) };
        let (result, lines) = run(&iis, &dnn, "Secret-123");
        assert_eq!(result.unwrap_err(), "'host' isn't a host account of this site.");
        assert_eq!(*iis.calls.lock().unwrap(), vec!["stop shop", "start shop"]);
        assert!(lines[1].starts_with("Stopping the site for a moment"));

        // A stopped site stays stopped, and a LocalDB file isn't recycled.
        let iis = FakeIis::running();
        let dnn = FakeDnn::on(DatabaseKind::LocalDbFile);
        run(&iis, &dnn, "Secret-123").0.unwrap();
        assert!(iis.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn refuses_a_weak_password_and_a_site_without_a_database() {
        let (iis, dnn) = (FakeIis::running(), FakeDnn::on(DatabaseKind::SqlServer));
        assert_eq!(run(&iis, &dnn, "short").0.unwrap_err(), "The password needs at least 7 characters.");
        let dnn = FakeDnn { database: None, ..FakeDnn::on(DatabaseKind::SqlServer) };
        assert_eq!(
            run(&iis, &dnn, "Secret-123").0.unwrap_err(),
            "The site's web.config doesn't name its database yet - install DNN first."
        );
        assert!(dnn.calls.lock().unwrap().is_empty() && iis.calls.lock().unwrap().is_empty());
    }
}
