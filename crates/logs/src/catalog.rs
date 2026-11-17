//! A website's logs and DNN Manager's own (`SiteLogCatalog`, `SiteLogSource`, `EventLogFilter` in
//! `DnnManager.Infrastructure/SiteLogs/SiteLogs.cs`): DNN's log files, IIS's request logs and HTTP.sys's errors, the
//! Windows event log entries about the site - and DNN Manager's daily log and the hosts file.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dnnm_infrastructure::process::system32;

use crate::clock::short_date_time;
use crate::event_log::EventLogTail;
use crate::tail::{FileTail, LogTail, WholeFileTail};

/// The group of DNN Manager's own log files.
pub const APP_GROUP: &str = "DNN Manager";
/// The hosts file in DNN Manager's list: a section of its own, first.
pub const HOSTS_GROUP: &str = "Windows";
pub const HOSTS_TITLE: &str = "Hosts file";

const FILES_PER_KIND: usize = 8;

/// Which Windows event log entries a log shows: of these providers, in that log, mentioning that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogFilter {
    pub log_name: String,
    pub providers: Vec<String>,
    pub message_contains: Option<String>,
}

/// One log a website has - a file (DNN's log, IIS's request log…) or entries of a Windows event log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteLogSource {
    /// What writes it: "DNN", "IIS", "Windows" - or "DNN Manager" for DNN Manager's own log.
    pub group: String,
    /// What it is, as the menu and the Logs tab name it: "DNN log - 2026.10.01".
    pub title: String,
    /// Where it is: the file's path with its size and time, or the event log and its filter.
    pub description: String,
    pub file_path: Option<PathBuf>,
    pub events: Option<EventLogFilter>,
    /// A file that is rewritten rather than added to (the hosts file): shown whole, and again whenever it changes.
    pub whole: bool,
}

impl SiteLogSource {
    /// Opens it: call `start` for its newest lines, then `poll` every `poll_every` for new ones.
    pub fn open(&self) -> Box<dyn LogTail> {
        match (&self.file_path, &self.events) {
            (Some(file), _) if self.whole => Box::new(WholeFileTail::new(file.clone())),
            (Some(file), _) => Box::new(FileTail::new(file.clone())),
            (None, Some(filter)) => Box::new(EventLogTail::new(filter.clone())),
            (None, None) => Box::new(WholeFileTail::new(PathBuf::new())),
        }
    }

    /// The catalog makes its sources anew each time it is asked: the same log is the same kind and name.
    pub fn same_log(&self, other: &SiteLogSource) -> bool {
        self.group == other.group
            && self.title == other.title
            && self.file_path == other.file_path
            && self.events.as_ref().map(|e| (&e.log_name, &e.message_contains))
                == other.events.as_ref().map(|e| (&e.log_name, &e.message_contains))
    }

    /// One of DNN Manager's own entries - its logs or the hosts file - not a site's.
    pub fn is_app_log(&self) -> bool {
        is_app_log(&self.group, &self.title)
    }
}

/// One of DNN Manager's own entries - its logs or the hosts file - not a site's. A site's "Windows" section has no
/// "Hosts file".
pub fn is_app_log(group: &str, title: &str) -> bool {
    group == APP_GROUP || group == HOSTS_GROUP && title == HOSTS_TITLE
}

/// `C:\Windows\System32\drivers\etc\hosts`.
pub fn hosts_file() -> PathBuf {
    system32(r"drivers\etc\hosts")
}

/// `Documents\DnnManager\logs` - where the daily log files are (`AppDataPaths.LogsDirectory`).
pub fn logs_directory() -> PathBuf {
    dnnm_infrastructure::paths::app_data_directory().join("logs")
}

/// The hosts file, in a section of its own; then DNN Manager's own log - a file a day, `logs\dnnmanager-yyyyMMdd.log` -
/// newest first, named by its day (`SiteLogCatalog.ForApp`). Tests pass folders of their own.
pub fn for_app(logs_directory: &Path, hosts_file: &Path) -> Vec<SiteLogSource> {
    let mut logs = Vec::new();
    if hosts_file.is_file() {
        let mut hosts = file_source(HOSTS_GROUP, HOSTS_TITLE.into(), hosts_file);
        hosts.whole = true;
        logs.push(hosts);
    }
    for file in newest(logs_directory, |name| has_prefix(name, "dnnmanager-") && has_suffix(name, ".log")) {
        if logs.iter().filter(|l| l.group == APP_GROUP).count() == FILES_PER_KIND {
            break;
        }
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let day = &name["dnnmanager-".len()..name.len() - ".log".len()];
        if let Some(day) = parse_day(day) {
            logs.push(file_source(APP_GROUP, format!("DNN Manager log - {day}"), &file));
        }
    }
    logs
}

/// The logs of site `site_name` (ID `site_id`), newest files first within each kind (`SiteLogCatalog.For`).
/// `iis_logs` is the site's request log folder: `IisManager::log_directory(site_name)`.
pub fn for_site(site_id: u64, directory: &Path, app_pool: &str, iis_logs: Option<&Path>) -> Vec<SiteLogSource> {
    for_site_with(site_id, directory, app_pool, iis_logs, &system32(r"LogFiles\HTTPERR"))
}

fn for_site_with(
    site_id: u64,
    directory: &Path,
    app_pool: &str,
    iis_logs: Option<&Path>,
    http_err: &Path,
) -> Vec<SiteLogSource> {
    let mut logs = Vec::new();
    // DNN 9 writes log4net files named by day ("2026.10.01.log.resources"); older versions plain ".log".
    let dnn_logs = directory.join("Portals").join("_default").join("Logs");
    let resources = newest(&dnn_logs, |n| has_suffix(n, ".log.resources"));
    let plain = newest(&dnn_logs, |n| has_suffix(n, ".log"));
    for file in resources.into_iter().chain(plain).take(FILES_PER_KIND) {
        logs.push(file_source("DNN", format!("DNN log - {}", log_name(&file)), &file));
    }
    if let Some(iis_logs) = iis_logs {
        for file in newest(iis_logs, |n| has_suffix(n, ".log")).into_iter().take(FILES_PER_KIND) {
            logs.push(file_source("IIS", format!("IIS requests - {}", log_name(&file)), &file));
        }
    }
    if let Some(file) = newest(http_err, |n| has_prefix(n, "httperr") && has_suffix(n, ".log")).into_iter().next() {
        logs.push(file_source("IIS", "HTTP.sys errors (all sites)".into(), &file));
    }

    // ASP.NET's errors name the application as /LM/W3SVC/<id>/ROOT; WAS names the app pool in quotes.
    logs.push(event_source(
        "ASP.NET errors and warnings",
        "Application event log - ASP.NET, this site",
        "Application",
        &["ASP.NET 4.0.30319.0", "ASP.NET 2.0.50727.0"],
        format!("/LM/W3SVC/{site_id}/"),
    ));
    if !app_pool.is_empty() {
        logs.push(event_source(
            "App pool events",
            &format!("System event log - IIS (WAS), app pool '{app_pool}'"),
            "System",
            &["Microsoft-Windows-WAS"],
            format!("'{app_pool}'"),
        ));
    }
    logs.push(event_source(
        "Worker process crashes",
        "Application event log - .NET Runtime and Application Error, w3wp.exe",
        "Application",
        &[".NET Runtime", "Application Error"],
        "w3wp.exe".into(),
    ));
    logs
}

fn event_source(title: &str, description: &str, log: &str, providers: &[&str], contains: String) -> SiteLogSource {
    SiteLogSource {
        group: "Windows".into(),
        title: title.into(),
        description: description.into(),
        file_path: None,
        events: Some(EventLogFilter {
            log_name: log.into(),
            providers: providers.iter().map(|p| p.to_string()).collect(),
            message_contains: Some(contains),
        }),
        whole: false,
    }
}

/// The files in `folder` whose name matches, newest written first; none when it can't be read.
fn newest(folder: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_str().is_some_and(&matches))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH), e.path()))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().map(|(_, p)| p).collect()
}

fn has_prefix(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name.is_char_boundary(prefix.len())
        && name[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn has_suffix(name: &str, suffix: &str) -> bool {
    let start = name.len().wrapping_sub(suffix.len());
    name.len() >= suffix.len() && name.is_char_boundary(start) && name[start..].eq_ignore_ascii_case(suffix)
}

/// `20261004` → `2026-10-04`; None when it isn't a day.
fn parse_day(day: &str) -> Option<String> {
    if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d): (u32, u32, u32) = (day[..4].parse().ok()?, day[4..6].parse().ok()?, day[6..].parse().ok()?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let valid = y >= 1 && (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1];
    valid.then(|| format!("{y:04}-{m:02}-{d:02}"))
}

fn log_name(file: &Path) -> String {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if has_suffix(name, ".log.resources") {
        return name[..name.len() - ".log.resources".len()].to_string();
    }
    file.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

fn file_source(group: &str, title: String, file: &Path) -> SiteLogSource {
    let (length, written) = std::fs::metadata(file)
        .map(|m| (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        .unwrap_or((0, SystemTime::UNIX_EPOCH));
    SiteLogSource {
        group: group.into(),
        title,
        description: format!("{}  ({}, {})", file.display(), size(length), short_date_time(written)),
        file_path: Some(file.to_path_buf()),
        events: None,
        whole: false,
    }
}

/// `{0:0.#} KB` under a megabyte, else MB - in the user's decimal separator.
fn size(bytes: u64) -> String {
    let decimal = dnnm_infrastructure::locale::number_separators().map_or('.', |(d, _)| d);
    let (value, unit) =
        if bytes < 1024 * 1024 { (bytes as f64 / 1024.0, "KB") } else { (bytes as f64 / 1024.0 / 1024.0, "MB") };
    format!("{} {unit}", one_decimal(value, decimal))
}

/// .NET's `0.#`: one decimal, left out when it is 0.
pub(crate) fn one_decimal(value: f64, decimal: char) -> String {
    let text = format!("{:.1}", (value * 10.0).round() / 10.0);
    let text = text.strip_suffix(".0").unwrap_or(&text);
    text.replace('.', &decimal.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    fn write(path: &Path, text: &str, age_days: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        let time = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        fs::File::options().write(true).open(path).unwrap().set_modified(time).unwrap();
    }

    #[test]
    fn the_daily_files_are_listed_newest_first_by_their_day() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        for (day, age) in [("20261002", 2), ("20261004", 0), ("20261003", 1)] {
            write(&logs.join(format!("dnnmanager-{day}.log")), "12:00:00 Started\r\n", age);
        }
        write(&logs.join("other.log"), "not DNN Manager's", 0);
        write(&logs.join("dnnmanager-20261399.log"), "not a day", 0);

        let found = for_app(&logs, &dir.path().join("no-hosts"));

        let titles: Vec<_> = found.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(
            titles,
            ["DNN Manager log - 2026-10-04", "DNN Manager log - 2026-10-03", "DNN Manager log - 2026-10-02"]
        );
        assert!(found.iter().all(|l| l.group == APP_GROUP && l.file_path.is_some() && !l.whole));
    }

    #[test]
    fn the_hosts_file_is_listed_first_in_a_section_of_its_own_shown_whole() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        write(&logs.join("dnnmanager-20261007.log"), "12:00:00 Started\r\n", 0);
        let hosts = dir.path().join("hosts");
        write(&hosts, "127.0.0.1 localhost\r\n", 0);

        let found = for_app(&logs, &hosts);

        let pairs: Vec<_> = found.iter().map(|l| (l.group.as_str(), l.title.as_str())).collect();
        assert_eq!(pairs, [(HOSTS_GROUP, HOSTS_TITLE), (APP_GROUP, "DNN Manager log - 2026-10-07")]);
        assert!(found[0].whole && found[0].file_path.as_deref() == Some(hosts.as_path()));
        assert!(found.iter().all(SiteLogSource::is_app_log));
        assert!(!is_app_log("Windows", "ASP.NET errors and warnings"));
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn a_site_lists_dnn_iis_http_sys_and_the_event_logs() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        let dnn = site.join(r"Portals\_default\Logs");
        write(&dnn.join("2026.10.01.log.resources"), "a\n", 1);
        write(&dnn.join("2026.10.02.log.resources"), "b\n", 0);
        write(&dnn.join("old.log"), "c\n", 0);
        for i in 0..10 {
            write(&dnn.join(format!("2026.09.{i:02}.log.resources")), "x\n", 10 + i);
        }
        let iis = dir.path().join("W3SVC3");
        write(&iis.join("u_ex261002.log"), "#Fields\n", 0);
        let http_err = dir.path().join("HTTPERR");
        write(&http_err.join("httperr1.log"), "#Fields\n", 1);
        write(&http_err.join("httperr2.log"), "#Fields\n", 0);

        let found = for_site_with(3, &site, "dnn10", Some(&iis), &http_err);

        let titles: Vec<_> = found.iter().map(|l| format!("{}|{}", l.group, l.title)).collect();
        assert_eq!(
            titles[..2],
            ["DNN|DNN log - 2026.10.02".to_string(), "DNN|DNN log - 2026.10.01".to_string()],
            "Newest first, by their day"
        );
        assert_eq!(found.iter().filter(|l| l.group == "DNN").count(), 8, "Eight files per kind");
        assert!(!titles.contains(&"DNN|DNN log - old".to_string()), "The .log.resources fill the eight first");
        assert_eq!(
            titles[8..],
            [
                "IIS|IIS requests - u_ex261002",
                "IIS|HTTP.sys errors (all sites)",
                "Windows|ASP.NET errors and warnings",
                "Windows|App pool events",
                "Windows|Worker process crashes"
            ]
            .map(String::from)
        );
        assert!(found[9].file_path.as_ref().unwrap().ends_with("httperr2.log"), "The newest HTTP.sys log only");
        let asp = found[10].events.as_ref().unwrap();
        assert_eq!(asp.message_contains.as_deref(), Some("/LM/W3SVC/3/"));
        assert_eq!(asp.providers, ["ASP.NET 4.0.30319.0", "ASP.NET 2.0.50727.0"]);
        assert_eq!(found[11].description, "System event log - IIS (WAS), app pool 'dnn10'");
        assert_eq!(found[11].events.as_ref().unwrap().message_contains.as_deref(), Some("'dnn10'"));
        assert!(found[8].description.starts_with(&format!("{}  (0 KB, ", iis.join("u_ex261002.log").display())));
    }

    #[test]
    fn no_app_pool_no_app_pool_events() {
        let dir = tempfile::tempdir().unwrap();
        let found = for_site_with(1, dir.path(), "", None, &dir.path().join("none"));
        let titles: Vec<_> = found.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(titles, ["ASP.NET errors and warnings", "Worker process crashes"]);
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn sizes_and_days() {
        assert_eq!(one_decimal(0.0, ','), "0");
        assert_eq!(one_decimal(1.5, ','), "1,5");
        assert_eq!(one_decimal(2.04, '.'), "2");
        assert_eq!(parse_day("20240229").as_deref(), Some("2024-02-29"));
        assert_eq!(parse_day("20250229"), None);
        assert_eq!(parse_day("2026-10-04"), None);
        assert_eq!(log_name(Path::new(r"C:\x\2026.10.01.log.resources")), "2026.10.01");
        assert_eq!(log_name(Path::new(r"C:\x\u_ex261001.log")), "u_ex261001");
    }

    #[test]
    fn the_same_log_is_the_same_kind_and_name() {
        let a = &for_site_with(1, Path::new("x"), "p", None, Path::new("none"))[0];
        let mut b = a.clone();
        b.events.as_mut().unwrap().providers.clear();
        assert!(a.same_log(&b), "The providers aren't compared");
        b.title = "other".into();
        assert!(!a.same_log(&b));
    }
}
