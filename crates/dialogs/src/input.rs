//! A one-line text prompt (`Controls/InputDialog.xaml(.cs)`) - optionally checked as it is typed, with what is wrong
//! under the box. Emits `DialogEvent::Done(text)` with the text trimmed; OK waits until the text isn't blank and the
//! check finds nothing wrong. Clone's question, suggestion and check: `dnnm_wiring::clone_project`.
//!
//! The XAML selects the text when the dialog opens; here the cursor is put after it (Ctrl+A selects it), as the other
//! dialogs of the port do.

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Subscription,
    Window, div, prelude::*, px,
};

use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Tooltip};

/// The window's title (`Title="DNN Manager"`).
pub const TITLE: &str = "DNN Manager";
/// OK's text when the caller gives none.
pub const OK: &str = "OK";

/// What is wrong with a text - None when it is fine.
pub type Validate = Box<ValidateFn>;
/// The check itself: the text, trimmed - what is wrong with it, or None.
pub type ValidateFn = dyn Fn(&str) -> Option<String>;

/// What the dialog shows for `typed` (`InputDialog.Check`): the red line, and whether OK is enabled. A blank text has
/// no problem shown, but OK waits.
pub fn check(typed: &str, validate: Option<&ValidateFn>) -> (Option<String>, bool) {
    let value = typed.trim();
    let problem = if value.is_empty() { None } else { validate.and_then(|v| v(value)) };
    let ok = !value.is_empty() && problem.is_none();
    (problem, ok)
}

pub struct InputDialog {
    question: SharedString,
    ok_text: SharedString,
    text: Entity<TextInput>,
    validate: Option<Validate>,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent<String>> for InputDialog {}

impl InputDialog {
    /// Asks `question`, the box filled with `initial`; OK labelled `ok_text` ([`OK`] for the usual one) waits until
    /// `validate` finds nothing wrong.
    pub fn new(
        question: impl Into<SharedString>,
        initial: Option<&str>,
        ok_text: impl Into<SharedString>,
        validate: Option<Validate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = initial.unwrap_or_default().to_string();
        let text = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_text(initial, cx);
            input
        });
        let subscription = cx.subscribe(&text, |_, _, _: &TextChanged, cx| cx.notify());
        window.focus(&text.read(cx).focus_handle(cx));
        Self { question: question.into(), ok_text: ok_text.into(), text, validate, _subscription: subscription }
    }

    fn checked(&self, cx: &App) -> (Option<String>, bool) {
        check(self.text.read(cx).text(), self.validate.as_deref())
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.checked(cx).1 {
            return;
        }
        let value = self.text.read(cx).text().trim().to_string();
        cx.emit(DialogEvent::Done(value));
    }
}

impl Focusable for InputDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.text.read(cx).focus_handle(cx)
    }
}

impl Render for InputDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (problem, ok) = self.checked(cx);
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().w_full().child(self.question.clone()))
            .child(dialog::text_field(&self.text, window, cx))
            .children(dialog::field_error(problem.map(SharedString::from), cx));

        let ok_tooltip = format!("{} (Enter)", self.ok_text);
        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", self.ok_text.clone(), ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text(ok_tooltip))
                    .when(ok, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Cancel (Esc)"))
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );

        div()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.submit(cx);
                }
            }))
            .child(dialog::frame(TITLE, 460.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_waits_without_a_problem_and_the_check_sees_the_trimmed_text() {
        let taken = |t: &str| (t == "shop").then(|| format!("A project named '{t}' already exists."));
        assert_eq!(check("   ", Some(&taken)), (None, false));
        assert_eq!(check(" shop ", Some(&taken)), (Some("A project named 'shop' already exists.".into()), false));
        assert_eq!(check(" store ", Some(&taken)), (None, true));
        assert_eq!(check("anything", None), (None, true));
    }
}
