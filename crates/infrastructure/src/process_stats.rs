//! The worker processes' CPU, memory, I/O and start time (`ProcessSampler`): working set, CPU time since the last
//! sample divided over the processors, I/O transfer counts since the process started.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use dnnm_application::project::ProcessStats;

/// One process's figures at one moment.
#[derive(Debug, Clone, Copy)]
struct Sample {
    cpu_time: Duration,
    at: Instant,
}

/// Keeps the previous CPU time of each process, so the next sample can tell the CPU used in between.
#[derive(Debug, Default)]
pub struct ProcessSampler {
    previous: HashMap<u32, Sample>,
}

impl ProcessSampler {
    /// The group's figures; None when none of the processes could be read.
    pub fn sample(&mut self, pids: &[u32]) -> Option<ProcessStats> {
        let processors = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let total_memory = total_physical_memory().unwrap_or(0);
        let mut stats = ProcessStats {
            cpu_percent: None,
            memory_bytes: 0,
            memory_percent: 0.0,
            read_bytes: 0,
            write_bytes: 0,
            started_ago: None,
        };
        let mut read_any = false;
        for &pid in pids {
            let Some(figures) = read_process(pid) else {
                self.previous.remove(&pid);
                continue;
            };
            read_any = true;
            let now = Instant::now();
            if let Some(before) = self.previous.get(&pid) {
                let elapsed = now.duration_since(before.at).as_secs_f64();
                if elapsed > 0.0 {
                    let used = figures.cpu_time.saturating_sub(before.cpu_time).as_secs_f64();
                    *stats.cpu_percent.get_or_insert(0.0) += used / elapsed / processors * 100.0;
                }
            }
            self.previous.insert(pid, Sample { cpu_time: figures.cpu_time, at: now });
            stats.memory_bytes += figures.working_set;
            stats.read_bytes += figures.read;
            stats.write_bytes += figures.write;
            // The oldest process: the site has been running since then.
            stats.started_ago = Some(stats.started_ago.map_or(figures.age, |a: Duration| a.max(figures.age)));
        }
        if total_memory > 0 {
            stats.memory_percent = stats.memory_bytes as f64 / total_memory as f64 * 100.0;
        }
        read_any.then_some(stats)
    }

    /// Forgets the processes that are gone, after a read of every group.
    pub fn keep_only(&mut self, pids: &std::collections::HashSet<u32>) {
        self.previous.retain(|pid, _| pids.contains(pid));
    }
}

struct Figures {
    cpu_time: Duration,
    working_set: u64,
    read: u64,
    write: u64,
    age: Duration,
}

#[cfg(windows)]
fn read_process(pid: u32) -> Option<Figures> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows::Win32::System::Threading::{
        GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn ticks(t: FILETIME) -> u64 {
        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
    }
    // SAFETY: the handle is closed before returning; every out-structure is sized as the call expects.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user).is_ok();
        let mut memory =
            PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        let has_memory = GetProcessMemoryInfo(handle, &mut memory, memory.cb).is_ok();
        let mut io = IO_COUNTERS::default();
        let has_io = GetProcessIoCounters(handle, &mut io).is_ok();
        let _ = CloseHandle(handle);
        if !times {
            return None;
        }
        let now = ticks(GetSystemTimeAsFileTime());
        // FILETIME counts 100 ns.
        let cpu_time = Duration::from_nanos((ticks(kernel) + ticks(user)) * 100);
        let age = Duration::from_nanos(now.saturating_sub(ticks(created)) * 100);
        Some(Figures {
            cpu_time,
            working_set: if has_memory { memory.WorkingSetSize as u64 } else { 0 },
            read: if has_io { io.ReadTransferCount } else { 0 },
            write: if has_io { io.WriteTransferCount } else { 0 },
            age,
        })
    }
}

#[cfg(not(windows))]
fn read_process(_pid: u32) -> Option<Figures> {
    None
}

#[cfg(windows)]
fn total_physical_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: dwLength is set as the call requires.
    unsafe { GlobalMemoryStatusEx(&mut status).ok()? };
    Some(status.ullTotalPhys)
}

#[cfg(not(windows))]
fn total_physical_memory() -> Option<u64> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn samples_this_process_and_gets_cpu_on_the_second_sample() {
        let mut sampler = ProcessSampler::default();
        let pid = std::process::id();
        let first = sampler.sample(&[pid]).unwrap();
        assert!(first.memory_bytes > 0 && first.cpu_percent.is_none() && first.memory_percent > 0.0);
        let mut x = 0u64;
        for i in 0..2_000_000u64 {
            x = x.wrapping_add(i * i);
        }
        assert!(x > 0);
        let second = sampler.sample(&[pid]).unwrap();
        assert!(second.cpu_percent.is_some());
    }

    #[test]
    fn a_missing_process_is_none() {
        assert!(ProcessSampler::default().sample(&[u32::MAX - 1]).is_none());
    }
}
