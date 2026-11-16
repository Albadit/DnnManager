//! The set-up with stand-ins for the system and IIS: the order of the steps, their texts, what is undone and what is
//! kept (`SetupProjectUseCaseTests`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use super::*;
use crate::iis::{IisBinding, IisSiteRuntime};
use crate::operation::tests::{Answer, Lines};
use crate::operation::{self, CancelToken, Outcome, Reporter};
use crate::use_cases::tests::FakeIis;

/// A stand-in system: records each call; `fail` names calls that fail; `existing` databases exist.
struct FakeSystem {
    root: PathBuf,
    calls: Mutex<Vec<String>>,
    fail: Vec<&'static str>,
    existing: Mutex<Vec<String>>,
    unreachable: bool,
    records: Mutex<Vec<ProjectRecord>>,
}

impl FakeSystem {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            calls: Mutex::new(vec![]),
            fail: vec![],
            existing: Mutex::new(vec![]),
            unreachable: false,
            records: Mutex::new(vec![]),
        }
    }

    fn call(&self, what: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(what.to_string());
        let name = what.split(' ').next().unwrap_or_default();
        if self.fail.contains(&name) { Err(format!("{name} broke")) } else { Ok(()) }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl SetupSystem for FakeSystem {
    fn project(&self, name: &str) -> DnnProject {
        DnnProject {
            name: name.into(),
            project_directory: self.root.join(name),
            backup_directory: self.root.join("backups").join(name),
        }
    }
    fn account_problems(&self, account: &DnnAccount) -> Vec<String> {
        if account.password.len() < 7 { vec!["The password needs at least 7 characters.".into()] } else { vec![] }
    }
    fn get_release(&self, api: &str, version: Option<&str>, _: &CancelToken) -> Result<DnnRelease, String> {
        self.call(&format!("release {api} {}", version.unwrap_or("latest")))?;
        Ok(DnnRelease {
            version: "10.0.1".into(),
            tag_name: "v10.0.1".into(),
            download_url: "https://example.org/DNN_Platform_10.0.1_Install.zip".into(),
            prerelease: false,
            sha256: None,
            upgrade_sha256: None,
        })
    }
    fn ensure_iis_features(&self, op: &mut Operation) -> Result<(), String> {
        op.report.success("IIS Web Server (IIS-WebServerRole)");
        self.call("features")
    }
    fn database_exists(&self, db: &DatabaseConnection, _: &CancelToken) -> Result<bool, String> {
        self.call(&format!("exists {}", db.database))?;
        if self.unreachable {
            return Err("A network-related error occurred.".into());
        }
        Ok(self.existing.lock().unwrap().contains(&db.database))
    }
    fn drop_database(&self, db: &DatabaseConnection, _: &CancelToken) -> Result<(), String> {
        self.existing.lock().unwrap().retain(|d| d != &db.database);
        self.call(&format!("drop {}", db.database))
    }
    fn check_database(
        &self,
        db: &DatabaseConnection,
        options: &DatabaseCheckOptions,
        _: &CancelToken,
    ) -> Result<Vec<DatabaseCheck>, String> {
        self.calls.lock().unwrap().push(format!(
            "check {} {} {}",
            db.database,
            options.minimum_major_version,
            options.site_login.as_deref().unwrap_or("-")
        ));
        let outcome = if self.unreachable { CheckOutcome::Failed } else { CheckOutcome::Passed };
        Ok(vec![DatabaseCheck { name: "Server reachable".into(), outcome, detail: "SQL Server 16".into() }])
    }
    fn download_and_extract(
        &self,
        release: &DnnRelease,
        dir: &Path,
        reporter: &mut dyn Reporter,
        _: &CancelToken,
    ) -> Result<(), String> {
        reporter.success("Extraction complete.");
        assert!(dir.is_dir());
        self.call(&format!("download {}", release.version))
    }
    fn ensure_gitignore(&self, _: &Path) -> Result<(), String> {
        self.call("gitignore")
    }
    fn check_package(&self, _: &Path) -> Result<(), String> {
        self.call("package")
    }
    fn prepare_local_db_file(&self, _: &Path, _: &DatabaseConnection, _: &CancelToken) -> Result<(), String> {
        self.call("prepare")
    }
    fn create_database(&self, db: &DatabaseConnection, collation: Option<&str>, _: &CancelToken) -> Result<(), String> {
        self.call(&format!("create {} {}", db.database, collation.unwrap_or("-")))?;
        self.existing.lock().unwrap().push(db.database.clone());
        Ok(())
    }
    fn grant_site_access(&self, db: &DatabaseConnection, identity: &str, _: &CancelToken) -> Result<(), String> {
        self.call(&format!("access {} {identity}", db.database))
    }
    fn grant_site_login(&self, project: &str, db: &str, _: &CancelToken) -> Result<(String, String), String> {
        self.call(&format!("login {project} {db}"))?;
        Ok((format!("dnn_{project}"), "Secret-1234".into()))
    }
    fn drop_login(&self, login: &str, _: &CancelToken) -> Result<(), String> {
        self.call(&format!("droplogin {login}"))
    }
    fn write_database_connection(&self, _: &Path, db: &DatabaseConnection) -> Result<(), String> {
        self.call(&format!("webconfig {} {}", db.user, db.password))
    }
    fn install(
        &self,
        site: &SiteAddress,
        _: &DnnAccount,
        _: &DatabaseConnection,
        _: &mut dyn Reporter,
        _: &CancelToken,
    ) -> Result<(), String> {
        self.call(&format!("install {}", site.alias))
    }
    fn complete(&self, _: &SiteAddress, a: &DnnAccount, _: &DatabaseConnection, _: &CancelToken) -> Result<(), String> {
        self.call(&format!("complete {}", a.user_name))
    }
    fn warm_up(&self, _: &SiteAddress, _: SystemTime, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
        self.call("warmup")
    }
    fn clean_up(&self, _: &Path, installed: bool) {
        let _ = self.call(&format!("cleanup {installed}"));
    }
    fn http_check(&self, url: &str, _: u64, _: &CancelToken) -> Result<u16, String> {
        self.call(&format!("http {url}"))?;
        Ok(200)
    }
    fn save_record(&self, record: &ProjectRecord) {
        self.records.lock().unwrap().push(record.clone());
    }
}

fn options() -> SetupOptions {
    SetupOptions {
        hostname_suffix: "dnndev.me".into(),
        site_port: 80,
        container_host: "localhost".into(),
        container_port: 1433,
        container_user: "sa".into(),
        container_password: "Sa-Password1".into(),
        collation: "Latin1_General_CI_AS".into(),
    }
}

fn account() -> DnnAccount {
    DnnAccount {
        user_name: "host".into(),
        password: "Host-Password1".into(),
        email: "host@dnndev.me".into(),
        website_name: "Shop".into(),
        language: "en-US".into(),
        template: "Default Website".into(),
    }
}

fn request(mode: InstallMode) -> SetupProjectRequest {
    SetupProjectRequest {
        project_name: "shop".into(),
        release_api_url: "https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases".into(),
        version: Some("v10.0.1".into()),
        host_name: Some("shop.dnndev.me".into()),
        port: Some(8080),
        install_mode: mode,
        account: (mode == InstallMode::Automatic).then(account),
        database: None,
    }
}

struct Run {
    outcome: Outcome,
    lines: Vec<String>,
    questions: Vec<String>,
}

fn run_setup(system: &Arc<FakeSystem>, iis: &Arc<FakeIis>, request: &SetupProjectRequest, answer: Answer) -> Run {
    let deps = SetupDeps { system: system.clone(), iis: iis.clone(), options: options() };
    let mut lines = Lines::default();
    let outcome = operation::run(&mut lines, &answer, CancelToken::new(), &CancelToken::new(), |op| {
        setup_project(&deps, request, op)
    });
    let questions = answer.1.lock().unwrap().clone();
    Run { outcome, lines: lines.0, questions }
}

#[test]
fn an_automatic_setup_runs_every_stage_in_order_and_keeps_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem::new(dir.path()));
    let iis = Arc::new(FakeIis::running());
    let run = run_setup(&system, &iis, &request(InstallMode::Automatic), Answer::yes());
    assert_eq!(run.outcome, Outcome::Finished, "{:#?}", run.lines);
    assert_eq!(
        system.calls(),
        vec![
            "release https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases v10.0.1",
            "features",
            "exists shop",
            "check shop 14 -",
            "exists shop",
            "download 10.0.1",
            "gitignore",
            "package",
            "create shop Latin1_General_CI_AS",
            "login shop shop",
            "webconfig dnn_shop Secret-1234",
            "install shop.dnndev.me:8080",
            "complete host",
            "warmup",
            "cleanup true",
            "cleanup false",
        ]
    );
    let steps: Vec<&str> = run.lines.iter().filter_map(|l| l.strip_prefix("## ")).collect();
    assert_eq!(
        steps,
        vec![
            "Step 1: DNN version",
            "Step 2: Checking IIS",
            "Step 3: Testing database connection",
            "Step 4: Creating project directory",
            "Step 5: Downloading DNN 10.0.1",
            "Step 6: Creating IIS application pool and website",
            "Step 7: Creating database",
            "Step 8: Configuring DNN",
            "Step 9: Running DNN installation",
            "Step 10: Creating host account",
            "Step 11: Starting website",
            "Setup complete",
        ]
    );
    assert!(run.lines.contains(&"ok Using DNN 10.0.1 (https://example.org/DNN_Platform_10.0.1_Install.zip)".into()));
    assert!(run.lines.contains(&"ok web.config connects to [shop] on localhost,1433 (user 'dnn_shop').".into()));
    assert!(
        run.lines
            .contains(&"ok Host account 'host' ready - it signs in without being asked to change its password.".into())
    );
    assert_eq!(
        run.lines.last().unwrap(),
        "ok DNN installation completed. Open http://shop.dnndev.me:8080 - sign in as 'host'."
    );
    assert!(!run.lines.iter().any(|l| l.contains("Secret-1234")), "the site login's password is never reported");
    assert!(iis.calls.lock().unwrap().contains(&"create shop.dnndev.me:8080 shop".to_string()));
    assert!(iis.calls.lock().unwrap().contains(&"recycle shop".to_string()));
    let records = system.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].install_mode, InstallMode::Automatic);
    assert_eq!(records[0].host_user_name.as_deref(), Some("host"));
}

#[test]
fn a_failure_before_the_install_undoes_everything_made() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem { fail: vec!["webconfig"], ..FakeSystem::new(dir.path()) });
    let iis = Arc::new(FakeIis::running());
    let run = run_setup(&system, &iis, &request(InstallMode::Automatic), Answer::yes());
    assert_eq!(run.outcome, Outcome::Failed("Could not write the site's connection string: webconfig broke".into()));
    let undo: Vec<&String> = run.lines.iter().skip_while(|l| *l != "## Undo").collect();
    assert_eq!(
        undo,
        vec![
            "## Undo",
            "ok Drop the login dnn_shop",
            "ok Drop database [shop] on localhost,1433",
            "ok Remove the IIS site and app pool 'shop'",
            &format!("ok Delete {}", dir.path().join("shop").display()),
        ]
    );
    assert!(system.calls().contains(&"droplogin dnn_shop".to_string()));
    assert!(system.calls().contains(&"drop shop".to_string()));
    assert!(!dir.path().join("shop").exists());
    assert!(system.records.lock().unwrap().is_empty());
}

#[test]
fn a_failed_dnn_install_is_kept_to_look_into() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem { fail: vec!["install"], ..FakeSystem::new(dir.path()) });
    let iis = Arc::new(FakeIis::running());
    let run = run_setup(&system, &iis, &request(InstallMode::Automatic), Answer::yes());
    assert_eq!(
        run.outcome,
        Outcome::Failed(
            "DNN's installation failed: install broke The project is left as it is to look into - remove it and set it up again (DNN can't install twice into the same files and database).".into()
        )
    );
    assert!(!run.lines.contains(&"## Undo".to_string()), "nothing is undone: {:#?}", run.lines);
    assert!(dir.path().join("shop").is_dir());
    assert_eq!(system.calls().iter().filter(|c| *c == "cleanup false").count(), 2);
}

#[test]
fn an_existing_database_is_dropped_only_when_the_user_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem::new(dir.path()));
    system.existing.lock().unwrap().push("shop".into());
    let iis = Arc::new(FakeIis::running());
    let run = run_setup(&system, &iis, &request(InstallMode::Automatic), Answer::yes());
    assert_eq!(run.outcome, Outcome::Finished);
    assert_eq!(
        run.questions,
        vec![
            "Database [shop] already exists on localhost,1433. Drop it and create it again? Everything in it is lost."
        ]
    );
    assert!(system.calls().contains(&"drop shop".to_string()));
    assert!(run.lines.contains(&"Dropped database [shop].".to_string()));

    // A manual install keeps the database that is there, and creates none.
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem::new(dir.path()));
    system.existing.lock().unwrap().push("shop".into());
    let run = run_setup(&system, &iis, &request(InstallMode::Manual), Answer::no());
    assert_eq!(run.outcome, Outcome::Finished, "{:#?}", run.lines);
    assert!(run.lines.contains(&"Keeping database [shop] as it is.".to_string()));
    assert!(!system.calls().iter().any(|c| c.starts_with("create") || c.starts_with("drop")));
}

#[test]
fn a_manual_setup_verifies_the_site_and_says_how_to_finish() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem::new(dir.path()));
    let iis = Arc::new(FakeIis::running());
    let mut req = request(InstallMode::Manual);
    req.database = Some(DatabaseConnection::windows(DatabaseKind::SqlServer, r".\SQLEXPRESS", "shopdb"));
    let run = run_setup(&system, &iis, &req, Answer::yes());
    assert_eq!(run.outcome, Outcome::Finished, "{:#?}", run.lines);
    let calls = system.calls();
    assert!(calls.contains(&r"check shopdb 14 IIS APPPOOL\shop".to_string()));
    assert!(calls.contains(&r"access shopdb IIS APPPOOL\shop".to_string()));
    assert!(calls.contains(&"http http://shop.dnndev.me:8080".to_string()));
    assert!(!calls.iter().any(|c| c.starts_with("install") || c.starts_with("package")));
    assert!(run.lines.contains(&r"In DNN's installation wizard, connect to: server '.\SQLEXPRESS', database 'shopdb', Windows authentication (integrated security).".to_string()));
    assert!(run.lines.contains(&"## Verify site".to_string()));
    assert_eq!(
        run.lines.last().unwrap(),
        "ok Open http://shop.dnndev.me:8080 to complete the DNN Installation Wizard."
    );
    assert_eq!(system.records.lock().unwrap()[0].install_mode, InstallMode::Manual);
}

#[test]
fn a_manual_setup_goes_on_without_an_unreachable_default_database() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem { unreachable: true, ..FakeSystem::new(dir.path()) });
    let iis = Arc::new(FakeIis::running());
    let run = run_setup(&system, &iis, &request(InstallMode::Manual), Answer::yes());
    assert_eq!(run.outcome, Outcome::Finished, "{:#?}", run.lines);
    assert!(run.lines.contains(&"! Server reachable: SQL Server 16".to_string()));
    assert!(run.lines.contains(&"Skipping the database. Start the SQL Server container and set the project up again, or point the site's web.config at your own database.".to_string()));
    assert!(run.lines.contains(&"Skipped - the database isn't reachable.".to_string()));

    // A database chosen on the page must be there.
    let mut req = request(InstallMode::Manual);
    req.database = Some(options().container_connection("shop"));
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem { unreachable: true, ..FakeSystem::new(dir.path()) });
    let run = run_setup(&system, &iis, &req, Answer::yes());
    assert_eq!(run.outcome, Outcome::Failed("The database isn't ready: Server reachable: SQL Server 16".into()));
    assert!(!dir.path().join("shop").exists());
}

#[test]
fn what_is_checked_before_anything_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem::new(dir.path()));
    let iis = Arc::new(FakeIis::running());

    let mut req = request(InstallMode::Automatic);
    req.account.as_mut().unwrap().password = "abc".into();
    let run = run_setup(&system, &iis, &req, Answer::yes());
    assert_eq!(run.outcome, Outcome::Failed("The password needs at least 7 characters.".into()));

    req.account = None;
    let run = run_setup(&system, &iis, &req, Answer::yes());
    assert_eq!(run.outcome, Outcome::Failed("An automatic install needs the host account and the website.".into()));

    let mut req = request(InstallMode::Manual);
    req.project_name = "bad name".into();
    assert!(
        matches!(run_setup(&system, &iis, &req, Answer::yes()).outcome, Outcome::Failed(e) if e.contains("unsupported character ' '"))
    );

    std::fs::create_dir(dir.path().join("shop")).unwrap();
    let run = run_setup(&system, &iis, &request(InstallMode::Manual), Answer::yes());
    assert_eq!(
        run.outcome,
        Outcome::Failed(format!(
            "A project named 'shop' already exists ({}). Choose another name.",
            dir.path().join("shop").display()
        ))
    );
    assert!(system.calls().is_empty());
}

#[test]
fn an_address_another_site_has_stops_the_setup_before_the_download() {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(FakeSystem::new(dir.path()));
    let iis = Arc::new(FakeIis::running().with_site("other", r"D:\other"));
    iis.sites.lock().unwrap().get_mut("other").unwrap().bindings =
        vec![IisBinding::parse("http", "*:8080:Shop.dnndev.me.", false)];
    let run = run_setup(&system, &iis, &request(InstallMode::Automatic), Answer::yes());
    assert_eq!(
        run.outcome,
        Outcome::Failed(
            "http://shop.dnndev.me:8080 is already the address of the IIS site 'other' - choose another host name or port."
                .into()
        )
    );
    assert!(!system.calls().iter().any(|c| c.starts_with("download")));
}

#[test]
fn site_using_ignores_the_site_being_replaced_and_other_ports() {
    let mut site = IisSiteRuntime {
        id: 1,
        state: "Started".into(),
        app_pool: "a".into(),
        app_pool_state: None,
        worker_process_ids: vec![],
        bindings: vec![IisBinding::parse("http", "*:80:shop.dnndev.me", false)],
        physical_path: PathBuf::new(),
    };
    let sites = [("a", &site)];
    assert_eq!(site_using(sites, " SHOP.dnndev.me ", 80, None), Some("a".into()));
    assert_eq!(site_using(sites, "shop.dnndev.me", 80, Some("A")), None);
    assert_eq!(site_using(sites, "shop.dnndev.me", 81, None), None);
    assert_eq!(site_using(sites, "", 80, None), None);
    site.bindings[0].protocol = "https".into();
    assert_eq!(site_using([("a", &site)], "shop.dnndev.me", 80, None), None);
}

#[test]
fn minimum_sql_server_and_lower() {
    let release = |v: &str| DnnRelease {
        version: v.into(),
        tag_name: v.into(),
        download_url: String::new(),
        prerelease: false,
        sha256: None,
        upgrade_sha256: None,
    };
    assert_eq!(minimum_sql_server_for(&release("9.13.4")), 11);
    assert_eq!(minimum_sql_server_for(&release("v10.0.1")), 14);
    assert_eq!(minimum_sql_server_for(&release("next")), 14);
    assert_eq!(lower("The site didn't answer."), "the site didn't answer.");
    assert_eq!(lower(""), "something went wrong.");
}

#[test]
fn connections_describe_themselves_without_the_password() {
    let c = DatabaseConnection::sql_login(DatabaseKind::SqlServer, "db1", "shop", "dnn", "Secret");
    assert_eq!(format!("{c:?}"), "[shop] on db1 (user 'dnn')");
    let file =
        DatabaseConnection::windows(DatabaseKind::LocalDbFile, DatabaseConnection::LOCAL_DB_SERVER, "Database.mdf");
    assert_eq!(file.describe(), r"App_Data\Database.mdf with (LocalDB)\MSSQLLocalDB (Windows authentication)");
    assert!(!format!("{:?}", account()).contains("Host-Password1"));
}
