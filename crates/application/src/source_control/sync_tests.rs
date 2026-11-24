//! The use cases against an in-memory project, server and store - a server that can lose the connection, refuse a
//! path, or have a file changed behind the comparison's back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::operation::tests::{Answer, Lines};
use crate::source_control::model::{Auth, EnvironmentKind, Mapping};

// ─── Stand-ins ─────────────────────────────────────────────────────────────

type Files = Arc<Mutex<BTreeMap<String, (Vec<u8>, i64)>>>;

#[derive(Default)]
struct Local(Mutex<HashMap<String, Files>>);

impl Local {
    fn files(&self, root: &Path) -> Files {
        self.0.lock().unwrap().entry(root.display().to_string()).or_default().clone()
    }
    fn put(&self, root: &Path, path: &str, text: &str, mtime: i64) {
        self.files(root).lock().unwrap().insert(path.into(), (text.as_bytes().to_vec(), mtime));
    }
    fn text(&self, root: &Path, path: &str) -> Option<String> {
        self.files(root).lock().unwrap().get(path).map(|(b, _)| String::from_utf8(b.clone()).unwrap())
    }
}

impl LocalFiles for Local {
    fn scan(&self, root: &Path, folder: &str, ignore: &IgnoreRules, _: &CancelToken) -> Result<Vec<(String, FileMeta)>, String> {
        Ok(self
            .files(root)
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| folder.is_empty() || p.starts_with(&format!("{folder}/")))
            .filter(|(p, _)| !ignore.matches(p))
            .map(|(p, (b, t))| (p.clone(), FileMeta::new(b.len() as u64, *t)))
            .collect())
    }
    fn stat(&self, root: &Path, path: &str) -> Option<FileMeta> {
        self.files(root).lock().unwrap().get(path).map(|(b, t)| FileMeta::new(b.len() as u64, *t))
    }
    fn hash(&self, root: &Path, path: &str) -> Result<String, String> {
        Ok(sha256_hex(&self.files(root).lock().unwrap().get(path).ok_or("missing")?.0))
    }
    fn open(&self, root: &Path, path: &str) -> Result<Box<dyn Read + Send>, String> {
        let bytes = self.files(root).lock().unwrap().get(path).ok_or("missing")?.0.clone();
        Ok(Box::new(std::io::Cursor::new(bytes)))
    }
    fn write(
        &self,
        root: &Path,
        path: &str,
        mtime: i64,
        fill: &mut dyn FnMut(&mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut bytes = Vec::new();
        fill(&mut bytes)?; // only a whole file is put in place
        self.files(root).lock().unwrap().insert(path.into(), (bytes, mtime));
        Ok(())
    }
}

/// The server: its files, a key, and what goes wrong.
#[derive(Default)]
struct Server {
    files: Files,
    key: String,
    /// The next this many uploads lose the connection halfway.
    drop_uploads: AtomicUsize,
    /// Paths it refuses to write.
    denied: Mutex<HashSet<String>>,
    connections: AtomicUsize,
    /// Changed on the server between the comparison and the transfer (on the first stat of it).
    changes_behind: Mutex<Option<(String, Vec<u8>)>>,
}

struct Connection(Arc<Server>);

impl RemoteFs for Connection {
    fn list(&mut self, folder: &str, _: &CancelToken) -> Result<Vec<(String, FileMeta)>, RemoteError> {
        let prefix = format!("{}/", folder.trim_end_matches('/'));
        Ok(self
            .0
            .files
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(p, (b, t))| p.strip_prefix(&prefix).map(|rel| (rel.to_string(), FileMeta::new(b.len() as u64, *t))))
            .collect())
    }
    fn stat(&mut self, path: &str) -> Result<Option<FileMeta>, RemoteError> {
        if let Some((p, bytes)) = self.0.changes_behind.lock().unwrap().take_if(|(p, _)| p == path) {
            self.0.files.lock().unwrap().insert(p, (bytes, 999_999));
        }
        Ok(self.0.files.lock().unwrap().get(path).map(|(b, t)| FileMeta::new(b.len() as u64, *t)))
    }
    fn download(&mut self, path: &str, to: &mut dyn Write, _: &mut dyn FnMut(u64), _: &CancelToken) -> Result<u64, RemoteError> {
        let bytes = self.0.files.lock().unwrap().get(path).ok_or(RemoteError::Other("no such file".into()))?.0.clone();
        to.write_all(&bytes).map_err(|e| RemoteError::Other(e.to_string()))?;
        Ok(bytes.len() as u64)
    }
    fn upload(&mut self, from: &mut dyn Read, path: &str, mtime: i64, _: &mut dyn FnMut(u64), _: &CancelToken) -> Result<(), RemoteError> {
        if self.0.denied.lock().unwrap().contains(path) {
            return Err(RemoteError::Denied("permission denied".into()));
        }
        let mut bytes = Vec::new();
        from.read_to_end(&mut bytes).unwrap();
        if self.0.drop_uploads.load(Ordering::SeqCst) > 0 {
            self.0.drop_uploads.fetch_sub(1, Ordering::SeqCst);
            // Broken off before the rename: the old file stays.
            return Err(RemoteError::Connection("reset by peer".into()));
        }
        self.0.files.lock().unwrap().insert(path.into(), (bytes, mtime));
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), RemoteError> {
        self.0.files.lock().unwrap().remove(path);
        Ok(())
    }
}

struct Remotes(Arc<Server>);

impl Remote for Remotes {
    fn host_key(&self, _: &Environment) -> Result<String, String> {
        Ok(self.0.key.clone())
    }
    fn connect(&self, env: &Environment) -> Result<Box<dyn RemoteFs>, RemoteError> {
        // FTP and FTPS trust what `connect` noted; SFTP the server's own key.
        let trusted = match env.protocol {
            crate::source_control::model::Protocol::Sftp => env.host_key.as_deref() == Some(self.0.key.as_str()),
            _ => env.host_key.is_some(),
        };
        if !trusted {
            return Err(RemoteError::Other("the server's key isn't the one trusted".into()));
        }
        self.0.connections.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Connection(self.0.clone())))
    }
}

#[derive(Default)]
struct State {
    listing: Mutex<Option<(i64, BTreeMap<String, FileMeta>)>>,
    records: Mutex<HashMap<String, SyncRecord>>,
    history: Mutex<Vec<HistoryLine>>,
    trusted: Mutex<Option<String>>,
}

impl SyncState for State {
    fn records(&self, _: &str) -> Result<HashMap<String, SyncRecord>, String> {
        Ok(self.records.lock().unwrap().clone())
    }
    fn set_record(&self, _: &str, path: &str, record: &SyncRecord) -> Result<(), String> {
        self.records.lock().unwrap().insert(path.into(), record.clone());
        Ok(())
    }
    fn remove_record(&self, _: &str, path: &str) -> Result<(), String> {
        self.records.lock().unwrap().remove(path);
        Ok(())
    }
    fn add_history(&self, _: &str, line: &HistoryLine) -> Result<(), String> {
        self.history.lock().unwrap().push(line.clone());
        Ok(())
    }
    fn trust_host_key(&self, _: &str, fingerprint: &str) -> Result<(), String> {
        *self.trusted.lock().unwrap() = Some(fingerprint.into());
        Ok(())
    }
    fn save_listing(&self, _: &str, at: i64, files: &[(String, FileMeta)]) -> Result<(), String> {
        *self.listing.lock().unwrap() = Some((at, files.iter().cloned().collect()));
        Ok(())
    }
    fn update_listing(&self, _: &str, path: &str, meta: Option<&FileMeta>) -> Result<(), String> {
        if let Some((_, files)) = self.listing.lock().unwrap().as_mut() {
            match meta {
                Some(m) => files.insert(path.into(), m.clone()),
                None => files.remove(path),
            };
        }
        Ok(())
    }
    fn listing(&self, _: &str) -> Result<Option<Listing>, String> {
        Ok(self.listing.lock().unwrap().clone().map(|(at, f)| (at, f.into_iter().collect())))
    }
}

struct World {
    local: Arc<Local>,
    server: Arc<Server>,
    state: Arc<State>,
    deps: SyncDeps,
    env: Environment,
}

fn world() -> World {
    let local = Arc::new(Local::default());
    let server = Arc::new(Server { key: "SHA256:abc".into(), ..Default::default() });
    let state = Arc::new(State::default());
    let deps = SyncDeps {
        local: local.clone(),
        remote: Arc::new(Remotes(server.clone())),
        state: state.clone(),
        project_dir: "/proj".into(),
        backups: "/backups".into(),
        now: || 1_000,
        retry_pause: Duration::ZERO,
    };
    let env = Environment {
        id: "shop-test".into(),
        project: "shop".into(),
        name: "Test".into(),
        kind: EnvironmentKind::Test,
        protocol: crate::source_control::model::Protocol::Sftp,
        host: "test.example".into(),
        port: 22,
        user: "deploy".into(),
        auth: Auth::Password,
        remote_root: "/www".into(),
        mappings: Vec::new(),
        ignore: Vec::new(),
        protected: Vec::new(),
        backup_remote: false,
        host_key: Some("SHA256:abc".into()),
    };
    World { local, server, state, deps, env }
}

impl World {
    fn here(&self, path: &str, text: &str, mtime: i64) {
        self.local.put(Path::new("/proj"), path, text, mtime);
    }
    fn there(&self, path: &str, text: &str, mtime: i64) {
        self.server.files.lock().unwrap().insert(format!("/www/{path}"), (text.as_bytes().to_vec(), mtime));
    }
    fn on_server(&self, path: &str) -> Option<String> {
        self.server.files.lock().unwrap().get(&format!("/www/{path}")).map(|(b, _)| String::from_utf8(b.clone()).unwrap())
    }
    fn scan(&self, answer: &Answer) -> (Result<Scan, String>, Vec<String>) {
        let mut lines = Lines::default();
        let result = {
            let mut op = Operation::new(&mut lines, answer, CancelToken::new());
            scan(&self.deps, &self.env, &mut op)
        };
        (result, lines.0)
    }
    fn changes(&self) -> Vec<(String, Change)> {
        let scan = self.scan(&Answer::yes()).0.unwrap();
        scan.entries.into_iter().filter(|e| e.change != Change::Unchanged).map(|e| (e.path, e.change)).collect()
    }
    fn entry(&self, path: &str) -> Entry {
        self.scan(&Answer::yes()).0.unwrap().entries.into_iter().find(|e| e.path == path).unwrap()
    }
    fn send(&self, items: &[TransferItem], answer: &Answer) -> (Result<TransferResult, String>, Vec<String>) {
        let mut lines = Lines::default();
        let result = {
            let mut op = Operation::new(&mut lines, answer, CancelToken::new());
            transfer(&self.deps, &self.env, items, false, &mut op)
        };
        (result, lines.0)
    }
    fn upload(&self, path: &str) -> Result<TransferResult, String> {
        self.send(&[TransferItem::from_entry(&self.entry(path), Action::Upload, false)], &Answer::yes()).0
    }
}

fn c(path: &str, change: Change) -> (String, Change) {
    (path.into(), change)
}

// ─── Comparing ────────────────────────────────────────────────────────────

#[test]
fn a_first_comparison_finds_what_is_only_on_one_side() {
    let w = world();
    w.here("web.config", "<config/>", 100);
    w.there("web.config", "<config/>", 100);
    w.here("Portals/0/skin.css", "body{}", 100);
    w.there("Portals/0/old.css", "x", 100);
    w.here("App_Data/Database.mdf", "db", 100);
    w.there("Portals/_default/Logs/a.log", "log", 100);
    assert_eq!(
        w.changes(),
        [c("Portals/0/old.css", Change::RemoteOnly), c("Portals/0/skin.css", Change::Added)],
        "the database and the logs are ignored; a file only there is never a deletion"
    );
}

#[test]
fn local_and_server_changes_after_a_sync() {
    let w = world();
    for p in ["a.js", "b.js", "c.js", "d.js"] {
        w.here(p, "one", 100);
    }
    let items: Vec<TransferItem> = w
        .scan(&Answer::yes())
        .0
        .unwrap()
        .entries
        .iter()
        .map(|e| TransferItem::from_entry(e, Action::Upload, false))
        .collect();
    w.send(&items, &Answer::yes()).0.unwrap();
    assert!(w.changes().is_empty(), "synced: nothing differs");
    w.here("a.js", "changed here", 200);
    w.there("b.js", "changed there", 300);
    w.here("c.js", "both", 200);
    w.there("c.js", "both, otherwise", 300);
    w.local.files(Path::new("/proj")).lock().unwrap().remove("d.js");
    assert_eq!(
        w.changes(),
        [c("a.js", Change::Modified), c("b.js", Change::Incoming), c("c.js", Change::Conflict), c("d.js", Change::Deleted)]
    );
}

#[test]
fn copied_files_with_other_times_are_read_and_recorded_as_the_same() {
    let w = world();
    w.here("same.js", "identical", 100);
    w.there("same.js", "identical", 5_000);
    w.here("other.js", "aaaa", 100);
    w.there("other.js", "bbbb", 5_000);
    assert_eq!(w.changes(), [c("other.js", Change::Incoming)]);
    assert!(w.state.records.lock().unwrap().contains_key("same.js"));
}

#[test]
fn only_the_mapped_folders_are_compared() {
    let mut w = world();
    w.env.mappings = vec![Mapping { local: "DesktopModules/Shop".into(), remote: "/modules/Shop".into() }];
    w.here("DesktopModules/Shop/View.ascx", "v", 100);
    w.here("web.config", "c", 100);
    w.server.files.lock().unwrap().insert("/modules/Shop/Edit.ascx".into(), (b"e".to_vec(), 100));
    assert_eq!(
        w.changes(),
        [c("DesktopModules/Shop/Edit.ascx", Change::RemoteOnly), c("DesktopModules/Shop/View.ascx", Change::Added)]
    );
    w.upload("DesktopModules/Shop/View.ascx").unwrap();
    assert!(w.server.files.lock().unwrap().contains_key("/modules/Shop/View.ascx"));
}

#[test]
fn a_new_server_key_is_trusted_only_when_the_user_says_so() {
    let mut w = world();
    w.env.host_key = None;
    let (result, _) = w.scan(&Answer::no());
    assert_eq!(result.unwrap_err(), CANCELLED);
    assert_eq!(w.server.connections.load(Ordering::SeqCst), 0, "no sign-in to an untrusted server");
    let answer = Answer::yes();
    w.scan(&answer).0.unwrap();
    assert_eq!(w.state.trusted.lock().unwrap().as_deref(), Some("SHA256:abc"));
    assert!(answer.1.lock().unwrap()[0].contains("SHA256:abc"), "the question shows the fingerprint");
    w.env.host_key = Some("SHA256:other".into());
    assert!(w.scan(&Answer::yes()).0.unwrap_err().contains("isn't the one trusted"), "a changed key is refused");
}

#[test]
fn ftps_needs_no_question_and_plain_ftp_a_warning_first() {
    use crate::source_control::model::Protocol;
    let mut w = world();
    w.env.protocol = Protocol::Ftps;
    w.env.host_key = None;
    let answer = Answer::no();
    w.scan(&answer).0.unwrap();
    assert!(answer.1.lock().unwrap().is_empty(), "the certificate is checked against the system's roots");
    assert_eq!(w.state.trusted.lock().unwrap().as_deref(), Some(TLS_CHECKED));

    w.env.protocol = Protocol::Ftp;
    w.env.host_key = None;
    let answer = Answer::no();
    assert_eq!(w.scan(&answer).0.unwrap_err(), CANCELLED);
    assert!(answer.1.lock().unwrap()[0].contains("unencrypted"));
    assert_eq!(w.server.connections.load(Ordering::SeqCst), 1, "no sign-in before the warning is accepted");
    w.scan(&Answer::yes()).0.unwrap();
    assert_eq!(w.state.trusted.lock().unwrap().as_deref(), Some(PLAIN_FTP_ACCEPTED));
}

// ─── Sending ──────────────────────────────────────────────────────────────

#[test]
fn an_upload_writes_the_server_and_the_record() {
    let w = world();
    w.here("Portals/0/skin.css", "body{color:red}", 100);
    let result = w.upload("Portals/0/skin.css").unwrap();
    assert_eq!(result.done, ["Portals/0/skin.css"]);
    assert_eq!(w.on_server("Portals/0/skin.css").as_deref(), Some("body{color:red}"));
    let record = w.state.records.lock().unwrap()["Portals/0/skin.css"].clone();
    assert_eq!(record.local.hash, record.remote.hash);
    assert_eq!(w.state.history.lock().unwrap()[0].outcome, "Done");
    assert!(w.changes().is_empty());
}

#[test]
fn replacing_a_server_file_asks_first_and_no_leaves_it() {
    let w = world();
    w.here("a.js", "new", 200);
    w.there("a.js", "old", 100);
    let item = TransferItem::from_entry(&w.entry("a.js"), Action::Upload, false);
    let answer = Answer::no();
    assert_eq!(w.send(std::slice::from_ref(&item), &answer).0.unwrap_err(), CANCELLED);
    assert!(answer.1.lock().unwrap()[0].starts_with("Replace 1 file(s) on Test"));
    assert_eq!(w.on_server("a.js").as_deref(), Some("old"));
    w.send(&[item], &Answer::yes()).0.unwrap();
    assert_eq!(w.on_server("a.js").as_deref(), Some("new"));
}

#[test]
fn a_conflict_is_never_sent_without_a_choice() {
    let w = world();
    w.here("c.js", "one", 100);
    w.upload("c.js").unwrap();
    w.here("c.js", "mine", 200);
    w.there("c.js", "theirs", 300);
    let entry = w.entry("c.js");
    assert_eq!(entry.change, Change::Conflict);
    let (result, _) = w.send(&[TransferItem::from_entry(&entry, Action::Upload, false)], &Answer::yes());
    assert_eq!(result.unwrap().skipped[0].1, "it changed on both sides - choose which one to keep");
    assert_eq!(w.on_server("c.js").as_deref(), Some("theirs"));
    // Keep the server's side: the project gets its copy.
    w.send(&[TransferItem::from_entry(&entry, Action::Download, true)], &Answer::yes()).0.unwrap();
    assert_eq!(w.local.text(Path::new("/proj"), "c.js").as_deref(), Some("theirs"));
    assert!(w.changes().is_empty());
}

#[test]
fn a_file_changed_on_the_server_after_the_comparison_is_left_alone() {
    let w = world();
    w.here("a.js", "mine", 200);
    w.there("a.js", "old", 100);
    let item = TransferItem::from_entry(&w.entry("a.js"), Action::Upload, false);
    *w.server.changes_behind.lock().unwrap() = Some(("/www/a.js".into(), b"someone else's".to_vec()));
    let result = w.send(&[item], &Answer::yes()).0.unwrap();
    assert!(result.skipped[0].1.contains("changed on the server since the comparison"));
    assert_eq!(w.on_server("a.js").as_deref(), Some("someone else's"), "never overwritten unseen");
}

#[test]
fn a_deletion_on_the_server_is_asked_for_and_only_after_a_sync() {
    let w = world();
    w.there("only-there.js", "x", 100);
    let remote_only = w.entry("only-there.js");
    let (result, _) = w.send(&[TransferItem::from_entry(&remote_only, Action::DeleteRemote, false)], &Answer::yes());
    assert!(result.unwrap().skipped[0].1.contains("only a file deleted here after a sync"));
    assert!(w.on_server("only-there.js").is_some());

    w.here("gone.js", "x", 100);
    w.upload("gone.js").unwrap();
    w.local.files(Path::new("/proj")).lock().unwrap().remove("gone.js");
    let item = TransferItem::from_entry(&w.entry("gone.js"), Action::DeleteRemote, false);
    let answer = Answer::no();
    assert_eq!(w.send(std::slice::from_ref(&item), &answer).0.unwrap_err(), CANCELLED);
    assert!(answer.1.lock().unwrap()[0].starts_with("Delete 1 file(s) on Test"));
    assert!(w.on_server("gone.js").is_some());
    w.send(&[item], &Answer::yes()).0.unwrap();
    assert!(w.on_server("gone.js").is_none());
    assert!(!w.state.records.lock().unwrap().contains_key("gone.js"));
}

#[test]
fn production_asks_again_for_protected_files_and_backs_up_what_it_replaces() {
    let mut w = world();
    w.env.kind = EnvironmentKind::Production;
    w.env.name = "Production".into();
    w.env.backup_remote = true;
    w.here("web.config", "<new/>", 200);
    w.there("web.config", "<live/>", 100);
    let item = TransferItem::from_entry(&w.entry("web.config"), Action::Upload, false);
    // Yes to replacing it, no to sending a protected file.
    let script = Scripted(Mutex::new(vec![true, false]), Mutex::new(Vec::new()));
    let mut lines = Lines::default();
    let result = {
        let mut op = Operation::new(&mut lines, &script, CancelToken::new());
        transfer(&w.deps, &w.env, std::slice::from_ref(&item), false, &mut op)
    };
    assert!(script.1.lock().unwrap()[1].contains("protected on Production"));
    assert_eq!(result.unwrap_err(), CANCELLED);
    assert_eq!(w.on_server("web.config").as_deref(), Some("<live/>"));
    w.send(&[item], &Answer::yes()).0.unwrap();
    assert_eq!(w.on_server("web.config").as_deref(), Some("<new/>"));
    assert_eq!(
        w.local.text(&Path::new("/backups").join("shop-test").join("1000"), "web.config").as_deref(),
        Some("<live/>"),
        "the live copy was kept first"
    );
}

#[test]
fn a_lost_connection_is_tried_again_and_a_refusal_fails_only_that_file() {
    let w = world();
    w.here("a.js", "a", 100);
    w.here("b.js", "b", 100);
    w.server.drop_uploads.store(1, Ordering::SeqCst);
    w.server.denied.lock().unwrap().insert("/www/b.js".into());
    let entries = w.scan(&Answer::yes()).0.unwrap().entries;
    let items: Vec<TransferItem> = entries.iter().map(|e| TransferItem::from_entry(e, Action::Upload, false)).collect();
    let (result, lines) = w.send(&items, &Answer::yes());
    assert!(result.unwrap_err().contains("1 sent, 1 failed"));
    assert!(lines.iter().any(|l| l.contains("trying again (2 of 3)")));
    assert_eq!(w.on_server("a.js").as_deref(), Some("a"), "sent on the second try");
    assert!(w.on_server("b.js").is_none());
    assert!(!w.state.records.lock().unwrap().contains_key("b.js"), "a failed file isn't recorded as synced");
    assert!(w.state.history.lock().unwrap().iter().any(|h| h.path == "b.js" && h.outcome.contains("refused")));
}

#[test]
fn three_lost_connections_give_up_without_recording_it() {
    let w = world();
    w.here("a.js", "new", 200);
    w.server.drop_uploads.store(3, Ordering::SeqCst);
    let result = w.upload("a.js");
    assert!(result.unwrap_err().contains("1 failed"));
    assert!(w.on_server("a.js").is_none(), "nothing half-written");
    assert!(w.state.records.lock().unwrap().is_empty());
}

#[test]
fn revert_local_takes_the_servers_copy_after_a_question() {
    let w = world();
    w.here("a.js", "one", 100);
    w.upload("a.js").unwrap();
    w.here("a.js", "local edit", 200);
    let entry = w.entry("a.js");
    assert_eq!(entry.change, Change::Modified);
    let answer = Answer::no();
    assert_eq!(w.send(&[TransferItem::from_entry(&entry, Action::Download, true)], &answer).0.unwrap_err(), CANCELLED);
    assert!(answer.1.lock().unwrap()[0].contains("Their changes here are lost"));
    w.send(&[TransferItem::from_entry(&entry, Action::Download, true)], &Answer::yes()).0.unwrap();
    assert_eq!(w.local.text(Path::new("/proj"), "a.js").as_deref(), Some("one"));
}

#[test]
fn the_diff_reads_both_copies() {
    let w = world();
    w.here("a.js", "mine", 100);
    w.there("a.js", "theirs", 100);
    let (remote, local) = read_both(&w.deps, &w.env, "a.js", &CancelToken::new()).unwrap();
    assert_eq!((remote.unwrap(), local.unwrap()), (b"theirs".to_vec(), b"mine".to_vec()));
    let mut untrusted = w.env.clone();
    untrusted.host_key = None;
    assert!(read_both(&w.deps, &untrusted, "a.js", &CancelToken::new()).unwrap_err().starts_with("Refresh first"));
}

#[test]
fn a_conflict_merged_by_hand_is_uploaded_only_once_its_markers_are_gone() {
    let w = world();
    w.here("c.js", "one\n", 100);
    w.upload("c.js").unwrap();
    w.here("c.js", "mine\n", 200);
    w.there("c.js", "theirs\n", 300);
    let mut lines = Lines::default();
    let answer = Answer::yes();
    let full = {
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        start_merge(&w.deps, &w.env, "c.js", &mut op).unwrap()
    };
    assert!(full.ends_with("c.js"));
    let merged = w.local.text(Path::new("/proj"), "c.js").unwrap();
    assert_eq!(merged, "<<<<<<< Test\ntheirs\n=======\nmine\n>>>>>>> project\n");
    assert_eq!(
        w.local.text(&Path::new("/backups").join("shop-test").join("1000-project"), "c.js").as_deref(),
        Some("mine\n"),
        "the project's copy kept first"
    );
    // Still marked: Keep mine leaves it alone.
    let entry = w.entry("c.js");
    let result = w.send(&[TransferItem::from_entry(&entry, Action::Upload, true)], &Answer::yes()).0.unwrap();
    assert!(result.skipped[0].1.contains("still has merge markers"));
    assert_eq!(w.on_server("c.js").as_deref(), Some("theirs\n"));
    // Merged: it goes.
    w.here("c.js", "theirs and mine\n", 400);
    let entry = w.entry("c.js");
    w.send(&[TransferItem::from_entry(&entry, Action::Upload, true)], &Answer::yes()).0.unwrap();
    assert_eq!(w.on_server("c.js").as_deref(), Some("theirs and mine\n"));
}

#[test]
fn a_change_here_is_seen_without_the_server() {
    let w = world();
    w.here("a.js", "one", 100);
    w.there("b.js", "only there", 100);
    w.upload("a.js").unwrap();
    let last = w.scan(&Answer::yes()).0.unwrap().entries;
    let connections = w.server.connections.load(Ordering::SeqCst);
    w.here("a.js", "edited", 200);
    w.here("new.js", "added", 200);
    let entries = reclassify_local(&w.deps, &w.env, &last, &CancelToken::new()).unwrap();
    let changes: Vec<(String, Change)> =
        entries.into_iter().filter(|e| e.change != Change::Unchanged).map(|e| (e.path, e.change)).collect();
    assert_eq!(changes, [c("a.js", Change::Modified), c("b.js", Change::RemoteOnly), c("new.js", Change::Added)]);
    assert_eq!(w.server.connections.load(Ordering::SeqCst), connections, "no connection");
}

#[test]
fn a_confirmed_preview_asks_nothing_more_but_still_checks_each_file() {
    let mut w = world();
    w.env.kind = crate::source_control::model::EnvironmentKind::Production;
    w.here("web.config", "<new/>", 200);
    w.there("web.config", "<live/>", 100);
    w.here("a.js", "a", 200);
    w.there("a.js", "old", 100);
    let entries = w.scan(&Answer::yes()).0.unwrap().entries;
    let items: Vec<TransferItem> = entries.iter().map(|e| TransferItem::from_entry(e, Action::Upload, false)).collect();
    *w.server.changes_behind.lock().unwrap() = Some(("/www/a.js".into(), b"changed meanwhile".to_vec()));
    let answer = Answer::no();
    let mut lines = Lines::default();
    let result = {
        let mut op = Operation::new(&mut lines, &answer, CancelToken::new());
        transfer(&w.deps, &w.env, &items, true, &mut op).unwrap()
    };
    assert!(answer.1.lock().unwrap().is_empty(), "the preview asked it all");
    assert_eq!(result.done, ["web.config"]);
    assert!(result.skipped[0].1.contains("changed on the server since the comparison"), "still checked");
}

#[test]
fn the_last_comparison_comes_back_without_a_connection_and_follows_transfers() {
    let w = world();
    assert_eq!(cached(&w.deps, &w.env, &CancelToken::new()).unwrap(), None, "nothing before a first comparison");
    w.here("a.js", "new", 100);
    w.there("gone.js", "x", 100);
    w.scan(&Answer::yes()).0.unwrap();
    w.upload("a.js").unwrap();
    let connections = w.server.connections.load(Ordering::SeqCst);
    let (at, entries) = cached(&w.deps, &w.env, &CancelToken::new()).unwrap().unwrap();
    assert_eq!(at, 1000);
    let changes: Vec<(String, Change)> =
        entries.into_iter().filter(|e| e.change != Change::Unchanged).map(|e| (e.path, e.change)).collect();
    assert_eq!(changes, [c("gone.js", Change::RemoteOnly)], "a.js was uploaded since: the listing knows");
    assert_eq!(w.server.connections.load(Ordering::SeqCst), connections, "no connection");
}

/// Answers the questions in turn, recording them.
struct Scripted(Mutex<Vec<bool>>, Mutex<Vec<String>>);

impl crate::operation::Prompt for Scripted {
    fn confirm(&self, question: &str, _: &str, _: &str, _: bool) -> bool {
        self.1.lock().unwrap().push(question.into());
        let mut answers = self.0.lock().unwrap();
        if answers.is_empty() { false } else { answers.remove(0) }
    }
    fn choose(&self, _: &str, _: &[&str]) -> Option<usize> {
        None
    }
}
