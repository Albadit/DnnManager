//! Source Control's local side: the project's folder ([`ProjectFiles`]) and what is kept in `dnnmanager.db`
//! ([`SourceControlStore`]): each project's environments, each file's record of its last sync, and the history.
//!
//! The three tables are this build's own (`sc_…`), made when first used; the WPF app never had them.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dnnm_application::operation::CancelToken;
use dnnm_application::source_control::model::{
    Auth, Environment, EnvironmentKind, FileMeta, HistoryLine, Mapping, Protocol, SyncRecord,
};
use dnnm_application::source_control::paths::{IgnoreRules, clean};
use dnnm_application::source_control::sync::{Listing, LocalFiles, SyncState};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::app_db::AppDatabase;

// ─── The project's folder ────────────────────────────────────────────────────

/// The project's files on this computer.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProjectFiles;

fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

fn meta_of(metadata: &fs::Metadata) -> FileMeta {
    FileMeta::new(metadata.len(), metadata.modified().map(unix_seconds).unwrap_or(0))
}

/// `root` and a project path, refused when the path would leave the folder.
fn full_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let clean = clean(path).filter(|p| !p.is_empty()).ok_or_else(|| format!("'{path}' isn't a path in the project"))?;
    Ok(clean.split('/').fold(root.to_path_buf(), |p, part| p.join(part)))
}

impl LocalFiles for ProjectFiles {
    fn scan(
        &self,
        root: &Path,
        folder: &str,
        ignore: &IgnoreRules,
        cancel: &CancelToken,
    ) -> Result<Vec<(String, FileMeta)>, String> {
        let start = if folder.is_empty() { root.to_path_buf() } else { full_path(root, folder)? };
        let mut found = Vec::new();
        let mut pending = vec![(start, folder.trim_matches('/').to_string())];
        while let Some((dir, relative)) = pending.pop() {
            cancel.check()?;
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                // The mapping's folder isn't made yet: nothing there.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", dir.display())),
            };
            for entry in entries.flatten() {
                let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
                // Links (and junctions) are left alone: they could lead anywhere, even back here.
                if meta.file_type().is_symlink() || crate::web_config::has_link(&dir, &entry.path()) {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
                let path = if relative.is_empty() { name } else { format!("{relative}/{name}") };
                if meta.is_dir() {
                    if !ignore.matches_folder(&path) {
                        pending.push((entry.path(), path));
                    }
                } else if meta.is_file() && !ignore.matches(&path) {
                    found.push((path, meta_of(&meta)));
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(found)
    }

    fn stat(&self, root: &Path, path: &str) -> Option<FileMeta> {
        let meta = fs::metadata(full_path(root, path).ok()?).ok()?;
        meta.is_file().then(|| meta_of(&meta))
    }

    fn hash(&self, root: &Path, path: &str) -> Result<String, String> {
        let mut file = fs::File::open(full_path(root, path)?).map_err(|e| format!("{path}: {e}"))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|e| format!("{path}: {e}"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    fn open(&self, root: &Path, path: &str) -> Result<Box<dyn Read + Send>, String> {
        let file = fs::File::open(full_path(root, path)?).map_err(|e| format!("{path}: {e}"))?;
        Ok(Box::new(std::io::BufReader::new(file)))
    }

    fn write(
        &self,
        root: &Path,
        path: &str,
        mtime: i64,
        fill: &mut dyn FnMut(&mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        let target = full_path(root, path)?;
        let folder = target.parent().ok_or_else(|| format!("{path} has no folder"))?;
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let partial = folder.join(format!(".{name}.dnnm-part"));
        let written = (|| {
            let mut file = fs::File::create(&partial).map_err(|e| format!("{path}: {e}"))?;
            {
                let mut writer = std::io::BufWriter::new(&mut file);
                fill(&mut writer)?;
                writer.flush().map_err(|e| format!("{path}: {e}"))?;
            }
            if mtime > 0 {
                let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime as u64));
            }
            file.sync_all().map_err(|e| format!("{path}: {e}"))?;
            drop(file);
            // Put in place only once it is all there.
            fs::rename(&partial, &target).map_err(|e| format!("{path}: {e}"))
        })();
        if written.is_err() {
            let _ = fs::remove_file(&partial);
        }
        written
    }
}

// ─── Watching the project ────────────────────────────────────────────────────

/// Tells when files change in a project's folder; dropped, it stops.
pub struct ProjectWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Whether a changed path matters: not inside `.git` or `.vs`, nor one of the partial files a transfer writes.
pub fn change_matters(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    !name.ends_with(".dnnm-part")
        && !path.components().any(|c| matches!(c.as_os_str().to_str(), Some(".git" | ".vs" | "node_modules")))
}

/// Watches `folder` and everything in it; `changed` is called (on the watcher's thread) for every change that
/// [`change_matters`] - often several for one save: the caller waits for a quiet moment.
pub fn watch(folder: &Path, changed: impl Fn() + Send + 'static) -> Result<ProjectWatcher, String> {
    use notify::Watcher;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && event.paths.iter().any(|p| change_matters(p))
        {
            changed();
        }
    })
    .map_err(|e| format!("The project's folder can't be watched: {e}"))?;
    watcher
        .watch(folder, notify::RecursiveMode::Recursive)
        .map_err(|e| format!("{} can't be watched: {e}", folder.display()))?;
    Ok(ProjectWatcher { _watcher: watcher })
}

// ─── dnnmanager.db ─────────────────────────────────────────────────────────

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sc_environments (
    id            TEXT NOT NULL PRIMARY KEY,
    project       TEXT NOT NULL COLLATE NOCASE,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    host          TEXT NOT NULL,
    port          INTEGER NOT NULL,
    user          TEXT NOT NULL,
    key_file      TEXT,
    remote_root   TEXT NOT NULL,
    mappings      TEXT NOT NULL,
    ignore        TEXT NOT NULL,
    protected     TEXT NOT NULL,
    backup_remote INTEGER NOT NULL,
    host_key      TEXT,
    protocol      TEXT NOT NULL DEFAULT 'sftp');
CREATE TABLE IF NOT EXISTS sc_records (
    env          TEXT NOT NULL,
    path         TEXT NOT NULL COLLATE NOCASE,
    local_size   INTEGER NOT NULL,
    local_mtime  INTEGER NOT NULL,
    local_hash   TEXT,
    remote_size  INTEGER NOT NULL,
    remote_mtime INTEGER NOT NULL,
    remote_hash  TEXT,
    synced       INTEGER NOT NULL,
    PRIMARY KEY (env, path));
CREATE TABLE IF NOT EXISTS sc_history (
    env     TEXT NOT NULL,
    at      INTEGER NOT NULL,
    action  TEXT NOT NULL,
    path    TEXT NOT NULL,
    outcome TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS sc_history_env ON sc_history (env, at);
CREATE TABLE IF NOT EXISTS sc_listing (
    env   TEXT NOT NULL,
    path  TEXT NOT NULL COLLATE NOCASE,
    size  INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    PRIMARY KEY (env, path));
CREATE TABLE IF NOT EXISTS sc_listed (
    env TEXT NOT NULL PRIMARY KEY,
    at  INTEGER NOT NULL);
"#;

/// Source Control's tables in `dnnmanager.db`.
#[derive(Debug, Clone)]
pub struct SourceControlStore {
    db: AppDatabase,
}

/// Lines of text kept in one column: one item a line.
fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn encode_mappings(mappings: &[Mapping]) -> String {
    mappings.iter().map(|m| format!("{}\t{}", m.local, m.remote)).collect::<Vec<_>>().join("\n")
}

fn decode_mappings(text: &str) -> Vec<Mapping> {
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(local, remote)| Mapping { local: local.into(), remote: remote.into() })
        .collect()
}

impl SourceControlStore {
    pub fn new(db: AppDatabase) -> Self {
        Self { db }
    }

    fn open(&self) -> Result<Connection, String> {
        let connection = self.db.connection()?;
        connection.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        // Made by the first Source Control build, before FTP: its servers were all SFTP.
        let has_protocol = connection
            .prepare("SELECT 1 FROM pragma_table_info('sc_environments') WHERE name = 'protocol'")
            .and_then(|mut s| s.exists([]))
            .map_err(|e| e.to_string())?;
        if !has_protocol {
            connection
                .execute_batch("ALTER TABLE sc_environments ADD COLUMN protocol TEXT NOT NULL DEFAULT 'sftp';")
                .map_err(|e| e.to_string())?;
        }
        Ok(connection)
    }

    /// The project's environments, Development first and Production last.
    pub fn environments(&self, project: &str) -> Result<Vec<Environment>, String> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(
                "SELECT id, project, name, kind, host, port, user, key_file, remote_root, mappings, ignore, protected, \
                 backup_remote, host_key, protocol FROM sc_environments WHERE project = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([project], |r| {
                let key_file: Option<String> = r.get(7)?;
                Ok(Environment {
                    id: r.get(0)?,
                    project: r.get(1)?,
                    name: r.get(2)?,
                    kind: EnvironmentKind::from_key(&r.get::<_, String>(3)?).unwrap_or(EnvironmentKind::Development),
                    protocol: Protocol::from_key(&r.get::<_, String>(14)?).unwrap_or(Protocol::Sftp),
                    host: r.get(4)?,
                    port: r.get::<_, i64>(5)?.clamp(1, 65535) as u16,
                    user: r.get(6)?,
                    auth: key_file.filter(|k| !k.is_empty()).map_or(Auth::Password, Auth::KeyFile),
                    remote_root: r.get(8)?,
                    mappings: decode_mappings(&r.get::<_, String>(9)?),
                    ignore: lines(&r.get::<_, String>(10)?),
                    protected: lines(&r.get::<_, String>(11)?),
                    backup_remote: r.get::<_, i64>(12)? != 0,
                    host_key: r.get(13)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut list: Vec<Environment> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        list.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(list)
    }

    /// Adds or changes an environment. A changed server (host, port or protocol) forgets the trusted key and the
    /// records: it is another server, or the same one reached another way.
    pub fn save_environment(&self, env: &Environment) -> Result<(), String> {
        let mut connection = self.open()?;
        let tx = connection.transaction().map_err(|e| e.to_string())?;
        let before: Option<(String, i64, String)> = tx
            .query_row("SELECT host, port, protocol FROM sc_environments WHERE id = ?1", [&env.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let moved = before.is_some_and(|(host, port, protocol)| {
            !host.eq_ignore_ascii_case(&env.host) || port != i64::from(env.port) || protocol != env.protocol.key()
        });
        let host_key = if moved { None } else { env.host_key.clone() };
        if moved {
            for table in ["sc_records", "sc_listing", "sc_listed"] {
                tx.execute(&format!("DELETE FROM {table} WHERE env = ?1"), [&env.id]).map_err(|e| e.to_string())?;
            }
        }
        let key_file = match &env.auth {
            Auth::Password => None,
            Auth::KeyFile(path) => Some(path.clone()),
        };
        tx.execute(
            "INSERT OR REPLACE INTO sc_environments (id, project, name, kind, host, port, user, key_file, remote_root, \
             mappings, ignore, protected, backup_remote, host_key, protocol) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
             ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                env.id,
                env.project,
                env.name,
                env.kind.key(),
                env.host,
                i64::from(env.port),
                env.user,
                key_file,
                env.remote_root,
                encode_mappings(&env.mappings),
                env.ignore.join("\n"),
                env.protected.join("\n"),
                i64::from(env.backup_remote),
                host_key,
                env.protocol.key(),
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Removes an environment with its records and history.
    pub fn delete_environment(&self, id: &str) -> Result<(), String> {
        let connection = self.open()?;
        for table in ["sc_records", "sc_history", "sc_listing", "sc_listed"] {
            connection.execute(&format!("DELETE FROM {table} WHERE env = ?1"), [id]).map_err(|e| e.to_string())?;
        }
        connection.execute("DELETE FROM sc_environments WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The environment's history, newest first, at most `limit` lines.
    pub fn history(&self, env: &str, limit: usize) -> Result<Vec<HistoryLine>, String> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare("SELECT at, action, path, outcome FROM sc_history WHERE env = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(params![env, limit as i64], |r| {
                Ok(HistoryLine { at: r.get(0)?, action: r.get(1)?, path: r.get(2)?, outcome: r.get(3)? })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
}

/// The history kept per environment: the newest lines.
const HISTORY_KEPT: i64 = 2000;

impl SyncState for SourceControlStore {
    fn records(&self, env: &str) -> Result<HashMap<String, SyncRecord>, String> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(
                "SELECT path, local_size, local_mtime, local_hash, remote_size, remote_mtime, remote_hash, synced \
                 FROM sc_records WHERE env = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([env], |r| {
                let meta = |size: i64, mtime: i64, hash: Option<String>| FileMeta { size: size.max(0) as u64, mtime, hash };
                Ok((
                    r.get::<_, String>(0)?,
                    SyncRecord {
                        local: meta(r.get(1)?, r.get(2)?, r.get(3)?),
                        remote: meta(r.get(4)?, r.get(5)?, r.get(6)?),
                        synced: r.get(7)?,
                    },
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    fn set_record(&self, env: &str, path: &str, record: &SyncRecord) -> Result<(), String> {
        self.open()?
            .execute(
                "INSERT OR REPLACE INTO sc_records (env, path, local_size, local_mtime, local_hash, remote_size, \
                 remote_mtime, remote_hash, synced) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    env,
                    path,
                    record.local.size as i64,
                    record.local.mtime,
                    record.local.hash,
                    record.remote.size as i64,
                    record.remote.mtime,
                    record.remote.hash,
                    record.synced,
                ],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn remove_record(&self, env: &str, path: &str) -> Result<(), String> {
        self.open()?
            .execute("DELETE FROM sc_records WHERE env = ?1 AND path = ?2", [env, path])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn add_history(&self, env: &str, line: &HistoryLine) -> Result<(), String> {
        let connection = self.open()?;
        connection
            .execute(
                "INSERT INTO sc_history (env, at, action, path, outcome) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![env, line.at, line.action, line.path, line.outcome],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute(
                "DELETE FROM sc_history WHERE env = ?1 AND rowid NOT IN \
                 (SELECT rowid FROM sc_history WHERE env = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2)",
                params![env, HISTORY_KEPT],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn trust_host_key(&self, env: &str, fingerprint: &str) -> Result<(), String> {
        self.open()?
            .execute("UPDATE sc_environments SET host_key = ?2 WHERE id = ?1", [env, fingerprint])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn save_listing(&self, env: &str, at: i64, files: &[(String, FileMeta)]) -> Result<(), String> {
        let mut connection = self.open()?;
        let tx = connection.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM sc_listing WHERE env = ?1", [env]).map_err(|e| e.to_string())?;
        {
            let mut insert = tx
                .prepare("INSERT OR REPLACE INTO sc_listing (env, path, size, mtime) VALUES (?1, ?2, ?3, ?4)")
                .map_err(|e| e.to_string())?;
            for (path, meta) in files {
                insert.execute(params![env, path, meta.size as i64, meta.mtime]).map_err(|e| e.to_string())?;
            }
        }
        tx.execute("INSERT OR REPLACE INTO sc_listed (env, at) VALUES (?1, ?2)", params![env, at])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn update_listing(&self, env: &str, path: &str, meta: Option<&FileMeta>) -> Result<(), String> {
        let connection = self.open()?;
        match meta {
            Some(m) => connection.execute(
                "INSERT OR REPLACE INTO sc_listing (env, path, size, mtime) VALUES (?1, ?2, ?3, ?4)",
                params![env, path, m.size as i64, m.mtime],
            ),
            None => connection.execute("DELETE FROM sc_listing WHERE env = ?1 AND path = ?2", [env, path]),
        }
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    fn listing(&self, env: &str) -> Result<Option<Listing>, String> {
        let connection = self.open()?;
        let at: Option<i64> = connection
            .query_row("SELECT at FROM sc_listed WHERE env = ?1", [env], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(at) = at else { return Ok(None) };
        let mut statement =
            connection.prepare("SELECT path, size, mtime FROM sc_listing WHERE env = ?1").map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([env], |r| Ok((r.get::<_, String>(0)?, FileMeta::new(r.get::<_, i64>(1)?.max(0) as u64, r.get(2)?))))
            .map_err(|e| e.to_string())?;
        Ok(Some((at, rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(id: &str, project: &str, kind: EnvironmentKind) -> Environment {
        Environment {
            id: id.into(),
            project: project.into(),
            name: kind.name().into(),
            kind,
            protocol: Protocol::Sftp,
            host: "example.com".into(),
            port: 22,
            user: "deploy".into(),
            auth: Auth::KeyFile("/home/me/.ssh/id_ed25519".into()),
            remote_root: "/var/www".into(),
            mappings: vec![Mapping { local: "DesktopModules/Shop".into(), remote: "/srv/shop".into() }],
            ignore: vec!["*.map".into()],
            protected: vec!["web.config".into()],
            backup_remote: true,
            host_key: Some("SHA256:x".into()),
        }
    }

    #[test]
    fn a_scan_skips_ignored_folders_and_gives_slash_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Portals").join("0")).unwrap();
        fs::create_dir_all(root.join("node_modules").join("x")).unwrap();
        fs::write(root.join("web.config"), "<c/>").unwrap();
        fs::write(root.join("Portals").join("0").join("skin.css"), "body{}").unwrap();
        fs::write(root.join("node_modules").join("x").join("i.js"), "x").unwrap();
        fs::write(root.join("debug.log"), "x").unwrap();
        let rules = IgnoreRules::new(dnnm_application::source_control::paths::DEFAULT_IGNORE);
        let found = ProjectFiles.scan(root, "", &rules, &CancelToken::new()).unwrap();
        let paths: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["Portals/0/skin.css", "web.config"]);
        assert_eq!(found[1].1.size, 4);
        assert!(ProjectFiles.scan(root, "Missing/Folder", &rules, &CancelToken::new()).unwrap().is_empty());
    }

    #[test]
    fn a_write_is_whole_or_not_at_all_and_keeps_the_time() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        ProjectFiles
            .write(root, "a/b/c.txt", 1_700_000_000, &mut |w| w.write_all(b"hello").map_err(|e| e.to_string()))
            .unwrap();
        let meta = ProjectFiles.stat(root, "a/b/c.txt").unwrap();
        assert_eq!((meta.size, meta.mtime), (5, 1_700_000_000));
        // A write that breaks off leaves the old file and no part behind.
        let failed = ProjectFiles.write(root, "a/b/c.txt", 0, &mut |w| {
            w.write_all(b"half").map_err(|e| e.to_string())?;
            Err("the connection was lost".into())
        });
        assert!(failed.is_err());
        assert_eq!(fs::read_to_string(root.join("a").join("b").join("c.txt")).unwrap(), "hello");
        assert_eq!(fs::read_dir(root.join("a").join("b")).unwrap().count(), 1);
        assert_eq!(
            ProjectFiles.hash(root, "a/b/c.txt").unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(ProjectFiles.write(root, "../outside.txt", 0, &mut |_| Ok(())).is_err(), "never outside the project");
    }

    #[test]
    fn a_change_in_the_folder_is_told() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _watch = watch(dir.path(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        fs::write(dir.path().join("skin.css"), "body{}").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "a new file is told");
        assert!(!change_matters(Path::new("C:/site/.git/index")));
        assert!(!change_matters(Path::new("/site/Portals/.skin.css.dnnm-part")));
        assert!(change_matters(Path::new("/site/Portals/skin.css")));
    }

    #[test]
    fn environments_records_and_history_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = SourceControlStore::new(AppDatabase::new(dir.path().join("dnnmanager.db")));
        store.save_environment(&environment("shop-prod", "shop", EnvironmentKind::Production)).unwrap();
        store.save_environment(&environment("shop-dev", "shop", EnvironmentKind::Development)).unwrap();
        store.save_environment(&environment("blog-test", "blog", EnvironmentKind::Test)).unwrap();
        let list = store.environments("SHOP").unwrap();
        assert_eq!(list.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["shop-dev", "shop-prod"]);
        assert_eq!(list[1], environment("shop-prod", "shop", EnvironmentKind::Production), "read back as saved");

        let record = SyncRecord { local: FileMeta::new(1, 2).with_hash("h"), remote: FileMeta::new(1, 3), synced: 9 };
        store.set_record("shop-prod", "Portals/0/a.css", &record).unwrap();
        assert_eq!(store.records("shop-prod").unwrap()["Portals/0/a.css"], record);
        store.remove_record("shop-prod", "portals/0/A.css").unwrap();
        assert!(store.records("shop-prod").unwrap().is_empty(), "paths without regard to case");

        for at in [1, 3, 2] {
            let line = HistoryLine { at, action: "Upload".into(), path: format!("f{at}"), outcome: "Done".into() };
            store.add_history("shop-prod", &line).unwrap();
        }
        let history = store.history("shop-prod", 2).unwrap();
        assert_eq!(history.iter().map(|h| h.at).collect::<Vec<_>>(), [3, 2]);

        store.trust_host_key("shop-dev", "SHA256:new").unwrap();
        assert_eq!(store.environments("shop").unwrap()[0].host_key.as_deref(), Some("SHA256:new"));
    }

    #[test]
    fn a_table_from_before_ftp_gets_its_protocol_column() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        db.connection()
            .unwrap()
            .execute_batch(
                "CREATE TABLE sc_environments (id TEXT NOT NULL PRIMARY KEY, project TEXT NOT NULL COLLATE NOCASE, \
                 name TEXT NOT NULL, kind TEXT NOT NULL, host TEXT NOT NULL, port INTEGER NOT NULL, user TEXT NOT NULL, \
                 key_file TEXT, remote_root TEXT NOT NULL, mappings TEXT NOT NULL, ignore TEXT NOT NULL, \
                 protected TEXT NOT NULL, backup_remote INTEGER NOT NULL, host_key TEXT); \
                 INSERT INTO sc_environments VALUES ('old', 'shop', 'Live', 'production', 'h', 22, 'u', NULL, '/w', '', '', '', 1, 'SHA256:k');",
            )
            .unwrap();
        let store = SourceControlStore::new(db);
        let env = &store.environments("shop").unwrap()[0];
        assert_eq!((env.protocol, env.host_key.as_deref()), (Protocol::Sftp, Some("SHA256:k")));
        let mut ftps = env.clone();
        ftps.protocol = Protocol::Ftps;
        store.save_environment(&ftps).unwrap();
        let read = &store.environments("shop").unwrap()[0];
        assert_eq!((read.protocol, read.host_key.as_deref()), (Protocol::Ftps, None), "another way in: trusted again");
    }

    #[test]
    fn the_server_listing_is_kept_and_followed() {
        let dir = tempfile::tempdir().unwrap();
        let store = SourceControlStore::new(AppDatabase::new(dir.path().join("dnnmanager.db")));
        assert_eq!(store.listing("e").unwrap(), None);
        let files: Vec<(String, FileMeta)> = (0..3).map(|i| (format!("f{i}.js"), FileMeta::new(i, 10))).collect();
        store.save_listing("e", 500, &files).unwrap();
        store.update_listing("e", "f1.js", None).unwrap();
        store.update_listing("e", "new.js", Some(&FileMeta::new(9, 20))).unwrap();
        let (at, mut listed) = store.listing("e").unwrap().unwrap();
        listed.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(at, 500);
        assert_eq!(listed.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["f0.js", "f2.js", "new.js"]);
        store.save_listing("e", 600, &[]).unwrap();
        assert_eq!(store.listing("e").unwrap(), Some((600, Vec::new())), "a new listing replaces the old");
    }

    #[test]
    fn another_server_forgets_the_key_and_the_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = SourceControlStore::new(AppDatabase::new(dir.path().join("dnnmanager.db")));
        let mut env = environment("shop-prod", "shop", EnvironmentKind::Production);
        store.save_environment(&env).unwrap();
        let record = SyncRecord { local: FileMeta::new(1, 2), remote: FileMeta::new(1, 2), synced: 0 };
        store.set_record(&env.id, "a", &record).unwrap();
        env.name = "Live".into();
        store.save_environment(&env).unwrap();
        assert_eq!(store.records(&env.id).unwrap().len(), 1, "a new name is the same server");
        env.host = "other.example.com".into();
        store.save_environment(&env).unwrap();
        assert!(store.records(&env.id).unwrap().is_empty());
        assert_eq!(store.environments("shop").unwrap()[0].host_key, None, "its key is trusted again only when asked");
        store.delete_environment(&env.id).unwrap();
        assert!(store.environments("shop").unwrap().is_empty());
    }
}
