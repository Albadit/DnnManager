//! What Test connection found (`Controls/DatabaseCheckList.xaml(.cs)`): "Testing the connection…" while it runs, then
//! each check on a line - a mark (passed, a warning, or failed) and "Name: detail", the name semibold. A function of
//! its state rather than a view, so Change connection, Settings → Database server and New project's Database card
//! draw it alike from what they hold.

use dnnm_ui::px;
use gpui::{App, Div, FontWeight, HighlightStyle, SharedString, StyledText, div, prelude::*};

use dnnm_application::use_cases::database_connection::{CheckOutcome, DatabaseCheckReport};
use dnnm_ui::theme::Theme;
use dnnm_ui::{icons, line_icon};

/// `GlyphCancel` of `Themes/Icons.xaml` - the failed check's mark.
const CANCEL: &str = "\u{E711}";

/// What the list shows (`ShowTesting` / `Show`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CheckListState {
    /// Nothing - no test yet, or the fields no longer say what was tested.
    #[default]
    Empty,
    /// "Testing the connection…" until the result is there.
    Testing,
    /// The checks of a test.
    Report(DatabaseCheckReport),
}

/// The list for `state`; nothing to draw for [`CheckListState::Empty`].
pub fn database_check_list(state: &CheckListState, cx: &App) -> Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    match state {
        CheckListState::Empty => div(),
        CheckListState::Testing => {
            div().text_size(theme.text.hint).text_color(c.text_muted).child("Testing the connection…")
        }
        CheckListState::Report(report) => {
            div().flex().flex_col().gap(px(3.0)).children(report.checks.iter().map(|check| {
                let (glyph, color) = match check.outcome {
                    CheckOutcome::Passed => (icons::CHECK, c.success_text),
                    CheckOutcome::Warning => (icons::WARNING, c.log_warn),
                    CheckOutcome::Failed => (CANCEL, c.error_text),
                };
                let line = format!("{}: {}", check.name, check.detail);
                let name = 0..check.name.len();
                let bold = HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };
                div()
                    .flex()
                    .items_start()
                    .gap(px(icons::LABEL_GAP))
                    .child(line_icon(glyph, icons::SIZE, color, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(c.text_primary)
                            .child(StyledText::new(SharedString::from(line)).with_highlights([(name, bold)])),
                    )
            }))
        }
    }
}
