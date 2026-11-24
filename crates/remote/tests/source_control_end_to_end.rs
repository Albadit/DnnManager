//! Source Control end to end against real servers - the use cases with the real clients, the real project folder and
//! the real tables (in a temporary database). Each runs only when its variable names a server, as
//! `host:port:user:password:folder`: `DNNM_SFTP_TEST` (see `real_server.rs`) and `DNNM_FTP_TEST` (plain FTP), e.g.
//!
//! ```text
//! docker run -d --name dnnm-ftp-test -p 127.0.0.1:2121:21 -p 127.0.0.1:30000-30009:30000-30009 \r
//!     -e PUBLICHOST=127.0.0.1 -e FTP_USER_NAME=tester -e FTP_USER_PASS=secret -e FTP_USER_HOME=/home/tester \r
//!     stilliard/pure-ftpd
//! DNNM_FTP_TEST=127.0.0.1:2121:tester:secret:/
//! ```
//!
//! It works in `<folder>/dnnm-e2e`, which it empties first.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dnnm_application::operation::{CancelToken, Operation, Prompt, Reporter};
use dnnm_application::source_control::model::{Auth, Change, Entry, Environment, EnvironmentKind, Protocol};
use dnnm_application::source_control::sync::{self, Action, SyncDeps, TransferItem};
use dnnm_infrastructure::app_db::AppDatabase;
use dnnm_infrastructure::source_control::{ProjectFiles, SourceControlStore};
use dnnm_remote::Servers as Sftp;

#[derive(Default)]
struct Lines(Vec<String>);

impl Reporter for Lines {
    fn step(&mut self, t: &str) {
        self.0.push(format!("## {t}"));
    }
    fn info(&mut self, t: &str) {
        self.0.push(t.into());
    }
    fn success(&mut self, t: &str) {
        self.0.push(format!("ok {t}"));
    }
    fn warn(&mut self, t: &str) {
        self.0.push(format!("warn {t}"));
    }
    fn fail(&mut self, t: &str) {
        self.0.push(format!("! {t}"));
    }
}

/// Says yes to everything, and keeps the questions.
#[derive(Default)]
struct Yes(Mutex<Vec<String>>);

impl Prompt for Yes {
    fn confirm(&self, q: &str, _: &str, _: &str, _: bool) -> bool {
        self.0.lock().unwrap().push(q.into());
        true
    }
    fn choose(&self, _: &str, _: &[&str]) -> Option<usize> {
        Some(0)
    }
}

struct Setup {
    deps: SyncDeps,
    env: Environment,
    store: SourceControlStore,
    root: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn setup(variable: &str, protocol: Protocol) -> Option<Setup> {
    let spec = std::env::var(variable).ok()?;
    let parts: Vec<&str> = spec.splitn(5, ':').collect();
    let [host, port, user, password, folder] = parts[..] else { panic!("host:port:user:password:folder") };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("shop");
    std::fs::create_dir_all(&root).unwrap();
    let store = SourceControlStore::new(AppDatabase::new(dir.path().join("dnnmanager.db")));
    let password = password.to_string();
    let env = Environment {
        id: "shop-e2e".into(),
        project: "shop".into(),
        name: "Production".into(),
        kind: EnvironmentKind::Production,
        protocol,
        host: host.into(),
        port: port.parse().unwrap(),
        user: user.into(),
        auth: Auth::Password,
        remote_root: format!("{}/dnnm-e2e", folder.trim_end_matches('/')),
        mappings: Vec::new(),
        ignore: Vec::new(),
        protected: Vec::new(),
        backup_remote: true,
        host_key: None,
    };
    store.save_environment(&env).unwrap();
    let deps = SyncDeps {
        local: Arc::new(ProjectFiles),
        remote: Arc::new(Sftp::new(Arc::new(move |_| Some(password.clone())))),
        state: Arc::new(store.clone()),
        project_dir: root.clone(),
        backups: dir.path().join("backups"),
        now: || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
        retry_pause: Duration::ZERO,
    };
    Some(Setup { deps, env, store, root, _dir: dir })
}

impl Setup {
    fn env(&self) -> Environment {
        self.store.environments("shop").unwrap().into_iter().next().unwrap()
    }
    fn scan(&self) -> HashMap<String, (Change, Entry)> {
        let (mut lines, yes) = (Lines::default(), Yes::default());
        let mut op = Operation::new(&mut lines, &yes, CancelToken::new());
        let scan = sync::scan(&self.deps, &self.env(), &mut op).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines.0));
        scan.entries.into_iter().map(|e| (e.path.clone(), (e.change, e))).collect()
    }
    fn send(&self, entries: &[&Entry], action: Action, resolved: bool) -> Vec<String> {
        let (mut lines, yes) = (Lines::default(), Yes::default());
        let items: Vec<TransferItem> = entries.iter().map(|e| TransferItem::from_entry(e, action, resolved)).collect();
        let mut op = Operation::new(&mut lines, &yes, CancelToken::new());
        sync::transfer(&self.deps, &self.env(), &items, false, &mut op).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines.0));
        yes.0.into_inner().unwrap()
    }
    fn write(&self, path: &str, text: &str) {
        let full = path.split('/').fold(self.root.clone(), |p, s| p.join(s));
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    /// Changes a file on the server, as someone else would.
    fn change_on_server(&self, path: &str, text: &str) {
        let mut fs = self.deps.remote.connect(&self.env()).unwrap();
        let remote = format!("{}/{path}", self.env.remote_root);
        fs.upload(&mut text.as_bytes(), &remote, 2_000_000_000, &mut |_| {}, &CancelToken::new()).unwrap();
    }
}

fn change(scan: &HashMap<String, (Change, Entry)>, path: &str) -> Change {
    scan.get(path).map_or(Change::Unchanged, |(c, _)| *c)
}

#[test]
fn over_sftp() {
    match setup("DNNM_SFTP_TEST", Protocol::Sftp) {
        Some(s) => compare_upload_change_both_sides_and_resolve(s),
        None => eprintln!("DNNM_SFTP_TEST isn't set - skipped"),
    }
}

#[test]
fn over_plain_ftp() {
    match setup("DNNM_FTP_TEST", Protocol::Ftp) {
        Some(s) => compare_upload_change_both_sides_and_resolve(s),
        None => eprintln!("DNNM_FTP_TEST isn't set - skipped"),
    }
}

fn compare_upload_change_both_sides_and_resolve(s: Setup) {
    // Trust the key (a scan asks; the test says yes), then empty what an earlier run left on the server.
    let first = s.scan();
    let leftovers: Vec<&Entry> = first.values().map(|(_, e)| e).collect();
    if !leftovers.is_empty() {
        let mut fs = s.deps.remote.connect(&s.env()).unwrap();
        for e in leftovers {
            fs.remove(&format!("{}/{}", s.env.remote_root, e.path)).unwrap();
        }
    }

    s.write("web.config", "<configuration/>");
    s.write("Portals/0/skin.css", "body{color:red}");
    s.write("DesktopModules/Shop/View.ascx", "<div>shop</div>");
    s.write("App_Data/Database.mdf", "local database - never sent");
    s.write("node_modules/x/index.js", "never sent");
    let scan = s.scan();
    assert_eq!(scan.len(), 3, "{:?}", scan.keys().collect::<Vec<_>>());
    assert!(scan.values().all(|(c, _)| *c == Change::Added));

    // Upload all: on Production web.config is asked about again.
    let all: Vec<&Entry> = scan.values().map(|(_, e)| e).collect();
    let questions = s.send(&all, Action::Upload, false);
    assert!(questions.iter().any(|q| q.contains("protected on Production") && q.contains("web.config")), "{questions:?}");
    let scan = s.scan();
    assert!(scan.values().all(|(c, _)| *c == Change::Unchanged), "{scan:?}");

    // A change on each side, and one on both.
    std::thread::sleep(Duration::from_millis(1100));
    s.write("Portals/0/skin.css", "body{color:blue}");
    s.change_on_server("DesktopModules/Shop/View.ascx", "<div>edited on the server</div>");
    s.write("web.config", "<configuration debug='true'/>");
    s.change_on_server("web.config", "<configuration live='true'/>");
    let scan = s.scan();
    assert_eq!(change(&scan, "Portals/0/skin.css"), Change::Modified);
    assert_eq!(change(&scan, "DesktopModules/Shop/View.ascx"), Change::Incoming);
    assert_eq!(change(&scan, "web.config"), Change::Conflict);

    // The diff reads both copies.
    let (remote, local) = sync::read_both(&s.deps, &s.env(), "web.config", &CancelToken::new()).unwrap();
    assert_eq!(remote.unwrap(), b"<configuration live='true'/>");
    assert_eq!(local.unwrap(), b"<configuration debug='true'/>");

    // Upload the change, download the server's, keep the server's side of the conflict.
    s.send(&[&scan["Portals/0/skin.css"].1], Action::Upload, false);
    s.send(&[&scan["DesktopModules/Shop/View.ascx"].1], Action::Download, false);
    s.send(&[&scan["web.config"].1], Action::Download, true);
    assert_eq!(std::fs::read_to_string(s.root.join("web.config")).unwrap(), "<configuration live='true'/>");
    assert_eq!(
        std::fs::read_to_string(s.root.join("DesktopModules").join("Shop").join("View.ascx")).unwrap(),
        "<div>edited on the server</div>"
    );
    let scan = s.scan();
    assert!(scan.values().all(|(c, _)| *c == Change::Unchanged), "{scan:?}");

    // The replaced server copy was backed up first.
    let backups = s.deps.backups.join(&s.env.id);
    let found = walk(&backups);
    assert!(found.iter().any(|p| p.ends_with("skin.css")), "{found:?}");

    // Deleted here: deleted on the server only on request; only there: never.
    std::fs::remove_file(s.root.join("Portals").join("0").join("skin.css")).unwrap();
    s.change_on_server("robots.txt", "User-agent: *");
    let scan = s.scan();
    assert_eq!(change(&scan, "Portals/0/skin.css"), Change::Deleted);
    assert_eq!(change(&scan, "robots.txt"), Change::RemoteOnly);
    let questions = s.send(&[&scan["Portals/0/skin.css"].1], Action::DeleteRemote, false);
    assert!(questions[0].starts_with("Delete 1 file(s) on Production"));
    let scan = s.scan();
    assert!(!scan.contains_key("Portals/0/skin.css"));
    assert_eq!(change(&scan, "robots.txt"), Change::RemoteOnly, "never deleted for being missing here");

    let history = s.store.history(&s.env.id, 50).unwrap();
    assert!(history.len() >= 6 && history.iter().all(|h| h.outcome == "Done"), "{history:?}");
}

fn walk(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                pending.push(e.path());
            } else {
                found.push(e.path().display().to_string().replace('\\', "/"));
            }
        }
    }
    found
}
