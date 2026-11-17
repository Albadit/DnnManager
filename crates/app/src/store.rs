//! The app's live state - `ServerStore`, `OperationRunner` and `ActivityLog` of the WPF app in one GPUI entity:
//! the projects as IIS and their folders tell them (read again every 5 s while the Projects page shows, 30 s
//! otherwise), this PC's figures (2 s), IIS's own state, the one operation that may run at a time with its Output,
//! and the toasts.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use gpui::{App, AppContext, Context, Entity, SharedString, Task};

use dnnm_application::iis::{IisManager, IisServerAction, IisServerState};
use dnnm_application::operation::{Operation, Outcome};
use dnnm_application::project::ProjectState;
use dnnm_application::settings::{Settings, SettingsStore};
use dnnm_application::use_cases::{SiteAction, control_sites, iis_server};
use dnnm_infrastructure::app_db::AppDatabase;
use dnnm_infrastructure::host_resources::{HostResources, HostSampler};
use dnnm_infrastructure::process_stats::ProcessSampler;
use dnnm_infrastructure::site_folder::{folder_size, read_folder};
use dnnm_keepwarm::{KeepWarmConfig, KeepWarmEvent, KeepWarmService, KeepWarmSettings, KeepWarmSite, KeepWarmStatus};
use futures::StreamExt;

use crate::runner::{self, Controls, LineKind, LogLine, Message, PromptRequest};

const PROJECTS_EVERY_SHOWN: Duration = Duration::from_secs(5);
const PROJECTS_EVERY_HIDDEN: Duration = Duration::from_secs(30);
const HOST_EVERY: Duration = Duration::from_secs(2);
const SIZE_EVERY: Duration = Duration::from_secs(60);
/// The running operation's record (`UnfinishedOperation.Area`).
const UNFINISHED: &str = "operation";
/// Each site's SQL Server is asked this often (5 s time-out each, in parallel).
const SQL_EVERY: Duration = Duration::from_secs(15);

/// Whether what the table shows can be trusted right now (`MonitorConnection`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Live,
    /// What is shown stays; the reason is the live indicator's tooltip.
    Reconnecting(String),
}

/// How an operation stands (`RUNNING` / `FINISHED` / `FAILED` / `CANCELLED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Finished,
    Failed,
    Cancelled,
}

/// One operation as the Output tab shows it.
#[derive(Debug, Clone)]
pub struct Run {
    pub title: SharedString,
    pub status: RunStatus,
    pub started: SystemTime,
    pub duration: Option<Duration>,
    pub lines: Vec<LogLine>,
    /// The header's facts ("Database: dnn10").
    pub facts: Vec<(SharedString, SharedString)>,
    pub error: Option<SharedString>,
}

impl Run {
    /// When each stage started, in order (its stage line's time) - a stage lasts until the next starts, or the run ends.
    pub fn stage_starts(&self) -> Vec<SystemTime> {
        self.lines.iter().filter(|l| l.kind == LineKind::Stage).map(|l| l.at).collect()
    }

    /// How long stage `index` took - so far, while it is the one running (`now`).
    pub fn stage_duration(&self, index: usize, now: SystemTime) -> Option<Duration> {
        let starts = self.stage_starts();
        let start = *starts.get(index)?;
        let end = match starts.get(index + 1) {
            Some(next) => *next,
            None => self.duration.map(|d| self.started + d).unwrap_or(now),
        };
        end.duration_since(start).ok()
    }

    /// The stages in order, each with the worst of its lines (the Output's stage rail).
    pub fn stages(&self) -> Vec<(SharedString, LineKind)> {
        let mut stages: Vec<(SharedString, LineKind)> = Vec::new();
        for line in &self.lines {
            match line.kind {
                LineKind::Stage => stages.push((line.text.clone(), LineKind::Success)),
                LineKind::Fail => {
                    if let Some(last) = stages.last_mut() {
                        last.1 = LineKind::Fail;
                    }
                }
                LineKind::Warn => {
                    if let Some(last) = stages.last_mut()
                        && last.1 != LineKind::Fail
                    {
                        last.1 = LineKind::Warn;
                    }
                }
                _ => {}
            }
        }
        // The stage still running isn't done yet.
        if self.status == RunStatus::Running
            && let Some(last) = stages.last_mut()
            && last.1 == LineKind::Success
        {
            last.1 = LineKind::Info;
        }
        stages
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Warning,
    Error,
}

/// A message in the bottom-right corner (`Services/Toast`).
#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
    /// Shows the Output tab.
    pub show_output: bool,
    pub shown_at: Instant,
}

impl Toast {
    /// Success fades after 3 s (8 s with a button); Warning and Error stay until closed.
    pub fn expired(&self, now: Instant) -> bool {
        let life = if self.show_output { 8 } else { 3 };
        self.kind == ToastKind::Success && now.duration_since(self.shown_at) > Duration::from_secs(life)
    }
}

/// What one read of IIS and the folders found.
struct Snapshot {
    projects: Result<Vec<ProjectState>, String>,
    iis_state: IisServerState,
    /// Every IIS site, DNN or not - New project checks an address against them.
    all_sites: Option<Vec<(String, dnnm_application::iis::IisSiteRuntime)>>,
}

/// What runs when the operation ends, with how it ended.
type OnDone = Box<dyn FnOnce(&Outcome, &mut App)>;

pub struct Store {
    pub settings: Settings,
    /// The settings couldn't be read: the defaults are used and nothing is saved over the database.
    pub settings_problem: Option<String>,
    db: AppDatabase,
    iis: Arc<dyn IisManager>,
    pub projects: Vec<ProjectState>,
    /// Every IIS site as last read (None until read) - New project's address check.
    pub all_sites: Option<Vec<(String, dnnm_application::iis::IisSiteRuntime)>>,
    /// Counts the reads that brought `all_sites`.
    pub sites_read: u64,
    /// Counts the operations that failed, with the last one's title and error - a notice while the window is hidden.
    pub failures: u64,
    pub last_failure: Option<(String, String)>,
    pub connection: Connection,
    pub iis_state: IisServerState,
    pub host: HostResources,
    /// What a row is doing while its operation runs ("Starting…").
    pub pending: HashMap<String, SharedString>,
    pub runs: Vec<Run>,
    pub toasts: Vec<Toast>,
    /// Cancel and "Stop undoing" of the running operation.
    controls: Option<Controls>,
    /// The running operation is undoing what it did: Cancel stops the undo.
    pub undoing: bool,
    /// A question from the running operation, shown as a dialog until answered.
    pub prompt: Option<PromptRequest>,
    /// Rows that say what the operation does once it has started - after its question was answered yes, at its
    /// first line (WPF's `StartSignal`): "Removing…", "Clearing cache…".
    pending_on_start: Option<(Vec<String>, SharedString)>,
    /// The toast an operation shows when it finishes (Clone, Back up, Clear cache…).
    success_toast: Option<String>,
    /// Sites removed by the running operation: keep warm lets them go when it ends.
    keep_warm_forget: Vec<String>,
    /// What runs when the operation ends, with how it ended (Settings' "Set up IIS" refreshing its card).
    on_done: Option<OnDone>,
    /// Keep warm (`KeepWarmService`): requests the sites kept warm so IIS doesn't shut them down for being idle.
    keep_warm: Arc<KeepWarmService>,
    /// The day's log file gets every operation's lines, as the Output tab does (`ActivityLog`).
    run_log: Arc<dnnm_logs::run_log::RunLog>,
    /// Keeps the hosts file's DNN Manager block in step with the sites' host names (`HostsFileService`); None for the
    /// fixture, which mustn't change the system.
    hosts: Option<dnnm_system::hosts::HostsKeeper>,
    /// IIS is the real one, not the fixture: the system may be changed and the operation record kept.
    pub(crate) real_system: bool,
    /// Each site's keep-warm status, by lower-cased name.
    keep_warm_status: HashMap<String, KeepWarmStatus>,
    /// The Output tab has a failure the user hasn't looked at.
    pub output_unseen_failure: bool,
    /// The Projects page is on screen: read every 5 s, not 30.
    pub projects_shown: bool,
    /// The Size column is shown: measure the folders.
    pub measure_sizes: bool,
    sizes: Arc<Mutex<HashMap<String, u64>>>,
    /// The last answer of each site's SQL Server (`SiteDatabaseChecks`), by site name.
    sql_checks: Arc<Mutex<HashMap<String, dnnm_sql::tester::SiteDatabaseCheck>>>,
    sampler: Arc<Mutex<ProcessSampler>>,
    next_toast: u64,
    _tasks: Vec<Task<()>>,
}

impl Store {
    pub fn new(
        db: AppDatabase,
        iis: Arc<dyn IisManager>,
        run_log: Arc<dnnm_logs::run_log::RunLog>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (settings, settings_problem) = match db.load() {
            Ok(s) if s.is_newer_than_build() => (
                Settings::default(),
                Some("The settings were saved by a newer DNN Manager - this one uses its defaults and doesn't change them.".into()),
            ),
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(format!("The settings can't be read: {e}"))),
        };
        let measure_sizes = settings.project_columns().iter().any(|c| c == "size");
        let (keep_warm, keep_warm_task) = start_keep_warm(&db, &settings, cx);
        let mut store = Self {
            settings,
            settings_problem,
            db,
            iis,
            projects: Vec::new(),
            all_sites: None,
            sites_read: 0,
            failures: 0,
            last_failure: None,
            connection: Connection::Connecting,
            iis_state: IisServerState::Unknown,
            host: HostResources::default(),
            pending: HashMap::new(),
            runs: Vec::new(),
            toasts: Vec::new(),
            controls: None,
            undoing: false,
            prompt: None,
            pending_on_start: None,
            success_toast: None,
            keep_warm_forget: Vec::new(),
            on_done: None,
            keep_warm,
            run_log,
            hosts: None,
            real_system: false,
            keep_warm_status: HashMap::new(),
            output_unseen_failure: false,
            projects_shown: true,
            measure_sizes,
            sizes: Arc::default(),
            sql_checks: Arc::default(),
            sampler: Arc::default(),
            next_toast: 0,
            _tasks: Vec::new(),
        };
        store._tasks =
            vec![store.watch_projects(cx), store.watch_host(cx), store.measure_folders(cx), store.watch_sql(cx), keep_warm_task];
        store
    }

    /// Reads IIS and the folders now, then on a timer.
    fn watch_projects(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((iis, base, sampler, sizes)) = this.update(cx, |s, _| {
                    (s.iis.clone(), PathBuf::from(s.settings.base_directory()), s.sampler.clone(), s.sizes.clone())
                }) else {
                    break;
                };
                let snapshot =
                    cx.background_executor().spawn(async move { read_snapshot(&*iis, &base, &sampler, &sizes) }).await;
                let Ok(shown) = this.update(cx, |s, cx| {
                    s.apply_and_tell(snapshot, cx);
                    cx.notify();
                    s.projects_shown
                }) else {
                    break;
                };
                let every = if shown { PROJECTS_EVERY_SHOWN } else { PROJECTS_EVERY_HIDDEN };
                cx.background_executor().timer(every).await;
            }
        })
    }

    fn watch_host(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let sampler = Arc::new(Mutex::new(HostSampler::default()));
            loop {
                let Ok(base) = this.update(cx, |s, _| PathBuf::from(s.settings.base_directory())) else { break };
                let sampler = sampler.clone();
                let host = cx
                    .background_executor()
                    .spawn(async move { sampler.lock().map(|mut s| s.sample(&base)).unwrap_or_default() })
                    .await;
                if this
                    .update(cx, |s, cx| {
                        s.host = host;
                        s.toasts.retain(|t| !t.expired(Instant::now()));
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(HOST_EVERY).await;
            }
        })
    }

    /// Asks each site's SQL Server - with the site's own web.config connection - whether it answers and the database is
    /// there (the SQL column). Not a LocalDB file, and with Windows sign-in only a server on this PC or the one in the
    /// settings (`ServerStateMonitor`).
    fn watch_sql(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((folders, options, checks)) = this.update(cx, |s, _| {
                    let folders: Vec<(String, PathBuf)> = s
                        .projects
                        .iter()
                        .filter(|p| p.database_name.is_some() && !p.database_is_file)
                        .map(|p| (p.name.clone(), p.directory.clone()))
                        .collect();
                    let options = dnnm_sql::model::DatabaseServerOptions {
                        kind: s.settings.get("sqlServer.type").unwrap_or("container").to_string(),
                        server: s.settings.get("sqlServer.server").unwrap_or_default().to_string(),
                    };
                    (folders, options, s.sql_checks.clone())
                }) else {
                    break;
                };
                if !folders.is_empty() {
                    let answers = cx
                        .background_executor()
                        .spawn(async move {
                            let sites: std::collections::BTreeMap<String, dnnm_sql::model::SiteSqlConnection> = folders
                                .into_iter()
                                .filter_map(|(name, dir)| {
                                    let config = dnnm_infrastructure::web_config::read_database_connection(
                                        &dir.join("web.config"),
                                    )
                                    .ok()?;
                                    dnnm_sql::tester::site_sql_connection(&config, &options).map(|c| (name, c))
                                })
                                .collect();
                            dnnm_sql::tester::ask(&dnnm_sql::tester::SqlClientTester, &sites, None)
                        })
                        .await;
                    if let Ok(mut checks) = checks.lock() {
                        *checks = answers.into_iter().collect();
                    }
                    let _ = this.update(cx, |s, cx| {
                        s.apply_sql_checks();
                        cx.notify();
                    });
                }
                cx.background_executor().timer(SQL_EVERY).await;
            }
        })
    }

    /// The last SQL answers on the rows.
    fn apply_sql_checks(&mut self) {
        let checks = self.sql_checks.lock().map(|m| m.clone()).unwrap_or_default();
        for p in &mut self.projects {
            if let Some(check) = checks.get(&p.name) {
                p.sql_reachable = Some(check.reachable);
                p.database_exists = check.exists;
                p.sql_problem = check.problem.clone();
            }
        }
    }

    /// The folders' sizes, only while the Size column shows - walking every site's folder is not free.
    fn measure_folders(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((wanted, folders, sizes)) = this.update(cx, |s, _| {
                    let folders: Vec<(String, PathBuf)> =
                        s.projects.iter().map(|p| (p.name.clone(), p.directory.clone())).collect();
                    (s.measure_sizes, folders, s.sizes.clone())
                }) else {
                    break;
                };
                if wanted && !folders.is_empty() {
                    cx.background_executor()
                        .spawn(async move {
                            let measured: HashMap<String, u64> = folders
                                .into_iter()
                                .filter_map(|(name, dir)| folder_size(&dir).map(|size| (name, size)))
                                .collect();
                            if let Ok(mut sizes) = sizes.lock() {
                                *sizes = measured;
                            }
                        })
                        .await;
                    let _ = this.update(cx, |s, cx| {
                        let sizes = s.sizes.lock().map(|m| m.clone()).unwrap_or_default();
                        for p in &mut s.projects {
                            p.size_bytes = sizes.get(&p.name).copied();
                        }
                        cx.notify();
                    });
                }
                cx.background_executor().timer(if wanted { SIZE_EVERY } else { Duration::from_secs(2) }).await;
            }
        })
    }

    /// The real IIS is read (not the fixture): the hosts file follows it, and a running operation is recorded so a
    /// later start can say what may be left half done. The fixture changes neither.
    pub fn use_real_system(&mut self) {
        // The hosts file the site names go in is Windows' (System32\drivers\etc) - IIS's sites are there only.
        if cfg!(windows) {
            self.hosts = Some(dnnm_system::hosts::HostsKeeper::new(dnnm_system::hosts::default_path()));
        }
        self.real_system = true;
    }

    fn apply(&mut self, snapshot: Snapshot) -> Vec<dnnm_system::hosts::Notice> {
        let mut notices = Vec::new();
        self.iis_state = snapshot.iis_state;
        if snapshot.all_sites.is_some() {
            self.all_sites = snapshot.all_sites;
            self.sites_read += 1;
        }
        match snapshot.projects {
            Ok(projects) => {
                // Only a complete read: a site missing from it would count as removed, and lose its keep warm.
                self.keep_warm.set_iis_state(snapshot.iis_state);
                self.keep_warm.set_sites(projects.iter().map(|p| KeepWarmSite::from_project(p, None)).collect());
                if let Some(hosts) = self.hosts.as_mut() {
                    let sites: Vec<_> = projects.iter().map(|p| p.site.clone()).collect();
                    if let Some(notice) = hosts.sync(&sites) {
                        if notice.warning {
                            log::warn!("{}", notice.message);
                        } else {
                            log::info!("{}", notice.message);
                        }
                        notices.push(notice);
                    }
                }
                self.projects = projects;
                self.apply_sql_checks();
                self.connection = Connection::Live;
            }
            // IIS not installed is a state, not a lost connection: the table says so.
            Err(_) if snapshot.iis_state == IisServerState::NotInstalled => {
                self.projects.clear();
                self.connection = Connection::Live;
            }
            Err(reason) => self.connection = Connection::Reconnecting(reason),
        }
        notices
    }

    /// Reads applied: the hosts file's notices become toasts (each said once by the keeper).
    fn apply_and_tell(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        for notice in self.apply(snapshot) {
            let kind = if notice.warning { ToastKind::Warning } else { ToastKind::Success };
            self.toast(kind, notice.message, false, cx);
        }
    }

    /// Reads everything again now - after an operation, so the rows show what it did.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let iis = self.iis.clone();
        let base = PathBuf::from(self.settings.base_directory());
        let (sampler, sizes) = (self.sampler.clone(), self.sizes.clone());
        cx.spawn(async move |this, cx| {
            let snapshot =
                cx.background_executor().spawn(async move { read_snapshot(&*iis, &base, &sampler, &sizes) }).await;
            let _ = this.update(cx, |s, cx| {
                s.apply_and_tell(snapshot, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// What a use case gets to act on (`dnnm_wiring::Services`): IIS, the database, the settings as they are now.
    pub fn services(&self) -> dnnm_wiring::Services {
        dnnm_wiring::Services { iis: self.iis.clone(), db: self.db.clone(), settings: self.settings.clone() }
    }

    /// IIS, for what the views read themselves (a site's log folder).
    pub fn iis(&self) -> &Arc<dyn IisManager> {
        &self.iis
    }

    /// `dnnmanager.db`, for the workspace's own state.
    pub fn db(&self) -> &AppDatabase {
        &self.db
    }

    pub fn is_busy(&self) -> bool {
        self.runs.last().is_some_and(|r| r.status == RunStatus::Running)
    }

    pub fn running_title(&self) -> Option<SharedString> {
        self.runs.last().filter(|r| r.status == RunStatus::Running).map(|r| r.title.clone())
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<SharedString>, show_output: bool, cx: &mut Context<Self>) {
        self.next_toast += 1;
        self.toasts.push(Toast { id: self.next_toast, kind, text: text.into(), show_output, shown_at: Instant::now() });
        cx.notify();
    }

    pub fn close_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }

    /// Runs `work` as the one operation (`OperationRunner.RunAsync`): a second while one runs is refused; its lines go
    /// to the Output tab as they come; its questions become dialogs; a failure becomes a toast with Show output.
    pub fn run_operation(
        &mut self,
        title: String,
        rows: Vec<String>,
        pending: &'static str,
        work: impl FnOnce(&mut Operation) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if let Some(running) = self.running_title() {
            self.toast(
                ToastKind::Warning,
                format!("'{running}' is still running - wait for it to finish, or cancel it first."),
                false,
                cx,
            );
            return;
        }
        for row in &rows {
            self.pending.insert(row.clone(), pending.into());
        }
        self.runs.push(Run {
            title: title.into(),
            status: RunStatus::Running,
            started: SystemTime::now(),
            duration: None,
            lines: Vec::new(),
            facts: Vec::new(),
            error: None,
        });
        self.undoing = false;
        self.run_log.begin_run(&self.runs.last().map(|r| r.title.to_string()).unwrap_or_default());
        self.write_unfinished(&[]);
        let (controls, mut messages) = runner::start(work);
        self.controls = Some(controls);
        cx.notify();
        let started = Instant::now();
        cx.spawn(async move |this, cx| {
            while let Some(message) = messages.next().await {
                let done = matches!(message, Message::Done(_));
                let applied = this.update(cx, |s, cx| {
                    s.apply_message(message, started, &rows, cx);
                    cx.notify();
                });
                if done || applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_message(&mut self, message: Message, started: Instant, rows: &[String], cx: &mut Context<Self>) {
        match message {
            Message::Line(line) => {
                if let Some((rows, text)) = self.pending_on_start.take() {
                    for row in rows {
                        self.pending.insert(row, text.clone());
                    }
                }
                match line.kind {
                    LineKind::Stage => self.run_log.step(&line.text),
                    LineKind::Info => self.run_log.info(&line.text),
                    LineKind::Success => self.run_log.success(&line.text),
                    LineKind::Warn => self.run_log.warn(&line.text),
                    LineKind::Fail => self.run_log.fail(&line.text),
                }
                if let Some(run) = self.runs.last_mut() {
                    run.lines.push(line);
                }
            }
            Message::Fact(name, value) => {
                if let Some(run) = self.runs.last_mut() {
                    run.facts.push((name, value));
                }
            }
            Message::Prompt(request) => {
                // Answered by the dialog; a second one can't come while the operation waits for the first.
                if let Some(earlier) = self.prompt.replace(request) {
                    earlier.answer(None);
                }
            }
            Message::Undoing => self.undoing = true,
            Message::Pending(left) => self.write_unfinished(&left),
            Message::Done(outcome) => {
                self.clear_unfinished();
                if let Some((late, _)) = self.pending_on_start.take() {
                    for row in late {
                        self.pending.remove(&row);
                    }
                }
                for row in rows {
                    self.pending.remove(row);
                }
                self.controls = None;
                self.undoing = false;
                if let Some(prompt) = self.prompt.take() {
                    prompt.answer(None);
                }
                if let Some(run) = self.runs.last_mut() {
                    run.duration = Some(started.elapsed());
                    run.status = match &outcome {
                        Outcome::Finished => RunStatus::Finished,
                        Outcome::Cancelled => RunStatus::Cancelled,
                        Outcome::Failed(e) => {
                            run.error = Some(e.clone().into());
                            RunStatus::Failed
                        }
                    };
                }
                match &outcome {
                    Outcome::Finished => self.run_log.end_run(dnnm_logs::run_log::RunStatus::Finished, None),
                    Outcome::Cancelled => self.run_log.end_run(dnnm_logs::run_log::RunStatus::Cancelled, None),
                    Outcome::Failed(e) => self.run_log.end_run(dnnm_logs::run_log::RunStatus::Failed, Some(e)),
                }
                for site in std::mem::take(&mut self.keep_warm_forget) {
                    self.keep_warm.set_enabled(&site, false);
                    self.keep_warm_status.remove(&site.to_lowercase());
                }
                if let Some(text) = self.success_toast.take()
                    && outcome == Outcome::Finished
                {
                    self.toast(ToastKind::Success, text, false, cx);
                }
                if let Some(done) = self.on_done.take() {
                    let ended = outcome.clone();
                    cx.defer(move |cx| done(&ended, cx));
                }
                if let Outcome::Failed(e) = outcome {
                    self.failures += 1;
                    let title = self.runs.last().map(|r| r.title.to_string()).unwrap_or_default();
                    self.last_failure = Some((title, e.clone()));
                    self.output_unseen_failure = true;
                    self.toast(ToastKind::Error, e, true, cx);
                }
                self.refresh(cx);
            }
        }
    }

    /// The running operation on disk (`UnfinishedOperation`, state area `operation`): its title, when it started, and
    /// what it would undo now - written as it changes, deleted when it ends.
    fn write_unfinished(&self, left: &[String]) {
        if !self.real_system {
            return;
        }
        let Some(run) = self.runs.last() else { return };
        let db = &self.db;
        let _ = db.set_state(UNFINISHED, "title", &run.title);
        if db.state(UNFINISHED, "startedUtc").ok().flatten().is_none() {
            let _ = db.set_state(UNFINISHED, "startedUtc", &dnnm_application::records::utc_now_round_trip());
        }
        let before: usize = db.state(UNFINISHED, "left").ok().flatten().and_then(|c| c.parse().ok()).unwrap_or(0);
        for i in left.len()..before {
            let _ = db.delete_state(UNFINISHED, &format!("left[{i}]"));
        }
        let _ = db.set_state(UNFINISHED, "left", &left.len().to_string());
        for (i, what) in left.iter().enumerate() {
            let _ = db.set_state(UNFINISHED, &format!("left[{i}]"), what);
        }
    }

    fn clear_unfinished(&self) {
        if !self.real_system {
            return;
        }
        let db = &self.db;
        let count: usize = db.state(UNFINISHED, "left").ok().flatten().and_then(|c| c.parse().ok()).unwrap_or(0);
        for i in 0..count {
            let _ = db.delete_state(UNFINISHED, &format!("left[{i}]"));
        }
        for key in ["title", "startedUtc", "left"] {
            let _ = db.delete_state(UNFINISHED, key);
        }
    }

    /// At start: an operation that was running when DNN Manager ended (`MainWindow` - "didn't finish").
    pub fn report_unfinished(&mut self, cx: &mut Context<Self>) {
        if !self.real_system {
            return;
        }
        let Some(title) = self.db.state(UNFINISHED, "title").ok().flatten() else { return };
        let started = self
            .db
            .state(UNFINISHED, "startedUtc")
            .ok()
            .flatten()
            .and_then(|s| dnnm_infrastructure::local_time::from_round_trip(&s))
            .map(dnnm_infrastructure::local_time::short_date_time)
            .unwrap_or_default();
        let count: usize = self.db.state(UNFINISHED, "left").ok().flatten().and_then(|c| c.parse().ok()).unwrap_or(0);
        let left: Vec<String> =
            (0..count).filter_map(|i| self.db.state(UNFINISHED, &format!("left[{i}]")).ok().flatten()).collect();
        let what = if left.is_empty() {
            "It hadn't changed anything that needed undoing yet.".to_string()
        } else {
            format!("It may have left half done what these would have undone: {}.", left.join("; "))
        };
        log::warn!("'{title}' didn't finish - DNN Manager ended while it ran");
        self.clear_unfinished();
        self.toast(
            ToastKind::Warning,
            format!("'{title}' didn't finish - DNN Manager ended while it ran (started {started}). {what}"),
            true,
            cx,
        );
    }

    /// Cancel: the operation stops and undoes what it did; pressed again while it undoes, the undo stops.
    pub fn cancel_operation(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = &self.controls {
            if self.undoing {
                controls.skip.cancel();
            } else {
                controls.cancel.cancel();
                self.run_log.cancelling();
            }
            // A question waiting for an answer is answered no.
            if let Some(prompt) = self.prompt.take() {
                prompt.answer(None);
            }
            cx.notify();
        }
    }

    /// The dialog's answer to the running operation's question.
    pub fn answer_prompt(&mut self, choice: Option<usize>, cx: &mut Context<Self>) {
        if let Some(prompt) = self.prompt.take() {
            prompt.answer(choice);
            cx.notify();
        }
    }

    /// Start, Stop or Restart the named sites as one operation (`ServerStore.ControlSitesAsync`).
    pub fn control_sites(&mut self, names: Vec<String>, action: SiteAction, cx: &mut Context<Self>) {
        if names.is_empty() {
            return;
        }
        let iis = self.iis.clone();
        let title = action.title(&names);
        let rows = names.clone();
        self.run_operation(title, rows, action.pending(), move |op| control_sites(&*iis, &names, action, op), cx);
    }

    /// One operation at a time: a second is refused with the WPF app's toast; true when refused.
    pub fn refuse_if_busy(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(running) = self.running_title() else { return false };
        self.toast(
            ToastKind::Warning,
            format!("'{running}' is still running - wait for it to finish, or cancel it first."),
            false,
            cx,
        );
        true
    }

    /// The fixture stands in for IIS only: its sites' folders and databases are real ones, so an operation that changes
    /// files or a database is refused there. True when refused.
    pub fn fixture_refuses(&mut self, cx: &mut Context<Self>) -> bool {
        if self.real_system {
            return false;
        }
        self.toast(
            ToastKind::Warning,
            "Not with the fixture: its sites' folders and databases are real, and this would change them.",
            false,
            cx,
        );
        true
    }

    /// Runs a use case that changes the system, with its success toast - refused while another runs, or with the fixture.
    pub fn run_changing(
        &mut self,
        title: String,
        success: Option<String>,
        work: impl FnOnce(&mut dnnm_application::operation::Operation) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_if_busy(cx) || self.fixture_refuses(cx) {
            return;
        }
        self.success_toast = success;
        self.run_operation(title, Vec::new(), "", work, cx);
    }

    /// Like [`Self::run_changing`], then `done` with how it ended - not when it was refused.
    pub fn run_then(
        &mut self,
        title: String,
        success: Option<String>,
        work: impl FnOnce(&mut dnnm_application::operation::Operation) -> Result<(), String> + Send + 'static,
        done: impl FnOnce(&Outcome, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_if_busy(cx) || self.fixture_refuses(cx) {
            return;
        }
        self.success_toast = success;
        self.on_done = Some(Box::new(done));
        self.run_operation(title, Vec::new(), "", work, cx);
    }

    /// A question of the UI's own (`Dialogs.Confirm`): shown as an operation's are; the answer's index arrives on the
    /// receiver (None when closed).
    pub fn ask(
        &mut self,
        question: impl Into<SharedString>,
        answers: Vec<SharedString>,
        danger: bool,
        cx: &mut Context<Self>,
    ) -> std::sync::mpsc::Receiver<Option<usize>> {
        let (request, answer) = runner::PromptRequest::new(question, answers, danger);
        if let Some(earlier) = self.prompt.replace(request) {
            earlier.answer(None);
        }
        cx.notify();
        answer
    }

    /// Saves the settings Settings hands over (Save): the rows that changed; the error when they couldn't be.
    pub fn save_settings(&mut self, next: Settings, cx: &mut Context<Self>) -> Result<(), String> {
        if let Some(problem) = &self.settings_problem {
            return Err(problem.clone());
        }
        self.db.save(&self.settings, &next).map_err(|e| e.to_string())?;
        self.settings = next;
        self.keep_warm.set_settings(KeepWarmSettings::from_settings(&self.settings));
        self.measure_sizes = self.settings.project_columns().iter().any(|c| c == "size");
        cx.notify();
        Ok(())
    }

    /// The settings read again from the database (after Troubleshoot's Reset settings).
    pub fn reload_settings(&mut self, cx: &mut Context<Self>) {
        match self.db.load() {
            Ok(settings) => {
                self.settings = settings;
                self.settings_problem = None;
            }
            Err(e) => self.settings_problem = Some(e.to_string()),
        }
        self.keep_warm.set_settings(KeepWarmSettings::from_settings(&self.settings));
        cx.notify();
    }

    /// A prompt for work outside an operation (a dialog reading what it shows): its questions become the same dialog.
    pub fn detached_prompt(&mut self, cx: &mut Context<Self>) -> Arc<dyn dnnm_application::operation::Prompt> {
        let (prompt, mut messages) = runner::ChannelPrompt::detached();
        cx.spawn(async move |this, cx| {
            while let Some(message) = messages.next().await {
                if let Message::Prompt(request) = message
                    && this
                        .update(cx, |s, cx| {
                            if let Some(earlier) = s.prompt.replace(request) {
                                earlier.answer(None);
                            }
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Arc::new(prompt)
    }

    /// Remove projects (`ServerStore.RemoveAsync` → `RemoveProjectUseCase`): it asks first; once it has started the rows
    /// say "Removing…". Keep warm forgets them; everything is read again after.
    pub fn remove_projects(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        use dnnm_wiring::remove_project::{REMOVING, RemoveProjectRequest, SiteToRemove, remove_project, remove_title};
        let sites: Vec<SiteToRemove> = self
            .projects
            .iter()
            .filter(|p| names.contains(&p.name))
            .map(|p| SiteToRemove {
                name: p.name.clone(),
                directory: p.directory.clone(),
                in_projects_folder: p.in_projects_folder,
            })
            .collect();
        if sites.is_empty() {
            return;
        }
        if self.refuse_if_busy(cx) || self.fixture_refuses(cx) {
            return;
        }
        let title = remove_title(&names);
        let services = self.services();
        self.pending_on_start = Some((names.clone(), REMOVING.into()));
        self.run_operation(
            title,
            Vec::new(),
            "",
            move |op| remove_project(&services, RemoveProjectRequest { sites }, op),
            cx,
        );
        // The service's own list follows at the next read; the removed sites are off at once.
        for name in &names {
            self.keep_warm_forget.push(name.clone());
        }
    }

    /// Clear a site's cache (`ServerStore.ClearCacheAsync` → `ClearSiteCacheUseCase`): asks first; "Clearing cache…" once
    /// started; a toast when done.
    pub fn clear_cache(&mut self, name: String, cx: &mut Context<Self>) {
        use dnnm_wiring::clear_site_cache::{CLEARING_CACHE, ClearSiteCacheRequest, clear_cache_title, clear_site_cache};
        let Some(directory) = self.projects.iter().find(|p| p.name == name).map(|p| p.directory.clone()) else { return };
        if self.refuse_if_busy(cx) || self.fixture_refuses(cx) {
            return;
        }
        let services = self.services();
        self.pending_on_start = Some((vec![name.clone()], CLEARING_CACHE.into()));
        self.success_toast = Some(dnnm_wiring::clear_site_cache::cache_cleared_toast(&name));
        let site = name.clone();
        self.run_operation(
            clear_cache_title(&name),
            Vec::new(),
            "",
            move |op| clear_site_cache(&services, ClearSiteCacheRequest { site, directory }, op),
            cx,
        );
    }

    /// Opens a project in an editor or SQL Server Management Studio ("Open with…") - it changes nothing, so the
    /// fixture allows it; what signing SSMS in says goes to the Output.
    pub fn open_with(&mut self, request: dnnm_wiring::open_with::OpenWithRequest, cx: &mut Context<Self>) {
        if self.refuse_if_busy(cx) {
            return;
        }
        let services = self.services();
        let title = dnnm_wiring::open_with::title(&request.entry);
        self.run_operation(title, Vec::new(), "", move |op| dnnm_wiring::open_with::open_with(&services, request, op), cx);
    }

    /// Start, Stop or Restart IIS from the status bar (`IisServerUseCase` - Stop and Restart ask first).
    pub fn control_iis(&mut self, action: IisServerAction, cx: &mut Context<Self>) {
        let iis = self.iis.clone();
        let title = match action {
            IisServerAction::Start => "Start IIS",
            IisServerAction::Stop => "Stop IIS",
            IisServerAction::Restart => "Restart IIS",
        };
        self.run_operation(title.into(), Vec::new(), "", move |op| iis_server(&*iis, action, op), cx);
    }

    pub fn clear_output(&mut self, cx: &mut Context<Self>) {
        if !self.is_busy() {
            self.runs.clear();
            self.output_unseen_failure = false;
            cx.notify();
        }
    }

    /// A site's keep-warm status (Off until the service says otherwise).
    pub fn keep_warm_status(&self, site: &str) -> KeepWarmStatus {
        self.keep_warm_status.get(&site.to_lowercase()).cloned().unwrap_or_else(|| self.keep_warm.status_of(site))
    }

    /// Keep warm on or off for sites (`ServerStore.ToggleKeepWarm`) - no operation; the row says it at once.
    pub fn set_keep_warm(&mut self, sites: &[String], on: bool, cx: &mut Context<Self>) {
        let minutes = KeepWarmSettings::from_settings(&self.settings).ping_minutes;
        for site in sites {
            self.keep_warm.set_enabled(site, on);
            self.keep_warm_status.insert(site.to_lowercase(), self.keep_warm.status_of(site));
            let line = if on {
                dnnm_keepwarm::view::switched_on_log(site, minutes)
            } else {
                dnnm_keepwarm::view::switched_off_log(site)
            };
            log::info!("{line}");
        }
        cx.notify();
    }

    /// Changes the settings and saves the rows that changed (theme, layout, columns - saved at once, as in WPF).
    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let before = self.settings.clone();
        change(&mut self.settings);
        self.keep_warm.set_settings(KeepWarmSettings::from_settings(&self.settings));
        self.measure_sizes = self.settings.project_columns().iter().any(|c| c == "size");
        if self.settings_problem.is_none()
            && let Err(e) = self.db.save(&before, &self.settings)
        {
            self.toast(ToastKind::Error, format!("The settings couldn't be saved: {e}"), false, cx);
        }
        cx.notify();
    }
}

/// Starts keep warm; its events come to the store through a channel (they are raised on the service's thread).
fn start_keep_warm(db: &AppDatabase, settings: &Settings, cx: &mut Context<Store>) -> (Arc<KeepWarmService>, Task<()>) {
    enum Note {
        Event(KeepWarmEvent),
        Sync,
    }
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Note>();
    let mut config = KeepWarmConfig::new(db.path(), KeepWarmSettings::from_settings(settings));
    let events = tx.clone();
    config.on_event = Some(Box::new(move |event| {
        let _ = events.unbounded_send(Note::Event(event));
    }));
    config.sync_sites = Some(Arc::new(move || {
        let _ = tx.unbounded_send(Note::Sync);
    }));
    let service = Arc::new(KeepWarmService::start(config));
    let task = cx.spawn(async move |this, cx| {
        while let Some(note) = rx.next().await {
            let applied = this.update(cx, |s, cx| {
                match note {
                    Note::Event(KeepWarmEvent::Status { site, status }) => {
                        s.keep_warm_status.insert(site.to_lowercase(), status);
                    }
                    Note::Event(KeepWarmEvent::Notice(notice)) => {
                        if notice.is_warning {
                            log::warn!("{}: {}", notice.site, notice.message);
                        } else {
                            log::info!("{}: {}", notice.site, notice.message);
                        }
                    }
                    Note::Sync => s.refresh(cx),
                }
                cx.notify();
            });
            if applied.is_err() {
                break;
            }
        }
    });
    (service, task)
}

/// One read: every IIS site with a DNN install (sites without `bin\DotNetNuke.dll` aren't listed), what its folder
/// says, its worker processes' figures and its last measured size.
fn read_snapshot(
    iis: &dyn IisManager,
    projects_folder: &std::path::Path,
    sampler: &Mutex<ProcessSampler>,
    sizes: &Mutex<HashMap<String, u64>>,
) -> Snapshot {
    let iis_state = iis.server_state();
    let read = iis.site_runtimes().map_err(|e| e.0);
    let all_sites = read.as_ref().ok().map(|sites| sites.iter().map(|(n, s)| (n.clone(), s.clone())).collect());
    let projects = read.map(|sites| {
        let sizes = sizes.lock().map(|m| m.clone()).unwrap_or_default();
        let mut sampler = sampler.lock().ok();
        let mut all_pids = HashSet::new();
        let mut projects: Vec<ProjectState> = sites
            .into_iter()
            .map(|(name, site)| {
                let mut p = ProjectState::from_site(name, site);
                read_folder(&mut p, projects_folder);
                p
            })
            .filter(|p| p.dnn_version.is_some())
            .collect();
        for p in &mut projects {
            all_pids.extend(p.site.worker_process_ids.iter().copied());
            if let Some(sampler) = sampler.as_mut() {
                p.stats = sampler.sample(&p.site.worker_process_ids);
            }
            p.size_bytes = sizes.get(&p.name).copied();
        }
        if let Some(sampler) = sampler.as_mut() {
            sampler.keep_only(&all_pids);
        }
        projects
    });
    Snapshot { projects, iis_state, all_sites }
}

/// Opens the store for the app's global use.
pub fn new_store(
    cx: &mut App,
    db: AppDatabase,
    iis: Arc<dyn IisManager>,
    run_log: Arc<dnnm_logs::run_log::RunLog>,
) -> Entity<Store> {
    cx.new(|cx| Store::new(db, iis, run_log, cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{LineKind, LogLine};

    fn line(at: SystemTime, kind: LineKind, text: &str) -> LogLine {
        LogLine { at, kind, text: text.to_string().into() }
    }

    #[test]
    fn a_stage_lasts_until_the_next_one_or_the_end_and_the_running_one_until_now() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut run = Run {
            title: "Create 'qwer'".into(),
            status: RunStatus::Running,
            started: t0,
            duration: None,
            lines: vec![
                line(at(0), LineKind::Stage, "DNN version"),
                line(at(1), LineKind::Info, "Using DNN 10.4.1"),
                line(at(3), LineKind::Stage, "Checking IIS"),
            ],
            facts: Vec::new(),
            error: None,
        };
        assert_eq!(run.stage_starts(), vec![at(0), at(3)]);
        assert_eq!(run.stage_duration(0, at(10)), Some(Duration::from_secs(3)));
        assert_eq!(run.stage_duration(1, at(10)), Some(Duration::from_secs(7)), "running: until now");
        assert_eq!(run.stage_duration(2, at(10)), None);
        run.status = RunStatus::Finished;
        run.duration = Some(Duration::from_secs(5));
        assert_eq!(run.stage_duration(1, at(10)), Some(Duration::from_secs(2)), "ended: until the run's end");
    }
}
