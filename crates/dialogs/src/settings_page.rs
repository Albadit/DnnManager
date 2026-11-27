//! Settings (`Pages/SettingsPage.xaml(.cs)`, `SettingsPage.Keyboard.cs`), opened over the page: a search box and the
//! categories on the left, the chosen one on the right, and Discard changes / Close / Save at the bottom. Every edit
//! makes Save ready; Save checks the values (`UserSettings.Validate`, [`crate::setting_rules`]) and hands the new
//! settings to the app ([`SettingsEvent::Save`]), which saves them, puts them to work and answers with
//! [`SettingsPage::saved`]. The keyboard shortcuts and the IIS features are saved at once
//! ([`SettingsEvent::SaveAtOnce`]); the Test and Set up cards work with the saved settings, off the UI thread.
//!
//! Secrets: the default host password and the SQL Server login's password are read from and written to the Windows
//! Credential Manager ([`SecretStore`]) - written only once the app said the settings were saved; the container's sa
//! password is the settings row `sqlServer.saPassword`, decrypted to show as dots and encrypted (AES-256-GCM) again before
//! it leaves the page. None is ever shown, logged or put in an event but that encrypted row.
//!
//! Deviations: Repositories is a field per URL (with an empty one at the end to add one) - the port has no multi-line
//! text box; Browse… opens Windows' folder picker without starting in the typed folder; About's Docker line waits for
//! Docker's own time limit (30 s) rather than 10 s.

use dnnm_ui::px;
use dnnm_ui::{Icon, IconSize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, PathPromptOptions, SharedString, Stateful, Subscription, Window, div, prelude::*};

use dnnm_application::settings::Settings;
use dnnm_application::source_control::model::{Auth, Environment};
use dnnm_sql::{DatabaseCheckOptions, DatabaseCheckReport, DatabaseConnection, DockerOptions, DockerStatus};
use dnnm_system::prereq::IisFeature;
use dnnm_system::secrets::{self, names};
use dnnm_ui::dialog;
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, ButtonKind, Pad, Tip, icons};

use crate::about::{self, Health, MachineFacts, UpdateStatus};
use crate::host_password::{PasswordField, PasswordFieldChanged};
use crate::setting_rules::{self as rules, Category};
use crate::settings_cards::{DockerCardState, IisCardState, SqlCardState};
use crate::settings_form::{CONTAINER, Field, Form, LOCAL_DB_FILE, SQL_SERVER, Secret, uses_login};
use crate::settings_keyboard::{self as keyboard, Recorded, ShortcutCommand, ShortcutRow, Slot};

// ─── What the page needs from the app ────────────────────────────────────────

/// The Windows Credential Manager (`ISecretStore`) - `dnnm_system::secrets::CredentialStore`, a stand-in in tests.
pub trait SecretStore: Send + Sync {
    fn read(&self, name: &str) -> Option<String>;
    fn write(&self, name: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

impl SecretStore for secrets::CredentialStore {
    fn read(&self, name: &str) -> Option<String> {
        secrets::CredentialStore::read(self, name)
    }

    fn write(&self, name: &str, secret: &str) -> Result<(), String> {
        secrets::CredentialStore::write(self, name, secret)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        secrets::CredentialStore::delete(self, name)
    }
}

/// Starting DNN Manager at sign-in - the scheduled task (`StartupTask`).
pub trait SignInTask: Send + Sync {
    /// The program the task starts; None when there is no task.
    fn target(&self) -> Option<PathBuf>;
    /// Why the task can't start `exe` (installed for this user only); None when it can.
    fn refusal(&self, exe: &Path) -> Option<String>;
    fn enable(&self, exe: &Path) -> Result<(), String>;
    fn disable(&self) -> Result<(), String>;
}

/// The scheduled task of `dnnm_system::startup`.
pub struct SystemSignIn;

impl SignInTask for SystemSignIn {
    fn target(&self) -> Option<PathBuf> {
        dnnm_system::startup::get_target()
    }

    fn refusal(&self, exe: &Path) -> Option<String> {
        dnnm_system::startup::refusal(exe)
    }

    fn enable(&self, exe: &Path) -> Result<(), String> {
        dnnm_system::startup::enable(exe)
    }

    fn disable(&self) -> Result<(), String> {
        dnnm_system::startup::disable()
    }
}

type Work<A, R> = Arc<dyn Fn(A) -> R + Send + Sync>;

/// What the page asks of the system - each run off the UI thread, but the secrets (as the WPF app reads them).
#[derive(Clone)]
pub struct SettingsServices {
    pub secrets: Arc<dyn SecretStore>,
    pub sign_in: Arc<dyn SignInTask>,
    /// Database server → Test: the checks Test connection runs on the server alone.
    pub test_database: Work<DatabaseConnection, Result<DatabaseCheckReport, String>>,
    /// Docker Desktop, its engine and the container of that name.
    pub docker_status: Work<String, Result<DockerStatus, String>>,
    pub start_docker: Work<(), Result<(), String>>,
    /// docker-compose.yml as Set up docker-compose writes it (without the password).
    pub render_compose: Work<DockerOptions, String>,
    /// Whether each IIS feature is enabled, by name.
    pub iis_feature_states: Work<Vec<IisFeature>, BTreeMap<String, bool>>,
    /// The terminal's shells on this PC: (key, name).
    pub shells: Work<(), Vec<(String, String)>>,
    /// Windows, IIS and the .NET Framework, for About.
    pub machine: Work<(), MachineFacts>,
    pub open_folder: Work<PathBuf, Result<(), String>>,
    /// `Documents\DnnManager`.
    pub data_folder: PathBuf,
}

impl SettingsServices {
    /// The real ones: the Credential Manager, the scheduled task, SQL Server, Docker, PowerShell, the registry.
    pub fn system() -> Self {
        Self {
            secrets: Arc::new(secrets::CredentialStore),
            sign_in: Arc::new(SystemSignIn),
            test_database: Arc::new(|connection: DatabaseConnection| {
                let options = DatabaseCheckOptions { server_only: true, ..DatabaseCheckOptions::new(true) };
                dnnm_sql::provisioner::DatabaseProvisioner.check(&connection, &options, None)
            }),
            docker_status: Arc::new(|name: String| dnnm_sql::docker::status(&name, None)),
            start_docker: Arc::new(|()| dnnm_system::prereq::start_docker_desktop()),
            render_compose: Arc::new(|options: DockerOptions| dnnm_sql::docker::render(&options)),
            iis_feature_states: Arc::new(|features: Vec<IisFeature>| {
                dnnm_system::prereq::get_iis_feature_states(&features)
            }),
            shells: Arc::new(|()| {
                dnnm_terminal::find_shells().user.iter().map(|s| (s.key.to_string(), s.name.to_string())).collect()
            }),
            machine: Arc::new(|()| about::read_machine()),
            open_folder: Arc::new(|folder: PathBuf| dnnm_infrastructure::shell::open_folder(&folder)),
            data_folder: dnnm_infrastructure::paths::app_data_directory(),
        }
    }
}

/// What the app passes in.
pub struct SettingsInputs {
    /// The settings as saved now.
    pub saved: Settings,
    /// Every command with a shortcut, in the order the app matches them (the first listed runs).
    pub commands: Vec<ShortcutCommand>,
    pub services: SettingsServices,
    /// Help offers What's new - not to a user still on the version they started with.
    pub offer_whats_new: bool,
    /// About's Update line, as the app's updater has it.
    pub update: UpdateStatus,
}

/// A toast's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Success,
    Warning,
    Error,
}

/// What the page asks the app to do.
#[derive(Debug, Clone)]
pub enum SettingsEvent {
    /// Save: write the rows that differ from the saved settings (the sa password is encrypted already), put them to
    /// work, then call [`SettingsPage::saved`]. While an operation runs, refuse with
    /// `saved(Err("'<operation>' is still running - save once it has finished."))`.
    Save(Settings),
    /// Saved at once - a keyboard shortcut or the IIS features changed; answer with [`SettingsPage::saved`] too.
    SaveAtOnce(Settings),
    /// ✕ or Close: back to the page under Settings (ask first when [`SettingsPage::has_unsaved_changes`]).
    Close,
    ShowGuide,
    /// Close Settings and point at each part of the window.
    StartTour,
    ShowWhatsNew,
    OpenUserGuide,
    /// "Edit upgrade path…": show DnnUpgradePathDialog; once saved, call [`SettingsPage::saved_elsewhere`].
    EditUpgradePath,
    /// Set up docker-compose: run SetupSqlContainerUseCase with the saved settings, then call
    /// [`SettingsPage::container_set_up`] (with the settings when it adopted the volume's sa password).
    SetUpContainer,
    /// Set up IIS: run the IIS features check (`ensure_iis_features`) as an operation, then call
    /// [`SettingsPage::iis_set_up`].
    SetUpIis,
    /// Install Docker Desktop: ask "Install Docker Desktop with winget? It downloads about 600 MB, and Windows may ask
    /// to enable WSL 2 or to restart afterwards." (Install Docker Desktop / Cancel), run it as the operation "Install
    /// Docker Desktop", then call [`SettingsPage::docker_changed`].
    InstallDocker,
    /// IIS → Edit…: show [`crate::iis_features::IisFeaturesDialog`] with these; on Done call
    /// [`SettingsPage::save_iis_features`].
    EditIisFeatures(Vec<IisFeature>),
    /// Reset all to defaults: ask "Put every keyboard shortcut back to its default?" (Reset all / Cancel); on yes call
    /// [`SettingsPage::reset_all_shortcuts`].
    ConfirmResetAllShortcuts,
    /// About was opened: ask GitHub for a newer DNN Manager (unless asked within 10 minutes), then
    /// [`SettingsPage::set_update_status`].
    CheckForUpdates,
    /// Servers → Add server…: the server dialog for a new one; once saved, [`SettingsPage::set_servers`].
    AddServer,
    /// Servers → Edit…: the server dialog for this one; once saved, [`SettingsPage::set_servers`].
    EditServer(Environment),
    /// Servers → Delete: ask first; once deleted, [`SettingsPage::set_servers`].
    DeleteServer(Environment),
    /// Servers → Import…: ask for a FileZilla Site Manager file, add its sites, then [`SettingsPage::set_servers`].
    ImportServers,
    /// Servers → Export…: ask whether the passwords go too, then for the file, and write it.
    ExportServers,
    Toast(Notice, String),
    /// An error dialog (`Dialogs.Error`).
    Error(String),
}

/// What a save the app was asked for was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Form,
    Shortcuts,
    IisFeatures(usize),
}

// ─── The page ─────────────────────────────────────────────────────────────

pub struct SettingsPage {
    pub(crate) saved: Settings,
    /// The saved sa password, decrypted - what the field is compared with.
    pub(crate) saved_sa: String,
    saved_host_password: String,
    saved_server_password: String,
    pub(crate) form: Form,
    saved_snapshot: String,
    pub(crate) dirty: bool,
    pub(crate) services: SettingsServices,
    category: Category,
    search: Entity<TextInput>,
    inputs: BTreeMap<Field, Entity<TextInput>>,
    passwords: BTreeMap<Secret, Entity<PasswordField>>,
    release_inputs: Vec<Entity<TextInput>>,
    release_subscriptions: Vec<Subscription>,
    open_select: Option<&'static str>,
    error: Option<String>,
    env_warning: Option<String>,
    // Start at sign-in: whether the task is there (None until read), the box, and what is said under it.
    starts_at_sign_in: Option<bool>,
    start_at_sign_in: bool,
    sign_in_enabled: bool,
    sign_in_hint: String,
    sign_in_tip: Option<String>,
    shells: Option<Vec<(String, String)>>,
    fonts: Vec<String>,
    // Keyboard shortcuts.
    commands: Vec<ShortcutCommand>,
    shortcut_search: Entity<TextInput>,
    /// The command whose shortcut is being recorded, and which of its shortcuts (or a new one).
    recording: Option<(String, Slot)>,
    interceptor: Option<Subscription>,
    shortcut_status: Option<String>,
    // Help and About.
    offer_whats_new: bool,
    update: UpdateStatus,
    machine: Option<MachineFacts>,
    docker_about: Option<String>,
    about_asked: bool,
    /// Source Control's servers, as the app read them ([`SettingsPage::set_servers`]) - or why they couldn't be.
    servers: Result<Vec<Environment>, String>,
    // The cards.
    pub(crate) sql_card: SqlCardState,
    pub(crate) docker_card: DockerCardState,
    pub(crate) iis_card: IisCardState,
    pending: Option<(Pending, Settings)>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsPage {}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Runs `work` on the background executor and `done` with its result on the page.
pub(crate) fn off_thread<T: Send + 'static>(
    cx: &mut Context<SettingsPage>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut SettingsPage, T, &mut Context<SettingsPage>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let result = cx.background_executor().spawn(async move { work() }).await;
        let _ = this.update(cx, |page, cx| {
            done(page, result, cx);
            cx.notify();
        });
    })
    .detach();
}

impl SettingsPage {
    pub fn new(inputs: SettingsInputs, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let search = cx.new(|cx| TextInput::search("Search settings", cx));
        subscriptions.push(cx.subscribe(&search, |this: &mut Self, _, _: &TextChanged, cx| this.search_changed(cx)));
        let shortcut_search = cx.new(|cx| TextInput::search("Search by action, area or key", cx));
        subscriptions.push(cx.subscribe(&shortcut_search, |_: &mut Self, _, _: &TextChanged, cx| cx.notify()));

        let mut text_inputs = BTreeMap::new();
        for field in Field::ALL {
            let input = cx.new(|cx| TextInput::new("", cx));
            subscriptions.push(cx.subscribe(&input, move |this: &mut Self, _, e: &TextChanged, cx| {
                if this.form.text(field) != e.0.as_ref() {
                    this.form.set_text(field, e.0.to_string());
                    this.edited(cx);
                }
            }));
            text_inputs.insert(field, input);
        }
        let mut passwords = BTreeMap::new();
        for secret in [Secret::Host, Secret::Sa, Secret::Server] {
            let input = cx.new(PasswordField::new);
            subscriptions.push(cx.subscribe(&input, move |this: &mut Self, input, _: &PasswordFieldChanged, cx| {
                let password = input.read(cx).password(cx);
                if this.form.password(secret) != password {
                    this.form.set_password(secret, password);
                    this.edited(cx);
                }
            }));
            passwords.insert(secret, input);
        }

        let fonts = dnnm_terminal::shells::installed_fonts(&cx.text_system().all_font_names())
            .into_iter()
            .map(String::from)
            .collect();
        // The GPUI build's own switches (the fixture, no elevation) aren't settings - not listed as overriding them.
        let env_warning = rules::environment_overrides(
            std::env::vars_os()
                .filter_map(|(k, _)| k.into_string().ok())
                .filter(|k| !k.to_ascii_uppercase().starts_with("DNNMANAGER_GPUI_")),
        );
        let focus = cx.focus_handle();
        window.focus(&focus);

        let mut page = Self {
            saved: inputs.saved,
            saved_sa: String::new(),
            saved_host_password: String::new(),
            saved_server_password: String::new(),
            form: Form::default(),
            saved_snapshot: String::new(),
            dirty: false,
            services: inputs.services,
            category: Category::General,
            search,
            inputs: text_inputs,
            passwords,
            release_inputs: Vec::new(),
            release_subscriptions: Vec::new(),
            open_select: None,
            error: None,
            env_warning,
            starts_at_sign_in: None,
            start_at_sign_in: false,
            sign_in_enabled: false,
            sign_in_hint: SIGN_IN_TASK.into(),
            sign_in_tip: None,
            shells: None,
            fonts,
            commands: inputs.commands,
            shortcut_search,
            recording: None,
            interceptor: None,
            shortcut_status: None,
            offer_whats_new: inputs.offer_whats_new,
            update: inputs.update,
            machine: None,
            docker_about: None,
            about_asked: false,
            servers: Ok(Vec::new()),
            sql_card: SqlCardState::default(),
            docker_card: DockerCardState::default(),
            iis_card: IisCardState::default(),
            pending: None,
            focus,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page.read_start_at_sign_in(cx);
        page.read_shells(cx);
        page
    }

    // ─── Public: what the app calls ───────────────────────────────────────

    /// Servers' list, read again by the app after it was opened or changed.
    pub fn set_servers(&mut self, servers: Result<Vec<Environment>, String>, cx: &mut Context<Self>) {
        self.servers = servers;
        cx.notify();
    }

    /// The form has edits that aren't saved - leaving the page would lose them.
    pub fn has_unsaved_changes(&self) -> bool {
        self.dirty
    }

    /// A password on the form was changed and not saved.
    pub fn has_unsaved_password(&self) -> bool {
        self.dirty
            && (self.form.host_password != self.saved_host_password
                || self.form.server_password != self.saved_server_password
                || self.form.sa_password != self.saved_sa)
    }

    /// The category shown.
    pub fn category(&self) -> Category {
        self.category
    }

    /// Shows `category` - the search goes when it leaves the category out.
    pub fn show_category(&mut self, category: Category, cx: &mut Context<Self>) {
        if !self.shown_categories(cx).contains(&category) {
            self.search.update(cx, |s, cx| s.set_text("", cx));
        }
        self.select_category(category, cx);
    }

    /// The next category shown (1) or the one before (-1), round the end - Ctrl+PageDown / Ctrl+PageUp.
    pub fn step_category(&mut self, by: i32, cx: &mut Context<Self>) {
        let shown = self.shown_categories(cx);
        if shown.is_empty() {
            return;
        }
        let index = shown.iter().position(|c| *c == self.category).unwrap_or(0) as i32;
        let n = shown.len() as i32;
        self.select_category(shown[(((index + by) % n) + n) as usize % n as usize], cx);
    }

    /// The keyboard in the search box (Ctrl+F).
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus_handle(cx));
    }

    /// The app's answer to [`SettingsEvent::Save`] or [`SettingsEvent::SaveAtOnce`]: Ok once saved, or why not.
    pub fn saved(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        let Some((kind, settings)) = self.pending.take() else { return };
        match (kind, result) {
            (Pending::Form, Ok(())) => {
                self.saved = settings;
                self.saved_sa = self.form.sa_password.clone();
                self.save_secrets(cx);
                // What another connection type's fields still show wasn't saved - show what is.
                let (server_password, sa) = (self.saved_server_password.clone(), self.saved_sa.clone());
                self.form.show_server(&self.saved, &server_password, &sa);
                self.sync_inputs(cx);
                self.saved_snapshot = self.form.snapshot();
                self.error = None;
                self.sql_card.settings_changed();
                self.edited(cx);
                self.apply_start_at_sign_in(cx);
            }
            (Pending::Form, Err(e)) => self.error = Some(e),
            (Pending::Shortcuts, Ok(())) => {
                self.saved = settings;
                self.shortcut_status = None;
            }
            (Pending::Shortcuts, Err(e)) => self.shortcut_status = Some(format!("Not saved - {e}")),
            (Pending::IisFeatures(count), Ok(())) => {
                self.saved = settings;
                self.iis_card.features_changed();
                cx.emit(SettingsEvent::Toast(Notice::Success, format!("{count} Windows features saved.")));
            }
            (Pending::IisFeatures(_), Err(e)) => {
                cx.emit(SettingsEvent::Error(format!("The Windows features could not be saved: {e}")));
            }
        }
        cx.notify();
    }

    /// The saved settings changed elsewhere (the upgrade path, the sa password Set up docker-compose adopted): taken
    /// as the saved ones; the form follows when it has no edits - otherwise only its sa password does, as a Save
    /// would put the old one back.
    pub fn saved_elsewhere(&mut self, saved: Settings, cx: &mut Context<Self>) {
        let clean = !self.dirty;
        self.saved = saved;
        self.sql_card.settings_changed();
        if clean {
            self.load(cx);
        } else {
            self.saved_sa = decrypt(&self.saved).unwrap_or_default();
            let sa = self.saved_sa.clone();
            self.form.sa_password = sa.clone();
            self.passwords[&Secret::Sa].update(cx, |p, cx| p.set_password(&sa, cx));
            self.edited(cx);
        }
        cx.notify();
    }

    /// Set up docker-compose has run: test Docker and the database server again - with the settings it saved, if any.
    pub fn container_set_up(&mut self, saved: Option<Settings>, cx: &mut Context<Self>) {
        if let Some(saved) = saved {
            self.saved_elsewhere(saved, cx);
        }
        self.test_docker(cx);
        self.test_sql(cx);
    }

    /// Docker Desktop was installed (or not): test again.
    pub fn docker_changed(&mut self, cx: &mut Context<Self>) {
        self.test_docker(cx);
    }

    /// Set up IIS has run: test again.
    pub fn iis_set_up(&mut self, cx: &mut Context<Self>) {
        self.test_iis(cx);
    }

    /// The IIS features dialog's result - saved at once.
    pub fn save_iis_features(&mut self, features: Vec<IisFeature>, cx: &mut Context<Self>) {
        let mut settings = self.saved.clone();
        rules::set_iis_features(&mut settings, &features);
        self.pending = Some((Pending::IisFeatures(features.len()), settings.clone()));
        cx.emit(SettingsEvent::SaveAtOnce(settings));
    }

    /// Reset all to defaults, once the app asked.
    pub fn reset_all_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.save_shortcuts(BTreeMap::new(), cx);
    }

    /// About's Update line.
    pub fn set_update_status(&mut self, update: UpdateStatus, cx: &mut Context<Self>) {
        self.update = update;
        cx.notify();
    }

    // ─── Loading and editing ──────────────────────────────────────────────

    /// The form from the saved settings and the passwords that go with them (`Load`) - Discard changes too.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.saved_host_password = self.services.secrets.read(names::DEFAULT_HOST_PASSWORD).unwrap_or_default();
        self.saved_server_password = self.services.secrets.read(names::DATABASE_SERVER_PASSWORD).unwrap_or_default();
        self.error = None;
        self.saved_sa = match decrypt(&self.saved) {
            Ok(sa) => sa,
            Err(e) => {
                self.error = Some(e);
                String::new()
            }
        };
        self.form = Form::load(&self.saved, &self.saved_host_password, &self.saved_server_password, &self.saved_sa);
        self.fit_shell();
        self.sync_inputs(cx);
        self.saved_snapshot = self.form.snapshot();
        if let Some(starts) = self.starts_at_sign_in {
            self.start_at_sign_in = starts;
        }
        self.dirty = false;
        cx.notify();
    }

    /// Every field shows what the form holds.
    fn sync_inputs(&mut self, cx: &mut Context<Self>) {
        for (field, input) in &self.inputs {
            let text = self.form.text(*field).to_string();
            if input.read(cx).text() != text {
                input.update(cx, |i, cx| i.set_text(text, cx));
            }
        }
        for (secret, input) in &self.passwords {
            let password = self.form.password(*secret).to_string();
            input.update(cx, |p, cx| p.set_password(&password, cx));
        }
        self.rebuild_release_inputs(cx);
    }

    fn sync_input(&mut self, field: Field, cx: &mut Context<Self>) {
        let text = self.form.text(field).to_string();
        self.inputs[&field].update(cx, |i, cx| i.set_text(text, cx));
    }

    /// One field per repository, and an empty one at the end to add another.
    fn rebuild_release_inputs(&mut self, cx: &mut Context<Self>) {
        self.release_inputs.clear();
        self.release_subscriptions.clear();
        let sources = self.form.release_sources.clone();
        for source in sources.iter().chain(std::iter::once(&String::new())) {
            self.push_release_input(source, cx);
        }
    }

    fn push_release_input(&mut self, text: &str, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("https://api.github.com/repos/<owner>/<repository>/releases", cx));
        let text = text.to_string();
        input.update(cx, |i, cx| i.set_text(text, cx));
        self.release_subscriptions.push(cx.subscribe(&input, |this: &mut Self, _, _: &TextChanged, cx| {
            let typed: Vec<String> = this.release_inputs.iter().map(|i| i.read(cx).text().to_string()).collect();
            if typed.last().is_some_and(|t| !t.trim().is_empty()) {
                this.push_release_input("", cx);
            }
            let sources: Vec<String> = typed.into_iter().filter(|t| !t.trim().is_empty()).collect();
            if this.form.release_sources != sources {
                this.form.release_sources = sources;
                this.edited(cx);
            }
        }));
        self.release_inputs.push(input);
    }

    /// Something was touched: there is something to save when the form now differs from what is saved (`Edited`).
    fn edited(&mut self, cx: &mut Context<Self>) {
        let start_changed = self.starts_at_sign_in.is_some_and(|starts| starts != self.start_at_sign_in);
        self.dirty = start_changed || self.form.snapshot() != self.saved_snapshot;
        cx.notify();
    }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown_categories(cx);
        if !shown.is_empty() && !shown.contains(&self.category) {
            self.select_category(shown[0], cx);
        }
        cx.notify();
    }

    fn keywords(&self, category: Category) -> String {
        match category {
            Category::Iis => {
                let features: Vec<String> =
                    rules::iis_features(&self.saved).iter().map(|f| format!("{} {}", f.label, f.name)).collect();
                format!("{} {}", category.keywords(), features.join(" "))
            }
            Category::Keyboard => {
                let commands: Vec<String> = self.commands.iter().map(|c| format!("{} {}", c.title, c.area)).collect();
                format!("{} {}", category.keywords(), commands.join(" "))
            }
            Category::Servers => {
                let servers: Vec<String> =
                    self.servers.iter().flatten().map(|s| format!("{} {}", s.name, s.host)).collect();
                format!("{} {}", category.keywords(), servers.join(" "))
            }
            other => other.keywords().to_string(),
        }
    }

    fn shown_categories(&self, cx: &App) -> Vec<Category> {
        let search = self.search.read(cx).text().to_string();
        Category::ALL.into_iter().filter(|c| rules::matches_all_words(&search, &self.keywords(*c))).collect()
    }

    fn select_category(&mut self, category: Category, cx: &mut Context<Self>) {
        self.category = category;
        self.open_select = None;
        self.stop_recording();
        match category {
            Category::About => self.show_about(cx),
            Category::Docker if self.docker_card.version == 0 => self.test_docker(cx),
            _ => {}
        }
        cx.notify();
    }

    fn fit_shell(&mut self) {
        // A shell that isn't installed (any more): the first one that is, as the terminal itself does.
        if let Some(shells) = &self.shells
            && !shells.iter().any(|(key, _)| key.eq_ignore_ascii_case(&self.form.shell))
            && let Some((first, _)) = shells.first()
        {
            self.form.shell = first.clone();
        }
    }

    fn read_shells(&mut self, cx: &mut Context<Self>) {
        let shells = self.services.shells.clone();
        off_thread(
            cx,
            move || shells(()),
            |page, shells, cx| {
                page.shells = Some(shells);
                page.fit_shell();
                if !page.dirty {
                    page.saved_snapshot = page.form.snapshot();
                }
                page.edited(cx);
            },
        );
    }

    // ─── Start at sign-in ─────────────────────────────────────────────────

    /// Whether the scheduled task is there - and which program it starts, when that isn't this one.
    fn read_start_at_sign_in(&mut self, cx: &mut Context<Self>) {
        let sign_in = self.services.sign_in.clone();
        off_thread(
            cx,
            move || {
                let exe = std::env::current_exe().ok();
                let target = sign_in.target();
                let refusal = exe.as_deref().and_then(|e| sign_in.refusal(e));
                (exe, target, refusal)
            },
            |page, (exe, target, refusal), cx| {
                page.starts_at_sign_in = Some(target.is_some());
                page.start_at_sign_in = target.is_some();
                // Installed for this user only: offered only to be switched off.
                page.sign_in_enabled = refusal.is_none() || target.is_some();
                if let Some(refusal) = refusal {
                    page.sign_in_hint = "Needs DNN Manager installed for all users (its Setup).".into();
                    page.sign_in_tip = Some(refusal);
                } else if let (Some(target), Some(exe)) = (&target, &exe) {
                    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
                    if !target.as_os_str().is_empty() && !same(target, exe) {
                        page.sign_in_hint = format!(
                            "Set up to start {} - untick and tick it again to start this copy ({}) instead.",
                            target.display(),
                            exe.display()
                        );
                    }
                }
                page.edited(cx);
            },
        );
    }

    /// Makes or removes the scheduled task when the box was changed - part of Save.
    fn apply_start_at_sign_in(&mut self, cx: &mut Context<Self>) {
        let enable = self.start_at_sign_in;
        if self.starts_at_sign_in.is_none_or(|starts| starts == enable) {
            return;
        }
        self.sign_in_enabled = false;
        let sign_in = self.services.sign_in.clone();
        off_thread(
            cx,
            move || {
                if enable {
                    std::env::current_exe()
                        .map_err(|_| "the program's own path is unknown.".to_string())
                        .and_then(|exe| sign_in.enable(&exe))
                } else {
                    sign_in.disable()
                }
            },
            move |page, result, cx| {
                page.sign_in_enabled = true;
                match result {
                    Ok(()) => {
                        page.starts_at_sign_in = Some(enable);
                        if enable {
                            page.sign_in_hint = SIGN_IN_TASK.into();
                        }
                    }
                    Err(e) => {
                        page.start_at_sign_in = !enable;
                        let what = if enable { "set up" } else { "removed" };
                        cx.emit(SettingsEvent::Toast(
                            Notice::Error,
                            format!("The other settings are saved, but the scheduled task could not be {what}: {e}"),
                        ));
                    }
                }
                page.edited(cx);
            },
        );
    }

    // ─── Saving ───────────────────────────────────────────────────────────

    /// Checks the form and hands the settings to the app (`SaveAsync`).
    fn save(&mut self, cx: &mut Context<Self>) {
        // Start from the saved settings, so rows this page doesn't edit (IIS features, columns…) stay as they are.
        let mut settings = self.saved.clone();
        let mut problem = self.form.apply(&mut settings).err().map(|(e, c)| (e, Some(c)));
        if problem.is_none() {
            // The sa password as typed: unchanged it keeps its saved row, changed it is encrypted now.
            if let Some(typed) = settings.rows().get("sqlServer.saPassword").cloned()
                && !secrets::is_protected(&typed)
            {
                let row = if typed == self.saved_sa {
                    rules::value(&self.saved, "sqlServer.saPassword")
                } else {
                    match secrets::protect(&typed) {
                        Ok(protected) => protected,
                        Err(e) => {
                            self.show_problem(format!("Not saved - {e}"), None, cx);
                            return;
                        }
                    }
                };
                settings.set("sqlServer.saPassword", row);
            }
            problem = rules::first_problem(&settings);
        }
        if let Some((error, category)) = problem {
            self.show_problem(error, category, cx);
            return;
        }
        self.pending = Some((Pending::Form, settings.clone()));
        cx.emit(SettingsEvent::Save(settings));
    }

    /// Opens the category the problem is in, shows it and says it in a toast.
    fn show_problem(&mut self, error: String, category: Option<Category>, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.set_text("", cx));
        if let Some(category) = category {
            self.select_category(category, cx);
        }
        cx.emit(SettingsEvent::Toast(Notice::Warning, format!("Not saved: {error}")));
        self.error = Some(error);
        cx.notify();
    }

    /// The secrets that go with the saved settings, in the Credential Manager (`SaveSecrets`).
    fn save_secrets(&mut self, cx: &mut Context<Self>) {
        let store = self.services.secrets.clone();
        let host = self.form.host_password.clone();
        if host != self.saved_host_password {
            let stored = if host.is_empty() {
                store.delete(names::DEFAULT_HOST_PASSWORD)
            } else {
                store.write(names::DEFAULT_HOST_PASSWORD, &host)
            };
            if let Err(e) = stored {
                cx.emit(SettingsEvent::Toast(
                    Notice::Error,
                    format!("The other settings are saved, but the host password isn't: {e}"),
                ));
                return;
            }
            self.saved_host_password = host;
        }
        let server = self.form.server_password.clone();
        if uses_login(&self.saved) && server != self.saved_server_password {
            let stored = if server.is_empty() {
                store.delete(names::DATABASE_SERVER_PASSWORD)
            } else {
                store.write(names::DATABASE_SERVER_PASSWORD, &server)
            };
            if let Err(e) = stored {
                cx.emit(SettingsEvent::Toast(
                    Notice::Error,
                    format!("The other settings are saved, but the database server's password isn't: {e}"),
                ));
                return;
            }
            self.saved_server_password = server;
        }
    }

    // ─── Keyboard shortcuts ───────────────────────────────────────────────

    fn shortcut_rows(&self) -> Vec<ShortcutRow> {
        keyboard::rows(&self.commands, &rules::custom_shortcuts(&self.saved))
    }

    fn save_shortcuts(&mut self, custom: BTreeMap<String, String>, cx: &mut Context<Self>) {
        let mut settings = self.saved.clone();
        rules::set_custom_shortcuts(&mut settings, &custom);
        self.pending = Some((Pending::Shortcuts, settings.clone()));
        cx.emit(SettingsEvent::SaveAtOnce(settings));
    }

    /// Every key goes to the shortcut being recorded now - not to the window's shortcuts.
    fn start_recording(&mut self, id: String, slot: Slot, cx: &mut Context<Self>) {
        self.recording = Some((id, slot));
        self.shortcut_status = None;
        let page = cx.entity().downgrade();
        self.interceptor = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let Some(page) = page.upgrade() else { return };
            if page.update(cx, |p, cx| p.record(&event.keystroke, cx)) {
                cx.stop_propagation();
            }
        }));
        cx.notify();
    }

    fn stop_recording(&mut self) {
        self.recording = None;
        self.interceptor = None;
    }

    /// A key pressed while recording; true when it was taken.
    fn record(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let Some((id, slot)) = self.recording.clone() else { return false };
        let Some(command) = self.commands.iter().find(|c| c.id == id).cloned() else {
            self.stop_recording();
            return false;
        };
        let chosen = match keyboard::recorded(keystroke) {
            Recorded::Wait => return true,
            Recorded::Cancel => {
                self.stop_recording();
                cx.notify();
                return true;
            }
            Recorded::Remove => None,
            Recorded::Chosen(shortcut) => {
                if let Some(problem) = shortcut.problem() {
                    self.shortcut_status = Some(format!("{shortcut}: {problem}"));
                    cx.notify();
                    return true;
                }
                Some(shortcut)
            }
        };
        self.stop_recording();
        match keyboard::with_shortcut(&rules::custom_shortcuts(&self.saved), &command, slot, chosen.as_ref()) {
            Ok(next) => self.save_shortcuts(next, cx),
            Err(problem) => self.shortcut_status = Some(problem.into()),
        }
        cx.notify();
        true
    }

    fn reset_shortcut(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut custom = rules::custom_shortcuts(&self.saved);
        custom.remove(id);
        self.save_shortcuts(custom, cx);
    }

    // ─── About ────────────────────────────────────────────────────────────

    /// About - the first time, asks Windows and Docker (and the app asks GitHub).
    fn show_about(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::CheckForUpdates);
        if self.about_asked {
            return;
        }
        self.about_asked = true;
        let machine = self.services.machine.clone();
        off_thread(cx, move || machine(()), |page, facts, _| page.machine = Some(facts));
        let docker = self.services.docker_status.clone();
        let container = rules::value(&self.saved, "docker.containerName");
        off_thread(cx, move || about::docker_text(&docker(container)), |page, text, _| page.docker_about = Some(text));
    }

    fn data_folders(&self) -> Vec<(&'static str, PathBuf)> {
        let root = self.services.data_folder.clone();
        vec![
            ("Settings", root.clone()),
            ("Backups", root.join("backups")),
            ("Deployments", root.join("deployments")),
            ("Logs", root.join("logs")),
            ("DNN packages", root.join("packages")),
        ]
    }

    /// Opens one of the folders with your files - made first when nothing has needed it yet.
    fn open_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if let Err(e) = std::fs::create_dir_all(&folder) {
            cx.emit(SettingsEvent::Toast(Notice::Error, format!("{} couldn't be made: {e}", folder.display())));
            return;
        }
        let open = self.services.open_folder.clone();
        off_thread(
            cx,
            move || open(folder),
            |_, result, cx| {
                if let Err(e) = result {
                    cx.emit(SettingsEvent::Toast(Notice::Error, e));
                }
            },
        );
    }

    fn browse_projects_folder(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Projects folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = answer.await
                && let Some(folder) = paths.into_iter().next()
            {
                let _ = this.update(cx, |page, cx| {
                    page.form.base_directory = folder.display().to_string();
                    page.sync_input(Field::BaseDirectory, cx);
                    page.edited(cx);
                });
            }
        })
        .detach();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        // Ctrl+F, Ctrl+PageDown / PageUp - with ⌘ on macOS.
        if !k.modifiers.secondary() || k.modifiers.alt {
            return;
        }
        match k.key.as_str() {
            "f" => self.focus_search(window, cx),
            "pagedown" => self.step_category(1, cx),
            "pageup" => self.step_category(-1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// The saved sa password, decrypted - or what is wrong with it.
fn decrypt(settings: &Settings) -> Result<String, String> {
    secrets::unprotect(&rules::value(settings, "sqlServer.saPassword")).map_err(|_| {
        format!(
            "The settings have an SA password that can't be decrypted - it was encrypted by another user or on another computer, or its key is no longer in {}. Enter the password again below.",
            secrets::store_name()
        )
    })
}

// ─── Drawing ──────────────────────────────────────────────────────────────

/// Start at sign-in's usual explanation (a tip beside the box); any other text under it is a problem.
const SIGN_IN_TASK: &str = "As Administrator, without a prompt at every sign-in (a scheduled task).";

/// A category's title. No spacing of its own: each page's flex column puts the gap under it.
pub(crate) fn category_title(text: &'static str, theme: &Theme) -> Div {
    div().text_size(theme.text.title).font_weight(FontWeight::SEMIBOLD).child(text)
}

/// A group's label. No spacing of its own: 22 px above it is the page's gap, 8 px under it its group's gap.
pub(crate) fn group_label(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_size(theme.text.medium).text_color(theme.colors.text_muted).child(text.into())
}

/// A muted hint. No spacing of its own: the gap above it is its parent's.
pub(crate) fn hint(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_size(theme.text.hint).text_color(theme.colors.text_muted).child(text.into())
}

/// `child` indented 22 px - under a check box's text.
fn indented(child: impl IntoElement) -> Div {
    div().pad(px(0.0), px(22.0)).child(child)
}

/// A group: its label, then `body` `gap` under it.
fn group(text: impl Into<SharedString>, gap: f32, theme: &Theme) -> Div {
    div().flex().flex_col().gap(px(gap)).child(group_label(text, theme))
}

/// A group whose label has its explanation as a tip beside it.
fn group_tip(text: &'static str, id: &'static str, tip: impl Into<SharedString>, gap: f32, cx: &App) -> Div {
    let theme = Theme::get(cx);
    column(gap).child(ui::with_tip(group_label(text, theme), id, tip, cx))
}

/// A flex column with `gap` between its children.
fn column(gap: f32) -> Div {
    div().flex().flex_col().gap(px(gap))
}

fn radio_dot(chosen: bool, cx: &App) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    // 2 px above the dot lines it up with the first line of text beside it.
    div().flex_none().pad(px(2.0), px(0.0)).child(
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.0))
            .rounded_full()
            .border_1()
            .border_color(if chosen { c.accent } else { c.input_border })
            .bg(c.input_bg)
            .when(chosen, |d| d.child(div().size(px(8.0)).rounded_full().bg(c.accent))),
    )
}

fn health_dot(health: Health, cx: &App) -> Option<Div> {
    let c = Theme::get(cx).colors;
    let color = match health {
        Health::None => return None,
        Health::Ok => c.success_text,
        Health::Warning => c.log_warn,
        Health::Unknown => c.text_muted,
        Health::Bad => c.error_text,
    };
    Some(div().flex_none().size(px(8.0)).rounded_full().bg(color))
}

impl SettingsPage {
    fn text_box(&self, field: Field, window: &Window, cx: &App) -> Div {
        dialog::text_field(&self.inputs[&field], window, cx)
    }

    /// A password with its eye, which shows and hides it (the same box as the dialogs').
    fn password_box(&self, secret: Secret) -> Div {
        div().child(self.passwords[&secret].clone())
    }

    fn search_box(&self, input: &Entity<TextInput>, window: &Window, cx: &App) -> Div {
        let theme = Theme::get(cx);
        let c = theme.colors;
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(theme.metrics.search_height)
            .pad(px(0.0), px(6.0))
            .rounded(theme.metrics.control_radius)
            .border_1()
            .border_color(if focused { c.focus_border } else { c.field_border })
            .bg(c.input_bg)
            .child(ui::icon(Icon::Search, c.text_muted, cx))
            .child(div().flex_1().child(input.clone()))
    }

    /// A labelled field: the label above it.
    fn labelled(&self, label: &'static str, field: Field, window: &Window, cx: &App) -> Div {
        column(4.0).flex_1().child(dialog::label(label, cx)).child(self.text_box(field, window, cx))
    }

    fn labelled_password(&self, label: &'static str, secret: Secret, cx: &App) -> Div {
        column(4.0).flex_1().child(dialog::label(label, cx)).child(self.password_box(secret))
    }

    /// A choice from a list (`ComboBox`): the options open over the page under it.
    fn combo(
        &self,
        id: &'static str,
        width: Option<f32>,
        current: &str,
        options: Vec<(String, String)>,
        pick: fn(&mut SettingsPage, String, &mut Context<SettingsPage>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open_select == Some(id);
        let label = options.iter().find(|(_, v)| v == current).map_or_else(|| current.to_string(), |(l, _)| l.clone());
        let items = options.iter().map(|(text, value)| (SharedString::from(text.clone()), value == current)).collect();
        let values: Vec<String> = options.into_iter().map(|(_, value)| value).collect();
        let toggle = cx.listener(move |this, open: &bool, _, cx| {
            if *open {
                this.open_select = Some(id);
            } else if this.open_select == Some(id) {
                this.open_select = None;
            } else {
                return;
            }
            cx.notify();
        });
        let choose = cx.listener(move |this, i: &usize, _, cx| {
            if let Some(value) = values.get(*i) {
                pick(this, value.clone(), cx);
                cx.notify();
            }
        });
        div()
            .when_some(width, |d, w| d.w(px(w)))
            .when(width.is_none(), |d| d.w_full())
            .child(dialog::select(id, label, open, items, toggle, choose, cx))
            .into_any_element()
    }

    fn check_box(
        &self,
        id: &'static str,
        on: bool,
        text: &'static str,
        toggle: fn(&mut Form, bool),
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        dialog::check(id, on, text, cx).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            toggle(&mut this.form, !on);
            this.edited(cx);
        }))
    }
}

/// `options` with `saved` added when it isn't one of them - so it stays chosen; inserted in order when `sorted`.
fn with_saved(
    mut options: Vec<(String, String)>,
    saved: (String, String),
    sorted_by: Option<fn(&str) -> i64>,
) -> Vec<(String, String)> {
    if options.iter().any(|(_, v)| *v == saved.1) {
        return options;
    }
    match sorted_by {
        Some(key) => {
            let at = options.iter().take_while(|(_, v)| key(v) < key(&saved.1)).count();
            options.insert(at, saved);
        }
        None => options.push(saved),
    }
    options
}

fn as_number(value: &str) -> i64 {
    value.parse().unwrap_or(0)
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let shown = self.shown_categories(cx);

        // The categories.
        let mut list = column(2.0);
        for category in shown.iter().copied() {
            let selected = category == self.category;
            list = list.child(
                ui::focusable_bare(div().id(SharedString::from(format!("settings-category-{}", category.tag()))), cx)
                    .flex()
                    .items_center()
                    .gap(px(icons::LABEL_GAP))
                    .pad(px(8.0), px(14.0))
                    .rounded(theme.metrics.control_radius)
                    .cursor_pointer()
                    .text_color(c.text_primary)
                    .when(selected, |d| d.bg(c.selection_bg))
                    .when(!selected, |d| d.hover(move |s| s.bg(c.hover_bg)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_category(category, cx)))
                    .child(ui::icon_sized(glyph(category), IconSize::Nav, c.text_muted, cx))
                    .child(div().min_w_0().child(category.label())),
            );
        }
        let left = div()
            .flex()
            .flex_col()
            .gap(px(if shown.is_empty() { 24.0 } else { 20.0 }))
            .w(px(250.0))
            .flex_none()
            .pad(px(24.0), px(0.0))
            .child(self.search_box(&self.search.clone(), window, cx))
            .when(shown.is_empty(), |d| d.child(hint("No setting matches.", &theme)))
            .child(list);

        let content = match self.category {
            Category::General => self.render_general(window, cx),
            Category::Projects => self.render_projects(window, cx),
            Category::Releases => self.render_releases(window, cx),
            Category::Sql => self.render_sql(window, cx),
            Category::Docker => self.render_docker(window, cx),
            Category::Iis => self.render_iis(cx),
            Category::Servers => self.render_servers(cx),
            Category::Keyboard => self.render_keyboard(window, cx),
            Category::Help => self.render_help(cx),
            Category::About => self.render_about(cx),
        };
        let right = div().id("settings-content").flex_1().min_w_0().overflow_y_scroll().child(
            // 832 wide at most: with the 28 px of the page's padding beside it, 860 as before.
            div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .max_w(px(832.0))
                .pad(px(24.0), px(0.0))
                .when_some(self.env_warning.clone(), |d, text| {
                    d.child(
                        div()
                            .pad(px(10.0), px(12.0))
                            .rounded(theme.metrics.card_radius)
                            .border_1()
                            .border_color(c.warn_banner_border)
                            .bg(c.warn_banner_bg)
                            .child(text),
                    )
                })
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(icons::LABEL_GAP))
                            .text_color(c.error_text)
                            .child(ui::line_icon(Icon::Error, IconSize::Medium, c.error_text, cx))
                            .child(error),
                    )
                })
                .child(content),
        );

        let dirty = self.dirty;
        div()
            .id("settings")
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    // Clicked elsewhere: a list closes, a shortcut being recorded stays as it was.
                    if this.open_select.is_some() || this.recording.is_some() {
                        this.open_select = None;
                        this.stop_recording();
                        cx.notify();
                    }
                }),
            )
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
                    .child(
                        div()
                            .pad(px(0.0), px(8.0))
                            .text_size(theme.text.large)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Settings"),
                    )
                    .child(
                        ui::icon_button("settings-close", Icon::Close, c.link_fg, true, cx)
                            .tip("Close (Esc)")
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::Close))),
                    ),
            )
            .child(div().flex().flex_1().min_h_0().gap(px(48.0)).pad(px(0.0), px(28.0)).child(left).child(right))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pad(px(12.0), px(28.0))
                    .border_t_1()
                    .border_color(c.card_border)
                    .child(div().text_size(theme.text.hint).text_color(c.log_warn).child(if dirty {
                        "You have unsaved changes."
                    } else {
                        ""
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .when(dirty, |d| {
                                d.child(
                                    ui::button("settings-discard", "Discard changes", ButtonKind::Secondary, true, cx)
                                        .tip("Discard changes")
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.load(cx))),
                                )
                            })
                            .child(
                                ui::button("settings-close-button", "Close", ButtonKind::Secondary, true, cx)
                                    .min_w(px(84.0))
                                    .tip("Close (Esc)")
                                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::Close))),
                            )
                            .child(
                                ui::button("settings-save", "Save", ButtonKind::Primary, dirty, cx)
                                    .min_w(px(84.0))
                                    .tip("Save")
                                    .when(dirty, |b| {
                                        b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(cx)))
                                    }),
                            ),
                    ),
            )
    }
}

fn glyph(category: Category) -> Icon {
    match category {
        Category::General => Icon::Settings,
        Category::Projects => Icon::Folder,
        Category::Releases => Icon::Download,
        Category::Sql => Icon::Database,
        Category::Docker => Icon::Container,
        Category::Iis => Icon::Globe,
        Category::Servers => Icon::SourceControl,
        Category::Keyboard => Icon::Keyboard,
        Category::Help => Icon::Question,
        Category::About => Icon::Info,
    }
}

// ─── The categories ───────────────────────────────────────────────────────

impl SettingsPage {
    fn render_general(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let f = &self.form;
        let ui_scales = with_saved(
            rules::UI_SCALES
                .iter()
                .map(|s| (if *s == 100 { "100 % (default)".into() } else { format!("{s} %") }, s.to_string()))
                .collect(),
            (format!("{} %", f.ui_scale), f.ui_scale.to_string()),
            None,
        );
        let font_size = f.font_size.to_string();
        let font_sizes = with_saved(
            rules::FONT_SIZES
                .iter()
                .map(|s| (if *s == 13 { "13 px (default)".into() } else { format!("{s} px") }, s.to_string()))
                .collect(),
            (format!("{font_size} px"), font_size.clone()),
            None,
        );
        let shells: Vec<(String, String)> = match &self.shells {
            Some(shells) => shells.iter().map(|(k, n)| (n.clone(), k.clone())).collect(),
            None => vec![(f.shell.clone(), f.shell.clone())],
        };
        let fonts = with_saved(
            std::iter::once(("Default".to_string(), String::new()))
                .chain(self.fonts.iter().map(|f| (f.clone(), f.clone())))
                .collect(),
            (f.terminal_font.clone(), f.terminal_font.clone()),
            None,
        );
        let terminal_sizes = with_saved(
            rules::terminal_font_sizes().iter().map(|s| (s.to_string(), s.to_string())).collect(),
            (f.terminal_font_size.to_string(), f.terminal_font_size.to_string()),
            None,
        );
        let (ui_scale, shell, terminal_font, terminal_size) =
            (f.ui_scale.to_string(), f.shell.clone(), f.terminal_font.clone(), f.terminal_font_size.to_string());
        let (keep_running, check_updates, animations) = (f.keep_running, f.check_updates, f.animations);

        let sign_in_on = self.start_at_sign_in;
        let sign_in_enabled = self.sign_in_enabled;
        let sign_in =
            dialog::check("start-at-sign-in", sign_in_on, "Start DNN Manager when you sign in to your computer", cx)
                .when(!sign_in_enabled, |d| d.opacity(0.5).cursor_default())
                .when(sign_in_enabled, |d| {
                    d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.start_at_sign_in = !sign_in_on;
                        this.edited(cx);
                    }))
                });
        // The usual explanation is a tip beside the box; a problem (not installed for all users, set up for another
        // copy) stays under it, to be seen.
        let explained = self.sign_in_hint == SIGN_IN_TASK;
        let sign_in = if explained {
            ui::with_tip(sign_in, "start-at-sign-in-tip", SIGN_IN_TASK, cx)
        } else {
            div().flex().child(sign_in)
        };
        let mut sign_in_hint = div()
            .id("start-at-sign-in-hint")
            .text_size(theme.text.hint)
            .text_color(theme.colors.text_muted)
            .child(self.sign_in_hint.clone());
        if let Some(tip) = self.sign_in_tip.clone() {
            sign_in_hint = sign_in_hint.tip(tip);
        }

        let startup = column(8.0)
            .child(column(2.0).child(sign_in).when(!explained, |d| d.child(indented(sign_in_hint))))
            .child(ui::with_tip(
                self.check_box(
                    "keep-running",
                    keep_running,
                    "Keep DNN Manager running when you close the window",
                    |f, on| f.keep_running = on,
                    cx,
                ),
                "keep-running-tip",
                "Closing hides it by the clock; operations and keep warm go on. Right-click its icon to quit.",
                cx,
            ))
            .child(ui::with_tip(
                self.check_box(
                    "check-updates",
                    check_updates,
                    "Look for a newer DNN Manager when it starts",
                    |f, on| f.check_updates = on,
                    cx,
                ),
                "check-updates-tip",
                "Asks GitHub once at start. Off: only when you check for updates yourself.",
                cx,
            ));

        let scale = group_tip(
            "Scale and font size",
            "scale-tip",
            "UI scale zooms everything; font size changes only the text. Applies when you save.",
            8.0,
            cx,
        )
        .child(
            div()
                .flex()
                .gap(px(12.0))
                .child(column(4.0).child(dialog::label("UI scale", cx)).child(self.combo(
                    "ui-scale",
                    Some(160.0),
                    &ui_scale,
                    ui_scales,
                    |p, v, cx| {
                        p.form.ui_scale = v.parse().unwrap_or(100);
                        p.edited(cx);
                    },
                    cx,
                )))
                .child(column(4.0).child(dialog::label("Font size", cx)).child(self.combo(
                    "app-font-size",
                    Some(160.0),
                    &font_size,
                    font_sizes,
                    |p, v, cx| {
                        p.form.font_size = v.parse().unwrap_or(13.0);
                        p.edited(cx);
                    },
                    cx,
                ))),
        );

        let animation = group("Animations", 8.0, &theme).child(ui::with_tip(
            self.check_box("animations", animations, "Play animations", |f, on| f.animations = on, cx),
            "animations-tip",
            "Slides and fades. Also off while Windows' animation effects are off.",
            cx,
        ));

        let terminal = group_tip(
            "Terminal",
            "terminal-tip",
            "Terminals run as you. The arrow beside + opens an Administrator one.",
            8.0,
            cx,
        )
        .child(
            column(14.0)
                .child(
                    column(4.0)
                        .child(ui::with_tip(dialog::label("Default shell", cx), "shell-tip", "What + opens.", cx))
                        .child(self.combo(
                            "default-shell",
                            Some(240.0),
                            &shell,
                            shells,
                            |p, v, cx| {
                                p.form.shell = v;
                                p.edited(cx);
                            },
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(
                            column(4.0)
                                .child(ui::with_tip(
                                    dialog::label("Font family", cx),
                                    "terminal-font-tip",
                                    "Also the font of the Output and Logs tabs.",
                                    cx,
                                ))
                                .child(self.combo(
                                    "terminal-font",
                                    Some(240.0),
                                    &terminal_font,
                                    fonts,
                                    |p, v, cx| {
                                        p.form.terminal_font = v;
                                        p.edited(cx);
                                    },
                                    cx,
                                )),
                        )
                        .child(column(4.0).child(dialog::label("Font size", cx)).child(self.combo(
                            "terminal-font-size",
                            Some(90.0),
                            &terminal_size,
                            terminal_sizes,
                            |p, v, cx| {
                                p.form.terminal_font_size = v.parse().unwrap_or(13);
                                p.edited(cx);
                            },
                            cx,
                        ))),
                ),
        );

        column(20.0)
            .child(category_title("General", &theme))
            .child(column(22.0).child(startup).child(scale).child(animation).child(terminal))
            .into_any_element()
    }
    fn render_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let f = self.form.clone();
        let data = &self.services.data_folder;
        let languages: Vec<(String, String)> =
            rules::LANGUAGES.iter().map(|l| (rules::language_name(l), l.to_string())).collect();
        let templates: Vec<(String, String)> =
            rules::TEMPLATES.iter().map(|t| (t.to_string(), t.to_string())).collect();
        let intervals = with_saved(
            rules::PING_INTERVALS.iter().map(|m| (rules::ping_interval_text(*m), m.to_string())).collect(),
            (rules::ping_interval_text(f.ping_minutes), f.ping_minutes.to_string()),
            Some(as_number),
        );
        let keep_days = with_saved(
            rules::KEEP_DAY_CHOICES.iter().map(|d| (rules::keep_days_text(*d), d.to_string())).collect(),
            (rules::keep_days_text(f.keep_days), f.keep_days.to_string()),
            Some(as_number),
        );
        let backups_tip = format!(
            "Project backups: {}\nDeployment packages: {}",
            data.join("backups").display(),
            data.join("deployments").display()
        );
        let automatic = f.install_automatic;
        let install_radio =
            |id: &'static str, chosen: bool, text: &'static str, value: bool, cx: &mut Context<Self>| {
                dialog::radio(id, chosen, text, cx).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.form.install_automatic = value;
                    this.edited(cx);
                }))
            };

        let folder = group_tip("Projects folder", "projects-folder-tip", "Every project is a folder in here.", 8.0, cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().child(self.text_box(Field::BaseDirectory, window, cx)))
                    .child(
                        ui::button("browse-projects-folder", "Browse…", ButtonKind::Secondary, true, cx)
                            .tip("Browse")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.browse_projects_folder(cx))),
                    ),
            );

        let address = group("Site address", 8.0, &theme).child(
            column(4.0)
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(column(4.0).flex_1().child(dialog::label("Hostname suffix", cx)).child(self.text_box(
                            Field::HostnameSuffix,
                            window,
                            cx,
                        )))
                        .child(column(4.0).w(px(140.0)).child(dialog::label("Site port", cx)).child(self.text_box(
                            Field::SitePort,
                            window,
                            cx,
                        ))),
                )
                .child(hint(f.url_preview(), &theme)),
        );

        let fields = column(10.0)
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .child(self.labelled("Host username", Field::HostUsername, window, cx))
                    .child(self.labelled_password("Host password", Secret::Host, cx)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .child(self.labelled("Host e-mail", Field::HostEmail, window, cx))
                    .child(self.labelled("Website name", Field::WebsiteName, window, cx)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .child(column(4.0).flex_1().child(dialog::label("Language", cx)).child(self.combo(
                        "dnn-language",
                        None,
                        &f.language,
                        languages,
                        |p, v, cx| {
                            p.form.language = v;
                            p.edited(cx);
                        },
                        cx,
                    )))
                    .child(column(4.0).flex_1().child(dialog::label("Site template", cx)).child(self.combo(
                        "dnn-template",
                        None,
                        &f.template,
                        templates,
                        |p, v, cx| {
                            p.form.template = v;
                            p.edited(cx);
                        },
                        cx,
                    ))),
            );
        let defaults = group_tip(
            "DNN defaults",
            "dnn-defaults-tip",
            format!(
                "What a new project starts with - New project lets you change each of them for that project.\n{}",
                f.defaults_hint()
            ),
            8.0,
            cx,
        )
        .child(
            column(16.0)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(install_radio(
                            "install-automatic",
                            automatic,
                            "Automatic setup (recommended) - DNN Manager installs DNN, the first visit shows the new site",
                            true,
                            cx,
                        ))
                        .child(install_radio(
                            "install-manual",
                            !automatic,
                            "Manual DNN setup - DNN's installation wizard on the first visit",
                            false,
                            cx,
                        )),
                )
                .child(fields),
        );

        let keep_warm = group_tip(
            "Keep warm",
            "keep-warm-tip",
            "Sites with the flame on are requested before IIS idles them, so their next page opens at once.",
            8.0,
            cx,
        )
        .child(
            div()
                .flex()
                .gap(px(12.0))
                .child(
                    column(4.0)
                        .w(px(180.0))
                        .child(ui::with_tip(
                            dialog::label("Interval", cx),
                            "keep-warm-interval-tip",
                            "Keep-alive is requested every interval; warm-up after a recycle or an IIS start.",
                            cx,
                        ))
                        .child(self.combo(
                            "keep-warm-interval",
                            Some(180.0),
                            &f.ping_minutes.to_string(),
                            intervals,
                            |p, v, cx| {
                                p.form.ping_minutes = v.parse().unwrap_or(5);
                                p.edited(cx);
                            },
                            cx,
                        )),
                )
                .child(self.labelled("Keep-alive page", Field::PingPath, window, cx))
                .child(self.labelled("Warm-up page", Field::WarmUpPath, window, cx)),
        );

        let backups = group("Backups", 8.0, &theme).child(
            column(4.0)
                .child(dialog::label("Keep backups and deployment packages", cx))
                .child(self.combo(
                    "backup-keep-days",
                    Some(280.0),
                    &f.keep_days.to_string(),
                    keep_days,
                    |p, v, cx| {
                        p.form.keep_days = v.parse().unwrap_or(0);
                        p.edited(cx);
                    },
                    cx,
                ))
                .child(
                    div()
                        .id("backups-hint")
                        .tip(backups_tip)
                        .child(hint("Older ones are deleted at each start - they can't be brought back.", &theme)),
                ),
        );

        column(28.0)
            .child(category_title("Projects", &theme))
            .child(column(22.0).child(folder).child(address).child(defaults).child(keep_warm).child(backups))
            .into_any_element()
    }

    fn render_releases(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let packages = self.services.data_folder.join("packages").display().to_string();
        let keep = self.form.keep_packages;
        let mut repositories = column(4.0);
        for input in &self.release_inputs {
            repositories = repositories.child(dialog::text_field(input, window, cx));
        }
        let repositories = group_tip(
            "Repositories",
            "repositories-tip",
            "GitHub releases API URLs, one per line - the repositories New project offers, with their versions.",
            8.0,
            cx,
        )
        .child(repositories);
        let install = group("Install packages", 8.0, &theme).child(ui::with_tip(
            self.check_box(
                "keep-dnn-packages",
                keep,
                "Keep downloaded DNN install packages for next time",
                |f, on| f.keep_packages = on,
                cx,
            ),
            "keep-dnn-packages-tip",
            format!("Installs the same version again without downloading it.\n{packages}"),
            cx,
        ));
        let upgrade_path = group_tip(
            "Upgrade path",
            "upgrade-path-tip",
            rules::upgrade_path_hint(&rules::upgrade_stops(&self.saved)),
            8.0,
            cx,
        )
        .child(
            div().flex().child(
                ui::button("edit-upgrade-path", "Edit upgrade path…", ButtonKind::Secondary, true, cx)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::EditUpgradePath))),
            ),
        );
        column(28.0)
            .child(category_title("DNN releases", &theme))
            .child(column(22.0).child(repositories).child(install).child(upgrade_path))
            .into_any_element()
    }

    fn server_type_card(
        &self,
        id: &'static str,
        server_type: &'static str,
        title: &'static str,
        text: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let chosen = self.form.server_type == server_type;
        ui::focusable_bare(div().id(id), cx)
            .flex()
            .items_start()
            .gap(px(8.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.form.server_type != server_type {
                    this.form.set_server_type(server_type);
                    this.sync_input(Field::ServerName, cx);
                    this.edited(cx);
                }
            }))
            .child(radio_dot(chosen, cx))
            .child(ui::with_tip(
                div().font_weight(FontWeight::SEMIBOLD).child(title),
                SharedString::from(format!("{id}-tip")),
                text,
                cx,
            ))
    }

    fn render_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let f = self.form.clone();
        let sql_server = f.server_type == SQL_SERVER;
        let container = f.server_type == CONTAINER;
        let ssms = f.ssms_remember;
        let auth_radio = |id: &'static str, chosen: bool, text: &'static str, sql: bool, cx: &mut Context<Self>| {
            dialog::radio(id, chosen, text, cx).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.form.sql_auth = sql;
                this.edited(cx);
            }))
        };

        let container_fields = group_tip(
            "Local SQL container",
            "local-sql-container-tip",
            "Set up docker-compose makes the container with this port and sa password.",
            18.0,
            cx,
        )
        .child(
            column(10.0)
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(self.labelled("Server host", Field::ContainerHost, window, cx))
                        .child(self.labelled("Port", Field::ContainerPort, window, cx)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(self.labelled("Username", Field::ContainerUser, window, cx))
                        .child(self.labelled_password("Password", Secret::Sa, cx)),
                ),
        );

        let server_fields = group(if sql_server { "SQL Server" } else { "LocalDB" }, 8.0, &theme).child(
            column(10.0)
                .child(
                    column(4.0)
                        .child(ui::with_tip(
                            dialog::label(if sql_server { "Server" } else { "LocalDB instance" }, cx),
                            "server-name-tip",
                            f.server_hint(),
                            cx,
                        ))
                        .child(self.text_box(Field::ServerName, window, cx)),
                )
                .when(sql_server, |d| {
                    d.child(
                        column(4.0)
                            .child(ui::with_tip(
                                dialog::label("Authentication", cx),
                                "authentication-tip",
                                f.auth_hint(),
                                cx,
                            ))
                            .child(
                                column(10.0)
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(20.0))
                                            .child(auth_radio(
                                                "windows-auth",
                                                !f.sql_auth,
                                                "Windows authentication",
                                                false,
                                                cx,
                                            ))
                                            .child(auth_radio(
                                                "sql-auth",
                                                f.sql_auth,
                                                "SQL Server authentication",
                                                true,
                                                cx,
                                            )),
                                    )
                                    .when(f.sql_auth, |d| {
                                        d.child(
                                            div()
                                                .flex()
                                                .gap(px(12.0))
                                                .child(self.labelled("Username", Field::ServerUser, window, cx))
                                                .child(self.labelled_password("Password", Secret::Server, cx)),
                                        )
                                    }),
                            ),
                    )
                }),
        );

        let connection = group("Connection type", 16.0, &theme).child(
            column(8.0)
                .child(self.server_type_card(
                    "server-container",
                    CONTAINER,
                    "Local SQL container (Docker)",
                    "SQL Server in the Docker container from Settings → Docker container, signed in to with the username below (sa by default).",
                    cx,
                ))
                .child(self.server_type_card(
                    "server-sql-server",
                    SQL_SERVER,
                    "SQL Server / SQL Server Express",
                    "A SQL Server you already run on this PC (or your network), with Windows or SQL Server authentication.",
                    cx,
                ))
                .child(self.server_type_card(
                    "server-local-db",
                    LOCAL_DB_FILE,
                    "SQL Server Express LocalDB (file)",
                    r"Each site's own App_Data\Database.mdf - fine for trying things out; DNN Manager can't open it while the site runs.",
                    cx,
                )),
        );

        let ssms_group = group("SQL Server Management Studio", 8.0, &theme).child(ui::with_tip(
            self.check_box(
                "ssms-remember-password",
                ssms,
                "Remember the password in SQL Server Management Studio",
                |f, on| f.ssms_remember = on,
                cx,
            ),
            "ssms-remember-password-tip",
            "When a project's right-click menu signs SSMS in, tick its 'Remember Password' - SSMS then keeps the password and connects straight away next time.",
            cx,
        ));

        let test = group("Test and set up", 8.0, &theme)
            .when(self.dirty, |d| d.child(hint("These use the saved settings - save your changes first.", &theme)))
            .child(self.render_sql_card(cx));

        column(20.0)
            .child(ui::with_tip(
                category_title("Database server", &theme),
                "database-server-tip",
                "Where new projects get their database. Existing projects keep theirs.",
                cx,
            ))
            .child(
                column(22.0)
                    .child(connection)
                    .when(container, |d| d.child(container_fields))
                    .when(!container, |d| d.child(server_fields))
                    .child(ssms_group)
                    .child(test),
            )
            .into_any_element()
    }

    fn render_docker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let container = group("Container", 18.0, &theme).child(
            column(10.0)
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(self.labelled("Container name", Field::ContainerName, window, cx))
                        .child(self.labelled("Volume name", Field::VolumeName, window, cx)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(self.labelled("Edition (MSSQL_PID)", Field::Edition, window, cx))
                        .child(self.labelled("Collation", Field::Collation, window, cx)),
                ),
        );
        let test = group("Test and set up", 8.0, &theme)
            .when(self.dirty, |d| d.child(hint("These use the saved settings - save your changes first.", &theme)))
            .child(self.render_docker_card(cx));
        column(20.0)
            .child(ui::with_tip(
                category_title("Docker container", &theme),
                "docker-container-tip",
                "The SQL Server container. After a change, save it and apply it with Set up docker-compose below.",
                cx,
            ))
            .child(column(22.0).child(container).child(test))
            .into_any_element()
    }

    fn render_iis(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        column(20.0)
            .child(ui::with_tip(
                category_title("IIS", &theme),
                "iis-tip",
                "The Windows features DNN Manager needs. These buttons act at once, without Save.",
                cx,
            ))
            .child(self.render_iis_card(cx))
            .into_any_element()
    }

    fn render_servers(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let count = self.servers.as_ref().map_or(0, Vec::len);
        let button = |id: &'static str, label: &'static str, tip: &'static str, enabled: bool, event: SettingsEvent, cx: &mut Context<Self>| {
            ui::button(id, label, ButtonKind::Secondary, enabled, cx)
                .tip(tip)
                .when(enabled, |b| b.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(event.clone()))))
        };
        let buttons = div()
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .child(button("servers-add", "Add server…", "A new server, as in FileZilla's Site Manager", true, SettingsEvent::AddServer, cx))
            .child(button(
                "servers-import",
                "Import…",
                "Add the sites of a FileZilla Site Manager file (FileZilla's File → Export…, or sitemanager.xml)",
                true,
                SettingsEvent::ImportServers,
                cx,
            ))
            .child(button(
                "servers-export",
                "Export…",
                "Every server to a file FileZilla - or DNN Manager on another computer - imports",
                count > 0,
                SettingsEvent::ExportServers,
                cx,
            ));

        let list = match &self.servers {
            Err(e) => div()
                .flex()
                .items_start()
                .gap(px(icons::LABEL_GAP))
                .text_color(c.error_text)
                .child(ui::line_icon(Icon::Error, IconSize::Medium, c.error_text, cx))
                .child(div().min_w_0().child(format!("The servers couldn't be read: {e}"))),
            Ok(servers) if servers.is_empty() => hint(
                "No servers yet. Add one, or import the sites you have in FileZilla: there, choose File → Export…, then Import… here.",
                &theme,
            ),
            Ok(servers) => {
                let mut rows = div().flex().flex_col();
                for (index, server) in servers.iter().enumerate() {
                    let mut details = format!(
                        "{} · {}@{}:{} · {}",
                        server.protocol.key().to_uppercase(),
                        server.user,
                        server.host,
                        server.port,
                        server.remote_root
                    );
                    if matches!(server.auth, Auth::KeyFile(_)) {
                        details.push_str(" · key file");
                    }
                    let (edit, delete) = (server.clone(), server.clone());
                    rows = rows.child(
                        div()
                            .id(SharedString::from(format!("server-{}", server.id)))
                            .flex()
                            .items_center()
                            .gap(px(16.0))
                            .pad(px(10.0), px(0.0))
                            .border_b_1()
                            .when(index == 0, |d| d.border_t_1())
                            .border_color(c.grid_line)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(server.name.clone()))
                                    .child(
                                        div()
                                            .text_size(theme.text.hint)
                                            .text_color(c.text_muted)
                                            .truncate()
                                            .child(details),
                                    ),
                            )
                            .child(
                                ui::button(SharedString::from(format!("server-edit-{}", server.id)), "Edit…", ButtonKind::Secondary, true, cx)
                                    .tip(format!("Edit {}", server.name))
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                        cx.emit(SettingsEvent::EditServer(edit.clone()))
                                    })),
                            )
                            .child(
                                ui::icon_button(SharedString::from(format!("server-delete-{}", server.id)), Icon::Delete, c.text_muted, true, cx)
                                    .tip(format!("Delete {}…", server.name))
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                        cx.emit(SettingsEvent::DeleteServer(delete.clone()))
                                    })),
                            ),
                    );
                }
                rows
            }
        };

        column(20.0)
            .child(ui::with_tip(
                category_title("Servers", &theme),
                "servers-tip",
                "The servers Source Control compares your projects with and sends files to - over SFTP, FTPS or FTP. Every project can use every server. These buttons act at once, without Save; passwords stay in your system's secure store.",
                cx,
            ))
            .child(buttons)
            .child(list)
            .into_any_element()
    }

    fn render_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let rows = self.shortcut_rows();
        let any_custom = rows.iter().any(|r| r.is_custom);
        let search = self.shortcut_search.read(cx).text().trim().to_string();
        let shown: Vec<ShortcutRow> = rows.into_iter().filter(|r| r.matches(&search)).collect();
        let header = |text: &'static str| div().text_color(c.text_primary).child(text);

        let mut list = div().flex().flex_col();
        for (i, row) in shown.iter().enumerate() {
            let recording_slot =
                self.recording.as_ref().filter(|(id, _)| *id == row.command.id).map(|(_, slot)| *slot);
            let reset_id = row.command.id.clone();
            // Each shortcut a chip (a click records a new one in its place); "+" records another after them. While
            // recording, the chip being changed - or the new one - says so.
            let chip = |key: String, text: String, slot: Slot, tip: &'static str, cx: &mut Context<Self>| {
                let id = row.command.id.clone();
                let lit = recording_slot == Some(slot);
                let pressed = id.clone();
                ui::focusable(div().id(SharedString::from(key)), cx)
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .pad(px(0.0), px(8.0))
                    .rounded(theme.metrics.control_radius)
                    .border_1()
                    .border_color(if lit { c.focus_border } else { c.card_border })
                    .bg(c.card_bg)
                    .text_size(theme.text.caption)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(move |s| s.bg(c.hover_bg))
                    .tip(tip)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.start_recording(id.clone(), slot, cx);
                        }),
                    )
                    // Enter or Space on it records too - from the keys after it (the mouse starts on the press).
                    .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                        if e.is_keyboard() {
                            this.start_recording(pressed.clone(), slot, cx);
                        }
                    }))
                    .child(if lit { "Press the keys…".to_string() } else { text })
            };
            let mut keys = div().flex().flex_wrap().items_center().gap(px(4.0));
            for (n, shortcut) in row.shortcuts.iter().enumerate() {
                let slot = Slot::At(n);
                keys = keys.child(chip(
                    format!("shortcut-{i}-{n}"),
                    shortcut.to_string(),
                    slot,
                    ShortcutRow::shortcut_tip(recording_slot == Some(slot)),
                    cx,
                ));
            }
            keys = keys.child(chip(
                format!("shortcut-{i}-add"),
                if row.shortcuts.is_empty() { "Add shortcut".into() } else { "+".into() },
                Slot::New,
                ShortcutRow::add_tip(recording_slot == Some(Slot::New)),
                cx,
            ));
            let custom = row.is_custom;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .pad(px(7.0), px(0.0))
                    .border_t_1()
                    .border_color(c.card_border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(row.command.title.clone())
                            .child(div().text_size(theme.text.caption).text_color(c.text_muted).child(row.detail()))
                            .when(!row.warning.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(theme.text.caption)
                                        .text_color(c.log_warn)
                                        .child(row.warning.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_none()
                            .child(div().w(px(200.0)).child(keys))
                            .child(
                                div().w(px(36.0)).flex().justify_end().child(
                                    ui::icon_button(
                                        SharedString::from(format!("reset-shortcut-{i}")),
                                        Icon::Reset,
                                        c.text_primary,
                                        custom,
                                        cx,
                                    )
                                    .tip(row.reset_tip())
                                    .when(custom, |b| {
                                        b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.reset_shortcut(&reset_id, cx)
                                        }))
                                    }),
                                ),
                            ),
                    ),
            );
        }

        // Search, then the status (when there is one), then the table: 12 and 10 apart; 4 under the header row, 8
        // around "No command matches.".
        let table = column(if shown.is_empty() { 8.0 } else { 4.0 })
            .child(
                div()
                    .flex()
                    .child(div().flex_1().child(header("Action")))
                    .child(div().w(px(200.0)).child(header("Shortcut")))
                    .child(div().w(px(36.0))),
            )
            .when(shown.is_empty(), |d| d.child(hint("No command matches.", &theme)))
            .child(list);
        let controls =
            column(12.0)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(div().flex_1().child(self.search_box(&self.shortcut_search.clone(), window, cx)))
                        .child(
                            ui::button(
                                "reset-all-shortcuts",
                                "Reset all to defaults",
                                ButtonKind::Secondary,
                                any_custom,
                                cx,
                            )
                            .tip("Reset all to defaults")
                            .when(any_custom, |b| {
                                b.on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                    cx.emit(SettingsEvent::ConfirmResetAllShortcuts)
                                }))
                            }),
                        ),
                )
                .child(
                    column(10.0)
                        .when_some(self.shortcut_status.clone(), |d, status| {
                            d.child(div().text_size(theme.text.hint).text_color(c.log_warn).child(status))
                        })
                        .child(table),
                );

        column(20.0)
            .child(ui::with_tip(
                category_title("Keyboard shortcuts", &theme),
                "keyboard-shortcuts-tip",
                "Click a shortcut and press the new keys - Esc cancels, Backspace removes it. Saved at once.",
                cx,
            ))
            .child(controls)
            .into_any_element()
    }

    fn help_row(
        &self,
        id: &'static str,
        title: &'static str,
        text: &'static str,
        button: Option<(&'static str, &'static str, SettingsEvent)>,
        first: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = Theme::get(cx).clone();
        div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .pad(px(14.0), px(0.0))
            .border_b_1()
            .when(first, |d| d.border_t_1())
            .border_color(theme.colors.grid_line)
            .child(div().flex_1().min_w_0().child(ui::with_tip(
                div().font_weight(FontWeight::SEMIBOLD).child(title),
                SharedString::from(format!("{id}-tip")),
                text,
                cx,
            )))
            .when_some(button, |d, (label, tip, event)| {
                d.child(
                    ui::button(id, label, ButtonKind::Secondary, true, cx)
                        .min_w(px(120.0))
                        .tip(tip)
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(event.clone()))),
                )
            })
    }

    fn render_help(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let rows = div()
            .flex()
            .flex_col()
            .child(self.help_row(
                "help-guide",
                "Getting started guide",
                "What DNN Manager does, what it needs before you start, and what every button does - in plain words, step by step.",
                Some((
                    "Show getting started guide",
                    "What DNN Manager does, what to set up first and what every button does - step by step",
                    SettingsEvent::ShowGuide,
                )),
                true,
                cx,
            ))
            .child(self.help_row(
                "help-tour",
                "Tour of the window",
                "Points at each part of the window, one at a time, and says what it is for. Settings closes first.",
                Some(("Take the tour", "Close Settings and point at each part of the window", SettingsEvent::StartTour)),
                false,
                cx,
            ))
            .child(self.help_row(
                "help-f1",
                "Help for the page you are on",
                "F1, or the ? at the top right, explains the page you are on.",
                None,
                false,
                cx,
            ))
            .when(self.offer_whats_new, |d| {
                d.child(self.help_row(
                    "help-whats-new",
                    "What's new in this version",
                    "What changed in the version you have.",
                    Some(("What's new", "What changed in this version of DNN Manager", SettingsEvent::ShowWhatsNew)),
                    false,
                    cx,
                ))
            })
            .child(self.help_row(
                "help-user-guide",
                "User guide",
                "Every page, button and setting in detail, on GitHub - opens in your browser.",
                Some(("Open user guide", "The full user guide on GitHub, in your browser", SettingsEvent::OpenUserGuide)),
                false,
                cx,
            ));
        column(20.0)
            .child(ui::with_tip(
                category_title("Help", &theme),
                "help-tip",
                "New to DNN Manager, or looking for something? Open these whenever you like - they don't change any setting.",
                cx,
            ))
            .child(rows)
            .into_any_element()
    }

    fn render_about(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let mut blocks = column(14.0);
        for section in about::sections(&self.update, self.machine.as_ref(), self.docker_about.as_deref()) {
            let mut rows = div().flex().flex_col();
            for row in section.rows {
                rows = rows.child(
                    div()
                        .flex()
                        .pad(px(3.0), px(0.0))
                        .child(div().w(px(180.0)).flex_none().text_color(c.text_muted).child(row.key))
                        .child(
                            column(4.0)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .children(health_dot(row.health, cx))
                                        .child(row.value),
                                )
                                .when_some(row.detail, |d, detail| d.child(hint(detail, &theme))),
                        ),
                );
            }
            blocks = blocks.child(
                column(4.0)
                    .child(
                        div()
                            .flex()
                            .gap(px(6.0))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(section.title))
                            .child(div().text_color(c.text_muted).child(section.subtitle)),
                    )
                    .child(rows),
            );
        }
        let mut folders = div().flex().flex_col();
        for (i, (name, folder)) in self.data_folders().into_iter().enumerate() {
            let text = folder.display().to_string();
            folders = folders.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .pad(px(4.0), px(0.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .child(div().w(px(110.0)).flex_none().text_color(c.text_muted).child(name))
                            .child(
                                div()
                                    .id(SharedString::from(format!("folder-path-{i}")))
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .tip(text.clone())
                                    .child(text),
                            ),
                    )
                    .child(
                        ui::link(SharedString::from(format!("open-folder-{i}")), "Open", cx)
                            .tip("Open folder")
                            .on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| this.open_folder(folder.clone(), cx)),
                            ),
                    ),
            );
        }
        let files = group_tip(
            "Your files",
            "your-files-tip",
            "Kept apart from the program: updating or uninstalling DNN Manager leaves these folders alone.",
            8.0,
            cx,
        )
        .child(folders);
        column(20.0)
            .child(category_title("About", &theme))
            .child(column(22.0).child(blocks).child(files))
            .into_any_element()
    }
}

/// How long Start Docker Desktop waits for the engine, between two looks.
pub(crate) const ENGINE_POLL: Duration = Duration::from_secs(5);
