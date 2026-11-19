//! Browse… for one kind of file: the Open dialog lists only files of that kind ("Site .zip" shows .zip files, not the
//! .bacpac and .log beside them). On Windows that is `dnnm_system::file_picker`, over the app's window; GPUI 0.2.2's
//! own prompt can't filter, so it is only the fallback (and what macOS and Linux get).

use std::path::PathBuf;

use gpui::{App, PathPromptOptions, Task, Window};

/// Asks for one existing file of the kind `kind` ("Site archive") with the extension `extension` ("zip"), titled
/// `title`. None when cancelled.
pub fn open_file(window: &Window, cx: &mut App, title: &str, kind: &str, extension: &str) -> Task<Option<PathBuf>> {
    #[cfg(windows)]
    {
        // background_spawn is AppContext's; only this branch uses it (elsewhere the import would be unused).
        use gpui::AppContext;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let owner = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
            _ => 0,
        };
        let kind = dnnm_system::file_picker::FileKind { name: format!("{kind} (*.{extension})"), patterns: format!("*.{extension}") };
        let (title, prompt_title) = (title.to_string(), title.to_string());
        let shown = cx.background_spawn(async move { dnnm_system::file_picker::open_file(owner, &title, &kind) });
        cx.spawn(async move |cx| match shown.await {
            Ok(path) => path,
            // The shell's dialog couldn't be made: GPUI's, which lists every file but still opens.
            Err(_) => {
                let paths = cx.update(|cx| cx.prompt_for_paths(options(prompt_title))).ok()?;
                paths.await.ok()?.ok()??.into_iter().next()
            }
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (window, kind, extension);
        let paths = cx.prompt_for_paths(options(title.to_string()));
        cx.spawn(async move |_| paths.await.ok()?.ok()??.into_iter().next())
    }
}

fn options(title: String) -> PathPromptOptions {
    PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(title.into()) }
}
