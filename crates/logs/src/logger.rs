//! The app's own warnings and errors to the daily log file (`DailyLogFileLoggerProvider` in
//! `DnnManager.Infrastructure/Files/DailyLogFileLogger.cs`), as a backend of the `log` crate:
//!
//! ```text
//! 14:58:19 [warning] iis: Could not read IIS site states
//! ```
//!
//! The same message from the same place is written once per 10 minutes: a background read that fails on every tick
//! doesn't fill the file. The `log` crate has no exceptions and no Critical level: an error is `[error]`, and the
//! lines after a message's first are indented - a code block in the Markdown the file reads as, as the C# indents an
//! exception's stack trace.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::daily_file::DailyLogFile;

pub const REPEAT_AFTER: Duration = Duration::from_secs(10 * 60);

/// A `log::Log` writing warnings and errors to the day's file.
pub struct FileLogger {
    file: Arc<DailyLogFile>,
    last_written: Mutex<HashMap<String, Instant>>,
}

impl FileLogger {
    pub fn new(file: Arc<DailyLogFile>) -> Self {
        Self { file, last_written: Mutex::new(HashMap::new()) }
    }

    /// Makes it the process's logger - once, at start (`builder.Logging…AddSingleton<ILoggerProvider, …>`).
    pub fn init(file: Arc<DailyLogFile>) -> Result<(), log::SetLoggerError> {
        log::set_boxed_logger(Box::new(Self::new(file)))?;
        log::set_max_level(LevelFilter::Warn);
        Ok(())
    }
}

/// The last part of a module path: `dnnm_infrastructure::iis` → `iis` (the C# keeps the class name).
fn category(target: &str) -> &str {
    target.rsplit("::").next().unwrap_or(target)
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let category = category(record.target());
        let message = record.args().to_string();
        let now = Instant::now();
        {
            let mut last = self.last_written.lock().unwrap_or_else(|e| e.into_inner());
            let key = format!("{category}|{message}");
            if last.get(&key).is_some_and(|at| now.duration_since(*at) < REPEAT_AFTER) {
                return;
            }
            // Bounded: messages naming a site or a file can be many over a long run.
            if last.len() > 500 {
                last.clear();
            }
            last.insert(key, now);
        }
        let tag = if record.level() == Level::Warn { "warning" } else { "error" };
        let text = format!("[{tag}] {category}: {}", message.trim_end().replace('\n', "\n    "));
        self.file.append(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
    }

    fn flush(&self) {
        self.file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn warnings_and_errors_are_written_and_a_repeat_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = DailyLogFile::open(dir.path().to_path_buf());
        let logger = FileLogger::new(Arc::clone(&file));
        let log = |level, text: &str| {
            logger.log(
                &Record::builder().level(level).target("dnnm_infrastructure::iis").args(format_args!("{text}")).build(),
            )
        };

        log(Level::Info, "Not interesting");
        for _ in 0..3 {
            log(Level::Warn, "Could not read IIS site states\nAccess is denied.");
        }
        log(Level::Error, "Failed to write web.config");
        file.close();

        let path = dir.path().join(crate::daily_file::file_name(&crate::clock::LocalDateTime::now()));
        let mut text = String::new();
        crate::tail::open_shared(&path).unwrap().read_to_string(&mut text).unwrap();
        assert!(!text.contains("Not interesting"), "Information isn't written");
        assert!(text.contains("[warning] iis: Could not read IIS site states\r\n    Access is denied.\r\n"), "{text}");
        assert_eq!(
            text.matches("Could not read IIS site states").count(),
            1,
            "A repeat within ten minutes is left out"
        );
        assert!(text.contains("[error] iis: Failed to write web.config"));
    }
}
