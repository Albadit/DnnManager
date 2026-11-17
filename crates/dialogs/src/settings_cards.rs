//! Settings' Test and set up cards: the database server (`Controls/DatabaseServerCard.xaml(.cs)`,
//! `DatabaseCheckList`), Docker (`Controls/DockerCard.xaml(.cs)`) and IIS (`Controls/IisCard.xaml(.cs)`). Each works
//! with the saved settings and asks off the UI thread; an answer for an older test (or for settings saved since) is
//! dropped. What changes the system - Set up docker-compose, Install Docker Desktop, Set up IIS - is the app's
//! operation, asked for with an event.
//!
//! Deviation: the IIS features' states come from `dnnm_system::prereq::get_iis_feature_states`, which reads a feature
//! it got no answer for as not enabled - so a feature shows Missing where the WPF app says Unknown, and the summary
//! never says the features couldn't be read.

use dnnm_ui::px;
use std::collections::BTreeMap;

use gpui::{AnyElement, App, ClickEvent, ClipboardItem, Context, Div, FontWeight, Hsla, SharedString, div, prelude::*};

use dnnm_application::settings::Settings;
use dnnm_sql::{CheckOutcome, DatabaseCheckReport, DockerStatus};
use dnnm_system::prereq::IisFeature;
use dnnm_system::secrets::names;
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, ButtonKind, Pad, Tip, icons};

use crate::setting_rules::{iis_features, value};
use crate::settings_form::{CONTAINER, LOCAL_DB_FILE, container_user, database_on_server, docker_options, uses_login};
use crate::settings_page::{ENGINE_POLL, Notice, SettingsEvent, SettingsPage, hint, off_thread};

// ─── Database server ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct SqlCardState {
    pub(crate) version: u64,
    pub(crate) testing: bool,
    pub(crate) report: Option<DatabaseCheckReport>,
}

impl SqlCardState {
    /// What was shown belongs to the server as it was.
    pub fn settings_changed(&mut self) {
        self.version += 1;
        self.testing = false;
        self.report = None;
    }
}

/// The server in the settings, in words - never the password (`ShowSummary`).
pub fn sql_summary(settings: &Settings) -> String {
    let kind = value(settings, "sqlServer.type");
    let server = value(settings, "sqlServer.server").trim().to_string();
    if kind.eq_ignore_ascii_case(CONTAINER) {
        format!(
            "Local SQL container (Docker) at {},{}, as {}.",
            value(settings, "sqlServer.host"),
            value(settings, "sqlServer.port"),
            container_user(settings)
        )
    } else if kind.eq_ignore_ascii_case(LOCAL_DB_FILE) {
        format!("SQL Server Express LocalDB ({server}) - each site's own database file.")
    } else if uses_login(settings) {
        format!("{server}, with sql server authentication as '{}'.", value(settings, "sqlServer.userName").trim())
    } else {
        format!("{server}, with windows authentication.")
    }
}

// ─── Docker ───────────────────────────────────────────────────────────────

/// A status word's colour (`StatusTone`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Warning,
    Muted,
}

impl Tone {
    fn color(self, cx: &App) -> Hsla {
        let c = Theme::get(cx).colors;
        match self {
            Tone::Good => c.success_text,
            Tone::Bad => c.error_text,
            Tone::Warning => c.log_warn,
            Tone::Muted => c.text_muted,
        }
    }
}

/// One of the card's lines: its status word and what is said beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub status: String,
    pub tone: Tone,
    pub detail: String,
}

fn line(status: &str, tone: Tone, detail: impl Into<String>) -> StatusLine {
    StatusLine { status: status.into(), tone, detail: detail.into() }
}

/// Docker Desktop, its engine and the container, as the card shows them (`Show`).
pub fn docker_lines(d: &DockerStatus, container: &str) -> [StatusLine; 3] {
    let desktop = if d.desktop_installed {
        line(
            "Installed",
            Tone::Good,
            d.client_version.as_ref().map_or_else(|| "Docker Desktop".to_string(), |v| format!("docker {v}")),
        )
    } else {
        line(
            "Not installed",
            Tone::Bad,
            "Runs the SQL Server container. Not needed if Settings → Database server points at a SQL Server you already run.",
        )
    };
    let engine = if d.engine_running {
        line("Running", Tone::Good, format!("engine {}", d.engine_version.clone().unwrap_or_default()))
    } else if d.desktop_installed {
        line("Stopped", Tone::Bad, "Start Docker Desktop and wait until the engine runs.")
    } else {
        line("-", Tone::Muted, "")
    };
    let sql = if !d.engine_running {
        line("-", Tone::Muted, format!("'{container}' - needs the Docker engine."))
    } else {
        match &d.container_state {
            None => line(
                "Not created",
                Tone::Bad,
                format!("No container named '{container}' - create it with Set up docker-compose below."),
            ),
            Some(state) if state.eq_ignore_ascii_case("running") => line(
                "Running",
                Tone::Good,
                format!("'{container}' - {}", d.container_status.clone().unwrap_or_default()),
            ),
            Some(state) => line(
                "Stopped",
                Tone::Bad,
                format!(
                    "'{container}' - {}. Start it with Set up docker-compose below.",
                    d.container_status.clone().unwrap_or_else(|| state.clone())
                ),
            ),
        }
    };
    [desktop, engine, sql]
}

/// What Set up docker-compose still needs, or None when it can run (`ShowSetUp`).
pub fn setup_missing(d: &DockerStatus) -> Option<&'static str> {
    if d.can_set_up() {
        None
    } else if !d.desktop_installed {
        Some("Needs Docker Desktop - install it above.")
    } else {
        Some("Needs the Docker engine running - start Docker Desktop above.")
    }
}

#[derive(Debug, Default)]
pub struct DockerCardState {
    /// 0 until the first test: the first time Docker container is shown, it tests by itself.
    pub(crate) version: u64,
    pub(crate) testing: bool,
    pub(crate) status: Option<DockerStatus>,
    pub(crate) setup_enabled: bool,
    pub(crate) setup_hint: Option<String>,
    /// The engine's line while Start Docker Desktop waits for it, or when it didn't come up.
    pub(crate) engine_note: Option<StatusLine>,
    pub(crate) starting: bool,
    pub(crate) yaml: Option<String>,
    pub(crate) copied: bool,
}

// ─── IIS ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct IisCardState {
    pub(crate) version: u64,
    pub(crate) testing: bool,
    pub(crate) states: Option<BTreeMap<String, bool>>,
}

impl IisCardState {
    /// The features were edited: a test still running was about the old list.
    pub fn features_changed(&mut self) {
        self.version += 1;
        self.testing = false;
        self.states = None;
    }
}

/// The table's rows: (feature, status, Windows feature name).
pub fn iis_rows(
    features: &[IisFeature],
    states: Option<&BTreeMap<String, bool>>,
) -> Vec<(String, &'static str, String)> {
    features
        .iter()
        .map(|f| {
            let status = match states {
                None => "Not tested",
                Some(states) => match states.get(&f.name) {
                    None => "Unknown",
                    Some(true) => "Enabled",
                    Some(false) => "Missing",
                },
            };
            (f.label.clone(), status, f.name.clone())
        })
        .collect()
}

/// The line under IIS (`Show`, `ShowUntested`).
pub fn iis_summary(rows: &[(String, &'static str, String)], tested: bool, testing: bool) -> String {
    if testing {
        return "Testing…".into();
    }
    if !tested {
        return "Windows features: not tested yet - press Test.".into();
    }
    let n = rows.len();
    let missing = rows.iter().filter(|r| r.1 == "Missing").count();
    let unknown = rows.iter().filter(|r| r.1 == "Unknown").count();
    if unknown == n && n > 0 {
        "Couldn't read the Windows features - DNN Manager needs administrator rights.".into()
    } else if missing == 0 {
        format!("All {n} Windows features are enabled.")
    } else {
        format!("{} of {n} Windows features enabled - {missing} missing.", n - missing - unknown)
    }
}

// ─── The cards on the page ────────────────────────────────────────────────

fn card(cx: &App) -> Div {
    let theme = Theme::get(cx);
    div()
        .flex()
        .flex_col()
        .pad(px(14.0), px(18.0))
        .rounded(theme.metrics.card_radius)
        .border_1()
        .border_color(theme.colors.card_border)
        .bg(theme.colors.card_bg)
}

fn card_title(text: &'static str) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).child(text)
}

impl SettingsPage {
    /// Tests the server in the saved settings.
    pub(crate) fn test_sql(&mut self, cx: &mut Context<Self>) {
        self.sql_card.version += 1;
        let version = self.sql_card.version;
        self.sql_card.testing = true;
        self.sql_card.report = None;
        let password = if uses_login(&self.saved) {
            self.services.secrets.read(names::DATABASE_SERVER_PASSWORD).unwrap_or_default()
        } else {
            String::new()
        };
        let connection = database_on_server(&self.saved, &self.saved_sa, &password);
        let test = self.services.test_database.clone();
        off_thread(
            cx,
            move || test(connection),
            move |page, report, _| {
                if page.sql_card.version == version {
                    page.sql_card.testing = false;
                    page.sql_card.report = report.ok();
                }
            },
        );
        cx.notify();
    }

    pub(crate) fn render_sql_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let enabled = !self.dirty;
        let testing = self.sql_card.testing;
        let mut lines = div().flex().flex_col().gap(px(3.0));
        if testing {
            lines = lines.child(hint("Testing the connection…", &theme));
        }
        // 8 under the title, 3 more above a check's line.
        let lines_gap =
            if !testing && self.sql_card.report.as_ref().is_some_and(|r| !r.checks.is_empty()) { 11.0 } else { 8.0 };
        if let Some(report) = &self.sql_card.report {
            for check in &report.checks {
                let (glyph, color) = match check.outcome {
                    CheckOutcome::Passed => (icons::CHECK, c.success_text),
                    CheckOutcome::Warning => (icons::WARNING, c.log_warn),
                    CheckOutcome::Failed => ("\u{E711}", c.error_text),
                };
                lines = lines.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(icons::LABEL_GAP))
                        .child(ui::line_icon(glyph, icons::SIZE, color, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{}: ", check.name)))
                                .child(check.detail.clone()),
                        ),
                );
            }
        }
        let test_enabled = enabled && !testing;
        card(cx)
            .gap(px(lines_gap))
            .when(!enabled, |d| d.opacity(0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .flex_1()
                            .min_w_0()
                            .child(card_title("Database server"))
                            .child(hint(sql_summary(&self.saved), &theme)),
                    )
                    .child(
                        ui::button(
                            "sql-test",
                            if testing { "Testing…" } else { "Test" },
                            ButtonKind::Secondary,
                            test_enabled,
                            cx,
                        )
                        .min_w(px(80.0))
                        .tip("Test")
                        .when(test_enabled, |b| {
                            b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test_sql(cx)))
                        }),
                    ),
            )
            .child(lines)
            .into_any_element()
    }

    /// Tests Docker Desktop, its engine and the SQL Server container.
    pub(crate) fn test_docker(&mut self, cx: &mut Context<Self>) {
        let card = &mut self.docker_card;
        card.version += 1;
        let version = card.version;
        card.testing = true;
        card.setup_enabled = false;
        card.setup_hint = Some("Testing Docker…".into());
        card.engine_note = None;
        let status = self.services.docker_status.clone();
        let container = value(&self.saved, "docker.containerName");
        off_thread(
            cx,
            move || status(container),
            move |page, result, cx| {
                let card = &mut page.docker_card;
                if card.version != version {
                    return;
                }
                card.testing = false;
                match result {
                    Ok(d) => {
                        let missing = setup_missing(&d);
                        card.setup_enabled = missing.is_none();
                        card.setup_hint = missing.map(String::from);
                        card.status = Some(d);
                    }
                    Err(e) => {
                        // Not known: not held back - Set up says itself what is missing.
                        card.setup_enabled = true;
                        card.setup_hint = None;
                        cx.emit(SettingsEvent::Error(format!("Could not test Docker: {e}")));
                    }
                }
            },
        );
        cx.notify();
    }

    /// Starts Docker Desktop, then looks for the engine every 5 seconds for up to two minutes.
    fn start_docker(&mut self, cx: &mut Context<Self>) {
        let start = self.services.start_docker.clone();
        off_thread(
            cx,
            move || start(()),
            |page, started, cx| {
                if let Err(e) = started {
                    cx.emit(SettingsEvent::Error(e));
                    return;
                }
                page.docker_card.starting = true;
                page.docker_card.engine_note =
                    Some(line("Starting…", Tone::Warning, "Docker Desktop is starting the engine."));
                let status = page.services.docker_status.clone();
                let container = value(&page.saved, "docker.containerName");
                cx.spawn(async move |this, cx| {
                    for _ in 0..24 {
                        cx.background_executor().timer(ENGINE_POLL).await;
                        let (status, container) = (status.clone(), container.clone());
                        let running = cx
                            .background_executor()
                            .spawn(async move { status(container).is_ok_and(|d| d.engine_running) })
                            .await;
                        if running {
                            let _ = this.update(cx, |page, cx| {
                                page.docker_card.starting = false;
                                page.test_docker(cx);
                                page.test_sql(cx);
                            });
                            return;
                        }
                    }
                    let _ = this.update(cx, |page, cx| {
                        page.docker_card.starting = false;
                        page.docker_card.engine_note = Some(line(
                            "Stopped",
                            Tone::Bad,
                            "The engine didn't start within two minutes - check Docker Desktop.",
                        ));
                        cx.notify();
                    });
                })
                .detach();
            },
        );
    }

    fn toggle_yaml(&mut self, cx: &mut Context<Self>) {
        if self.docker_card.yaml.take().is_none() {
            let options = docker_options(&self.saved, &self.saved_sa);
            self.docker_card.yaml = Some((self.services.render_compose)(options));
        }
        cx.notify();
    }

    fn copy_yaml(&mut self, cx: &mut Context<Self>) {
        let Some(yaml) = self.docker_card.yaml.clone() else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(yaml));
        self.docker_card.copied = true;
        cx.emit(SettingsEvent::Toast(Notice::Success, "docker-compose.yml copied to the clipboard.".into()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            let _ = this.update(cx, |page, cx| {
                page.docker_card.copied = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_docker_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let enabled = !self.dirty;
        let state = &self.docker_card;
        let container = value(&self.saved, "docker.containerName");
        let untested = line("Not tested", Tone::Muted, "");
        let mut lines = match &state.status {
            Some(d) => docker_lines(d, &container),
            None => [untested.clone(), untested.clone(), untested],
        };
        if state.testing {
            for l in &mut lines {
                l.tone = Tone::Muted;
            }
        }
        if let Some(note) = &state.engine_note {
            lines[1] = note.clone();
        }
        let installed = state.status.as_ref().map(|d| d.desktop_installed);
        let running = state.status.as_ref().is_some_and(|d| d.engine_running);
        let names = ["Docker Desktop", "Docker engine", "SQL Server container"];
        let mut rows = div().flex().flex_col();
        for (i, (name, l)) in names.into_iter().zip(lines).enumerate() {
            let action: Option<(&'static str, bool)> = match i {
                0 if installed == Some(false) => Some(("Install Docker Desktop", true)),
                1 if installed == Some(true) && !running => Some(("Start Docker Desktop", !state.starting)),
                _ => None,
            };
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .pad(px(8.0), px(0.0))
                    .when(i > 0, |d| d.border_t_1().border_color(c.grid_line))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .child(div().w(px(170.0)).flex_none().child(name))
                            .child(
                                div()
                                    .w(px(110.0))
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(l.tone.color(cx))
                                    .child(l.status),
                            )
                            .child(div().flex_1().min_w_0().text_color(c.text_muted).child(l.detail)),
                    )
                    .when_some(action, |d, (label, on)| {
                        let on = on && enabled;
                        d.child(
                            ui::button(
                                SharedString::from(format!("docker-action-{i}")),
                                label,
                                ButtonKind::Secondary,
                                on,
                                cx,
                            )
                            .tip(label)
                            .when(on, |b| {
                                b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if i == 0 {
                                        cx.emit(SettingsEvent::InstallDocker);
                                    } else {
                                        this.start_docker(cx);
                                    }
                                }))
                            }),
                        )
                    }),
            );
        }

        let setup = state.setup_enabled && enabled;
        let setup_tip = state.setup_hint.clone().unwrap_or_else(|| "Set up docker-compose".into());
        let yaml_shown = state.yaml.is_some();
        let testing = state.testing;
        let test_enabled = enabled && !testing;
        card(cx)
            .gap(px(8.0))
            .when(!enabled, |d| d.opacity(0.5))
            .child(
                div().flex().items_center().child(div().flex_1().child(card_title("Docker"))).child(
                    ui::button(
                        "docker-test",
                        if testing { "Testing…" } else { "Test" },
                        ButtonKind::Secondary,
                        test_enabled,
                        cx,
                    )
                    .min_w(px(80.0))
                    .tip("Test")
                    .when(test_enabled, |b| {
                        b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test_docker(cx)))
                    }),
                ),
            )
            .child(self.docker_body(rows, setup, setup_tip, yaml_shown, cx))
            .into_any_element()
    }

    /// The Docker card under its title: the three rows, the buttons, docker-compose.yml when shown - 10 apart.
    fn docker_body(&self, rows: Div, setup: bool, setup_tip: String, yaml_shown: bool, cx: &mut Context<Self>) -> Div {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let state = &self.docker_card;
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(rows)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        ui::button("setup-compose", "Set up docker-compose", ButtonKind::Primary, setup, cx)
                            .tip(setup_tip)
                            .when(setup, |b| {
                                b.on_click(
                                    cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::SetUpContainer)),
                                )
                            }),
                    )
                    .child({
                        let label = if yaml_shown { "Hide docker-compose.yml" } else { "Show docker-compose.yml" };
                        ui::button("show-yaml", label, ButtonKind::Secondary, true, cx)
                            .tip(label)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_yaml(cx)))
                    })
                    .when_some(state.setup_hint.clone(), |d, text| d.child(hint(text, &theme))),
            )
            .when_some(state.yaml.clone(), |d, yaml| {
                let copied = self.docker_card.copied;
                d.child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id("docker-compose-yml")
                                .max_h(px(380.0))
                                .overflow_y_scroll()
                                .p(px(8.0))
                                .rounded(theme.metrics.control_radius)
                                .border_1()
                                .border_color(c.field_border)
                                .bg(c.input_bg)
                                .font_family(theme.mono_font.clone())
                                .text_size(theme.text.caption)
                                .children(
                                    yaml.lines()
                                        .map(|l| div().whitespace_nowrap().child(l.to_string()))
                                        .collect::<Vec<_>>(),
                                ),
                        )
                        .child(
                            div().absolute().top(px(6.0)).right(px(24.0)).child(
                                div()
                                    .id("copy-yaml")
                                    .flex()
                                    .items_center()
                                    .gap(px(icons::LABEL_GAP))
                                    .pad(px(3.0), px(10.0))
                                    .rounded(theme.metrics.control_radius)
                                    .border_1()
                                    .border_color(c.card_border)
                                    .bg(c.card_bg)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(c.hover_bg))
                                    .tip("Copy")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_yaml(cx)))
                                    .child(ui::icon("\u{E8C8}", icons::SIZE, c.text_primary, cx))
                                    .child(if copied { "Copied" } else { "Copy" }),
                            ),
                        ),
                )
            })
    }

    /// Tests the IIS Windows features (one PowerShell call - takes a few seconds).
    pub(crate) fn test_iis(&mut self, cx: &mut Context<Self>) {
        self.iis_card.version += 1;
        let version = self.iis_card.version;
        self.iis_card.testing = true;
        let features = iis_features(&self.saved);
        let read = self.services.iis_feature_states.clone();
        off_thread(
            cx,
            move || read(features),
            move |page, states, _| {
                if page.iis_card.version == version {
                    page.iis_card.testing = false;
                    page.iis_card.states = Some(states);
                }
            },
        );
        cx.notify();
    }

    pub(crate) fn render_iis_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let features = iis_features(&self.saved);
        let rows = iis_rows(&features, self.iis_card.states.as_ref());
        let testing = self.iis_card.testing;
        let summary = iis_summary(&rows, self.iis_card.states.is_some(), testing);
        let columns = |feature: Div, status: Div, name: Div| {
            div()
                .flex()
                .items_center()
                .gap(px(24.0))
                .child(feature.w(px(220.0)).flex_none())
                .child(status.w(px(110.0)).flex_none())
                .child(name.flex_1().min_w_0())
        };
        let mut table =
            div().flex().flex_col().rounded(theme.metrics.control_radius).border_1().border_color(c.card_border).child(
                div().pad(px(7.0), px(12.0)).bg(c.header_bg).border_b_1().border_color(c.card_border).child(columns(
                    div().font_weight(FontWeight::SEMIBOLD).text_color(c.text_muted).child("Feature"),
                    div().font_weight(FontWeight::SEMIBOLD).text_color(c.text_muted).child("Status"),
                    div().font_weight(FontWeight::SEMIBOLD).text_color(c.text_muted).child("Windows feature name"),
                )),
            );
        for (i, (label, status, name)) in rows.into_iter().enumerate() {
            let (color, bold) = match status {
                "Enabled" => (c.success_text, true),
                "Missing" => (c.error_text, true),
                _ => (c.text_muted, false),
            };
            table = table.child(
                div().pad(px(6.0), px(12.0)).when(i > 0, |d| d.border_t_1().border_color(c.grid_line)).child(columns(
                    div().child(label),
                    div().text_color(color).when(bold, |d| d.font_weight(FontWeight::SEMIBOLD)).child(status),
                    div()
                        .text_color(c.text_muted)
                        .font_family(theme.mono_font.clone())
                        .text_size(theme.text.caption)
                        .child(name),
                )),
            );
        }
        card(cx)
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .flex_1()
                            .min_w_0()
                            .child(card_title("IIS"))
                            .child(hint(summary, &theme)),
                    )
                    .child(
                        ui::button("iis-edit", "Edit…", ButtonKind::Secondary, true, cx)
                            .min_w(px(80.0))
                            .tip("Add or remove Windows features")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                cx.emit(SettingsEvent::EditIisFeatures(iis_features(&this.saved)))
                            })),
                    )
                    .child(
                        ui::button(
                            "iis-test",
                            if testing { "Testing…" } else { "Test" },
                            ButtonKind::Secondary,
                            !testing,
                            cx,
                        )
                        .min_w(px(80.0))
                        .tip("Test")
                        .when(!testing, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test_iis(cx)))),
                    ),
            )
            .child(
                div().flex().flex_col().gap(px(10.0)).child(table).child(
                    div().flex().child(
                        ui::button("iis-setup", "Set up IIS", ButtonKind::Primary, true, cx)
                            .tip("Set up IIS")
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::SetUpIis))),
                    ),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_the_server_in_words_never_its_password() {
        let mut s = Settings::default();
        s.set("sqlServer.saPassword", "Secret-123");
        assert_eq!(sql_summary(&s), "Local SQL container (Docker) at localhost,1433, as sa.");
        s.set("sqlServer.type", "sqlServer");
        assert_eq!(sql_summary(&s), r".\SQLEXPRESS, with windows authentication.");
        s.set("sqlServer.authentication", "sql");
        s.set("sqlServer.userName", "dnn");
        assert_eq!(sql_summary(&s), r".\SQLEXPRESS, with sql server authentication as 'dnn'.");
        s.set("sqlServer.type", "localDbFile");
        s.set("sqlServer.server", r"(LocalDB)\MSSQLLocalDB");
        assert_eq!(
            sql_summary(&s),
            r"SQL Server Express LocalDB ((LocalDB)\MSSQLLocalDB) - each site's own database file."
        );
        assert!(!sql_summary(&s).contains("Secret-123"));
    }

    #[test]
    fn docker_lines_say_what_is_missing() {
        let none = DockerStatus::default();
        let lines = docker_lines(&none, "dnn-sqlserver");
        assert_eq!((lines[0].status.as_str(), lines[0].tone), ("Not installed", Tone::Bad));
        assert_eq!(lines[1].status, "-");
        assert_eq!(lines[2].detail, "'dnn-sqlserver' - needs the Docker engine.");
        assert_eq!(setup_missing(&none), Some("Needs Docker Desktop - install it above."));

        let stopped =
            DockerStatus { desktop_installed: true, client_version: Some("29.1".into()), ..Default::default() };
        let lines = docker_lines(&stopped, "dnn-sqlserver");
        assert_eq!(lines[0].detail, "docker 29.1");
        assert_eq!(lines[1].status, "Stopped");
        assert_eq!(setup_missing(&stopped), Some("Needs the Docker engine running - start Docker Desktop above."));

        let up = DockerStatus {
            desktop_installed: true,
            engine_running: true,
            engine_version: Some("29.1".into()),
            container_state: Some("exited".into()),
            container_status: Some("Exited (0) 2 hours ago".into()),
            ..Default::default()
        };
        let lines = docker_lines(&up, "dnn-sqlserver");
        assert_eq!(lines[1].detail, "engine 29.1");
        assert_eq!(
            lines[2].detail,
            "'dnn-sqlserver' - Exited (0) 2 hours ago. Start it with Set up docker-compose below."
        );
        assert_eq!(setup_missing(&up), None);
        let no_container = DockerStatus { container_state: None, ..up };
        assert_eq!(docker_lines(&no_container, "x")[2].status, "Not created");
    }

    #[test]
    fn iis_summary_counts_what_is_missing() {
        let features = vec![
            IisFeature { name: "A".into(), label: "Alpha".into() },
            IisFeature { name: "B".into(), label: "Beta".into() },
        ];
        let untested = iis_rows(&features, None);
        assert_eq!(iis_summary(&untested, false, false), "Windows features: not tested yet - press Test.");
        assert_eq!(iis_summary(&untested, false, true), "Testing…");
        let states = BTreeMap::from([("A".to_string(), true), ("B".to_string(), false)]);
        assert_eq!(
            iis_summary(&iis_rows(&features, Some(&states)), true, false),
            "1 of 2 Windows features enabled - 1 missing."
        );
        let all = BTreeMap::from([("A".to_string(), true), ("B".to_string(), true)]);
        assert_eq!(iis_summary(&iis_rows(&features, Some(&all)), true, false), "All 2 Windows features are enabled.");
        assert_eq!(
            iis_summary(&iis_rows(&features, Some(&BTreeMap::new())), true, false),
            "Couldn't read the Windows features - DNN Manager needs administrator rights."
        );
    }
}
