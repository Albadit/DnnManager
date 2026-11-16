//! Edit app pool (`Controls/AppPoolDialog.xaml(.cs)`): a site's app pool - its .NET runtime, pipeline, 32-bit mode,
//! identity, idle time-out and start mode. A specific user account is kept as it is - its password isn't known here.
//! Emits `DialogEvent::Done(EditAppPoolRequest)`; the app runs `dnnm_wiring::edit_app_pool::edit_app_pool` with it.
//!
//! The app gives it the pool's name and settings from `dnnm_wiring::edit_app_pool::current_pool`.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString,
    Subscription, Window, div, prelude::*, px,
};

use dnnm_application::iis::IisPoolSettings;
use dnnm_application::use_cases::edit_app_pool::EditAppPoolRequest;
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Tooltip};

/// IIS takes an idle time-out of up to 30 days.
pub const MAX_IDLE_MINUTES: i64 = 43_200;

const IDENTITIES: [&str; 4] = ["ApplicationPoolIdentity", "NetworkService", "LocalService", "LocalSystem"];

/// The dialog's first line.
pub fn intro(site: &str, pool: &str) -> String {
    format!("The settings of '{pool}', the app pool of '{site}'. Saving recycles the pool.")
}

/// A list's choices - (text shown, value meant) - and the one selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    pub items: Vec<(String, String)>,
    pub selected: usize,
}

impl Choices {
    /// Offers `offered` and selects `current` - added as a choice of its own when IIS has a value that isn't among
    /// them, so opening and saving the dialog doesn't change it (`Fill`).
    pub fn new(current: &str, offered: &[(&str, &str)]) -> Self {
        let mut items: Vec<(String, String)> = offered.iter().map(|(t, v)| (t.to_string(), v.to_string())).collect();
        let selected = match items.iter().position(|(_, v)| v.eq_ignore_ascii_case(current)) {
            Some(i) => i,
            None => {
                items.push((current.to_string(), current.to_string()));
                items.len() - 1
            }
        };
        Self { items, selected }
    }

    pub fn value(&self) -> &str {
        self.items.get(self.selected).map_or("", |(_, v)| v.as_str())
    }

    pub fn text(&self) -> &str {
        self.items.get(self.selected).map_or("", |(t, _)| t.as_str())
    }
}

/// The identity list, whether it can be changed, and the hint under it.
pub fn identity_choices(current: &str) -> (Choices, bool, &'static str) {
    if IDENTITIES.iter().any(|i| i.eq_ignore_ascii_case(current)) {
        let offered: Vec<(&str, &str)> = IDENTITIES.iter().map(|i| (*i, *i)).collect();
        (
            Choices::new(current, &offered),
            true,
            "Another identity needs access to the site's folder (granted when saved) and, with Windows authentication, to its database.",
        )
    } else {
        // A specific account needs its password to be set again, which this dialog doesn't ask for - so it stays.
        let text = if current.eq_ignore_ascii_case("SpecificUser") {
            "Specific user (kept)".to_string()
        } else {
            format!("{current} (kept)")
        };
        (Choices::new(current, &[(&text, current)]), false, "A specific user account is changed in IIS Manager.")
    }
}

/// The idle time-out box's text for `idle`: whole minutes, rounded half to even as .NET's `Math.Round`.
pub fn idle_text(idle: Duration) -> String {
    ((idle.as_secs_f64() / 60.0).round_ties_even() as i64).to_string()
}

/// The idle time-out typed; None when it isn't a whole number of minutes in range. Left as it was, it is the current
/// value exactly - which can have seconds the box rounds away.
pub fn idle_timeout(typed: &str, current: Duration) -> Option<Duration> {
    let text = typed.trim();
    if text == idle_text(current) {
        return Some(current);
    }
    text.parse::<i64>().ok().filter(|m| (0..=MAX_IDLE_MINUTES).contains(m)).map(|m| Duration::from_secs(m as u64 * 60))
}

/// The red line under the form.
pub fn idle_problem(typed: &str, current: Duration) -> Option<String> {
    idle_timeout(typed, current).is_none().then(|| {
        format!("The idle time-out is a whole number of minutes from 0 (never) to {MAX_IDLE_MINUTES} (30 days).")
    })
}

/// Saving the settings as they are would recycle the pool for nothing.
pub fn changed(chosen: &IisPoolSettings, current: &IisPoolSettings) -> bool {
    let differ = |a: &str, b: &str| !a.eq_ignore_ascii_case(b);
    differ(&chosen.runtime, &current.runtime)
        || differ(&chosen.pipeline, &current.pipeline)
        || chosen.enable_32bit != current.enable_32bit
        || differ(&chosen.identity, &current.identity)
        || chosen.idle_timeout != current.idle_timeout
        || differ(&chosen.start_mode, &current.start_mode)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Runtime,
    Pipeline,
    Identity,
    StartMode,
}

pub struct AppPoolDialog {
    site: String,
    directory: PathBuf,
    pool: String,
    current: IisPoolSettings,
    runtime: Choices,
    pipeline: Choices,
    identity: Choices,
    identity_editable: bool,
    identity_hint: &'static str,
    start_mode: Choices,
    enable_32bit: bool,
    idle: Entity<TextInput>,
    open: Option<List>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent<EditAppPoolRequest>> for AppPoolDialog {}

impl AppPoolDialog {
    /// For the site `site` serving `directory`, whose app pool `pool` has the settings `current`.
    pub fn new(
        site: &str,
        directory: PathBuf,
        pool: &str,
        current: IisPoolSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let idle = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_text(idle_text(current.idle_timeout), cx);
            input
        });
        let subscription = cx.subscribe(&idle, |_, _, _: &TextChanged, cx| cx.notify());
        let (identity, identity_editable, identity_hint) = identity_choices(&current.identity);
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            site: site.to_string(),
            directory,
            pool: pool.to_string(),
            runtime: Choices::new(&current.runtime, &[("v4.0", "v4.0"), ("No Managed Code", "")]),
            pipeline: Choices::new(&current.pipeline, &[("Integrated", "Integrated"), ("Classic", "Classic")]),
            identity,
            identity_editable,
            identity_hint,
            start_mode: Choices::new(
                &current.start_mode,
                &[("OnDemand", "OnDemand"), ("AlwaysRunning", "AlwaysRunning")],
            ),
            enable_32bit: current.enable_32bit,
            current,
            idle,
            open: None,
            focus,
            _subscription: subscription,
        }
    }

    /// The settings as they are to be (`Chosen`).
    fn chosen(&self, cx: &App) -> IisPoolSettings {
        IisPoolSettings {
            runtime: self.runtime.value().to_string(),
            pipeline: self.pipeline.value().to_string(),
            enable_32bit: self.enable_32bit,
            identity: self.identity.value().to_string(),
            idle_timeout: idle_timeout(self.idle.read(cx).text(), self.current.idle_timeout)
                .unwrap_or(self.current.idle_timeout),
            start_mode: self.start_mode.value().to_string(),
        }
    }

    fn can_save(&self, cx: &App) -> bool {
        idle_problem(self.idle.read(cx).text(), self.current.idle_timeout).is_none()
            && changed(&self.chosen(cx), &self.current)
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        cx.emit(DialogEvent::Done(EditAppPoolRequest {
            site_name: self.site.clone(),
            directory: self.directory.clone(),
            settings: self.chosen(cx),
        }));
    }

    fn choices_mut(&mut self, list: List) -> &mut Choices {
        match list {
            List::Runtime => &mut self.runtime,
            List::Pipeline => &mut self.pipeline,
            List::Identity => &mut self.identity,
            List::StartMode => &mut self.start_mode,
        }
    }

    /// A list (`ComboBox`): its box opens and closes it; an option chooses and closes.
    fn list(&mut self, list: List, id: &'static str, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let open = self.open == Some(list);
        let choices = self.choices_mut(list).clone();
        let options = choices
            .items
            .iter()
            .enumerate()
            .map(|(i, (text, _))| {
                dialog::option(SharedString::from(format!("{id}-{i}")), text.clone(), i == choices.selected, cx)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.choices_mut(list).selected = i;
                        this.open = None;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .id(id)
            .when(!enabled, |d| d.opacity(0.5))
            .when(enabled, |d| {
                d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.open = if this.open == Some(list) { None } else { Some(list) };
                    cx.notify();
                }))
            })
            .child(dialog::select(
                SharedString::from(format!("{id}-box")),
                choices.text().to_string(),
                open,
                options,
                cx,
            ))
            .into_any_element()
    }
}

impl Focusable for AppPoolDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AppPoolDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let problem = idle_problem(self.idle.read(cx).text(), self.current.idle_timeout);
        let ok = self.can_save(cx);
        let runtime = self.list(List::Runtime, "runtime", true, cx);
        let pipeline = self.list(List::Pipeline, "pipeline", true, cx);
        let identity = self.list(List::Identity, "identity", self.identity_editable, cx);
        let start_mode = self.list(List::StartMode, "start-mode", true, cx);
        // A label 4 above its box; the rows 14 apart, the check box (4 above and under it built in) 10 from its
        // neighbours.
        let column = || div().flex_1().min_w_0().flex().flex_col().gap(px(4.0));
        let stack = |gap: f32| div().w_full().flex().flex_col().gap(px(gap));

        let body = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(div().w_full().child(intro(&self.site, &self.pool)))
            .child(
                stack(10.0)
                    .child(
                        div()
                            .flex()
                            .gap(px(12.0))
                            .child(column().child(dialog::label(".NET CLR version", cx)).child(runtime))
                            .child(column().child(dialog::label("Pipeline mode", cx)).child(pipeline)),
                    )
                    .child(dialog::check("enable-32bit", self.enable_32bit, "Enable 32-bit applications", cx).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.enable_32bit = !this.enable_32bit;
                            cx.notify();
                        }),
                    ))
                    .child(
                        stack(4.0)
                            .child(dialog::label("Identity", cx))
                            .child(identity)
                            .child(dialog::hint(self.identity_hint, cx)),
                    ),
            )
            .child(
                stack(10.0)
                    .child(
                        stack(4.0)
                            .child(
                                div()
                                    .flex()
                                    .gap(px(12.0))
                                    .child(
                                        column()
                                            .child(dialog::label("Idle time-out (minutes)", cx))
                                            .child(dialog::text_field(&self.idle, window, cx)),
                                    )
                                    .child(column().child(dialog::label("Start mode", cx)).child(start_mode)),
                            )
                            .child(dialog::hint("An idle time-out of 0 never shuts the pool down for being idle.", cx)),
                    )
                    .children(dialog::field_error(problem.map(SharedString::from), cx)),
            );

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Save", ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Save (Enter)"))
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
            .child(dialog::frame("Edit app pool", 460.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> IisPoolSettings {
        IisPoolSettings {
            runtime: "v4.0".into(),
            pipeline: "Integrated".into(),
            enable_32bit: false,
            identity: "ApplicationPoolIdentity".into(),
            idle_timeout: Duration::from_secs(20 * 60),
            start_mode: "OnDemand".into(),
        }
    }

    #[test]
    fn lists_select_the_current_value_or_add_it() {
        let runtime = Choices::new("", &[("v4.0", "v4.0"), ("No Managed Code", "")]);
        assert_eq!((runtime.text(), runtime.value()), ("No Managed Code", ""));
        let odd = Choices::new("v2.0", &[("v4.0", "v4.0"), ("No Managed Code", "")]);
        assert_eq!((odd.items.len(), odd.text(), odd.value()), (3, "v2.0", "v2.0"));
        let start = Choices::new("alwaysrunning", &[("OnDemand", "OnDemand"), ("AlwaysRunning", "AlwaysRunning")]);
        assert_eq!(start.value(), "AlwaysRunning");
    }

    #[test]
    fn a_specific_user_is_kept() {
        let (choices, editable, hint) = identity_choices("SpecificUser");
        assert_eq!((choices.text(), choices.value(), editable), ("Specific user (kept)", "SpecificUser", false));
        assert_eq!(hint, "A specific user account is changed in IIS Manager.");
        let (choices, editable, _) = identity_choices("Odd");
        assert_eq!((choices.text(), editable), ("Odd (kept)", false));
        let (choices, editable, hint) = identity_choices("networkservice");
        assert_eq!((choices.items.len(), choices.value(), editable), (4, "NetworkService", true));
        assert!(hint.starts_with("Another identity needs access"));
    }

    #[test]
    fn idle_time_out_in_whole_minutes() {
        let twenty = Duration::from_secs(20 * 60);
        assert_eq!(idle_text(Duration::from_secs(150)), "2", "2.5 rounds to even");
        assert_eq!(idle_text(Duration::from_secs(210)), "4", "3.5 rounds to even");
        // Left as it was: the exact value, seconds and all.
        assert_eq!(idle_timeout(" 2 ", Duration::from_secs(150)), Some(Duration::from_secs(150)));
        assert_eq!(idle_timeout("0", twenty), Some(Duration::ZERO));
        assert_eq!(idle_timeout("43200", twenty), Some(Duration::from_secs(43_200 * 60)));
        for bad in ["43201", "-1", "1.5", "", "x"] {
            assert_eq!(idle_timeout(bad, twenty), None, "{bad:?}");
        }
        assert_eq!(
            idle_problem("x", twenty).as_deref(),
            Some("The idle time-out is a whole number of minutes from 0 (never) to 43200 (30 days).")
        );
        assert_eq!(idle_problem("20", twenty), None);
    }

    #[test]
    fn saving_needs_a_change() {
        let now = current();
        assert!(!changed(&IisPoolSettings { identity: "applicationpoolidentity".into(), ..now.clone() }, &now));
        assert!(changed(&IisPoolSettings { enable_32bit: true, ..now.clone() }, &now));
        assert!(changed(&IisPoolSettings { runtime: String::new(), ..now.clone() }, &now));
        assert!(changed(&IisPoolSettings { idle_timeout: Duration::ZERO, ..now.clone() }, &now));
        assert!(changed(&IisPoolSettings { start_mode: "AlwaysRunning".into(), ..now.clone() }, &now));
        assert_eq!(intro("shop", "shop"), "The settings of 'shop', the app pool of 'shop'. Saving recycles the pool.");
    }
}
