//! Signing in to a site's database as its web.config says (`Sql/SqlConnectionTester.cs`), and what the Projects table
//! shows of each site's database (`Sql/SiteDatabaseChecks.cs`, the SQL part of `Monitoring/ServerStateMonitor.cs`).

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use dnnm_application::operation::CancelToken;

use crate::address::SqlServerAddress;
use crate::client::{Conn, SqlError, block_on, values};
use crate::connection_strings::{self, explained, for_server};
use crate::model::{DatabaseServerOptions, SiteSqlConnection};

/// `ISqlConnectionTester`: a trait so the site checks can be tested without a server.
pub trait SqlConnectionTester: Sync {
    /// Signs in to `connection`'s database (not [master] - contained users only exist in their own database) and
    /// describes what it reached, e.g. "[db] on Azure SQL Database 12.0.2000.8".
    fn test(
        &self,
        connection: &SiteSqlConnection,
        timeout_seconds: u64,
        cancel: Option<&CancelToken>,
    ) -> Result<String, String>;

    /// The names of all databases on `server`'s SQL Server.
    fn list_databases(
        &self,
        server: &SiteSqlConnection,
        timeout_seconds: u64,
        cancel: Option<&CancelToken>,
    ) -> Result<Vec<String>, String>;
}

/// The tester that asks the server.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlClientTester;

impl SqlConnectionTester for SqlClientTester {
    fn test(
        &self,
        connection: &SiteSqlConnection,
        timeout_seconds: u64,
        cancel: Option<&CancelToken>,
    ) -> Result<String, String> {
        if connection.server.trim().is_empty() {
            return Err("SQL server is empty.".into());
        }
        let database = if connection.database.trim().is_empty() { "master" } else { connection.database.as_str() };
        // No user: Windows authentication, as DNN Manager's own account.
        let settings = for_server(
            &connection.server,
            Some(database),
            &connection.user,
            Some(&connection.password),
            timeout_seconds,
        );
        let row = block_on(cancel, async move {
            let mut conn = Conn::open(&settings).await?;
            let rows = conn
                .rows(
                    "SELECT DB_NAME(), CAST(SERVERPROPERTY('EngineEdition') AS int), CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128))",
                    &[],
                )
                .await?;
            Ok(rows.first().map(values).unwrap_or_default())
        })
        .map_err(|e| failure(&e))?;
        let database = row.first().and_then(|v| v.as_text()).unwrap_or_default();
        let engine = if row.get(1).and_then(|v| v.as_i64()) == Some(5) { "Azure SQL Database" } else { "SQL Server" };
        let version = row.get(2).and_then(|v| v.as_text()).map(|v| format!(" {v}")).unwrap_or_default();
        Ok(format!("[{database}] on {engine}{version}"))
    }

    fn list_databases(
        &self,
        server: &SiteSqlConnection,
        timeout_seconds: u64,
        cancel: Option<&CancelToken>,
    ) -> Result<Vec<String>, String> {
        if server.server.trim().is_empty() {
            return Err("SQL server is empty.".into());
        }
        // No pool: each time asked for real, so a server that comes back is seen at the next look.
        let settings =
            for_server(&server.server, Some("master"), &server.user, Some(&server.password), timeout_seconds);
        block_on(cancel, async move {
            let mut conn = Conn::open(&settings).await?;
            let rows = conn.rows("SELECT name FROM sys.databases", &[]).await?;
            Ok(rows.iter().filter_map(|r| values(r).into_iter().next().and_then(|v| v.into_text())).collect())
        })
        .map_err(|e| failure(&e))
    }
}

/// A cancel stays "Cancelled."; anything else is the message, explained when it is about a certificate.
fn failure(e: &SqlError) -> String {
    if e.cancelled { e.message.clone() } else { explained(&e.message) }
}

/// What a look at a site's database found: does its server answer, is the database on it, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteDatabaseCheck {
    pub reachable: bool,
    pub exists: bool,
    pub problem: Option<String>,
}

impl SiteDatabaseCheck {
    fn live() -> Self {
        Self { reachable: true, exists: true, problem: None }
    }
}

/// A server that doesn't answer is given this long - the sites are asked in parallel.
pub const TIMEOUT_SECONDS: u64 = 5;

/// What each of `sites` (name → its web.config connection) found, by name (`SiteDatabaseChecks.AskAsync`): once per
/// server and login for the list of its databases, and a site's own database directly when that list can't be read
/// or doesn't have it. A server that doesn't answer is asked once more, not once per site.
pub fn ask(
    tester: &dyn SqlConnectionTester,
    sites: &BTreeMap<String, SiteSqlConnection>,
    cancel: Option<&CancelToken>,
) -> BTreeMap<String, SiteDatabaseCheck> {
    let found = Mutex::new(BTreeMap::new());
    let ask_directly = |site: &SiteSqlConnection| match tester.test(site, TIMEOUT_SECONDS, cancel) {
        Ok(_) => SiteDatabaseCheck::live(),
        Err(e) => SiteDatabaseCheck { reachable: false, exists: false, problem: Some(first_line(Some(&e))) },
    };

    // Server, user and password: one list of databases each.
    type Login = (String, String, String);
    let mut servers: Vec<(Login, Vec<(&String, &SiteSqlConnection)>)> = Vec::new();
    for (name, site) in sites {
        let key = (site.server.clone(), site.user.clone(), site.password.clone());
        match servers.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push((name, site)),
            None => servers.push((key, vec![(name, site)])),
        }
    }

    std::thread::scope(|scope| {
        for ((server, user, password), of_server) in &servers {
            let found = &found;
            let ask_directly = &ask_directly;
            scope.spawn(move || {
                let master = SiteSqlConnection::new(server, "master", user, password);
                let list = tester.list_databases(&master, TIMEOUT_SECONDS, cancel);
                if let Ok(names) = &list {
                    let databases: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
                    let databases = &databases;
                    std::thread::scope(|inner| {
                        for (name, site) in of_server {
                            inner.spawn(move || {
                                let check = if databases.contains(&site.database.to_lowercase()) {
                                    SiteDatabaseCheck::live()
                                } else {
                                    let direct = ask_directly(site);
                                    if direct.reachable {
                                        direct
                                    } else {
                                        SiteDatabaseCheck {
                                            reachable: true,
                                            exists: false,
                                            problem: Some(format!("[{}] isn't on {}", site.database, site.server)),
                                        }
                                    }
                                };
                                found.lock().unwrap().insert((*name).clone(), check);
                            });
                        }
                    });
                    return;
                }

                // The list can't be read: one site is asked directly. Failing the same way, the server (or the
                // login) is the problem - every site of it has that answer. Otherwise its login may see only its own
                // database (a contained database user): each is asked.
                let (first_name, first_site) = of_server[0];
                let first = ask_directly(first_site);
                found.lock().unwrap().insert(first_name.clone(), first.clone());
                let list_error = first_line(list.as_ref().err().map(String::as_str));
                if !first.reachable && first.problem.as_deref() == Some(list_error.as_str()) {
                    for (name, _) in &of_server[1..] {
                        found.lock().unwrap().insert((*name).clone(), first.clone());
                    }
                } else {
                    std::thread::scope(|inner| {
                        for (name, site) in &of_server[1..] {
                            inner.spawn(move || {
                                let check = ask_directly(site);
                                found.lock().unwrap().insert((*name).clone(), check);
                            });
                        }
                    });
                }
            });
        }
    });
    found.into_inner().unwrap()
}

fn first_line(text: Option<&str>) -> String {
    text.unwrap_or("it doesn't answer").split('\n').next().unwrap_or_default().trim().to_string()
}

/// The connection the Projects table asks a site's database with, from its web.config (`ServerStateMonitor`): the
/// site's login - or Windows authentication, as DNN Manager's user, only for a server on this PC or the one in
/// Settings → Database server (the site can change its own web.config). None for a LocalDB file or instance - asking
/// every few seconds would keep the instance running - or a server it may not sign in to as the user.
pub fn site_sql_connection(
    connection: &dnnm_infrastructure::web_config::DatabaseConnection,
    chosen: &DatabaseServerOptions,
) -> Option<SiteSqlConnection> {
    use dnnm_infrastructure::web_config::DatabaseKind;
    if connection.kind == DatabaseKind::LocalDbFile
        || connection.database.is_empty()
        || SqlServerAddress::parse(&connection.server).is_local_db
        || connection_strings::is_local_db(&connection.server)
    {
        return None;
    }
    if connection.windows_authentication {
        SqlServerAddress::may_sign_in_as_user(&connection.server, chosen)
            .then(|| SiteSqlConnection::new(&connection.server, &connection.database, "", ""))
    } else {
        Some(SiteSqlConnection::new(&connection.server, &connection.database, &connection.user, &connection.password))
    }
}

/// One site's database, asked as the Projects table asks it (5 s, the site's own connection) - for a caller that has
/// a single site; several go through [`ask`], which asks each server once.
pub fn check_site(
    tester: &dyn SqlConnectionTester,
    site: &SiteSqlConnection,
    cancel: Option<&CancelToken>,
) -> SiteDatabaseCheck {
    let mut sites = BTreeMap::new();
    sites.insert(String::new(), site.clone());
    ask(tester, &sites, cancel).remove("").unwrap_or(SiteDatabaseCheck {
        reachable: false,
        exists: false,
        problem: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeTester {
        lists: AtomicUsize,
        tests: AtomicUsize,
        /// The list can't be read on any server (a login that can't sign in to master).
        list_fails: bool,
    }

    impl SqlConnectionTester for FakeTester {
        fn test(&self, c: &SiteSqlConnection, _: u64, _: Option<&CancelToken>) -> Result<String, String> {
            self.tests.fetch_add(1, Ordering::SeqCst);
            if c.database == "dnn_contained" {
                Ok("[dnn_contained] on SQL Server".into())
            } else {
                Err("A network-related error occurred.\nMore detail.".into())
            }
        }

        fn list_databases(
            &self,
            s: &SiteSqlConnection,
            _: u64,
            _: Option<&CancelToken>,
        ) -> Result<Vec<String>, String> {
            self.lists.fetch_add(1, Ordering::SeqCst);
            if s.server == "gone,1433" {
                Err("A network-related error occurred.".into())
            } else if self.list_fails {
                Err("Login failed for user 'user'.".into())
            } else {
                Ok(vec!["master".into(), "dnn_live".into()])
            }
        }
    }

    fn check(reachable: bool, exists: bool, problem: Option<&str>) -> SiteDatabaseCheck {
        SiteDatabaseCheck { reachable, exists, problem: problem.map(str::to_string) }
    }

    #[test]
    fn each_database_is_live_missing_or_offline() {
        let tester = FakeTester::default();
        let sites: BTreeMap<String, SiteSqlConnection> = [
            ("live", SiteSqlConnection::new("localhost,1433", "dnn_live", "sa", "secret")),
            ("missing", SiteSqlConnection::new("localhost,1433", "dnn_missing", "sa", "secret")),
            ("contained", SiteSqlConnection::new("localhost,1433", "dnn_contained", "sa", "secret")),
            ("offline", SiteSqlConnection::new("gone,1433", "dnn_gone", "sa", "secret")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let checks = ask(&tester, &sites, None);

        assert_eq!(checks["live"], check(true, true, None));
        assert_eq!(checks["missing"], check(true, false, Some("[dnn_missing] isn't on localhost,1433")));
        assert_eq!(checks["contained"], check(true, true, None), "Not in its login's list, but it answers directly.");
        assert_eq!(checks["offline"], check(false, false, Some("A network-related error occurred.")));
        assert_eq!(tester.lists.load(Ordering::SeqCst), 2, "The list is read once per server and login.");
    }

    #[test]
    fn a_server_that_doesnt_answer_is_asked_once_not_once_per_site() {
        let tester = FakeTester::default();
        let sites: BTreeMap<String, SiteSqlConnection> = (0..19)
            .map(|i| (format!("site{i}"), SiteSqlConnection::new("gone,1433", &format!("dnn_{i}"), "sa", "secret")))
            .collect();

        let checks = ask(&tester, &sites, None);

        assert_eq!(checks.len(), 19);
        assert!(checks.values().all(|c| *c == check(false, false, Some("A network-related error occurred."))));
        assert_eq!(tester.lists.load(Ordering::SeqCst), 1);
        assert_eq!(
            tester.tests.load(Ordering::SeqCst),
            1,
            "Each connection to a server that is down waits for the time-out."
        );
    }

    #[test]
    fn a_login_that_sees_only_its_own_database_is_asked_per_site() {
        let tester = FakeTester { list_fails: true, ..Default::default() };
        let sites: BTreeMap<String, SiteSqlConnection> = [
            ("contained", SiteSqlConnection::new("localhost,1433", "dnn_contained", "user", "secret")),
            ("other", SiteSqlConnection::new("localhost,1433", "dnn_other", "user", "secret")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let checks = ask(&tester, &sites, None);

        assert_eq!(checks["contained"], check(true, true, None));
        assert!(!checks["other"].reachable);
        assert_eq!(
            tester.tests.load(Ordering::SeqCst),
            2,
            "The first answered otherwise than the list: each site is asked."
        );
    }

    #[test]
    fn one_site_is_checked_as_the_table_checks_it() {
        let tester = FakeTester::default();
        let site = SiteSqlConnection::new("localhost,1433", "dnn_missing", "sa", "secret");
        assert_eq!(check_site(&tester, &site, None), check(true, false, Some("[dnn_missing] isn't on localhost,1433")));
    }

    #[test]
    fn the_monitor_signs_in_as_the_user_only_where_it_may() {
        use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind};
        let chosen = DatabaseServerOptions { kind: "container".into(), server: String::new() };
        let c = |server: &str, windows: bool| DatabaseConnection {
            kind: DatabaseKind::SqlServer,
            server: server.into(),
            database: "shop".into(),
            windows_authentication: windows,
            user: if windows { String::new() } else { "u".into() },
            password: if windows { String::new() } else { "p".into() },
        };
        assert_eq!(
            site_sql_connection(&c(r".\SQLEXPRESS", true), &chosen),
            Some(SiteSqlConnection::new(r".\SQLEXPRESS", "shop", "", ""))
        );
        assert_eq!(site_sql_connection(&c("evil.example", true), &chosen), None);
        assert_eq!(
            site_sql_connection(&c("db.example.com", false), &chosen),
            Some(SiteSqlConnection::new("db.example.com", "shop", "u", "p"))
        );
        assert_eq!(site_sql_connection(&c(r"(localdb)\MSSQLLocalDB", true), &chosen), None);
    }

    /// The real tester against a server that isn't there: the table's answer and its 5 s.
    #[test]
    fn a_server_that_isnt_there_is_not_reachable() {
        let site = SiteSqlConnection::new("127.0.0.1,1", "shop", "sa", "x");
        let result = check_site(&SqlClientTester, &site, None);
        assert!(!result.reachable && !result.exists);
        assert!(result.problem.unwrap().starts_with("A network-related or instance-specific error occurred"));
    }
}
