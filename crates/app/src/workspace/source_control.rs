//! The Source Control page in the window: made the first time it is shown, given the projects as the store has
//! them, and its requests run - a comparison or a transfer as an operation (Output shows it; Cancel stops it), an
//! environment edited in its dialog and saved (its password in the OS's store).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{AppContext, Context, Entity, Window};

use dnnm_application::operation::{CancelToken, Outcome};
use dnnm_application::source_control::model::{Entry, Environment, HistoryLine};
use dnnm_application::source_control::sync::BothCopies;
use dnnm_dialogs::source_control_environment::{EnvironmentDialog, EnvironmentEdit};
use dnnm_dialogs::source_control_page::{
    SourceControlBackend, SourceControlEvent, SourceControlPage, SourceControlSide, Status,
};
use dnnm_dialogs::source_control_preview::PreviewDialog;
use dnnm_wiring::source_control as wiring;

use super::Workspace;
use crate::store::ToastKind;

/// The page's reads, through the wiring.
struct Backend(dnnm_wiring::Services);

impl SourceControlBackend for Backend {
    fn environments(&self, project: &str) -> Result<Vec<Environment>, String> {
        wiring::environments(&self.0, project)
    }

    fn history(&self, env: &Environment) -> Result<Vec<HistoryLine>, String> {
        wiring::history(&self.0, env)
    }

    fn read_both(
        &self,
        project_dir: &std::path::Path,
        env: &Environment,
        path: &str,
        cancel: &CancelToken,
    ) -> Result<BothCopies, String> {
        wiring::read_both(&self.0, project_dir, env, path, cancel)
    }

    fn reclassify_local(&self, project_dir: &std::path::Path, env: &Environment, last: &[Entry]) -> Result<Vec<Entry>, String> {
        wiring::reclassify_local(&self.0, project_dir, env, last)
    }

    fn cached(&self, project_dir: &std::path::Path, env: &Environment) -> Result<Option<(i64, Vec<Entry>)>, String> {
        wiring::cached(&self.0, project_dir, env)
    }
}

/// The settings rows that keep the project and server chosen last, to choose them again at the next start.
const PROJECT_KEY: &str = "sourceControl.project";
const SERVER_KEY: &str = "sourceControl.server";

/// How long the folder must be quiet before the list is checked again: one save is several events.
const QUIET: std::time::Duration = std::time::Duration::from_millis(800);

fn now_text() -> String {
    dnnm_infrastructure::local_time::hms(std::time::SystemTime::now())
}

impl Workspace {
    /// Watches the chosen project's folder (another one: the old watch ends); a change, once the folder is quiet,
    /// checks the list again without connecting.
    fn watch_project(&mut self, folder: Option<PathBuf>, page: Entity<SourceControlPage>, cx: &mut Context<Self>) {
        self.source_control_watch = None;
        let Some(folder) = folder.filter(|f| f.is_dir()) else { return };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let watcher = match dnnm_infrastructure::source_control::watch(&folder, move || {
            let _ = tx.unbounded_send(());
        }) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("{e}");
                return;
            }
        };
        let weak = page.downgrade();
        let task = cx.spawn(async move |_, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                // Until it is quiet: each new event starts the wait again.
                loop {
                    cx.background_executor().timer(QUIET).await;
                    let mut more = false;
                    while rx.try_recv().is_ok() {
                        more = true;
                    }
                    if !more {
                        break;
                    }
                }
                if cx.update(|cx| weak.update(cx, |p, cx| p.local_changed(cx))).is_err() {
                    break;
                }
            }
        });
        self.source_control_watch = Some((watcher, task));
    }

    /// The page, made the first time.
    pub(super) fn source_control_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<SourceControlPage> {
        if let Some(page) = &self.source_control {
            return page.clone();
        }
        let backend = Arc::new(Backend(self.store.read(cx).services()));
        let settings = &self.store.read(cx).settings;
        let project = settings.get(PROJECT_KEY).map(str::to_string);
        let server = settings.get(SERVER_KEY).map(str::to_string);
        let page = cx.new(|cx| {
            let mut page = SourceControlPage::new(backend, cx);
            page.restore(project, server);
            page
        });
        let sub = cx.subscribe_in(&page, window, |ws, page, e: &SourceControlEvent, window, cx| {
            ws.source_control_event(page.clone(), e.clone(), window, cx)
        });
        self._subscriptions.push(sub);
        self.source_control = Some(page.clone());
        self.sync_source_control_projects(cx);
        page
    }

    /// The page's files for the primary side bar - made with the page (which is made first).
    pub(super) fn source_control_side(&mut self, cx: &mut Context<Self>) -> Entity<SourceControlSide> {
        if let Some(side) = &self.source_control_side {
            return side.clone();
        }
        let page = self.source_control.clone().expect("the page is made before its side bar");
        let side = cx.new(|cx| SourceControlSide::new(page, cx));
        self.source_control_side = Some(side.clone());
        side
    }

    /// The projects the page offers: every DNN site the store knows, with its folder.
    pub(super) fn sync_source_control_projects(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.source_control.clone() else { return };
        let mut projects: Vec<(String, PathBuf)> = self
            .store
            .read(cx)
            .projects
            .iter()
            .filter(|p| p.dnn_version.is_some() && !p.directory.as_os_str().is_empty())
            .map(|p| (p.name.clone(), p.directory.clone()))
            .collect();
        projects.sort_by_key(|(n, _)| n.to_lowercase());
        page.update(cx, |p, cx| p.set_projects(projects, cx));
    }

    fn source_control_event(
        &mut self,
        page: Entity<SourceControlPage>,
        event: SourceControlEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let services = self.store.read(cx).services();
        let weak = page.downgrade();
        match event {
            SourceControlEvent::Scan { project_dir, env } => {
                let title = format!("Compare {} with {}", env.project, env.label());
                let found = Arc::new(Mutex::new(None));
                let kept = found.clone();
                let env_id = env.id.clone();
                page.update(cx, |p, cx| p.set_status(Status::Working("Comparing with the server…".into()), cx));
                self.store.update(cx, |s, cx| {
                    s.run_remote(
                        title,
                        move |op| {
                            let scan = wiring::scan(&services, &project_dir, &env, op)?;
                            *kept.lock().map_err(|_| "The comparison was lost.")? = Some(scan);
                            Ok(())
                        },
                        move |outcome, cx| {
                            let _ = weak.update(cx, |p, cx| match outcome {
                                Outcome::Finished => {
                                    let entries = found.lock().ok().and_then(|mut f| f.take()).map(|s| s.entries).unwrap_or_default();
                                    p.set_scan(&env_id, entries, now_text(), cx);
                                }
                                Outcome::Cancelled => p.set_status(Status::NotCompared, cx),
                                Outcome::Failed(e) => p.set_status(Status::Failed(e.clone()), cx),
                            });
                        },
                        cx,
                    )
                });
            }
            SourceControlEvent::Transfer { project_dir, env, items, title } => {
                // The preview first: every file and what happens to it; confirmed, nothing is asked again.
                let preview = dnnm_application::source_control::plan::preview(&env, &items);
                let dialog_title = format!("{title} - preview");
                let shown_env = env.clone();
                let view = cx.new(|_| PreviewDialog::new(dialog_title, shown_env, preview));
                self.show_dialog(
                    view,
                    move |ws, (): (), _, cx| {
                        let (services, project_dir, env, items) =
                            (ws.store.read(cx).services(), project_dir.clone(), env.clone(), items.clone());
                        let weak = weak.clone();
                        let _ = weak.update(cx, |p, cx| p.set_status(Status::Working(format!("{title}…")), cx));
                        ws.store.update(cx, |s, cx| {
                            s.run_remote(
                                format!("{title} - {}", env.label()),
                                move |op| wiring::transfer(&services, &project_dir, &env, &items, true, op).map(|_| ()),
                                move |_, cx| {
                                    // Whatever happened, the list says what is so now.
                                    let _ = weak.update(cx, |p, cx| p.refresh(cx));
                                },
                                cx,
                            )
                        });
                    },
                    window,
                    cx,
                );
            }
            SourceControlEvent::EditEnvironment { env, is_new } => {
                let has_secret = !is_new && wiring::has_secret(&env);
                let view = cx.new(|cx| EnvironmentDialog::new(env, is_new, has_secret, window, cx));
                self.show_dialog(
                    view,
                    move |ws, edit: EnvironmentEdit, _, cx| {
                        let services = ws.store.read(cx).services();
                        match wiring::save_environment(&services, &edit.env, edit.secret.as_deref()) {
                            Ok(()) => {
                                let id = edit.env.id.clone();
                                let _ = weak.update(cx, |p, cx| p.reload_environments(Some(id), cx));
                                ws.store.update(cx, |s, cx| {
                                    s.toast(ToastKind::Success, format!("Saved the server {}", edit.env.name), false, cx)
                                });
                            }
                            Err(e) => ws.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx)),
                        }
                    },
                    window,
                    cx,
                );
            }
            SourceControlEvent::DeleteEnvironment(env) => {
                let answer = self.store.update(cx, |s, cx| {
                    s.ask(
                        format!(
                            "Delete the server {} ({})? Its records of what was synced and its history go too, for every project; nothing on the server or in a project is touched.",
                            env.name, env.host
                        ),
                        vec!["Delete".into(), "Cancel".into()],
                        true,
                        cx,
                    )
                });
                let store = self.store.downgrade();
                cx.spawn(async move |_, cx| {
                    let chosen = cx.background_executor().spawn(async move { answer.recv().ok().flatten() }).await;
                    if chosen != Some(0) {
                        return;
                    }
                    let result = wiring::delete_environment(&services, &env);
                    let _ = cx.update(|cx| {
                        let _ = weak.update(cx, |p, cx| p.reload_environments(None, cx));
                        if let Err(e) = result {
                            let _ = store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                        }
                    });
                })
                .detach();
            }
            SourceControlEvent::Merge { project_dir, env, path } => {
                let title = format!("Merge {path} with {}", env.label());
                let written = Arc::new(Mutex::new(None));
                let kept = written.clone();
                let dir = project_dir.clone();
                let store = self.store.downgrade();
                self.store.update(cx, |s, cx| {
                    s.run_remote(
                        title,
                        move |op| {
                            let file = wiring::start_merge(&services, &project_dir, &env, &path, op)?;
                            *kept.lock().map_err(|_| "The merge was lost.")? = Some(file);
                            Ok(())
                        },
                        move |outcome, cx| {
                            if *outcome != Outcome::Finished {
                                return;
                            }
                            let file = written.lock().ok().and_then(|mut w| w.take());
                            if let Some(file) = file
                                && let Err(e) = wiring::open_for_merge(&file, &dir)
                            {
                                let _ = store.update(cx, |s, cx| s.toast(ToastKind::Warning, e, false, cx));
                            }
                            let _ = weak.update(cx, |p, cx| p.local_changed(cx));
                        },
                        cx,
                    )
                });
            }
            SourceControlEvent::Watch(folder) => self.watch_project(folder, page, cx),
            SourceControlEvent::Chosen { project, env } => self.store.update(cx, |s, cx| {
                s.update_settings(
                    |st| {
                        st.set(PROJECT_KEY, project.unwrap_or_default());
                        st.set(SERVER_KEY, env.unwrap_or_default());
                    },
                    cx,
                )
            }),
            SourceControlEvent::OpenFolder(project) => {
                let folder = wiring::server_backups(&project.to_string_lossy());
                if let Err(e) = std::fs::create_dir_all(&folder).map_err(|e| e.to_string()).and_then(|_| {
                    dnnm_infrastructure::shell::open_folder(&folder)
                }) {
                    self.store.update(cx, |s, cx| s.toast(ToastKind::Error, e, false, cx));
                }
            }
        }
    }
}
