//! Running in the background and quitting (`MainWindow.FollowKeepRunning`, `OnClosing`, `Quit`): with "Keep DNN
//! Manager running when you close the window" on, the notification area has DNN Manager's icon and closing the window
//! hides it - an operation, the terminals and what was typed go on. Quitting asks first while an operation runs (it is
//! cancelled and undone, then DNN Manager closes) or a password in Settings isn't saved.
//!
//! Deviation: WPF offers "Quit now" when the undo takes over 20 seconds; here quitting waits for the undo.

use gpui::{Context, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use dnnm_tray::{TrayEvent, TrayIcon};

use super::Workspace;
use super::modals::Modal;
use crate::store::ToastKind;

fn hwnd(window: &Window) -> Option<isize> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Windows' Move pointer for the sash corner (`dnnm_system::move_pointer`): its subclass on this window, at the start.
pub(super) fn install_move_pointer(window: &Window) {
    #[cfg(windows)]
    if let Some(hwnd) = hwnd(window) {
        dnnm_system::move_pointer::install(hwnd);
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// The Move pointer on (over the sash corner, or while it is dragged) or off.
pub(super) fn move_pointer(on: bool) {
    #[cfg(windows)]
    dnnm_system::move_pointer::set(on);
    #[cfg(not(windows))]
    let _ = on;
}

impl Workspace {
    /// The icon there while the setting is on - from the start, and as soon as it is saved.
    pub(crate) fn follow_keep_running(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.store.read(cx).settings.get("window.keepRunningWhenClosed").is_none_or(|v| v.eq_ignore_ascii_case("true"));
        if wanted == self.tray.is_some() {
            return;
        }
        if !wanted {
            self.tray = None;
            return;
        }
        // Its events come on the icon's own thread.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<TrayEvent>();
        match TrayIcon::new(dnnm_tray::DEFAULT_TIP, move |event| {
            let _ = tx.unbounded_send(event);
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => {
                log::warn!("The notification area icon couldn't be added: {e}");
                return;
            }
        }
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while let Some(event) = rx.next().await {
                let alive = this.update_in(cx, |ws, window, cx| match event {
                    TrayEvent::Open => ws.show_from_background(window, cx),
                    TrayEvent::Quit => ws.quit(window, cx),
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// The window back from the notification area, or from a second start of DNN Manager.
    pub(crate) fn show_from_background(&mut self, window: &mut Window, _: &mut Context<Self>) {
        self.hidden = false;
        if let Some(hwnd) = hwnd(window) {
            dnnm_tray::show_window(hwnd, true);
        }
        window.activate_window();
    }

    /// Quits - from the icon's menu or the palette; the window comes back first when there is a question to ask.
    pub(crate) fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unsaved_password =
            matches!(&self.settings, Some(Modal::Settings(page, _)) if page.read(cx).has_unsaved_password());
        if self.store.read(cx).is_busy() || unsaved_password {
            self.show_from_background(window, cx);
        }
        self.quitting = true;
        if self.close_requested(window, cx) {
            cx.quit();
        }
    }

    /// The window's ✕ (`OnClosing`): true when DNN Manager may close now.
    pub(crate) fn close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Running in the background: the window hides, without a Windows notification - the icon says it is there.
        if self.tray.is_some() && !self.quitting {
            self.save_workspace(window, cx);
            if let Some(hwnd) = hwnd(window) {
                self.hidden = true;
                dnnm_tray::show_window(hwnd, false);
                return false;
            }
        }
        self.quitting = false;
        // Unsaved settings are kept for the next start - all but a password, so only that is asked about.
        let unsaved_password =
            matches!(&self.settings, Some(Modal::Settings(page, _)) if page.read(cx).has_unsaved_password());
        if unsaved_password && !self.password_may_go {
            self.ask(
                "You changed a password in the settings and didn't save it. The other unsaved settings are kept for the next start, but passwords aren't. Quit anyway?",
                &["Quit", "Stay"],
                false,
                window,
                cx,
                |ws, choice, window, cx| {
                    if choice == Some(0) {
                        ws.password_may_go = true;
                        ws.quit(window, cx);
                    }
                },
            );
            return false;
        }
        if let Some(running) = self.store.read(cx).running_title() {
            if self.quit_when_idle {
                return false;
            }
            let question = format!(
                "'{running}' is still running. Quit anyway? It is cancelled first, and what it did is put back - DNN Manager closes once that is done."
            );
            self.ask(question, &["Quit anyway", "Keep running"], false, window, cx, |ws, choice, _, cx| {
                if choice == Some(0) {
                    ws.quit_when_idle = true;
                    ws.store.update(cx, |s, cx| {
                        s.cancel_operation(cx);
                        s.toast(ToastKind::Warning, "DNN Manager closes once the operation has stopped.", false, cx);
                    });
                }
            });
            return false;
        }
        self.quit_cleanly(window, cx);
        true
    }

    /// What goes with DNN Manager: the workspace saved for the next start, the shells ended, the icon removed.
    pub(crate) fn quit_cleanly(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_workspace(window, cx);
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |t, cx| t.close_all(cx));
        }
        self.tray = None;
    }

    /// An operation failed while the window is hidden: a Windows notification from the icon says so.
    pub(crate) fn notice_failure(&mut self, title: &str, error: &str) {
        if self.hidden
            && let Some(tray) = &self.tray
        {
            tray.show_notice(&format!("{title} failed"), error);
        }
    }
}
