//! This PC's figures for the status bar (`HostResourceMonitor`): memory in use, processor use since the last sample,
//! and the projects folder's drive.

use std::path::Path;

/// One reading of the PC.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostResources {
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    /// None on the first sample: CPU use is the time between two.
    pub cpu_percent: Option<f64>,
    /// The drive's root, `C:\`; None when the folder's drive can't be read.
    pub disk_root: Option<String>,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
}

/// Keeps the previous processor times, so each sample tells the use in between.
#[derive(Debug, Default)]
pub struct HostSampler {
    previous: Option<(u64, u64)>,
}

impl HostSampler {
    pub fn sample(&mut self, projects_folder: &Path) -> HostResources {
        let mut r = HostResources::default();
        if let Some((used, total)) = memory() {
            r.memory_used_bytes = used;
            r.memory_total_bytes = total;
        }
        if let Some((idle, busy_and_idle)) = cpu_times() {
            if let Some((idle0, total0)) = self.previous {
                let total = busy_and_idle.saturating_sub(total0);
                if total > 0 {
                    let idle = idle.saturating_sub(idle0);
                    r.cpu_percent = Some((total.saturating_sub(idle)) as f64 / total as f64 * 100.0);
                }
            }
            self.previous = Some((idle, busy_and_idle));
        }
        if let Some((root, used, total)) = disk(projects_folder) {
            r.disk_root = Some(root);
            r.disk_used_bytes = used;
            r.disk_total_bytes = total;
        }
        r
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

#[cfg(not(windows))]
fn memory() -> Option<(u64, u64)> {
    None
}
#[cfg(not(windows))]
fn cpu_times() -> Option<(u64, u64)> {
    None
}
#[cfg(not(windows))]
fn disk(_: &Path) -> Option<(String, u64, u64)> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

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
