//! DNN Manager's own log (`DailyLogFile` in `DnnManager.Infrastructure/Files/DailyLogFile.cs`): lines appended to
//! `logs\dnnmanager-yyyyMMdd.log` in the user's DNN Manager folder, a new file each day.
//!
//! - Files older than 30 days go, and those of the name before 1.7.4 (`dnnmanager-yyyy-MM-dd.log`) - at the start and
//!   at each new day. A day's file stops at 50 MB, saying so; all the files together stay under 200 MB, oldest first.
//! - A file deleted while it is written to (Clean up data, or by hand) is started again with the next line.
//! - Lines reach the disk in batches every 2 s (a thread of its own): most come from the UI thread, which mustn't wait
//!   for the disk line by line. A warning, an error and how an operation ended reach it at once, and so does the first
//!   line of a new file. The rest goes at [`DailyLogFile::close`] (or when it is dropped).
//! - Best effort: a write that fails is tried again a minute later, without disturbing the app.
//!
//! Unlike the C#, the port doesn't write through a link or junction (`web_config::has_link`): DNN Manager runs
//! elevated and the Documents folder is the user's to change.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use dnnm_infrastructure::web_config::has_link;

use crate::clock::LocalDateTime;

const DAYS_KEPT: u64 = 30;
/// How large a day's file may get.
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// How large all the files in the logs folder may get together; the oldest go first.
pub const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
/// How long a line may wait in memory before it is written to the disk.
pub const FLUSH_EVERY: Duration = Duration::from_secs(2);
const RETRY_AFTER: Duration = Duration::from_secs(60);

// Lines that go to the disk at once: the tags the run log and the logger give warnings, errors and the end of an
// operation.
const FLUSHED_AT_ONCE: [&str; 5] = ["[warning]", "[error]", "[critical]", "[success]", "[cancelled]"];

/// Whether `line` goes to the disk at once rather than with the next batch.
pub fn is_urgent(line: &str) -> bool {
    FLUSHED_AT_ONCE.iter().any(|tag| line.starts_with(tag))
}

/// The day's file name: `dnnmanager-20261004.log`.
pub fn file_name(day: &LocalDateTime) -> String {
    format!("dnnmanager-{}.log", day.day_key())
}

#[derive(Default)]
struct State {
    writer: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    day: Option<(u16, u8, u8)>,
    // Bytes in the file, flushed or not.
    length: u64,
    // The day's file reached MAX_BYTES: nothing more is written to it.
    full: bool,
    // Lines written since the last flush.
    pending: bool,
    // After a write failed: nothing is written before this.
    retry_at: Option<Instant>,
    // Closed (the app is ending): a line that still comes is written at once - there is no thread to flush it.
    closed: bool,
}

/// The day's log file. Shared: `Arc<DailyLogFile>` for the logger and the run log.
pub struct DailyLogFile {
    directory: PathBuf,
    state: Arc<Mutex<State>>,
    flusher: Mutex<Option<(Sender<()>, JoinHandle<()>)>>,
}

impl DailyLogFile {
    /// Writes to `directory` - `catalog::logs_directory()` in the app, a temporary folder in tests.
    pub fn open(directory: PathBuf) -> Arc<Self> {
        delete_old_files(&directory, None);
        let state = Arc::new(Mutex::new(State::default()));
        let (stop, stopped) = mpsc::channel();
        let flushed = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("dnnmanager-log".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(FLUSH_EVERY) {
                    flush_state(&mut lock(&flushed));
                }
            })
            .ok();
        Arc::new(Self { directory, state, flusher: Mutex::new(thread.map(|t| (stop, t))) })
    }

    /// A line, after its time: `11:57:09 IIS site and app pool 'test-web' removed.`
    pub fn append(&self, line: &str) {
        self.append_at(LocalDateTime::now(), line);
    }

    pub fn append_at(&self, time: LocalDateTime, line: &str) {
        self.write(time, &format!("{} {line}", time.hms()), is_urgent(line));
    }

    fn write(&self, time: LocalDateTime, text: &str, urgent: bool) {
        let mut state = lock(&self.state);
        if state.retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let result = (|| {
            let opened = self.writer(&mut state, time)?;
            if state.full {
                return Ok(());
            }
            let closed = state.closed;
            let length = state.length;
            let writer = state.writer.as_mut().expect("opened above");
            if length >= MAX_BYTES {
                write!(
                    writer,
                    "{} --- the log reached {} MB today: the rest of today's lines aren't kept ---\r\n",
                    time.hms(),
                    MAX_BYTES / 1_048_576
                )?;
                writer.flush()?;
                state.full = true;
                return Ok(());
            }
            writer.write_all(text.as_bytes())?;
            writer.write_all(b"\r\n")?;
            state.length += text.len() as u64 + 2;
            if urgent || opened || closed {
                state.writer.as_mut().expect("opened above").flush()?;
            } else {
                state.pending = true;
            }
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            failed(&mut state);
        }
    }

    /// The day's writer, opened when it isn't yet - a new day, or its file deleted meanwhile. True when it was opened.
    fn writer(&self, state: &mut State, time: LocalDateTime) -> std::io::Result<bool> {
        let day = (time.year, time.month, time.day);
        if state.writer.is_some() && state.day == Some(day) && state.path.as_ref().is_some_and(|p| p.is_file()) {
            return Ok(false);
        }
        let path = self.directory.join(file_name(&time));
        // A new day: the files too old to keep go, as at the start.
        if state.writer.is_some() && state.day != Some(day) {
            delete_old_files(&self.directory, Some(&path));
        }
        if state.day != Some(day) {
            state.full = false;
        }
        // Its file may be gone: nothing to say about a flush that fails.
        if let Some(mut old) = state.writer.take() {
            let _ = old.flush();
        }
        state.pending = false;
        std::fs::create_dir_all(&self.directory)?;
        let base = self.directory.parent().unwrap_or(&self.directory);
        if has_link(base, &path) {
            return Err(std::io::Error::other("the log folder or file is a link"));
        }
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Shared, so the log can be opened (or copied) - and deleted - while the app is running.
            options.share_mode(7);
        }
        let file = options.open(&path)?;
        state.length = file.metadata()?.len();
        state.writer = Some(BufWriter::with_capacity(64 * 1024, file));
        state.path = Some(path);
        state.day = Some(day);
        Ok(true)
    }

    /// Writes the lines waiting in memory to the disk - the thread's work, and the last thing before closing.
    pub fn flush(&self) {
        flush_state(&mut lock(&self.state));
    }

    /// The app is ending: what waits in memory is written, and a line that still comes is written at once.
    pub fn close(&self) {
        if let Some((stop, thread)) = lock(&self.flusher).take() {
            let _ = stop.send(());
            let _ = thread.join();
        }
        let mut state = lock(&self.state);
        if let Some(mut writer) = state.writer.take() {
            // Nowhere to say it.
            let _ = writer.flush();
        }
        state.pending = false;
        state.closed = true;
    }

    /// The folder the files are in.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl Drop for DailyLogFile {
    fn drop(&mut self) {
        self.close();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn flush_state(state: &mut State) {
    if !state.pending {
        return;
    }
    let Some(writer) = state.writer.as_mut() else { return };
    match writer.flush() {
        Ok(()) => state.pending = false,
        Err(_) => failed(state),
    }
}

/// A write or a flush failed: what is buffered is let go (it can't reach the file), and nothing is tried for a minute.
fn failed(state: &mut State) {
    state.retry_at = Some(Instant::now() + RETRY_AFTER);
    state.pending = false;
    if let Some(writer) = state.writer.take() {
        drop(writer.into_parts());
    }
}

/// Deletes the files older than 30 days and those of the old name, then the oldest until all together are under
/// [`MAX_TOTAL_BYTES`] - never `keep`, the file being written.
fn delete_old_files(directory: &Path, keep: Option<&Path>) {
    let base = directory.parent().unwrap_or(directory);
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    if has_link(base, directory) {
        return;
    }
    let cutoff = SystemTime::now() - Duration::from_secs(DAYS_KEPT * 86_400);
    let mut left = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() || meta.file_type().is_symlink() || !name.to_ascii_lowercase().ends_with(".log") {
            continue;
        }
        let written = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if written < cutoff || is_old_name(&name) {
            let _ = std::fs::remove_file(entry.path());
        } else {
            left.push((written, meta.len(), entry.path()));
        }
    }
    let mut total: u64 = left.iter().map(|f| f.1).sum();
    left.sort_by_key(|f| f.0);
    for (_, length, path) in left {
        if total <= MAX_TOTAL_BYTES {
            break;
        }
        if keep.is_some_and(|k| k.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy())) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= length;
        }
    }
}

/// `dnnmanager-2026-10-04.log` - the name before 1.7.4.
fn is_old_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let Some(day) = name.strip_prefix("dnnmanager-").and_then(|n| n.strip_suffix(".log")) else { return false };
    let b = day.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn read_shared(path: &Path) -> String {
        let mut text = String::new();
        crate::tail::open_shared(path).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    fn today_file(dir: &Path) -> PathBuf {
        dir.join(file_name(&LocalDateTime::now()))
    }

    #[test]
    fn a_log_deleted_while_written_to_is_started_again() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let log = DailyLogFile::open(logs.clone());
        let file = today_file(&logs);

        log.append("before");
        assert!(file.is_file());

        // Clean up data: the file goes while it is open - and so does the folder.
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&logs).unwrap();
        log.append("after");

        let text = read_shared(&file);
        assert!(text.contains("after") && !text.contains("before"), "{text}");
    }

    #[test]
    fn the_file_is_named_by_its_day_and_the_old_names_go_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let old = logs.join("dnnmanager-2026-10-03.log");
        let kept = logs.join("dnnmanager-20261003.log");
        std::fs::write(&old, "12:00:00 the name before 1.7.4").unwrap();
        std::fs::write(&kept, "12:00:00 yesterday").unwrap();

        let log = DailyLogFile::open(logs.clone());
        let day = LocalDateTime { year: 2026, month: 10, day: 4, hour: 9, minute: 30, second: 0 };
        log.append_at(day, "today");

        assert!(!old.exists(), "The old name is deleted");
        assert!(kept.exists(), "Yesterday's file stays");
        assert_eq!(file_name(&day), "dnnmanager-20261004.log");
        assert_eq!(read_shared(&logs.join("dnnmanager-20261004.log")), "09:30:00 today\r\n");
    }

    #[test]
    fn warnings_errors_and_the_end_of_an_operation_reach_the_disk_at_once() {
        for (line, urgent) in [
            ("[warning] IIS is busy", true),
            ("[error] Install stopped at Database", true),
            ("[critical] App: Unhandled fatal error", true),
            ("[success] Installed test-web (12 s)", true),
            ("[cancelled] Install was cancelled (3 s)", true),
            ("Copying files…", false),
            ("        → check the folder", false),
        ] {
            assert_eq!(is_urgent(line), urgent, "{line}");
        }
    }

    #[test]
    fn plain_lines_wait_for_the_next_batch_and_a_warning_takes_them_along() {
        let dir = tempfile::tempdir().unwrap();
        let log = DailyLogFile::open(dir.path().to_path_buf());
        let file = today_file(dir.path());
        log.append("first");
        assert!(read_shared(&file).contains("first"), "A new file shows its first line at once");
        log.append("second");
        assert!(!read_shared(&file).contains("second"), "A plain line waits (the flush comes after 2 s)");
        log.append("[warning] something went wrong");
        let text = read_shared(&file);
        assert!(text.contains("second") && text.contains("[warning] something went wrong"), "{text}");
    }

    #[test]
    fn what_is_still_in_memory_is_written_when_the_log_closes() {
        let dir = tempfile::tempdir().unwrap();
        let log = DailyLogFile::open(dir.path().to_path_buf());
        let file = today_file(dir.path());
        log.append("first");
        for i in 0..50 {
            log.append(&format!("line {i}"));
        }
        log.close();
        assert!(read_shared(&file).contains("line 49"));
        log.append("after closing");
        assert!(read_shared(&file).contains("after closing"), "A line after closing is written at once");
    }

    #[test]
    fn the_batch_reaches_the_disk_within_flush_every() {
        let dir = tempfile::tempdir().unwrap();
        let log = DailyLogFile::open(dir.path().to_path_buf());
        let file = today_file(dir.path());
        log.append("first");
        log.append("second");
        std::thread::sleep(FLUSH_EVERY + Duration::from_millis(700));
        assert!(read_shared(&file).contains("second"));
    }

    #[test]
    fn all_the_logs_together_stay_under_the_cap_the_oldest_going_first() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().to_path_buf();
        // Five days of 60 MB (sparse: nothing is written): 300 MB, over the 200 MB cap.
        let files: Vec<PathBuf> = (1..=5u64)
            .map(|day| {
                let at = SystemTime::now() - Duration::from_secs(day * 86_400);
                let path = logs.join(file_name(&LocalDateTime::from_system_time(at)));
                let file = File::create(&path).unwrap();
                file.set_len(60 * 1024 * 1024).unwrap();
                file.set_modified(at).unwrap();
                path
            })
            .collect();

        let _log = DailyLogFile::open(logs);

        let left: Vec<_> = files.iter().filter(|f| f.exists()).cloned().collect();
        assert_eq!(left, files[..3], "The newest are kept, the oldest went");
    }

    #[test]
    fn old_names_are_recognised() {
        assert!(is_old_name("dnnmanager-2026-10-04.log"));
        assert!(is_old_name("DnnManager-2026-10-04.LOG"));
        assert!(!is_old_name("dnnmanager-20261004.log"));
        assert!(!is_old_name("dnnmanager-2026-10-0x.log"));
    }
}
