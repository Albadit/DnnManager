//! The New project page's rules, without a window: what each field says and what is still needed.

use std::path::{Path, PathBuf};

use super::*;
use dnnm_application::iis::IisBinding;

fn site(path: &str, binding: &str) -> IisSiteRuntime {
    IisSiteRuntime {
        id: 1,
        state: "Started".into(),
        app_pool: "x".into(),
        app_pool_state: None,
        worker_process_ids: vec![],
        bindings: vec![IisBinding::parse("http", binding, false)],
        physical_path: PathBuf::from(path),
    }
}

fn release(version: &str, prerelease: bool) -> DnnRelease {
    DnnRelease {
        version: version.into(),
        tag_name: format!("v{version}"),
        download_url: String::new(),
        prerelease,
        sha256: None,
        upgrade_sha256: None,
    }
}

#[test]
fn repository_labels_and_language_names() {
    assert_eq!(
        repository_label("https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases"),
        "dnnsoftware/Dnn.Platform"
    );
    assert_eq!(repository_label("https://example.org/feed"), "https://example.org/feed");
    assert_eq!(repository_label("not a url"), "not a url");
    assert_eq!(language_name("de-DE"), "Deutsch (Deutschland) (de-DE)");
    assert_eq!(language_name("xx-YY"), "xx-YY");
}

#[test]
fn versions_mark_the_latest_pre_releases_and_kept_packages() {
    let list = DnnReleaseList {
        releases: vec![release("10.1.0-rc1", true), release("10.0.1", false), release("9.13.4", false)],
        saved_at: None,
    };
    let (options, hint, selected) =
        versions_for("dnnsoftware/Dnn.Platform", &Ok(list.clone()), &|r| r.version == "9.13.4", true);
    let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, vec!["10.1.0-rc1  (pre-release)", "10.0.1  (latest)", "9.13.4  - kept, no download"]);
    assert_eq!(selected, Some(1), "the latest release, not the newer pre-release");
    assert_eq!(hint, "3 releases of dnnsoftware/Dnn.Platform, highest version first - the latest release is selected.");

    // Saved as a round-trip time, shown to the minute.
    let offline = DnnReleaseList { saved_at: Some("2026-11-16T10:00:52.0135567+01:00".into()), ..list };
    let (_, hint, _) = versions_for("a/b", &Ok(offline), &|_| false, true);
    assert_eq!(
        hint,
        "GitHub can't be reached - the releases of a/b as of 2026-11-16 10:00. Without internet only a version marked \"kept, no download\" can be set up. GitHub is asked again when you come back to this page."
    );

    let (options, hint, selected) = versions_for("a/b", &Err("timed out".into()), &|_| false, false);
    assert_eq!((options[0].label.as_str(), options[0].release.is_none(), selected), ("Latest release", true, Some(0)));
    assert!(hint.starts_with("Could not load the versions (timed out) - the latest release is used."));
    assert!(hint.ends_with("Keep downloaded DNN install packages while online."));
    let (_, hint, _) = versions_for("a/b", &Err("timed out".into()), &|_| false, true);
    assert!(hint.ends_with("GitHub is asked again when you come back to this page."));

    let (options, hint, selected) = versions_for("a/b", &Ok(DnnReleaseList::default()), &|_| false, true);
    assert!(options.is_empty() && selected.is_none());
    assert_eq!(hint, "a/b has no release with a DNN install package.");
}

#[test]
fn iis_problems_and_hints() {
    assert_eq!(iis_problem("", "80", None, "shop").as_deref(), Some("Enter the host name the site answers on."));
    assert_eq!(
        iis_problem("shop_x.me", "80", None, "shop").as_deref(),
        Some("The host name can only have letters, digits, '.' and '-'.")
    );
    assert_eq!(
        iis_problem("shop.me", "0", None, "shop").as_deref(),
        Some("The port must be a number between 1 and 65535.")
    );
    assert!(iis_problem("shop.me", "65536", None, "shop").is_some());
    let sites = vec![("other".to_string(), site(r"D:\other", "*:8080:shop.dnndev.me"))];
    assert_eq!(
        iis_problem("shop.dnndev.me", "8080", Some(&sites), "shop").as_deref(),
        Some(
            "http://shop.dnndev.me:8080 is already the address of the IIS site 'other' - choose another host name or port."
        )
    );
    assert_eq!(iis_problem("shop.dnndev.me", "8080", Some(&sites), "other"), None, "the site it replaces");

    let (hint, bad) = iis_hint("", "", "80", None, "dnndev.me");
    assert!(!bad);
    assert_eq!(
        hint,
        "The IIS website and its app pool are named '<project>'; the site answers at http://<host> - DNN Manager adds the host name to this PC's hosts file, so it opens without internet too."
    );
    let (hint, _) = iis_hint("shop", "shop.example.com", "8080", None, "dnndev.me");
    assert!(hint.contains("http://shop.example.com:8080") && hint.ends_with("not what the internet has at that name."));
    assert_eq!(
        iis_hint("shop", "", "x", None, "dnndev.me"),
        ("The port must be a number between 1 and 65535.".into(), true)
    );
    // An empty host name isn't an error yet.
    assert!(!iis_hint("", "", "80", Some("Enter the host name the site answers on."), "dnndev.me").1);
    assert!(!is_custom_domain("shop.DNNDEV.me", "dnndev.me") && !is_custom_domain("dnndev.me", "dnndev.me"));
}

#[test]
fn names_that_cant_be_used() {
    assert_eq!(name_error("", false, None), None);
    assert!(name_error("bad name", false, None).unwrap().contains("unsupported character"));
    assert_eq!(
        name_error("shop", true, None).as_deref(),
        Some("A project named 'shop' already exists. Choose another name.")
    );
    assert_eq!(
        name_error("shop", false, Some(r"D:\other")).as_deref(),
        Some(r"IIS already has a site named 'shop', serving D:\other - choose another name.")
    );
    let sites = vec![("Shop".to_string(), site(r"C:\DNN\shop\", "*:80:shop.me"))];
    assert_eq!(site_serving_another_folder(Some(&sites), "shop", Path::new(r"C:\DNN\shop")), None);
    assert_eq!(
        site_serving_another_folder(Some(&sites), "shop", Path::new(r"C:\DNN\x")).as_deref(),
        Some(r"C:\DNN\shop\")
    );
    assert_eq!(site_serving_another_folder(None, "shop", Path::new(r"C:\DNN\x")), None);
}

#[test]
fn import_problems() {
    let there = |p: &Path| p.to_string_lossy().starts_with("C:\\there");
    assert_eq!(import_problem("", "", &there).as_deref(), Some("Choose the site .zip."));
    assert_eq!(import_problem("D:\\x.zip", "", &there).as_deref(), Some("File not found: D:\\x.zip"));
    assert_eq!(import_problem("C:\\there\\x.zip", " ", &there).as_deref(), Some("Choose the database .bacpac."));
    assert_eq!(
        import_problem("C:\\there\\x.zip", "C:\\there\\web.config.bak", &there).as_deref(),
        Some("The database backup must be a .bacpac or .bak file.")
    );
    assert_eq!(import_problem("C:\\there\\x.zip", "C:\\there\\shop.BAK", &there), None);
}

fn fields(db_type: DbType) -> DbFields {
    DbFields {
        db_type,
        server: "localhost,1433".into(),
        name: "shop".into(),
        sql_auth: false,
        user: "sa".into(),
        password: "pw".into(),
    }
}

#[test]
fn database_fields_problems_and_connections() {
    assert_eq!(fields(DbType::Container).problem(), None);
    let mut f = fields(DbType::LocalDb);
    f.server = " ".into();
    assert_eq!(f.problem().as_deref(), Some("Enter the LocalDB instance."));
    let mut f = fields(DbType::SqlServer);
    f.name = "a]b".into();
    assert_eq!(f.problem().as_deref(), Some("The database name can have at most 128 characters, and no ']'."));
    let mut f = fields(DbType::Container);
    f.password.clear();
    assert_eq!(f.problem().as_deref(), Some("Enter the password."));
    let f = DbFields { user: String::new(), password: String::new(), ..fields(DbType::SqlServer) };
    assert_eq!(f.problem(), None, "Windows authentication needs no login");
    assert_eq!(f.connection(), DatabaseConnection::windows(DatabaseKind::SqlServer, "localhost,1433", "shop"));
    let file = fields(DbType::LocalDb).connection();
    assert_eq!((file.kind, file.database.as_str()), (DatabaseKind::LocalDbFile, "Database.mdf"));
}

#[test]
fn database_texts_say_what_the_server_answered() {
    let f = fields(DbType::Container);
    assert_eq!(
        database_texts(&f, &DbAnswer::Asking, true, true, false),
        (String::new(), None, "Checking whether database [shop] on localhost,1433 is free…".into())
    );
    assert_eq!(
        database_texts(&f, &DbAnswer::Free, true, true, true).2,
        "Database [shop] on localhost,1433 is free. For this project only."
    );
    assert_eq!(
        database_texts(&f, &DbAnswer::Exists, true, true, false).0,
        "Database [shop] on localhost,1433 already exists - choose another name, or remove that database first."
    );
    assert_eq!(
        database_texts(&f, &DbAnswer::Exists, false, true, false).2,
        "Database [shop] on localhost,1433 already exists - the setup asks whether to drop it or install into it."
    );
    let (error, tip, _) =
        database_texts(&f, &DbAnswer::NoAnswer("Login failed for user 'sa'.".into()), true, true, false);
    assert_eq!(error, "SQL Server at localhost,1433 refuses the login.");
    assert_eq!(tip.as_deref(), Some("Login failed for user 'sa'.\n\nAsked again every 10 seconds."));
    assert_eq!(
        database_texts(&f, &DbAnswer::NoAnswer("timeout".into()), true, true, false).0,
        "Can't reach SQL Server at localhost,1433."
    );
    // An empty name isn't an error yet.
    let empty = DbFields { name: String::new(), ..f };
    assert_eq!(database_texts(&empty, &DbAnswer::None, true, false, false).0, "");
    assert_eq!(database_texts(&empty, &DbAnswer::None, true, true, false).0, "Enter the database name.");
    assert_eq!(
        database_texts(&fields(DbType::LocalDb), &DbAnswer::None, true, true, false).2,
        r"The site's own App_Data\Database.mdf."
    );
    assert!(database_free(DbType::LocalDb, &DbAnswer::None, true));
    assert!(database_free(DbType::Container, &DbAnswer::Exists, false));
    assert!(!database_free(DbType::Container, &DbAnswer::Exists, true));
    assert!(!database_free(DbType::Container, &DbAnswer::NoAnswer(String::new()), false));
}

#[test]
fn still_needed_names_what_is_missing_in_card_order() {
    let all = Readiness {
        name: true,
        iis: true,
        importing: false,
        import: false,
        version: true,
        database: true,
        account: true,
    };
    assert!(all.can_run());
    assert_eq!(all.still_needed(), "");
    let none = Readiness { name: false, iis: false, version: false, database: false, account: false, ..all };
    assert_eq!(
        none.still_needed(),
        "Still needed: a project name, a host name and port, a DNN version, a database, the host account."
    );
    let import = Readiness { importing: true, import: false, ..none };
    assert_eq!(
        import.still_needed(),
        "Still needed: a project name, a host name and port, the site's .zip and its database backup."
    );
    let ready_import = Readiness { name: true, iis: true, importing: true, import: true, ..none };
    assert!(ready_import.can_run(), "an import needs no version, database or account");
}

#[test]
fn hints_word_for_word() {
    assert_eq!(
        account_hint("en-US"),
        "The host (superuser) account signs in without being asked to change its password. Defaults in Settings → Projects → DNN defaults."
    );
    assert!(account_hint("nl-NL").ends_with(
        " Nederlands (Nederland) (nl-NL): DNN downloads its language pack while installing (needs internet)."
    ));
    assert_eq!(
        name_hint(true, "localhost,1433"),
        "Letters, digits, '-', '_' or '.' - the name becomes the folder, the IIS site, the hostname and the database, on the local SQL container (localhost,1433)."
    );
    assert!(name_hint(false, "x").ends_with("the hostname and the database (see Database below)."));
}

#[test]
fn defaults_come_from_the_settings() {
    let settings = Settings::from_rows([
        ("projects.dnnDefaults.hostEmail".to_string(), String::new()),
        ("projects.hostnameSuffix".to_string(), "local.test".to_string()),
        ("sqlServer.type".to_string(), "sqlServer".to_string()),
        ("sqlServer.server".to_string(), "db1".to_string()),
        ("sqlServer.authentication".to_string(), "sql".to_string()),
        ("sqlServer.userName".to_string(), "dnn".to_string()),
    ]);
    let dnn = DnnDefaults::of(&settings);
    assert!(dnn.automatic);
    assert_eq!((dnn.host_username.as_str(), dnn.host_email.as_str()), ("host", "host@local.test"));
    let secrets = PageSecrets {
        default_host_password: None,
        database_server_password: Some("pw".into()),
        sa_password: "sa-pw".into(),
    };
    let sql = database_defaults(&settings, &secrets, DbType::SqlServer, "shop");
    assert_eq!(
        (sql.server.as_str(), sql.sql_auth, sql.user.as_str(), sql.password.as_str()),
        ("db1", true, "dnn", "pw")
    );
    let container = database_defaults(&settings, &secrets, DbType::Container, "shop");
    assert_eq!(
        (container.server.as_str(), container.user.as_str(), container.password.as_str()),
        ("localhost,1433", "dnn", "sa-pw")
    );
    let local = database_defaults(&settings, &secrets, DbType::LocalDb, "shop");
    assert_eq!(local.server, DatabaseConnection::LOCAL_DB_SERVER);
    let other = database_defaults(&Settings::default(), &secrets, DbType::SqlServer, "shop");
    assert_eq!((other.server.as_str(), other.sql_auth, other.password.as_str()), (r".\SQLEXPRESS", false, ""));
}
