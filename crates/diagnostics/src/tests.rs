//! The tabs from made-up snapshots: their labels and texts are the WPF app's.

use std::time::Duration;

use dnnm_application::iis::{IisBindingDetails, IisPoolDetails, IisSiteDetails, IisSiteRuntime};
use dnnm_application::project::ProjectState;
use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind};

use crate::database::{DatabaseInspection, DatabaseProperties, DnnDatabaseFacts, DnnPortalDetails, SqlServerFacts};
use crate::format::LocalDateTime;
use crate::model::{Health, Section, TabKind};
use crate::snapshot::{FolderFacts, ProjectSnapshot};
use crate::tabs::{issues, same_version, tab, tabs};
use crate::web_config::WebConfigInspection;

fn pool() -> IisPoolDetails {
    IisPoolDetails {
        name: "shop".into(),
        state: Some("Started".into()),
        runtime: "v4.0".into(),
        pipeline: "Classic".into(),
        identity_type: "ApplicationPoolIdentity".into(),
        account: r"IIS APPPOOL\shop".into(),
        start_mode: "OnDemand".into(),
        idle_timeout: Duration::from_secs(1200),
        idle_timeout_action: Some("Terminate".into()),
        recycle_interval: Duration::from_secs(29 * 3600),
        recycle_at: vec![Duration::from_secs(3 * 3600)],
        private_memory_limit_kb: 0,
        virtual_memory_limit_kb: 0,
        queue_length: 1000,
        enable_32bit: true,
        rapid_fail_protection: true,
        rapid_fail_max_crashes: 5,
        rapid_fail_interval: Duration::from_secs(300),
        max_processes: 1,
        load_user_profile: true,
        worker_process_ids: vec![],
    }
}

fn snapshot() -> ProjectSnapshot {
    let dir = std::env::temp_dir();
    let runtime = IisSiteRuntime {
        id: 4,
        state: "Started".into(),
        app_pool: "shop".into(),
        app_pool_state: Some("Started".into()),
        worker_process_ids: vec![],
        bindings: vec![],
        physical_path: dir.clone(),
    };
    let mut project = ProjectState::from_site("shop".into(), runtime);
    project.dnn_version = Some("10.0.1".into());
    let mut https = IisBindingDetails::parse("https", "*:443:shop.dnndev.me", true);
    https.certificate_hash = Some("AB12".into());
    https.certificate_store = Some("My".into());
    ProjectSnapshot {
        project,
        iis: Some(IisSiteDetails {
            id: 4,
            name: "shop".into(),
            state: "Started".into(),
            physical_path: dir.clone(),
            config_path: "MACHINE/WEBROOT/APPHOST/shop".into(),
            server_auto_start: true,
            preload_enabled: Some(false),
            bindings: vec![IisBindingDetails::parse("http", "*:80:shop.dnndev.me", false), https],
            pool: Some(pool()),
        }),
        config: WebConfigInspection {
            path: dir.join("web.config"),
            has_dnn_section: true,
            machine_key: Some("fixed keys (SHA1 / AES)".into()),
            max_request_length_kb: Some(29_297),
            ..Default::default()
        },
        folders: vec![],
        assemblies: Default::default(),
        dnn_product: Some("DNN Platform".into()),
        connection: Some(DatabaseConnection {
            kind: DatabaseKind::SqlServer,
            server: r".\SQL".into(),
            database: "shop".into(),
            windows_authentication: false,
            user: "dnn".into(),
            password: "secret".into(),
        }),
        database: None,
        database_read: false,
        record: None,
        folder: FolderFacts::default(),
    }
}

fn row<'a>(sections: &'a [Section], title: &str, label: &str) -> Option<&'a crate::model::Row> {
    sections.iter().find(|s| s.title == title)?.rows.iter().find(|r| r.label == label)
}

#[test]
fn general_tab_names_the_project_and_its_health() {
    let s = snapshot();
    let found = issues(&s);
    let general = tab(&s, TabKind::General, &found);
    assert_eq!(general[0].source.as_deref(), Some("the IIS site, its folder and bin"));
    assert_eq!(row(&general, "Project", "DNN version").unwrap().value, "10.0.1 (DNN Platform)");
    assert_eq!(row(&general, "Project", "Type").unwrap().value, "DNN site");
    assert_eq!(row(&general, "Project", "Backups").unwrap().value, "none");
    let health = &general[1];
    assert_eq!(health.note.as_deref(), Some("Reading the database…"));
    assert_eq!(
        row(&general, "Project health", "Database configuration").unwrap().value,
        r"[shop] on .\SQL - SQL Server login 'dnn'"
    );
    let detected = row(&general, "Project health", "Detected issues").unwrap();
    assert_eq!((detected.value.as_str(), detected.health), ("1 problem, 2 warnings", Health::Bad));
    assert!(!format!("{general:?}").contains("secret"), "the password is never shown");
}

#[test]
fn issues_name_what_was_detected_problems_first() {
    let texts: Vec<String> = issues(&snapshot()).into_iter().map(|i| format!("{} | {}", i.area, i.text)).collect();
    assert_eq!(
        texts,
        [
            "IIS | https binding shop.dnndev.me:443 uses certificate AB12, which isn't in the My store.",
            "IIS | Application pool 'shop' runs 32-bit worker processes (enable32BitAppOnWin64).",
            "IIS | Application pool 'shop' uses the Classic pipeline - DNN runs in Integrated mode.",
        ]
    );
}

#[test]
fn iis_tab_shows_the_site_bindings_and_pool() {
    let s = snapshot();
    let iis = tab(&s, TabKind::Iis, &[]);
    let titles: Vec<&str> = iis.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Site", "Bindings", "Application pool", "Worker processes"]);
    let table = iis[1].table.as_ref().unwrap();
    assert_eq!(table.rows[0].cells, ["http", "http://shop.dnndev.me", "80", "all", "-", "-", "-"]);
    assert_eq!(table.rows[1].cells[4..], ["not found", "-", "yes"]);
    assert_eq!(table.rows[1].detail.as_deref(), Some("Certificate AB12 isn't in the My store"));
    assert_eq!(row(&iis, "Application pool", "Regular recycling").unwrap().value, "every 1 d 5 h; at 03:00");
    assert_eq!(row(&iis, "Application pool", "Idle time-out").unwrap().value, "20 min - then Terminate");
    assert_eq!(row(&iis, "Application pool", "Pipeline mode").unwrap().health, Health::Warning);
    assert_eq!(iis[3].note.as_deref(), Some("No worker process - IIS starts one on the site's first request."));

    let mut without = snapshot();
    without.iis = None;
    let iis = tab(&without, TabKind::Iis, &[]);
    assert_eq!(
        iis[0].note.as_deref(),
        Some("IIS's configuration couldn't be read - DNN Manager reads it with administrator rights.")
    );
}

#[test]
fn the_database_read_fills_dnn_and_database_tabs() {
    let dnn = DnnDatabaseFacts {
        version: Some("10.0.0".into()),
        installed: Some(LocalDateTime::new(2025, 1, 2, 10, 0, 0)),
        upgraded: Some(LocalDateTime::new(2026, 3, 4, 10, 0, 0)),
        portals: vec![DnnPortalDetails {
            id: 0,
            name: "My Website".into(),
            primary_alias: Some("shop.dnndev.me".into()),
            other_aliases: vec![("old.dnndev.me:8080".into(), Some("nl-NL".into()))],
            ..Default::default()
        }],
        schedule_items: Some(12),
        schedule_enabled: Some(9),
        host_settings: [
            ("SchedulerMode".to_string(), "0".to_string()),
            ("MaxUploadSize".to_string(), "10485760".to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let inspection = DatabaseInspection {
        server: Some(SqlServerFacts {
            version: Some("16.0.1000.6".into()),
            level: Some("RTM".into()),
            collation: Some("Latin1_General_CI_AS".into()),
            major_version: Some(16),
            ..Default::default()
        }),
        database: Some(DatabaseProperties {
            name: "shop".into(),
            state: Some("ONLINE".into()),
            compatibility_level: Some(100),
            collation: Some("SQL_Latin1_General_CP1_CI_AS".into()),
            ..Default::default()
        }),
        dnn: Some(dnn),
        ..Default::default()
    };
    let s = snapshot().with_database(inspection);
    let all = tabs(&s);
    let dnn_tab = &all[2].sections;
    assert_eq!(row(dnn_tab, "Installation", "Scheduler").unwrap().value, "disabled");
    assert_eq!(row(dnn_tab, "Installation", "Scheduler").unwrap().detail.as_deref(), Some("12 jobs, 9 enabled."));
    assert!(row(dnn_tab, "Installation", "Last upgrade").is_some());
    let portals = dnn_tab.iter().find(|s| s.title == "Portals").unwrap().table.as_ref().unwrap();
    assert_eq!(portals.rows[0].cells[2], "https://shop.dnndev.me");
    assert_eq!(portals.rows[0].cells[3], "old.dnndev.me:8080 (nl-NL)");
    assert_eq!(portals.rows[0].detail.as_deref(), Some("No IIS binding for old.dnndev.me:8080"));

    let database = &all[3].sections;
    assert_eq!(row(database, "SQL Server", "Version").unwrap().value, "16.0.1000.6 RTM - SQL Server 2022");
    assert_eq!(row(database, "Database", "Compatibility level").unwrap().value, "100 (SQL Server 2008)");
    assert_eq!(row(database, "DNN database", "DNN version").unwrap().health, Health::Bad);

    let found = issues(&s);
    assert!(
        found
            .iter()
            .any(|i| i.text == "DNN's files are 10.0.1, its database 10.0.0 - an upgrade didn't run or didn't finish.")
    );
    assert!(found.iter().any(|i| i.text
        == "Portal alias old.dnndev.me:8080 (portal 0) has no IIS binding - its requests don't reach this site."));

    let advanced = &all[4].sections;
    let limit = row(advanced, "Limits", "Effective upload limit").unwrap();
    assert_eq!((limit.value.as_str(), limit.health), ("10.0 MB", Health::Warning));
    assert_eq!(
        row(advanced, "Limits", "DNN upload limit").unwrap().detail.as_deref(),
        Some("Host setting MaxUploadSize.")
    );
    assert_eq!(row(advanced, "Configuration", "URL rewrite rules").unwrap().value, "none");
}

#[test]
fn without_a_connection_the_database_tab_says_why() {
    let mut s = snapshot();
    s.connection = None;
    s.project.database_problem = Some("the site's folder has no web.config".into());
    let database = tab(&s, TabKind::Database, &[]);
    assert_eq!(database.len(), 1);
    assert_eq!(database[0].note.as_deref(), Some("The database isn't known: the site's folder has no web.config."));
    let mut plain = snapshot();
    plain.project.dnn_version = None;
    assert_eq!(
        tab(&plain, TabKind::Dnn, &[])[0].note.as_deref(),
        Some(r"No DNN here - the site's folder has no bin\DotNetNuke.dll.")
    );
}

#[test]
fn versions_compare_as_dotnet_does() {
    assert!(same_version(Some("10.0.1"), Some("10.0.1.0")));
    assert!(same_version(Some("9.13"), Some("9.13.0")));
    assert!(!same_version(Some("10.0.1"), Some("10.0.0")));
    assert!(same_version(None, Some("1.0")));
    assert!(same_version(Some("x"), Some("x")));
}
