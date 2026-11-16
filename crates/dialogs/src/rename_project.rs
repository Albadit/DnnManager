//! Rename project (`Controls/RenameProjectDialog.xaml(.cs)`): the project's new name - checked as it is typed - and
//! whether its host name and its database are renamed with it. Emits `DialogEvent::Done(RenameProjectRequest)`; the
//! app runs `dnnm_wiring::rename_project::rename_project` with it.
//!
//! What it shows comes from `dnnm_wiring::rename_project::dialog_facts` (IIS's sites, the host name and database to
//! offer). The XAML selects the name when the dialog opens; here the cursor is put after it (Ctrl+A selects it).

use std::path::PathBuf;

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString, Subscription,
    Window, div, prelude::*, px,
};

use dnnm_application::use_cases::rename_project::{
    RenameDialogFacts, RenameProjectRequest, hostname_for, name_problem,
};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Pad, Tooltip};

/// The dialog's first line.
pub fn intro(site: &str) -> String {
    format!(
        "Renaming '{site}' changes its folder (when it is in the projects folder), its IIS site and its app pool. Its backups stay under the old name."
    )
}

/// What the dialog shows for the name typed (`RenameProjectDialog.Check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameCheck {
    /// The red line under the name.
    pub problem: Option<String>,
    /// Rename is enabled.
    pub can_rename: bool,
    /// The check box texts - None when there is nothing of the kind to offer.
    pub host_text: Option<String>,
    pub database_text: Option<String>,
}

/// Checks the name typed for the project `site`.
pub fn check(typed: &str, site: &str, facts: &RenameDialogFacts) -> RenameCheck {
    let name = typed.trim();
    // The same name again is no rename - Rename just waits, without calling it a mistake. Another casing of it is a
    // rename.
    let unchanged = name == site;
    let problem = if name.is_empty() || unchanged { None } else { name_problem(name, site, &facts.sites) };
    // While the box is empty, the texts keep showing the current name rather than an empty one.
    let shown = if name.is_empty() { site } else { name };
    RenameCheck {
        can_rename: !name.is_empty() && !unchanged && problem.is_none(),
        problem,
        host_text: facts
            .current_host
            .as_ref()
            .map(|host| format!("Also change the host name: {host} → {}", hostname_for(shown, &facts.hostname_suffix))),
        database_text: facts.database.as_ref().map(|db| format!("Also rename the database: [{db}] → [{shown}]")),
    }
}

/// What Rename asks for: without a host name or a database to offer, that part is false.
pub fn request(
    site: &str,
    directory: PathBuf,
    typed: &str,
    facts: &RenameDialogFacts,
    rename_host: bool,
    rename_database: bool,
) -> RenameProjectRequest {
    RenameProjectRequest {
        site_name: site.to_string(),
        directory,
        new_name: typed.trim().to_string(),
        rename_host: facts.current_host.is_some() && rename_host,
        rename_database: facts.database.is_some() && rename_database,
    }
}

pub struct RenameProjectDialog {
    site: String,
    directory: PathBuf,
    facts: RenameDialogFacts,
    name: Entity<TextInput>,
    rename_host: bool,
    rename_database: bool,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent<RenameProjectRequest>> for RenameProjectDialog {}

impl RenameProjectDialog {
    /// For the project `site` serving `directory`, with what `dialog_facts` found.
    pub fn new(
        site: &str,
        directory: PathBuf,
        facts: RenameDialogFacts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_text(site.to_string(), cx);
            input
        });
        let subscription = cx.subscribe(&name, |_, _, _: &TextChanged, cx| cx.notify());
        window.focus(&name.read(cx).focus_handle(cx));
        Self {
            site: site.to_string(),
            directory,
            facts,
            name,
            rename_host: true,
            rename_database: true,
            _subscription: subscription,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let typed = self.name.read(cx).text().to_string();
        if !check(&typed, &self.site, &self.facts).can_rename {
            return;
        }
        cx.emit(DialogEvent::Done(request(
            &self.site,
            self.directory.clone(),
            &typed,
            &self.facts,
            self.rename_host,
            self.rename_database,
        )));
    }
}

impl Focusable for RenameProjectDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}

impl Render for RenameProjectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let checked = check(self.name.read(cx).text(), &self.site, &self.facts);
        // Only for a project that has a host name, and one that has a database: the texts follow the typed name.
        let has_options = checked.host_text.is_some() || checked.database_text.is_some();
        let options = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .when_some(checked.host_text, |d, text| {
                d.child(dialog::check("rename-host", self.rename_host, text, cx).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.rename_host = !this.rename_host;
                        cx.notify();
                    },
                )))
            })
            .when_some(checked.database_text, |d, text| {
                d.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(dialog::check("rename-database", self.rename_database, text, cx).on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.rename_database = !this.rename_database;
                                cx.notify();
                            },
                        )))
                        // Under the check box's text.
                        .child(
                            div()
                                .w_full()
                                .pad(px(0.0), px(22.0))
                                .child(dialog::hint("The site is stopped while the database is renamed.", cx)),
                        ),
                )
            });
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().w_full().child(intro(&self.site)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(dialog::label("New name", cx))
                    .child(dialog::text_field(&self.name, window, cx)),
            )
            .children(dialog::field_error(checked.problem.map(SharedString::from), cx))
            .when(has_options, |d| d.child(options));

        let ok = checked.can_rename;
        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Rename", ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Rename (Enter)"))
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
            .child(dialog::frame("Rename project", 460.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(host: bool, database: bool) -> RenameDialogFacts {
        RenameDialogFacts {
            sites: vec!["shop".into(), "Taken".into()],
            current_host: host.then(|| "shop.dnndev.me".to_string()),
            hostname_suffix: "dnndev.me".into(),
            database: database.then(|| "shop".to_string()),
        }
    }

    #[test]
    fn the_same_name_or_none_waits_without_a_problem() {
        for typed in ["shop", "", "   "] {
            let c = check(typed, "shop", &facts(true, true));
            assert_eq!((c.problem, c.can_rename), (None, false), "{typed:?}");
        }
        // While empty, the check boxes keep the current name.
        let c = check("", "shop", &facts(true, true));
        assert_eq!(c.host_text.as_deref(), Some("Also change the host name: shop.dnndev.me → shop.dnndev.me"));
        assert_eq!(c.database_text.as_deref(), Some("Also rename the database: [shop] → [shop]"));
    }

    #[test]
    fn other_capitals_are_a_rename_and_taken_or_bad_names_a_problem() {
        let c = check(" Shop ", "shop", &facts(false, false));
        assert_eq!((c.problem.clone(), c.can_rename), (None, true));
        assert_eq!((c.host_text, c.database_text), (None, None));
        let c = check("taken", "shop", &facts(false, false));
        assert_eq!(c.problem.as_deref(), Some("IIS already has a site named 'taken'."));
        assert!(!c.can_rename);
        let c = check("-x", "shop", &facts(false, false));
        assert_eq!(c.problem.as_deref(), Some("Project name must start with a letter or a digit."));
        let c = check("store", "shop", &facts(true, true));
        assert!(c.can_rename);
        assert_eq!(c.host_text.as_deref(), Some("Also change the host name: shop.dnndev.me → store.dnndev.me"));
        assert_eq!(c.database_text.as_deref(), Some("Also rename the database: [shop] → [store]"));
    }

    #[test]
    fn the_request_offers_only_what_was_shown() {
        let r = request("shop", PathBuf::from(r"C:\DNN\shop"), " store ", &facts(false, true), true, false);
        assert_eq!(r.new_name, "store");
        assert_eq!((r.rename_host, r.rename_database), (false, false));
        let r = request("shop", PathBuf::new(), "store", &facts(true, true), true, true);
        assert_eq!((r.rename_host, r.rename_database), (true, true));
        assert!(intro("shop").starts_with("Renaming 'shop' changes its folder"));
    }
}
