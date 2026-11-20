//! Change host password (`Controls/HostPasswordDialog.xaml(.cs)`): which host account gets which new password - typed
//! twice, or generated and shown. A site with one host account isn't asked which. Emits
//! `DialogEvent::Done(HostPasswordChoice)`; the app runs `dnnm_wiring::change_host_password` with it.
//!
//! Also [`PasswordField`], the password box with its eye button (`Controls/PasswordInput.xaml`), which Change
//! connection uses too: dots in [`crate::password_input::PasswordInput`], the text itself in a `TextInput` while it is
//! shown, the two kept alike.

use dnnm_ui::px;
use dnnm_ui::{Icon, IconSize};
use std::fmt;

use gpui::{App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Subscription, Window, div, prelude::*};

use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tip};

use crate::password_input::{PasswordChanged, PasswordInput};

/// `GlyphShow` and `GlyphHide` of `Themes/Icons.xaml`: the eye, and the eye struck through while the text shows.
const SHOW: Icon = Icon::Show;
const HIDE: Icon = Icon::Hide;

/// The account and its new password.
#[derive(Clone, PartialEq, Eq)]
pub struct HostPasswordChoice {
    pub user_name: String,
    pub password: String,
}

impl fmt::Debug for HostPasswordChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostPasswordChoice").field("user_name", &self.user_name).finish_non_exhaustive()
    }
}

/// The text above the fields: the one host account by name, or any of them.
pub fn intro(site: &str, hosts: &[String]) -> String {
    let first = match hosts {
        [only] => format!("A new password for '{only}', the host (superuser) account of '{site}'."),
        _ => format!("A new password for a host (superuser) account of '{site}'."),
    };
    format!("{first} DNN Manager doesn't keep it - note it down.")
}

/// What is wrong with the two passwords (`Password_Changed`); nothing while the first is empty.
pub fn password_problem(password: &str, again: &str) -> Option<String> {
    if password.is_empty() {
        return None;
    }
    dnnm_dnn::install::password_problem(password)
        .or_else(|| (!again.is_empty() && again != password).then(|| "The two passwords aren't the same.".into()))
}

/// Change password may be pressed: a password DNN takes, typed the same twice.
pub fn can_change(password: &str, again: &str) -> bool {
    password_problem(password, again).is_none() && !password.is_empty() && again == password
}

/// Why Change password can't go on without a host account chosen (`Ok_Click`).
pub fn no_account_problem(hosts: &[String]) -> &'static str {
    if hosts.is_empty() { "The site's database has no host account to change." } else { "Choose the host account." }
}

pub struct HostPasswordDialog {
    site: String,
    hosts: Vec<String>,
    chosen: Option<usize>,
    open: bool,
    password: Entity<PasswordField>,
    again: Entity<PasswordField>,
    /// Set by a Change password the dialog can't go on with; cleared by an edit.
    refused: Option<&'static str>,
    focus: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<DialogEvent<HostPasswordChoice>> for HostPasswordDialog {}

impl HostPasswordDialog {
    /// `hosts`: the site's host accounts - from its database, or the one DNN Manager set up when that can't be read.
    pub fn new(site: &str, hosts: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(PasswordField::new);
        let again = cx.new(PasswordField::new);
        let edited = |this: &mut Self, _: Entity<PasswordField>, _: &PasswordFieldChanged, cx: &mut Context<Self>| {
            this.refused = None;
            cx.notify();
        };
        let subscriptions = [cx.subscribe(&password, edited), cx.subscribe(&again, edited)];
        password.read(cx).focus(window, cx);
        Self {
            site: site.to_string(),
            chosen: (!hosts.is_empty()).then_some(0),
            hosts,
            open: false,
            password,
            again,
            refused: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generated = dnnm_dnn::install::generate_password(16);
        self.password.update(cx, |f, cx| {
            f.set_password(&generated, cx);
            f.reveal(window, cx);
        });
        self.again.update(cx, |f, cx| f.set_password(&generated, cx));
    }

    fn texts(&self, cx: &App) -> (String, String) {
        (self.password.read(cx).password(cx), self.again.read(cx).password(cx))
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let (password, again) = self.texts(cx);
        if !can_change(&password, &again) {
            return;
        }
        match self.chosen.and_then(|i| self.hosts.get(i)) {
            Some(user) => cx.emit(DialogEvent::Done(HostPasswordChoice { user_name: user.clone(), password })),
            None => {
                self.refused = Some(no_account_problem(&self.hosts));
                cx.notify();
            }
        }
    }
}

impl Focusable for HostPasswordDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HostPasswordDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let (password, again) = self.texts(cx);
        let problem = self.refused.map(str::to_string).or_else(|| password_problem(&password, &again));
        let ok = can_change(&password, &again);

        let options = self
            .hosts
            .iter()
            .enumerate()
            .map(|(i, host)| (SharedString::from(host.clone()), self.chosen == Some(i)))
            .collect();
        let chosen = self.chosen.and_then(|i| self.hosts.get(i)).cloned().unwrap_or_default();

        // Each box 4 under its label, the fields 10 apart.
        let stack = |gap: f32| div().w_full().flex().flex_col().gap(px(gap));
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().w_full().child(intro(&self.site, &self.hosts)))
            // Only when there is a choice: a site with one host account names it in the intro instead.
            .when(self.hosts.len() != 1, |d| {
                d.child(
                    stack(4.0).child(dialog::label("Host account", cx)).child(dialog::select(
                        "host-account-box",
                        chosen,
                        self.open,
                        options,
                        cx.listener(|this, open: &bool, _, cx| {
                            this.open = *open;
                            cx.notify();
                        }),
                        cx.listener(|this, i: &usize, _, cx| {
                            this.chosen = Some(*i);
                            this.refused = None;
                            cx.notify();
                        }),
                        cx,
                    )),
                )
            })
            .child(
                stack(4.0)
                    .child(
                        div().flex().items_center().justify_between().child("New password").child(
                            ui::link("generate-password", "Generate", cx)
                                .text_size(theme.text.caption)
                                .tip("Generate password")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.generate(window, cx))),
                        ),
                    )
                    .child(self.password.clone()),
            )
            .child(stack(4.0).child(dialog::label("New password again", cx)).child(self.again.clone()))
            .children(dialog::field_error(problem.map(SharedString::from), cx));

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Change password", ui::ButtonKind::Primary, ok, cx)
                    .tip("Change password (Enter)")
                    .when(ok, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .tip("Cancel (Esc)")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.submit(cx);
                }
            }))
            .child(dialog::frame("Change host password", 460.0, body, buttons, cx))
    }
}

// ─── The password box with its eye ───────────────────────────────────────────

/// The password changed, typed or set.
#[derive(Debug, Clone, Copy)]
pub struct PasswordFieldChanged;

/// A password box with an eye button that shows and hides what's in it (`PasswordInput`).
pub struct PasswordField {
    hidden: Entity<PasswordInput>,
    shown: Entity<TextInput>,
    revealed: bool,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<PasswordFieldChanged> for PasswordField {}

impl PasswordField {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let hidden = cx.new(PasswordInput::new);
        let shown = cx.new(|cx| TextInput::new("", cx));
        // Each box follows the other; setting one to what it already holds changes nothing, so it doesn't echo.
        let subscriptions = [
            cx.subscribe(&hidden, |this, hidden, _: &PasswordChanged, cx| {
                let password = hidden.read(cx).password().to_string();
                if this.shown.read(cx).text().as_ref() != password {
                    this.shown.update(cx, |s, cx| s.set_text(password, cx));
                }
                cx.emit(PasswordFieldChanged);
            }),
            cx.subscribe(&shown, |this, _, changed: &TextChanged, cx| {
                let text = changed.0.to_string();
                this.hidden.update(cx, |h, cx| h.set_password(&text, cx));
                cx.emit(PasswordFieldChanged);
            }),
        ];
        Self { hidden, shown, revealed: false, _subscriptions: subscriptions }
    }

    pub fn password(&self, cx: &App) -> String {
        self.hidden.read(cx).password().to_string()
    }

    pub fn set_password(&mut self, password: &str, cx: &mut Context<Self>) {
        self.hidden.update(cx, |h, cx| h.set_password(password, cx));
    }

    /// Shows the password - e.g. one just generated, so it can be read and copied.
    pub fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.revealed {
            self.toggle(window, cx);
        }
    }

    /// Puts the keyboard in whichever box shows.
    pub fn focus(&self, window: &mut Window, cx: &App) {
        if self.revealed {
            window.focus(&self.shown.focus_handle(cx));
        } else {
            window.focus(&self.hidden.focus_handle(cx));
        }
    }

    fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.hidden.focus_handle(cx).is_focused(window) || self.shown.focus_handle(cx).is_focused(window)
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.has_focus(window, cx);
        self.revealed = !self.revealed;
        if had_focus {
            self.focus(window, cx);
        }
        cx.notify();
    }
}

impl Render for PasswordField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let focused = self.has_focus(window, cx);
        let tip = if self.revealed { "Hide password" } else { "Show password" };
        let (muted, primary, hover) = (c.text_muted, c.text_primary, c.hover_bg);
        // The text 6 in from the left and clear of the eye, which sits 1 in from the border.
        div()
            .flex()
            .items_center()
            .h(theme.metrics.control_height)
            .p(px(1.0))
            .rounded(theme.metrics.control_radius)
            .border_1()
            .border_color(if focused { c.focus_border } else { c.field_border })
            .bg(c.input_bg)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pad(px(0.0), px(5.0))
                    .map(|d| if self.revealed { d.child(self.shown.clone()) } else { d.child(self.hidden.clone()) }),
            )
            .child(
                ui::focusable_bare(div().id("password-eye"), cx)
                    .flex_none()
                    .h_full()
                    .w(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.metrics.small_radius)
                    .cursor_pointer()
                    .group("password-eye")
                    .hover(move |s| s.bg(hover))
                    .tip(tip)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle(window, cx)))
                    // An svg takes no colour from its parent: its own, and the brighter one while the button is hovered.
                    .child(
                        gpui::svg()
                            .path((if self.revealed { HIDE } else { SHOW }).path(IconSize::Medium))
                            .size(gpui::px(dnnm_ui::icons::drawn_size(IconSize::Medium)))
                            .flex_none()
                            .text_color(muted)
                            .group_hover("password-eye", move |s| s.text_color(primary)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_intro_names_the_only_host_account() {
        assert_eq!(
            intro("shop", &hosts(&["host"])),
            "A new password for 'host', the host (superuser) account of 'shop'. DNN Manager doesn't keep it - note it down."
        );
        assert_eq!(
            intro("shop", &hosts(&["host", "admin2"])),
            "A new password for a host (superuser) account of 'shop'. DNN Manager doesn't keep it - note it down."
        );
    }

    #[test]
    fn passwords_are_checked_as_the_wpf_dialog_checks_them() {
        assert_eq!(password_problem("", "x"), None);
        assert_eq!(password_problem("short", "").as_deref(), Some("The password needs at least 7 characters."));
        assert_eq!(
            password_problem("Secret<123", "").as_deref(),
            Some("The password can't contain < or &# - DNN's login form refuses them.")
        );
        assert_eq!(password_problem("Secret-123", "Secret-12").as_deref(), Some("The two passwords aren't the same."));
        assert_eq!(password_problem("Secret-123", ""), None);
        assert!(!can_change("Secret-123", ""));
        assert!(!can_change("", ""));
        assert!(can_change("Secret-123", "Secret-123"));
    }

    #[test]
    fn without_a_host_account_it_says_why() {
        assert_eq!(no_account_problem(&[]), "The site's database has no host account to change.");
        assert_eq!(no_account_problem(&hosts(&["a", "b"])), "Choose the host account.");
        let choice = HostPasswordChoice { user_name: "host".into(), password: "Secret-123".into() };
        assert!(!format!("{choice:?}").contains("Secret-123"));
    }
}
