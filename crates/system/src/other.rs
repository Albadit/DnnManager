//! The Windows-only services on macOS and Linux: the same names and signatures the other crates use, saying that the
//! feature is Windows-only (or doing the harmless thing: no running marker, no locking processes, nothing elevated).
//! DNN Manager's sites are IIS sites, so these are reached there only through Settings and Troubleshoot - not by an
//! operation on a project.

/// The sentence a Windows-only feature answers with.
fn only_on_windows(what: &str) -> String {
    format!("{what} works only on Windows.")
}

/// Starting DNN Manager at sign-in (Windows' Task Scheduler).
pub mod startup {
    use std::path::{Path, PathBuf};

    /// No sign-in task off Windows.
    pub fn get_target() -> Option<PathBuf> {
        None
    }

    pub fn refusal(_exe_path: &Path) -> Option<String> {
        Some(super::only_on_windows("Starting DNN Manager at sign-in"))
    }

    pub fn enable(_exe_path: &Path) -> Result<(), String> {
        Err(super::only_on_windows("Starting DNN Manager at sign-in"))
    }

    /// There is no task to remove.
    pub fn disable() -> Result<(), String> {
        Ok(())
    }
}

/// One DNN Manager at a time (Windows' named events): off Windows each start runs on its own.
pub mod single_instance {
    /// The running marker; nothing to keep alive here.
    pub struct RunningMarker;

    /// No marker: Setup, which reads it, is Windows-only.
    pub fn create_running_marker() -> Option<(RunningMarker, bool)> {
        None
    }

    pub fn is_running() -> bool {
        false
    }

    /// Never handed off: this start runs.
    pub fn hand_off_to_running() -> bool {
        false
    }

    /// Listens for later starts; nothing to listen to here.
    pub struct Listener;

    pub fn listen(_on_activate: impl Fn() + Send + 'static, _on_quit: impl Fn() + Send + 'static) -> Option<Listener> {
        None
    }
}

/// Programs DNN Manager may start (`trusted.rs`): off Windows DNN Manager doesn't run elevated, so - as for an
/// unelevated Windows start - there is no boundary: the program found on PATH.
pub mod trusted {
    use std::path::{Path, PathBuf};

    pub fn is_elevated() -> bool {
        false
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Trust {
        AdminOnly,
        OthersCanChange,
        Unknown,
    }

    /// A program DNN Manager may start: its path, or None and why.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct Resolved {
        pub path: Option<PathBuf>,
        pub refused: Option<String>,
        pub reason: Option<String>,
    }

    impl Resolved {
        /// Why it isn't started, as a sentence for the user.
        pub fn problem(&self, name: &str) -> String {
            match &self.refused {
                None => format!("{name} isn't installed (or not on PATH)."),
                Some(refused) => {
                    format!("DNN Manager doesn't start {refused}: {}", self.reason.as_deref().unwrap_or_default())
                }
            }
        }
    }

    /// A full path as it is (when it is a file), a bare name looked up on PATH.
    pub fn resolve(name: &str) -> Resolved {
        let given = Path::new(name);
        if given.is_absolute() {
            return Resolved { path: given.is_file().then(|| given.to_path_buf()), ..Default::default() };
        }
        let path = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join(name)).find(|p| p.is_file()))
            .unwrap_or_default();
        Resolved { path, ..Default::default() }
    }
}

/// The update helper (`update_helper.rs`): the part the app uses when it restarts.
pub mod update_helper {
    use std::time::{Duration, Instant};

    /// Whether the process `pid` has ended within `timeout` (also when it is already gone) - polled through /proc on
    /// Linux; elsewhere it waits the time out (a restart only waits for the old process to close).
    pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
        let started = Instant::now();
        let proc = std::path::Path::new("/proc");
        if !proc.is_dir() {
            std::thread::sleep(timeout.min(Duration::from_secs(2)));
            return true;
        }
        while proc.join(pid.to_string()).exists() {
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }
}

/// The files DNN Manager handles with administrator rights - on a project, so Windows-only; the parts Settings and
/// Troubleshoot reach are here.
pub mod files {
    pub use crate::files_backup_filter as backup_filter;

    /// Copying, zipping and unzipping a site.
    pub mod copier {
        use std::path::Path;

        use dnnm_application::operation::{CancelToken, Reporter};

        fn refused() -> String {
            super::super::only_on_windows("Copying a DNN site")
        }

        /// Why an extraction stopped.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum ExtractError {
            InUse(String),
            Failed(String),
        }

        impl std::fmt::Display for ExtractError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::InUse(m) | Self::Failed(m) => f.write_str(m),
                }
            }
        }

        pub fn copy(
            _src: &Path,
            _dest: &Path,
            _report: &mut dyn Reporter,
            _progress: &mut (dyn FnMut(&str) + Send),
            _cancel: &CancelToken,
        ) -> Result<(), String> {
            Err(refused())
        }

        pub fn extract_zip(
            _zip_path: &Path,
            _dest: &Path,
            _report: &mut dyn Reporter,
            _progress: &mut dyn FnMut(&str),
            _cancel: &CancelToken,
        ) -> Result<(), ExtractError> {
            Err(ExtractError::Failed(refused()))
        }

        pub fn remove_files_not_in_zip(
            _zip_path: &Path,
            _directory: &Path,
            _excluded_paths: &[String],
            _report: &mut dyn Reporter,
            _cancel: &CancelToken,
        ) -> Result<usize, String> {
            Err(refused())
        }

        pub fn create_zip(
            _src: &Path,
            _zip_path: &Path,
            _excluded_paths: &[String],
            _report: &mut dyn Reporter,
            _progress: &mut dyn FnMut(&str),
            _cancel: &CancelToken,
        ) -> Result<(), String> {
            Err(refused())
        }
    }

    /// A zip entry's names (`safe_zip.rs`) - plain text rules, the same everywhere.
    pub mod safe_zip {
        /// An entry's name with '/' between its folders - some zip tools write '\'.
        pub fn name(raw: &str) -> String {
            raw.replace('\\', "/")
        }

        /// A folder entry: its name ends with a separator.
        pub fn is_folder(raw: &str) -> bool {
            name(raw).ends_with('/')
        }

        /// The file name part of an entry's name.
        pub fn file_name(raw: &str) -> String {
            let n = name(raw);
            n.rsplit('/').next().unwrap_or_default().to_string()
        }
    }

    /// The processes that hold a site's folder (Windows' Restart Manager): none found here.
    pub mod file_locks {
        use std::path::Path;

        /// A process that keeps a folder from being deleted.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct LockingProcess {
            pub id: u32,
            pub name: String,
            pub exe: String,
            pub reason: String,
            pub can_close: bool,
        }

        pub fn find_lockers(_directory: &Path) -> Vec<LockingProcess> {
            Vec::new()
        }

        /// Nothing is closed: all are still running.
        pub fn close(lockers: &[LockingProcess]) -> Vec<LockingProcess> {
            lockers.to_vec()
        }

        pub fn stop_programs_running_from(_directory: &Path) -> Vec<String> {
            Vec::new()
        }

        pub fn schedule_delete_on_restart(_directory: &Path) -> Result<(), String> {
            Err(super::super::only_on_windows("Deleting a folder at the next restart"))
        }
    }

    /// The projects folder's checks (Windows folders and their permissions): nothing to refuse here.
    pub mod folder_guard {
        pub fn projects_folder_problem(_folder: &str) -> Option<String> {
            None
        }
    }

    /// DNN Manager's private temporary folder (ProgramData, locked down): the system's own off Windows.
    pub mod private_temp {
        use std::path::PathBuf;

        pub fn path() -> std::io::Result<PathBuf> {
            let folder = std::env::temp_dir().join("DnnManager");
            std::fs::create_dir_all(&folder)?;
            Ok(folder)
        }
    }

    /// Paths in folders others can write to: links are symbolic links here.
    pub mod safe_path {
        use std::collections::HashSet;
        use std::path::{Path, PathBuf};

        /// The line an error about a link ends with: what to do about it.
        pub const LINK_HINT: &str =
            "Hint: if the link is meant to be there, copy the files in instead of linking them.";

        /// Folders already found to be no link.
        #[derive(Debug, Default, Clone)]
        pub struct CheckedFolders(HashSet<PathBuf>);

        impl CheckedFolders {
            pub fn new() -> Self {
                Self::default()
            }

            pub fn contains(&self, folder: &Path) -> bool {
                self.0.contains(folder)
            }
        }

        /// A full path without its trailing separator; None when it isn't valid.
        pub fn full(path: &Path) -> Option<PathBuf> {
            let text = path.to_str()?;
            if text.is_empty() || text.contains('\0') {
                return None;
            }
            let absolute = std::path::absolute(path).ok()?;
            let text = absolute.to_str()?;
            let trimmed = text.trim_end_matches('/');
            Some(PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed }))
        }

        pub fn is_same_or_inside(path: &Path, folder: &Path) -> bool {
            matches!((full(path), full(folder)), (Some(p), Some(f)) if p.starts_with(&f))
        }

        pub fn is_inside(path: &Path, folder: &Path) -> bool {
            matches!((full(path), full(folder)), (Some(p), Some(f)) if p.starts_with(&f) && p != f)
        }

        /// Whether `path` exists and is a symbolic link. One that can't be read isn't trusted either.
        pub fn is_link(path: &Path) -> bool {
            match std::fs::symlink_metadata(path) {
                Ok(meta) => meta.file_type().is_symlink(),
                Err(e) => e.kind() != std::io::ErrorKind::NotFound,
            }
        }

        /// Whether `path` - or a folder on the way to it from `root`, the root included - is a link. A path outside
        /// the root counts as one.
        pub fn has_link(root: &Path, path: &Path, _checked: Option<&mut CheckedFolders>) -> bool {
            let (Some(top), Some(at)) = (full(root), full(path)) else { return true };
            if !at.starts_with(&top) {
                return true;
            }
            at.ancestors().take_while(|a| a.starts_with(&top)).any(is_link)
        }

        /// `relative` under `root`, strictly inside it and through no link; None otherwise.
        pub fn under(root: &Path, relative: &str, checked: Option<&mut CheckedFolders>) -> Option<PathBuf> {
            if relative.is_empty() || relative.contains('\0') || relative.starts_with(['/', '\\']) {
                return None;
            }
            let top = full(root)?;
            let target = full(&top.join(relative.replace('\\', "/")))?;
            if !target.starts_with(&top) || target == top || relative.split(['/', '\\']).any(|p| p == "..") {
                return None;
            }
            (!has_link(&top, &target, checked)).then_some(target)
        }

        pub fn ensure_no_link(root: &Path, path: &Path, checked: Option<&mut CheckedFolders>) -> std::io::Result<()> {
            if has_link(root, path, checked) {
                return Err(std::io::Error::other(format!(
                    "{} is reached through a link - DNN Manager doesn't delete or write through those.\n{LINK_HINT}",
                    path.display()
                )));
            }
            Ok(())
        }

        /// Deletes the folder `path`, never following a link in it (a link is deleted itself).
        pub fn delete_tree(path: &Path) -> std::io::Result<()> {
            let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok(()) };
            if meta.file_type().is_symlink() {
                return std::fs::remove_file(path);
            }
            if meta.is_dir() { std::fs::remove_dir_all(path) } else { Ok(()) }
        }

        pub fn long_path(path: &Path) -> PathBuf {
            full(path).unwrap_or_else(|| path.to_path_buf())
        }
    }
}
