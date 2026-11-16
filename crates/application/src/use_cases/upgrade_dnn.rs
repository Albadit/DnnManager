//! `UpgradeDnnUseCase` (`src/DnnManager.Application/UseCases/UpgradeDnnUseCase.cs`): upgrades a project's DNN as DNN's
//! suggested upgrade path says - one listed version at a time, never straight to the newest - checking it all the way
//! (.docs/dnn-upgrades.md):
//!
//! 1. **Analyse** the site, its database, IIS and this PC and make the **plan**: the steps, each with its checks.
//!    Anything blocking - nothing is changed.
//! 2. For each step: a **backup** of the site's files and database (with the plan, the site's inventory and web.config
//!    beside it); the step's **files** (its upgrade package, or from DNN 10.2 on its install package as DNN's own local
//!    upgrade puts it in); DNN's **upgrade** (`Install.aspx?mode=upgrade`); a **restart**; then the **checks**.
//! 3. A step that fails - or a cancel - stops the chain: what tells why is kept with the stage's backup, the cause and
//!    fixes are said, and the site is put back from that backup - the last version that worked. The steps before it
//!    stay done; their backups go back further.
//!
//! The analyser, the checks, DNN's installer and the packages live in `dnnm-dnn`, which this crate can't use: the use
//! case sees them through [`UpgradeSystem`], and the plan as [`UpgradePlan`] (the wiring turns `dnnm-dnn`'s into it).
//!
//! Deviations from the C#: the reporter has no transient status line, so `reporter.Progress` lines are left out; the
//! `Context` header is a [`Reporter::fact`]; the request has no test accounts (the app never gives any). Putting a
//! step's backup back runs as an undo step on a thread of its own, so its lines are kept and written to the Output once
//! it has finished - after the undo's own line; when the runner undoes it (a cancel), only the undo's line is shown.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use dnnm_domain::DnnRelease;

use super::backups::{self, NOTE_FILE, Stamp};
use super::export_project::ExportProjectRequest;
use crate::iis::{IisManager, IisSiteDetails, states};
use crate::operation::{CANCELLED, CancelToken, Operation, Prompt, Reporter};
use crate::records::ProjectRecords;

/// How long putting a step's backup back may take as an undo step - a whole site and its database (a BACPAC import).
pub const RESTORE_LIMIT: Duration = Duration::from_secs(20 * 60);

/// How long the site's worker process gets to end before it is ended by force.
const WORKER_EXIT: Duration = Duration::from_secs(60);

const NL: &str = "\r\n";

/// What to upgrade and to which DNN (`UpgradeDnnRequest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeDnnRequest {
    /// The IIS site's name.
    pub site_name: String,
    /// The folder the site serves.
    pub directory: PathBuf,
    /// The GitHub releases API of the repository the versions come from.
    pub release_api_url: String,
    /// The version to end at, e.g. `v10.3.3` - reached through every step of DNN's upgrade path.
    pub version: String,
}

/// A DNN version as the upgrade path has it: major, minor and build (`DnnUpgradePath.Normalize`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DnnNumber {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}

impl DnnNumber {
    pub const fn new(major: u32, minor: u32, build: u32) -> Self {
        Self { major, minor, build }
    }

    /// `DnnInstall.Number` then `Normalize`: `10.1.0` of `v10.1.0-rc1`, `10.3.0` of `10.3` - None when it has no number.
    pub fn of(version: &str) -> Option<Self> {
        let text = version.trim_start_matches(['v', 'V']);
        let mut parts = Vec::new();
        for part in text.split('.') {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() {
                break;
            }
            parts.push(digits.parse::<u32>().ok().filter(|n| *n <= i32::MAX as u32)?);
            if digits.len() < part.len() || parts.len() == 4 {
                break;
            }
        }
        if parts.len() < 2 {
            return None;
        }
        Some(Self::new(parts[0], parts[1], parts.get(2).copied().unwrap_or(0)))
    }

    /// `Short`: 10.3.3.
    pub fn short(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.build)
    }

    /// DNN's way of writing a version in the path (`DnnUpgradeStep.Name`): 09.13.09.
    pub fn name(&self) -> String {
        format!("{:02}.{:02}.{:02}", self.major, self.minor, self.build)
    }
}

/// How a finding weighs on the upgrade (`UpgradeFindingSeverity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Compatible,
    Warning,
    Blocking,
    Unknown,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Something the analyser or the checks found (`UpgradeFinding`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub area: String,
    pub title: String,
    pub detail: Option<String>,
    pub fix: Option<String>,
}

impl Finding {
    pub fn new(severity: Severity, area: &str, title: &str) -> Self {
        Self { severity, area: area.into(), title: title.into(), detail: None, fix: None }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} - {}", self.severity, self.area, self.title)?;
        if let Some(detail) = &self.detail {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

/// How a step is carried out (`DnnUpgradeMethod`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StepMethod {
    /// The release's `…_Upgrade.zip` over the site's files.
    #[default]
    UpgradePackage,
    /// From DNN 10.2: the release's `…_Install.zip` as DNN's own local upgrade puts it in.
    LocalUpgrade,
    /// Not carried out by DNN Manager.
    Manual,
}

/// A step of the plan with what was checked for it (`PlannedStep`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub from: DnnNumber,
    pub to: DnnNumber,
    pub method: StepMethod,
    pub checks: Vec<Finding>,
}

impl fmt::Display for PlanStep {
    /// As `DnnUpgradeStep.ToString()`: `09.13.09 → 10.02.05`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} → {}", self.from.name(), self.to.name())
    }
}

/// The upgrade plan (`DnnUpgradePlan`): the site's version, the target, the steps between them each with its checks,
/// what was found about the site as a whole, and the plan as text (`Describe`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpgradePlan {
    pub current: DnnNumber,
    pub target: DnnNumber,
    pub steps: Vec<PlanStep>,
    pub site: Vec<Finding>,
    /// `DnnUpgradePlan.Describe()`, lines joined with CRLF.
    pub description: String,
}

impl UpgradePlan {
    pub fn all(&self) -> impl Iterator<Item = &Finding> {
        self.site.iter().chain(self.steps.iter().flat_map(|s| &s.checks))
    }

    pub fn can_start(&self) -> bool {
        !self.steps.is_empty() && self.all().all(|f| f.severity != Severity::Blocking)
    }

    pub fn blocking(&self) -> Vec<&Finding> {
        self.all().filter(|f| f.severity == Severity::Blocking).collect()
    }
}

/// What the site's web.config says about its database, as far as signing in to it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteDatabase {
    pub server: String,
    pub windows_authentication: bool,
    pub local_db_file: bool,
}

/// The site on this machine through its first http binding with a host name (`DnnSiteAddress`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteAddress {
    pub directory: PathBuf,
    /// `host[:port]`, the port only when it isn't 80.
    pub alias: String,
    pub port: u16,
}

/// What the stage's notes say about the site as it was before the step (`DnnSiteState`, as `WriteStageNotesAsync`
/// writes it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BeforeNotes {
    /// The content counts as text; None when they couldn't be counted.
    pub counts: Option<String>,
    /// Pages, modules and scheduled jobs recorded to compare with (module types not counted).
    pub recorded: usize,
    pub extensions: Vec<String>,
    /// Each page checked: its address and what was wrong with it (None: fine).
    pub pages: Vec<(String, Option<String>)>,
}

/// Why a step most likely failed, and what to do (`DnnUpgradeDiagnosis`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnosis {
    pub cause: String,
    pub fixes: Vec<String>,
}

/// A step's backup to put back (`RestoreBackupRequest` with `SignInAgreed`: the user agreed before the upgrade began).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRestore {
    pub site_name: String,
    pub directory: PathBuf,
    pub site_zip: PathBuf,
    pub database: Option<PathBuf>,
}

/// What the upgrade acts on besides IIS and the project records: the analyser, the releases, the backups (Export and
/// Restore backup), DNN's packages and installer, and the checks after each step.
pub trait UpgradeSystem: Send + Sync + 'static {
    /// The site as captured right before a step - what the checks after it compare with.
    type Before: Send;

    /// The database the site's web.config names; None when it names none.
    fn database(&self, site_name: &str, directory: &Path) -> Option<SiteDatabase>;
    /// `SqlServerAddress.MaySignInAsUser` with the settings' database server.
    fn may_sign_in_as_user(&self, server: &str) -> bool;
    /// `SqlServerAddress.SignInQuestion`.
    fn sign_in_question(&self, server: &str, why: &str) -> String;

    /// Inspects the site and plans the upgrade to `target` with the path's edits from Settings.
    fn plan(&self, site_name: &str, directory: &Path, target: DnnNumber) -> UpgradePlan;
    /// Whether `version` was added to the path in Settings (a version DNN may never have released).
    fn added_in_settings(&self, version: DnnNumber) -> bool;

    fn list_releases(&self, api_url: &str, cancel: &CancelToken) -> Result<Vec<DnnRelease>, String>;
    fn get_release(&self, api_url: &str, version: &str, cancel: &CancelToken) -> Result<DnnRelease, String>;

    /// `Documents\DnnManager\backups\<site>`.
    fn backup_directory(&self, site_name: &str) -> PathBuf;
    /// `DateTime.Now`.
    fn now(&self) -> Stamp;
    /// `ExportProjectUseCase`: the step's backup.
    fn export(&self, request: &ExportProjectRequest, op: &mut Operation) -> Result<(), String>;
    /// `RestoreBackupUseCase`: the step's backup put back.
    fn restore(&self, request: &StepRestore, op: &mut Operation) -> Result<(), String>;

    fn capture(&self, site: &SiteAddress, cancel: &CancelToken) -> Result<Self::Before, String>;
    fn notes_of(&self, before: &Self::Before) -> BeforeNotes;
    /// The checks after a step to `expected`; Err only for a cancel.
    #[allow(clippy::too_many_arguments)]
    fn validate(
        &self,
        site: &SiteAddress,
        before: &Self::Before,
        expected: DnnNumber,
        since: SystemTime,
        app_pool: &str,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<Vec<Finding>, String>;
    fn collect_diagnostics(
        &self,
        directory: &Path,
        app_pool: &str,
        since: SystemTime,
        folder: &Path,
    ) -> Result<Vec<String>, String>;
    fn explain(&self, error: &str, evidence: &[String]) -> Diagnosis;

    /// The step's files: the upgrade package, or (`local_upgrade`) the install package as DNN's local upgrade does.
    fn extract(
        &self,
        release: &DnnRelease,
        directory: &Path,
        local_upgrade: bool,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// `Install.aspx?mode=upgrade`, its whole answer kept in `output`.
    fn upgrade(
        &self,
        site: &SiteAddress,
        output: &Path,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// The first visit after the restart.
    fn warm_up(
        &self,
        site: &SiteAddress,
        since: SystemTime,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// DNN's installer pages and its copies of web.config removed after a step passed.
    fn clean_up(&self, directory: &Path);
}

/// What the upgrade uses.
pub struct UpgradeDnnDeps<S: UpgradeSystem> {
    pub iis: Arc<dyn IisManager>,
    pub records: Arc<dyn ProjectRecords>,
    pub system: Arc<S>,
}

// Servers the user agreed to sign in to with their Windows account - for as long as DNN Manager runs: the upgrade
// dialog plans again at every version picked.
static AGREED_SERVERS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// None when DNN Manager may sign in to `database` - the one the site's web.config names - else why it doesn't. With
/// Windows authentication to a server that isn't on this PC and isn't the one in Settings → Database server, the user is
/// asked first: the site's app pool can change web.config, and that server would get their sign-in.
fn sign_in_refused<S: UpgradeSystem>(
    system: &S,
    database: Option<&SiteDatabase>,
    why: &str,
    prompt: &dyn Prompt,
) -> Option<String> {
    let database = database.filter(|d| !d.local_db_file && d.windows_authentication)?;
    let server = &database.server;
    let agreed = AGREED_SERVERS.lock().map(|s| s.contains(&server.to_lowercase())).unwrap_or(false);
    if system.may_sign_in_as_user(server) || agreed {
        return None;
    }
    if prompt.confirm(&system.sign_in_question(server, why), &format!("Sign in to {server}"), "Don't sign in", false) {
        if let Ok(mut agreed) = AGREED_SERVERS.lock() {
            agreed.insert(server.to_lowercase());
        }
        return None;
    }
    Some(format!(
        "Not signed in to {server} with your Windows account - the upgrade reads and backs up its database there. Nothing was changed."
    ))
}

/// What the analyser makes of the site - the plan, before anything changes (`PlanAsync`). The dialog shows it.
pub fn plan<S: UpgradeSystem>(
    deps: &UpgradeDnnDeps<S>,
    site_name: &str,
    directory: &Path,
    target: DnnNumber,
    prompt: &dyn Prompt,
) -> Result<UpgradePlan, String> {
    let database = deps.system.database(site_name, directory);
    if let Some(refused) =
        sign_in_refused(deps.system.as_ref(), database.as_ref(), "Analysing the site for the upgrade", prompt)
    {
        return Err(refused);
    }
    Ok(deps.system.plan(site_name, directory, target))
}

/// The upgrade (`ExecuteAsync`).
pub fn upgrade_dnn<S: UpgradeSystem>(
    deps: &UpgradeDnnDeps<S>,
    req: &UpgradeDnnRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let system = deps.system.as_ref();
    // ─── Analyse and plan - nothing changes until it has passed ───
    let Some(target) = DnnNumber::of(&req.version) else {
        return Err(format!("'{}' isn't a DNN version.", req.version));
    };
    let Some(site) = deps.iis.site_details(&req.site_name) else {
        return Err(format!("IIS has no site named '{}'.", req.site_name));
    };
    let database = system.database(&req.site_name, &req.directory);
    if let Some(refused) = sign_in_refused(system, database.as_ref(), "Upgrading DNN", op.prompt) {
        return Err(refused);
    }
    let plan = system.plan(&req.site_name, &req.directory, target);
    let stages: Vec<String> =
        std::iter::once("Analyse".to_string()).chain(plan.steps.iter().map(|s| s.to.name())).collect();
    op.report.plan(&stages.iter().map(String::as_str).collect::<Vec<_>>());
    op.report.fact(
        "Upgrade",
        &format!("DNN {} → {} in {} step(s)", plan.current.name(), plan.target.name(), plan.steps.len()),
    );
    op.report.step("Analysing the site and planning the upgrade");
    for line in plan.description.lines() {
        if !line.is_empty() {
            op.report.info(line);
        }
    }
    if !plan.can_start() {
        let blocking = plan.blocking();
        for b in &blocking {
            let fix = b.fix.as_ref().map(|f| format!(" → {f}")).unwrap_or_default();
            op.report.fail(&format!("{}: {}{fix}", b.area, b.title));
        }
        let titles: Vec<String> = blocking.iter().map(|b| format!("{}.", b.title)).collect();
        return Err(format!("The upgrade didn't start - nothing was changed: {}", titles.join(" ")));
    }
    let (Some(address), Some(_)) = (address_of(&site, &req.directory), database) else {
        return Err("The site has no http binding with a host name, or no database - nothing was changed.".into());
    };
    // Every step's release first: a chain that can't be finished isn't started. One list from GitHub for all the steps
    // (its API allows 60 requests an hour) - a release it doesn't hold is asked for on its own.
    let listed = system.list_releases(&req.release_api_url, &op.cancel);
    let mut releases = Vec::new();
    for planned in &plan.steps {
        let known = listed
            .as_ref()
            .ok()
            .and_then(|list| list.iter().find(|r| DnnNumber::of(&r.version) == Some(planned.to)).cloned());
        let found = match known {
            Some(release) => Ok(release),
            None => system.get_release(&req.release_api_url, &planned.to.short(), &op.cancel),
        };
        match found {
            Ok(release) => releases.push(release),
            Err(e) if e == CANCELLED => return Err(e),
            Err(e) => {
                // A version typed in Settings may be one DNN never released.
                let yours = if system.added_in_settings(planned.to) {
                    " It was added in Settings → DNN releases - check it there."
                } else {
                    ""
                };
                return Err(format!(
                    "DNN {}, a step of the upgrade path, wasn't found: {e}{yours} Nothing was changed.",
                    planned.to.short()
                ));
            }
        }
    }
    op.report.success(&format!("The plan holds: {} step(s), nothing blocking.", plan.steps.len()));

    let mut done: Vec<String> = Vec::new();
    let mut stage_backups: Vec<(String, PathBuf)> = Vec::new();
    for (i, (planned, release)) in plan.steps.iter().zip(&releases).enumerate() {
        op.cancel.check()?;
        let context = StepContext { req, site: &site, address: &address, plan: &plan, step: planned, number: i + 1 };
        let result = run_step(deps, &context, release, op)?;
        if let Some(folder) = &result.folder {
            stage_backups.push((planned.from.short(), folder.clone()));
        }
        if !result.passed {
            let reached = if !result.put_back {
                let folder = result.folder.as_ref().map(|f| f.display().to_string());
                format!(
                    "The site may be half-upgraded: put it back with Restore backup ({}).",
                    folder.unwrap_or_else(|| "the step's backup".into())
                )
            } else if done.is_empty() {
                format!("The site is at DNN {} again, as it was.", planned.from.short())
            } else {
                format!(
                    "The site is at DNN {} - the steps before it ({}) stay done.",
                    planned.from.short(),
                    done.join(", ")
                )
            };
            let backups: Vec<String> =
                stage_backups.iter().map(|(from, folder)| format!("DNN {from}: {}", folder.display())).collect();
            op.report.info(&format!("Backups to go back further: {}", backups.join("; ")));
            return Err(format!("Step {} ({planned}) failed: {} {reached}", i + 1, result.error.unwrap_or_default()));
        }
        done.push(planned.to_string());
    }

    op.report.step("Upgrade complete");
    op.report.success(&format!(
        "'{}' runs DNN {} (was {}) - {} step(s): {}.",
        req.site_name,
        plan.target.short(),
        plan.current.short(),
        done.len(),
        done.join(", ")
    ));
    for (from, folder) in &stage_backups {
        op.report
            .info(&format!("Backup of DNN {from}: {} - Restore backup puts the site back to it.", folder.display()));
    }
    Ok(())
}

/// The site on this machine through its first http binding with a host name - as DNN's installer is run.
pub fn address_of(site: &IisSiteDetails, directory: &Path) -> Option<SiteAddress> {
    site.bindings
        .iter()
        .find(|b| b.protocol.eq_ignore_ascii_case("http") && b.port.is_some() && !b.host.is_empty())
        .map(|b| {
            let port = b.port.unwrap_or(80);
            let alias = if port == 80 { b.host.clone() } else { format!("{}:{port}", b.host) };
            SiteAddress { directory: directory.to_path_buf(), alias, port }
        })
}

struct StepContext<'a> {
    req: &'a UpgradeDnnRequest,
    site: &'a IisSiteDetails,
    address: &'a SiteAddress,
    plan: &'a UpgradePlan,
    step: &'a PlanStep,
    number: usize,
}

/// How a step went; `put_back`: the site is as it was before it (it passed, never changed, or was put back).
struct StepResult {
    passed: bool,
    error: Option<String>,
    folder: Option<PathBuf>,
    put_back: bool,
}

/// A line the restore wrote while it ran as an undo step - written to the Output once it is done.
enum Line {
    Step(String),
    Info(String),
    Success(String),
    Warn(String),
    Fail(String),
}

/// The restore's reporter: it runs on the undo's own thread, where the operation's reporter can't go.
#[derive(Clone, Default)]
struct Kept(Arc<Mutex<Vec<Line>>>);

impl Kept {
    fn push(&self, line: Line) {
        if let Ok(mut lines) = self.0.lock() {
            lines.push(line);
        }
    }

    fn write_to(&self, reporter: &mut dyn Reporter) {
        let lines = self.0.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default();
        for line in lines {
            match line {
                Line::Step(t) => reporter.step(&t),
                Line::Info(t) => reporter.info(&t),
                Line::Success(t) => reporter.success(&t),
                Line::Warn(t) => reporter.warn(&t),
                Line::Fail(t) => reporter.fail(&t),
            }
        }
    }
}

impl Reporter for Kept {
    fn step(&mut self, title: &str) {
        self.push(Line::Step(title.into()));
    }
    fn info(&mut self, text: &str) {
        self.push(Line::Info(text.into()));
    }
    fn success(&mut self, text: &str) {
        self.push(Line::Success(text.into()));
    }
    fn warn(&mut self, text: &str) {
        self.push(Line::Warn(text.into()));
    }
    fn fail(&mut self, text: &str) {
        self.push(Line::Fail(text.into()));
    }
}

/// Putting a backup back asks nothing: the user agreed to signing in before the upgrade began.
struct NoQuestions;

impl Prompt for NoQuestions {
    fn confirm(&self, _: &str, _: &str, _: &str, _: bool) -> bool {
        false
    }
    fn choose(&self, _: &str, _: &[&str]) -> Option<usize> {
        None
    }
}

/// What a step needs once its backup is made.
struct Stage {
    folder: PathBuf,
    app_pool: String,
    was_running: bool,
    since: SystemTime,
    restore_lines: Kept,
}

/// One step: backup, files, DNN's upgrade, restart, checks - and on failure the diagnosis and the way back. Err: a
/// cancel, which the runner undoes.
fn run_step<S: UpgradeSystem>(
    deps: &UpgradeDnnDeps<S>,
    c: &StepContext,
    release: &DnnRelease,
    op: &mut Operation,
) -> Result<StepResult, String> {
    let system = deps.system.as_ref();
    let (req, step) = (c.req, c.step);
    let app_pool = c.site.pool.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| req.site_name.clone());

    // ─── Backup, and what the site was like ───
    op.report.step(&format!("Step {}: {step} - backing up DNN {}", c.number, step.from.short()));
    let folder = backups::new_folder(&system.backup_directory(&req.site_name), &req.site_name, system.now());
    let backup = StepRestore {
        site_name: req.site_name.clone(),
        directory: req.directory.clone(),
        site_zip: folder.join(backups::site_zip_name(&req.site_name)),
        database: Some(folder.join(backups::database_name(&req.site_name, ".bacpac"))),
    };
    let export = ExportProjectRequest {
        project_name: req.site_name.clone(),
        project_directory: req.directory.clone(),
        zip_path: Some(backup.site_zip.clone()),
        bacpac_path: backup.database.clone(),
    };
    match system.export(&export, op) {
        Ok(()) => {}
        Err(e) if e == CANCELLED || op.cancel.is_cancelled() => return Err(CANCELLED.into()),
        Err(e) => {
            return Ok(StepResult {
                passed: false,
                error: Some(format!("The backup failed, so the step didn't start - nothing was changed: {e}")),
                folder: None,
                put_back: true,
            });
        }
    }
    // The backup stays whatever comes next: it is this step's way back.
    op.undo.keep();
    let before = system.capture(c.address, &op.cancel)?;
    write_stage_notes(system, &folder, req, step, c.plan, &system.notes_of(&before))
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    op.report.success(&format!("Backup of DNN {}: {}", step.from.short(), folder.display()));

    // From here on the site changes: a failure or a cancel puts this backup back - and a site that was stopped stays so.
    let was_running = states::is_started(&c.site.state);
    let restore_lines = Kept::default();
    {
        let (system, iis, lines) = (Arc::clone(&deps.system), Arc::clone(&deps.iis), restore_lines.clone());
        let site_name = req.site_name.clone();
        // A whole site and its database put back: it has longer than an ordinary undo step. Once started it runs to
        // the end - a half-restored site is worse than either.
        op.undo.add(
            format!("Put '{}' back as DNN {} (the step's backup)", req.site_name, step.from.short()),
            RESTORE_LIMIT,
            move |_| {
                let mut reporter = lines;
                let mut restore = Operation::new(&mut reporter, &NoQuestions, CancelToken::new());
                let restored = system.restore(&backup, &mut restore);
                if !was_running {
                    let _ = iis.stop_site(&site_name);
                }
                restored
            },
        );
    }
    let stage = Stage { folder, app_pool, was_running, since: SystemTime::now(), restore_lines };
    match change_and_check(deps, c, release, &stage, &before, op) {
        Ok(result) => Ok(result),
        Err(e) if e == CANCELLED || op.cancel.is_cancelled() => Err(CANCELLED.into()),
        // Whatever it was (an address the checks can't read, a folder they may not write): the site is put back.
        Err(e) => Ok(fail(deps, c, &stage, &format!("Unexpected error: {e}"), &[], op)),
    }
}

/// The step's changes - the new files, DNN's upgrade, the restart - and its checks; a failure puts the backup back.
fn change_and_check<S: UpgradeSystem>(
    deps: &UpgradeDnnDeps<S>,
    c: &StepContext,
    release: &DnnRelease,
    stage: &Stage,
    before: &S::Before,
    op: &mut Operation,
) -> Result<StepResult, String> {
    let system = deps.system.as_ref();
    let (req, step, n) = (c.req, c.step, c.number);
    let to = step.to.short();

    // ─── The new files ───
    let how = if step.method == StepMethod::LocalUpgrade {
        " (DNN's local upgrade, from its install package)"
    } else {
        " (its upgrade package)"
    };
    op.report.step(&format!("Step {n}: putting in DNN {to}'s files{how}"));
    if let Err(e) = stop_and_free(deps.iis.as_ref(), &req.site_name, &req.directory, op.report) {
        return Ok(fail(deps, c, stage, &format!("Could not stop the site for the new files: {e}"), &[], op));
    }
    let local = step.method == StepMethod::LocalUpgrade;
    let files = system.extract(release, &req.directory, local, op.report, &op.cancel);
    if files.as_ref().is_err_and(|e| e == CANCELLED) {
        return Err(CANCELLED.into());
    }
    let started = deps.iis.start_site(&req.site_name);
    if let Err(e) = files {
        return Ok(fail(deps, c, stage, &e, &[], op));
    }
    if let Err(e) = started {
        return Ok(fail(deps, c, stage, &format!("Could not start the site to upgrade it: {e}"), &[], op));
    }

    // ─── DNN's upgrade ───
    op.report.step(&format!("Step {n}: running DNN's upgrade to {to}"));
    let output = stage.folder.join(format!("dnn-upgrade-output-{to}.html"));
    let upgraded = system.upgrade(c.address, &output, op.report, &op.cancel);
    if upgraded.as_ref().is_err_and(|e| e == CANCELLED) {
        return Err(CANCELLED.into());
    }
    op.report.info(&format!("DNN's output, as it answered: {}", output.display()));
    if let Err(e) = upgraded {
        return Ok(fail(deps, c, stage, &e, &[], op));
    }

    // ─── Restart - DNN leaves its lock behind, held open by its worker process, then the checks ───
    op.report.step(&format!("Step {n}: restarting the site"));
    if let Err(e) = stop_and_free(deps.iis.as_ref(), &req.site_name, &req.directory, op.report) {
        return Ok(fail(deps, c, stage, &e, &[], op));
    }
    if let Err(e) = deps.iis.start_site(&req.site_name) {
        return Ok(fail(deps, c, stage, &format!("The site didn't start again: {e}"), &[], op));
    }
    match system.warm_up(c.address, stage.since, op.report, &op.cancel) {
        Ok(()) => {}
        Err(e) if e == CANCELLED => return Err(e),
        Err(e) => return Ok(fail(deps, c, stage, &e, &[], op)),
    }

    op.report.step(&format!("Step {n}: checking DNN {to}"));
    let found = system.validate(c.address, before, step.to, stage.since, &stage.app_pool, op.report, &op.cancel)?;
    let blocking: Vec<&Finding> = found.iter().filter(|f| f.severity == Severity::Blocking).collect();
    if !blocking.is_empty() {
        let error = blocking
            .iter()
            .map(|b| match &b.detail {
                None => format!("{}.", b.title),
                Some(detail) => format!("{} ({detail}).", b.title),
            })
            .collect::<Vec<_>>()
            .join(" ");
        let evidence: Vec<String> = found.iter().map(Finding::to_string).collect();
        return Ok(fail(deps, c, stage, &error, &evidence, op));
    }

    // ─── Passed: DNN's installer pages and its copies of web.config go; this version is the next step's start ───
    system.clean_up(&req.directory);
    if !stage.was_running {
        let _ = deps.iis.stop_site(&req.site_name);
    }
    if let Some(mut record) = deps.records.find(&req.site_name) {
        record.dnn_version = Some(to.clone());
        deps.records.save(&record);
    }
    op.undo.keep();
    append(
        &stage.folder.join(NOTE_FILE),
        &format!("{NL}Result: passed - the site ran DNN {to} after this step ({}).{NL}", system.now().display()),
    )
    .map_err(|e| e.to_string())?;
    op.report.success(&format!("Step {n} passed: '{}' runs DNN {to}.", req.site_name));
    Ok(StepResult { passed: true, error: None, folder: Some(stage.folder.clone()), put_back: true })
}

/// A step failed: what tells why is kept with the stage's backup, the cause and fixes are said, and the site is put
/// back from the backup - the version that worked before the step.
fn fail<S: UpgradeSystem>(
    deps: &UpgradeDnnDeps<S>,
    c: &StepContext,
    stage: &Stage,
    error: &str,
    evidence: &[String],
    op: &mut Operation,
) -> StepResult {
    let system = deps.system.as_ref();
    let step = c.step;
    op.report.fail(error);
    let kept = stage.folder.join("failed-upgrade");
    let mut diagnosis = system.explain(error, evidence);
    // What tells why is a help, not a must: whatever goes wrong collecting it, the site is still put back below.
    let collected = (|| -> Result<(), String> {
        let mut diagnostics = system.collect_diagnostics(&c.req.directory, &stage.app_pool, stage.since, &kept)?;
        // Where DNN's own output ended - its last lines say what it was doing when it stopped.
        let output = stage.folder.join(format!("dnn-upgrade-output-{}.html", step.to.short()));
        if output.is_file() {
            let html = std::fs::read(&output).map_err(|e| e.to_string())?;
            let html = String::from_utf8_lossy(&html);
            diagnostics.extend(last_lines(&html, 8).into_iter().map(|l| format!("DNN's output: {l}")));
        }
        for line in &diagnostics {
            op.report.info(line);
        }
        let all: Vec<String> = diagnostics.iter().chain(evidence).cloned().collect();
        diagnosis = system.explain(error, &all);
        op.report.warn(&format!("Likely cause: {}", diagnosis.cause));
        for fix in &diagnosis.fixes {
            op.report.info(&format!("→ {fix}"));
        }
        std::fs::create_dir_all(&kept).map_err(|e| e.to_string())?;
        let fixes: String = diagnosis.fixes.iter().map(|f| format!("- {f}{NL}")).collect();
        let text = format!(
            "Step {step} failed at {}.{NL}{NL}{error}{NL}{NL}Likely cause: {}{NL}{fixes}{NL}{}",
            system.now().display(),
            diagnosis.cause,
            all.join(NL)
        );
        std::fs::write(kept.join("what-happened.txt"), text).map_err(|e| e.to_string())?;
        op.report.info(&format!("Kept to see why: {}", kept.display()));
        Ok(())
    })();
    if let Err(e) = collected {
        op.report.warn(&format!("Could not keep what tells why the step failed: {e}"));
    }

    op.report.step(&format!("Putting the site back as DNN {} (the step's backup)", step.from.short()));
    let back = op.undo.run(&mut *op.report, &CancelToken::new());
    stage.restore_lines.write_to(op.report);
    let error = if back {
        format!("{error} Likely cause: {}", diagnosis.cause)
    } else {
        format!(
            "{error} Likely cause: {} Putting the backup back didn't fully work - see the Output tab.",
            diagnosis.cause
        )
    };
    StepResult { passed: false, error: Some(error), folder: Some(stage.folder.clone()), put_back: back }
}

/// The stage's notes beside its backup: why it was made, the plan, and the site's inventory as it was.
fn write_stage_notes<S: UpgradeSystem>(
    system: &S,
    folder: &Path,
    req: &UpgradeDnnRequest,
    step: &PlanStep,
    plan: &UpgradePlan,
    before: &BeforeNotes,
) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let mut extensions = before.extensions.clone();
    extensions.sort_by_key(|e| e.to_lowercase());
    let extensions: Vec<String> = extensions.iter().map(|e| format!("  {e}")).collect();
    let pages: Vec<String> = before
        .pages
        .iter()
        .map(|(url, problem)| format!("  {url}: {}", problem.as_deref().unwrap_or("fine")))
        .collect();
    let mut notes = String::new();
    let mut line = |text: &str| {
        notes.push_str(text);
        notes.push_str(NL);
    };
    line(&format!("Before upgrading DNN {step} - {}", system.now().display()));
    line(&format!("Site: {} ({}), DNN {}", req.site_name, req.directory.display(), step.from.short()));
    line(&format!(
        "Content: {} ({} pages, modules and scheduled jobs recorded to compare with)",
        before.counts.as_deref().unwrap_or("couldn't be counted"),
        before.recorded
    ));
    line("");
    line(&format!("Extensions ({}):", before.extensions.len()));
    line(&extensions.join(NL));
    line("");
    line("Pages checked before the step:");
    line(&pages.join(NL));
    line("");
    line("The plan:");
    line(&plan.description);
    std::fs::write(folder.join(NOTE_FILE), notes)?;
    // web.config on its own too - the zip has it, but this is the one to compare with.
    let web_config = req.directory.join("web.config");
    if web_config.is_file() {
        std::fs::copy(&web_config, folder.join("web.config.before"))?;
    }
    Ok(())
}

fn append(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).create(true).open(file)?.write_all(text.as_bytes())
}

/// The site stopped and its worker process gone, and the installBlocker.lock DNN left deleted. DNN opens the lock
/// without closing it and, at the end of its upgrade, tries for a minute to delete it - which it can't while it holds
/// it open itself (tested on IIS: an IOException in its log). Only the worker process ending frees it; a lock left
/// makes DNN answer every request with "the site was accessed while an installation/upgrade was in progress".
fn stop_and_free(
    iis: &dyn IisManager,
    site_name: &str,
    directory: &Path,
    reporter: &mut dyn Reporter,
) -> Result<(), String> {
    iis.stop_site_and_wait(site_name, WORKER_EXIT).map_err(|e| e.0)?;
    let lock = directory.join("installBlocker.lock");
    let mut attempt = 1;
    while lock.exists() {
        match std::fs::remove_file(&lock) {
            Ok(()) => reporter.info(
                "Removed the installBlocker.lock DNN left behind - while it is there, DNN takes every visit for an upgrade in progress.",
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                if attempt == 10 {
                    return Err(format!(
                        "DNN's installBlocker.lock can't be deleted, even with the site stopped: {e} While it is there, DNN takes every visit for an upgrade in progress."
                    ));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        attempt += 1;
    }
    Ok(())
}

/// The last `count` lines of DNN's own output in its HTML answer - before ASP.NET's error page, if it broke off into
/// one: that page's head (its title, its styles) comes before its "Server Error in" heading (`LastLines`).
pub fn last_lines(html: &str, count: usize) -> Vec<String> {
    let mut text = html.to_string();
    for (open, close) in [("<head", "</head>"), ("<style", "</style>"), ("<script", "</script>"), ("<!--", "-->")] {
        text = remove_blocks(&text, open, close);
    }
    if let Some(cut) = find_ignore_case(&text, "Server Error in", 0) {
        text.truncate(cut);
    }
    let text = line_breaks(&text);
    let text = strip_tags(&text);
    let lines: Vec<String> =
        html_decode(&text).split('\n').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let skip = lines.len().saturating_sub(count);
    lines.into_iter().skip(skip).collect()
}

/// The byte index of `needle` (ASCII) in `text` from `from`, ignoring ASCII case.
fn find_ignore_case(text: &str, needle: &str, from: usize) -> Option<usize> {
    let (hay, needle) = (text.as_bytes(), needle.as_bytes());
    (from..=hay.len().saturating_sub(needle.len()))
        .find(|&i| i + needle.len() <= hay.len() && hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Removes `<open … close` blocks; an `<open` tag only when a name ends right after it (`<header` isn't `<head`).
fn remove_blocks(text: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(start) = find_ignore_case(text, open, at) {
        let after = text.as_bytes().get(start + open.len()).copied();
        let is_tag = open == "<!--" || !after.is_some_and(|b| b.is_ascii_alphanumeric());
        if !is_tag {
            out.push_str(&text[at..start + open.len()]);
            at = start + open.len();
            continue;
        }
        // Without its end it isn't a block (as the regex wouldn't match): left as it is.
        let Some(end) = find_ignore_case(text, close, start + open.len()) else { break };
        out.push_str(&text[at..start]);
        at = end + close.len();
    }
    out.push_str(&text[at..]);
    out
}

/// `<br>`, `</hN>`, `</p>`, `</div>` and `</li>` as line breaks.
fn line_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tag = &rest[start..];
        let Some(end) = tag.find('>') else {
            out.push_str(tag);
            return out;
        };
        let inner = tag[1..end].trim().to_ascii_lowercase();
        let inner = inner.trim_end_matches('/').trim_end();
        let breaks = inner == "br"
            || matches!(inner, "/p" | "/div" | "/li")
            || (inner.len() == 3 && inner.starts_with("/h") && inner.as_bytes()[2].is_ascii_digit());
        if breaks {
            out.push('\n');
        } else {
            out.push_str(&tag[..=end]);
        }
        rest = &tag[end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        match rest[start..].find('>') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `WebUtility.HtmlDecode` for what DNN's pages hold: named entities of HTML's basics and numeric ones.
fn html_decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let decoded = tail.find(';').filter(|&end| end <= 10).and_then(|end| {
            let entity = &tail[1..end];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iis::{IisBindingDetails, IisError, IisServerAction, IisSiteRuntime};
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{Outcome, run};
    use crate::records::{InstallMode, ProjectRecord};
    use std::collections::BTreeMap;

    fn v(text: &str) -> DnnNumber {
        DnnNumber::of(text).unwrap()
    }

    #[test]
    fn numbers_of_dnn_versions() {
        assert_eq!(DnnNumber::of("v10.1.0-rc1"), Some(DnnNumber::new(10, 1, 0)));
        assert_eq!(DnnNumber::of("10.3"), Some(DnnNumber::new(10, 3, 0)));
        assert_eq!(DnnNumber::of("9.3.2.24"), Some(DnnNumber::new(9, 3, 2)));
        assert_eq!(DnnNumber::of("next"), None);
        assert_eq!(DnnNumber::of("10"), None);
        assert_eq!(v("9.13.9").name(), "09.13.09");
        assert_eq!(v("09.13.09").short(), "9.13.9");
    }

    #[test]
    fn dnns_last_lines_before_aspnets_error_page() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body>Upgrading DNN<br/>\
                    Executing Script:10.03.00.SqlDataProvider &amp; more<br>\
                    <!-- hidden --><script>var a = 1;</script><p>Error!</p>\
                    <html><head><title>Lock</title></head><h1>Server Error in '/' Application.</h1>stack</body></html>";
        assert_eq!(
            last_lines(html, 8),
            vec!["Upgrading DNN", "Executing Script:10.03.00.SqlDataProvider & more", "Error!"]
        );
        assert_eq!(last_lines(html, 1), vec!["Error!"]);
        assert_eq!(last_lines("<header>kept</header>", 2), vec!["kept"]);
    }

    // ─── Stand-ins ───

    struct FakeIis {
        calls: Mutex<Vec<String>>,
        state: &'static str,
    }

    impl FakeIis {
        fn new(state: &'static str) -> Self {
            Self { calls: Mutex::new(vec![]), state }
        }
        fn call(&self, what: String) -> Result<(), IisError> {
            self.calls.lock().unwrap().push(what);
            Ok(())
        }
    }

    impl IisManager for FakeIis {
        fn is_available(&self) -> bool {
            true
        }
        fn server_state(&self) -> crate::iis::IisServerState {
            crate::iis::IisServerState::Running
        }
        fn site_runtimes(&self) -> Result<BTreeMap<String, IisSiteRuntime>, IisError> {
            Ok(BTreeMap::new())
        }
        fn start_site(&self, site: &str) -> Result<(), IisError> {
            self.call(format!("start {site}"))
        }
        fn stop_site(&self, site: &str) -> Result<(), IisError> {
            self.call(format!("stop {site}"))
        }
        fn stop_site_and_wait(&self, site: &str, _: Duration) -> Result<(), IisError> {
            self.call(format!("stop+wait {site}"))
        }
        fn restart_site(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn recycle_app_pool(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn control_server(&self, _: IisServerAction) -> Result<(), IisError> {
            Ok(())
        }
        fn create_site(&self, _: &str, _: &Path, _: &str, _: u16) -> Result<(), IisError> {
            Ok(())
        }
        fn remove_site(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn log_directory(&self, _: &str) -> Option<PathBuf> {
            None
        }
        fn app_pool_identity(&self, _: &str) -> String {
            String::new()
        }
        fn enable_user_profile(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn site_traffic(&self) -> std::collections::HashMap<String, crate::project::SiteTraffic> {
            Default::default()
        }
        fn requests_served(&self) -> std::collections::HashMap<String, u64> {
            Default::default()
        }
        fn grant_permissions(&self, _: &Path, _: &[String]) -> Result<(), IisError> {
            Ok(())
        }
        fn remove_app_pool_profile(&self, _: &str) -> Result<(), IisError> {
            Ok(())
        }
        fn site_details(&self, site: &str) -> Option<IisSiteDetails> {
            (site == "shop").then(|| IisSiteDetails {
                id: 1,
                name: "shop".into(),
                state: self.state.into(),
                physical_path: PathBuf::new(),
                config_path: String::new(),
                server_auto_start: true,
                preload_enabled: None,
                bindings: vec![IisBindingDetails::parse("http", "*:8080:shop.dnndev.me", false)],
                pool: None,
            })
        }
    }

    #[derive(Default)]
    struct FakeRecords(Mutex<Option<ProjectRecord>>);

    impl ProjectRecords for FakeRecords {
        fn find(&self, _: &str) -> Option<ProjectRecord> {
            self.0.lock().unwrap().clone()
        }
        fn save(&self, record: &ProjectRecord) {
            *self.0.lock().unwrap() = Some(record.clone());
        }
        fn remove(&self, _: &str) {}
    }

    /// A stand-in for everything else: records what it is asked, fails where it is told to.
    struct Fake {
        backups: PathBuf,
        plan: UpgradePlan,
        windows_server: Option<&'static str>,
        known: Vec<&'static str>,
        fail_export: bool,
        fail_upgrade_to: Option<&'static str>,
        blocking_check_at: Option<&'static str>,
        fail_restore: bool,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(backups: &Path, steps: &[(&str, &str)]) -> Self {
            let steps = steps
                .iter()
                .map(|(f, t)| PlanStep { from: v(f), to: v(t), method: StepMethod::UpgradePackage, checks: vec![] })
                .collect::<Vec<_>>();
            Self {
                backups: backups.to_path_buf(),
                plan: UpgradePlan {
                    current: steps[0].from,
                    target: steps[steps.len() - 1].to,
                    steps,
                    site: vec![Finding::new(Severity::Compatible, "IIS", "Fine")],
                    description: "Current: DNN 09.13.09\r\nTarget:  DNN 10.03.03".into(),
                },
                windows_server: None,
                known: vec!["9.13.9", "10.2.5", "10.3.3"],
                fail_export: false,
                fail_upgrade_to: None,
                blocking_check_at: None,
                fail_restore: false,
                calls: Mutex::new(vec![]),
            }
        }
        fn call(&self, what: impl Into<String>) {
            self.calls.lock().unwrap().push(what.into());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn release(version: &str) -> DnnRelease {
        DnnRelease {
            version: version.into(),
            tag_name: format!("v{version}"),
            download_url: format!("https://example.invalid/DNN_Platform_{version}_Install.zip"),
            prerelease: false,
            sha256: None,
            upgrade_sha256: None,
        }
    }

    impl UpgradeSystem for Fake {
        type Before = String;

        fn database(&self, _: &str, _: &Path) -> Option<SiteDatabase> {
            Some(SiteDatabase {
                server: self.windows_server.unwrap_or("localhost").into(),
                windows_authentication: self.windows_server.is_some(),
                local_db_file: false,
            })
        }
        fn may_sign_in_as_user(&self, server: &str) -> bool {
            server == "localhost"
        }
        fn sign_in_question(&self, server: &str, why: &str) -> String {
            format!("{why}: {server}?")
        }
        fn plan(&self, _: &str, _: &Path, _: DnnNumber) -> UpgradePlan {
            self.plan.clone()
        }
        fn added_in_settings(&self, version: DnnNumber) -> bool {
            version == v("10.3.3")
        }
        fn list_releases(&self, _: &str, _: &CancelToken) -> Result<Vec<DnnRelease>, String> {
            Ok(self.known.iter().take(1).map(|k| release(k)).collect())
        }
        fn get_release(&self, _: &str, version: &str, _: &CancelToken) -> Result<DnnRelease, String> {
            self.call(format!("get {version}"));
            if self.known.contains(&version) { Ok(release(version)) } else { Err("Not Found.".into()) }
        }
        fn backup_directory(&self, site: &str) -> PathBuf {
            self.backups.join(site)
        }
        fn now(&self) -> Stamp {
            let n = self.calls.lock().unwrap().len() as u8;
            Stamp { year: 2026, month: 11, day: 16, hour: 12, minute: 0, second: n % 60 }
        }
        fn export(&self, request: &ExportProjectRequest, op: &mut Operation) -> Result<(), String> {
            self.call("export");
            if self.fail_export {
                return Err("SqlPackage failed.".into());
            }
            let folder = request.zip_path.as_ref().unwrap().parent().unwrap().to_path_buf();
            op.undo.delete_folder_on_undo(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            Ok(())
        }
        fn restore(&self, request: &StepRestore, op: &mut Operation) -> Result<(), String> {
            self.call(format!("restore {}", request.site_zip.file_name().unwrap().to_string_lossy()));
            op.report.info("Files put back.");
            if self.fail_restore { Err("the import failed".into()) } else { Ok(()) }
        }
        fn capture(&self, site: &SiteAddress, _: &CancelToken) -> Result<String, String> {
            self.call(format!("capture {}", site.alias));
            Ok("before".into())
        }
        fn notes_of(&self, _: &String) -> BeforeNotes {
            BeforeNotes {
                counts: Some("1 portal(s)".into()),
                recorded: 3,
                extensions: vec!["b".into(), "A".into()],
                pages: vec![("shop.dnndev.me:8080/".into(), None)],
            }
        }
        fn validate(
            &self,
            _: &SiteAddress,
            before: &String,
            expected: DnnNumber,
            _: SystemTime,
            _: &str,
            _: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<Vec<Finding>, String> {
            assert_eq!(before, "before");
            self.call(format!("validate {}", expected.short()));
            let mut found = vec![Finding::new(Severity::Compatible, "Version", "Files and database")];
            if self.blocking_check_at == Some(expected.short().as_str()) {
                let mut lost = Finding::new(Severity::Blocking, "Content", "Pages lost");
                lost.detail = Some("Home".into());
                found.push(lost);
            }
            Ok(found)
        }
        fn collect_diagnostics(&self, _: &Path, _: &str, _: SystemTime, _: &Path) -> Result<Vec<String>, String> {
            Ok(vec!["DNN's log: [ERROR] boom".into()])
        }
        fn explain(&self, _: &str, evidence: &[String]) -> Diagnosis {
            Diagnosis { cause: format!("{} clue(s)", evidence.len()), fixes: vec!["Look at the log.".into()] }
        }
        fn extract(
            &self,
            release: &DnnRelease,
            _: &Path,
            local: bool,
            _: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<(), String> {
            self.call(format!("extract {} {local}", release.version));
            Ok(())
        }
        fn upgrade(
            &self,
            site: &SiteAddress,
            output: &Path,
            _: &mut dyn Reporter,
            _: &CancelToken,
        ) -> Result<(), String> {
            let to =
                output.file_name().unwrap().to_string_lossy().replace("dnn-upgrade-output-", "").replace(".html", "");
            self.call(format!("upgrade {} {to}", site.alias));
            if self.fail_upgrade_to == Some(to.as_str()) {
                std::fs::write(output, "<p>Executing Script:10.03.00</p><p>Error!</p>").unwrap();
                return Err("DNN's upgrade reported an error.".into());
            }
            Ok(())
        }
        fn warm_up(&self, _: &SiteAddress, _: SystemTime, _: &mut dyn Reporter, _: &CancelToken) -> Result<(), String> {
            self.call("warm up");
            Ok(())
        }
        fn clean_up(&self, _: &Path) {
            self.call("clean up");
        }
    }

    struct Setup {
        dir: tempfile::TempDir,
        iis: Arc<FakeIis>,
        records: Arc<FakeRecords>,
        system: Arc<Fake>,
    }

    impl Setup {
        fn new(state: &'static str, fake: impl FnOnce(&Path) -> Fake) -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("site")).unwrap();
            std::fs::write(dir.path().join("site").join("web.config"), "<configuration />").unwrap();
            let records = Arc::new(FakeRecords::default());
            records.save(&ProjectRecord {
                site: "shop".into(),
                install_mode: InstallMode::Automatic,
                created_utc: String::new(),
                dnn_version: Some("9.13.9".into()),
                host_user_name: None,
            });
            let system = Arc::new(fake(&dir.path().join("backups")));
            Self { iis: Arc::new(FakeIis::new(state)), records, system, dir }
        }

        fn run(&self, version: &str, answer: &Answer) -> (Outcome, Vec<String>) {
            let deps = UpgradeDnnDeps {
                iis: self.iis.clone() as Arc<dyn IisManager>,
                records: self.records.clone() as Arc<dyn ProjectRecords>,
                system: self.system.clone(),
            };
            let req = UpgradeDnnRequest {
                site_name: "shop".into(),
                directory: self.dir.path().join("site"),
                release_api_url: "https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases".into(),
                version: version.into(),
            };
            let mut lines = Lines::default();
            let outcome =
                run(&mut lines, answer, CancelToken::new(), &CancelToken::new(), |op| upgrade_dnn(&deps, &req, op));
            (outcome, lines.0)
        }

        fn stage_folders(&self) -> Vec<PathBuf> {
            let mut folders: Vec<PathBuf> = std::fs::read_dir(self.dir.path().join("backups").join("shop"))
                .map(|d| d.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            folders.sort();
            folders
        }
    }

    #[test]
    fn every_step_is_backed_up_upgraded_restarted_and_checked_in_turn() {
        let setup = Setup::new("Started", |b| Fake::new(b, &[("9.13.9", "10.2.5"), ("10.2.5", "10.3.3")]));
        let (outcome, lines) = setup.run("v10.3.3", &Answer::yes());
        assert_eq!(outcome, Outcome::Finished, "{lines:#?}");
        assert_eq!(
            setup.system.calls(),
            vec![
                "get 10.2.5",
                "get 10.3.3",
                "export",
                "capture shop.dnndev.me:8080",
                "extract 10.2.5 false",
                "upgrade shop.dnndev.me:8080 10.2.5",
                "warm up",
                "validate 10.2.5",
                "clean up",
                "export",
                "capture shop.dnndev.me:8080",
                "extract 10.3.3 false",
                "upgrade shop.dnndev.me:8080 10.3.3",
                "warm up",
                "validate 10.3.3",
                "clean up",
            ]
        );
        assert_eq!(
            *setup.iis.calls.lock().unwrap(),
            ["stop+wait shop", "start shop", "stop+wait shop", "start shop"].repeat(2),
            "a site that ran is left running"
        );
        assert_eq!(setup.records.find("shop").unwrap().dnn_version.as_deref(), Some("10.3.3"));
        assert!(lines.contains(&"## Step 1: 09.13.09 → 10.02.05 - backing up DNN 9.13.9".to_string()));
        assert!(lines.contains(&"## Step 2: putting in DNN 10.3.3's files (its upgrade package)".to_string()));
        assert!(lines.contains(&"Current: DNN 09.13.09".to_string()));
        assert!(lines.contains(&"ok The plan holds: 2 step(s), nothing blocking.".to_string()));
        assert!(
            lines.contains(
                &"ok 'shop' runs DNN 10.3.3 (was 9.13.9) - 2 step(s): 09.13.09 → 10.02.05, 10.02.05 → 10.03.03."
                    .to_string()
            )
        );
        let folders = setup.stage_folders();
        assert_eq!(folders.len(), 2);
        let notes = std::fs::read_to_string(folders[0].join("backup.txt")).unwrap();
        assert!(notes.starts_with("Before upgrading DNN 09.13.09 → 10.02.05 - 2026-11-16 12:00:"), "{notes}");
        assert!(
            notes.contains("Content: 1 portal(s) (3 pages, modules and scheduled jobs recorded to compare with)\r\n")
        );
        assert!(notes.contains("Extensions (2):\r\n  A\r\n  b\r\n"));
        assert!(notes.contains("  shop.dnndev.me:8080/: fine\r\n"));
        assert!(notes.contains("\r\nResult: passed - the site ran DNN 10.2.5 after this step ("));
        assert_eq!(std::fs::read_to_string(folders[0].join("web.config.before")).unwrap(), "<configuration />");
    }

    #[test]
    fn a_failed_step_is_put_back_from_its_backup_and_the_steps_before_stay_done() {
        let setup = Setup::new("Stopped", |b| Fake {
            fail_upgrade_to: Some("10.3.3"),
            ..Fake::new(b, &[("9.13.9", "10.2.5"), ("10.2.5", "10.3.3")])
        });
        let (outcome, lines) = setup.run("10.3.3", &Answer::yes());
        let Outcome::Failed(error) = outcome else { panic!("{lines:#?}") };
        assert!(
            error.starts_with("Step 2 (10.02.05 → 10.03.03) failed: DNN's upgrade reported an error. Likely cause: 3 clue(s) The site is at DNN 10.2.5 - the steps before it (09.13.09 → 10.02.05) stay done."),
            "{error}"
        );
        let calls = setup.system.calls();
        assert_eq!(calls.last().unwrap(), "restore shop.zip");
        assert!(!calls.contains(&"validate 10.3.3".to_string()));
        // The stopped site was stopped again after the step that passed, and after the backup went back.
        let iis = setup.iis.calls.lock().unwrap().clone();
        assert_eq!(iis.iter().filter(|c| *c == "stop shop").count(), 2, "{iis:?}");
        assert_eq!(setup.records.find("shop").unwrap().dnn_version.as_deref(), Some("10.2.5"));
        let at = |text: &str| lines.iter().position(|l| l == text).unwrap_or_else(|| panic!("{text}: {lines:#?}"));
        assert!(at("! DNN's upgrade reported an error.") < at("DNN's output: Error!"));
        assert!(at("warn Likely cause: 3 clue(s)") < at("## Putting the site back as DNN 10.2.5 (the step's backup)"));
        assert!(at("ok Put 'shop' back as DNN 10.2.5 (the step's backup)") < at("Files put back."));
        let folders = setup.stage_folders();
        let happened = std::fs::read_to_string(folders[1].join("failed-upgrade").join("what-happened.txt")).unwrap();
        assert!(
            happened
                .contains("DNN's upgrade reported an error.\r\n\r\nLikely cause: 3 clue(s)\r\n- Look at the log.\r\n")
        );
        assert!(happened.contains("DNN's output: Executing Script:10.03.00"));
        assert!(lines.iter().any(|l| l.starts_with("Backups to go back further: DNN 9.13.9: ")));
    }

    #[test]
    fn a_blocking_check_fails_the_step_and_a_failed_restore_says_the_site_may_be_half_upgraded() {
        let setup = Setup::new("Started", |b| Fake {
            blocking_check_at: Some("10.2.5"),
            fail_restore: true,
            ..Fake::new(b, &[("9.13.9", "10.2.5")])
        });
        let (outcome, lines) = setup.run("10.2.5", &Answer::yes());
        let Outcome::Failed(error) = outcome else { panic!() };
        assert!(error.starts_with("Step 1 (09.13.09 → 10.02.05) failed: Pages lost (Home). Likely cause: 3 clue(s) Putting the backup back didn't fully work - see the Output tab. The site may be half-upgraded: put it back with Restore backup ("), "{error}");
        assert!(lines.contains(&"! Put 'shop' back as DNN 9.13.9 (the step's backup): the import failed".to_string()));
        assert_eq!(setup.records.find("shop").unwrap().dnn_version.as_deref(), Some("9.13.9"));
    }

    #[test]
    fn a_failed_backup_changes_nothing() {
        let setup = Setup::new("Started", |b| Fake { fail_export: true, ..Fake::new(b, &[("9.13.9", "10.2.5")]) });
        let (outcome, _) = setup.run("10.2.5", &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed("Step 1 (09.13.09 → 10.02.05) failed: The backup failed, so the step didn't start - nothing was changed: SqlPackage failed. The site is at DNN 9.13.9 again, as it was.".into())
        );
        assert!(setup.iis.calls.lock().unwrap().is_empty());
        assert_eq!(setup.system.calls(), vec!["get 10.2.5", "export"]);
    }

    #[test]
    fn nothing_starts_while_something_blocks_or_a_release_is_missing() {
        let setup = Setup::new("Started", |b| {
            let mut fake = Fake::new(b, &[("9.13.9", "10.2.5")]);
            let mut finding = Finding::new(Severity::Blocking, ".NET Framework", "DNN 10 needs .NET Framework 4.8");
            finding.fix = Some("Install .NET Framework 4.8".into());
            fake.plan.site.push(finding);
            fake
        });
        let (outcome, lines) = setup.run("10.2.5", &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed("The upgrade didn't start - nothing was changed: DNN 10 needs .NET Framework 4.8.".into())
        );
        assert!(
            lines.contains(&"! .NET Framework: DNN 10 needs .NET Framework 4.8 → Install .NET Framework 4.8".into())
        );
        assert!(setup.system.calls().is_empty());

        let setup = Setup::new("Started", |b| Fake { known: vec!["9.13.9"], ..Fake::new(b, &[("9.13.9", "10.3.3")]) });
        let (outcome, _) = setup.run("10.3.3", &Answer::yes());
        assert_eq!(
            outcome,
            Outcome::Failed("DNN 10.3.3, a step of the upgrade path, wasn't found: Not Found. It was added in Settings → DNN releases - check it there. Nothing was changed.".into())
        );
        let (outcome, _) = setup.run("next", &Answer::yes());
        assert_eq!(outcome, Outcome::Failed("'next' isn't a DNN version.".into()));
    }

    #[test]
    fn a_windows_sign_in_to_another_server_is_asked_first() {
        let setup = Setup::new("Started", |b| Fake {
            windows_server: Some("sql.other"),
            ..Fake::new(b, &[("9.13.9", "10.2.5")])
        });
        let no = Answer::no();
        let (outcome, _) = setup.run("10.2.5", &no);
        assert_eq!(
            outcome,
            Outcome::Failed("Not signed in to sql.other with your Windows account - the upgrade reads and backs up its database there. Nothing was changed.".into())
        );
        assert_eq!(no.1.lock().unwrap().as_slice(), ["Upgrading DNN: sql.other?"]);
        // A yes holds for the rest of the run: the plan doesn't ask again.
        let deps = UpgradeDnnDeps {
            iis: setup.iis.clone() as Arc<dyn IisManager>,
            records: setup.records.clone() as Arc<dyn ProjectRecords>,
            system: setup.system.clone(),
        };
        let yes = Answer::yes();
        assert!(plan(&deps, "shop", Path::new("x"), v("10.2.5"), &yes).is_ok());
        assert!(plan(&deps, "shop", Path::new("x"), v("10.2.5"), &no).is_ok());
        assert_eq!(yes.1.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_lock_left_behind_is_removed_when_the_site_stops() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("installBlocker.lock"), "").unwrap();
        let iis = FakeIis::new("Started");
        let mut lines = Lines::default();
        stop_and_free(&iis, "shop", dir.path(), &mut lines).unwrap();
        assert!(!dir.path().join("installBlocker.lock").exists());
        assert_eq!(lines.0.len(), 1);
    }
}
