//! Edit Windows features (`Controls/IisFeaturesDialog.xaml(.cs)`): the IIS Windows features Settings → IIS checks and
//! enables, a row each - how it is shown and its Windows feature name - added, removed, or the defaults again. Emits
//! `DialogEvent::Done(features)` only when something changed and every row is right.

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window, div,
    prelude::*, px,
};

use dnnm_system::prereq::{IisFeature, default_features, is_iis_feature_name};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Tooltip, icons};

/// A row as typed: (shown as, Windows feature name).
pub type FeatureRow = (String, String);

/// The features the rows make: the name trimmed; an empty label shows the feature by its name.
pub fn features_of(rows: &[FeatureRow]) -> Vec<IisFeature> {
    rows.iter()
        .map(|(label, name)| {
            let name = name.trim().to_string();
            let label = if label.trim().is_empty() { name.clone() } else { label.trim().to_string() };
            IisFeature { name, label }
        })
        .collect()
}

/// What is wrong with the rows, or None (`FindProblem`).
pub fn problem_of(rows: &[FeatureRow]) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for (label, name) in rows {
        let name = name.trim();
        if name.is_empty() {
            return Some(if label.trim().is_empty() {
                "Fill in or remove the empty row.".into()
            } else {
                format!("'{}' needs its Windows feature name.", label.trim())
            });
        }
        if !is_iis_feature_name(name) {
            return Some(format!("'{name}' isn't a Windows feature name: letters, digits, '-', '_' and '.' only."));
        }
        if seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            return Some(format!("{name} is there twice."));
        }
        seen.push(name.to_string());
    }
    None
}

struct Row {
    label: Entity<TextInput>,
    name: Entity<TextInput>,
    _subscriptions: [Subscription; 2],
}

pub struct IisFeaturesDialog {
    original: Vec<IisFeature>,
    rows: Vec<Row>,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent<Vec<IisFeature>>> for IisFeaturesDialog {}

impl IisFeaturesDialog {
    pub fn new(features: Vec<IisFeature>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut dialog = Self { original: features.clone(), rows: Vec::new(), focus: cx.focus_handle() };
        dialog.fill(&features, cx);
        match dialog.rows.first() {
            Some(row) => window.focus(&row.label.focus_handle(cx)),
            None => window.focus(&dialog.focus),
        }
        dialog
    }

    fn fill(&mut self, features: &[IisFeature], cx: &mut Context<Self>) {
        self.rows.clear();
        for f in features {
            self.add_row(&f.label, &f.name, cx);
        }
        cx.notify();
    }

    fn add_row(&mut self, label: &str, name: &str, cx: &mut Context<Self>) -> usize {
        let make = |text: &str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| TextInput::new("", cx));
            input.update(cx, |i, cx| i.set_text(text.to_string(), cx));
            input
        };
        let label = make(label, cx);
        let name = make(name, cx);
        let notify = |cx: &mut Context<Self>, input: &Entity<TextInput>| {
            cx.subscribe(input, |_, _, _: &TextChanged, cx| cx.notify())
        };
        let subscriptions = [notify(cx, &label), notify(cx, &name)];
        self.rows.push(Row { label, name, _subscriptions: subscriptions });
        self.rows.len() - 1
    }

    fn typed(&self, cx: &App) -> Vec<FeatureRow> {
        self.rows.iter().map(|r| (r.label.read(cx).text().to_string(), r.name.read(cx).text().to_string())).collect()
    }

    fn changed(&self, features: &[IisFeature]) -> bool {
        features.len() != self.original.len()
            || features.iter().zip(&self.original).any(|(a, b)| a.name != b.name || a.label != b.label)
    }
}

impl Focusable for IisFeaturesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for IisFeaturesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let typed = self.typed(cx);
        let problem = problem_of(&typed);
        let features = features_of(&typed);
        let ok = problem.is_none() && self.changed(&features);

        let mut list = div().id("iis-feature-rows").flex().flex_col().gap(px(6.0)).max_h(px(420.0)).overflow_y_scroll();
        for (i, row) in self.rows.iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(div().w(px(230.0)).child(dialog::text_field(&row.label, window, cx)))
                            .child(
                                div()
                                    .w(px(230.0))
                                    .font_family(theme.mono_font.clone())
                                    .child(dialog::text_field(&row.name, window, cx)),
                            ),
                    )
                    .child(
                        div().child(
                            ui::icon_button(
                                SharedString::from(format!("remove-feature-{i}")),
                                icons::DELETE,
                                c.text_primary,
                                true,
                                cx,
                            )
                            .tooltip(Tooltip::text("Remove this feature"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if i < this.rows.len() {
                                    this.rows.remove(i);
                                }
                                cx.notify();
                            })),
                        ),
                    ),
            );
        }

        // The intro 18 above the column labels, 4 above the rows; the links 10 under the rows, the problem 10 under
        // them.
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(18.0))
                    .child(div().child(
                        "The Windows features Test checks and Set up IIS enables. dism /online /get-features lists their names.",
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .child(div().w(px(230.0)).child(dialog::label("Shown as", cx)))
                                    .child(div().child(dialog::label("Windows feature name", cx))),
                            )
                            .child(list),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(16.0))
                    .child(ui::link("add-feature", "Add feature", cx).on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            let i = this.add_row("", "", cx);
                            window.focus(&this.rows[i].label.focus_handle(cx));
                            cx.notify();
                        },
                    )))
                    .child(ui::link("default-features", "Use the defaults", cx).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| this.fill(&default_features(), cx),
                    ))),
            )
            .children(dialog::field_error(problem.map(SharedString::from), cx));

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Save", ui::ButtonKind::Primary, ok, cx)
                    .tooltip(Tooltip::text("Save (Enter)"))
                    .when(ok, |b| {
                        b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let features = features_of(&this.typed(cx));
                            cx.emit(DialogEvent::Done(features));
                        }))
                    }),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .tooltip(Tooltip::text("Cancel (Esc)"))
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );

        div().track_focus(&self.focus).child(dialog::frame("Edit Windows features", 560.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, name: &str) -> FeatureRow {
        (label.to_string(), name.to_string())
    }

    #[test]
    fn every_row_needs_a_windows_feature_name_once() {
        assert_eq!(problem_of(&[row("", " ")]).as_deref(), Some("Fill in or remove the empty row."));
        assert_eq!(problem_of(&[row("ASP.NET", "")]).as_deref(), Some("'ASP.NET' needs its Windows feature name."));
        assert_eq!(
            problem_of(&[row("x", "IIS ASPNET")]).as_deref(),
            Some("'IIS ASPNET' isn't a Windows feature name: letters, digits, '-', '_' and '.' only.")
        );
        assert_eq!(
            problem_of(&[row("a", "IIS-ASPNET45"), row("b", "iis-aspnet45")]).as_deref(),
            Some("iis-aspnet45 is there twice.")
        );
        assert_eq!(problem_of(&[row("a", "IIS-ASPNET45")]), None);
    }

    #[test]
    fn an_empty_label_shows_the_feature_by_its_name() {
        assert_eq!(
            features_of(&[row(" ", " IIS-ASPNET45 ")]),
            vec![IisFeature { name: "IIS-ASPNET45".into(), label: "IIS-ASPNET45".into() }]
        );
    }
}
