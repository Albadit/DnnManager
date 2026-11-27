//! DNN Manager in GPUI - the composition root (`Program.cs`): administrator rights, the settings, the theme, the
//! key bindings and the window.

// A window app: no console window behind it in a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod logs_view;
mod pages;
mod palette;
mod runner;
mod store;
mod workspace;
mod workspace_state;

use std::sync::Arc;

use gpui::{App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, size};

use dnnm_application::iis::IisManager;
use dnnm_infrastructure::iis_fixture::FixtureIis;
use dnnm_infrastructure::{app_db::AppDatabase, iis::AppcmdIis, locale, paths, windows_security};

use dnnm_ui::theme::Theme;
use crate::workspace::Workspace;
use crate::workspace_state::WindowState;

/// Set (to anything) to run without administrator rights - for working on the UI; IIS then can't be read.
const NO_ELEVATE: &str = "DNNMANAGER_GPUI_NO_ELEVATE";
/// `--after <pid>` (`AppRestart.AfterArgument`): started by a restart - wait for that DNN Manager to end first.
pub(crate) const AFTER_ARGUMENT: &str = "--after";

/// Every default key binding - made again, with the user's changes, when Settings saves its shortcuts.
pub(crate) fn bind_default_keys(cx: &mut App) {
    // First: the later bindings of the same context win, so a page's own keys go over Tab's and the menus'.
    dnnm_ui::focus::bind_keys(cx);
    dnnm_ui::menu::bind_keys(cx);
    dnnm_terminal::bind_keys(cx);
    dnnm_ui::text_input::bind_keys(cx);
    dnnm_ui::dialog::bind_keys(cx);
    workspace::bind_keys(cx);
    palette::bind_keys(cx);
    pages::projects::bind_keys(cx);
    dnnm_dialogs::guide::bind_keys(cx);
    dnnm_dialogs::tour::bind_keys(cx);
    dnnm_dialogs::whats_new::bind_keys(cx);
}

/// Started by a restart: waits (up to 30 seconds) for the previous DNN Manager to exit (`AppRestart.WaitForPrevious`).
fn wait_for_previous() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|a| a == AFTER_ARGUMENT)
        && let Some(pid) = args.get(at + 1).and_then(|p| p.parse::<u32>().ok())
    {
        dnnm_system::update_helper::wait_for_exit(pid, std::time::Duration::from_secs(30));
    }
}

/// The arguments without `--after <pid>`.
fn own_args() -> Vec<String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(at) = args.iter().position(|a| a == AFTER_ARGUMENT) {
        args.drain(at..(at + 2).min(args.len()));
    }
    args
}

/// A file of appcmd's site XML: the app shows those sites instead of IIS's, and Start / Stop change only its copy.
const FIXTURE: &str = "DNNMANAGER_GPUI_FIXTURE";

/// DNN Manager's daily log gets DNN Manager's own lines - not those of the libraries under it (GPUI's renderer).
struct OwnLog(dnnm_logs::logger::FileLogger);

impl log::Log for OwnLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let target = metadata.target();
        (target.starts_with("dnnm") || target.starts_with("DnnManager")) && self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.0.log(record);
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

/// The window's place from the last start - if it is still on a screen (a monitor may be gone) - or centred.
fn initial_bounds(state: &WindowState, cx: &App) -> WindowBounds {
    let centred = Bounds::centered(None, size(gpui::px(1180.0), gpui::px(800.0)), cx);
    let (Some(left), Some(top), Some(width), Some(height)) = (state.left, state.top, state.width, state.height) else {
        return WindowBounds::Windowed(centred);
    };
    // The window's place and size are the screen's, not the UI scale's (`gpui::px`, not the app's scaled `px`).
    let bounds = Bounds::new(
        point(gpui::px(left), gpui::px(top)),
        size(gpui::px(width.max(400.0)), gpui::px(height.max(270.0))),
    );
    let on_screen = cx.displays().iter().any(|d| d.bounds().intersects(&bounds));
    match (on_screen, state.maximized) {
        (false, _) => WindowBounds::Windowed(centred),
        (true, true) => WindowBounds::Maximized(bounds),
        (true, false) => WindowBounds::Windowed(bounds),
    }
}

fn main() {
    // The update's helper: a copy of this exe started with `--apply-update <plan>` - it installs the update once
    // DNN Manager has closed and starts it again, and never opens a window of its own.
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if dnnm_system::update_helper::is_helper(&args) {
            std::process::exit(dnnm_system::update_helper::run(&args));
        }
    }
    // IIS's configuration needs administrator rights: started without, start again with the UAC prompt
    // (`AdminElevation`). If the user says no, run anyway - the Projects page says why it can't read IIS.
    let fixture = std::env::var_os(FIXTURE).map(std::path::PathBuf::from);
    wait_for_previous();
    // One DNN Manager at a time (`SingleInstance`): a second start brings the running one forward and ends. Only in a
    // release build (what the packages ship): a debug build - testing - opens as many as asked, and so does the
    // fixture, a UI check.
    let single_instance = fixture.is_none() && !cfg!(debug_assertions);
    if single_instance && dnnm_system::single_instance::hand_off_to_running() {
        return;
    }
    if !windows_security::is_elevated()
        && std::env::var_os(NO_ELEVATE).is_none()
        && fixture.is_none()
        && windows_security::relaunch_elevated(&own_args())
    {
        return;
    }

    // Numbers as the WPF app shows them: in the user's own separators (.NET formats in the current culture).
    if let Some((decimal, group)) = locale::number_separators() {
        dnnm_application::project::view::set_number_format(decimal, group);
    }

    // DNN Manager's own log: Documents\DnnManager\logs\dnnmanager-<yyyymmdd>.log (`DailyLogFile`).
    let log_file = dnnm_logs::daily_file::DailyLogFile::open(dnnm_logs::catalog::logs_directory());
    if log::set_boxed_logger(Box::new(OwnLog(dnnm_logs::logger::FileLogger::new(log_file.clone())))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let run_log = Arc::new(dnnm_logs::run_log::RunLog::new(log_file.clone()));

    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        let db = AppDatabase::new(paths::database_file());
        let iis: Arc<dyn IisManager> = match &fixture {
            Some(file) => match FixtureIis::load(file) {
                Ok(fixture) => Arc::new(fixture),
                Err(e) => {
                    eprintln!("The fixture can't be read: {e}");
                    std::process::exit(2);
                }
            },
            None => Arc::new(AppcmdIis),
        };
        let store = store::new_store(cx, db, iis, run_log.clone());
        if fixture.is_none() {
            store.update(cx, |s, _| s.use_real_system());
        }
        store.update(cx, |s, cx| s.report_unfinished(cx));
        let closing = log_file.clone();
        // The editors and SSMS looked for early, as the Projects page does, so "Open with…" fills at once.
        dnnm_openwith::warm_up();
        cx.on_app_quit(move |_| {
            closing.close();
            // A password left on the clipboard for SSMS goes with DNN Manager (`Program.cs`).
            dnnm_openwith::secret_clipboard::clear_if_still_ours();
            async {}
        })
        .detach();
        let settings = store.read(cx).settings.clone();
        workspace::apply_ui_scale(&settings);
        cx.set_global(
            Theme::new(settings.theme(), cx.window_appearance(), settings.font_size(), settings.density())
                .with_animations(settings.animations() && dnnm_system::motion::system_animations()),
        );
        // The defaults, then the user's own shortcuts (Settings → Keyboard shortcuts).
        workspace::apply_shortcuts(&settings, cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // Where it was left; else 1180 × 800, centred (`MainWindow.xaml`); at least 400 × 270.
        let options = WindowOptions {
            window_bounds: Some(initial_bounds(&WindowState::load(store.read(cx).db()), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("DNN Manager".into()),
                appears_transparent: true,
                // macOS draws its own close / minimize / zoom at the left: centred in the 36 px title bar (they are
                // 14 px high), 12 px in from the side. The title bar leaves room for them (`workspace::MAC_CAPTION`).
                traffic_light_position: cfg!(target_os = "macos").then(|| point(gpui::px(12.0), gpui::px(11.0))),
            }),
            window_min_size: Some(size(gpui::px(400.0), gpui::px(270.0))),
            app_id: Some("DnnManager.NET".into()),
            ..Default::default()
        };
        // While it runs: marked as running, and listening for a second start (bring forward) and Setup (quit).
        let marker = if single_instance { dnnm_system::single_instance::create_running_marker() } else { None };
        let window = cx.open_window(options, |window, cx| cx.new(|cx| Workspace::new(store.clone(), window, cx)));
        if let Ok(handle) = &window {
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<bool>();
            let (activate, quit) = (tx.clone(), tx);
            let listener = if single_instance {
                dnnm_system::single_instance::listen(
                    move || {
                        let _ = activate.unbounded_send(true);
                    },
                    move || {
                        let _ = quit.unbounded_send(false);
                    },
                )
            } else {
                None
            };
            let handle = *handle;
            cx.spawn(async move |cx| {
                let _keep = (listener, marker);
                use futures::StreamExt;
                while let Some(activate) = rx.next().await {
                    if activate {
                        let _ = handle.update(cx, |ws, window, cx| ws.show_from_background(window, cx));
                    } else {
                        let _ = cx.update(|cx| cx.quit());
                    }
                }
            })
            .detach();
        }
        if let Err(e) = window {
            eprintln!("DNN Manager can't open its window: {e}");
            cx.quit();
        }
        cx.activate(true);
    });
}
