//! Settings and Troubleshoot over the page (`MainWindow.ShowModal` / `CloseModal`): the page dimmed behind them, a
//! click on it or Esc closing them - asking first when Settings has unsaved changes. What they ask the app to do
//! (save, set up IIS or the container, resets…) is done here.

use std::sync::Arc;

use gpui::{AnyView, AppContext, Context, Entity, SharedString, Subscription, Window};

use dnnm_dialogs::setting_rules::Category;
use dnnm_dialogs::settings_page::{Notice, SettingsEvent, SettingsInputs, SettingsPage, SettingsServices, SystemSignIn};
use dnnm_dialogs::troubleshoot_page::{self as troubleshoot, AppDataCleaner, TroubleshootEvent, TroubleshootPage};

use super::Workspace;
use crate::store::ToastKind;

/// What is open over the page.
pub(crate) enum Modal {
    // Each with its subscriptions: they end when it closes (kept on the workspace they would pile up, and a page's
    // store observer would go on running after it was gone).
    Settings(Entity<SettingsPage>, #[allow(dead_code, reason = "held: dropped, they end")] Vec<Subscription>),
    Troubleshoot(Entity<TroubleshootPage>, #[allow(dead_code, reason = "held: dropped, they end")] Vec<Subscription>),
}

impl Modal {
    pub(crate) fn view(&self) -> AnyView {
        match self {
            Modal::Settings(v, _) => v.clone().into(),
            Modal::Troubleshoot(v, _) => v.clone().into(),
        }
    }

    pub(crate) fn key(&self) -> &'static str {
        match self {
            Modal::Settings(_, _) => "Settings",
            Modal::Troubleshoot(_, _) => "Troubleshoot",
        }
    }
}

pub(crate) fn toast_kind(notice: Notice) -> ToastKind {
    match notice {
        Notice::Success => ToastKind::Success,
        Notice::Warning => ToastKind::Warning,
        Notice::Error => ToastKind::Error,
    }
}

/// Takes the container volume's old sa password (Set up docker-compose), on the operation's thread.
type AdoptPassword = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

const UNSAVED: &str = "The settings have unsaved changes. Close them and lose the changes?";

impl Workspace {
    /// A question of the window's own (`Dialogs.Confirm`): `then` gets the answer's index, None when closed.
    pub(crate) fn ask(
        &mut self,
        question: impl Into<SharedString>,
        answers: &[&str],
        danger: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Workspace, Option<usize>, &mut Window, &mut Context<Workspace>) + 'static,
    ) {
        let answers = answers.iter().map(|a| SharedString::from(a.to_string())).collect();
        let answer = self.store.update(cx, |s, cx| s.ask(question, answers, danger, cx));
        cx.spawn_in(window, async move |this, cx| {
            let choice = cx.background_executor().spawn(async move { answer.recv().unwrap_or(None) }).await;
            let _ = this.update_in(cx, |ws, window, cx| then(ws, choice, window, cx));
        })
        .detach();
    }

    /// An error dialog (`Dialogs.Error`).
    pub(crate) fn error_dialog(&mut self, text: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        self.ask(text, &["OK"], false, window, cx, |_, _, _, _| {});
    }

    /// Closes what is open over the page - asking first when Settings has unsaved changes - then runs `then`; not when
    /// the user chose to stay.
    pub(crate) fn close_modal_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) {
        let unsaved = matches!(&self.settings, Some(Modal::Settings(page, _)) if page.read(cx).has_unsaved_changes());
        if !unsaved {
            self.drop_modal(window, cx);
            then(self, window, cx);
            return;
        }
        self.ask(UNSAVED, &["Discard changes", "Stay"], false, window, cx, move |ws, choice, window, cx| {
            if choice == Some(0) {
                ws.drop_modal(window, cx);
                then(ws, window, cx);
            }
        });
    }

    pub(crate) fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal_then(window, cx, |_, _, _| {});
    }

    fn drop_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
        }
    }

    /// Settings over the page - on `category` when given; Ctrl+, again closes it (`settings.open`).
    pub(crate) fn open_settings_on(&mut self, category: Option<Category>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(Modal::Settings(page, _)) = &self.settings {
            match category {
                Some(category) => page.update(cx, |p, cx| p.show_category(category, cx)),
                None => self.close_modal(window, cx),
            }
            return;
        }
        self.close_modal_then(window, cx, move |ws, window, cx| {
            let store = ws.store.read(cx);
            let inputs = SettingsInputs {
                saved: store.settings.clone(),
                commands: super::commands::shortcut_commands(),
                services: SettingsServices::system(),
                offer_whats_new: !ws.is_new_user(cx),
                update: ws.update_status.clone(),
            };
            let page = cx.new(|cx| {
                let mut page = SettingsPage::new(inputs, window, cx);
                if let Some(category) = category {
                    page.show_category(category, cx);
                }
                page
            });
            let sub = cx.subscribe_in(&page, window, |ws, page, e: &SettingsEvent, window, cx| {
                ws.settings_event(page.clone(), e.clone(), window, cx)
            });
            ws.servers_changed(&page, cx);
            ws.settings = Some(Modal::Settings(page, vec![sub]));
            cx.notify();
        });
    }

    /// Troubleshoot over the page (`troubleshoot.open`).
    pub(crate) fn open_troubleshoot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if matches!(self.settings, Some(Modal::Troubleshoot(_, _))) {
            return;
        }
        self.close_modal_then(window, cx, |ws, window, cx| {
            let busy = ws.store.read(cx).running_title().map(|t| t.to_string());
            let page = cx.new(|cx| TroubleshootPage::new(AppDataCleaner::for_current_user(), busy, window, cx));
            let sub = cx.subscribe_in(&page, window, |ws, page, e: &TroubleshootEvent, window, cx| {
                ws.troubleshoot_event(page.clone(), e.clone(), window, cx)
            });
            // The buttons wait while an operation runs.
            let watch = cx.observe(&ws.store, {
                let page = page.downgrade();
                move |_, store, cx| {
                    let busy = store.read(cx).running_title().map(|t| t.to_string());
                    let _ = page.update(cx, |p, cx| p.set_busy(busy, cx));
                }
            });
            ws.settings = Some(Modal::Troubleshoot(page, vec![sub, watch]));
            cx.notify();
        });
    }

    fn settings_event(&mut self, page: Entity<SettingsPage>, e: SettingsEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            SettingsEvent::Save(next) | SettingsEvent::SaveAtOnce(next) => {
                let result = match self.store.read(cx).running_title() {
                    Some(running) => Err(format!("'{running}' is still running - save once it has finished.")),
                    None => self.store.update(cx, |s, cx| s.save_settings(next, cx)),
                };
                let ok = result.is_ok();
                page.update(cx, |p, cx| p.saved(result, cx));
                if ok {
                    self.settings_changed(window, cx);
                }
            }
            SettingsEvent::Close => self.close_modal(window, cx),
            SettingsEvent::ShowGuide => self.show_guide(window, cx),
            SettingsEvent::StartTour => self.close_modal_then(window, cx, |ws, window, cx| ws.start_tour(window, cx)),
            SettingsEvent::ShowWhatsNew => self.show_whats_new(window, cx),
            SettingsEvent::OpenUserGuide => self.open_link(dnnm_dialogs::help_menu::USER_GUIDE_URL, cx),
            SettingsEvent::EditUpgradePath => {
                let stops = dnnm_dialogs::setting_rules::upgrade_stops(&self.store.read(cx).settings);
                let view = cx.new(|cx| dnnm_dialogs::dnn_upgrade_path::DnnUpgradePathDialog::new(stops, window, cx));
                self.show_dialog(
                    view,
                    move |ws, stops: Vec<dnnm_dnn::upgrades::path::UpgradeStop>, _, cx| {
                        let mut next = ws.store.read(cx).settings.clone();
                        dnnm_dialogs::dnn_upgrade_path::set_upgrade_stops(&mut next, &stops);
                        match ws.store.update(cx, |s, cx| s.save_settings(next, cx)) {
                            Ok(()) => {
                                let saved = ws.store.read(cx).settings.clone();
                                page.update(cx, |p, cx| p.saved_elsewhere(saved, cx));
                                let text = dnnm_dialogs::dnn_upgrade_path::saved_toast(&stops);
                                ws.store.update(cx, |s, cx| s.toast(ToastKind::Success, text, false, cx));
                            }
                            Err(e) => {
                                let text = dnnm_dialogs::dnn_upgrade_path::not_saved(&e);
                                ws.store.update(cx, |s, cx| s.toast(ToastKind::Error, text, false, cx));
                            }
                        }
                    },
                    window,
                    cx,
                );
            }
            SettingsEvent::SetUpContainer => {
                let services = self.store.read(cx).services();
                // The volume's old password adopted: saved through the store, on the UI thread.
                let (tx, rx) = std::sync::mpsc::channel::<String>();
                let adopt: AdoptPassword = Arc::new(move |password: &str| {
                    tx.send(password.to_string()).map_err(|e| e.to_string())
                });
                let store = self.store.downgrade();
                let page = page.downgrade();
                self.store.update(cx, |s, cx| {
                    s.run_then(
                        dnnm_wiring::sql_container::SETUP_COMPOSE_TITLE.into(),
                        None,
                        move |op| {
                            dnnm_wiring::sql_container::setup_sql_container(
                                &services,
                                dnnm_wiring::sql_container::SetupSqlContainerRequest {
                                    adopt_password: Some(adopt),
                                },
                                op,
                            )
                        },
                        move |_, cx| {
                            let adopted = rx.try_iter().last();
                            let saved = adopted.and_then(|password| {
                                let protected = dnnm_system::secrets::protect(&password).ok()?;
                                store
                                    .update(cx, |s, cx| {
                                        let mut next = s.settings.clone();
                                        next.set(dnnm_system::secrets::SA_PASSWORD_KEY, &protected);
                                        s.save_settings(next, cx).ok().map(|_| s.settings.clone())
                                    })
                                    .ok()
                                    .flatten()
                            });
                            let _ = page.update(cx, |p, cx| p.container_set_up(saved, cx));
                        },
                        cx,
                    )
                });
            }
            SettingsEvent::SetUpIis => {
                let features = dnnm_dialogs::setting_rules::iis_features(&self.store.read(cx).settings);
                let page = page.downgrade();
                self.store.update(cx, |s, cx| {
                    s.run_then(
                        "Set up IIS".into(),
                        None,
                        move |op| dnnm_system::prereq::ensure_iis_features(&features, op),
                        move |_, cx| {
                            let _ = page.update(cx, |p, cx| p.iis_set_up(cx));
                        },
                        cx,
                    )
                });
            }
            SettingsEvent::InstallDocker => {
                let question = "Install Docker Desktop with winget? It downloads about 600 MB, and Windows may ask to enable WSL 2 or to restart afterwards.";
                let page = page.downgrade();
                self.ask(question, &["Install Docker Desktop", "Cancel"], false, window, cx, move |ws, choice, _, cx| {
                    if choice != Some(0) {
                        return;
                    }
                    ws.store.update(cx, |s, cx| {
                        s.run_then(
                            "Install Docker Desktop".into(),
                            None,
                            |op| dnnm_system::prereq::install_docker_desktop(op.report),
                            move |_, cx| {
                                let _ = page.update(cx, |p, cx| p.docker_changed(cx));
                            },
                            cx,
                        )
                    });
                });
            }
            SettingsEvent::EditIisFeatures(features) => {
                let view = cx.new(|cx| dnnm_dialogs::iis_features::IisFeaturesDialog::new(features, window, cx));
                self.show_dialog(
                    view,
                    move |_, features, _, cx| page.update(cx, |p, cx| p.save_iis_features(features, cx)),
                    window,
                    cx,
                );
            }
            SettingsEvent::ConfirmResetAllShortcuts => {
                self.ask(
                    "Put every keyboard shortcut back to its default?",
                    &["Reset all", "Cancel"],
                    false,
                    window,
                    cx,
                    move |_, choice, _, cx| {
                        if choice == Some(0) {
                            page.update(cx, |p, cx| p.reset_all_shortcuts(cx));
                        }
                    },
                );
            }
            SettingsEvent::CheckForUpdates => self.check_for_updates(Some(page.downgrade()), false, cx),
            SettingsEvent::AddServer => self.edit_server(page, None, window, cx),
            SettingsEvent::EditServer(env) => self.edit_server(page, Some(env), window, cx),
            SettingsEvent::DeleteServer(env) => self.delete_server(page, env, window, cx),
            SettingsEvent::ImportServers => self.import_servers(page, window, cx),
            SettingsEvent::ExportServers => self.export_servers(window, cx),
            SettingsEvent::Toast(notice, text) => self.store.update(cx, |s, cx| s.toast(toast_kind(notice), text, false, cx)),
            SettingsEvent::Error(text) => self.error_dialog(text, window, cx),
        }
    }

    fn troubleshoot_event(
        &mut self,
        page: Entity<TroubleshootPage>,
        e: TroubleshootEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The fixture is a UI check: nothing of the user's is cleaned up or reset.
        let changes = matches!(
            e,
            TroubleshootEvent::CleanUp { .. } | TroubleshootEvent::ResetSettings | TroubleshootEvent::FactoryReset
        );
        if changes && self.store.update(cx, |s, cx| s.fixture_refuses(cx)) {
            return;
        }
        match e {
            TroubleshootEvent::Close => self.close_modal(window, cx),
            TroubleshootEvent::Restart => self.restart(window, cx),
            TroubleshootEvent::CleanUp { question, kinds } => {
                self.ask(question, &["Clean up data", "Cancel"], true, window, cx, move |_, choice, window, cx| {
                    if choice != Some(0) {
                        return;
                    }
                    let cleaner = AppDataCleaner::for_current_user();
                    let page = page.downgrade();
                    cx.spawn_in(window, async move |ws, cx| {
                        let results = cx
                            .background_executor()
                            .spawn(async move { kinds.iter().map(|k| cleaner.clean(*k)).collect::<Vec<_>>() })
                            .await;
                        let Ok(Some((notice, text))) = page.update(cx, |p, cx| Some(p.cleaned_up(&results, cx))) else {
                            return;
                        };
                        let _ = ws.update(cx, |ws, cx| {
                            ws.store.update(cx, |s, cx| s.toast(toast_kind(notice), text, false, cx))
                        });
                    })
                    .detach();
                });
            }
            TroubleshootEvent::ResetSettings => {
                self.ask(
                    troubleshoot::RESET_SETTINGS_QUESTION,
                    &["Reset settings", "Cancel"],
                    true,
                    window,
                    cx,
                    |ws, choice, window, cx| {
                        if choice != Some(0) {
                            return;
                        }
                        let database = ws.store.read(cx).db().path().to_path_buf();
                        let new_sa = dnnm_wiring::setup_project::new_sql_password();
                        if let Err(e) = troubleshoot::reset_settings(&database, &new_sa) {
                            ws.error_dialog(format!("The settings could not be reset: {e}"), window, cx);
                            return;
                        }
                        let problems = troubleshoot::forget_secrets_and_start(
                            &dnnm_system::secrets::CredentialStore,
                            &SystemSignIn,
                        );
                        ws.store.update(cx, |s, cx| s.reload_settings(cx));
                        ws.settings_changed(window, cx);
                        match troubleshoot::reset_settings_outcome(&problems) {
                            (Notice::Error, text) => ws.error_dialog(text, window, cx),
                            (notice, text) => ws.store.update(cx, |s, cx| s.toast(toast_kind(notice), text, false, cx)),
                        }
                    },
                );
            }
            TroubleshootEvent::FactoryReset => {
                self.ask(
                    troubleshoot::FACTORY_RESET_QUESTION,
                    &["Reset to factory defaults", "Cancel"],
                    true,
                    window,
                    cx,
                    |ws, choice, window, cx| {
                        if choice != Some(0) {
                            return;
                        }
                        // The workspace isn't saved over what the reset removes.
                        ws.keep_workspace = false;
                        let new_sa = dnnm_wiring::setup_project::new_sql_password();
                        let result = troubleshoot::factory_reset(
                            &AppDataCleaner::for_current_user(),
                            &new_sa,
                            &dnnm_system::secrets::CredentialStore,
                            &SystemSignIn,
                        );
                        match result {
                            Err(e) => {
                                ws.keep_workspace = true;
                                ws.error_dialog(e, window, cx);
                            }
                            Ok(problems) if !problems.is_empty() => {
                                let text = troubleshoot::problems_text("DNN Manager is reset, except:", &problems);
                                ws.ask(text, &["OK"], false, window, cx, |ws, _, window, cx| ws.restart(window, cx));
                            }
                            Ok(_) => ws.restart(window, cx),
                        }
                    },
                );
            }
        }
    }

    /// Saved settings put to work beyond the store: the shortcuts, New project's defaults.
    fn settings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_keep_running(window, cx);
        let settings = self.store.read(cx).settings.clone();
        super::shortcuts::apply(&settings, cx);
        if let Some(setup) = self.setup.clone() {
            setup.update(cx, |p, cx| p.set_settings(settings, cx));
        }
    }

    /// Restarts DNN Manager (`AppRestart.Restart`): asks first while an operation runs, as quitting does; the new one
    /// starts with the same arguments, then this one quits.
    pub(crate) fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let go = |ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
            if ws.keep_workspace {
                ws.save_workspace(window, cx);
            }
            // `--after <pid>`: the new DNN Manager waits for this one to end before anything else - else it would find it
            // running and only bring its window forward.
            let started = std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .arg(crate::AFTER_ARGUMENT)
                    .arg(std::process::id().to_string())
                    .args(std::env::args_os().skip(1))
                    .spawn()
            });
            match started {
                Ok(_) => cx.quit(),
                Err(e) => {
                    ws.keep_workspace = true;
                    ws.error_dialog(format!("DNN Manager couldn't restart: {e}"), window, cx);
                }
            }
        };
        let Some(running) = self.store.read(cx).running_title() else {
            go(self, window, cx);
            return;
        };
        let question = format!("'{running}' is still running. Cancel it and restart DNN Manager?");
        self.ask(question, &["Cancel it and restart", "Keep running"], true, window, cx, move |ws, choice, window, cx| {
            if choice == Some(0) {
                ws.store.update(cx, |s, cx| s.cancel_operation(cx));
                go(ws, window, cx);
            } else {
                ws.keep_workspace = true;
            }
        });
    }
}
