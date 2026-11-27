//! Check for Updates (the gear menu, Settings → About, and at the start when `updates.checkAtStart` is on): asks GitHub
//! for the newest release and says what it found - in a toast, and on Settings → About - and shows **Update** in the
//! title bar while there is a newer one.
//!
//! **Update** downloads the release's package for this system and checks it (size and GitHub's SHA-256). On Windows,
//! for the DNN Manager the .msi installed, the update's helper (a copy of this exe) waits for DNN Manager to close,
//! runs msiexec and starts DNN Manager again; the next start says how it went. On macOS and Linux - and for a
//! Windows build the .msi didn't install - the package is opened for you to install (the .dmg, the .deb), or the
//! release page when there is no package to download.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{Context, WeakEntity, Window};

use dnnm_application::operation::Outcome;
use dnnm_dialogs::about::{Health, UpdateStatus};
use dnnm_dialogs::settings_page::SettingsPage;
use dnnm_system::updates;

use super::Workspace;
use crate::store::ToastKind;

/// Where an update is downloaded to and where its helper, plan, log and result are kept.
fn update_folder() -> Result<PathBuf, String> {
    Ok(dnnm_sql::dirs::temp()?.join("update"))
}

/// The helper's result, read by the DNN Manager it starts.
fn result_file() -> Option<PathBuf> {
    update_folder().ok().map(|f| f.join("result.json"))
}

/// Whether Update installs the .msi itself: on Windows, for the DNN Manager the .msi installed, in a release build.
fn installs_itself() -> bool {
    cfg!(windows)
        && !cfg!(debug_assertions)
        && std::env::current_exe().is_ok_and(|exe| updates::installed_by_msi(&exe))
}

/// About asks again only after this (`AboutInfo`: "unless asked within 10 minutes").
const ASK_AGAIN: Duration = Duration::from_secs(10 * 60);

fn current() -> updates::Version {
    // A pre-release's version has a suffix (2.6.0-rc1): compared by its numbers, as the tags are.
    updates::try_parse_version(super::help::VERSION)
        .unwrap_or(updates::Version { major: 0, minor: 0, build: None, revision: None })
}

impl Workspace {
    /// Asks GitHub; `toast`: say the result in a toast (Check for Updates…), else only on About (`page`).
    pub(crate) fn check_for_updates(&mut self, page: Option<WeakEntity<SettingsPage>>, toast: bool, cx: &mut Context<Self>) {
        let recent = self.update_checked.is_some_and(|at| at.elapsed() < ASK_AGAIN);
        if !toast && recent {
            if let Some(page) = page {
                let status = self.update_status.clone();
                let _ = page.update(cx, |p, cx| p.set_update_status(status, cx));
            }
            return;
        }
        self.update_status = UpdateStatus { text: "Checking for updates…".into(), health: Health::None, problem: None };
        if let Some(page) = &page {
            let status = self.update_status.clone();
            let _ = page.update(cx, |p, cx| p.set_update_status(status, cx));
        }
        cx.spawn(async move |this, cx| {
            let latest = cx.background_executor().spawn(async { updates::get_latest(updates::LATEST_RELEASE_URL) }).await;
            let current = current();
            let (status, available) = match latest {
                Ok(release) if updates::is_newer(&release.version, &current) => (
                    UpdateStatus {
                        text: format!("Update available - {}: Update in the title bar installs it", release.tag),
                        health: Health::Warning,
                        problem: None,
                    },
                    Some(release),
                ),
                Ok(_) => (
                    UpdateStatus { text: format!("Up to date - v{current} is the newest release"), health: Health::Ok, problem: None },
                    None,
                ),
                Err(e) => (
                    UpdateStatus { text: "Unable to reach GitHub".into(), health: Health::Unknown, problem: Some(e) },
                    None,
                ),
            };
            let _ = this.update(cx, |ws, cx| {
                ws.update_checked = Some(Instant::now());
                ws.update_status = status.clone();
                ws.update_available = available;
                if let Some(page) = page {
                    let status = status.clone();
                    let _ = page.update(cx, |p, cx| p.set_update_status(status, cx));
                }
                if toast {
                    let kind = if status.health == Health::Unknown { ToastKind::Warning } else { ToastKind::Success };
                    ws.store.update(cx, |s, cx| s.toast(kind, status.text, false, cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// **Update**: downloads the newer release's package and installs it ([module docs](self)).
    pub(crate) fn install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(release) = self.update_available.clone() else { return };
        let Some(asset) = release.package_here().cloned() else {
            // No package for this system: the release page lists what there is.
            self.open_link(&release.page_url, cx);
            return;
        };
        let Ok(folder) = update_folder() else {
            self.open_link(&release.page_url, cx);
            return;
        };
        let downloaded = Arc::new(Mutex::new(None::<PathBuf>));
        let kept = downloaded.clone();
        let title = format!("Update DNN Manager to {}", release.tag);
        let workspace = cx.entity().downgrade();
        let handle = window.window_handle();
        let to = release.clone();
        self.store.update(cx, |s, cx| {
            s.run_remote(
                title,
                move |op| {
                    op.report.step(&format!("Download {}", asset.name));
                    let cancel = op.cancel.clone();
                    let mut last = -1i64;
                    let mut progress = |share: f64| {
                        // A line every 10 %: the Output shows how far it is without a line per block.
                        let tenth = (share * 10.0).floor() as i64;
                        if tenth > last {
                            last = tenth;
                            op.report.info(&format!("{} %", tenth * 10));
                        }
                    };
                    let path = updates::download(&release, &asset, &folder, &mut progress, &cancel)?;
                    op.report.success(&format!("Downloaded and checked: {} (size and SHA-256 as GitHub lists them)", asset.name));
                    *kept.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                    Ok(())
                },
                move |outcome, cx| {
                    let path = downloaded.lock().unwrap_or_else(|e| e.into_inner()).take();
                    let (Outcome::Finished, Some(path)) = (outcome, path) else { return };
                    let _ = handle.update(cx, |_, window, cx| {
                        let _ = workspace.update(cx, |ws, cx| ws.apply_update(&to, path, window, cx));
                    });
                },
                cx,
            )
        });
    }

    /// The package is downloaded and checked: installed by the helper (DNN Manager closes and opens again), or opened
    /// for the user to install.
    fn apply_update(&mut self, release: &updates::AppRelease, package: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !installs_itself() {
            cx.open_with_system(&package);
            let text = format!(
                "{} is downloaded and opened - install it from there, then start DNN Manager again.",
                package.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            );
            self.store.update(cx, |s, cx| s.toast(ToastKind::Success, text, false, cx));
            return;
        }
        match self.start_update_helper(release, package) {
            Ok(()) => {
                log::info!("Updating to {}: DNN Manager closes and the update's helper installs it.", release.tag);
                self.quit(window, cx);
            }
            Err(e) => {
                let text = format!("The update couldn't be started: {e}");
                self.store.update(cx, |s, cx| s.toast(ToastKind::Error, text, false, cx));
            }
        }
    }

    #[cfg(windows)]
    fn start_update_helper(&self, release: &updates::AppRelease, package: PathBuf) -> Result<(), String> {
        use dnnm_system::update_helper::{UpdatePlan, failed_log_path, start_helper};
        let folder = update_folder()?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let asset = release.package_here().ok_or("the release has no package for this system")?;
        let result = folder.join("result.json");
        let _ = std::fs::remove_file(&result);
        let plan = UpdatePlan {
            kind: updates::UpdateKind::Msi,
            package_sha256: asset.sha256.clone().unwrap_or_default(),
            package_size: asset.size,
            package,
            app_exe: exe.clone(),
            all_users: true,
            wait_for_process_id: std::process::id(),
            from_version: super::help::VERSION.to_string(),
            to_version: release.version.to_string(),
            log_file: folder.join("update.log"),
            result_file: result,
            failed_log_file: Some(failed_log_path(
                &dnnm_logs::catalog::logs_directory(),
                &release.version.to_string(),
            )),
        };
        start_helper(&plan, &exe, &folder.join("helper")).map_err(|e| e.to_string())
    }

    #[cfg(not(windows))]
    fn start_update_helper(&self, _: &updates::AppRelease, _: PathBuf) -> Result<(), String> {
        Err("DNN Manager installs updates itself only on Windows".into())
    }

    /// At the start: how the update before it went, when the helper started this DNN Manager.
    #[cfg(windows)]
    pub(crate) fn report_update_result(&mut self, cx: &mut Context<Self>) {
        let Some(file) = result_file() else { return };
        let Some(result) = dnnm_system::update_helper::UpdateResult::try_read(Some(&file)) else { return };
        let _ = std::fs::remove_file(&file);
        let (kind, text) = if result.installed {
            (ToastKind::Success, format!("{} You're on version {}.", result.message, super::help::VERSION))
        } else {
            let log = result.log_file.map(|l| format!(" The log: {}", l.display())).unwrap_or_default();
            (ToastKind::Error, format!("The update didn't install: {}{log}", result.message))
        };
        self.store.update(cx, |s, cx| s.toast(kind, text, false, cx));
    }

    #[cfg(not(windows))]
    pub(crate) fn report_update_result(&mut self, _: &mut Context<Self>) {}
}
