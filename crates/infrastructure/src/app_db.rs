//! `dnnmanager.db` (`AppDatabase`), shared with the WPF app: the same tables, the same rows, the same
//! `PRAGMA user_version` count of schema steps. This build reads and writes what it uses and leaves every other row
//! as it is; it never changes the schema of a database made by a newer build.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::records::{InstallMode, ProjectRecord, ProjectRecords};
use dnnm_application::settings::{Settings, SettingsStore};
use rusqlite::{Connection, OpenFlags, params};

/// The schema steps the WPF app had run when this port was made (`AppDatabase.Steps`).
pub const SCHEMA_VERSION: i64 = 5;

/// The tables as the WPF app's five steps leave them, for a new database.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    site           TEXT NOT NULL PRIMARY KEY COLLATE NOCASE,
    install_mode   TEXT NOT NULL,
    created_utc    TEXT NOT NULL,
    dnn_version    TEXT,
    host_user_name TEXT);
CREATE TABLE IF NOT EXISTS dnn_releases (
    api        TEXT NOT NULL COLLATE NOCASE,
    position   INTEGER NOT NULL,
    version    TEXT NOT NULL,
    tag        TEXT NOT NULL,
    url        TEXT NOT NULL,
    prerelease INTEGER NOT NULL,
    saved_utc  TEXT NOT NULL, sha256 TEXT, upgrade_sha256 TEXT,
    PRIMARY KEY (api, position));
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS state (
    area  TEXT NOT NULL COLLATE NOCASE,
    key   TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (area, key));
CREATE TABLE IF NOT EXISTS keep_warm (site TEXT NOT NULL PRIMARY KEY COLLATE NOCASE);
"#;

/// The database file. Each call opens its own short-lived connection: the WPF app may have it open too.
#[derive(Debug, Clone)]
pub struct AppDatabase {
    path: PathBuf,
}

impl AppDatabase {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Result<Connection, String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(&self.path, flags).map_err(|e| e.to_string())?;
        connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if version == 0 {
            // A new database: the tables as the WPF app would make them, so it can open it later.
            connection
                .execute_batch(&format!("BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"))
                .map_err(|e| e.to_string())?;
        }
        Ok(connection)
    }

    /// The sites kept warm.
    pub fn keep_warm_sites(&self) -> Result<Vec<String>, String> {
        let connection = self.open()?;
        let mut statement =
            connection.prepare("SELECT site FROM keep_warm ORDER BY site").map_err(|e| e.to_string())?;
        let rows = statement.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn set_keep_warm(&self, site: &str, on: bool) -> Result<(), String> {
        let connection = self.open()?;
        let sql = if on {
            "INSERT OR IGNORE INTO keep_warm (site) VALUES (?1)"
        } else {
            "DELETE FROM keep_warm WHERE site = ?1"
        };
        connection.execute(sql, params![site]).map(|_| ()).map_err(|e| e.to_string())
    }

    /// One value of the workspace (`state` area and key).
    pub fn state(&self, area: &str, key: &str) -> Result<Option<String>, String> {
        let connection = self.open()?;
        let mut statement =
            connection.prepare("SELECT value FROM state WHERE area = ?1 AND key = ?2").map_err(|e| e.to_string())?;
        let mut rows = statement.query(params![area, key]).map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => row.get(0).map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    /// Removes one value of the workspace - a value that is null in the WPF app has no row.
    pub fn delete_state(&self, area: &str, key: &str) -> Result<(), String> {
        let connection = self.open()?;
        connection
            .execute("DELETE FROM state WHERE area = ?1 AND key = ?2", params![area, key])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn set_state(&self, area: &str, key: &str, value: &str) -> Result<(), String> {
        let connection = self.open()?;
        connection
            .execute(
                "INSERT INTO state (area, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (area, key) DO UPDATE SET value = excluded.value",
                params![area, key, value],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The `projects` table (`ProjectRecords`), over the database file.
#[derive(Debug, Clone)]
pub struct ProjectTable(pub AppDatabase);

impl ProjectRecords for ProjectTable {
    fn find(&self, site: &str) -> Option<ProjectRecord> {
        let read = || -> Result<Option<ProjectRecord>, String> {
            let connection = self.0.open()?;
            let mut statement = connection
                .prepare("SELECT site, install_mode, created_utc, dnn_version, host_user_name FROM projects WHERE site = ?1")
                .map_err(|e| e.to_string())?;
            let mut rows = statement.query(params![site]).map_err(|e| e.to_string())?;
            let Some(row) = rows.next().map_err(|e| e.to_string())? else { return Ok(None) };
            let mode: String = row.get(1).map_err(|e| e.to_string())?;
            Ok(Some(ProjectRecord {
                site: row.get(0).map_err(|e| e.to_string())?,
                install_mode: InstallMode::parse(&mode).ok_or_else(|| format!("unknown install mode {mode}"))?,
                created_utc: row.get(2).map_err(|e| e.to_string())?,
                dnn_version: row.get(3).map_err(|e| e.to_string())?,
                host_user_name: row.get(4).map_err(|e| e.to_string())?,
            }))
        };
        read().unwrap_or_else(|e| {
            log::warn!("Could not read the record of {site}: {e}");
            None
        })
    }

    fn save(&self, record: &ProjectRecord) {
        let write = || -> Result<(), String> {
            self.0.open()?
                .execute(
                    "INSERT OR REPLACE INTO projects (site, install_mode, created_utc, dnn_version, host_user_name)                      VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        record.site,
                        record.install_mode.as_str(),
                        record.created_utc,
                        record.dnn_version,
                        record.host_user_name
                    ],
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        if let Err(e) = write() {
            log::warn!("Could not save the record of {}: {e}", record.site);
        }
    }

    fn remove(&self, site: &str) {
        let delete = || -> Result<(), String> {
            self.0.open()?
                .execute("DELETE FROM projects WHERE site = ?1", params![site])
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        if let Err(e) = delete() {
            log::warn!("Could not remove the record of {site}: {e}");
        }
    }
}

impl SettingsStore for AppDatabase {
    fn load(&self) -> Result<Settings, String> {
        let connection = self.open()?;
        let mut statement = connection.prepare("SELECT key, value FROM settings").map_err(|e| e.to_string())?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        Ok(Settings::from_rows(rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?))
    }

    fn save(&self, before: &Settings, after: &Settings) -> Result<(), String> {
        if after.is_newer_than_build() {
            return Err("The settings were saved by a newer DNN Manager - this one doesn't change them.".into());
        }
        let mut connection = self.open()?;
        let tx = connection.transaction().map_err(|e| e.to_string())?;
        for (key, value) in after.rows() {
            if before.rows().get(key) != Some(value) {
                tx.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        for key in before.rows().keys().filter(|k| !after.rows().contains_key(*k)) {
            tx.execute("DELETE FROM settings WHERE key = ?1", params![key]).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_database_gets_the_wpf_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        assert!(db.load().unwrap().rows().is_empty());
        let connection = Connection::open(db.path()).unwrap();
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn saves_only_what_changed_and_keeps_other_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        let mut first = db.load().unwrap();
        first.set("unknown.toThisBuild", "kept");
        first.set("appearance.theme", "light");
        db.save(&Settings::default(), &first).unwrap();

        let before = db.load().unwrap();
        let mut after = before.clone();
        after.set_theme(dnnm_application::settings::ThemeChoice::Dark);
        db.save(&before, &after).unwrap();

        let reread = db.load().unwrap();
        assert_eq!(reread.get("appearance.theme"), Some("dark"));
        assert_eq!(reread.get("unknown.toThisBuild"), Some("kept"));
    }

    #[test]
    fn refuses_to_overwrite_newer_settings() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        let mut newer = Settings::default();
        newer.set("version", "99");
        assert!(db.save(&Settings::default(), &newer).is_err());
    }

    #[test]
    fn project_records_round_trip_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let db = ProjectTable(AppDatabase::new(dir.path().join("dnnmanager.db")));
        let record = ProjectRecord {
            site: "test-bond".into(),
            install_mode: InstallMode::Automatic,
            created_utc: "2026-11-13T14:44:35.5371802Z".into(),
            dnn_version: Some("10.4.1".into()),
            host_user_name: Some("host".into()),
        };
        db.save(&record);
        assert_eq!(db.find("TEST-BOND"), Some(record));
        db.remove("test-bond");
        assert_eq!(db.find("test-bond"), None);
    }

    #[test]
    fn keep_warm_and_state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        db.set_keep_warm("dnn10", true).unwrap();
        db.set_keep_warm("DNN10", true).unwrap();
        assert_eq!(db.keep_warm_sites().unwrap(), vec!["dnn10"]);
        db.set_keep_warm("Dnn10", false).unwrap();
        assert!(db.keep_warm_sites().unwrap().is_empty());
        db.set_state("layout", "sidebarWidth", "230").unwrap();
        assert_eq!(db.state("Layout", "sidebarWidth").unwrap().as_deref(), Some("230"));
    }
}
