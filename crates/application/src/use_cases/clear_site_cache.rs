//! `ClearSiteCacheUseCase.cs`: the three-dot menu's "Clear website cache" - for a DNN site its file cache
//! (`Portals\_default\Cache`) and its bundled CSS / JavaScript (`App_Data\ClientDependency`), then for any site its app
//! pool is recycled, which empties what ASP.NET and DNN keep in memory. The site restarts; its next request builds
//! everything anew. Nothing is noted to undo: a cache is made again by the site itself.
//!
//! The site's folder is the app pool's to write: the file work goes through [`CacheFiles`], which never deletes
//! through a link or junction (`SafePath`).
//!
//! Deviation: line breaks are `\n` (the C#'s `Environment.NewLine` is `\r\n`).

use std::path::{Path, PathBuf};

use crate::iis::IisManager;
use crate::operation::{CANCELLED, Operation};

const NL: &str = "\n";

/// DNN's caches on disk, inside the site's folder. Their files are deleted; the folders stay.
pub const CACHE_FOLDERS: [&str; 2] = [r"Portals\_default\Cache", r"App_Data\ClientDependency"];

/// Which site's cache to clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearSiteCacheRequest {
    pub site: String,
    pub directory: PathBuf,
}

/// The file work, never through a link or junction (`SafePath`).
pub trait CacheFiles: Send + Sync {
    /// The line an error about a link ends with: what to do about it (`SafePath.LinkHint`).
    fn link_hint(&self) -> &str;
    /// Whether `path` is a link or junction itself.
    fn is_link(&self, path: &Path) -> bool;
    /// Whether `path`, or a folder on the way to it from `root`, is a link or junction.
    fn has_link(&self, root: &Path, path: &Path) -> bool;
    /// Every file under `folder`, links and junctions (and what is in them) left out.
    fn files_under(&self, folder: &Path) -> Vec<PathBuf>;
    /// Deletes one file, read-only or not.
    fn delete_file(&self, file: &Path) -> std::io::Result<()>;
    /// The folders directly in `folder`, links included.
    fn subfolders(&self, folder: &Path) -> Vec<PathBuf>;
    /// Deletes a folder with everything in it; a link in it is only unlinked.
    fn delete_tree(&self, folder: &Path) -> std::io::Result<()>;
}

/// Whether the folder holds a DNN site: `bin\DotNetNuke.dll`.
pub fn is_dnn(directory: &Path) -> bool {
    directory.join("bin").join("DotNetNuke.dll").is_file()
}

pub fn clear_site_cache(
    iis: &dyn IisManager,
    files: &dyn CacheFiles,
    request: &ClearSiteCacheRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let (site, directory) = (&request.site, request.directory.as_path());
    let dnn = is_dnn(directory);
    let what = if dnn {
        "DNN's cached files and bundled CSS / JavaScript are deleted and its app pool is recycled"
    } else {
        "Its app pool is recycled, which empties what it holds in memory"
    };
    if !op.prompt.confirm(
        &format!(
            "Clear the cache of '{site}'?{NL}{NL}{what} - the site restarts, and its next request takes longer while it \
             builds everything anew."
        ),
        "Clear cache",
        "Cancel",
        false,
    ) {
        return Err(CANCELLED.into());
    }

    if dnn {
        op.report.step("Deleting DNN's cached files");
        // The site's folder itself a link or junction: its cache isn't the site's - nothing is deleted there, and that
        // is said rather than passed over as done.
        if files.is_link(directory) {
            return Err(format!(
                "{} is a link or junction - DNN Manager doesn't delete through those: what it points to isn't \
                 necessarily the site's cache.{NL}{}",
                directory.display(),
                files.link_hint()
            ));
        }
        let (mut deleted, mut locked) = (0usize, 0usize);
        for folder in CACHE_FOLDERS.iter().map(|f| directory.join(f)).filter(|f| f.is_dir()) {
            // Not through a junction or link - the cache folder itself, a folder on the way, or one in it: the app
            // pool can write here, and what one points to isn't the cache.
            if files.has_link(directory, &folder) {
                op.report.warn(&format!(
                    "Left {} as it is: it is a link or junction, or reached through one - deleting there could reach \
                     a folder outside the site.{NL}{}",
                    folder.display(),
                    files.link_hint()
                ));
                continue;
            }
            for file in files.files_under(&folder) {
                op.cancel.check()?;
                match files.delete_file(&file) {
                    Ok(()) => deleted += 1,
                    // In use - the recycle below lets go of it, and DNN writes it anew.
                    Err(_) => locked += 1,
                }
            }
            // A link among them is only unlinked, never gone into.
            for sub in files.subfolders(&folder) {
                let _ = files.delete_tree(&sub);
            }
        }
        op.report.success(&if locked == 0 {
            format!("Deleted {deleted} cached file(s).")
        } else {
            format!("Deleted {deleted} cached file(s); {locked} in use were left.")
        });
    }

    op.report.step("Recycling the app pool");
    iis.recycle_app_pool(site).map_err(|e| format!("Could not recycle the app pool of '{site}': {e}"))?;
    op.report.success(&format!("App pool of '{site}' recycled - its memory cache is empty."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::tests::{Answer, Lines};
    use crate::operation::{CancelToken, Prompt};
    use crate::use_cases::tests::FakeIis;
    use std::sync::Mutex;

    /// Real files in a temporary folder; the paths in `links` count as links.
    #[derive(Default)]
    struct TempFiles {
        links: Vec<PathBuf>,
        locked: Vec<PathBuf>,
        trees: Mutex<Vec<PathBuf>>,
    }

    impl CacheFiles for TempFiles {
        fn link_hint(&self) -> &str {
            "Hint: recreate it."
        }
        fn is_link(&self, path: &Path) -> bool {
            self.links.iter().any(|l| l == path)
        }
        fn has_link(&self, root: &Path, path: &Path) -> bool {
            self.links.iter().any(|l| path.starts_with(l) && l.starts_with(root))
        }
        fn files_under(&self, folder: &Path) -> Vec<PathBuf> {
            let mut found = Vec::new();
            for entry in std::fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { found.extend(self.files_under(&path)) } else { found.push(path) }
            }
            found
        }
        fn delete_file(&self, file: &Path) -> std::io::Result<()> {
            if self.locked.iter().any(|l| l == file) {
                return Err(std::io::Error::other("in use"));
            }
            std::fs::remove_file(file)
        }
        fn subfolders(&self, folder: &Path) -> Vec<PathBuf> {
            std::fs::read_dir(folder).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect()
        }
        fn delete_tree(&self, folder: &Path) -> std::io::Result<()> {
            self.trees.lock().unwrap().push(folder.to_path_buf());
            std::fs::remove_dir_all(folder)
        }
    }

    fn dnn_site() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (file, text) in [
            (r"bin\DotNetNuke.dll", ""),
            (r"Portals\_default\Cache\a.resources", "x"),
            (r"Portals\_default\Cache\Pages\b.resources", "x"),
            (r"App_Data\ClientDependency\c.js", "x"),
            (r"App_Data\Database.mdf", "keep"),
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    fn run(
        iis: &FakeIis,
        files: &TempFiles,
        prompt: &dyn Prompt,
        directory: &Path,
    ) -> (Result<(), String>, Vec<String>) {
        let mut lines = Lines::default();
        let mut op = Operation::new(&mut lines, prompt, CancelToken::new());
        let request = ClearSiteCacheRequest { site: "shop".into(), directory: directory.to_path_buf() };
        let result = clear_site_cache(iis, files, &request, &mut op);
        assert!(op.undo.is_empty());
        drop(op);
        (result, lines.0)
    }

    #[test]
    fn asks_first_and_a_no_changes_nothing() {
        let site = dnn_site();
        let (iis, files, no) = (FakeIis::running(), TempFiles::default(), Answer::no());
        let (result, lines) = run(&iis, &files, &no, site.path());
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(lines.is_empty() && iis.calls.lock().unwrap().is_empty());
        assert!(site.path().join(r"Portals\_default\Cache\a.resources").exists());
        assert_eq!(
            no.1.lock().unwrap()[0],
            "Clear the cache of 'shop'?\n\nDNN's cached files and bundled CSS / JavaScript are deleted and its app pool \
             is recycled - the site restarts, and its next request takes longer while it builds everything anew."
        );
    }

    #[test]
    fn deletes_dnns_cache_files_keeps_the_folders_and_recycles() {
        let site = dnn_site();
        let locked = site.path().join(r"App_Data\ClientDependency\c.js");
        let (iis, files) = (FakeIis::running(), TempFiles { locked: vec![locked.clone()], ..Default::default() });
        let (result, lines) = run(&iis, &files, &Answer::yes(), site.path());
        result.unwrap();
        assert_eq!(
            lines,
            vec![
                "## Deleting DNN's cached files",
                "ok Deleted 2 cached file(s); 1 in use were left.",
                "## Recycling the app pool",
                "ok App pool of 'shop' recycled - its memory cache is empty."
            ]
        );
        let cache = site.path().join(r"Portals\_default\Cache");
        assert!(cache.is_dir() && !cache.join("Pages").exists() && locked.exists());
        assert!(site.path().join(r"App_Data\Database.mdf").exists());
        assert_eq!(*iis.calls.lock().unwrap(), vec!["recycle shop"]);
    }

    #[test]
    fn another_site_is_only_recycled() {
        let dir = tempfile::tempdir().unwrap();
        let (iis, files, yes) = (FakeIis::running(), TempFiles::default(), Answer::yes());
        let (result, lines) = run(&iis, &files, &yes, dir.path());
        result.unwrap();
        assert!(
            yes.1.lock().unwrap()[0].contains("Its app pool is recycled, which empties what it holds in memory - ")
        );
        assert_eq!(lines[0], "## Recycling the app pool");
    }

    #[test]
    fn never_deletes_through_a_link() {
        let site = dnn_site();
        let files = TempFiles { links: vec![site.path().to_path_buf()], ..Default::default() };
        let (result, _) = run(&FakeIis::running(), &files, &Answer::yes(), site.path());
        assert!(result.unwrap_err().ends_with(
            "is a link or junction - DNN Manager doesn't delete through those: what it points to isn't necessarily \
             the site's cache.\nHint: recreate it."
        ));
        assert!(site.path().join(r"Portals\_default\Cache\a.resources").exists());

        let cache = site.path().join(r"Portals\_default\Cache");
        let files = TempFiles { links: vec![cache.clone()], ..Default::default() };
        let (result, lines) = run(&FakeIis::running(), &files, &Answer::yes(), site.path());
        result.unwrap();
        assert!(lines[1].starts_with(&format!("warn Left {} as it is: it is a link or junction", cache.display())));
        assert_eq!(lines[2], "ok Deleted 1 cached file(s).");
        assert!(cache.join("a.resources").exists());
    }

    #[test]
    fn a_failed_recycle_is_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let iis = FakeIis { failing: vec!["shop"], ..FakeIis::running() };
        let (result, _) = run(&iis, &TempFiles::default(), &Answer::yes(), dir.path());
        assert_eq!(result.unwrap_err(), "Could not recycle the app pool of 'shop': access denied");
    }
}
