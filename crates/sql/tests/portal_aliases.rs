//! Portal aliases against a real SQL Server - a site brought here and exported for the live server again. Runs only
//! when `DNNM_SQL_ALIAS_TEST` names a throwaway SQL Server container as `container:port:sa-password`; it works in a
//! database of its own (`dnnm_alias_test`), made and dropped here. Never point it at DNN Manager's own container:
//!
//! ```text
//! docker run -d --name dnnm-alias-test -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD='Test_12345!' -p 127.0.0.1:14339:1433 \
//!     mcr.microsoft.com/mssql/server:2022-latest
//! DNNM_SQL_ALIAS_TEST='dnnm-alias-test:14339:Test_12345!' cargo test -p dnnm-sql --test portal_aliases -- --nocapture
//! docker rm -f dnnm-alias-test
//! ```

use std::collections::HashMap;

use dnnm_application::operation::CancelToken;
use dnnm_application::use_cases::portal_aliases;
use dnnm_sql::{DatabaseConnection, DatabaseKind, DockerOptions, SqlAuthentication};
use dnnm_sql::container::SqlServerService;
use dnnm_sql::provisioner::DatabaseProvisioner;

const DB: &str = "dnnm_alias_test";

/// A DNN 10 shaped PortalAlias, Portals and PortalSettings, with the portals of a live site of six portals.
const SCHEMA: &str = "
CREATE TABLE dbo.Portals (PortalID int NOT NULL PRIMARY KEY);
CREATE TABLE dbo.PortalAlias (
    PortalAliasID int IDENTITY(1,1) NOT NULL PRIMARY KEY, PortalID int NOT NULL, HTTPAlias nvarchar(200) NOT NULL,
    CultureCode nvarchar(10) NULL, Skin nvarchar(100) NULL, BrowserType nvarchar(10) NULL, IsPrimary bit NOT NULL DEFAULT 0,
    CreatedByUserID int NULL, CreatedOnDate datetime NULL, LastModifiedByUserID int NULL, LastModifiedOnDate datetime NULL);
CREATE UNIQUE INDEX IX_PortalAlias ON dbo.PortalAlias (HTTPAlias);
CREATE TABLE dbo.PortalSettings (
    PortalSettingID int IDENTITY(1,1) NOT NULL PRIMARY KEY, PortalID int NOT NULL, SettingName nvarchar(50) NOT NULL,
    SettingValue nvarchar(max) NULL, CreatedByUserID int NULL, CreatedOnDate datetime NULL, LastModifiedByUserID int NULL,
    LastModifiedOnDate datetime NULL, CultureCode nvarchar(10) NULL);
INSERT INTO dbo.Portals VALUES (0), (1), (2), (3), (4), (5);
INSERT INTO dbo.PortalAlias (PortalID, HTTPAlias, IsPrimary) VALUES
    (0, N'goededoelen.bondtest.nl', 1), (0, N'community.bondtest.nl', 0),
    (1, N'projectflip.bondtest.nl', 1), (2, N'sngv.bondtest.nl', 1),
    (4, N'grootvlaardingsdictee.bondtest.nl', 1), (4, N'grootvlaardingsdictee.dnndev.me', 0),
    (4, N'goededoelen.dnndev.me/grootvlaardingsdictee', 0),
    (5, N'rotaryvlaardingen.bondtest.nl', 0), (5, N'www.rotary-vlaardingen.nl', 1);
";

/// Each portal's aliases, `*` before the primary one, sorted.
fn aliases(db: &DatabaseConnection) -> HashMap<i32, Vec<String>> {
    let (rows, _) = DatabaseProvisioner.portal_aliases(db, None).unwrap();
    let mut out: HashMap<i32, Vec<String>> = HashMap::new();
    for r in rows {
        out.entry(r.portal).or_default().push(format!("{}{}", if r.primary { "*" } else { "" }, r.alias));
    }
    out.values_mut().for_each(|v| v.sort());
    out
}

#[test]
fn a_live_site_brought_here_and_exported_again() {
    let Ok(spec) = std::env::var("DNNM_SQL_ALIAS_TEST") else {
        eprintln!("DNNM_SQL_ALIAS_TEST isn't set - skipped");
        return;
    };
    let mut parts = spec.splitn(3, ':');
    let (container, port, password) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
    assert_ne!(container, "dnn-sqlserver", "a throwaway container only");
    let service = SqlServerService::new(DockerOptions {
        container_name: container.into(),
        sa_password: password.into(),
        default_port: port.parse().unwrap(),
        ..DockerOptions::default()
    });
    let mut db = DatabaseConnection::new(DatabaseKind::SqlServer, &format!("localhost,{port}"), DB, SqlAuthentication::Sql);
    db.user = "sa".into();
    db.password = password.into();
    let cancel = CancelToken::new();
    let _ = service.drop_database(DB, &cancel);
    // The test's own database, through sqlcmd in the throwaway container.
    let sqlcmd = |database: &str, sql: &str| {
        let out = std::process::Command::new("docker")
            .args(["exec", container, "/opt/mssql-tools18/bin/sqlcmd", "-C", "-b", "-S", "localhost", "-U", "sa"])
            .args(["-P", password, "-d", database, "-Q", sql])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    };
    sqlcmd("master", &format!("CREATE DATABASE [{DB}];"));
    sqlcmd(DB, SCHEMA);

    // Brought here at goededoelen.dnndev.me: every portal gets its own address, primary; the live ones stay.
    let plan = service.remap_portal_aliases(DB, "dnndev.me", "goededoelen.dnndev.me", &cancel).unwrap();
    assert_eq!(plan.len(), 6);
    let here = aliases(&db);
    println!("here: {here:#?}");
    assert_eq!(here[&0], ["*goededoelen.dnndev.me", "community.bondtest.nl", "goededoelen.bondtest.nl"]);
    assert_eq!(here[&1], ["*projectflip.goededoelen.dnndev.me", "projectflip.bondtest.nl"]);
    assert_eq!(here[&3], ["*portal3.goededoelen.dnndev.me"]);
    assert_eq!(
        here[&4],
        [
            "*grootvlaardingsdictee.goededoelen.dnndev.me",
            "goededoelen.dnndev.me/grootvlaardingsdictee",
            "grootvlaardingsdictee.bondtest.nl"
        ]
    );
    assert_eq!(here[&5], ["*rotary-vlaardingen.goededoelen.dnndev.me", "rotaryvlaardingen.bondtest.nl", "www.rotary-vlaardingen.nl"]);
    let (_, noted) = DatabaseProvisioner.portal_aliases(&db, None).unwrap();
    assert_eq!(noted[&5], "www.rotary-vlaardingen.nl");
    assert_eq!(noted.len(), 5, "portal 3 had no live alias");

    // Brought here again, as a clone on another address: the local ones move, the noted live ones stay noted.
    service.remap_portal_aliases(DB, "dnndev.me", "copy.dnndev.me:8080", &cancel).unwrap();
    let copy = aliases(&db);
    assert_eq!(copy[&1], ["*projectflip.copy.dnndev.me:8080", "projectflip.bondtest.nl"]);
    assert_eq!(DatabaseProvisioner.portal_aliases(&db, None).unwrap().1, noted);

    // Exported for the live server: the local aliases go, the live primary ones are primary, the notes are cleared.
    let (rows, noted) = DatabaseProvisioner.portal_aliases(&db, None).unwrap();
    let plan = portal_aliases::delocalize(&rows, &noted, "dnndev.me", &["copy.dnndev.me".into()]);
    DatabaseProvisioner.apply_live_aliases(&db, &plan, None).unwrap();
    let live = aliases(&db);
    println!("live: {live:#?}");
    assert_eq!(live[&0], ["*goededoelen.bondtest.nl", "community.bondtest.nl"]);
    assert_eq!(live[&1], ["*projectflip.bondtest.nl"]);
    assert!(!live.contains_key(&3), "made here: no live alias");
    assert_eq!(live[&4], ["*grootvlaardingsdictee.bondtest.nl"]);
    assert_eq!(live[&5], ["*www.rotary-vlaardingen.nl", "rotaryvlaardingen.bondtest.nl"]);
    assert!(DatabaseProvisioner.portal_aliases(&db, None).unwrap().1.is_empty());

    service.drop_database(DB, &cancel).unwrap();
}
