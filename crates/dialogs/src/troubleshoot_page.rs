//! Troubleshoot (`Pages/TroubleshootPage.xaml(.cs)`), opened over the page like Settings: restart DNN Manager, clean
//! up the data it keeps in `Documents\DnnManager` (`Settings/AppDataCleaner.cs`, ported here), reset its settings, or
//! reset it to factory defaults. Nothing here touches a project's IIS site, folder or database.
//!
//! The view only asks: every button that changes something emits an event the app turns into its confirmation
//! dialog, and the app then runs the plain functions below - [`AppDataCleaner::clean`], [`reset_settings`],
//! [`forget_secrets_and_start`], [`factory_reset`] - off the UI thread, and tells the view what came of it.
//!
//! Deviations: a reset writes only `version` and the sa password - every other setting is then its default by having
//! no row (the WPF app writes them all back at its next start); the age of a backup folder named with its date is
//! read as UTC, so `delete_expired` can be a few hours off a day boundary.

use dnnm_ui::px;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString, Window, div, prelude::*};
use rusqlite::Connection;

use dnnm_application::project::view::byte_size;
use dnnm_system::files::safe_path::{has_link, is_link};
use dnnm_system::secrets::{self, SA_PASSWORD_KEY, names};
use dnnm_ui::dialog;
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, ButtonKind, Pad, Tip, icons};

use crate::settings_page::{Notice, SecretStore, SignInTask, hint};

// ─── AppDataCleaner ───────────────────────────────────────────────────────

/// What Clean up data can delete from `Documents\DnnManager` (`AppDataKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppDataKind {
    /// The activity log files (`logs\`).
    Logs,
    /// Kept DNN install packages (`packages\`), and the DNN versions saved with them (`dnn_releases`).
    DnnPackages,
    /// Project backups (`backups\<project>\`).
    ProjectBackups,
    /// Packages made by Export for deployment (`deployments\`).
    Deployments,
    /// Which sites are kept warm (`keep_warm`) - for a factory reset.
    KeepWarmChoices,
    /// What versions before 1.8 left: their settings files, the old workspace, damaged databases put aside.
    OldSettingsFiles,
}

/// What a clean-up freed, and how many files it couldn't delete (in use, or not allowed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupResult {
    pub freed: u64,
    pub skipped: usize,
}

/// What [`AppDataCleaner::delete_expired`] deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpiredResult {
    pub deleted: Vec<PathBuf>,
    pub freed: u64,
    pub skipped: usize,
}

const OLD_SETTINGS_FILE_NAMES: [&str; 4] =
    ["settings.json", "settings.json.bak", "appsettings.json", "docker-compose.yml"];

/// Measures and deletes DNN Manager's own data in `root` (`Documents\DnnManager`) and its database - never a
/// project's folder, IIS site or database, never through a link or junction.
#[derive(Debug, Clone)]
pub struct AppDataCleaner {
    root: PathBuf,
    database: PathBuf,
}

impl AppDataCleaner {
    pub fn new(root: impl Into<PathBuf>, database: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), database: database.into() }
    }

    /// The user's own: `Documents\DnnManager` and its `dnnmanager.db`.
    pub fn for_current_user() -> Self {
        Self::new(dnnm_infrastructure::paths::app_data_directory(), dnnm_infrastructure::paths::database_file())
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// There, and neither it nor a folder on the way from the root a link.
    fn usable(&self, folder: &Path) -> bool {
        folder.is_dir() && !has_link(&self.root, folder, None)
    }

    /// Every file in `folder`, hidden and system ones too - never through a link or junction.
    fn all_files(&self, folder: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if self.usable(folder) {
            walk(folder, &mut files);
        }
        files
    }

    fn safe_folders(folder: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(folder)
            .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !is_link(p)).collect())
            .unwrap_or_default()
    }

    fn old_settings_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if !self.usable(&self.root) {
            return files;
        }
        for path in top_level_files(&self.root) {
            let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let damaged = name.starts_with("dnnmanager.damaged-")
                && [".db", ".db-wal", ".db-shm", ".db-journal"].iter().any(|e| name.ends_with(e));
            if OLD_SETTINGS_FILE_NAMES.contains(&name.as_str()) || damaged {
                files.push(path);
            }
        }
        let state = self.dir("state");
        if self.usable(&state) {
            files.extend(
                top_level_files(&state)
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))),
            );
        }
        files
    }

    fn files(&self, kind: AppDataKind) -> Vec<PathBuf> {
        match kind {
            AppDataKind::Logs => self.all_files(&self.dir("logs")),
            AppDataKind::DnnPackages => self.all_files(&self.dir("packages")),
            AppDataKind::ProjectBackups | AppDataKind::Deployments => {
                let top = self.dir(if kind == AppDataKind::ProjectBackups { "backups" } else { "deployments" });
                if !self.usable(&top) {
                    return Vec::new();
                }
                Self::safe_folders(&top).iter().flat_map(|d| self.all_files(d)).collect()
            }
            AppDataKind::OldSettingsFiles => self.old_settings_files(),
            AppDataKind::KeepWarmChoices => Vec::new(),
        }
    }

    /// The table a kind has in the database, and what a row of it takes.
    fn table(kind: AppDataKind) -> Option<(&'static str, &'static str)> {
        match kind {
            AppDataKind::KeepWarmChoices => Some(("keep_warm", "length(site)")),
            AppDataKind::DnnPackages => {
                Some(("dnn_releases", "length(api) + length(version) + length(tag) + length(url)"))
            }
            _ => None,
        }
    }

    fn open(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open(&self.database)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        Ok(connection)
    }

    fn table_size(&self, connection: &Connection, table: (&str, &str)) -> rusqlite::Result<u64> {
        let size: Option<i64> =
            connection.query_row(&format!("SELECT SUM({}) FROM {}", table.1, table.0), [], |r| r.get(0))?;
        Ok(size.unwrap_or(0).max(0) as u64)
    }

    /// How much `kind` takes now, in bytes - its files, and what it has in the database.
    pub fn measure(&self, kind: AppDataKind) -> u64 {
        let mut bytes: u64 = self.files(kind).iter().map(|f| length(f)).sum();
        if let Some(table) = Self::table(kind)
            && self.database.is_file()
            && let Ok(size) = self.open().and_then(|c| self.table_size(&c, table))
        {
            bytes += size;
        }
        bytes
    }

    /// Deletes `kind`, then the folders it leaves empty.
    pub fn clean(&self, kind: AppDataKind) -> CleanupResult {
        let mut result = delete_files(&self.files(kind));
        for folder in self.emptied_folders(kind) {
            delete_empty_folders(&folder);
        }
        if let Some(table) = Self::table(kind) {
            let cleared = self.open().and_then(|c| {
                let size = self.table_size(&c, table)?;
                c.execute(&format!("DELETE FROM {}", table.0), [])?;
                Ok(size)
            });
            match cleared {
                Ok(size) => result.freed += size,
                Err(_) => result.skipped += 1,
            }
        }
        result
    }

    fn emptied_folders(&self, kind: AppDataKind) -> Vec<PathBuf> {
        match kind {
            AppDataKind::DnnPackages if self.usable(&self.dir("packages")) => vec![self.dir("packages")],
            AppDataKind::ProjectBackups if self.usable(&self.dir("backups")) => {
                Self::safe_folders(&self.dir("backups"))
            }
            AppDataKind::Deployments if self.usable(&self.dir("deployments")) => {
                Self::safe_folders(&self.dir("deployments"))
            }
            AppDataKind::OldSettingsFiles if self.usable(&self.dir("state")) => vec![self.dir("state")],
            _ => Vec::new(),
        }
    }

    /// Deletes the project backups (`backups\<project>\<project>_<date>\`) and deployment packages made more than
    /// `keep_days` days before `now` (`backups.keepDays`); nothing when it is 0. A folder's age is the date in its
    /// name, or when it was last written to.
    pub fn delete_expired(&self, keep_days: u32, now: SystemTime) -> ExpiredResult {
        let mut result = ExpiredResult::default();
        if keep_days == 0 {
            return result;
        }
        let before = now.checked_sub(Duration::from_secs(u64::from(keep_days) * 86_400)).unwrap_or(UNIX_EPOCH);
        let mut expired = Vec::new();
        let (backups, deployments) = (self.dir("backups"), self.dir("deployments"));
        if self.usable(&backups) {
            for project in Self::safe_folders(&backups) {
                expired.extend(Self::safe_folders(&project).into_iter().filter(|f| made_at(f) < before));
            }
        }
        if self.usable(&deployments) {
            expired.extend(Self::safe_folders(&deployments).into_iter().filter(|f| made_at(f) < before));
        }
        for folder in expired {
            let deleted = delete_files(&self.all_files(&folder));
            result.freed += deleted.freed;
            result.skipped += deleted.skipped;
            delete_empty_folders(&folder);
            if deleted.skipped == 0 {
                result.deleted.push(folder);
            }
        }
        // A project with no backup left keeps no empty folder.
        if self.usable(&backups) {
            for project in Self::safe_folders(&backups) {
                delete_empty_folders(&project);
            }
        }
        result
    }
}

fn walk(folder: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_link(&path) {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, files),
            Ok(t) if t.is_file() => files.push(path),
            _ => {}
        }
    }
}

fn top_level_files(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| !is_link(p) && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
                .collect()
        })
        .unwrap_or_default()
}

fn length(file: &Path) -> u64 {
    std::fs::metadata(file).map(|m| m.len()).unwrap_or(0)
}

fn delete_files(files: &[PathBuf]) -> CleanupResult {
    let mut result = CleanupResult::default();
    for file in files {
        let size = length(file);
        // A read-only file would refuse.
        if let Ok(meta) = std::fs::metadata(file) {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(file, permissions);
        }
        match std::fs::remove_file(file) {
            Ok(()) => result.freed += size,
            Err(_) => result.skipped += 1,
        }
    }
    result
}

/// Removes the empty folders inside `folder`, and it too when it is left empty - never through a link.
fn delete_empty_folders(folder: &Path) {
    if !folder.is_dir() || is_link(folder) {
        return;
    }
    for child in AppDataCleaner::safe_folders(folder) {
        delete_empty_folders(&child);
    }
    // In use (Explorer has it open): an empty folder costs nothing.
    if std::fs::read_dir(folder).is_ok_and(|mut e| e.next().is_none()) {
        let _ = std::fs::remove_dir(folder);
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// When a backup or package folder was made: the date in its name (`<project>_yyyyMMdd_HHmmss`), or when it was
/// last written to; never, when neither can be read.
pub fn made_at(folder: &Path) -> SystemTime {
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.len() > 15 && name.is_char_boundary(name.len() - 15) {
        let stamp = &name[name.len() - 15..];
        let n = |r: std::ops::Range<usize>| {
            stamp.get(r).filter(|s| s.bytes().all(|b| b.is_ascii_digit()))?.parse::<i64>().ok()
        };
        if stamp.as_bytes()[8] == b'_'
            && let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) =
                (n(0..4), n(4..6), n(6..8), n(9..11), n(11..13), n(13..15))
            && (1..=12).contains(&mo)
            && (1..=31).contains(&d)
            && h < 24
            && mi < 60
            && s < 60
        {
            let seconds = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s;
            if seconds >= 0 {
                return UNIX_EPOCH + Duration::from_secs(seconds as u64);
            }
        }
    }
    std::fs::metadata(folder)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::now() + Duration::from_secs(86_400 * 365 * 1000))
}

// ─── Resets ───────────────────────────────────────────────────────────────

/// Replaces the settings with the defaults (`SettingsStore.ResetToDefaults`): every row goes, the settings version
/// stays, and so does the container's sa password - the container was made with it - unless this Windows account
/// can't decrypt it; then `new_sa_password` (`SqlPasswords.New`, from the app) is kept, encrypted. Off Windows there
/// is no encryption for it (DPAPI): the settings are reset without an sa password - the SQL container is Windows'.
pub fn reset_settings(database: &Path, new_sa_password: &str) -> Result<(), String> {
    let mut connection = Connection::open(database).map_err(|e| e.to_string())?;
    connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let tx =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let kept: Option<String> =
        tx.query_row("SELECT value FROM settings WHERE key = ?1", [SA_PASSWORD_KEY], |r| r.get(0)).ok();
    let sa: Option<String> = match kept.filter(|v| secrets::decrypts(v)) {
        Some(kept) if !kept.is_empty() => Some(kept),
        #[cfg(windows)]
        _ => Some(secrets::protect(new_sa_password)?),
        #[cfg(not(windows))]
        _ => secrets::protect(new_sa_password).ok(),
    };
    tx.execute("DELETE FROM settings", []).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES ('version', ?1)",
        [dnnm_application::settings::CURRENT_VERSION.to_string()],
    )
    .map_err(|e| e.to_string())?;
    if let Some(sa) = sa {
        tx.execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", [SA_PASSWORD_KEY, sa.as_str()])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Removes the saved passwords and starting at sign-in (`ForgetSecretsAndStartAsync`) - what went wrong, if anything.
pub fn forget_secrets_and_start(secrets: &dyn SecretStore, sign_in: &dyn SignInTask) -> Vec<String> {
    let mut problems = Vec::new();
    for name in [names::DEFAULT_HOST_PASSWORD, names::DATABASE_SERVER_PASSWORD] {
        if let Err(e) = secrets.delete(name) {
            problems.push(e);
        }
    }
    if sign_in.target().is_some()
        && let Err(e) = sign_in.disable()
    {
        problems.push(format!("Starting at sign-in: {e}"));
    }
    problems
}

/// The remembered workspace (`WorkspaceService.Forget`): the window, the open page, unsaved form values - the next
/// start opens as a first one. The app stops saving its workspace before, and resumes it if the restart is refused.
pub fn forget_workspace(database: &Path) -> Result<(), String> {
    let connection = Connection::open(database).map_err(|e| e.to_string())?;
    connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    connection.execute("DELETE FROM state", []).map(|_| ()).map_err(|e| e.to_string())
}

/// Reset to factory defaults, all but the restart (`Reset_Click`): the settings first - Err("The settings could not
/// be reset: …") and nothing else is removed when that fails - then the passwords and sign-in, the logs, kept
/// packages, keep warm's sites, old settings files and the workspace. Ok with what couldn't be removed.
pub fn factory_reset(
    cleaner: &AppDataCleaner,
    new_sa_password: &str,
    secrets: &dyn SecretStore,
    sign_in: &dyn SignInTask,
) -> Result<Vec<String>, String> {
    reset_settings(&cleaner.database, new_sa_password).map_err(|e| format!("The settings could not be reset: {e}"))?;
    let mut problems = forget_secrets_and_start(secrets, sign_in);
    for kind in
        [AppDataKind::Logs, AppDataKind::DnnPackages, AppDataKind::KeepWarmChoices, AppDataKind::OldSettingsFiles]
    {
        cleaner.clean(kind);
    }
    if let Err(e) = forget_workspace(&cleaner.database) {
        problems.push(format!("The remembered workspace: {e}"));
    }
    Ok(problems)
}

/// The text of an error dialog for what a reset couldn't do: "<intro>\n\n• a\n• b".
pub fn problems_text(intro: &str, problems: &[String]) -> String {
    let list: Vec<String> = problems.iter().map(|p| format!("• {p}")).collect();
    format!("{intro}\n\n{}", list.join("\n"))
}

/// What Reset settings says when done: a toast, or the error dialog's text.
pub fn reset_settings_outcome(problems: &[String]) -> (Notice, String) {
    if problems.is_empty() {
        (Notice::Success, "The settings are back to their defaults.".into())
    } else {
        (Notice::Error, problems_text("The settings are reset, except:", problems))
    }
}

/// What Clean up data says when done.
pub fn cleanup_outcome(results: &[CleanupResult]) -> (Notice, String) {
    let freed: u64 = results.iter().map(|r| r.freed).sum();
    let skipped: usize = results.iter().map(|r| r.skipped).sum();
    let freed = byte_size(freed as f64);
    if skipped == 0 {
        (Notice::Success, format!("Cleaned up {freed}."))
    } else {
        (Notice::Warning, format!("Cleaned up {freed} - {skipped} file(s) are in use and were left."))
    }
}

pub const RESET_SETTINGS_QUESTION: &str = "Put every setting back to its default, as DNN Manager is installed?\n\nThe current settings aren't kept. The saved DNN host password and database server login go too, and DNN Manager stops starting at sign-in. The defaults apply at once - no restart.\n\nKept: your projects, the logs, the kept DNN packages and which sites are kept warm.";

pub const FACTORY_RESET_QUESTION: &str = "Reset DNN Manager to factory defaults?\n\nRemoved: the settings, the saved passwords (the DNN host password and the database server login's), starting at sign-in, which sites are kept warm, the logs, the kept DNN packages, older versions' settings files, and the remembered workspace (the window, the open page, unsaved form values).\n\nKept: your projects - their IIS sites, folders and databases - and their backups.\n\nDNN Manager restarts afterwards.";

/// Clean up data's question, for the kinds chosen (`Cleanup_Click`).
pub fn cleanup_question(root: &Path, chosen: &[AppDataKind]) -> String {
    let list: Vec<String> = chosen.iter().map(|k| format!("• {}", choice_name(*k))).collect();
    let backups = if chosen.iter().any(|k| matches!(k, AppDataKind::ProjectBackups | AppDataKind::Deployments)) {
        "\n\nBackups and deployment packages can't be brought back."
    } else {
        ""
    };
    format!("Delete this data from {}?\n\n{}{backups}", root.display(), list.join("\n"))
}

// ─── The page ─────────────────────────────────────────────────────────────

const CHOICES: [AppDataKind; 5] = [
    AppDataKind::Logs,
    AppDataKind::DnnPackages,
    AppDataKind::ProjectBackups,
    AppDataKind::Deployments,
    AppDataKind::OldSettingsFiles,
];

fn choice_name(kind: AppDataKind) -> &'static str {
    match kind {
        AppDataKind::Logs => "Logs",
        AppDataKind::DnnPackages => "Kept DNN packages",
        AppDataKind::ProjectBackups => "Project backups",
        AppDataKind::Deployments => "Deployment packages",
        AppDataKind::OldSettingsFiles => "Old settings files",
        AppDataKind::KeepWarmChoices => "Keep warm",
    }
}

fn choice_detail(kind: AppDataKind) -> &'static str {
    match kind {
        AppDataKind::Logs => "one file per day",
        AppDataKind::DnnPackages => "to install a version again offline",
        AppDataKind::ProjectBackups => "every project's .zip and .bacpac",
        AppDataKind::Deployments => "made by Export for deployment",
        _ => "left by versions before 1.8",
    }
}

/// An action's explanation: a tip beside its title, or - a warning about what it removes - shown under it.
#[derive(Debug, Clone, Copy)]
enum Explain {
    Tip(&'static str),
    Warning(&'static str),
}

/// What the page asks the app to do.
#[derive(Debug, Clone)]
pub enum TroubleshootEvent {
    Close,
    /// Restart DNN Manager (`AppRestart.Restart`: it asks as on quit).
    Restart,
    /// Ask `question` (danger: "Clean up data" / "Cancel"); on yes run [`AppDataCleaner::clean`] for each kind off the
    /// UI thread, then call [`TroubleshootPage::cleaned_up`].
    CleanUp {
        question: String,
        kinds: Vec<AppDataKind>,
    },
    /// Ask [`RESET_SETTINGS_QUESTION`] ("Reset settings" / "Cancel"); on yes: [`reset_settings`] (an error: "The
    /// settings could not be reset: …"), [`forget_secrets_and_start`], load and apply the defaults, then
    /// [`reset_settings_outcome`].
    ResetSettings,
    /// Ask [`FACTORY_RESET_QUESTION`] ("Reset to factory defaults" / "Cancel"); on yes: [`factory_reset`], the
    /// problems in an error ("DNN Manager is reset, except:", [`problems_text`]), then restart.
    FactoryReset,
}

pub struct TroubleshootPage {
    cleaner: AppDataCleaner,
    chosen: [bool; 5],
    sizes: Option<[u64; 5]>,
    measuring: u64,
    busy: Option<String>,
    cleaning: bool,
    focus: FocusHandle,
}

impl EventEmitter<TroubleshootEvent> for TroubleshootPage {}

impl Focusable for TroubleshootPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TroubleshootPage {
    /// `cleaner`: [`AppDataCleaner::for_current_user`]; `busy`: the operation running now, if any.
    pub fn new(cleaner: AppDataCleaner, busy: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        // Logs and packages are DNN Manager's own; project backups are the user's - never ticked for them.
        let mut page = Self {
            cleaner,
            chosen: [true, true, false, false, false],
            sizes: None,
            measuring: 0,
            busy,
            cleaning: false,
            focus,
        };
        page.measure(cx);
        page
    }

    /// The operation running now: the buttons wait until it has finished.
    pub fn set_busy(&mut self, busy: Option<String>, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }

    /// The app ran Clean up data: say what it freed and measure again.
    pub fn cleaned_up(&mut self, results: &[CleanupResult], cx: &mut Context<Self>) -> (Notice, String) {
        self.cleaning = false;
        self.measure(cx);
        cleanup_outcome(results)
    }

    /// Measures each kind off the UI thread (backups can be large) - again each time the page is shown.
    pub fn measure(&mut self, cx: &mut Context<Self>) {
        self.measuring += 1;
        let measuring = self.measuring;
        self.sizes = None;
        let cleaner = self.cleaner.clone();
        cx.spawn(async move |this, cx| {
            let sizes = cx.background_executor().spawn(async move { CHOICES.map(|k| cleaner.measure(k)) }).await;
            let _ = this.update(cx, |page, cx| {
                if page.measuring == measuring {
                    page.sizes = Some(sizes);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn action_row(
        &self,
        id: &'static str,
        title: &'static str,
        text: Explain,
        button: (&'static str, &'static str, ButtonKind, bool),
        event: TroubleshootEvent,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = Theme::get(cx).clone();
        let (label, tip, kind, enabled) = button;
        div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .child(div().flex().flex_col().gap(px(2.0)).flex_1().min_w_0().map(|d| {
                let title = div().font_weight(FontWeight::SEMIBOLD).child(title);
                match text {
                    Explain::Tip(tip) => d.child(ui::with_tip(title, SharedString::from(format!("{id}-tip")), tip, cx)),
                    Explain::Warning(text) => d.child(title).child(hint(text, &theme)),
                }
            }))
            .child(
                ui::button(id, label, kind, enabled, cx)
                    .min_w(px(120.0))
                    .tip(tip)
                    .when(enabled, |b| b.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(event.clone())))),
            )
    }
}

impl Render for TroubleshootPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let idle = self.busy.is_none();
        let row = |d: gpui::Div| d.pad(px(16.0), px(0.0)).border_b_1().border_color(c.grid_line);

        // The check boxes carry 4 px inside above and below: 2 more between them and their notes.
        let mut boxes = div().flex().flex_col().gap(px(2.0));
        for (i, kind) in CHOICES.into_iter().enumerate() {
            let text = match self.sizes {
                Some(sizes) => {
                    format!("{} - {} ({})", choice_name(kind), choice_detail(kind), byte_size(sizes[i] as f64))
                }
                None => format!("{} (measuring…)", choice_name(kind)),
            };
            let on = self.chosen[i];
            boxes = boxes.child(dialog::check(SharedString::from(format!("clean-{i}")), on, text, cx).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.chosen[i] = !on;
                    cx.notify();
                }),
            ));
            let note = match kind {
                AppDataKind::ProjectBackups => Some("They can't be brought back."),
                AppDataKind::Deployments => Some("They can hold a live connection string - delete them once deployed."),
                AppDataKind::OldSettingsFiles => Some("They can hold the sa password in plain text."),
                _ => None,
            };
            if let Some(note) = note {
                boxes = boxes.child(div().pad(px(0.0), px(22.0)).child(hint(note, &theme)));
            }
        }
        let kinds: Vec<AppDataKind> =
            CHOICES.into_iter().zip(self.chosen).filter(|(_, on)| *on).map(|(k, _)| k).collect();
        let can_clean = idle && !self.cleaning && !kinds.is_empty();
        let question = cleanup_question(&self.cleaner.root, &kinds);

        div()
            .id("troubleshoot")
            .key_context("Modal")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme.font.clone())
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pad(px(14.0), px(20.0))
                    .border_b_1()
                    .border_color(c.card_border)
                    // 8 more than the bar's 20 puts the title 28 from the edge, in line with the page.
                    .child(div().pad(px(0.0), px(8.0)).child(ui::with_tip(
                        div().text_size(theme.text.large).font_weight(FontWeight::SEMIBOLD).child("Troubleshoot"),
                        "troubleshoot-tip",
                        "Restart, clean up or reset DNN Manager. Your projects are never touched here.",
                        cx,
                    )))
                    .child(
                        ui::icon_button("troubleshoot-close", icons::CLOSE, c.link_fg, true, cx)
                            .tip("Close (Esc)")
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(TroubleshootEvent::Close))),
                    ),
            )
            .child(
                div().id("troubleshoot-content").flex_1().min_h_0().overflow_y_scroll().flex().justify_center().child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_w(px(860.0))
                        .gap(px(16.0))
                        .pad(px(22.0), px(28.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(row(self.action_row(
                                            "restart",
                                            "Restart DNN Manager",
                                            Explain::Tip("Closes and starts it again. Your projects, settings and data are kept."),
                                            ("Restart", "Close DNN Manager and open it again - nothing is lost", ButtonKind::Secondary, idle),
                                            TroubleshootEvent::Restart,
                                            cx,
                                        ))
                                        .border_t_1())
                                        .child(row(div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(10.0))
                                            .child(self.action_row(
                                                "clean-up",
                                                "Clean up data",
                                                Explain::Warning(r"The data selected below is removed from Documents\DnnManager."),
                                                ("Clean up data", "Delete what is ticked below, to free disk space", ButtonKind::Secondary, can_clean),
                                                TroubleshootEvent::CleanUp { question, kinds },
                                                cx,
                                            ))
                                            .child(boxes)))
                                        .child(row(self.action_row(
                                            "reset-settings",
                                            "Reset settings to defaults",
                                            Explain::Warning("Every setting back to its default, passwords and start at sign-in too. Projects, logs and keep warm stay."),
                                            (
                                                "Reset settings",
                                                "Put every setting back as it was at install - your websites aren't touched",
                                                ButtonKind::Secondary,
                                                idle,
                                            ),
                                            TroubleshootEvent::ResetSettings,
                                            cx,
                                        )))
                                        .child(row(self.action_row(
                                            "factory-reset",
                                            "Reset to factory defaults",
                                            Explain::Warning("Removes settings, passwords, logs, kept packages and keep warm, then restarts. Projects and backups stay."),
                                            (
                                                "Reset to factory defaults",
                                                "Put DNN Manager back as it was installed - your websites and backups are kept; asks first",
                                                ButtonKind::Danger,
                                                idle,
                                            ),
                                            TroubleshootEvent::FactoryReset,
                                            cx,
                                        ))),
                                )
                                .when_some(self.busy.clone(), |d, op| {
                                    d.child(hint(format!("'{op}' is running - these wait until it has finished."), &theme))
                                }),
                        ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn database(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE settings (key TEXT NOT NULL PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE state (area TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL);
             CREATE TABLE keep_warm (site TEXT NOT NULL PRIMARY KEY);
             CREATE TABLE dnn_releases (api TEXT, position INTEGER, version TEXT, tag TEXT, url TEXT);
             INSERT INTO keep_warm VALUES ('abcd');
             INSERT INTO settings VALUES ('projects.sitePort', '8080');
             INSERT INTO state VALUES ('window', 'x', 'y');",
        )
        .unwrap();
    }

    fn setup() -> (tempfile::TempDir, AppDataCleaner) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("DnnManager");
        let db = root.join("dnnmanager.db");
        std::fs::create_dir_all(&root).unwrap();
        database(&db);
        write(&root.join("logs/2026-11-16.log"), 100);
        write(&root.join("packages/dnnsoftware.Dnn.Platform/DNN_10.zip"), 1000);
        write(&root.join("backups/site1/site1_20200101_120000/site1.zip"), 50);
        write(&root.join("deployments/site1_20200101_120000/site1.bacpac"), 70);
        write(&root.join("settings.json"), 10);
        write(&root.join("dnnmanager.damaged-1.db"), 5);
        write(&root.join("state/forms.json"), 3);
        write(&root.join("keep.txt"), 1);
        (dir, AppDataCleaner::new(root, db))
    }

    #[test]
    fn measures_files_and_table_rows() {
        let (_dir, cleaner) = setup();
        assert_eq!(cleaner.measure(AppDataKind::Logs), 100);
        assert_eq!(cleaner.measure(AppDataKind::DnnPackages), 1000);
        assert_eq!(cleaner.measure(AppDataKind::ProjectBackups), 50);
        assert_eq!(cleaner.measure(AppDataKind::Deployments), 70);
        assert_eq!(cleaner.measure(AppDataKind::OldSettingsFiles), 18);
        assert_eq!(cleaner.measure(AppDataKind::KeepWarmChoices), 4);
    }

    #[test]
    fn cleans_only_its_own_kind_and_the_folders_left_empty() {
        let (_dir, cleaner) = setup();
        let root = cleaner.root.clone();
        assert_eq!(cleaner.clean(AppDataKind::ProjectBackups), CleanupResult { freed: 50, skipped: 0 });
        assert!(!root.join("backups/site1").exists());
        assert!(root.join("backups").is_dir(), "the backups folder itself stays");
        assert_eq!(cleaner.clean(AppDataKind::OldSettingsFiles).freed, 18);
        assert!(root.join("keep.txt").is_file() && root.join("dnnmanager.db").is_file());
        assert_eq!(cleaner.clean(AppDataKind::KeepWarmChoices), CleanupResult { freed: 4, skipped: 0 });
        assert_eq!(cleaner.measure(AppDataKind::KeepWarmChoices), 0);
        assert!(root.join("logs/2026-11-16.log").is_file());
        assert_eq!(cleaner.clean(AppDataKind::DnnPackages).freed, 1000);
        assert!(!root.join("packages").exists());
    }

    #[test]
    fn deletes_backups_older_than_kept_by_the_date_in_their_name() {
        let (_dir, cleaner) = setup();
        let root = cleaner.root.clone();
        write(&root.join("backups/site2/site2_29991231_000000/x.zip"), 9);
        let result = cleaner.delete_expired(30, SystemTime::now());
        assert_eq!(result.freed, 120);
        assert_eq!(result.deleted.len(), 2);
        assert!(root.join("backups/site2/site2_29991231_000000/x.zip").is_file());
        assert!(!root.join("backups/site1").exists());
        assert_eq!(cleaner.delete_expired(0, SystemTime::now()), ExpiredResult::default());
    }

    #[test]
    fn reads_the_date_in_a_folder_name() {
        let at = made_at(Path::new(r"C:\nowhere\site_19700102_000001"));
        assert_eq!(at, UNIX_EPOCH + Duration::from_secs(86_401));
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    // The sa password is encrypted with DPAPI, which only Windows has.
    #[cfg(windows)]
    #[test]
    fn a_settings_reset_keeps_only_the_version_and_the_sa_password() {
        let (_dir, cleaner) = setup();
        let c = Connection::open(&cleaner.database).unwrap();
        let kept = secrets::protect("Old-Sa-1").unwrap();
        c.execute("INSERT INTO settings VALUES (?1, ?2)", [SA_PASSWORD_KEY, &kept]).unwrap();
        reset_settings(&cleaner.database, "New-Sa-1").unwrap();
        let rows: BTreeMap<String, String> = c
            .prepare("SELECT key, value FROM settings")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows["version"], "3");
        assert_eq!(secrets::unprotect(&rows[SA_PASSWORD_KEY]).unwrap(), "Old-Sa-1");

        // One this account can't decrypt goes - the new one is kept, encrypted.
        c.execute("UPDATE settings SET value = 'dpapi:AAAA' WHERE key = ?1", [SA_PASSWORD_KEY]).unwrap();
        reset_settings(&cleaner.database, "New-Sa-1").unwrap();
        let sa: String =
            c.query_row("SELECT value FROM settings WHERE key = ?1", [SA_PASSWORD_KEY], |r| r.get(0)).unwrap();
        assert_eq!(secrets::unprotect(&sa).unwrap(), "New-Sa-1");
    }

    // Off Windows nothing can encrypt the sa password: the reset keeps only the version.
    #[cfg(not(windows))]
    #[test]
    fn off_windows_a_settings_reset_keeps_only_the_version() {
        let (_dir, cleaner) = setup();
        reset_settings(&cleaner.database, "New-Sa-1").unwrap();
        let c = Connection::open(&cleaner.database).unwrap();
        let keys: Vec<String> =
            c.prepare("SELECT key FROM settings").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
        assert_eq!(keys, vec!["version".to_string()]);
    }

    #[derive(Default)]
    struct Secrets(Mutex<Vec<String>>);
    impl SecretStore for Secrets {
        fn read(&self, _: &str) -> Option<String> {
            None
        }
        fn write(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(name.into());
            if name == names::DATABASE_SERVER_PASSWORD { Err("refused".into()) } else { Ok(()) }
        }
    }
    struct SignIn(bool);
    impl SignInTask for SignIn {
        fn target(&self) -> Option<PathBuf> {
            self.0.then(|| PathBuf::from("x.exe"))
        }
        fn refusal(&self, _: &Path) -> Option<String> {
            None
        }
        fn enable(&self, _: &Path) -> Result<(), String> {
            Ok(())
        }
        fn disable(&self) -> Result<(), String> {
            Err("Access is denied.".into())
        }
    }

    #[test]
    fn a_factory_reset_removes_what_it_says_and_lists_what_it_couldnt() {
        let (_dir, cleaner) = setup();
        let secrets = Secrets::default();
        let problems = factory_reset(&cleaner, "New-Sa-1", &secrets, &SignIn(true)).unwrap();
        assert_eq!(problems, vec!["refused".to_string(), "Starting at sign-in: Access is denied.".to_string()]);
        assert_eq!(secrets.0.lock().unwrap().len(), 2);
        assert!(!cleaner.root.join("logs/2026-11-16.log").exists());
        assert!(cleaner.root.join("backups/site1/site1_20200101_120000/site1.zip").is_file(), "backups stay");
        let c = Connection::open(&cleaner.database).unwrap();
        let state: i64 = c.query_row("SELECT COUNT(*) FROM state", [], |r| r.get(0)).unwrap();
        assert_eq!(state, 0);
        assert!(forget_secrets_and_start(&Secrets::default(), &SignIn(false)).len() == 1);
        assert_eq!(
            problems_text("DNN Manager is reset, except:", &problems),
            "DNN Manager is reset, except:\n\n• refused\n• Starting at sign-in: Access is denied."
        );
    }

    #[test]
    fn texts_of_clean_up() {
        assert_eq!(
            cleanup_outcome(&[CleanupResult { freed: 2048, skipped: 0 }]),
            (Notice::Success, "Cleaned up 2 KB.".to_string())
        );
        assert_eq!(
            cleanup_outcome(&[CleanupResult { freed: 0, skipped: 2 }]).1,
            "Cleaned up 0 B - 2 file(s) are in use and were left."
        );
        let q = cleanup_question(Path::new(r"C:\D"), &[AppDataKind::Logs, AppDataKind::Deployments]);
        assert_eq!(
            q,
            "Delete this data from C:\\D?\n\n• Logs\n• Deployment packages\n\nBackups and deployment packages can't be brought back."
        );
        assert_eq!(reset_settings_outcome(&[]).1, "The settings are back to their defaults.");
    }
}
