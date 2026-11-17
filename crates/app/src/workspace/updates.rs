//! Check for Updates (the gear menu, Settings → About, and at the start when `updates.checkAtStart` is on): asks GitHub
//! for the newest release and says what it found - in a toast, and on Settings → About. A newer release is installed
//! by its package (.msi, .dmg, .deb) from the release page; DNN Manager doesn't replace itself.

use std::time::{Duration, Instant};

use gpui::{Context, WeakEntity};

use dnnm_dialogs::about::{Health, UpdateStatus};
use dnnm_dialogs::settings_page::SettingsPage;
use dnnm_system::updates;

use super::Workspace;
use crate::store::ToastKind;

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
                        text: format!("Update available - {}: download its package from {}", release.tag, release.page_url),
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
}
