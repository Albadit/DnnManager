//! The sites kept warm: a row each in the `keep_warm` table of `dnnmanager.db` - `KeepWarmRecords.cs`. Written the
//! moment one is switched on or off, so the next start keeps the same sites warm. When the database can't be read, no
//! site counts as kept warm.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};

/// The `keep_warm` table of the database at a path (the app's `dnnmanager.db`; a test's own file).
#[derive(Debug, Clone)]
pub struct KeepWarmRecords {
    path: PathBuf,
}

impl KeepWarmRecords {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The site's name as recorded when `site` is kept warm (whatever the case of its name); None when it isn't.
    pub fn find(&self, site: &str) -> Option<String> {
        self.read("SELECT site FROM keep_warm WHERE site = ?1", Some(site)).into_iter().next()
    }

    /// The sites kept warm, by name.
    pub fn list(&self) -> Vec<String> {
        self.read("SELECT site FROM keep_warm ORDER BY site", None)
    }

    /// Switches `site` on (a row) or off (no row).
    pub fn save(&self, site: &str, enabled: bool) {
        if enabled {
            self.write("INSERT OR IGNORE INTO keep_warm (site) VALUES (?1)", site);
        } else {
            self.remove(site);
        }
    }

    pub fn remove(&self, site: &str) {
        self.write("DELETE FROM keep_warm WHERE site = ?1", site);
    }

    // The app creates and upgrades the database (infrastructure's app_db); this only opens it - a missing file is an
    // error, not a new empty database. A short busy timeout: the WPF app may have it open.
    fn open(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        Ok(connection)
    }

    fn read(&self, sql: &str, site: Option<&str>) -> Vec<String> {
        let result = (|| {
            let connection = self.open()?;
            let mut statement = connection.prepare(sql)?;
            let rows: rusqlite::Result<Vec<String>> = match site {
                Some(site) => statement.query_map(params![site], |r| r.get::<_, String>(0))?.collect(),
                None => statement.query_map([], |r| r.get::<_, String>(0))?.collect(),
            };
            rows
        })();
        result.unwrap_or_else(|e: rusqlite::Error| {
            log::warn!("Could not read the sites kept warm: {e}");
            Vec::new()
        })
    }

    fn write(&self, sql: &str, site: &str) {
        if let Err(e) = self.open().and_then(|c| c.execute(sql, params![site])) {
            log::warn!("Could not save whether {site} is kept warm: {e}");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A database with the `keep_warm` table as the app creates it, in a temporary folder.
    pub(crate) fn database(dir: &tempfile::TempDir) -> KeepWarmRecords {
        let path = dir.path().join("dnnmanager.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE IF NOT EXISTS keep_warm (site TEXT NOT NULL PRIMARY KEY COLLATE NOCASE);")
            .unwrap();
        KeepWarmRecords::new(path)
    }

    #[test]
    fn records_are_the_sites_kept_warm() {
        let dir = tempfile::tempdir().unwrap();
        assert!(KeepWarmRecords::new(dir.path().join("none.db")).list().is_empty(), "No database: none.");
        assert!(!dir.path().join("none.db").exists(), "…and none made.");

        let records = database(&dir);
        assert_eq!(records.find("shop"), None);
        records.save("shop", true);
        records.save("blog", true);
        records.save("SHOP", true);
        assert_eq!(records.find("SHOP").as_deref(), Some("shop"), "Found whatever the case of its name.");
        assert_eq!(records.list(), vec!["blog", "shop"]);

        records.save("blog", false);
        assert_eq!(records.find("blog"), None, "Switched off: no row.");
        assert_eq!(
            KeepWarmRecords::new(records.path()).find("shop").as_deref(),
            Some("shop"),
            "Kept for the next start."
        );
        records.remove("Shop");
        assert!(records.list().is_empty());
    }
}
