//! Settings → Servers: Source Control's servers added, edited, deleted, imported from a FileZilla Site Manager file and
//! exported to one. Each acts at once (no Save); afterwards Settings' list and the Source Control page read the
//! servers again.

use std::path::PathBuf;

use gpui::{AppContext, Context, Entity, Window};

use dnnm_application::source_control::model::Environment;
use dnnm_dialogs::settings_page::SettingsPage;
use dnnm_dialogs::source_control_environment::{self as server_dialog, EnvironmentDialog, EnvironmentEdit};
use dnnm_wiring::source_control as wiring;

use super::Workspace;
use crate::store::ToastKind;

/// What Export… suggests as the file's name.
const EXPORT_NAME: &str = "DnnManager-servers.xml";

impl Workspace {
    /// Settings' list and the Source Control page read the servers again.
    pub(crate) fn servers_changed(&mut self, page: &Entity<SettingsPage>, cx: &mut Context<Self>) {
        let services = self.store.read(cx).services();
        let servers = wiring::environments(&services, "");
        page.update(cx, |p, cx| p.set_servers(servers, cx));
        if let Some(source_control) = &self.source_control {
            source_control.update(cx, |p, cx| p.reload_environments(None, cx));
        }
    }

    /// Add server… (`env` None) or Edit…: the server dialog, then saved with its password.
    pub(crate) fn edit_server(&mut self, page: Entity<SettingsPage>, env: Option<Environment>, window: &mut Window, cx: &mut Context<Self>) {
        let is_new = env.is_none();
        let env = env.unwrap_or_else(|| server_dialog::new_environment(""));
        let has_secret = !is_new && wiring::has_secret(&env);
        let view = cx.new(|cx| EnvironmentDialog::new(env, is_new, has_secret, window, cx));
        self.show_dialog(
            view,
            move |ws, edit: EnvironmentEdit, _, cx| {
                let services = ws.store.read(cx).services();
                match wiring::save_environment(&services, &edit.env, edit.secret.as_deref()) {
                    Ok(()) => {
                        ws.servers_changed(&page, cx);
                        let text = format!("Saved the server {}", edit.env.name);
                        ws.store.update(cx, |s, cx| s.toast(ToastKind::Success, text, false, cx));
                    }
                    Err(e) => ws.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx)),
                }
            },
            window,
            cx,
        );
    }

    pub(crate) fn delete_server(&mut self, page: Entity<SettingsPage>, env: Environment, window: &mut Window, cx: &mut Context<Self>) {
        let question = format!(
            "Delete the server {} ({})? Its records of what was synced and its history go too, for every project; nothing on the server or in a project is touched.",
            env.name, env.host
        );
        self.ask(question, &["Delete", "Cancel"], true, window, cx, move |ws, choice, _, cx| {
            if choice != Some(0) {
                return;
            }
            let services = ws.store.read(cx).services();
            let result = wiring::delete_environment(&services, &env);
            ws.servers_changed(&page, cx);
            let (kind, text) = match result {
                Ok(()) => (ToastKind::Success, format!("Deleted the server {}", env.name)),
                Err(e) => (ToastKind::Error, e),
            };
            ws.store.update(cx, |s, cx| s.toast(kind, text, false, cx));
        });
    }

    /// Import…: a FileZilla Site Manager file, its sites added (off the UI thread: the passwords go to the OS's
    /// store, which may ask - the macOS Keychain).
    pub(crate) fn import_servers(&mut self, page: Entity<SettingsPage>, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = dnnm_dialogs::file_prompt::open_file(window, cx, "Import servers", "FileZilla Site Manager", "xml");
        let services = self.store.read(cx).services();
        cx.spawn(async move |this, cx| {
            let Some(file) = chosen.await else { return };
            let result = cx.background_executor().spawn(async move { wiring::import_servers(&services, &file) }).await;
            let _ = this.update(cx, |ws, cx| {
                ws.servers_changed(&page, cx);
                let (kind, text) = match result {
                    Ok(summary) if summary.added.is_empty() => (ToastKind::Warning, summary.message()),
                    Ok(summary) if summary.notes.is_empty() && summary.skipped.is_empty() => (ToastKind::Success, summary.message()),
                    Ok(summary) => (ToastKind::Warning, summary.message()),
                    Err(e) => (ToastKind::Error, format!("The servers couldn't be imported: {e}")),
                };
                ws.store.update(cx, |s, cx| s.toast(kind, text, false, cx));
            });
        })
        .detach();
    }

    /// Export…: asks whether the passwords go too (they are readable in the file, as FileZilla writes them), then
    /// for the file.
    pub(crate) fn export_servers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let question = "Export every server to a FileZilla Site Manager file?\n\nWith passwords, anyone who has the file can read them - FileZilla keeps them in base64, not encrypted. Without, you type each password again after importing.";
        self.ask(question, &["Without passwords", "With passwords", "Cancel"], false, window, cx, move |ws, choice, _, cx| {
            let with_passwords = match choice {
                Some(0) => false,
                Some(1) => true,
                _ => return,
            };
            let documents = dnnm_infrastructure::paths::app_data_directory();
            let start: PathBuf = documents.parent().map(std::path::Path::to_path_buf).unwrap_or(documents);
            let chosen = cx.prompt_for_new_path(&start, Some(EXPORT_NAME));
            let services = ws.store.read(cx).services();
            cx.spawn(async move |this, cx| {
                let Ok(Ok(Some(file))) = chosen.await else { return };
                let shown = file.display().to_string();
                let result = cx
                    .background_executor()
                    .spawn(async move { wiring::export_servers(&services, &file, with_passwords) })
                    .await;
                let _ = this.update(cx, |ws, cx| {
                    let (kind, text) = match result {
                        Ok(n) => {
                            let what = if n == 1 { "1 server".to_string() } else { format!("{n} servers") };
                            let passwords = if with_passwords { " with their passwords - keep the file safe" } else { "" };
                            (ToastKind::Success, format!("Exported {what} to {shown}{passwords}."))
                        }
                        Err(e) => (ToastKind::Error, format!("The servers couldn't be exported: {e}")),
                    };
                    ws.store.update(cx, |s, cx| s.toast(kind, text, false, cx));
                });
            })
            .detach();
        });
    }
}
