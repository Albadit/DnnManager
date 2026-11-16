//! Upgrade DNN (`Controls/UpgradeDnnDialog.xaml(.cs)`): asks which newer DNN a project is upgraded to - only releases
//! above its DNN are offered, the latest release chosen - then shows the plan DNN's upgrade path makes of it: every
//! step in between, each with what the analyser checked (Compatible, Warning, Blocking, Unknown). The upgrade starts
//! only when nothing blocks.
//!
//! The versions and the plan come through the app (they read GitHub, the site, its database and this PC): the dialog
//! runs them on the background executor. Emits `DialogEvent::Done(UpgradeChoice)`; the app then runs
//! `dnnm_wiring::upgrade_dnn::upgrade_dnn` with an `UpgradeDnnRequest` titled `texts::operation_title(site,
//! choice.version)`, and shows `texts::done` when it finishes.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, Pixels,
    SharedString, Window, div, prelude::*, px,
};

use dnnm_application::use_cases::upgrade_dnn::{DnnNumber, Finding, Severity, StepMethod, UpgradePlan};
use dnnm_domain::{DnnRelease, DnnReleaseList};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tooltip};

use crate::markdown::stack_with_gaps;

/// The DNN a project is to be upgraded to (`UpgradeChoice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeChoice {
    pub release_api_url: String,
    /// The release's tag (`v10.3.3`) - the request's version.
    pub tag: String,
    /// The release's version (`10.3.3`) - for the operation's title and the toast.
    pub version: String,
}

/// The releases of a repository (`DnnReleaseCatalog.GetAsync`) - called on the background executor.
pub type LoadReleases = Arc<dyn Fn(&str) -> Result<DnnReleaseList, String> + Send + Sync>;
/// The plan to a release's version (`UpgradeDnnUseCase.PlanAsync`, e.g. `dnnm_wiring::upgrade_dnn::plan_upgrade`) -
/// called on the background executor; it may ask to sign in to the database's server.
pub type PlanUpgrade = Arc<dyn Fn(&str) -> Result<UpgradePlan, String> + Send + Sync>;

/// The text at the top.
pub fn intro(site: &str, current: &str) -> String {
    format!(
        "Upgrades '{site}' from DNN {current} as DNN's suggested upgrade path says - through every version it lists on \
         the way, never straight to the newest. Before each step the site and its database are backed up; after it the \
         site is restarted and checked. A step that fails puts the site back to the version before it."
    )
}

/// `https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases` → `dnnsoftware/Dnn.Platform`.
pub fn repository_label(api: &str) -> String {
    let Some(rest) = api.split_once("://").map(|(_, r)| r) else { return api.to_string() };
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or_default();
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 && parts[0] == "repos" { format!("{}/{}", parts[1], parts[2]) } else { api.to_string() }
}

/// Only what is newer than the site's DNN - an upgrade never goes back - and no pre-release; the first marked latest.
pub fn newer_releases(releases: &[DnnRelease], current: Option<DnnNumber>) -> Vec<(DnnRelease, String)> {
    let newer: Vec<&DnnRelease> = releases
        .iter()
        .filter(|r| !r.prerelease && DnnNumber::of(&r.version).is_some_and(|n| current.is_none_or(|c| n > c)))
        .collect();
    newer
        .iter()
        .enumerate()
        .map(|(i, r)| ((*r).clone(), if i == 0 { format!("{}  (latest)", r.version) } else { r.version.clone() }))
        .collect()
}

/// The hint when the releases come from the list saved for offline use.
pub fn offline_hint(saved_at: &str) -> String {
    // Saved as a round-trip time: shown to the minute.
    let shown: String = saved_at.replace('T', " ").chars().take(16).collect();
    format!(
        "GitHub can't be reached - the releases as of {shown}. The upgrade downloads each step's package, so it needs internet."
    )
}

/// What a line of the plan is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Heading,
    Note,
    Finding(Severity),
}

/// A line of the plan as shown (`PlanLine`): its mark, its text, its tooltip (a finding's fix) and the room above it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanLine {
    pub kind: LineKind,
    pub mark: String,
    pub text: String,
    pub tip: Option<String>,
    pub space_above: f32,
}

fn method(method: StepMethod) -> &'static str {
    match method {
        StepMethod::LocalUpgrade => "DNN's local upgrade, from the install package",
        StepMethod::Manual => "by hand",
        StepMethod::UpgradePackage => "upgrade package",
    }
}

fn heading(text: String, space_above: f32) -> PlanLine {
    PlanLine { kind: LineKind::Heading, mark: String::new(), text, tip: None, space_above }
}

fn finding_line(f: &Finding) -> PlanLine {
    let mut text = format!("{}: {}", f.area, f.title);
    if let Some(detail) = &f.detail {
        text.push_str(&format!(" - {detail}"));
    }
    if let Some(fix) = &f.fix {
        text.push_str(&format!(" → {fix}"));
    }
    PlanLine {
        kind: LineKind::Finding(f.severity),
        mark: f.severity.to_string(),
        text,
        tip: f.fix.clone(),
        space_above: 1.0,
    }
}

/// The plan's lines: the path, the site's findings, then each step's checks (`Lines`).
pub fn plan_lines(plan: &UpgradePlan) -> Vec<PlanLine> {
    let mut lines =
        vec![heading(format!("DNN {} → {}: the required path", plan.current.name(), plan.target.name()), 0.0)];
    lines.extend(plan.steps.iter().enumerate().map(|(i, s)| PlanLine {
        kind: LineKind::Note,
        mark: format!("Step {}", i + 1),
        text: format!("{s}  ({})", method(s.method)),
        tip: None,
        space_above: 1.0,
    }));
    lines.push(heading("The site".into(), 12.0));
    lines.extend(plan.site.iter().map(finding_line));
    for (i, step) in plan.steps.iter().enumerate() {
        lines.push(heading(format!("Step {} - {step}", i + 1), 12.0));
        lines.extend(step.checks.iter().map(finding_line));
    }
    lines
}

/// The room between the plan's lines: a line's `space_above`, at least what the one before keeps under it (4 under a
/// heading, 1 under any other line).
fn plan_gaps(lines: &[PlanLine]) -> Vec<Pixels> {
    lines
        .windows(2)
        .map(|w| {
            let under = if w[0].kind == LineKind::Heading { 4.0 } else { 1.0 };
            px(w[1].space_above.max(under))
        })
        .collect()
}

/// What the dialog says about a plan: the problem (blocking findings) and the hint (the steps).
pub fn plan_summary(plan: &UpgradePlan) -> (Option<String>, String) {
    let blocking = plan.blocking().len();
    let problem = (blocking > 0).then(|| {
        format!("{blocking} blocking problem(s) - the upgrade can't start until they're solved (see the plan).")
    });
    let path: Vec<String> =
        std::iter::once(plan.current.name()).chain(plan.steps.iter().map(|s| s.to.name())).collect();
    (problem, format!("{} step(s): {}", plan.steps.len(), path.join(" → ")))
}

/// Which list is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    None,
    Repository,
    Version,
}

pub struct UpgradeDnnDialog {
    site: String,
    /// The site's DNN as its row shows it.
    current_text: String,
    current: Option<DnnNumber>,
    sources: Vec<(String, String)>,
    source: Option<usize>,
    options: Vec<(DnnRelease, String)>,
    version: Option<usize>,
    open: Open,
    hint: String,
    problem: Option<String>,
    plan: Option<UpgradePlan>,
    lines: Vec<PlanLine>,
    loading: u64,
    planning: u64,
    load: LoadReleases,
    plan_upgrade: PlanUpgrade,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent<UpgradeChoice>> for UpgradeDnnDialog {}

impl UpgradeDnnDialog {
    /// `current`: the site's DNN as its row shows it; `release_apis`: `dnnm_wiring::upgrade_dnn::release_apis`.
    pub fn new(
        site: &str,
        current: &str,
        release_apis: Vec<String>,
        load: LoadReleases,
        plan_upgrade: PlanUpgrade,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let sources: Vec<(String, String)> =
            release_apis.into_iter().map(|api| (repository_label(&api), api)).collect();
        let mut dialog = Self {
            site: site.to_string(),
            current_text: current.to_string(),
            current: DnnNumber::of(current),
            sources,
            source: None,
            options: Vec::new(),
            version: None,
            open: Open::None,
            hint: String::new(),
            problem: None,
            plan: None,
            lines: Vec::new(),
            loading: 0,
            planning: 0,
            load,
            plan_upgrade,
            focus,
        };
        if dialog.sources.is_empty() {
            dialog.show(Some("No DNN repository is set (Settings → DNN releases).".into()), None);
        } else {
            dialog.choose_repository(0, cx);
        }
        dialog
    }

    fn show(&mut self, problem: Option<String>, hint: Option<String>) {
        self.problem = problem;
        if let Some(hint) = hint {
            self.hint = hint;
        }
    }

    fn can_start(&self) -> bool {
        self.problem.is_none() && self.plan.as_ref().is_some_and(UpgradePlan::can_start)
    }

    fn choose_repository(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((label, api)) = self.sources.get(index).cloned() else { return };
        self.source = Some(index);
        self.loading += 1;
        let loading = self.loading;
        self.options.clear();
        self.version = None;
        self.plan = None;
        self.lines.clear();
        self.show(None, Some("Loading the versions…".into()));
        let load = Arc::clone(&self.load);
        let task = cx.background_executor().spawn(async move { load(&api) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if loading != this.loading {
                    return;
                }
                match result {
                    Err(e) => this.show(Some(format!("Could not load the versions of {label}: {e}")), None),
                    Ok(list) => {
                        let newer = newer_releases(&list.releases, this.current);
                        if newer.is_empty() {
                            this.show(
                                None,
                                Some(format!(
                                    "The site runs the newest DNN release of {label} - there is nothing newer to upgrade to."
                                )),
                            );
                        } else {
                            this.options = newer;
                            this.choose_version(0, cx);
                            if let Some(saved_at) = &list.saved_at {
                                this.show(None, Some(offline_hint(saved_at)));
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A version chosen: the site is analysed and the plan to it shown - off the UI thread.
    fn choose_version(&mut self, index: usize, cx: &mut Context<Self>) {
        self.plan = None;
        let Some((release, _)) = self.options.get(index).cloned() else { return };
        self.version = Some(index);
        if DnnNumber::of(&release.version).is_none() {
            return;
        }
        self.planning += 1;
        let planning = self.planning;
        self.lines = vec![heading("Analysing the site, its database, IIS and this PC…".into(), 0.0)];
        let plan = Arc::clone(&self.plan_upgrade);
        let version = release.version.clone();
        let task = cx.background_executor().spawn(async move { plan(&version) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if planning != this.planning {
                    return;
                }
                match result {
                    Err(e) => this.show(Some(format!("The site couldn't be analysed: {e}")), None),
                    Ok(plan) => {
                        this.lines = plan_lines(&plan);
                        let (problem, hint) = plan_summary(&plan);
                        this.plan = Some(plan);
                        this.show(problem, Some(hint));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_start() {
            return;
        }
        let (Some(source), Some(version)) = (self.source, self.version) else { return };
        let (Some((_, api)), Some((release, _))) = (self.sources.get(source), self.options.get(version)) else {
            return;
        };
        cx.emit(DialogEvent::Done(UpgradeChoice {
            release_api_url: api.clone(),
            tag: release.tag_name.clone(),
            version: release.version.clone(),
        }));
    }

    fn line(&self, i: usize, line: &PlanLine, cx: &App) -> AnyElement {
        let theme = Theme::get(cx);
        let c = theme.colors;
        let mark_color = match line.kind {
            LineKind::Finding(Severity::Compatible) => c.success_text,
            LineKind::Finding(Severity::Warning) => c.log_warn,
            LineKind::Finding(Severity::Blocking) => c.error_text,
            _ => c.text_muted,
        };
        let text_color = if line.kind == LineKind::Finding(Severity::Blocking) { c.error_text } else { c.text_primary };
        div()
            .id(("plan-line", i))
            .w_full()
            .flex()
            .when(line.kind != LineKind::Heading, |d| {
                d.child(
                    div()
                        .w(px(74.0))
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(mark_color)
                        .child(line.mark.clone()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(text_color)
                    .when(line.kind == LineKind::Heading, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(line.text.clone()),
            )
            .when_some(line.tip.clone(), |d, tip| d.tooltip(Tooltip::text(tip)))
            .into_any_element()
    }
}

impl Focusable for UpgradeDnnDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for UpgradeDnnDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (c, radius) = {
            let theme = Theme::get(cx);
            (theme.colors, theme.metrics.control_radius)
        };
        let ok = self.can_start();

        let repository = self.source.and_then(|i| self.sources.get(i)).map(|s| s.0.clone()).unwrap_or_default();
        let repositories: Vec<AnyElement> = self
            .sources
            .iter()
            .enumerate()
            .map(|(i, (label, _))| {
                dialog::option(("repository", i), label.clone(), Some(i) == self.source, cx)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.open = Open::None;
                        cx.stop_propagation();
                        if Some(i) != this.source {
                            this.choose_repository(i, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let version = self.version.and_then(|i| self.options.get(i)).map(|o| o.1.clone()).unwrap_or_default();
        let versions: Vec<AnyElement> = self
            .options
            .iter()
            .enumerate()
            .map(|(i, (_, label))| {
                dialog::option(("version", i), label.clone(), Some(i) == self.version, cx)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.open = Open::None;
                        cx.stop_propagation();
                        if Some(i) != this.version {
                            this.choose_version(i, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let toggle = |which: Open| {
            cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                this.open = if this.open == which { Open::None } else { which };
                cx.notify();
            })
        };

        let current = self.current_text.clone();
        let lines: Vec<AnyElement> = self.lines.iter().enumerate().map(|(i, l)| self.line(i, l, cx)).collect();
        let stack = |gap: f32| div().w_full().flex().flex_col().gap(px(gap));
        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(div().w_full().child(intro(&self.site, &current)))
            .when(self.sources.len() > 1, |d| {
                d.child(stack(4.0).child(dialog::label("Repository", cx)).child(
                    div().id("repository-box").on_click(toggle(Open::Repository)).child(dialog::select(
                        "repository",
                        repository,
                        self.open == Open::Repository,
                        repositories,
                        cx,
                    )),
                ))
            })
            .child(
                stack(14.0)
                    .child(
                        stack(4.0)
                            .child(dialog::label("Upgrade to", cx))
                            .child(div().id("version-box").on_click(toggle(Open::Version)).child(dialog::select(
                                "version",
                                version,
                                self.open == Open::Version,
                                versions,
                                cx,
                            )))
                            .child(dialog::hint(self.hint.clone(), cx)),
                    )
                    .child(
                        stack(10.0)
                            // The plan: the steps, and what the analyser found - Compatible, Warning, Blocking, Unknown.
                            .child(
                                div()
                                    .pad(px(10.0), px(12.0))
                                    .rounded(radius)
                                    .border_1()
                                    .border_color(c.card_border)
                                    .bg(c.card_bg)
                                    .child(stack_with_gaps(lines, plan_gaps(&self.lines))),
                            )
                            .children(dialog::field_error(self.problem.clone().map(SharedString::from), cx)),
                    ),
            );

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Start upgrade", ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Back up and upgrade, step by step (Enter)"))
                    .when(ok, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Cancel (Esc)"))
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.submit(cx);
                }
            }))
            .child(dialog::frame("Upgrade DNN", 700.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dnnm_application::use_cases::upgrade_dnn::PlanStep;

    fn release(version: &str, prerelease: bool) -> DnnRelease {
        DnnRelease {
            version: version.into(),
            tag_name: format!("v{version}"),
            download_url: String::new(),
            prerelease,
            sha256: None,
            upgrade_sha256: None,
        }
    }

    #[test]
    fn only_newer_releases_are_offered_the_first_as_latest() {
        let releases =
            [release("10.4.0-rc1", true), release("10.3.3", false), release("10.2.5", false), release("9.13.9", false)];
        let offered = newer_releases(&releases, DnnNumber::of("9.13.9"));
        let labels: Vec<&str> = offered.iter().map(|o| o.1.as_str()).collect();
        assert_eq!(labels, ["10.3.3  (latest)", "10.2.5"]);
        assert!(newer_releases(&releases, DnnNumber::of("10.3.3")).is_empty());
        assert_eq!(newer_releases(&releases, None).len(), 3, "an unknown DNN gets every release");
    }

    #[test]
    fn repositories_are_named_by_owner_and_name() {
        assert_eq!(
            repository_label("https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases"),
            "dnnsoftware/Dnn.Platform"
        );
        assert_eq!(repository_label("not a url"), "not a url");
        assert_eq!(repository_label("https://example.com/x"), "https://example.com/x");
    }

    fn plan(blocking: bool) -> UpgradePlan {
        let mut check = Finding::new(Severity::Warning, "Extensions", "Built against DNN 8");
        check.detail = Some("Shop.dll".into());
        let mut site = vec![Finding::new(Severity::Compatible, "IIS", "App pool on CLR v4.0")];
        if blocking {
            let mut f = Finding::new(Severity::Blocking, ".NET Framework", "Too old");
            f.fix = Some("Install .NET Framework 4.8".into());
            site.push(f);
        }
        UpgradePlan {
            current: DnnNumber::new(9, 13, 9),
            target: DnnNumber::new(10, 3, 3),
            steps: vec![
                PlanStep {
                    from: DnnNumber::new(9, 13, 9),
                    to: DnnNumber::new(10, 2, 5),
                    method: StepMethod::UpgradePackage,
                    checks: vec![check],
                },
                PlanStep {
                    from: DnnNumber::new(10, 2, 5),
                    to: DnnNumber::new(10, 3, 3),
                    method: StepMethod::LocalUpgrade,
                    checks: vec![],
                },
            ],
            site,
            description: String::new(),
        }
    }

    #[test]
    fn the_plan_is_shown_as_the_dialog_lists_it() {
        let lines = plan_lines(&plan(true));
        let texts: Vec<(&str, &str)> = lines.iter().map(|l| (l.mark.as_str(), l.text.as_str())).collect();
        assert_eq!(
            texts,
            [
                ("", "DNN 09.13.09 → 10.03.03: the required path"),
                ("Step 1", "09.13.09 → 10.02.05  (upgrade package)"),
                ("Step 2", "10.02.05 → 10.03.03  (DNN's local upgrade, from the install package)"),
                ("", "The site"),
                ("Compatible", "IIS: App pool on CLR v4.0"),
                ("Blocking", ".NET Framework: Too old → Install .NET Framework 4.8"),
                ("", "Step 1 - 09.13.09 → 10.02.05"),
                ("Warning", "Extensions: Built against DNN 8 - Shop.dll"),
                ("", "Step 2 - 10.02.05 → 10.03.03"),
            ]
        );
        assert_eq!(lines[5].tip.as_deref(), Some("Install .NET Framework 4.8"));
        assert_eq!(
            plan_summary(&plan(true)),
            (
                Some("1 blocking problem(s) - the upgrade can't start until they're solved (see the plan).".into()),
                "2 step(s): 09.13.09 → 10.02.05 → 10.03.03".into()
            )
        );
        assert_eq!(plan_summary(&plan(false)).0, None);
        assert!(
            intro("shop", "9.13.9").starts_with("Upgrades 'shop' from DNN 9.13.9 as DNN's suggested upgrade path says")
        );
        assert_eq!(
            offline_hint("2026-11-16T12:30:05.0000000Z"),
            "GitHub can't be reached - the releases as of 2026-11-16 12:30. The upgrade downloads each step's package, so it needs internet."
        );
    }
}
