//! Clear website cache (`ClearSiteCacheUseCase`) joined to IIS and `SafePath` (dnnm-system's `safe_path`); and the
//! WPF texts the app shows around it (`ProjectMenu.AddSiteTools`, `ServerStore.ClearCacheAsync`, `Onboarding.cs`).

use std::path::{Path, PathBuf};

use dnnm_application::operation::Operation;
pub use dnnm_application::use_cases::clear_site_cache::ClearSiteCacheRequest;
use dnnm_application::use_cases::clear_site_cache::{self as use_case, CacheFiles};
use dnnm_system::files::safe_path;

use crate::Services;

/// The row's menu item (its three dots, or a right-click), enabled while the row is idle and no operation runs.
pub const CLEAR_CACHE_MENU_ITEM: &str = "Clear website cache…";
/// Its tooltip for a DNN site ([`is_dnn`] of the row's folder)…
pub const CLEAR_CACHE_TOOLTIP_DNN: &str =
    "Deletes DNN's cached files and bundled CSS / JavaScript, then recycles the app pool.";
/// …and for any other site.
pub const CLEAR_CACHE_TOOLTIP_OTHER: &str = "Recycles the app pool, which empties what the site keeps in memory.";
/// The guide's (Onboarding) entry.
pub const CLEAR_CACHE_GUIDE_TITLE: &str = "Clear website cache…";
pub const CLEAR_CACHE_GUIDE_TEXT: &str = "Deletes the website's temporary files and restarts it. Helps when pages show old content. The next page loads a little slower.";
/// What the row says once the operation has started (after the question was answered yes).
pub const CLEARING_CACHE: &str = "Clearing cache…";

pub use dnnm_application::use_cases::clear_site_cache::is_dnn;

/// The operation's title.
pub fn clear_cache_title(site: &str) -> String {
    format!("Clear the cache of '{site}'")
}

/// The success toast once it has finished.
pub fn cache_cleared_toast(site: &str) -> String {
    format!("The cache of '{site}' was cleared.")
}

/// Clears the site's cache, asking first.
pub fn clear_site_cache(services: &Services, request: ClearSiteCacheRequest, op: &mut Operation) -> Result<(), String> {
    use_case::clear_site_cache(services.iis.as_ref(), &SafeFiles, &request, op)
}

/// `SafePath` for the cache folders.
struct SafeFiles;

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Off Windows a link is a symbolic link.
#[cfg(not(windows))]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// `SafePath.Recursive`: every file, without links or junctions and without going into them.
fn walk(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if is_reparse_point(&meta) {
            continue;
        }
        if meta.is_dir() {
            walk(&path, found);
        } else {
            found.push(path);
        }
    }
}

impl CacheFiles for SafeFiles {
    fn link_hint(&self) -> &str {
        safe_path::LINK_HINT
    }

    fn is_link(&self, path: &Path) -> bool {
        safe_path::is_link(path)
    }

    fn has_link(&self, root: &Path, path: &Path) -> bool {
        safe_path::has_link(root, path, None)
    }

    fn files_under(&self, folder: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        walk(folder, &mut found);
        found
    }

    fn delete_file(&self, file: &Path) -> std::io::Result<()> {
        // `File.SetAttributes(Normal)` first: DNN writes some of its cache read-only.
        let meta = std::fs::symlink_metadata(file)?;
        if meta.permissions().readonly() {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            std::fs::set_permissions(file, permissions)?;
        }
        std::fs::remove_file(file)
    }

    fn subfolders(&self, folder: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
        entries
            .flatten()
            .filter(|e| std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir() || is_reparse_point(&m)))
            .map(|e| e.path())
            .collect()
    }

    fn delete_tree(&self, folder: &Path) -> std::io::Result<()> {
        safe_path::delete_tree(folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(clear_cache_title("shop"), "Clear the cache of 'shop'");
        assert_eq!(cache_cleared_toast("shop"), "The cache of 'shop' was cleared.");
    }

    #[test]
    fn the_cache_walk_deletes_read_only_files_and_keeps_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("Cache");
        std::fs::create_dir_all(cache.join("Pages")).unwrap();
        let file = cache.join("Pages").join("a.resources");
        std::fs::write(&file, "x").unwrap();
        std::fs::write(cache.join("b.resources"), "x").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();

        let files = SafeFiles;
        let mut found = files.files_under(&cache);
        found.sort();
        assert_eq!(found, vec![cache.join("Pages").join("a.resources"), cache.join("b.resources")]);
        for f in &found {
            files.delete_file(f).unwrap();
        }
        assert_eq!(files.subfolders(&cache), vec![cache.join("Pages")]);
        files.delete_tree(&cache.join("Pages")).unwrap();
        assert!(cache.is_dir() && std::fs::read_dir(&cache).unwrap().next().is_none());
        assert!(!files.is_link(&cache) && !files.has_link(dir.path(), &cache));
    }
}
