//! Export for deployment (`Controls/DeploymentExportDialog.xaml(.cs)`): what the package for the live server is made
//! for - the live domains, whether the live site uses HTTPS, the live database's connection string, whether the
//! database goes along, and the folder it is written to, outside the site's folder so the package doesn't end up in
//! the next one. Emits `DialogEvent::Done(ExportForDeploymentRequest)`; the app runs
//! `dnnm_wiring::export_for_deployment::export_for_deployment` with it.

use std::path::{Path, PathBuf};

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, PathPromptOptions,
    SharedString, Subscription, Window, div, prelude::*, px,
};

use dnnm_application::use_cases::export_for_deployment::{ExportForDeploymentRequest, folder_problem, parse_domains};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Tooltip};

/// The text at the top.
pub fn intro(site: &str) -> String {
    format!(
        "A package of '{site}' for the live server: the site's files without logs, caches, source control and editor \
         folders, with web.config ready for the server - and the database as a .bacpac, with the live domain as its \
         portal alias. The local site and its database are not changed."
    )
}

/// What's wrong with the form - the domains first, then the folder; None when Export may go on.
pub fn problem(domains: &str, folder: &str, site_directory: &Path) -> Option<String> {
    parse_domains(domains).1.or_else(|| folder_problem(folder, site_directory))
}

/// The request the form makes (`Request()`); None while it has a problem.
pub fn request(
    site: &str,
    site_directory: &Path,
    has_database: bool,
    form: &DeploymentForm,
) -> Option<ExportForDeploymentRequest> {
    if problem(&form.domains, &form.folder, site_directory).is_some() {
        return None;
    }
    let folder = PathBuf::from(form.folder.trim());
    let connection_string = form.connection_string.trim();
    Some(ExportForDeploymentRequest {
        project_name: site.to_string(),
        project_directory: site_directory.to_path_buf(),
        output_folder: std::path::absolute(&folder).unwrap_or(folder),
        domains: parse_domains(&form.domains).0,
        https: form.https,
        connection_string: (!connection_string.is_empty()).then(|| connection_string.to_string()),
        include_database: has_database && form.include_database,
    })
}

/// What the form holds.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DeploymentForm {
    pub domains: String,
    pub https: bool,
    pub connection_string: String,
    pub include_database: bool,
    pub folder: String,
}

pub struct DeploymentExportDialog {
    site: String,
    directory: PathBuf,
    has_database: bool,
    https: bool,
    include_database: bool,
    domains: Entity<TextInput>,
    connection: Entity<TextInput>,
    folder: Entity<TextInput>,
    focus: FocusHandle,
    _subscriptions: [Subscription; 3],
}

impl EventEmitter<DialogEvent<ExportForDeploymentRequest>> for DeploymentExportDialog {}

impl DeploymentExportDialog {
    /// `default_folder`: `dnnm_wiring::export_for_deployment::default_folder`; `has_database`:
    /// `dnnm_wiring::export_for_deployment::has_database` - without one, Include the database is off and says why.
    pub fn new(
        site: &str,
        directory: &Path,
        default_folder: &Path,
        has_database: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let domains = cx.new(|cx| TextInput::new("", cx));
        let connection = cx.new(|cx| TextInput::new("", cx));
        let folder = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_text(default_folder.display().to_string(), cx);
            input
        });
        let changed = |_: &mut Self, _: Entity<TextInput>, _: &TextChanged, cx: &mut Context<Self>| cx.notify();
        let subscriptions =
            [cx.subscribe(&domains, changed), cx.subscribe(&connection, changed), cx.subscribe(&folder, changed)];
        domains.read(cx).focus_handle(cx).focus(window);
        Self {
            site: site.to_string(),
            directory: directory.to_path_buf(),
            has_database,
            https: true,
            include_database: has_database,
            domains,
            connection,
            folder,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn form(&self, cx: &App) -> DeploymentForm {
        DeploymentForm {
            domains: self.domains.read(cx).text().to_string(),
            https: self.https,
            connection_string: self.connection.read(cx).text().to_string(),
            include_database: self.include_database,
            folder: self.folder.read(cx).text().to_string(),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = request(&self.site, &self.directory, self.has_database, &self.form(cx)) {
            cx.emit(DialogEvent::Done(request));
        }
    }

    /// Browse…: Windows' folder picker, starting nowhere in particular; the folder chosen goes into the field.
    fn browse(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export to folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.folder.update(cx, |f, cx| f.set_text(path.display().to_string(), cx));
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

impl Focusable for DeploymentExportDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DeploymentExportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form(cx);
        let problem = problem(&form.domains, &form.folder, &self.directory);
        let ok = problem.is_none();
        let site = self.site.clone();

        // Fields under their labels 4 apart, the sections 14 apart; a check box (4 above and under it built in) 10 under
        // the field before it.
        let stack = |gap: f32| div().w_full().flex().flex_col().gap(px(gap));
        let domains = stack(4.0)
            .child(dialog::label("Live domain(s)", cx))
            .child(dialog::text_field(&self.domains, window, cx))
            .child(dialog::hint(
                "Comma- or space-separated host names, e.g. www.example.com, example.com - the first is the primary portal alias. Left empty, the portal aliases stay as they are.",
                cx,
            ));
        let https = stack(4.0)
            .child(dialog::check("https", self.https, "The live site uses HTTPS", cx).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.https = !this.https;
                    cx.notify();
                },
            )))
            .child(dialog::hint(
                "Switches DNN's SSL setting on, and turns the HTTPS redirect rules DNN Manager switched off back on.",
                cx,
            ));
        let connection = stack(4.0)
            .child(dialog::label("Live database connection string (optional)", cx))
            .child(dialog::text_field(&self.connection, window, cx))
            .child(dialog::hint("Left empty, web.config gets a placeholder to fill in on the server.", cx));
        let database = stack(4.0)
            .child(
                dialog::check("include-database", self.include_database, "Include the database (.bacpac)", cx)
                    .when(!self.has_database, |c| c.opacity(0.5).cursor_default())
                    .when(self.has_database, |c| {
                        c.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.include_database = !this.include_database;
                            cx.notify();
                        }))
                    }),
            )
            .when(!self.has_database, |d| {
                d.child(dialog::hint("The site's web.config names no database that can be exported.", cx))
            });
        let folder = stack(4.0)
            .child(dialog::label("Folder", cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().child(dialog::text_field(&self.folder, window, cx)))
                    .child(
                        ui::button("browse", "Browse…", ui::ButtonKind::Secondary, true, cx)
                            .min_w(px(80.0))
                            .tooltip(Tooltip::text("Browse…"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.browse(cx))),
                    ),
            )
            .child(dialog::hint(format!("Gets {site}.zip, {site}.bacpac and DEPLOY.txt."), cx));

        let body = div().flex().flex_col().gap(px(12.0)).child(div().w_full().child(intro(&site))).child(
            stack(14.0)
                .child(stack(10.0).child(domains).child(https))
                .child(stack(10.0).child(connection).child(database))
                .child(stack(10.0).child(folder).children(dialog::field_error(problem.map(SharedString::from), cx))),
        );

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Export", ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Export (Enter)"))
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
            .child(dialog::frame("Export for deployment", 520.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(domains: &str, folder: &str) -> DeploymentForm {
        DeploymentForm {
            domains: domains.into(),
            https: true,
            connection_string: "  ".into(),
            include_database: true,
            folder: folder.into(),
        }
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn the_form_makes_the_request() {
        let site = Path::new(r"C:\DNN\shop");
        let req = request("shop", site, true, &form("WWW.example.com, example.com", r" D:\Deploy\shop ")).unwrap();
        assert_eq!(req.domains, ["www.example.com", "example.com"]);
        assert_eq!(req.output_folder, Path::new(r"D:\Deploy\shop"));
        assert_eq!(req.connection_string, None);
        assert!(req.https && req.include_database);
        let without = request("shop", site, false, &form("", r"D:\Deploy")).unwrap();
        assert!(!without.include_database, "no database to export: never included");
        assert!(without.domains.is_empty());
    }

    #[test]
    fn the_domains_are_checked_before_the_folder() {
        let site = Path::new(r"C:\DNN\shop");
        assert_eq!(
            problem("http://x.com", r"C:\DNN\shop\out", site).unwrap(),
            "'http://x.com' isn't a host name - enter it like www.example.com or example.com:8080, without http:// or a path."
        );
        assert_eq!(problem("x.com", r"C:\DNN\shop\out", site).unwrap(), "Choose a folder outside the site's folder.");
        assert_eq!(request("shop", site, true, &form("x.com", "")), None);
        assert!(intro("shop").starts_with("A package of 'shop' for the live server: "));
    }
}
