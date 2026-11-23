//! This PC's figures for the status bar (`HostResourceMonitor`): memory in use, processor use since the last sample,
//! and the projects folder's drive. Windows reads them itself; macOS and Linux through sysinfo (Mach's host statistics
//! and `statfs`, `/proc/meminfo`, `/proc/stat` and `statvfs`). What can't be read stays unknown - never 0.

use std::path::Path;

/// One reading of the PC.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostResources {
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    /// None on the first sample: CPU use is the time between two.
    pub cpu_percent: Option<f64>,
    /// The drive's root, `C:\` - on macOS and Linux the disk's mount point, `/`; None when it can't be read.
    pub disk_root: Option<String>,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
}

/// Keeps the previous processor times, so each sample tells the use in between.
#[derive(Debug, Default)]
pub struct HostSampler {
    #[cfg(windows)]
    previous: Option<(u64, u64)>,
    /// sysinfo's readers, made at the first sample.
    #[cfg(not(windows))]
    probe: Option<unix::Probe>,
}

impl HostSampler {
    pub fn sample(&mut self, projects_folder: &Path) -> HostResources {
        let mut r = HostResources::default();
        if let Some((used, total)) = self.memory() {
            r.memory_used_bytes = used;
            r.memory_total_bytes = total;
        }
        r.cpu_percent = self.cpu_percent();
        if let Some((root, used, total)) = self.disk(projects_folder) {
            r.disk_root = Some(root);
            r.disk_used_bytes = used;
            r.disk_total_bytes = total;
        }
        r
    }

    #[cfg(windows)]
    fn memory(&mut self) -> Option<(u64, u64)> {
        memory()
    }

    /// The processors' use since the last sample, 0-100; None on the first.
    #[cfg(windows)]
    fn cpu_percent(&mut self) -> Option<f64> {
        let (idle, busy_and_idle) = cpu_times()?;
        let (idle0, total0) = self.previous.replace((idle, busy_and_idle))?;
        let total = busy_and_idle.saturating_sub(total0);
        (total > 0).then(|| total.saturating_sub(idle.saturating_sub(idle0)) as f64 / total as f64 * 100.0)
    }

    #[cfg(windows)]
    fn disk(&mut self, folder: &Path) -> Option<(String, u64, u64)> {
        disk(folder)
    }

    #[cfg(not(windows))]
    fn memory(&mut self) -> Option<(u64, u64)> {
        self.probe.get_or_insert_with(unix::Probe::new).memory()
    }

    #[cfg(not(windows))]
    fn cpu_percent(&mut self) -> Option<f64> {
        self.probe.get_or_insert_with(unix::Probe::new).cpu_percent()
    }

    #[cfg(not(windows))]
    fn disk(&mut self, folder: &Path) -> Option<(String, u64, u64)> {
        self.probe.get_or_insert_with(unix::Probe::new).disk(folder)
    }
}

#[cfg(windows)]
fn memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut s = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: dwLength is set as the call requires.
    unsafe { GlobalMemoryStatusEx(&mut s).ok()? };
    Some((s.ullTotalPhys - s.ullAvailPhys, s.ullTotalPhys))
}

/// Idle time and kernel + user time (kernel time includes idle) - 100 ns ticks.
#[cfg(windows)]
fn cpu_times() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    fn ticks(t: FILETIME) -> u64 {
        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
    }
    let (mut idle, mut kernel, mut user) = Default::default();
    // SAFETY: three out-parameters of the right type.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
    Some((ticks(idle), ticks(kernel) + ticks(user)))
}

#[cfg(windows)]
fn disk(folder: &Path) -> Option<(String, u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    // The drive's root: the folder may not exist yet, its drive does.
    let root = folder.components().next()?.as_os_str().to_string_lossy().into_owned() + "\\";
    let wide: Vec<u16> = std::ffi::OsStr::new(&root).encode_wide().chain(Some(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: a NUL-terminated root path and two out-parameters.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), None, Some(&mut total), Some(&mut free)).ok()? };
    Some((root, total.saturating_sub(free), total))
}

/// The disk whose mount point holds `folder` - the deepest such - out of `mounts` (mount point, total, available):
/// its mount point, used and total bytes. None when no disk holds it or its size is unknown.
#[cfg_attr(windows, allow(dead_code))]
fn disk_of<'a>(folder: &Path, mounts: impl IntoIterator<Item = (&'a Path, u64, u64)>) -> Option<(String, u64, u64)> {
    let (mount, total, available) = mounts
        .into_iter()
        .filter(|(mount, _, _)| folder.starts_with(mount))
        .max_by_key(|(mount, _, _)| mount.as_os_str().len())?;
    (total > 0).then(|| (mount.display().to_string(), total.saturating_sub(available), total))
}

/// Where the projects go, as far as it exists: the folder itself, else the nearest folder above it that does. A
/// folder that isn't a full path on this OS (the Windows default `C:\DNN` on a Mac) gives the home folder.
#[cfg_attr(windows, allow(dead_code))]
fn existing_folder(folder: &Path, home: Option<&Path>) -> Option<std::path::PathBuf> {
    let start = if folder.is_absolute() { folder } else { home? };
    start.ancestors().find_map(|a| std::fs::canonicalize(a).ok())
}

#[cfg(not(windows))]
mod unix {
    use std::path::Path;

    use sysinfo::{Disks, System};

    #[derive(Debug)]
    pub(super) struct Probe {
        system: System,
        disks: Disks,
        /// The processors were read once: sysinfo's next reading is the use since then.
        cpu_read: bool,
    }

    impl Probe {
        pub(super) fn new() -> Self {
            Self { system: System::new(), disks: Disks::new(), cpu_read: false }
        }

        /// In use and the total. In use is what isn't available: memory macOS or Linux can give back at once (the
        /// file cache) counts as free, as Activity Monitor and `free` count it.
        pub(super) fn memory(&mut self) -> Option<(u64, u64)> {
            self.system.refresh_memory();
            let total = self.system.total_memory();
            (total > 0).then(|| (self.system.used_memory().min(total), total))
        }

        pub(super) fn cpu_percent(&mut self) -> Option<f64> {
            self.system.refresh_cpu_usage();
            let read_before = std::mem::replace(&mut self.cpu_read, true);
            let usage = f64::from(self.system.global_cpu_usage());
            (read_before && usage.is_finite()).then(|| usage.clamp(0.0, 100.0))
        }

        pub(super) fn disk(&mut self, folder: &Path) -> Option<(String, u64, u64)> {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let folder = super::existing_folder(folder, home.as_deref())?;
            // Listed again each time: a disk mounted since is seen.
            self.disks.refresh_list();
            super::disk_of(
                &folder,
                self.disks.list().iter().map(|d| (d.mount_point(), d.total_space(), d.available_space())),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_is_on_the_disk_mounted_deepest() {
        let mounts = [
            (Path::new("/"), 500, 200),
            (Path::new("/System/Volumes/Data"), 500, 120),
            (Path::new("/Volumes/Projects"), 1000, 900),
        ];
        assert_eq!(disk_of(Path::new("/Volumes/Projects/DNN"), mounts), Some(("/Volumes/Projects".into(), 100, 1000)));
        assert_eq!(disk_of(Path::new("/Users/me/DNN"), mounts), Some(("/".into(), 300, 500)));
        assert_eq!(disk_of(Path::new("/Users/me"), [(Path::new("/"), 0, 0)]), None, "no size: unknown, not 0");
        assert_eq!(disk_of(Path::new("/Users/me"), [(Path::new("/Volumes/x"), 1, 1)]), None);
    }

    #[test]
    fn a_folder_not_made_yet_is_on_its_parents_disk() {
        let dir = tempfile::tempdir().unwrap();
        let found = existing_folder(&dir.path().join("not").join("made"), None).unwrap();
        assert_eq!(found, std::fs::canonicalize(dir.path()).unwrap());
        // Not a full path here (C:\DNN on a Mac is relative): the home folder's disk.
        let relative = Path::new("relative-folder");
        assert_eq!(existing_folder(relative, Some(dir.path())), Some(found));
        assert_eq!(existing_folder(relative, None), None);
    }

    /// The real readings on whatever OS runs the test: memory and the disk at once, the processors from the second
    /// sample on.
    #[test]
    fn reads_memory_cpu_and_the_disk_on_this_os() {
        let folder = std::env::temp_dir();
        let mut sampler = HostSampler::default();
        let first = sampler.sample(&folder);
        assert!(first.memory_total_bytes > first.memory_used_bytes && first.memory_used_bytes > 0, "{first:?}");
        assert!(first.cpu_percent.is_none(), "use is the time between two samples");
        assert!(first.disk_root.is_some() && first.disk_total_bytes > 0, "{first:?}");
        assert!(first.disk_total_bytes >= first.disk_used_bytes);
        // sysinfo needs 200 ms between two processor readings.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let second = sampler.sample(&folder);
        assert!(second.cpu_percent.is_some_and(|c| (0.0..=100.0).contains(&c)), "{second:?}");
    }

    #[cfg(windows)]
    #[test]
    fn reads_memory_cpu_and_the_drive() {
        let mut sampler = HostSampler::default();
        let first = sampler.sample(Path::new(r"C:\DNN"));
        assert!(first.memory_total_bytes > first.memory_used_bytes && first.memory_used_bytes > 0);
        assert!(first.cpu_percent.is_none());
        assert_eq!(first.disk_root.as_deref(), Some(r"C:\"));
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = sampler.sample(Path::new(r"C:\DNN"));
        assert!(second.cpu_percent.is_some_and(|c| (0.0..=100.0).contains(&c)));
    }
}
