//! The keep-warm service - `KeepWarmService.cs`. Keeps the sites switched to "keep warm" warm while DNN Manager runs:
//! requests each one before IIS would shut its worker process down for being idle, and warms it up again as soon as
//! that process is gone (a recycle, a crash, IIS or the site started again).
//!
//! **What it costs.** One thread that sleeps until the next request is due - no timer at all while no site is kept
//! warm - and a short-lived thread per request (at most [`rules::MAX_IN_FLIGHT`] at once). A request is DNN's tiny
//! `KeepAlive.aspx`, sent no more often than the site's app pool needs - and not while the site is in use anyway (IIS's
//! request counter went up meanwhile). A full warm-up only goes to a site without a worker process, one at a time, or to
//! one whose keep-alive answer showed DNN had to start.
//!
//! **When it holds back.** Nothing is sent to a site that is stopped, while IIS is stopped, while one of DNN Manager's
//! operations runs on the site (or on IIS itself), while a debugger is attached to the site's worker process, or - for a
//! warm-up - while the SQL Server its database is on doesn't answer. A site that keeps failing is tried again after 1, 2
//! and 5 minutes, then left alone until it is started again, Check now is pressed, the settings change or the PC wakes.
//!
//! **How it works.** One loop owns all state; everything else posts messages to it: IIS's state and the sites as the
//! app reads them, the PC waking up, operations starting and ending, the user switching a site on or off, the settings
//! changing, and the requests' answers. Each site has at most one request on its way; an answer that comes back after
//! what it was for changed is dropped without counting.
//!
//! Where the port differs from the C#: the app hands over complete reads of the sites ([`KeepWarmService::set_sites`])
//! instead of the monitor's events - the service tells added, changed and removed sites apart itself; IIS's request
//! counters and "read the sites again now" are closures the app passes in ([`KeepWarmConfig`]); a request running when
//! the service stops isn't cancelled - its thread ends with the request's own time limit and its answer is dropped.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use dnnm_application::iis::{IisServerState, IisSiteRuntime, states};
use dnnm_application::project::ProjectState;

use crate::debugger;
use crate::local_time;
use crate::model::{
    KeepWarmNotice, KeepWarmOutcome, KeepWarmOutcomeKind, KeepWarmPause, KeepWarmRequestKind, KeepWarmState,
    KeepWarmStatus,
};
use crate::plan::{KeepWarmPlan, LocalSiteTarget};
use crate::records::KeepWarmRecords;
use crate::requester::{KeepWarmRequester, sql_answers};
use crate::rules;
use crate::settings::KeepWarmSettings;
use crate::sql_address::SqlServerAddress;

const NEVER: i64 = i64::MAX;
// How long a site waits before a debugger or a failure an operation may have caused is looked at again.
const RECHECK_AFTER: Duration = Duration::from_secs(30);
const SQL_RECHECK_AFTER: Duration = Duration::from_secs(60);
// A worker process turning up this soon after a successful request was started by that request.
const OWN_WORKER_WITHIN: Duration = Duration::from_secs(60);
// Changes to a site this soon after an operation ended are that operation's doing.
const OPERATION_LINGER: Duration = Duration::from_secs(30);
// The request counters are read once for all sites whose request is due at about the same time.
const COUNTERS_FRESH_FOR: Duration = Duration::from_secs(5);
// The loop never spins: when something is due "now" but nothing could be done, it looks again after this.
const MIN_WAIT: Duration = Duration::from_millis(250);

fn ms(span: Duration) -> i64 {
    span.as_millis() as i64
}

fn key(site: &str) -> String {
    site.to_lowercase()
}

/// One IIS site as the app knows it - what keep warm looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepWarmSite {
    pub name: String,
    /// State, app pool state, worker processes and bindings.
    pub site: IisSiteRuntime,
    /// The app pool's idle time-out: None when unknown, zero when IIS never shuts its worker process down.
    pub idle_timeout: Option<Duration>,
    /// From web.config: a warm-up waits for the SQL Server the site's database is on.
    pub database_name: Option<String>,
    pub database_server: Option<String>,
    pub database_is_file: bool,
}

impl KeepWarmSite {
    pub fn from_project(project: &ProjectState, idle_timeout: Option<Duration>) -> Self {
        Self {
            name: project.name.clone(),
            site: project.site.clone(),
            idle_timeout,
            database_name: project.database_name.clone(),
            database_server: project.database_server.clone(),
            database_is_file: project.database_is_file,
        }
    }
}

/// What the service tells the app - on the service's own thread: a handler hands it on and returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepWarmEvent {
    /// A site's status changed.
    Status { site: String, status: KeepWarmStatus },
    /// Something worth a line in the activity log.
    Notice(KeepWarmNotice),
}

/// IIS's "Total Method Requests" per site name - what tells whether a site is in use.
pub type RequestsServed = Arc<dyn Fn() -> HashMap<String, u64> + Send + Sync>;

/// How the service is started.
pub struct KeepWarmConfig {
    /// `dnnmanager.db`: the `keep_warm` records.
    pub db_path: std::path::PathBuf,
    pub settings: KeepWarmSettings,
    /// How long after the start sites without a worker process are first warmed up (a minute; tests make it shorter).
    pub start_up_delay: Duration,
    /// Status changes and notices.
    pub on_event: Option<Box<dyn Fn(KeepWarmEvent) + Send>>,
    /// IIS's request counters; None: a site in use is still requested.
    pub requests_served: Option<RequestsServed>,
    /// Asks the app to read the sites again now (a warm-up started a worker process it doesn't know of yet). Called on
    /// a thread of its own.
    pub sync_sites: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl KeepWarmConfig {
    pub fn new(db_path: impl Into<std::path::PathBuf>, settings: KeepWarmSettings) -> Self {
        Self {
            db_path: db_path.into(),
            settings,
            start_up_delay: rules::START_UP_DELAY,
            on_event: None,
            requests_served: None,
            sync_sites: None,
        }
    }
}

// What other threads read: the records and the statuses, by lower-cased site name.
struct Shared {
    records: RwLock<HashMap<String, String>>,
    statuses: RwLock<HashMap<String, KeepWarmStatus>>,
    stopping: AtomicBool,
    throttled: AtomicBool,
}

/// The keep-warm service: a handle on its thread. Stops when dropped (or [`Self::stop`]).
pub struct KeepWarmService {
    tx: Sender<Message>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl KeepWarmService {
    /// Loads which sites are kept warm and starts the loop. Feed it with [`Self::set_iis_state`] and
    /// [`Self::set_sites`].
    pub fn start(config: KeepWarmConfig) -> Self {
        let store = KeepWarmRecords::new(&config.db_path);
        let shared = Arc::new(Shared {
            records: RwLock::new(store.list().into_iter().map(|s| (key(&s), s)).collect()),
            statuses: RwLock::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            throttled: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel();
        let mut core = Core::new(config, store, shared.clone(), tx.clone());
        let thread = thread::Builder::new()
            .name("keep warm".into())
            .spawn(move || core.run(rx))
            .expect("the keep-warm thread starts");
        Self { tx, shared, thread: Mutex::new(Some(thread)) }
    }

    /// How `site`'s keep warm stands now.
    pub fn status_of(&self, site: &str) -> KeepWarmStatus {
        let k = key(site);
        if let Some(status) = self.shared.statuses.read().ok().and_then(|s| s.get(&k).cloned()) {
            return status;
        }
        if self.shared.records.read().is_ok_and(|r| r.contains_key(&k)) {
            KeepWarmStatus::waiting()
        } else {
            KeepWarmStatus::off()
        }
    }

    /// Whether `site` is kept warm (has a record).
    pub fn is_kept_warm(&self, site: &str) -> bool {
        self.shared.records.read().is_ok_and(|r| r.contains_key(&key(site)))
    }

    /// Switches keep warm on or off for `site` (and remembers it). Switched on, it is checked at once. The status
    /// reads "Waiting" or "Off" at once - as the WPF app's row does - and the service's events follow.
    pub fn set_enabled(&self, site: &str, enabled: bool) {
        if let Ok(mut statuses) = self.shared.statuses.write() {
            statuses.insert(key(site), if enabled { KeepWarmStatus::waiting() } else { KeepWarmStatus::off() });
        }
        self.post(Message::Enable(site.to_string(), enabled));
    }

    /// Requests `site` now - also after it failed too often - and shows how it answers.
    pub fn check_now(&self, site: &str) {
        self.post(Message::CheckNow(site.to_string()));
    }

    /// One of DNN Manager's operations runs: what it may be changing is held back until [`Self::resume`]. Another call
    /// replaces what is held back.
    pub fn pause(&self, pause: KeepWarmPause) {
        self.post(Message::Pause(Some(pause)));
    }

    /// The operation has ended and what it changed has been read: keep warm goes on as before.
    pub fn resume(&self) {
        self.post(Message::Pause(None));
    }

    /// What an operation was about - `sites`, or every site when None - is checked again at once, with a new chance.
    pub fn recheck(&self, sites: Option<Vec<String>>) {
        self.post(Message::Recheck(sites));
    }

    /// The PC woke up: the workers may be about to idle out, and everything may have changed - all checked again.
    pub fn resumed_from_sleep(&self) {
        self.post(Message::Resumed);
    }

    /// Settings → Projects → Keep warm changed: checked with the new pages and interval right away.
    pub fn set_settings(&self, settings: KeepWarmSettings) {
        self.post(Message::Settings(settings));
    }

    /// IIS itself: running, stopped, not installed…
    pub fn set_iis_state(&self, state: IisServerState) {
        self.post(Message::Iis(state));
    }

    /// A complete, successful read of IIS's sites. A site missing from it is gone from IIS: its record goes too (unless
    /// IIS isn't installed). Don't call it with a read that failed.
    pub fn set_sites(&self, sites: Vec<KeepWarmSite>) {
        self.post(Message::Sites(sites));
    }

    /// DNN Manager is in efficiency mode: answers measured meanwhile can be slower than the site was.
    pub fn set_throttled(&self, throttled: bool) {
        self.shared.throttled.store(throttled, Ordering::Relaxed);
    }

    /// Stops the loop and waits for it. Requests on their way end with their own time limit; their answers are dropped.
    pub fn stop(&self) {
        if self.shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.tx.send(Message::Stop);
        if let Some(thread) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = thread.join();
        }
    }

    fn post(&self, message: Message) {
        let _ = self.tx.send(message);
    }
}

impl Drop for KeepWarmService {
    fn drop(&mut self) {
        self.stop();
    }
}

// ─── Messages ─────────────────────────────────────────────────────────

enum Message {
    Sites(Vec<KeepWarmSite>),
    Iis(IisServerState),
    Resumed,
    Pause(Option<KeepWarmPause>),
    Recheck(Option<Vec<String>>),
    Enable(String, bool),
    Settings(KeepWarmSettings),
    CheckNow(String),
    Completed(Box<Request>, KeepWarmOutcome),
    Stop,
}

/// A request on its way, with what it was sent for.
#[derive(Clone)]
struct Request {
    site: String,
    generation: u64,
    epoch: u64,
    kind: KeepWarmRequestKind,
    cold: bool,
    target: LocalSiteTarget,
    path: String,
    sql: Option<(String, u16)>,
    check_in_use: bool,
    baseline: Option<u64>,
    own: u32,
}

// Generations come from one counter for every site: a site removed and added again under its name never has a
// generation the old one had.
static GENERATIONS: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATIONS.fetch_add(1, Ordering::Relaxed) + 1
}

struct Site {
    name: String,
    /// The site as the app last reported it; None until it has.
    project: Option<KeepWarmSite>,
    /// Changed whenever an answer on its way can no longer tell anything (stopped, switched off).
    generation: u64,
    in_flight: Option<KeepWarmRequestKind>,
    /// A request as soon as it may be sent: switched on, started, resumed, Check now…
    probe_wanted: bool,
    /// The warm-up page next: the keep-alive page answered slowly (DNN had to start), or doesn't exist.
    warm_up_wanted: bool,
    /// The user asked (switched it on, Check now): no waiting for the start-up delay or a failure's backoff.
    user_asked: bool,
    failures: u32,
    stopped: bool,
    failure_reason: Option<String>,
    /// Nothing is sent before this (a failure's backoff, a debugger or the database looked at again).
    not_before: i64,
    wait_status: Option<KeepWarmStatus>,
    /// Its worker process ended soon after DNN started, this many times in a row - a crashing site.
    early_deaths: u32,
    /// It failed while an operation ran - maybe the operation's doing: tried again once that has ended.
    retry_after_operation: bool,
    /// The worker processes the last answer came from.
    warm_pids: Vec<u32>,
    /// A warm-up of a site without a worker process started one, which the app hasn't reported yet.
    assume_worker_until: Option<i64>,
    /// When the last request was sent, or the site was last found in use - the interval counts from then.
    last_request_at: Option<i64>,
    last_ok_at: Option<i64>,
    /// When DNN last started for one of its requests (a cold warm-up, or a slow answer).
    last_warm_up_ok_at: Option<i64>,
    last_elapsed: Duration,
    last_ok_local: Option<SystemTime>,
    last_note: Option<String>,
    in_use: bool,
    /// The site's request counter at the last look, and how many requests were sent since.
    baseline: Option<u64>,
    own: u32,
    /// The keep-alive page answered 404 this many times in a row - twice, and it is taken to be missing.
    ping_misses: u32,
    ping_path_missing: bool,
    /// Has answered since it was switched on - the first answer is worth a line in the log.
    was_warm: bool,
    status: KeepWarmStatus,
}

impl Site {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            project: None,
            generation: next_generation(),
            in_flight: None,
            probe_wanted: false,
            warm_up_wanted: false,
            user_asked: false,
            failures: 0,
            stopped: false,
            failure_reason: None,
            not_before: 0,
            wait_status: None,
            early_deaths: 0,
            retry_after_operation: false,
            warm_pids: Vec::new(),
            assume_worker_until: None,
            last_request_at: None,
            last_ok_at: None,
            last_warm_up_ok_at: None,
            last_elapsed: Duration::ZERO,
            last_ok_local: None,
            last_note: None,
            in_use: false,
            baseline: None,
            own: 0,
            ping_misses: 0,
            ping_path_missing: false,
            was_warm: false,
            status: KeepWarmStatus::off(),
        }
    }

    fn rearm(&mut self) {
        self.failures = 0;
        self.stopped = false;
        self.failure_reason = None;
        self.not_before = 0;
        self.wait_status = None;
    }

    /// Checks the site as soon as it may be - when `rearm`, with a new chance: its failures, a missing keep-alive page
    /// and quick ends of its worker process are forgotten.
    fn recheck(&mut self, rearm: bool) {
        if rearm {
            self.rearm();
            self.ping_misses = 0;
            self.ping_path_missing = false;
            self.early_deaths = 0;
        }
        self.probe_wanted = true;
    }
}

// Started, and its app pool not stopped: it can answer.
fn running(site: &IisSiteRuntime) -> bool {
    states::is_started(&site.state)
        && !matches!(site.app_pool_state.as_deref(), Some(states::STOPPED) | Some(states::STOPPING))
}

// ─── The loop ─────────────────────────────────────────────────────────

struct Core {
    shared: Arc<Shared>,
    store: KeepWarmRecords,
    tx: Sender<Message>,
    requester: Arc<KeepWarmRequester>,
    on_event: Option<Box<dyn Fn(KeepWarmEvent) + Send>>,
    counters: Option<Arc<Counters>>,
    sync_sites: Option<Arc<dyn Fn() + Send + Sync>>,
    start_up_delay: Duration,
    epoch: Instant,

    sites: HashMap<String, Site>,
    /// The sites the last read of IIS had.
    known: HashSet<String>,
    settings: KeepWarmSettings,
    iis_state: Option<IisServerState>,
    /// What the operation running now holds back; None while none runs.
    pause: Option<KeepWarmPause>,
    /// Counts the operations: an answer to a request sent before the latest one began says little about now.
    pause_epoch: u64,
    started_at: i64,
    operation_ended_at: Option<i64>,
    in_flight: usize,
    cold_in_flight: usize,
    /// The records of sites that were gone from IIS when DNN Manager started have been dropped.
    records_reconciled: bool,
}

/// IIS's request counters, read once for the sites whose request is due at about the same time.
struct Counters {
    read: RequestsServed,
    last: Mutex<Option<(Instant, HashMap<String, u64>)>>,
}

impl Counters {
    fn get(&self) -> Option<HashMap<String, u64>> {
        // One read at a time: sites that fall due together wait for the first one's read, and use it.
        let mut last = self.last.lock().ok()?;
        if let Some((at, counters)) = last.as_ref()
            && at.elapsed() < COUNTERS_FRESH_FOR
        {
            return (!counters.is_empty()).then(|| counters.clone());
        }
        let counters: HashMap<String, u64> = (self.read)().into_iter().map(|(k, v)| (key(&k), v)).collect();
        *last = Some((Instant::now(), counters.clone()));
        (!counters.is_empty()).then_some(counters)
    }
}

impl Core {
    fn new(config: KeepWarmConfig, store: KeepWarmRecords, shared: Arc<Shared>, tx: Sender<Message>) -> Self {
        Self {
            shared,
            store,
            tx,
            requester: Arc::new(KeepWarmRequester::new()),
            on_event: config.on_event,
            counters: config.requests_served.map(|read| Arc::new(Counters { read, last: Mutex::new(None) })),
            sync_sites: config.sync_sites,
            start_up_delay: config.start_up_delay,
            epoch: Instant::now(),
            sites: HashMap::new(),
            known: HashSet::new(),
            settings: config.settings,
            iis_state: None,
            pause: None,
            pause_epoch: 0,
            started_at: 0,
            operation_ended_at: None,
            in_flight: 0,
            cold_in_flight: 0,
            records_reconciled: false,
        }
    }

    fn now(&self) -> i64 {
        ms(self.epoch.elapsed())
    }

    fn run(&mut self, rx: Receiver<Message>) {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(Message::Stop) => return,
                    Ok(message) => self.handle(message),
                    Err(_) => break,
                }
            }
            let wake = self.pump();
            // Nothing due: wait for the next message, however long that takes - no timer ticks for nothing.
            let message = if wake == NEVER {
                match rx.recv() {
                    Ok(m) => m,
                    Err(_) => return,
                }
            } else {
                let wait = Duration::from_millis((wake - self.now()).max(ms(MIN_WAIT)) as u64);
                match rx.recv_timeout(wait) {
                    Ok(m) => m,
                    // Something is due.
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            if matches!(message, Message::Stop) {
                return;
            }
            self.handle(message);
        }
    }

    fn handle(&mut self, message: Message) {
        let result = catch_unwind(AssertUnwindSafe(|| match message {
            Message::Sites(sites) => self.on_sites(sites),
            Message::Iis(state) => self.on_iis(state),
            Message::Resumed => {
                // After sleep the workers may be about to idle out, and everything may have changed: check them all.
                for k in self.enabled_sites() {
                    self.site_mut(&k).recheck(true);
                }
            }
            Message::Pause(pause) => self.on_pause(pause),
            Message::Recheck(sites) => {
                let wanted: Option<HashSet<String>> = sites.map(|s| s.iter().map(|n| key(n)).collect());
                for k in self.enabled_sites() {
                    if wanted.as_ref().is_none_or(|w| w.contains(&k)) {
                        self.site_mut(&k).recheck(true);
                    }
                }
            }
            Message::Enable(site, enabled) => self.on_enable(&site, enabled),
            Message::Settings(settings) => self.on_settings_changed(settings),
            Message::CheckNow(site) => {
                let k = key(&site);
                if self.sites.contains_key(&k) && self.is_enabled(&k) {
                    let site = self.site_mut(&k);
                    site.recheck(true);
                    site.user_asked = true;
                }
            }
            Message::Completed(request, outcome) => self.on_completed(*request, outcome),
            Message::Stop => {}
        }));
        if result.is_err() {
            log::error!("Keep warm couldn't handle a message");
        }
    }

    fn site_mut(&mut self, k: &str) -> &mut Site {
        self.sites.get_mut(k).expect("a site that is there")
    }

    fn get_or_add(&mut self, name: &str) -> &mut Site {
        self.sites.entry(key(name)).or_insert_with(|| Site::new(name))
    }

    fn is_enabled(&self, k: &str) -> bool {
        self.shared.records.read().is_ok_and(|r| r.contains_key(k))
    }

    fn enabled_sites(&self) -> Vec<String> {
        self.sites.keys().filter(|k| self.is_enabled(k)).cloned().collect()
    }

    /// The running operation holds the site back entirely: it is about the site, or about IIS itself.
    fn held_back(&self, site: &Site) -> bool {
        self.pause.as_ref().is_some_and(|p| {
            p.everything
                || p.sites
                    .as_ref()
                    .is_some_and(|s| s.iter().any(|n| n.eq_ignore_ascii_case(&site.name) || key(n) == key(&site.name)))
        })
    }

    // An operation ended this recently: what changes now may be its doing.
    fn recently_operated(&self, now: i64) -> bool {
        self.operation_ended_at.is_some_and(|ended| now - ended < ms(OPERATION_LINGER))
    }

    // ─── What changed ─────────────────────────────────────────────────

    fn on_iis(&mut self, state: IisServerState) {
        let was = self.iis_state;
        self.iis_state = Some(state);
        if state != IisServerState::Running {
            for site in self.sites.values_mut() {
                site.generation = next_generation();
            }
        } else if was.is_some_and(|w| w != IisServerState::Running) {
            // IIS started again: its sites are cold.
            for k in self.enabled_sites() {
                self.site_mut(&k).recheck(true);
            }
        }
    }

    fn on_sites(&mut self, sites: Vec<KeepWarmSite>) {
        let now_known: HashSet<String> = sites.iter().map(|s| key(&s.name)).collect();
        let removed: Vec<String> = self.known.difference(&now_known).cloned().collect();
        for site in sites {
            let k = key(&site.name);
            if !self.known.contains(&k) {
                // Read again: the record may have gone with a site removed meanwhile.
                let record = self.store.find(&site.name);
                if let Ok(mut records) = self.shared.records.write() {
                    match record {
                        Some(r) => records.insert(k.clone(), r),
                        None => records.remove(&k),
                    };
                }
            }
            self.on_project(site);
        }
        for k in removed {
            self.on_removed(&k);
        }
        self.known = now_known;

        if !self.records_reconciled && self.iis_state.is_some_and(|s| s != IisServerState::NotInstalled) {
            // The first complete read of IIS: a record of a site that isn't there (removed or renamed in IIS while DNN
            // Manager was closed) goes - a new site of that name starts cold.
            self.records_reconciled = true;
            for stale in self.store.list().into_iter().filter(|s| !self.known.contains(&key(s))) {
                if let Ok(mut records) = self.shared.records.write() {
                    records.remove(&key(&stale));
                }
                if let Ok(mut statuses) = self.shared.statuses.write() {
                    statuses.remove(&key(&stale));
                }
                self.store.remove(&stale);
            }
        }
    }

    fn on_removed(&mut self, k: &str) {
        let name = match self.sites.remove(k) {
            Some(mut gone) => {
                gone.generation = next_generation();
                gone.name
            }
            None => k.to_string(),
        };
        // Gone from IIS (removed or renamed there): a new site of the same name starts cold, so its record goes too -
        // not when IIS itself isn't there (then every site reads as gone).
        let had = self.shared.records.write().ok().and_then(|mut r| r.remove(k)).is_some();
        if had && self.iis_state != Some(IisServerState::NotInstalled) {
            self.store.remove(&name);
        }
        if let Ok(mut statuses) = self.shared.statuses.write() {
            statuses.remove(k);
        }
        self.requester.forget_cookies(&name);
    }

    fn on_project(&mut self, project: KeepWarmSite) {
        let now = self.now();
        let k = key(&project.name);
        let enabled = self.is_enabled(&k);
        let died_soon_context = self.pause.is_none()
            && !self.recently_operated(now)
            && self.iis_state == Some(IisServerState::Running)
            && running(&project.site);
        let requester = self.requester.clone();
        let site = self.get_or_add(&project.name);
        let before = site.project.replace(project.clone());

        // Seen for the first time (DNN Manager started, the site added): find out how it is.
        let Some(before) = before else {
            site.probe_wanted = true;
            return;
        };

        let was_running = running(&before.site);
        let is_running = running(&project.site);
        if was_running && !is_running {
            // Stopped: what is on its way says nothing anymore; nothing is sent until it runs again.
            site.generation = next_generation();
            site.probe_wanted = false;
            site.warm_up_wanted = false;
        } else if !was_running && is_running {
            // Started (by anyone): a new chance, and a warm-up.
            site.recheck(true);
        }

        let old_pids = &before.site.worker_process_ids;
        let new_pids = &project.site.worker_process_ids;
        if old_pids == new_pids {
            return;
        }
        // What the app reports now is what there is.
        site.assume_worker_until = None;
        if new_pids.is_empty() {
            // Its sessions went with it.
            requester.forget_cookies(&site.name);
            let died_soon = enabled
                && died_soon_context
                && site.last_warm_up_ok_at.is_some_and(|at| now - at < ms(rules::DIED_SOON_AFTER));
            if !old_pids.is_empty() && died_soon {
                // Once can be a recycle; again is a site that keeps crashing - given up on like any failing site.
                site.early_deaths += 1;
                if site.early_deaths >= 2 {
                    site.failures = site.failures.max(site.early_deaths - 2);
                    let notice = fail(
                        site,
                        "its worker process keeps ending soon after it was warmed up - it may be crashing",
                        now,
                    );
                    self.emit_notice(notice);
                }
            }
            // A warm-up follows: it has no worker process now.
            return;
        }
        if site.in_flight.is_some() || *new_pids == site.warm_pids {
            return;
        }
        // The worker process our own warm-up started - the app only now saw it.
        if site.warm_pids.is_empty() && site.last_ok_at.is_some_and(|ok| now - ok < ms(OWN_WORKER_WITHIN)) {
            site.warm_pids = new_pids.clone();
            return;
        }
        // A new worker process (a recycle, someone's request): it may not have started DNN yet.
        site.probe_wanted = true;
    }

    fn on_pause(&mut self, pause: Option<KeepWarmPause>) {
        let starting = pause.is_some();
        self.pause = pause;
        if starting {
            self.pause_epoch += 1;
            return;
        }
        // What the operation was about is rechecked when the app says so (recheck) - not after an operation that never
        // began. What failed meanwhile is tried again now.
        self.operation_ended_at = Some(self.now());
        for site in self.sites.values_mut() {
            site.retry_after_operation = false;
        }
    }

    fn on_enable(&mut self, name: &str, enabled: bool) {
        self.store.save(name, enabled);
        if let Ok(mut records) = self.shared.records.write() {
            if enabled {
                records.insert(key(name), name.to_string());
            } else {
                records.remove(&key(name));
            }
        }
        let site = self.get_or_add(name);
        // Whatever is on its way was sent before.
        site.generation = next_generation();
        site.warm_up_wanted = false;
        site.was_warm = false;
        site.recheck(true);
        if enabled {
            site.user_asked = true;
        } else {
            site.probe_wanted = false;
            site.user_asked = false;
        }
    }

    fn on_settings_changed(&mut self, settings: KeepWarmSettings) {
        if settings == self.settings {
            return;
        }
        self.settings = settings;
        // Other pages, another interval: checked with them right away.
        for k in self.enabled_sites() {
            self.site_mut(&k).recheck(true);
        }
    }

    // ─── Sending ──────────────────────────────────────────────────────

    /// Sends what is due, sets every site's status, and says when something is due next.
    fn pump(&mut self) -> i64 {
        let mut wake = NEVER;
        // Asked for by the user first; then by name, so the same site doesn't always wait for the others.
        let mut order: Vec<(bool, String)> = self.sites.iter().map(|(k, s)| (s.user_asked, k.clone())).collect();
        order.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, k) in order {
            let now = self.now();
            match catch_unwind(AssertUnwindSafe(|| self.evaluate(&k, now))) {
                Ok(at) => wake = wake.min(at),
                Err(_) => log::error!("Keep warm couldn't look at {k}"),
            }
        }
        wake
    }

    /// What the site needs now: sends its request when one is due and may go. Returns when to look again.
    fn evaluate(&mut self, k: &str, now: i64) -> i64 {
        if !self.is_enabled(k) {
            self.set_status(k, KeepWarmStatus::off());
            return NEVER;
        }
        let Some(project) = self.sites[k].project.clone() else {
            self.set_status(k, KeepWarmStatus::new(KeepWarmState::Waiting, "Waiting - IIS's sites are being read"));
            return NEVER;
        };
        // Its status says what is on its way.
        if self.sites[k].in_flight.is_some() {
            return NEVER;
        }
        if let Some(blocked) = self.blocker(&project.site) {
            self.set_status(k, KeepWarmStatus::new(KeepWarmState::Paused, format!("Paused - {blocked}")));
            return NEVER;
        }
        if self.held_back(&self.sites[k]) {
            let text = if self.pause.as_ref().is_some_and(|p| p.everything) {
                "Paused - IIS is being started, stopped or restarted"
            } else {
                "Paused - an operation on the site is running"
            };
            self.set_status(k, KeepWarmStatus::new(KeepWarmState::Paused, text));
            return NEVER;
        }
        if self.sites[k].retry_after_operation && self.pause.is_some() {
            self.set_status(
                k,
                KeepWarmStatus::new(
                    KeepWarmState::Waiting,
                    "Waiting - didn't answer while an operation ran; tried again once it has ended",
                ),
            );
            return NEVER;
        }
        let site = &self.sites[k];
        if site.stopped && !site.user_asked {
            let text = format!(
                "Stopped - {} ({} times in a row). Check now, or start the site again, to try again.",
                site.failure_reason.as_deref().unwrap_or_default(),
                site.failures
            );
            self.set_status(k, KeepWarmStatus::new(KeepWarmState::Stopped, text));
            return NEVER;
        }
        if now < site.not_before && !site.user_asked {
            let not_before = site.not_before;
            if let Some(waiting) = site.wait_status.clone() {
                self.set_status(k, waiting);
            }
            return not_before;
        }

        let plan = KeepWarmPlan::new(&self.settings, &project.site, project.idle_timeout);
        let Some(target) = plan.target.clone() else {
            self.set_status(
                k,
                KeepWarmStatus::new(KeepWarmState::Paused, "Paused - the site has no http or https binding to request"),
            );
            return NEVER;
        };

        // The worker process a warm-up just started counts as there until the app says otherwise - then, if it still
        // reports none, it is warmed up again.
        let no_pids = project.site.worker_process_ids.is_empty();
        let assumed = no_pids && site.assume_worker_until.is_some_and(|until| now < until);
        let no_worker = no_pids && !assumed;
        let mut periodic = false;
        let kind = if no_worker || site.warm_up_wanted {
            KeepWarmRequestKind::WarmUp
        } else if site.probe_wanted || site.user_asked {
            KeepWarmRequestKind::Ping
        } else {
            let due =
                site.last_request_at.unwrap_or(now) + ms(plan.interval) + ms(rules::jitter(&site.name, plan.interval));
            if now < due {
                let status = warm_status(site);
                let until = site.assume_worker_until;
                self.set_status(k, status);
                return if assumed { due.min(until.unwrap_or(due)) } else { due };
            }
            periodic = true;
            KeepWarmRequestKind::Ping
        };

        // A cold start costs seconds of CPU: not while an operation runs, one at a time, and not right after DNN
        // Manager (maybe Windows) started.
        let cold = kind == KeepWarmRequestKind::WarmUp && no_worker;
        if cold && self.pause.is_some() {
            self.set_status(
                k,
                KeepWarmStatus::new(KeepWarmState::Waiting, "Waiting - warms up once the running operation has ended"),
            );
            return NEVER;
        }
        if cold && !site.user_asked {
            let allowed_at = self.started_at + ms(self.start_up_delay);
            if now < allowed_at {
                self.set_status(
                    k,
                    KeepWarmStatus::new(
                        KeepWarmState::Waiting,
                        "Waiting - warms up a minute after DNN Manager started",
                    ),
                );
                return allowed_at;
            }
            if self.cold_in_flight > 0 {
                self.set_status(k, KeepWarmStatus::new(KeepWarmState::Waiting, "Waiting - another site is warming up"));
                return NEVER;
            }
        }
        // The answers wake the loop up again.
        if self.in_flight >= rules::MAX_IN_FLIGHT {
            return NEVER;
        }

        // Someone is debugging the site: a request would stop at their breakpoints.
        if !no_worker && debugger::attached(&project.site.worker_process_ids) {
            let site = self.site_mut(k);
            site.not_before = now + ms(RECHECK_AFTER);
            let status = KeepWarmStatus::new(
                KeepWarmState::Paused,
                "Paused - a debugger is attached to the site's worker process",
            );
            site.wait_status = Some(status.clone());
            let not_before = site.not_before;
            self.set_status(k, status);
            return not_before;
        }

        self.send(k, &project, kind, cold, periodic, &plan, target, now);
        NEVER
    }

    /// Why nothing may be sent to the site now; None when something may.
    fn blocker(&self, site: &IisSiteRuntime) -> Option<&'static str> {
        match self.iis_state {
            Some(IisServerState::NotInstalled) => return Some("IIS isn't installed"),
            None | Some(IisServerState::Running) => {}
            Some(_) => return Some("IIS is stopped"),
        }
        if matches!(site.app_pool_state.as_deref(), Some(states::STOPPED) | Some(states::STOPPING)) {
            return Some("the site's app pool is stopped");
        }
        if states::is_started(&site.state) { None } else { Some("the site is stopped") }
    }

    #[allow(clippy::too_many_arguments)]
    fn send(
        &mut self,
        k: &str,
        project: &KeepWarmSite,
        mut kind: KeepWarmRequestKind,
        cold: bool,
        periodic: bool,
        plan: &KeepWarmPlan,
        target: LocalSiteTarget,
        now: i64,
    ) {
        let epoch = self.pause_epoch;
        let site = self.site_mut(k);
        // Without a keep-alive page, its warm-up page keeps it warm - asked for as a warm-up.
        if kind == KeepWarmRequestKind::Ping && site.ping_path_missing {
            kind = KeepWarmRequestKind::WarmUp;
        }
        let path = if kind == KeepWarmRequestKind::WarmUp { plan.warm_up_path.clone() } else { plan.ping_path.clone() };
        // DNN can't start without its database: a warm-up waits for the SQL Server the site's web.config connects to
        // (when it is reached over TCP).
        let sql = if cold && project.database_name.is_some() && !project.database_is_file {
            SqlServerAddress::parse(project.database_server.as_deref()).tcp_endpoint()
        } else {
            None
        };
        // Skipped while in use only when the next request still comes within the idle time-out.
        let check_in_use =
            periodic && !site.user_asked && rules::may_skip_when_in_use(&site.name, plan.interval, plan.idle_timeout);
        let request = Request {
            site: k.to_string(),
            generation: site.generation,
            epoch,
            kind,
            cold,
            target,
            path,
            sql,
            check_in_use,
            baseline: site.baseline,
            own: site.own,
        };

        site.in_flight = Some(kind);
        site.probe_wanted = false;
        site.warm_up_wanted = false;
        site.user_asked = false;
        site.last_request_at = Some(now);
        let was_warm = site.status.state == KeepWarmState::Warm;
        self.in_flight += 1;
        if cold {
            self.cold_in_flight += 1;
        }
        // A light request to a warm site says nothing new until it has answered; anything else is shown.
        if kind == KeepWarmRequestKind::WarmUp {
            let text = if cold { "Warming up - DNN is starting…" } else { "Warming up…" };
            self.set_status(k, KeepWarmStatus::new(KeepWarmState::WarmingUp, text));
        } else if !was_warm {
            self.set_status(k, KeepWarmStatus::new(KeepWarmState::WarmingUp, "Checking whether the site answers…"));
        }

        let (tx, shared, requester, counters) =
            (self.tx.clone(), self.shared.clone(), self.requester.clone(), self.counters.clone());
        let spawned = thread::Builder::new().name("keep warm request".into()).spawn(move || {
            let outcome =
                catch_unwind(AssertUnwindSafe(|| send_or_skip(&request, &requester, counters.as_deref(), &shared)))
                    .unwrap_or_else(|_| {
                        KeepWarmOutcome::failed(KeepWarmOutcomeKind::NotAnswering, "the request failed")
                    });
            // DNN Manager is closing: nobody to tell.
            if !shared.stopping.load(Ordering::Relaxed) {
                let _ = tx.send(Message::Completed(Box::new(request), outcome));
            }
        });
        if let Err(e) = spawned {
            log::warn!("A keep-warm request to {k} failed: {e}");
            let _ = self.tx.send(Message::Completed(
                Box::new(Request {
                    site: k.to_string(),
                    generation: 0,
                    epoch,
                    kind,
                    cold,
                    target: LocalSiteTarget::new("http", "127.0.0.1", 80, ""),
                    path: String::new(),
                    sql: None,
                    check_in_use: false,
                    baseline: None,
                    own: 0,
                }),
                KeepWarmOutcome::failed(KeepWarmOutcomeKind::NotAnswering, format!("the request failed: {e}")),
            ));
        }
    }

    // ─── Answers ──────────────────────────────────────────────────────

    fn on_completed(&mut self, request: Request, outcome: KeepWarmOutcome) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if request.cold {
            self.cold_in_flight = self.cold_in_flight.saturating_sub(1);
        }
        let k = request.site.clone();
        if !self.sites.contains_key(&k) {
            return;
        }
        let enabled = self.is_enabled(&k);
        let held_back = self.held_back(&self.sites[&k]);
        let iis_ok = matches!(self.iis_state, None | Some(IisServerState::Running));
        let pausing = self.pause.is_some();
        let pause_epoch = self.pause_epoch;
        let now = self.now();
        let recently_operated = self.recently_operated(now);
        let settings = self.settings.clone();
        let site = self.site_mut(&k);
        site.in_flight = None;

        // Sent before something changed that it can't tell about (the site stopped, switched off, IIS stopped), or
        // about a site an operation now holds back: tried again once things have settled, not counted.
        let project = site.project.clone();
        let Some(project) = project
            .filter(|p| request.generation == site.generation && enabled && running(&p.site) && iis_ok && !held_back)
        else {
            if enabled {
                site.probe_wanted = true;
            }
            return;
        };

        if let Some(served) = outcome.requests_served {
            site.baseline = Some(served);
            site.own = outcome.requests;
        } else if site.baseline.is_some() {
            site.own += outcome.requests;
        }

        let failed = matches!(outcome.kind, KeepWarmOutcomeKind::Failed | KeepWarmOutcomeKind::NotAnswering);
        // A failure while an operation runs may be the operation's doing: not counted, and tried again once the
        // operation has ended - not again and again meanwhile.
        if failed && pausing {
            site.retry_after_operation = true;
            site.probe_wanted = true;
            return;
        }
        // One that began before an operation, or came right after one: not counted either - looked at again soon.
        if failed && (request.epoch != pause_epoch || recently_operated) {
            site.not_before = now + ms(RECHECK_AFTER);
            site.wait_status = Some(KeepWarmStatus::new(
                KeepWarmState::Waiting,
                "Waiting - didn't answer right after an operation; tried again in a moment",
            ));
            site.probe_wanted = true;
            return;
        }
        let pids = &project.site.worker_process_ids;
        if outcome.timed_out && !request.cold {
            // No answer from a running worker process a debugger is attached to: it is probably stopped at a
            // breakpoint - not the site's fault, and asking again soon would only pile requests up behind it.
            if debugger::attached(pids) {
                site.not_before = now + ms(KeepWarmPlan::new(&settings, &project.site, project.idle_timeout).interval);
                site.wait_status = Some(KeepWarmStatus::new(
                    KeepWarmState::Paused,
                    "Paused - a debugger is attached and the site didn't answer; it may be stopped at a breakpoint",
                ));
                site.probe_wanted = true;
                return;
            }
            // DNN may be starting slowly (after a rebuild): the warm-up page next, with a warm-up's longer time limit -
            // only that one not answering counts.
            if request.kind == KeepWarmRequestKind::Ping {
                site.warm_up_wanted = true;
                return;
            }
        }

        let mut notices = Vec::new();
        let mut sync = false;
        match outcome.kind {
            KeepWarmOutcomeKind::InUse => {
                site.in_use = true;
                site.warm_pids = pids.clone();
            }
            KeepWarmOutcomeKind::SqlDown => {
                // Not the site's fault: looked at again in a minute, without counting.
                site.not_before = now + ms(SQL_RECHECK_AFTER);
                site.wait_status = Some(KeepWarmStatus::new(
                    KeepWarmState::Paused,
                    format!("Paused - {}", outcome.reason.as_deref().unwrap_or_default()),
                ));
                site.warm_up_wanted = request.kind == KeepWarmRequestKind::WarmUp;
            }
            KeepWarmOutcomeKind::PingPathMissing => {
                // The warm-up page now. A single 404 can come from something else answering for a moment: only a
                // second one in a row means the page isn't there.
                site.ping_misses += 1;
                site.warm_up_wanted = true;
                if site.ping_misses >= 2 && !site.ping_path_missing {
                    site.ping_path_missing = true;
                    notices.push(notice(
                        site,
                        format!(
                            "'{}' has no {} - keep warm requests its warm-up page instead.",
                            site.name, request.path
                        ),
                        false,
                    ));
                }
            }
            KeepWarmOutcomeKind::Ok => {
                // A worker process that outlived the time a crashing one lasts: what ended earlier was no crash loop.
                if site.early_deaths > 0
                    && site.last_warm_up_ok_at.is_some_and(|w| now - w >= ms(rules::DIED_SOON_AFTER))
                    && !pids.is_empty()
                    && *pids == site.warm_pids
                {
                    site.early_deaths = 0;
                }
                let recovered = (site.failures > 0 || site.stopped) && site.early_deaths == 0;
                site.rearm();
                site.in_use = false;
                site.last_ok_at = Some(now);
                site.last_ok_local = Some(SystemTime::now());
                site.last_elapsed = outcome.elapsed;
                site.last_note = outcome.note.clone();
                site.warm_pids = pids.clone();
                if request.cold && pids.is_empty() {
                    // It started a worker process the app doesn't know yet: read the sites now - and don't warm it up
                    // again meanwhile.
                    site.assume_worker_until = Some(now + ms(OWN_WORKER_WITHIN));
                    sync = true;
                }
                if request.kind == KeepWarmRequestKind::Ping {
                    site.ping_misses = 0;
                }
                // DNN started for this answer - the time from which a crash would show.
                if request.cold || outcome.slow() {
                    site.last_warm_up_ok_at = Some(now);
                }
                // DNN had to start for a keep-alive request: its warm-up page gets compiled too.
                if request.kind == KeepWarmRequestKind::Ping && outcome.slow() {
                    site.warm_up_wanted = true;
                }
                if recovered {
                    notices.push(notice(site, format!("Keep warm: '{}' answers again.", site.name), false));
                } else if !site.was_warm {
                    let text =
                        format!("'{}' is warm - it answered in {}.", site.name, rules::duration(outcome.elapsed));
                    notices.push(notice(site, text, false));
                }
                site.was_warm = true;
            }
            _ => {
                let reason = outcome.reason.clone().unwrap_or_else(|| "the site didn't answer".into());
                notices.extend(fail(site, &reason, now));
            }
        }
        for n in notices {
            self.emit_notice(Some(n));
        }
        if sync && let Some(sync_sites) = self.sync_sites.clone() {
            let _ = thread::Builder::new().name("keep warm sync".into()).spawn(move || sync_sites());
        }
    }

    // ─── Telling ──────────────────────────────────────────────────────

    fn set_status(&mut self, k: &str, status: KeepWarmStatus) {
        let shown = self.shared.statuses.read().ok().and_then(|s| s.get(k).cloned());
        let Some(site) = self.sites.get_mut(k) else { return };
        if site.status == status && shown.as_ref() == Some(&status) {
            return;
        }
        site.status = status.clone();
        let name = site.name.clone();
        if let Ok(mut statuses) = self.shared.statuses.write() {
            statuses.insert(k.to_string(), status.clone());
        }
        self.emit(KeepWarmEvent::Status { site: name, status });
    }

    fn emit_notice(&self, notice: Option<KeepWarmNotice>) {
        if let Some(notice) = notice {
            self.emit(KeepWarmEvent::Notice(notice));
        }
    }

    fn emit(&self, event: KeepWarmEvent) {
        if let Some(on_event) = &self.on_event
            && catch_unwind(AssertUnwindSafe(|| on_event(event))).is_err()
        {
            log::warn!("A keep-warm event handler failed");
        }
    }
}

/// A failure: the backoff, or - after too many in a row - stopped. Returns the notice it is worth, if any.
fn fail(site: &mut Site, reason: &str, now: i64) -> Option<KeepWarmNotice> {
    site.failures += 1;
    site.failure_reason = Some(reason.to_string());
    site.warm_up_wanted = false;
    site.probe_wanted = true;
    site.in_use = false;
    if site.failures >= rules::FAILURES_BEFORE_STOP {
        site.stopped = true;
        let text = format!(
            "Keep warm stopped trying '{}' after {} failures in a row: {reason}. Check now on its overview (IIS), or starting the site, tries again.",
            site.name, site.failures
        );
        return Some(notice(site, text, true));
    }
    let wait = rules::backoff(site.failures);
    site.not_before = now + ms(wait);
    site.wait_status = Some(KeepWarmStatus::new(
        KeepWarmState::Failing,
        format!("Failing - {reason}. Tried again at {}.", local_time::hm(SystemTime::now() + wait)),
    ));
    (site.failures == 1).then(|| {
        notice(site, format!("Keep warm: '{}' - {reason}. Tried again in {}.", site.name, rules::span(wait)), true)
    })
}

fn notice(site: &Site, message: String, is_warning: bool) -> KeepWarmNotice {
    KeepWarmNotice { site: site.name.clone(), message, is_warning }
}

fn warm_status(site: &Site) -> KeepWarmStatus {
    if site.in_use {
        return KeepWarmStatus::new(KeepWarmState::Warm, "Warm - the site is in use");
    }
    let Some(at) = site.last_ok_local else {
        return KeepWarmStatus::new(KeepWarmState::Warm, "Warm");
    };
    let text = format!("Warm - answered in {} at {}", rules::duration(site.last_elapsed), local_time::hm(at));
    match &site.last_note {
        Some(note) => KeepWarmStatus::new(KeepWarmState::Warm, format!("{text} - {note}")),
        None => KeepWarmStatus::new(KeepWarmState::Warm, text),
    }
}

/// On a request's own thread: waits for the database, skips a site in use, or sends.
fn send_or_skip(
    request: &Request,
    requester: &KeepWarmRequester,
    counters: Option<&Counters>,
    shared: &Shared,
) -> KeepWarmOutcome {
    if let Some((host, port)) = &request.sql
        && !sql_answers(host, *port)
    {
        return KeepWarmOutcome::failed(
            KeepWarmOutcomeKind::SqlDown,
            "the SQL Server container doesn't answer - DNN can't start without its database",
        );
    }
    let mut served = None;
    if request.check_in_use
        && let Some(count) = counters.and_then(Counters::get).and_then(|c| c.get(&request.site).copied())
    {
        served = Some(count);
        // More requests than ours since the last look: it is in use, and IIS counts that as not idle.
        if let Some(baseline) = request.baseline
            && count >= baseline
            && count - baseline > u64::from(request.own)
        {
            return KeepWarmOutcome { requests_served: Some(count), ..KeepWarmOutcome::of(KeepWarmOutcomeKind::InUse) };
        }
    }
    let outcome = requester.send(&request.site, &request.target, &request.path, request.kind, &shared.throttled);
    match served {
        Some(count) => KeepWarmOutcome { requests_served: Some(count), ..outcome },
        None => outcome,
    }
}
