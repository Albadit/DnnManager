//! Edit host names (`Controls/BindingsDialog.xaml(.cs)`): the host names and ports a site answers on over http, one
//! row each. Every row remembers the binding it started as, so DNN's portal alias of a changed host name can be renamed
//! rather than a new one added; https and other bindings are only listed. Emits
//! `DialogEvent::Done(EditBindingsRequest)`; the app runs `dnnm_wiring::edit_bindings::edit_bindings` with it.
//!
//! The app gives it the site's bindings split by `dnnm_application::use_cases::edit_bindings::split_bindings`. As the
//! C#, only this site's own bindings are checked for doubles - IIS refuses a binding another site has.

use dnnm_ui::px;
use std::path::PathBuf;

use gpui::{App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Subscription, Window, div, prelude::*};

use dnnm_application::use_cases::edit_bindings::{EditBindingsRequest, HttpBindingEdit};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Tip, icons};

/// The dialog's first line.
pub fn intro(site: &str) -> String {
    format!(
        "The host names and ports '{site}' answers on over http. DNN's portal aliases follow: a changed host name's alias is renamed, a new one's is added."
    )
}

/// One row as it is typed, and the binding it started as (None for one added here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRow {
    pub old_host: Option<String>,
    pub old_port: Option<u16>,
    pub host: String,
    pub port: String,
}

impl BindingRow {
    /// The host name as it is saved: trimmed, lower case.
    pub fn host_name(&self) -> String {
        self.host.trim().to_lowercase()
    }

    /// The port, when it is a number from 1 to 65535 (`int.TryParse`: a sign is allowed).
    pub fn port_number(&self) -> Option<u16> {
        self.port.trim().parse::<i64>().ok().filter(|p| (1..=65535).contains(p)).map(|p| p as u16)
    }
}

/// A DNS name: dot-separated labels of letters, digits and hyphens, at most 253 characters in all.
pub fn is_host_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// What is wrong with the rows; None when they can be saved (`FindProblem`).
pub fn problem(rows: &[BindingRow]) -> Option<String> {
    if rows.is_empty() {
        return Some("Keep at least one host name - the site needs an http binding.".into());
    }
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let host = row.host_name();
        let shown = if host.is_empty() { "any host name".to_string() } else { format!("'{host}'") };
        if !host.is_empty() && !is_host_name(&host) {
            return Some(format!(
                "'{host}' isn't a host name: letters, digits, '-' and '.', each part 1 to 63 characters, not starting or ending with '-'."
            ));
        }
        let Some(port) = row.port_number() else {
            return Some(format!("The port of {shown} must be a number between 1 and 65535."));
        };
        // IIS can't have the same binding twice.
        if !seen.insert((host.clone(), port)) {
            let who = if host.is_empty() { "Any host name".to_string() } else { shown };
            return Some(format!("{who} on port {port} is there twice."));
        }
    }
    None
}

/// The http bindings as they are to be, in order, each with what it was.
pub fn edits(rows: &[BindingRow]) -> Vec<HttpBindingEdit> {
    rows.iter()
        .map(|r| HttpBindingEdit {
            old_host: r.old_host.clone(),
            old_port: r.old_port,
            host: r.host_name(),
            port: r.port_number().unwrap_or(0),
        })
        .collect()
}

/// False while every binding is still there, as it was and in its place: saving that would only recycle the site for
/// nothing.
pub fn changed(rows: &[BindingRow], original: &[(String, u16)]) -> bool {
    let edits = edits(rows);
    edits.len() != original.len()
        || edits
            .iter()
            .zip(original)
            .any(|(e, (host, port))| e.old_host.is_none() || !e.host.eq_ignore_ascii_case(host) || e.port != *port)
}

/// A new host name most likely goes on the port the site already uses.
pub fn port_for_new_row(rows: &[BindingRow]) -> u16 {
    rows.iter().find_map(BindingRow::port_number).unwrap_or(80)
}

struct Row {
    id: usize,
    old_host: Option<String>,
    old_port: Option<u16>,
    host: Entity<TextInput>,
    port: Entity<TextInput>,
    _subscriptions: [Subscription; 2],
}

pub struct BindingsDialog {
    site: String,
    directory: PathBuf,
    original: Vec<(String, u16)>,
    others: Vec<String>,
    rows: Vec<Row>,
    next_id: usize,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent<EditBindingsRequest>> for BindingsDialog {}

impl BindingsDialog {
    /// For the site `site` serving `directory`: its http bindings (host, port) to edit, and its other bindings as text
    /// ("https *:443:www.example.com"), shown as they are.
    pub fn new(
        site: &str,
        directory: PathBuf,
        http: Vec<(String, u16)>,
        others: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut dialog = Self {
            site: site.to_string(),
            directory,
            original: http.clone(),
            others,
            rows: Vec::new(),
            next_id: 0,
            focus: cx.focus_handle(),
        };
        for (host, port) in http {
            dialog.add_row(Some(host.clone()), Some(port), &host, port, cx);
        }
        let first = dialog.rows.first().map(|r| r.host.read(cx).focus_handle(cx)).unwrap_or(dialog.focus.clone());
        window.focus(&first);
        dialog
    }

    fn add_row(
        &mut self,
        old_host: Option<String>,
        old_port: Option<u16>,
        host: &str,
        port: u16,
        cx: &mut Context<Self>,
    ) {
        let text = |value: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx);
                input.set_text(value, cx);
                input
            })
        };
        let (host, port) = (text(host.to_string(), cx), text(port.to_string(), cx));
        let notify = |_: &mut Self, _: Entity<TextInput>, _: &TextChanged, cx: &mut Context<Self>| cx.notify();
        let subscriptions = [cx.subscribe(&host, notify), cx.subscribe(&port, notify)];
        self.rows.push(Row { id: self.next_id, old_host, old_port, host, port, _subscriptions: subscriptions });
        self.next_id += 1;
    }

    fn typed(&self, cx: &App) -> Vec<BindingRow> {
        self.rows
            .iter()
            .map(|r| BindingRow {
                old_host: r.old_host.clone(),
                old_port: r.old_port,
                host: r.host.read(cx).text().to_string(),
                port: r.port.read(cx).text().to_string(),
            })
            .collect()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let rows = self.typed(cx);
        if problem(&rows).is_some() || !changed(&rows, &self.original) {
            return;
        }
        cx.emit(DialogEvent::Done(EditBindingsRequest {
            site_name: self.site.clone(),
            directory: self.directory.clone(),
            bindings: edits(&rows),
        }));
    }
}

impl Focusable for BindingsDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.rows.first().map(|r| r.host.read(cx).focus_handle(cx)).unwrap_or(self.focus.clone())
    }
}

impl Render for BindingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let rows = self.typed(cx);
        let found = problem(&rows);
        let ok = found.is_none() && changed(&rows, &self.original);

        let row_views: Vec<_> = self
            .rows
            .iter()
            .map(|row| {
                let id = row.id;
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().w(px(260.0)).child(dialog::text_field(&row.host, window, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(div().w(px(70.0)).child(dialog::text_field(&row.port, window, cx)))
                            .child(
                                ui::icon_button(
                                    ("remove-binding", id),
                                    icons::DELETE,
                                    theme.colors.text_primary,
                                    true,
                                    cx,
                                )
                                .tip("Remove this host name")
                                .on_click(cx.listener(
                                    move |this, _: &ClickEvent, _, cx| {
                                        this.rows.retain(|r| r.id != id);
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
            })
            .collect();

        let stack = |gap: f32| div().w_full().flex().flex_col().gap(px(gap));
        // The labels 4 above the rows (what an empty host name means in the tip beside its label), the rows 6 apart;
        // Add host name 10 under them.
        let table = stack(4.0)
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        ui::with_tip(
                            dialog::label("Host name", cx),
                            "host-name-tip",
                            "An empty host name answers on any host name.",
                            cx,
                        )
                        .w(px(260.0)),
                    )
                    .child(dialog::label("Port", cx)),
            )
            .when(!row_views.is_empty(), |d| d.child(stack(6.0).children(row_views)));
        let add = div().flex().child(ui::link("add-binding", "Add host name", cx).on_click(cx.listener(
            |this, _: &ClickEvent, window, cx| {
                let port = port_for_new_row(&this.typed(cx));
                this.add_row(None, None, "", port, cx);
                if let Some(row) = this.rows.last() {
                    window.focus(&row.host.read(cx).focus_handle(cx));
                }
                cx.notify();
            },
        )));
        let bindings = stack(10.0).child(table).child(add);

        let body =
            stack(12.0)
                .child(bindings)
                .when(!self.others.is_empty(), |d| {
                    d.child(stack(4.0).child(dialog::label("Kept as they are", cx)).child(
                        stack(2.0).children(self.others.iter().map(|other| div().w_full().child(other.clone()))),
                    ))
                })
                .children(dialog::field_error(found.map(SharedString::from), cx));

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Save", ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tip("Save (Enter)")
                    .when(ok, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(80.0))
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
            .child(dialog::frame_with_tip("Edit host names", Some(intro(&self.site)), 460.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(old: Option<(&str, u16)>, host: &str, port: &str) -> BindingRow {
        BindingRow {
            old_host: old.map(|(h, _)| h.to_string()),
            old_port: old.map(|(_, p)| p),
            host: host.into(),
            port: port.into(),
        }
    }

    #[test]
    fn host_names_are_dns_names() {
        for good in ["a", "shop.dnndev.me", "x-1.example", &"a".repeat(63)] {
            assert!(is_host_name(good), "{good}");
        }
        for bad in ["-a", "a-", "a..b", ".a", "a_b", "ä.me", &"a".repeat(64), &format!("{}a", "a.".repeat(127))] {
            assert!(!is_host_name(bad), "{bad}");
        }
    }

    #[test]
    fn problems_in_the_order_the_rows_have_them() {
        assert_eq!(problem(&[]).as_deref(), Some("Keep at least one host name - the site needs an http binding."));
        assert_eq!(
            problem(&[row(None, "a_b", "80")]).as_deref(),
            Some(
                "'a_b' isn't a host name: letters, digits, '-' and '.', each part 1 to 63 characters, not starting or ending with '-'."
            )
        );
        assert_eq!(
            problem(&[row(None, "", "0")]).as_deref(),
            Some("The port of any host name must be a number between 1 and 65535.")
        );
        assert_eq!(
            problem(&[row(None, " A.me ", "x")]).as_deref(),
            Some("The port of 'a.me' must be a number between 1 and 65535.")
        );
        assert_eq!(
            problem(&[row(None, "A.me", "80"), row(None, "a.me ", "+80")]).as_deref(),
            Some("'a.me' on port 80 is there twice.")
        );
        assert_eq!(
            problem(&[row(None, "", "81"), row(None, " ", "81")]).as_deref(),
            Some("Any host name on port 81 is there twice.")
        );
        assert_eq!(problem(&[row(None, "a.me", "65535"), row(None, "", "1")]), None);
    }

    #[test]
    fn saving_needs_a_change() {
        let original = vec![("a.me".to_string(), 80), (String::new(), 8080)];
        let same = vec![row(Some(("a.me", 80)), "A.ME", "80"), row(Some(("", 8080)), "", "8080")];
        assert!(!changed(&same, &original));
        let moved = vec![row(Some(("", 8080)), "", "8080"), row(Some(("a.me", 80)), "a.me", "80")];
        assert!(changed(&moved, &original));
        assert!(changed(&same[..1], &original));
        let mut added = same.clone();
        added[1] = row(None, "", "8080");
        assert!(changed(&added, &original));
        let mut port = same.clone();
        port[1].port = "8081".into();
        assert!(changed(&port, &original));
    }

    #[test]
    fn edits_and_the_port_of_a_new_row() {
        let rows = vec![row(Some(("a.me", 80)), " B.me ", "8080"), row(None, "", "x")];
        assert_eq!(
            edits(&rows),
            vec![
                HttpBindingEdit { old_host: Some("a.me".into()), old_port: Some(80), host: "b.me".into(), port: 8080 },
                HttpBindingEdit { old_host: None, old_port: None, host: String::new(), port: 0 },
            ]
        );
        assert_eq!(port_for_new_row(&rows), 8080);
        assert_eq!(port_for_new_row(&[row(None, "", "x")]), 80);
        assert!(intro("shop").starts_with("The host names and ports 'shop' answers on over http."));
    }
}
