//! Reading what DNN logs - its own logs (`Portals\_default\Logs`) and its database scripts' (`Providers\DataProviders\
//! SqlDataProvider`) - the same way for the installer and the upgrade's checks (`src/DnnManager.Infrastructure/Dnn/
//! DnnLogFiles.cs`, with `DnnInstaller.LogTime`).

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

/// The file's lines, read while DNN may still be writing it (Rust opens files shared for reading, writing and
/// deleting); none when it can't be read.
pub fn read_shared(file: &Path) -> Vec<String> {
    match std::fs::read(file) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            text.lines().map(str::to_string).collect()
        }
        Err(_) => Vec::new(),
    }
}

/// A log line short enough for a message: its first 300 characters.
pub fn short(text: &str) -> String {
    if text.chars().count() > 300 {
        format!("{}\u{2026}", text.chars().take(300).collect::<String>())
    } else {
        text.to_string()
    }
}

/// When the file was last written; None when it can't be told.
pub fn modified(file: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(file).and_then(|m| m.modified()).ok().map(DateTime::<Utc>::from)
}

/// The files in `folder` whose names end with `suffix` (case ignored) - `Directory.EnumerateFiles(folder, "*suffix")`.
pub fn files_ending(folder: &Path, prefix: &str, suffix: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            name.starts_with(&prefix.to_lowercase()) && name.ends_with(&suffix.to_lowercase())
        })
        .collect();
    files.sort();
    files
}

/// DNN's database scripts that reported a problem since `since`, with the first line they logged: a script that ran
/// cleanly leaves an empty log behind.
pub fn script_problems(site_directory: &Path, since: DateTime<Utc>) -> Vec<(String, String)> {
    let scripts = site_directory.join("Providers").join("DataProviders").join("SqlDataProvider");
    files_ending(&scripts, "", ".log.resources")
        .into_iter()
        .filter(|f| modified(f).is_some_and(|m| m >= since))
        .filter_map(|file| {
            let first = read_shared(&file).into_iter().find(|l| !l.trim().trim_matches('\u{feff}').is_empty())?;
            Some((file.file_name()?.to_string_lossy().into_owned(), short(&first)))
        })
        .collect()
}

/// The local time a DNN log line starts with: DNN 9 writes "2026-10-01 14:39:20,123 [MACHINE][Thread:12][ERROR] …",
/// DNN 10 "2026-10-01 14:39:20.123+02:00 [MACHINE][D:2][T:28][ERROR] …" - the first 19 characters are the same.
pub fn log_time(line: &str) -> Option<DateTime<Local>> {
    let start = line.get(..19)?;
    let naive = NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

/// A log entry's first line starts with its date: 2026-10-10 18:01:49 (Serilog and log4net alike).
pub fn starts_with_date(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() >= 10 && b[0].is_ascii_digit() && b[4] == b'-' && b[7] == b'-'
}

/// Now, for the times DNN Manager compares with the logs.
pub fn now() -> DateTime<Utc> {
    DateTime::<Utc>::from(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_times_in_dnn_9s_and_10s_formats() {
        let nine = log_time("2026-10-04 23:37:54,504 [PC][Thread:9][ERROR] x").unwrap();
        let ten = log_time("2026-10-04 23:37:54.504+02:00 [PC][D:2][T:28][ERROR] x").unwrap();
        assert_eq!(nine, ten);
        assert!(log_time("Acme audit: version change refused").is_none());
        assert!(starts_with_date("2026-10-04 23:37"));
        assert!(!starts_with_date("   at System.IO"));
    }

    #[test]
    fn a_script_that_ran_cleanly_leaves_an_empty_log() {
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("Providers").join("DataProviders").join("SqlDataProvider");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("10.03.00.log.resources"), "\u{feff}\r\n  \r\nError in UpdateDatabaseVersion\r\n")
            .unwrap();
        std::fs::write(scripts.join("10.02.00.log.resources"), "").unwrap();
        let since = now() - chrono::Duration::minutes(5);
        assert_eq!(
            script_problems(dir.path(), since),
            [("10.03.00.log.resources".to_string(), "Error in UpdateDatabaseVersion".to_string())]
        );
        assert_eq!(short(&"x".repeat(301)).chars().count(), 301);
    }
}
