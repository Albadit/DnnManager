//! The checks after each upgrade step, and the diagnostics when one fails (`src/DnnManager.Infrastructure/Dnn/
//! DnnUpgradeChecks.cs`, `IDnnUpgradeChecks`): the database read through `SiteSql`, the site through HTTP as a visitor
//! and as the accounts given, DNN's logs from its folder, and Windows' event logs.
//!
//! The C# reads the event logs with .NET's EventLogReader; the port asks System32's `wevtutil.exe qe` for the same
//! XPath query, rendered as XML, and reads it with roxmltree.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use dnnm_application::operation::{CANCELLED, CancelToken, Reporter};
use dnnm_infrastructure::web_config::DatabaseConnection;

use crate::install::DnnSiteAddress;
use crate::log_files::{files_ending, log_time, modified, read_shared, script_problems, short};
use crate::site_http::Session;
use crate::site_sql::{SitePages, SiteSql};
use crate::upgrades::{
    DnnContentItem, DnnPageState, DnnSiteState, DnnTestAccount, UpgradeFinding, UpgradeFindingSeverity, compare,
};
use crate::version::Version;

/// Pages sampled per portal besides its home page.
const PAGES_PER_PORTAL: usize = 6;

/// What DNN puts in a page when a module on it failed to load.
const MODULE_ERRORS: [&str; 3] =
    ["is currently unavailable", "A critical error has occurred", "An error has occurred."];

/// `IDnnUpgradeChecks`.
pub struct UpgradeChecks<S: SiteSql> {
    sql: S,
}

/// A page to open: its portal ("portal 0"), the alias it is reached on and its path.
type PageToCheck = (String, String, String);

fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or_default().trim().to_string()
}

impl<S: SiteSql> UpgradeChecks<S> {
    pub fn new(sql: S) -> Self {
        Self { sql }
    }

    /// The site as it is now - taken right before a step.
    pub fn capture(
        &self,
        site: &DnnSiteAddress,
        database: &DatabaseConnection,
        cancel: &CancelToken,
    ) -> Result<DnnSiteState, String> {
        let counts = self.sql.counts(database).ok().flatten();
        let (extensions, pages) = self.read_site(site, database);
        let mut states = Vec::new();
        for (portal, alias, path) in pages {
            cancel.check()?;
            let problem = problem(&alias, site.port, &path);
            states.push(DnnPageState { portal, url: format!("{alias}/{path}"), problem });
        }
        Ok(DnnSiteState { counts, extensions, pages: states, items: self.items(database) })
    }

    /// The site after a step to `expected`, against `before`; each check reported as it goes. What DNN and Windows
    /// logged since `since` is looked at too.
    #[allow(clippy::too_many_arguments)]
    pub fn validate(
        &self,
        site: &DnnSiteAddress,
        database: &DatabaseConnection,
        before: &DnnSiteState,
        expected: Version,
        since: DateTime<Utc>,
        accounts: &[DnnTestAccount],
        app_pool: &str,
        reporter: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<Vec<UpgradeFinding>, String> {
        use UpgradeFindingSeverity::*;
        let mut found = Vec::new();
        let mut add = |finding: UpgradeFinding, reporter: &mut dyn Reporter| {
            let text = match &finding.detail {
                None => format!("{}: {}", finding.area, finding.title),
                Some(detail) => format!("{}: {} - {detail}", finding.area, finding.title),
            };
            match finding.severity {
                Blocking => reporter.fail(&text),
                Warning => reporter.warn(&text),
                Compatible => reporter.success(&text),
                Unknown => reporter.info(&text),
            }
            found.push(finding);
        };

        // The database: at the version of the files and of the step, with nothing lost.
        let expected_name = expected.to_string_n(3);
        let files = dnnm_infrastructure::dnn_install::dnn_version(&site.directory);
        let finding = match self.sql.dnn_version(database) {
            Err(problem) => UpgradeFinding::new(Blocking, "Database", "The database can't be read")
                .with_detail(first_line(&problem)),
            Ok(db)
                if db.as_deref() == Some(expected_name.as_str())
                    && files.as_deref() == Some(expected_name.as_str()) =>
            {
                UpgradeFinding::new(Compatible, "Version", format!("Files and database at DNN {expected_name}"))
            }
            Ok(db) => UpgradeFinding::new(
                Blocking,
                "Version",
                format!(
                    "Expected DNN {expected_name} - the files are at {}, the database at {}",
                    files.as_deref().unwrap_or("?"),
                    db.as_deref().unwrap_or("?")
                ),
            )
            .with_detail("DNN's upgrade didn't run, or didn't finish."),
        };
        add(finding, reporter);
        cancel.check()?;

        let counts = self.sql.counts(database).ok().flatten();
        let (extensions, pages) = self.read_site(site, database);
        let items = self.items(database);
        let compared = compare(before, &items, &extensions);
        let lost_counts = match (&counts, &before.counts) {
            (Some(now), Some(was)) => now.lost(was),
            _ => Vec::new(),
        };
        match (&counts, &before.counts) {
            (Some(counts), Some(_)) if !items.is_empty() => {
                if !lost_counts.is_empty() || !compared.lost.is_empty() || !compared.fewer_permissions.is_empty() {
                    let detail = lost_counts
                        .iter()
                        .cloned()
                        .chain(compared.lost.iter().take(10).map(|i| format!("{} {}", i.kind, i.name)))
                        .chain(compared.fewer_permissions.iter().take(10).map(|p| format!("fewer permissions on {p}")))
                        .collect::<Vec<_>>()
                        .join(", ");
                    add(UpgradeFinding::new(Blocking, "Content", "Content was lost").with_detail(detail), reporter);
                } else {
                    add(UpgradeFinding::new(Compatible, "Content", format!("Nothing lost: {counts}")), reporter);
                }
            }
            _ => add(UpgradeFinding::new(Unknown, "Content", "The site's content couldn't be read"), reporter),
        }
        if !compared.removed_with_extension.is_empty() {
            let detail = compared
                .removed_with_extension
                .iter()
                .take(10)
                .map(|i| {
                    format!(
                        "{} {}{}",
                        i.kind,
                        i.name,
                        i.package.as_ref().map(|p| format!(" ({p})")).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            add(
                UpgradeFinding::new(
                    Warning,
                    "Content",
                    format!(
                        "DNN's upgrade removed {} page(s) / module(s) of its own (its Admin and Host pages, or with their extensions)",
                        compared.removed_with_extension.len()
                    ),
                )
                .with_detail(detail),
                reporter,
            );
        }
        if !compared.jobs_gone.is_empty() {
            let names = compared.jobs_gone.iter().map(|j| j.name.as_str()).collect::<Vec<_>>().join(", ");
            add(
                UpgradeFinding::new(
                    Warning,
                    "Scheduler",
                    format!("{} scheduled job(s) are gone", compared.jobs_gone.len()),
                )
                .with_detail(names),
                reporter,
            );
        }
        let gone = except(&before.extensions, &extensions);
        let added = except(&extensions, &before.extensions);
        if !gone.is_empty() {
            add(
                UpgradeFinding::new(
                    Warning,
                    "Extensions",
                    format!("{} extension(s) are gone: {}", gone.len(), gone.iter().take(15).cloned().collect::<Vec<_>>().join(", ")),
                )
                .with_detail("DNN's upgrade removes some of its own (DNN 10: the Telerik ones) - check none of them was one you use."),
                reporter,
            );
        }
        if !added.is_empty() {
            add(
                UpgradeFinding::new(
                    Compatible,
                    "Extensions",
                    format!(
                        "{} extension(s) added by the upgrade: {}",
                        added.len(),
                        added.iter().take(15).cloned().collect::<Vec<_>>().join(", ")
                    ),
                ),
                reporter,
            );
        }

        // The pages: every portal's home page and a sample of pages - a page that worked before must still work.
        let mut broken = 0;
        for (portal, alias, path) in &pages {
            cancel.check()?;
            let url = format!("{alias}/{path}");
            let Some(problem) = problem(alias, site.port, path) else { continue };
            let was_fine = before.pages.iter().find(|p| p.url == url).is_some_and(|p| p.problem.is_none());
            broken += 1;
            let (severity, detail) = if was_fine {
                (Blocking, format!("It worked before the step ({portal})."))
            } else {
                (Warning, format!("It didn't work before the step either ({portal})."))
            };
            add(UpgradeFinding::new(severity, "Pages", format!("{url}: {problem}")).with_detail(detail), reporter);
        }
        if broken == 0 {
            let mut portals: Vec<&str> = pages.iter().map(|p| p.0.as_str()).collect();
            portals.dedup();
            add(
                UpgradeFinding::new(
                    Compatible,
                    "Pages",
                    format!("{} page(s) of {} portal(s) answer without errors", pages.len(), portals.len()),
                ),
                reporter,
            );
        }

        // Signing in - the host also sees the Persona Bar.
        for account in accounts {
            cancel.check()?;
            let signed = Session::new(&site.alias, site.port, Duration::from_secs(120))
                .and_then(|mut s| s.sign_in(&account.user_name, &account.password))
                .unwrap_or_else(|e| crate::site_http::SignIn { signed_in: false, persona_bar: false, detail: e });
            let who = if account.is_host {
                format!("The host '{}'", account.user_name)
            } else {
                format!("'{}'", account.user_name)
            };
            // The Persona Bar came with DNN 9: DNN 8 and older show their control bar.
            let finding = if !signed.signed_in {
                UpgradeFinding::new(Blocking, "Sign-in", format!("{who} can't sign in")).with_detail(signed.detail)
            } else if account.is_host && expected.major >= 9 && !signed.persona_bar {
                UpgradeFinding::new(
                    Blocking,
                    "Persona Bar",
                    format!("{who} signs in, but the Persona Bar doesn't show"),
                )
            } else if account.is_host && expected.major >= 9 {
                UpgradeFinding::new(Compatible, "Sign-in", format!("{who} signs in and sees the Persona Bar"))
            } else {
                UpgradeFinding::new(Compatible, "Sign-in", format!("{who} signs in"))
            };
            add(finding, reporter);
        }

        // What DNN and Windows logged meanwhile.
        for line in dnn_log_errors(&site.directory, since).into_iter().take(8) {
            add(UpgradeFinding::new(Warning, "DNN log", line), reporter);
        }
        for entry in events(app_pool, &site.directory, since).into_iter().filter(|e| e.level <= 2).take(5) {
            add(
                UpgradeFinding::new(
                    Warning,
                    "Event log",
                    format!("{} {}: {}", entry.time.format("%H:%M:%S"), entry.source, first_line(&entry.message)),
                ),
                reporter,
            );
        }
        Ok(found)
    }

    /// Keeps what tells why a step failed in `folder`: DNN's logs and its database scripts' logs written since `since`,
    /// and the site's and its app pool's Windows events. What it found, as lines.
    pub fn collect_diagnostics(
        &self,
        site_directory: &Path,
        app_pool: &str,
        since: DateTime<Utc>,
        folder: &Path,
        cancel: &CancelToken,
    ) -> Result<Vec<String>, String> {
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let mut lines = Vec::new();
        // DNN's logs, and its database scripts' (an empty one ran cleanly).
        let sources = [
            ("Portals\\_default\\Logs", "", ".resources"),
            ("Providers\\DataProviders\\SqlDataProvider", "", ".log.resources"),
            ("App_Data", "", ""),
        ];
        let window = since - chrono::Duration::minutes(1);
        for (relative, prefix, suffix) in sources {
            let source = relative.split('\\').fold(site_directory.to_path_buf(), |p, part| p.join(part));
            // App_Data's "*.log*": a name with ".log" in it.
            let files = files_ending(&source, prefix, suffix)
                .into_iter()
                .filter(|f| !suffix.is_empty() || file_name(f).to_lowercase().contains(".log"));
            for file in files {
                if cancel.is_cancelled() {
                    return Err(CANCELLED.into());
                }
                if modified(&file).is_none_or(|m| m < window) {
                    continue;
                }
                let name = file_name(&file);
                let target = folder.join(format!("{}_{name}", relative.replace('\\', "_")));
                match std::fs::read(&file).and_then(|bytes| std::fs::write(&target, bytes)) {
                    Ok(()) => {
                        lines.push(format!("Kept {relative}\\{name}"));
                        // A database script's log holds only what went wrong - its first lines name the SQL and the error.
                        if relative.ends_with("SqlDataProvider") {
                            lines.extend(
                                read_shared(&target)
                                    .iter()
                                    .map(|l| l.trim().trim_matches('\u{feff}').to_string())
                                    .filter(|l| !l.is_empty())
                                    .take(3)
                                    .map(|l| format!("Database script {name} failed: {}", short(&l))),
                            );
                        }
                    }
                    Err(e) => lines.push(format!("Couldn't keep {relative}\\{name}: {e}")),
                }
            }
        }
        lines.extend(
            dnn_log_errors(site_directory, since)
                .into_iter()
                .filter(|l| !l.starts_with("Database script"))
                .take(10)
                .map(|l| format!("DNN log: {l}")),
        );
        let events = events(app_pool, site_directory, since);
        if !events.is_empty() {
            let text: String = events
                .iter()
                .map(|e| {
                    format!(
                        "{}  {}  {} ({})\r\n{}\r\n\r\n",
                        e.time.format("%Y-%m-%d %H:%M:%S"),
                        e.level_name,
                        e.source,
                        e.id,
                        e.message
                    )
                })
                .collect();
            std::fs::write(folder.join("windows-events.txt"), text).map_err(|e| e.to_string())?;
            lines.push(format!(
                "Kept {} Windows event(s) about the site and its app pool (windows-events.txt)",
                events.len()
            ));
            lines.extend(
                events
                    .iter()
                    .filter(|e| e.level <= 2)
                    .take(5)
                    .map(|e| format!("Event log: {}: {}", e.source, first_line(&e.message))),
            );
        }
        Ok(lines)
    }

    /// The site's extensions (by name) and, per portal, its home page and a sample of its pages - on this site's binding.
    fn read_site(&self, site: &DnnSiteAddress, database: &DatabaseConnection) -> (Vec<String>, Vec<PageToCheck>) {
        let found = self.sql.site_pages(database, PAGES_PER_PORTAL).unwrap_or_default();
        let pages = pages_on_binding(&found, &site.alias, site.port);
        (found.extensions, pages)
    }

    /// Every page, module and scheduled job, with its permissions; empty when the database can't be read.
    fn items(&self, database: &DatabaseConnection) -> Vec<DnnContentItem> {
        self.sql.content_items(database).unwrap_or_default()
    }
}

/// The pages to open: per portal with an alias on this site's binding, its home page and the sampled pages.
fn pages_on_binding(found: &SitePages, site_alias: &str, port: u16) -> Vec<PageToCheck> {
    // The site's own host name and port; a portal alias on another binding isn't reachable this way.
    let host = site_alias.split(':').next().unwrap_or_default();
    let mut portals: Vec<i32> = found.aliases.iter().map(|a| a.0).collect();
    portals.dedup();
    let mut pages = Vec::new();
    for portal in portals {
        let Some(alias) =
            found.aliases.iter().filter(|a| a.0 == portal).map(|a| &a.1).find(|a| on_binding(a, host, port))
        else {
            continue;
        };
        let name = format!("portal {portal}");
        let alias = alias.trim_end_matches('/').to_string();
        pages.push((name.clone(), alias.clone(), String::new()));
        for tab in found.tabs.get(&portal).into_iter().flatten() {
            pages.push((name.clone(), alias.clone(), format!("Default.aspx?TabId={tab}")));
        }
    }
    pages
}

fn except(list: &[String], other: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        if !other.iter().any(|o| o.eq_ignore_ascii_case(item)) && !out.iter().any(|o| o.eq_ignore_ascii_case(item)) {
            out.push(item.clone());
        }
    }
    out
}

fn file_name(file: &Path) -> String {
    file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whether a portal alias (`host[:port][/child]`) is served by this site's binding.
fn on_binding(alias: &str, host: &str, port: u16) -> bool {
    let host_port = alias.split('/').next().unwrap_or_default();
    let mut parts = host_port.split(':');
    let alias_host = parts.next().unwrap_or_default();
    let alias_port = parts.next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(80);
    alias_host.eq_ignore_ascii_case(host) && alias_port == port
}

/// What is wrong with a page - None when it answers (or asks to sign in) without a module error.
fn problem(alias: &str, port: u16, path: &str) -> Option<String> {
    let page = match Session::new(alias, port, Duration::from_secs(180)).and_then(|mut s| s.get(path)) {
        Ok(page) => page,
        Err(e) => return Some(first_line(&e)),
    };
    if page.at_installer() {
        return Some("it sends visitors to DNN's installer".into());
    }
    if page.at_error_page() {
        return Some("it shows DNN's error page".into());
    }
    if page.at_login() && page.status == 200 {
        return None;
    }
    if page.status != 200 {
        return Some(format!("HTTP {}", page.status));
    }
    let html = page.html.to_lowercase();
    MODULE_ERRORS.iter().find(|e| html.contains(&e.to_lowercase())).map(|e| format!("a module error (\"{e}\")"))
}

/// The [ERROR] and [FATAL] lines DNN logged since `since` - with the line after one when it carries the message (DNN
/// puts a SQL error's text there) - and its database scripts' problems.
pub fn dnn_log_errors(site_directory: &Path, since: DateTime<Utc>) -> Vec<String> {
    // A second's slack: DNN's log times are to the millisecond, the comparison to the second.
    let since_local = since.with_timezone(&Local) - chrono::Duration::seconds(1);
    let logs = site_directory.join("Portals").join("_default").join("Logs");
    let mut errors = Vec::new();
    for file in files_ending(&logs, "", ".log.resources") {
        let lines = read_shared(&file);
        for (i, line) in lines.iter().enumerate() {
            if !line.contains("[ERROR]") && !line.contains("[FATAL]") {
                continue;
            }
            if log_time(line).is_none_or(|at| at < since_local) {
                continue;
            }
            let next = lines.get(i + 1).map(|l| l.trim()).unwrap_or_default();
            let message = if !next.is_empty() && log_time(next).is_none() && !next.starts_with("at ") {
                format!("{line} → {next}")
            } else {
                line.clone()
            };
            errors.push(short(&message));
        }
    }
    errors.extend(
        script_problems(site_directory, since)
            .into_iter()
            .map(|(file, first)| format!("Database script {file}: {first}")),
    );
    errors
}

/// A Windows event about the site or its app pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEntry {
    pub time: DateTime<Local>,
    /// 1 critical, 2 error, 3 warning, 4 information.
    pub level: u8,
    pub level_name: String,
    pub source: String,
    pub id: u32,
    pub message: String,
}

/// Whether an event's message is about the site or its app pool.
type EventFilter<'a> = Box<dyn Fn(&str) -> bool + 'a>;

/// Windows' events since `since` about the site - ASP.NET's errors and warnings naming its folder, worker process
/// crashes - and its app pool (IIS's WAS: recycles, rapid-fail shutdowns). Empty when the event log can't be read.
fn events(app_pool: &str, site_directory: &Path, since: DateTime<Utc>) -> Vec<EventEntry> {
    let since = since.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let directory = site_directory.to_string_lossy().to_lowercase();
    let pool = app_pool.to_lowercase();
    let queries: [(&str, String, EventFilter<'_>); 2] = [
        (
            "Application",
            format!(
                "*[System[Provider[@Name='ASP.NET 4.0.30319.0' or @Name='.NET Runtime' or @Name='Application Error'] and TimeCreated[@SystemTime>='{since}']]]"
            ),
            Box::new(|m: &str| {
                let m = m.to_lowercase();
                m.contains(&directory) || m.contains("w3wp.exe") && m.contains(&pool)
            }),
        ),
        (
            "System",
            format!("*[System[Provider[@Name='Microsoft-Windows-WAS'] and TimeCreated[@SystemTime>='{since}']]]"),
            Box::new(|m: &str| !pool.is_empty() && m.to_lowercase().contains(&format!("'{pool}'"))),
        ),
    ];
    let mut entries = Vec::new();
    for (log, query, about) in &queries {
        let query_arg = format!("/q:{query}");
        let Ok(output) = dnnm_infrastructure::process::run(
            dnnm_infrastructure::process::system32("wevtutil.exe"),
            &["qe", log, &query_arg, "/f:RenderedXml", "/c:1000"],
            Duration::from_secs(60),
        ) else {
            continue;
        };
        if !output.success() {
            // The event log can't be read here - the checks go on without it.
            continue;
        }
        for entry in parse_events(&output.stdout) {
            if entries.len() >= 200 {
                break;
            }
            if about(&entry.message) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|e| e.time);
    entries
}

/// The events of `wevtutil qe … /f:RenderedXml`: one `<Event>` after another.
fn parse_events(xml: &str) -> Vec<EventEntry> {
    let wrapped = format!("<Events>{}</Events>", xml.trim_start_matches('\u{feff}'));
    let Ok(doc) = roxmltree::Document::parse(&wrapped) else { return Vec::new() };
    fn named<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        node.children().find(|n| n.tag_name().name() == name)
    }
    doc.root_element()
        .children()
        .filter(|n| n.tag_name().name() == "Event")
        .filter_map(|event| {
            let system = named(event, "System")?;
            let rendering = named(event, "RenderingInfo");
            let time = named(system, "TimeCreated")?.attribute("SystemTime")?;
            let time = DateTime::parse_from_rfc3339(time).ok()?.with_timezone(&Local);
            let level: u8 = named(system, "Level").and_then(|l| l.text()).and_then(|t| t.parse().ok()).unwrap_or(4);
            let text = |name: &str| rendering.and_then(|r| named(r, name)).and_then(|n| n.text()).map(str::to_string);
            Some(EventEntry {
                time,
                level,
                level_name: text("Level").unwrap_or_else(|| format!("Level {level}")),
                source: named(system, "Provider")?.attribute("Name").unwrap_or_default().to_string(),
                id: named(system, "EventID").and_then(|e| e.text()).and_then(|t| t.parse().ok()).unwrap_or(0),
                message: text("Message").unwrap_or_default(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn dnns_log_errors_are_read_in_dnn_9s_and_dnn_10s_formats_with_their_message() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("Portals").join("_default").join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        let since = Local.with_ymd_and_hms(2026, 10, 4, 23, 30, 0).unwrap().with_timezone(&Utc);
        std::fs::write(
            logs.join("2026.10.04.log.resources"),
            [
                "2026-10-04 23:20:00,123 [PC][Thread:9][ERROR] DotNetNuke.Old - before the step",
                "2026-10-04 23:37:54,504 [PC][Thread:9][ERROR] DotNetNuke.Nine - DNN 9's format",
                "2026-10-04 23:37:54.504+02:00 [Albadit][D:2][T:28][ERROR] DotNetNuke.Data.PetaPoco.PetaPocoHelper - [1] Error executing SQL: ;Exec dbo.UpdateDatabaseVersionAndName @0, @1, @2, @3",
                "Acme audit: version change refused",
                "2026-10-04 23:38:00.000+02:00 [Albadit][D:2][T:28][INFO] DotNetNuke.Fine - not an error",
                "2026-10-04 23:38:55.008+02:00 [Albadit][D:2][T:28][FATAL] DotNetNuke.Web - System.IO.IOException: …installBlocker.lock",
                "   at System.IO.__Error.WinIOError(Int32 errorCode, String maybeFullPath)",
            ]
            .join("\r\n"),
        )
        .unwrap();
        let errors = dnn_log_errors(dir.path(), since);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].contains("DNN 9's format"));
        assert!(errors[1].contains("UpdateDatabaseVersionAndName @0, @1, @2, @3 → Acme audit: version change refused"));
        assert!(!errors[2].contains("WinIOError"), "A stack trace line isn't the message.");
    }

    use chrono::TimeZone;

    #[test]
    fn only_portals_on_this_sites_binding_are_opened() {
        let found = SitePages {
            extensions: vec![],
            aliases: vec![
                (0, "shop.dnndev.me".into()),
                (0, "www.shop.example".into()),
                (1, "other.dnndev.me".into()),
                (2, "shop.dnndev.me/child/".into()),
            ],
            tabs: BTreeMap::from([(0, vec![21, 22]), (2, vec![70])]),
        };
        let pages = pages_on_binding(&found, "shop.dnndev.me", 80);
        let urls: Vec<_> = pages.iter().map(|(p, a, path)| format!("{p}: {a}/{path}")).collect();
        assert_eq!(
            urls,
            [
                "portal 0: shop.dnndev.me/",
                "portal 0: shop.dnndev.me/Default.aspx?TabId=21",
                "portal 0: shop.dnndev.me/Default.aspx?TabId=22",
                "portal 2: shop.dnndev.me/child/",
                "portal 2: shop.dnndev.me/child/Default.aspx?TabId=70",
            ]
        );
        assert!(on_binding("shop.dnndev.me:8081", "shop.dnndev.me", 8081));
        assert!(!on_binding("shop.dnndev.me", "shop.dnndev.me", 8081));
        assert_eq!(except(&["A".into(), "b".into()], &["a".into()]), ["b"]);
    }

    #[test]
    fn windows_events_are_read_from_wevtutils_xml() {
        let xml = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-WAS'/><EventID Qualifiers='32768'>5002</EventID><Level>1</Level><TimeCreated SystemTime='2026-10-04T21:38:55.0081234Z'/></System><RenderingInfo Culture='en-US'><Message>Application pool 'shop' is being automatically disabled.</Message><Level>Critical</Level></RenderingInfo></Event>
<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='ASP.NET 4.0.30319.0'/><EventID>1309</EventID><Level>3</Level><TimeCreated SystemTime='2026-10-04T21:38:50Z'/></System></Event>"#;
        let events = parse_events(xml);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].source, "Microsoft-Windows-WAS");
        assert_eq!((events[0].id, events[0].level, events[0].level_name.as_str()), (5002, 1, "Critical"));
        assert_eq!(events[1].level_name, "Level 3");
    }

    #[cfg(windows)]
    #[test]
    fn the_event_log_query_runs() {
        // Read-only: asks the System log for WAS events of a pool that doesn't exist.
        let since = crate::log_files::now() - chrono::Duration::minutes(10);
        assert!(events("dnnm-no-such-pool", Path::new(r"C:\no\such\site"), since).is_empty());
    }
}
