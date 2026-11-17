//! The service against a web server of the test's own on 127.0.0.1 and a database in a temporary folder - the
//! service tests of `KeepWarmTests.cs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dnnm_application::iis::{IisBinding, IisServerState, IisSiteRuntime};

use crate::records::tests::database;
use crate::requester::tests::{Answer, TinyServer, answer};
use crate::{
    KeepWarmConfig, KeepWarmEvent, KeepWarmNotice, KeepWarmPause, KeepWarmRecords, KeepWarmService, KeepWarmSettings,
    KeepWarmSite, KeepWarmState,
};

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(30), "Timed out waiting for {what}.");
        thread::sleep(Duration::from_millis(20));
    }
}

/// What mustn't happen: `condition` keeps holding for 300 ms.
fn stays(what: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        assert!(condition(), "{what}");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(condition(), "{what}");
}

fn site(name: &str, port: u16, state: &str, workers: Vec<u32>) -> KeepWarmSite {
    KeepWarmSite {
        name: name.into(),
        site: IisSiteRuntime {
            id: 1,
            state: state.into(),
            app_pool: name.into(),
            app_pool_state: Some(state.into()),
            worker_process_ids: workers,
            bindings: vec![IisBinding::parse("http", &format!("*:{port}:{name}.dnndev.me"), false)],
            physical_path: PathBuf::from(format!(r"C:\DNN\{name}")),
        },
        idle_timeout: Some(Duration::from_secs(20 * 60)),
        database_name: None,
        database_server: None,
        database_is_file: false,
    }
}

struct Fixture {
    service: KeepWarmService,
    events: Arc<Mutex<Vec<KeepWarmEvent>>>,
    syncs: Arc<AtomicUsize>,
}

fn start(records: &KeepWarmRecords) -> Fixture {
    let events = Arc::new(Mutex::new(Vec::new()));
    let syncs = Arc::new(AtomicUsize::new(0));
    let mut config = KeepWarmConfig::new(records.path(), KeepWarmSettings::default());
    config.start_up_delay = Duration::ZERO;
    let seen = events.clone();
    config.on_event = Some(Box::new(move |e| seen.lock().unwrap().push(e)));
    let counted = syncs.clone();
    config.sync_sites = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    }));
    Fixture { service: KeepWarmService::start(config), events, syncs }
}

impl Fixture {
    fn notices(&self) -> Vec<KeepWarmNotice> {
        let events = self.events.lock().unwrap();
        events.iter().filter_map(|e| if let KeepWarmEvent::Notice(n) = e { Some(n.clone()) } else { None }).collect()
    }

    fn wait_for(&self, site: &str, state: KeepWarmState) {
        wait_until(&format!("{site} to be {state:?} ({})", self.service.status_of(site).text), || {
            self.service.status_of(site).state == state
        });
    }
}

#[test]
fn keeps_a_site_warm_and_holds_back_when_it_should() {
    let keep_alive_missing = Arc::new(AtomicBool::new(false));
    let failing = Arc::new(AtomicBool::new(false));
    let (missing, fails) = (keep_alive_missing.clone(), failing.clone());
    let server = TinyServer::start(move |path| match path {
        "/KeepAlive.aspx" if fails.load(Ordering::SeqCst) => answer(500),
        "/KeepAlive.aspx" if !missing.load(Ordering::SeqCst) => {
            Answer { status: 200, body: "ok".into(), ..Default::default() }
        }
        "/" => Answer { status: 200, body: "<html>home</html>".into(), ..Default::default() },
        _ => answer(404),
    });
    let dir = tempfile::tempdir().unwrap();
    let records = database(&dir);
    let f = start(&records);
    let s = &f.service;
    let worker = std::process::id();
    let shop = |state: &str, workers: Vec<u32>| vec![site("shop", server.port, state, workers)];

    s.set_iis_state(IisServerState::Running);
    s.set_sites(shop("Started", vec![worker]));
    stays("Not kept warm: nothing is sent.", || s.status_of("shop").state == KeepWarmState::Off && server.count() == 0);

    // Switched on: it has a worker process, so its keep-alive page - at once.
    s.set_enabled("shop", true);
    f.wait_for("shop", KeepWarmState::Warm);
    assert_eq!(server.paths(), vec!["/KeepAlive.aspx"]);
    assert_eq!(server.seen()[0].host, Some(format!("shop.dnndev.me:{}", server.port)));
    assert_eq!(records.find("shop").as_deref(), Some("shop"), "Remembered.");
    assert!(f.notices().iter().any(|n| n.message.starts_with("'shop' is warm - it answered in ")));
    assert!(s.status_of("shop").text.starts_with("Warm - answered in "), "{}", s.status_of("shop").text);

    // Stopped: paused, and nothing is sent.
    s.set_sites(shop("Stopped", vec![]));
    f.wait_for("shop", KeepWarmState::Paused);
    // The test's app pool stops with its site.
    assert_eq!(s.status_of("shop").text, "Paused - the site's app pool is stopped");

    // Started again without a worker process: warmed up with its home page - once.
    s.set_sites(shop("Started", vec![]));
    wait_until("the warm-up request", || server.count() >= 2);
    f.wait_for("shop", KeepWarmState::Warm);
    stays("Warmed up once.", || server.count() == 2);
    assert_eq!(server.paths(), vec!["/KeepAlive.aspx", "/"]);
    wait_until("the sites to be read again", || f.syncs.load(Ordering::SeqCst) == 1);
    s.set_sites(shop("Started", vec![worker]));
    stays("The worker process the warm-up started is its own - no check of it.", || server.count() == 2);

    // An operation on the site holds it back; one that never began changes nothing.
    s.pause(KeepWarmPause::for_sites(vec!["shop".into()]));
    f.wait_for("shop", KeepWarmState::Paused);
    assert_eq!(s.status_of("shop").text, "Paused - an operation on the site is running");
    s.resume();
    f.wait_for("shop", KeepWarmState::Warm);
    stays("Nothing to check after an operation that never began.", || server.count() == 2);
    // One that ran gets it checked again once it has ended - not even Check now goes before.
    s.pause(KeepWarmPause::for_sites(vec!["shop".into()]));
    f.wait_for("shop", KeepWarmState::Paused);
    s.check_now("shop");
    stays("Not even Check now while the operation runs.", || server.count() == 2);
    s.recheck(Some(vec!["shop".into()]));
    s.resume();
    wait_until("the recheck", || server.count() == 3);
    assert_eq!(server.paths()[2], "/KeepAlive.aspx");

    // Its keep-alive page goes missing: one 404 only brings a warm-up; a second one in a row means it isn't there.
    keep_alive_missing.store(true, Ordering::SeqCst);
    s.check_now("shop");
    wait_until("a 404 and a warm-up", || server.count() == 5);
    assert_eq!(server.paths()[3..], ["/KeepAlive.aspx", "/"]);
    assert!(!f.notices().iter().any(|n| n.message.contains("has no")), "Not after one 404.");
    s.set_sites(shop("Started", vec![worker, 4]));
    wait_until("a second 404 and a warm-up", || server.count() == 7);
    wait_until("the notice", || {
        f.notices()
            .iter()
            .any(|n| n.message == "'shop' has no /KeepAlive.aspx - keep warm requests its warm-up page instead.")
    });

    // It fails while someone else's operation runs: not counted - tried again once that has ended.
    keep_alive_missing.store(false, Ordering::SeqCst);
    failing.store(true, Ordering::SeqCst);
    s.pause(KeepWarmPause::warm_ups());
    let sent = server.count();
    s.check_now("shop");
    wait_until("the failing request", || server.count() > sent);
    f.wait_for("shop", KeepWarmState::Waiting);
    stays("Not again while the operation runs.", || server.count() == sent + 1);
    assert!(!f.notices().iter().any(|n| n.is_warning), "Not counted as a failure.");
    failing.store(false, Ordering::SeqCst);
    s.resume();
    f.wait_for("shop", KeepWarmState::Warm);
    wait_until("the request after the operation", || server.count() >= sent + 2);

    // Gone from IIS: its record goes with it - a new site of the same name starts cold.
    s.set_sites(vec![]);
    wait_until("the record to go", || records.find("shop").is_none());
    wait_until("the status to be off", || s.status_of("shop").state == KeepWarmState::Off);
}

#[test]
fn forgets_sites_removed_from_iis_while_it_was_closed() {
    let dir = tempfile::tempdir().unwrap();
    let records = database(&dir);
    records.save("shop", true);
    records.save("gone", true);
    let f = start(&records);
    assert_eq!(f.service.status_of("gone").state, KeepWarmState::Waiting, "Kept warm, as far as it knows yet.");
    let mut shop = site("shop", 1, "Started", vec![]);
    shop.site.bindings.clear();
    f.service.set_iis_state(IisServerState::Running);
    f.service.set_sites(vec![shop]);
    wait_until("the record of the site gone from IIS to go", || records.find("gone").is_none());
    assert!(records.find("shop").is_some(), "A site that is there keeps its record.");
    assert_eq!(f.service.status_of("gone").state, KeepWarmState::Off);
    f.wait_for("shop", KeepWarmState::Paused);
    assert_eq!(f.service.status_of("shop").text, "Paused - the site has no http or https binding to request");
}

#[test]
fn keeps_each_site_on_its_own_once_each_and_again_after_a_restart() {
    let down = Arc::new(AtomicBool::new(false));
    let is_down = down.clone();
    let server = TinyServer::start(move |path| match path {
        "/KeepAlive.aspx" if is_down.load(Ordering::SeqCst) => answer(503),
        "/KeepAlive.aspx" => Answer { status: 200, body: "ok".into(), ..Default::default() },
        _ => answer(404),
    });
    let dir = tempfile::tempdir().unwrap();
    let records = database(&dir);
    let worker = std::process::id();
    let sites = || ["a", "b", "c"].iter().map(|n| site(n, server.port, "Started", vec![worker])).collect::<Vec<_>>();
    let sent_to = |name: &str| {
        let host = format!("{name}.dnndev.me:{}", server.port);
        server.seen().iter().filter(|s| s.host.as_deref() == Some(host.as_str())).count()
    };

    let f = start(&records);
    let s = &f.service;
    s.set_iis_state(IisServerState::Running);
    s.set_sites(sites());
    // A switched on three times over: still one site, one request.
    s.set_enabled("a", true);
    s.set_enabled("a", true);
    s.set_enabled("c", true);
    s.set_enabled("a", true);
    f.wait_for("a", KeepWarmState::Warm);
    f.wait_for("c", KeepWarmState::Warm);
    wait_until("A and C to be requested", || sent_to("a") >= 1 && sent_to("c") >= 1);
    stays("One request each, none to B.", || sent_to("a") == 1 && sent_to("c") == 1 && sent_to("b") == 0);
    assert_eq!(s.status_of("b").state, KeepWarmState::Off);

    // A goes down for a while: failing, not given up on - and warm again once it answers.
    down.store(true, Ordering::SeqCst);
    s.check_now("a");
    f.wait_for("a", KeepWarmState::Failing);
    let text = s.status_of("a").text;
    assert!(
        text.starts_with(
            "Failing - the site answers 503 Service Unavailable - its app pool may be stopping or failing. Tried again at "
        ),
        "{text}"
    );
    assert!(f.notices().iter().any(|n| n.is_warning && n.message.ends_with("Tried again in 1 minute.")));
    down.store(false, Ordering::SeqCst);
    s.check_now("a");
    f.wait_for("a", KeepWarmState::Warm);
    assert_eq!(s.status_of("c").state, KeepWarmState::Warm, "C goes on as before.");

    // C off: nothing more is sent to it, not even when asked.
    s.set_enabled("c", false);
    f.wait_for("c", KeepWarmState::Off);
    let to_c = sent_to("c");
    s.check_now("c");
    stays("Switched off: nothing is sent.", || sent_to("c") == to_c);
    assert_eq!(records.find("c"), None);
    drop(f);

    // DNN Manager closed and opened again: A is kept warm again, B and C aren't.
    let (a, b, c) = (sent_to("a"), sent_to("b"), sent_to("c"));
    let restarted = start(&records);
    assert_eq!(restarted.service.status_of("a").state, KeepWarmState::Waiting, "Remembered before IIS was read.");
    restarted.service.set_iis_state(IisServerState::Running);
    restarted.service.set_sites(sites());
    wait_until("A to be requested after the restart", || sent_to("a") > a);
    restarted.wait_for("a", KeepWarmState::Warm);
    stays("B and C aren't kept warm after the restart.", || sent_to("b") == b && sent_to("c") == c);
    assert_eq!(restarted.service.status_of("c").state, KeepWarmState::Off);
}
