//! The deployment preview before Source Control sends anything: what each file will be - new on the server, replaced
//! there, deleted there, new in the project, replaced in the project - with the bytes, the protected ones named apart,
//! and what is left alone with why. Confirmed, the transfer runs without asking again (`previewed`); Esc or Cancel
//! sends nothing. Emits `DialogEvent::Done(())`.

use dnnm_ui::px;

use gpui::{ClickEvent, Context, EventEmitter, FontWeight, ListSizingBehavior, SharedString, Window, div, prelude::*, uniform_list};

use dnnm_application::source_control::model::{Environment, EnvironmentKind};
use dnnm_application::source_control::plan::{Planned, Preview, Step};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tip};

const STEPS: [Step; 5] = [Step::Replace, Step::Delete, Step::Overwrite, Step::Upload, Step::Download];

/// "3 new on the server · 2 replaced on the server · 1.2 MB".
pub fn summary(preview: &Preview) -> String {
    let mut parts: Vec<String> = STEPS
        .iter()
        .filter_map(|&step| {
            let n = preview.count(step);
            (n > 0).then(|| format!("{n} {}", step.title().to_lowercase()))
        })
        .collect();
    let bytes = preview.bytes();
    if bytes > 0 {
        parts.push(dnnm_application::project::view::byte_size(bytes as f64));
    }
    if !preview.refused.is_empty() {
        parts.push(format!("{} left alone", preview.refused.len()));
    }
    parts.join(" · ")
}

/// The dialog's rows: a heading per kind of step, then its files; then what is left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Heading(String, bool),
    File(Planned),
    Refused(String, String),
}

fn lines(preview: &Preview) -> Vec<Line> {
    let mut out = Vec::new();
    for step in STEPS {
        let files: Vec<&Planned> = preview.planned.iter().filter(|p| p.step == step).collect();
        if !files.is_empty() {
            out.push(Line::Heading(format!("{} ({})", step.title(), files.len()), step.destructive()));
            out.extend(files.into_iter().cloned().map(Line::File));
        }
    }
    if !preview.refused.is_empty() {
        out.push(Line::Heading(format!("Left alone ({})", preview.refused.len()), false));
        out.extend(preview.refused.iter().cloned().map(|(p, why)| Line::Refused(p, why)));
    }
    out
}

pub struct PreviewDialog {
    title: SharedString,
    env: Environment,
    preview: Preview,
    lines: Vec<Line>,
}

impl EventEmitter<DialogEvent<()>> for PreviewDialog {}

impl PreviewDialog {
    pub fn new(title: impl Into<SharedString>, env: Environment, preview: Preview) -> Self {
        let lines = lines(&preview);
        Self { title: title.into(), env, preview, lines }
    }
}

impl Render for PreviewDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let production = self.env.kind == EnvironmentKind::Production;
        let protected = self.preview.protected();
        let lines = self.lines.clone();
        let list = uniform_list("sc-preview-files", lines.len(), move |range, _, cx| {
            let theme = Theme::get(cx).clone();
            let c = theme.colors;
            range
                .map(|i| match &lines[i] {
                    Line::Heading(text, destructive) => div()
                        .pad(px(4.0), px(8.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if *destructive { c.error_text } else { c.text_primary })
                        .child(text.clone()),
                    Line::File(p) => div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .pad(px(1.0), px(20.0))
                        .child(div().flex_1().min_w_0().child(ui::one_line(p.path.clone())))
                        .when(p.protected, |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .pad(px(0.0), px(6.0))
                                    .rounded(theme.metrics.small_radius)
                                    .border_1()
                                    .border_color(c.error_text)
                                    .text_color(c.error_text)
                                    .text_size(theme.text.small)
                                    .child("protected"),
                            )
                        })
                        .when(p.bytes > 0, |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_color(c.text_muted)
                                    .child(dnnm_application::project::view::byte_size(p.bytes as f64)),
                            )
                        }),
                    Line::Refused(path, why) => div()
                        .flex()
                        .gap(px(8.0))
                        .pad(px(1.0), px(20.0))
                        .text_color(c.text_muted)
                        .child(div().flex_none().max_w(px(260.0)).child(ui::one_line(path.clone())))
                        .child(div().flex_1().min_w_0().child(ui::one_line(why.clone()))),
                })
                .collect()
        })
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .max_h(px(320.0));

        let warning = (!protected.is_empty()).then(|| {
            div()
                .p(px(10.0))
                .rounded(theme.metrics.control_radius)
                .border_1()
                .border_color(c.warn_banner_border)
                .bg(c.warn_banner_bg)
                .child(format!(
                    "{} protected file(s) on {}: configuration, assemblies, or what the live site made. They are sent too when you confirm.",
                    protected.len(),
                    self.env.name
                ))
        });
        let nothing = self.preview.is_empty();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().text_color(c.text_muted).child(format!("{} - {}", self.env.label(), summary(&self.preview))))
            .children(warning)
            .when(nothing, |d| d.child(div().text_color(c.text_muted).child("Nothing can be sent - see why below.")))
            .child(
                div()
                    .rounded(theme.metrics.control_radius)
                    .border_1()
                    .border_color(c.card_border)
                    .bg(c.input_bg)
                    .text_size(theme.text.caption)
                    .child(list),
            )
            .child(dialog::hint(
                "Each file is checked again right before it is sent: one changed since the comparison is left alone.",
                cx,
            ));
        let destructive = self.preview.destructive();
        let kind = if destructive && (production || !protected.is_empty()) { ui::ButtonKind::Danger } else { ui::ButtonKind::Primary };
        let label = match (self.preview.count(Step::Upload) + self.preview.count(Step::Replace) > 0, destructive) {
            (true, _) => "Deploy",
            (false, true) => "Apply",
            (false, false) => "Download",
        };
        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", label, kind, !nothing, cx)
                    .min_w(px(90.0))
                    .tip("Send these files now")
                    .when(!nothing, |b| b.on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Done(()))))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(90.0))
                    .tip("Send nothing (Esc)")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );
        div().child(dialog::frame(self.title.clone(), 640.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(path: &str, step: Step, bytes: u64, protected: bool) -> Planned {
        Planned { path: path.into(), step, bytes, protected }
    }

    #[test]
    fn the_summary_and_the_lines_go_from_what_is_destroyed_to_what_is_new() {
        let preview = Preview {
            planned: vec![
                planned("a.css", Step::Upload, 1024, false),
                planned("web.config", Step::Replace, 2048, true),
                planned("old.js", Step::Delete, 0, false),
            ],
            refused: vec![("c.js".into(), "a conflict".into())],
        };
        assert_eq!(
            summary(&preview),
            "1 replaced on the server · 1 deleted on the server · 1 new on the server · 3 KB · 1 left alone"
        );
        let headings: Vec<String> = lines(&preview)
            .into_iter()
            .filter_map(|l| if let Line::Heading(t, _) = l { Some(t) } else { None })
            .collect();
        assert_eq!(
            headings,
            ["Replaced on the server (1)", "Deleted on the server (1)", "New on the server (1)", "Left alone (1)"]
        );
    }
}
