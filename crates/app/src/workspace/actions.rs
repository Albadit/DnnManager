//! A project's edits (`ProjectEdits`, `ProjectMenu`): Rename, Edit host names, Edit app pool, Change connection,
//! Change host password and Upgrade DNN - each reads what its dialog needs off the UI thread, shows the dialog, and runs
//! its use case as an operation with its success toast.

use std::sync::Arc;

use gpui::{AppContext, Context, Window};

use dnnm_application::use_cases::change_database_connection::ChangeConnectionRequest;
use dnnm_application::use_cases::change_host_password::HostPasswordRequest;
use dnnm_application::use_cases::upgrade_dnn::UpgradeDnnRequest;
use dnnm_dialogs::database_connection::DatabaseConnectionDialog;
use dnnm_dialogs::deployment_export::DeploymentExportDialog;
use dnnm_dialogs::input::InputDialog;
use dnnm_dialogs::edit_app_pool::AppPoolDialog;
use dnnm_dialogs::edit_bindings::BindingsDialog;
use dnnm_dialogs::host_password::HostPasswordDialog;
use dnnm_dialogs::rename_project::RenameProjectDialog;
use dnnm_dialogs::upgrade_dnn::UpgradeDnnDialog;
use dnnm_wiring::backups::{self, texts as backup_texts};
use dnnm_wiring::{
    clone_project as clone, export_for_deployment as deployment, export_project, restore_backup,
};
use dnnm_wiring::{
    change_database_connection as connection, change_host_password as host, edit_app_pool as pool,
    edit_bindings as bindings, rename_project as rename, upgrade_dnn as upgrade,
};

use super::Workspace;
use crate::pages::projects::ProjectRun;
use crate::store::ToastKind;

/// What can be done to a project from its menu, its Details and the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAction {
    Rename,
    EditBindings,
    EditAppPool,
    ChangeConnection,
    ChangeHostPassword,
    UpgradeDnn,
    /// "Clone…": a copy under a name asked for (`ProjectMenu.Clone`).
    Clone,
    /// Export ▸ "For deployment…" (`ProjectEdits.ExportForDeployment`).
    ExportForDeployment,
}

impl ProjectAction {
    /// The menu's and the Details' text for it.
    pub fn label(self) -> &'static str {
        match self {
            ProjectAction::Rename => rename::MENU_ITEM,
            ProjectAction::EditBindings => bindings::BUTTON,
            ProjectAction::EditAppPool => pool::BUTTON,
            ProjectAction::ChangeConnection => connection::CHANGE_CONNECTION_BUTTON,
            ProjectAction::ChangeHostPassword => host::CHANGE_HOST_PASSWORD_BUTTON,
            ProjectAction::UpgradeDnn => upgrade::texts::UPGRADE_DNN,
            ProjectAction::Clone => clone::CLONE_MENU_ITEM,
            ProjectAction::ExportForDeployment => backup_texts::FOR_DEPLOYMENT,
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            ProjectAction::Rename => rename::BUTTON_TOOLTIP,
            ProjectAction::EditBindings => bindings::BUTTON_TOOLTIP,
            ProjectAction::EditAppPool => pool::BUTTON_TOOLTIP,
            ProjectAction::ChangeConnection => connection::CHANGE_CONNECTION_TOOLTIP,
            ProjectAction::ChangeHostPassword => host::CHANGE_HOST_PASSWORD_TOOLTIP,
            ProjectAction::UpgradeDnn => upgrade::texts::BUTTON_TOOLTIP,
            // WPF's Clone… has no tooltip; the menu leaves it out.
            ProjectAction::Clone => "",
            ProjectAction::ExportForDeployment => backup_texts::FOR_DEPLOYMENT_TOOLTIP,
        }
    }
}

impl Workspace {
    /// Starts an edit of a project: its dialog, then its operation.
    pub(crate) fn project_action(
        &mut self,
        action: ProjectAction,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.read(cx);
        let Some(p) = store.projects.iter().find(|p| p.name == name).cloned() else { return };
        // `ProjectEdits.CanEdit`: idle, and the folder there.
        if !rename::can_edit(store.is_busy(), &p.directory) {
            return;
        }
        let services = store.services();
        let weak = cx.entity().downgrade();
        let error = |ws: &mut Workspace, text: String, cx: &mut Context<Workspace>| {
            ws.store.update(cx, |s, cx| s.toast(ToastKind::Error, text, false, cx));
        };
        match action {
            ProjectAction::Rename => {
                cx.spawn_in(window, async move |_, cx| {
                    let (s, n, d, b) = (services.clone(), name.clone(), p.directory.clone(), p.site.bindings.clone());
                    let facts = cx.background_executor().spawn(async move { rename::dialog_facts(&s, &n, &d, &b) }).await;
                    let _ = weak.update_in(cx, |ws, window, cx| {
                        let view = cx.new(|cx| RenameProjectDialog::new(&name, p.directory.clone(), facts, window, cx));
                        ws.show_dialog(
                            view,
                            move |ws, request: dnnm_application::use_cases::rename_project::RenameProjectRequest, _, cx| {
                                let title = rename::title(&request.site_name, &request.new_name);
                                let toast = rename::done_toast(&request.site_name, &request.new_name);
                                let services = ws.store.read(cx).services();
                                ws.store.update(cx, |s, cx| {
                                    s.run_changing(title, Some(toast), move |op| rename::rename_project(&services, request, op), cx)
                                });
                            },
                            window,
                            cx,
                        );
                    });
                })
                .detach();
            }
            ProjectAction::EditBindings => {
                let (http, others) = dnnm_application::use_cases::edit_bindings::split_bindings(&p.site.bindings);
                let view = cx.new(|cx| BindingsDialog::new(&name, p.directory.clone(), http, others, window, cx));
                self.show_dialog(
                    view,
                    move |ws, request: dnnm_application::use_cases::edit_bindings::EditBindingsRequest, _, cx| {
                        let title = bindings::title(&request.site_name);
                        let toast = bindings::done_toast(&request.site_name, &request.bindings);
                        let services = ws.store.read(cx).services();
                        ws.store.update(cx, |s, cx| {
                            s.run_changing(title, Some(toast), move |op| bindings::edit_bindings(&services, request, op), cx)
                        });
                    },
                    window,
                    cx,
                );
            }
            ProjectAction::EditAppPool => {
                cx.spawn_in(window, async move |_, cx| {
                    let (s, n) = (services.clone(), name.clone());
                    let current = cx.background_executor().spawn(async move { pool::current_pool(&s, &n) }).await;
                    let _ = weak.update_in(cx, |ws, window, cx| match current {
                        Err(e) => error(ws, e, cx),
                        Ok((pool_name, settings)) => {
                            let view = cx.new(|cx| {
                                AppPoolDialog::new(&name, p.directory.clone(), &pool_name, settings, window, cx)
                            });
                            ws.show_dialog(
                                view,
                                move |ws, request: dnnm_application::use_cases::edit_app_pool::EditAppPoolRequest, _, cx| {
                                    let title = pool::title(&request.site_name);
                                    let toast = pool::done_toast(&request.site_name);
                                    let services = ws.store.read(cx).services();
                                    ws.store.update(cx, |s, cx| {
                                        s.run_changing(title, Some(toast), move |op| pool::edit_app_pool(&services, request, op), cx)
                                    });
                                },
                                window,
                                cx,
                            );
                        }
                    });
                })
                .detach();
            }
            ProjectAction::ChangeConnection => {
                cx.spawn_in(window, async move |_, cx| {
                    let (s, n, d) = (services.clone(), name.clone(), p.directory.clone());
                    let (current, login, defaults) = cx
                        .background_executor()
                        .spawn(async move {
                            (
                                connection::database_of(&s.settings, &d),
                                s.iis.app_pool_identity(&n),
                                connection::connection_defaults(&s.settings),
                            )
                        })
                        .await;
                    let _ = weak.update_in(cx, |ws, window, cx| {
                        let tester: dnnm_dialogs::database_connection::ConnectionTester =
                            Arc::new(|c, l| connection::test_connection(c, l));
                        let view =
                            cx.new(|cx| DatabaseConnectionDialog::new(&name, current, &login, defaults, tester, window, cx));
                        let directory = p.directory.clone();
                        ws.show_dialog(
                            view,
                            move |ws, chosen: dnnm_application::use_cases::database_connection::DatabaseConnection, _, cx| {
                                let title = connection::change_connection_title(&name);
                                let toast = connection::change_connection_done(&name, &chosen);
                                let request = ChangeConnectionRequest {
                                    site_name: name.clone(),
                                    directory: directory.clone(),
                                    connection: chosen,
                                };
                                let services = ws.store.read(cx).services();
                                ws.store.update(cx, |s, cx| {
                                    s.run_changing(
                                        title,
                                        Some(toast),
                                        move |op| connection::change_database_connection(&services, request, op),
                                        cx,
                                    )
                                });
                            },
                            window,
                            cx,
                        );
                    });
                })
                .detach();
            }
            ProjectAction::ChangeHostPassword => {
                cx.spawn_in(window, async move |_, cx| {
                    let (s, d) = (services.clone(), p.directory.clone());
                    let hosts = cx
                        .background_executor()
                        .spawn(async move {
                            let config = dnnm_infrastructure::web_config::read_database_connection(&d.join("web.config"))?;
                            host::host_accounts(&config, &s.settings)
                        })
                        .await;
                    let _ = weak.update_in(cx, |ws, window, cx| match hosts {
                        Err(e) => error(ws, e, cx),
                        Ok(hosts) => {
                            let view = cx.new(|cx| HostPasswordDialog::new(&name, hosts, window, cx));
                            let directory = p.directory.clone();
                            ws.show_dialog(
                                view,
                                move |ws, choice: dnnm_dialogs::host_password::HostPasswordChoice, _, cx| {
                                    let title = host::change_host_password_title(&name);
                                    let toast = host::change_host_password_done(&name, &choice.user_name);
                                    let request = HostPasswordRequest {
                                        site_name: name.clone(),
                                        directory: directory.clone(),
                                        user_name: choice.user_name,
                                        new_password: choice.password,
                                    };
                                    let services = ws.store.read(cx).services();
                                    ws.store.update(cx, |s, cx| {
                                        s.run_changing(
                                            title,
                                            Some(toast),
                                            move |op| host::change_host_password(&services, request, op),
                                            cx,
                                        )
                                    });
                                },
                                window,
                                cx,
                            );
                        }
                    });
                })
                .detach();
            }
            ProjectAction::UpgradeDnn => {
                let current = p.dnn_version.clone().unwrap_or_default();
                let apis = upgrade::release_apis(&services.settings);
                let load_services = services.clone();
                let load: dnnm_dialogs::upgrade_dnn::LoadReleases =
                    Arc::new(move |api| upgrade::list_releases(&load_services, api));
                // The plan may ask to sign in to the database's server - a question shown like an operation's.
                let prompt = self.store.update(cx, |s, cx| s.detached_prompt(cx));
                let (plan_services, plan_site, plan_dir) = (services.clone(), name.clone(), p.directory.clone());
                let plan: dnnm_dialogs::upgrade_dnn::PlanUpgrade = Arc::new(move |target| {
                    upgrade::plan_upgrade(&plan_services, &plan_site, &plan_dir, target, &*prompt)
                });
                let view = cx.new(|cx| UpgradeDnnDialog::new(&name, &current, apis, load, plan, window, cx));
                let directory = p.directory.clone();
                self.show_dialog(
                    view,
                    move |ws, choice: dnnm_dialogs::upgrade_dnn::UpgradeChoice, _, cx| {
                        let title = upgrade::texts::operation_title(&name, &choice.version);
                        let toast = upgrade::texts::done(&name, &choice.version);
                        let request = UpgradeDnnRequest {
                            site_name: name.clone(),
                            directory: directory.clone(),
                            release_api_url: choice.release_api_url,
                            version: choice.tag,
                        };
                        let services = ws.store.read(cx).services();
                        ws.store.update(cx, |s, cx| {
                            s.run_changing(title, Some(toast), move |op| upgrade::upgrade_dnn(&services, request, op), cx)
                        });
                    },
                    window,
                    cx,
                );
            }
            ProjectAction::Clone => {
                // The name asked for: a valid project name, no project folder or IIS site of that name yet.
                let store = self.store.read(cx);
                let folder = std::path::PathBuf::from(store.settings.base_directory());
                let sites: Vec<String> = store.projects.iter().map(|p| p.name.clone()).collect();
                let problem = move |n: &str| clone::clone_name_problem(n, &folder, &sites);
                let suggested = clone::suggested_clone_name(&name, &problem);
                let validate: dnnm_dialogs::input::Validate = Box::new(problem);
                let view = cx.new(|cx| {
                    InputDialog::new(clone::clone_question(&name), Some(&suggested), clone::CLONE_OK, Some(validate), window, cx)
                });
                let source = p.directory.clone();
                self.show_dialog(
                    view,
                    move |ws, target: String, _, cx| {
                        let title = clone::clone_title(&name, &target);
                        let toast = clone::clone_done(&name, &target);
                        let request = clone::clone_request(&source, &target);
                        let services = ws.store.read(cx).services();
                        ws.store.update(cx, |s, cx| {
                            s.run_changing(title, Some(toast), move |op| clone::clone_project(&services, request, op), cx)
                        });
                    },
                    window,
                    cx,
                );
            }
            ProjectAction::ExportForDeployment => {
                cx.spawn_in(window, async move |_, cx| {
                    let (s, d) = (services.clone(), p.directory.clone());
                    let has_database =
                        cx.background_executor().spawn(async move { deployment::has_database(&s, &d) }).await;
                    let _ = weak.update_in(cx, |ws, window, cx| {
                        let folder = deployment::default_folder(&name, backups::now());
                        let view = cx.new(|cx| {
                            DeploymentExportDialog::new(&name, &p.directory, &folder, has_database, window, cx)
                        });
                        ws.show_dialog(
                            view,
                            move |ws,
                                  request: dnnm_application::use_cases::export_for_deployment::ExportForDeploymentRequest,
                                  _,
                                  cx| {
                                let title = backup_texts::deployment_title(&name);
                                let toast = backup_texts::deployment_done(&name);
                                let services = ws.store.read(cx).services();
                                ws.store.update(cx, |s, cx| {
                                    s.run_changing(
                                        title,
                                        Some(toast),
                                        move |op| deployment::export_for_deployment(&services, request, op),
                                        cx,
                                    )
                                });
                            },
                            window,
                            cx,
                        );
                    });
                })
                .detach();
            }
        }
    }

    /// Runs what a project's menu starts without a dialog: a backup (Export ▸), or putting one back (Restore backup ▸,
    /// which asks first).
    pub(crate) fn project_run(&mut self, run: ProjectRun, name: String, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let Some(p) = store.projects.iter().find(|p| p.name == name).cloned() else { return };
        let services = store.services();
        let (title, toast, work): (String, String, Box<dyn FnOnce(&mut dnnm_application::operation::Operation) -> Result<(), String> + Send>) =
            match run {
                ProjectRun::Backup(kind) => {
                    let (request, folder) = export_project::backup_request(&name, &p.directory, kind, backups::now());
                    (
                        backup_texts::backup_title(&name),
                        backup_texts::backup_done(&name, &folder),
                        Box::new(move |op| export_project::export_project(&services, request, op)),
                    )
                }
                ProjectRun::Restore(backup) => {
                    let (title, toast) =
                        (restore_backup::restore_title(&name, &backup), restore_backup::restore_done(&name, &backup));
                    let choice = restore_backup::RestoreChoice { site_name: name.clone(), directory: p.directory.clone(), backup };
                    (title, toast, Box::new(move |op| restore_backup::restore_backup(&services, choice, op)))
                }
            };
        self.store.update(cx, |s, cx| s.run_changing(title, Some(toast), work, cx));
    }
}
