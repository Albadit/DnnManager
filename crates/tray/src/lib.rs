//! DNN Manager's icon in the notification area (by the clock) - `Services/TrayIcon.cs`: there while "Keep DNN Manager
//! running when you close the window" is on (Settings → General). A click opens the window, a right-click offers Open
//! (bold) and Quit, and [`TrayIcon::show_notice`] shows a Windows notification for what happens while the window is
//! hidden (a failed operation: "`<title>` failed" with the error). It comes back when Explorer restarts
//! (`TaskbarCreated`).
//!
//! Made with `Shell_NotifyIcon` on a window of its own that is never shown, on a thread of its own with its message
//! loop - GPUI owns the UI thread's loop. The events reach the app through the callback given to [`TrayIcon::new`],
//! **called on the tray's thread**: the app passes them on to its UI thread (a channel its async task reads).
//! Dropping the [`TrayIcon`] removes the icon and ends the thread.
//!
//! Deviations from the C#: the menu is Windows' own (its default item is drawn bold), not one in the app's style; the
//! icon comes from `dnn.ico` built into this crate rather than from the exe's resources, which the Rust exe may not
//! carry. The window is a hidden top-level tool window, as in the C#: a message-only window wouldn't hear
//! `TaskbarCreated`, which is broadcast.

/// What the icon tells the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// The icon was clicked (or Enter / Space on it), or "Open DNN Manager" chosen on its menu.
    Open,
    /// "Quit DNN Manager" was chosen on its menu.
    Quit,
}

/// The menu's items, as the C# names them.
pub const OPEN_TEXT: &str = "Open DNN Manager";
pub const QUIT_TEXT: &str = "Quit DNN Manager";
/// The tooltip the WPF app gives it.
pub const DEFAULT_TIP: &str = "DNN Manager";

/// The app's icon, `src/DnnManager.Presentation/Assets/dnn.ico`.
pub const APP_ICO: &[u8] = include_bytes!("../../../assets/dnn.ico");

/// One image of an `.ico` file: its size in pixels and where its data is in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IcoImage {
    pub width: u32,
    pub height: u32,
    pub offset: usize,
    pub len: usize,
}

/// The images of an `.ico` file (ICONDIR and its 16-byte entries); none when it isn't one.
pub fn ico_images(ico: &[u8]) -> Vec<IcoImage> {
    let u16_at = |at: usize| ico.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| ico.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if u16_at(0) != Some(0) || u16_at(2) != Some(1) {
        return Vec::new();
    }
    let count = u16_at(4).unwrap_or(0) as usize;
    (0..count)
        .filter_map(|i| {
            let entry = 6 + i * 16;
            let side = |b: u8| if b == 0 { 256 } else { u32::from(b) };
            let width = side(*ico.get(entry)?);
            let height = side(*ico.get(entry + 1)?);
            let len = u32_at(entry + 8)? as usize;
            let offset = u32_at(entry + 12)? as usize;
            (offset.checked_add(len)? <= ico.len() && len > 0).then_some(IcoImage { width, height, offset, len })
        })
        .collect()
}

/// The image to draw at `size` pixels: the smallest at least that big, else the biggest.
pub fn best_image(images: &[IcoImage], size: u32) -> Option<IcoImage> {
    images
        .iter()
        .filter(|i| i.width >= size)
        .min_by_key(|i| i.width)
        .or_else(|| images.iter().max_by_key(|i| i.width))
        .copied()
}

/// `text` cut to `max` UTF-16 units with "…" at the end when longer (`TrayIcon.Truncate`).
pub fn truncate(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        if units + c.len_utf16() > max - 1 {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

/// Copies `text` into a fixed wide-character field, cut to fit with its terminating zero.
pub fn fill_wide(field: &mut [u16], text: &str) {
    let Some(room) = field.len().checked_sub(1) else { return };
    let mut n = 0;
    for unit in text.encode_utf16().take(room) {
        field[n] = unit;
        n += 1;
    }
    field[n..].fill(0);
}

#[cfg(windows)]
pub use imp::TrayIcon;

/// Off Windows there is no notification area icon: [`TrayIcon::new`] says so, and DNN Manager closes with its window.
#[cfg(not(windows))]
pub struct TrayIcon;

#[cfg(not(windows))]
impl TrayIcon {
    pub fn new(_tip: &str, _on_event: impl Fn(TrayEvent) + Send + 'static) -> Result<Self, String> {
        Err("The notification area icon is only available on Windows.".into())
    }

    pub fn show_notice(&self, _title: &str, _text: &str) {}
}

/// Off Windows the window is never hidden (there is no icon to bring it back from).
#[cfg(not(windows))]
pub fn show_window(_hwnd: isize, _show: bool) {}

/// Hides the window (running in the background) or shows it again and brings it forward - `hwnd` as
/// `raw_window_handle` gives it (`Hide` / `ShowFromBackground`).
#[cfg(windows)]
pub fn show_window(hwnd: isize, show: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow};
    let hwnd = HWND(hwnd as _);
    // SAFETY: a window handle of this process; a stale one only fails the calls.
    unsafe {
        if !show {
            let _ = ShowWindow(hwnd, SW_HIDE);
            return;
        }
        let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(hwnd);
    }
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::JoinHandle;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_NONE, NIIF_RESPECT_QUIET_TIME, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NIM_SETVERSION, NOTIFY_ICON_DATA_FLAGS, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, ChangeWindowMessageFilterEx, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW,
        DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
        GetSystemMetrics, HICON, IDI_APPLICATION, LR_DEFAULTCOLOR, LoadIconW, MF_SEPARATOR, MF_STRING, MSG,
        MSGFLT_ALLOW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON,
        SetForegroundWindow, SetMenuDefaultItem, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
        TranslateMessage, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_NULL, WNDCLASSW, WS_EX_TOOLWINDOW,
    };
    use windows::core::{PCWSTR, w};

    use super::{APP_ICO, OPEN_TEXT, QUIT_TEXT, TrayEvent, best_image, fill_wide, ico_images, truncate};

    // The message Explorer sends this icon's window about clicks on it (WM_APP + 1).
    const CALLBACK_MESSAGE: u32 = WM_APP + 1;
    // The app has queued a command for the tray's thread.
    const COMMAND_MESSAGE: u32 = WM_APP + 2;
    // With version 4, what was done is in the low word of lParam: a click (or Enter / Space on the icon), a right-click.
    const NIN_SELECT: u32 = 0x400;
    const NIN_KEYSELECT: u32 = 0x401;
    const NOTIFYICON_VERSION_4: u32 = 4;
    const MENU_OPEN: usize = 1;
    const MENU_QUIT: usize = 2;

    enum Command {
        Notice { title: String, text: String },
        Remove,
    }

    /// What the tray's window needs, on its own thread.
    struct State {
        hwnd: HWND,
        icon: HICON,
        owns_icon: bool,
        tip: String,
        taskbar_created: u32,
        commands: Receiver<Command>,
        on_event: Rc<dyn Fn(TrayEvent)>,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    /// The icon. Use it from any thread; dropping it removes the icon.
    pub struct TrayIcon {
        hwnd: isize,
        commands: Sender<Command>,
        thread: Option<JoinHandle<()>>,
    }

    impl TrayIcon {
        /// Adds the icon with `tip` as its tooltip; `on_event` is called on the tray's own thread. An error when its
        /// window can't be made. No Explorer yet (an early sign-in start): the icon is added when the taskbar is.
        pub fn new(tip: &str, on_event: impl Fn(TrayEvent) + Send + 'static) -> Result<Self, String> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, String>>();
            let (commands, commands_rx) = mpsc::channel();
            let tip = tip.to_string();
            let thread = std::thread::Builder::new()
                .name("tray-icon".into())
                .spawn(move || run(tip, Rc::new(on_event), commands_rx, ready_tx))
                .map_err(|e| e.to_string())?;
            match ready_rx.recv() {
                Ok(Ok(hwnd)) => Ok(Self { hwnd, commands, thread: Some(thread) }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => {
                    let _ = thread.join();
                    Err("The notification area icon's thread ended".into())
                }
            }
        }

        /// A Windows notification from the icon - for what happens while the window is hidden; Windows' quiet time is
        /// respected.
        pub fn show_notice(&self, title: &str, text: &str) {
            self.send(Command::Notice { title: title.to_string(), text: text.to_string() });
        }

        fn send(&self, command: Command) {
            if self.commands.send(command).is_ok() {
                // SAFETY: posting to the tray's window; it may be gone already, which only fails the post.
                let _ = unsafe { PostMessageW(Some(HWND(self.hwnd as _)), COMMAND_MESSAGE, WPARAM(0), LPARAM(0)) };
            }
        }
    }

    impl Drop for TrayIcon {
        fn drop(&mut self) {
            self.send(Command::Remove);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// The tray's thread: its window, the icon, and the message loop until the window is destroyed.
    fn run(
        tip: String,
        on_event: Rc<dyn Fn(TrayEvent)>,
        commands: Receiver<Command>,
        ready: Sender<Result<isize, String>>,
    ) {
        let hwnd = match create_window() {
            Ok(hwnd) => hwnd,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        // SAFETY: plain Win32 calls on the window this thread just made.
        unsafe {
            let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
            // DNN Manager runs elevated and Explorer doesn't: Windows drops its messages unless they are let through.
            let _ = ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None);
            let _ = ChangeWindowMessageFilterEx(hwnd, CALLBACK_MESSAGE, MSGFLT_ALLOW, None);
            let (icon, owns_icon) = load_icon();
            STATE.with(|s| {
                *s.borrow_mut() = Some(State { hwnd, icon, owns_icon, tip, taskbar_created, commands, on_event });
            });
            add();
            let _ = ready.send(Ok(hwnd.0 as isize));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            STATE.with(|s| s.borrow_mut().take());
        }
    }

    fn create_window() -> Result<HWND, String> {
        // SAFETY: registering a class (again is harmless: it fails and the existing one is used) and making a window
        // that is never shown.
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = w!("DnnManager.TrayIcon");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("DNN Manager"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| format!("Could not make the notification area icon's window: {e}"))
        }
    }

    /// The app's icon at the small icon size, or Windows' own application icon (shared, so never destroyed).
    fn load_icon() -> (HICON, bool) {
        // SAFETY: the bytes are one image of the embedded .ico, checked to lie inside it.
        unsafe {
            let size = GetSystemMetrics(SM_CXSMICON).max(16);
            if let Some(image) = best_image(&ico_images(APP_ICO), size as u32)
                && let Ok(icon) = CreateIconFromResourceEx(
                    &APP_ICO[image.offset..image.offset + image.len],
                    true,
                    0x0003_0000,
                    size,
                    size,
                    LR_DEFAULTCOLOR,
                )
            {
                return (icon, true);
            }
            (LoadIconW(None, IDI_APPLICATION).unwrap_or_default(), false)
        }
    }

    fn data(state: &State, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: state.hwnd,
            uID: 1,
            uFlags: flags,
            uCallbackMessage: CALLBACK_MESSAGE,
            hIcon: state.icon,
            ..Default::default()
        };
        fill_wide(&mut data.szTip, &state.tip);
        data
    }

    fn add() {
        STATE.with(|s| {
            let s = s.borrow();
            let Some(state) = s.as_ref() else { return };
            let mut data = data(state, NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
            // SAFETY: a filled NOTIFYICONDATAW for this window.
            unsafe {
                // No Explorer (yet): it is added when the taskbar is created.
                if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                    return;
                }
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
            }
        });
    }

    fn notice(title: &str, text: &str) {
        STATE.with(|s| {
            let s = s.borrow();
            let Some(state) = s.as_ref() else { return };
            let mut data = data(state, NIF_INFO);
            fill_wide(&mut data.szInfoTitle, &truncate(title, 63));
            fill_wide(&mut data.szInfo, &truncate(text, 255));
            data.dwInfoFlags = NIIF_NONE | NIIF_RESPECT_QUIET_TIME;
            // SAFETY: a filled NOTIFYICONDATAW for this window.
            let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        });
    }

    fn remove() {
        STATE.with(|s| {
            let s = s.borrow();
            let Some(state) = s.as_ref() else { return };
            let data = data(state, NOTIFY_ICON_DATA_FLAGS(0));
            // SAFETY: removing this window's icon and freeing the icon it made.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                if state.owns_icon {
                    let _ = DestroyIcon(state.icon);
                }
            }
        });
    }

    fn raise(event: TrayEvent) {
        // Called outside the borrow: the app's callback may take its time.
        let on_event = STATE.with(|s| s.borrow().as_ref().map(|state| state.on_event.clone()));
        if let Some(on_event) = on_event {
            on_event(event);
        }
    }

    /// Open and Quit where the pointer is, Open the default (bold).
    fn show_menu(hwnd: HWND) {
        // SAFETY: a popup menu owned and destroyed here; TrackPopupMenu runs its own loop, during which the window
        // procedure may run again - no state is borrowed meanwhile.
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let open: Vec<u16> = OPEN_TEXT.encode_utf16().chain([0]).collect();
            let quit: Vec<u16> = QUIT_TEXT.encode_utf16().chain([0]).collect();
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, PCWSTR(open.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, PCWSTR(quit.as_ptr()));
            let _ = SetMenuDefaultItem(menu, MENU_OPEN as u32, 0);
            let mut at = POINT::default();
            let _ = GetCursorPos(&mut at);
            // A menu of an app that isn't in front doesn't close on a click elsewhere: put its window in front first
            // (Explorer lets this app take the foreground after a click on its icon).
            let _ = SetForegroundWindow(hwnd);
            let chosen =
                TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, at.x, at.y, None, hwnd, None).0
                    as usize;
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match chosen {
                MENU_OPEN => raise(TrayEvent::Open),
                MENU_QUIT => raise(TrayEvent::Quit),
                _ => {}
            }
        }
    }

    extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let taskbar_created = STATE.with(|s| s.borrow().as_ref().map(|state| state.taskbar_created));
        if taskbar_created.is_some_and(|m| m == msg) {
            add();
            return LRESULT(0);
        }
        match msg {
            CALLBACK_MESSAGE => {
                match (lparam.0 as u32) & 0xFFFF {
                    NIN_SELECT | NIN_KEYSELECT => raise(TrayEvent::Open),
                    WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            COMMAND_MESSAGE => {
                loop {
                    let next = STATE.with(|s| s.borrow().as_ref().and_then(|state| state.commands.try_recv().ok()));
                    match next {
                        Some(Command::Notice { title, text }) => notice(&title, &text),
                        Some(Command::Remove) => {
                            // SAFETY: this thread's own window; WM_DESTROY removes the icon and ends the loop.
                            let _ = unsafe { DestroyWindow(hwnd) };
                            break;
                        }
                        None => break,
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                remove();
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            // SAFETY: the default handling of everything else.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_embedded_icon_makes_an_icon_handle() {
            // No shell needed: an icon handle made from the .ico's image, and freed.
            let (icon, owned) = load_icon();
            assert!(owned, "dnn.ico should load");
            assert!(!icon.is_invalid());
            unsafe { DestroyIcon(icon).unwrap() };
        }

        /// Adds the icon to the notification area and removes it again - needs a signed-in desktop with Explorer.
        #[test]
        #[ignore = "adds an icon to the notification area for a moment; needs Explorer"]
        fn adds_and_removes_the_icon() {
            let tray = TrayIcon::new("DNN Manager test", |_| {}).unwrap();
            // Removed on drop: the thread ends only once its window, and with it the icon, is gone.
            drop(tray);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_icon_has_a_small_image() {
        let images = ico_images(APP_ICO);
        assert!(!images.is_empty());
        let small = best_image(&images, 16).unwrap();
        assert!(small.width >= 16);
        assert!(images.iter().filter(|i| i.width >= 16).all(|i| i.width >= small.width));
        assert!(ico_images(b"not an icon").is_empty());
    }

    #[test]
    fn best_image_falls_back_to_the_biggest() {
        let img = |width| IcoImage { width, height: width, offset: 0, len: 1 };
        let images = [img(16), img(32), img(48)];
        assert_eq!(best_image(&images, 20).unwrap().width, 32);
        assert_eq!(best_image(&images, 64).unwrap().width, 48);
        assert_eq!(best_image(&[], 16), None);
    }

    #[test]
    fn notices_are_cut_with_an_ellipsis() {
        assert_eq!(truncate("short", 63), "short");
        let long = "x".repeat(70);
        let cut = truncate(&long, 63);
        assert_eq!(cut.encode_utf16().count(), 63);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn wide_fields_end_with_a_zero() {
        let mut field = [7u16; 5];
        fill_wide(&mut field, "DNN Manager");
        assert_eq!(field, [b'D' as u16, b'N' as u16, b'N' as u16, b' ' as u16, 0]);
        fill_wide(&mut field, "a");
        assert_eq!(field, [b'a' as u16, 0, 0, 0, 0]);
    }
}
