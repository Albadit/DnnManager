//! Where DNN Manager keeps its files: `Documents\DnnManager` (`AppDataPaths`), shared with the WPF app.

use std::path::PathBuf;

/// `Documents\DnnManager` - the Documents folder Windows knows, also when it is moved (OneDrive, another drive).
pub fn app_data_directory() -> PathBuf {
    documents_folder()
        .unwrap_or_else(|| {
            std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("."), PathBuf::from).join("Documents")
        })
        .join("DnnManager")
}

/// `dnnmanager.db`: settings, workspace, project records, keep warm and the saved DNN versions.
pub fn database_file() -> PathBuf {
    app_data_directory().join("dnnmanager.db")
}

#[cfg(windows)]
fn documents_folder() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: SHGetKnownFolderPath returns a NUL-terminated string Windows allocated; it is copied, then freed.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

#[cfg(not(windows))]
fn documents_folder() -> Option<PathBuf> {
    None
}
