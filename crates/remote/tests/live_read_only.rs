//! Source Control against a live FTPS server, read only - for checking a real site (an Azure App Service, say) without
//! changing a byte there. It runs only when `DNNM_FTPS_READONLY` names the server as `host:port:user:password:folder`:
//!
//! ```text
//! DNNM_FTPS_READONLY='waws-prod-xx.ftp.azurewebsites.windows.net:21:app\$app:<password>:/site/wwwroot' \
//!     cargo test -p dnnm-remote --test live_read_only -- --nocapture
//! ```
//!
//! It signs in (the certificate checked against the system's roots), compares the whole folder with an empty project,
//! reads a file's facts, downloads a few small files into a temporary project, compares again, shows the last
//! comparison from the cache and reads both copies of a file for the diff. Nothing is uploaded, renamed or deleted.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dnnm_application::operation::{CancelToken, Operation, Prompt, Reporter};
use dnnm_application::source_control::model::{Auth, Change, Environment, EnvironmentKind, Protocol};
use dnnm_application::source_control::plan;
use dnnm_application::source_control::sync::{self, Action, SyncDeps, TransferItem};
use dnnm_infrastructure::app_db::AppDatabase;
use dnnm_infrastructure::source_control::{ProjectFiles, SourceControlStore};
use dnnm_remote::Servers;

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

#[test]
fn a_live_ftps_site_read_only() {
    let Ok(spec) = std::env::var("DNNM_FTPS_READONLY") else {
        eprintln!("DNNM_FTPS_READONLY isn't set - skipped");
        return;
    };
    let parts: Vec<&str> = spec.splitn(5, ':').collect();
    let [host, port, user, password, folder] = parts[..] else { panic!("host:port:user:password:folder") };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("site");
    std::fs::create_dir_all(&root).unwrap();
    let store = SourceControlStore::new(AppDatabase::new(dir.path().join("dnnmanager.db")));
    let password = password.to_string();
    let env = Environment {
        id: "live".into(),
        project: "site".into(),
        name: "Production".into(),
        kind: EnvironmentKind::Production,
        protocol: Protocol::Ftps,
        host: host.into(),
        port: port.parse().unwrap(),
        user: user.into(),
        auth: Auth::Password,
        remote_root: folder.trim_end_matches('/').to_string(),
        mappings: Vec::new(),
        ignore: Vec::new(),
        protected: Vec::new(),
        backup_remote: true,
        host_key: None,
    };
    store.save_environment(&env).unwrap();
    let deps = SyncDeps {
        local: Arc::new(ProjectFiles),
        remote: Arc::new(Servers::new(Arc::new(move |_| Some(password.clone())))),
        state: Arc::new(store.clone()),
        project_dir: root.clone(),
        backups: dir.path().join("backups"),
        now: || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
        retry_pause: Duration::from_secs(1),
    };
    let current = || store.environments("site").unwrap().into_iter().next().unwrap();

    // 1. Compare the whole folder with an empty project: every file is on the server only.
    let started = Instant::now();
    let (mut lines, yes) = (Lines::default(), Yes::default());
    let mut op = Operation::new(&mut lines, &yes, CancelToken::new());
    let scan = sync::scan(&deps, &env, &mut op).unwrap_or_else(|e| panic!("scan: {e}\n{:#?}", lines.0));
    println!("1. scan: {} files in {:.1?}; questions: {:?}", scan.entries.len(), started.elapsed(), yes.0.lock().unwrap());
    for l in &lines.0 {
        println!("     {l}");
    }
    assert!(!scan.entries.is_empty(), "the server listed nothing");
    let not_remote_only: Vec<_> = scan.entries.iter().filter(|e| e.change != Change::RemoteOnly).take(5).collect();
    assert!(not_remote_only.is_empty(), "an empty project: all on the server only - {not_remote_only:?}");
    let total: u64 = scan.entries.iter().filter_map(|e| e.remote.as_ref()).map(|m| m.size).sum();
    println!("   {:.1} MB on the server", total as f64 / 1e6);
    let env = current();
    assert!(env.host_key.is_some(), "the connection is trusted after the first comparison");

    // 2. One file's facts, straight from the server.
    let mut fs = deps.remote.connect(&env).expect("connect");
    let probe = scan.entries.iter().find(|e| e.path.eq_ignore_ascii_case("web.config")).or(scan.entries.first()).unwrap();
    let meta = fs.stat(&format!("{}/{}", env.remote_root, probe.path)).expect("stat").expect("the file is there");
    println!("2. stat {}: {} bytes, mtime {}", probe.path, meta.size, meta.mtime);
    assert_eq!(Some(meta.size), probe.remote.as_ref().map(|m| m.size));
    drop(fs);

    // 3. Download a few small files (style sheets and scripts first - no secrets) into the project, after a preview.
    let mut small: Vec<&_> = scan
        .entries
        .iter()
        .filter(|e| e.remote.as_ref().is_some_and(|m| m.size > 0 && m.size < 50_000))
        .filter(|e| e.path.ends_with(".css") || e.path.ends_with(".js"))
        .take(5)
        .collect();
    if small.is_empty() {
        small = scan.entries.iter().filter(|e| e.remote.as_ref().is_some_and(|m| m.size < 50_000)).take(5).collect();
    }
    let items: Vec<TransferItem> = small.iter().map(|e| TransferItem::from_entry(e, Action::Download, false)).collect();
    let preview = plan::preview(&env, &items);
    println!("3. preview: {} to download, {} bytes, refused {:?}", preview.planned.len(), preview.bytes(), preview.refused);
    let started = Instant::now();
    let (mut lines, yes) = (Lines::default(), Yes::default());
    let mut op = Operation::new(&mut lines, &yes, CancelToken::new());
    sync::transfer(&deps, &env, &items, true, &mut op).unwrap_or_else(|e| panic!("download: {e}\n{:#?}", lines.0));
    println!("   downloaded in {:.1?}", started.elapsed());
    for item in &items {
        let local = item.path.split('/').fold(root.clone(), |p, s| p.join(s));
        let len = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
        println!("     {} - {} bytes", item.path, len);
        assert_eq!(Some(len), item.remote.as_ref().map(|m| m.size), "{}", item.path);
    }

    // 4. Compare again: the downloaded files are the same on both sides now.
    let (mut lines, yes) = (Lines::default(), Yes::default());
    let mut op = Operation::new(&mut lines, &yes, CancelToken::new());
    let again = sync::scan(&deps, &current(), &mut op).unwrap_or_else(|e| panic!("scan: {e}\n{:#?}", lines.0));
    let unchanged: Vec<&str> =
        again.entries.iter().filter(|e| e.change == Change::Unchanged).map(|e| e.path.as_str()).collect();
    let others: Vec<(&str, Change)> = again
        .entries
        .iter()
        .filter(|e| items.iter().any(|i| i.path == e.path) && e.change != Change::Unchanged)
        .map(|e| (e.path.as_str(), e.change))
        .collect();
    println!("4. scan again: {} unchanged ({unchanged:?}); downloaded but not unchanged: {others:?}", unchanged.len());
    assert!(others.is_empty());

    // 5. The last comparison, from the cache, without connecting.
    let (at, cached) = sync::cached(&deps, &current(), &CancelToken::new()).unwrap().expect("a saved comparison");
    println!("5. cached: {} files, saved at {at}", cached.len());
    assert_eq!(cached.len(), again.entries.len());

    // 6. Both copies of a downloaded file, as the diff reads them.
    let path = &items[0].path;
    let (remote, local) = sync::read_both(&deps, &current(), path, &CancelToken::new()).unwrap();
    println!("6. read both copies of {path}: equal = {}", remote == local);
    assert_eq!(remote, local);
}
