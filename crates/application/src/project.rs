//! What the app knows about one project (`ProjectState` of the C# monitor) and how the Projects table words it
//! (`ProjectRow`). The texts are the WPF app's, word for word.

use std::path::PathBuf;
use std::time::Duration;

use crate::iis::{IisSiteRuntime, states};

/// Shown where there is nothing to show.
pub const NONE: &str = "-";
/// Not known yet: the database state and the folder size arrive after the row is shown.
pub const NOT_YET: &str = "…";

/// The worker processes' resource use - `ProcessGroupStats`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessStats {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: u64,
    pub memory_percent: f64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    /// How long ago the oldest worker process started.
    pub started_ago: Option<Duration>,
}

/// A site's HTTP traffic since IIS started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteTraffic {
    pub bytes_received: u64,
    pub bytes_sent: u64,
}

/// One IIS site, and what is in the folder it serves.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectState {
    pub name: String,
    pub directory: PathBuf,
    /// Where a browser opens it; empty when it has no web binding.
    pub site_url: String,
    /// The folder is inside the projects folder: a DNN Manager project.
    pub in_projects_folder: bool,
    /// From `bin\DotNetNuke.dll`; None when the folder holds no DNN.
    pub dnn_version: Option<String>,
    /// From web.config's SiteSqlServer connection.
    pub database_name: Option<String>,
    /// Why the database isn't known.
    pub database_problem: Option<String>,
    pub database_server: Option<String>,
    pub database_is_file: bool,
    /// Whether the SQL Server answers; None until asked.
    pub sql_reachable: Option<bool>,
    pub database_exists: bool,
    pub sql_problem: Option<String>,
    pub size_bytes: Option<u64>,
    pub site: IisSiteRuntime,
    pub stats: Option<ProcessStats>,
    pub traffic: Option<SiteTraffic>,
}

impl ProjectState {
    /// A project as IIS alone tells it - before its folder has been read.
    pub fn from_site(name: String, site: IisSiteRuntime) -> Self {
        Self {
            site_url: site.browse_url().unwrap_or_default(),
            directory: site.physical_path.clone(),
            name,
            in_projects_folder: false,
            dnn_version: None,
            database_name: None,
            database_problem: None,
            database_server: None,
            database_is_file: false,
            sql_reachable: None,
            database_exists: false,
            sql_problem: None,
            size_bytes: None,
            site,
            stats: None,
            traffic: None,
        }
    }
}

/// The state a row shows, from the site's and its app pool's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteRunState {
    Running,
    Starting,
    Stopping,
    Stopped,
    Unknown,
}

/// The row texts of the Projects table.
pub mod view {
    use super::*;

    pub fn run_state(site: &IisSiteRuntime) -> SiteRunState {
        // An app pool on its way down is the site on its way down: until the worker process has ended it can't start.
        if site.app_pool_state.as_deref() == Some(states::STOPPING) {
            return SiteRunState::Stopping;
        }
        match site.state.as_str() {
            states::STARTED => match site.app_pool_state.as_deref() {
                Some(states::STOPPED) => SiteRunState::Stopped,
                Some(states::STARTING) => SiteRunState::Starting,
                _ => SiteRunState::Running,
            },
            states::STOPPED => SiteRunState::Stopped,
            states::STARTING => SiteRunState::Starting,
            states::STOPPING => SiteRunState::Stopping,
            _ => SiteRunState::Unknown,
        }
    }

    /// A started site whose app pool is stopped answers 503: for the user that is a stopped site.
    fn pool_stopped(site: &IisSiteRuntime) -> bool {
        site.state == states::STARTED && site.app_pool_state.as_deref() == Some(states::STOPPED)
    }

    pub fn state_text(site: &IisSiteRuntime) -> &'static str {
        match run_state(site) {
            SiteRunState::Running => "Running",
            SiteRunState::Stopped if pool_stopped(site) => "App pool stopped",
            SiteRunState::Stopped => "Stopped",
            SiteRunState::Starting => "Starting…",
            SiteRunState::Stopping => "Stopping…",
            SiteRunState::Unknown => "State unknown",
        }
    }

    pub fn url(p: &ProjectState) -> String {
        if p.site_url.is_empty() { NONE.into() } else { p.site_url.clone() }
    }

    pub fn dnn(p: &ProjectState) -> String {
        p.dnn_version.clone().unwrap_or_else(|| "(none)".into())
    }

    pub fn database(p: &ProjectState) -> String {
        match (&p.database_name, &p.database_problem) {
            (Some(name), _) => name.clone(),
            (None, None) => NONE.into(),
            (None, Some(_)) => "not set".into(),
        }
    }

    pub fn database_tip(p: &ProjectState) -> Option<String> {
        p.database_problem
            .clone()
            .or_else(|| p.database_name.as_ref().map(|_| "From the site's web.config (SiteSqlServer)".into()))
    }

    /// "Live", "Offline", "(none)", "File", "…" - or "-" for a site without a database.
    pub fn sql(p: &ProjectState) -> &'static str {
        if p.database_name.is_none() {
            return NONE;
        }
        if p.database_is_file {
            return "File";
        }
        match p.sql_reachable {
            None => NOT_YET,
            Some(false) => "Offline",
            Some(true) if p.database_exists => "Live",
            Some(true) => "(none)",
        }
    }

    pub fn sql_tip(p: &ProjectState) -> Option<String> {
        let server = p.database_server.as_deref().unwrap_or_default();
        let Some(name) = &p.database_name else { return p.database_problem.clone() };
        if p.database_is_file {
            return Some("A LocalDB file the site attaches in its own instance - not asked while the site runs".into());
        }
        Some(match p.sql_reachable {
            None => format!("Asking {server}…"),
            Some(false) => format!(
                "{server} doesn't answer the site's web.config connection: {}",
                p.sql_problem.as_deref().unwrap_or_default()
            ),
            Some(true) if p.database_exists => {
                format!("[{name}] on {server} answers the site's web.config connection")
            }
            Some(true) => p.sql_problem.clone().unwrap_or_else(|| format!("[{name}] isn't on {server}")),
        })
    }

    pub fn size(p: &ProjectState) -> String {
        match p.size_bytes {
            Some(bytes) => format!("{} MB", group_thousands(bytes as f64 / 1024.0 / 1024.0, 1)),
            None => NOT_YET.into(),
        }
    }

    pub fn ports(site: &IisSiteRuntime) -> String {
        join_or_none(site.ports())
    }

    pub fn pids(site: &IisSiteRuntime) -> String {
        join_or_none(site.worker_process_ids.iter())
    }

    pub fn app_pool(site: &IisSiteRuntime) -> String {
        match (&site.app_pool, &site.app_pool_state) {
            (pool, _) if pool.is_empty() => NONE.into(),
            (pool, Some(state)) => format!("{pool} - {state}"),
            (pool, None) => pool.clone(),
        }
    }

    pub fn cpu(p: &ProjectState) -> String {
        p.stats.as_ref().and_then(|s| s.cpu_percent).map_or(NONE.into(), |c| format!("{}%", decimal(c, 2)))
    }

    pub fn memory(p: &ProjectState) -> String {
        p.stats.as_ref().map_or(NONE.into(), |s| byte_size(s.memory_bytes as f64))
    }

    pub fn memory_percent(p: &ProjectState) -> String {
        p.stats.as_ref().map_or(NONE.into(), |s| format!("{}%", decimal(s.memory_percent, 2)))
    }

    pub fn disk(p: &ProjectState) -> String {
        p.stats.as_ref().map_or(NONE.into(), |s| {
            format!("{} / {}", byte_size(s.read_bytes as f64), byte_size(s.write_bytes as f64))
        })
    }

    pub fn network(p: &ProjectState) -> String {
        p.traffic.map_or(NONE.into(), |t| {
            format!("{} / {}", byte_size(t.bytes_received as f64), byte_size(t.bytes_sent as f64))
        })
    }

    pub fn last_started(p: &ProjectState) -> String {
        p.stats.as_ref().and_then(|s| s.started_ago).map_or(NONE.into(), ago)
    }

    /// Everything the search box looks in.
    pub fn search_key(p: &ProjectState) -> String {
        let hosts: Vec<&str> = p.site.bindings.iter().map(|b| b.host.as_str()).filter(|h| !h.is_empty()).collect();
        [
            p.name.clone(),
            url(p),
            hosts.join(", "),
            p.directory.display().to_string(),
            dnn(p),
            database(p),
            sql(p).into(),
            state_text(&p.site).into(),
            p.site.id.to_string(),
            ports(&p.site),
            pids(&p.site),
        ]
        .join("\n")
        .to_lowercase()
    }

    /// "just now", "5 minutes ago", "1 hour ago", "3 days ago".
    pub fn ago(span: Duration) -> String {
        let minutes = span.as_secs() / 60;
        let (n, unit) = match minutes {
            0 => return "just now".into(),
            m if m < 60 => (m, "minute"),
            m if m < 60 * 24 => (m / 60, "hour"),
            m => (m / 60 / 24, "day"),
        };
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    }

    /// `ByteSize.Format`: B, KB, MB with one decimal, GB with two.
    pub fn byte_size(bytes: f64) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;
        if bytes >= GB {
            format!("{} GB", group_thousands(bytes / GB, 2))
        } else if bytes >= MB {
            format!("{} MB", group_thousands(bytes / MB, 1))
        } else if bytes >= KB {
            format!("{} KB", group_thousands(bytes / KB, 0))
        } else {
            format!("{} B", group_thousands(bytes, 0))
        }
    }

    /// The user's number separators - decimal and thousands - as Windows has them (.NET formats in the user's
    /// culture: "0,39%" and "1.234,5 MB" on a Dutch Windows). Set once at start; "." and "," until then.
    static NUMBER_FORMAT: std::sync::OnceLock<(char, char)> = std::sync::OnceLock::new();

    pub fn set_number_format(decimal: char, group: char) {
        let _ = NUMBER_FORMAT.set((decimal, group));
    }

    fn separators() -> (char, char) {
        NUMBER_FORMAT.get().copied().unwrap_or(('.', ','))
    }

    /// .NET's "N" format: thousands grouped, a fixed number of decimals, in the user's separators.
    pub fn group_thousands(value: f64, decimals: usize) -> String {
        format_number(value, decimals, true)
    }

    /// .NET's "0.00" format: a fixed number of decimals, no grouping, in the user's decimal separator.
    pub fn decimal(value: f64, decimals: usize) -> String {
        format_number(value, decimals, false)
    }

    fn format_number(value: f64, decimals: usize, grouping: bool) -> String {
        let (decimal_separator, group_separator) = separators();
        let text = format!("{value:.decimals$}");
        let (whole, fraction) = text.split_once('.').map_or((text.as_str(), None), |(w, f)| (w, Some(f)));
        let (sign, digits) = whole.strip_prefix('-').map_or(("", whole), |d| ("-", d));
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            if grouping && i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push(group_separator);
            }
            grouped.push(c);
        }
        match fraction {
            Some(f) => format!("{sign}{grouped}{decimal_separator}{f}"),
            None => format!("{sign}{grouped}"),
        }
    }

    fn join_or_none<T: ToString>(items: impl IntoIterator<Item = T>) -> String {
        let text = items.into_iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
        if text.is_empty() { NONE.into() } else { text }
    }
}

#[cfg(test)]
mod tests {
    use super::view::*;
    use super::*;
    use crate::iis::IisBinding;

    fn site(state: &str, pool: Option<&str>) -> IisSiteRuntime {
        IisSiteRuntime {
            id: 3,
            state: state.into(),
            app_pool: "dnn10".into(),
            app_pool_state: pool.map(Into::into),
            worker_process_ids: vec![],
            bindings: vec![IisBinding::parse("http", "*:80:dnn10.dnndev.me", false)],
            physical_path: PathBuf::from(r"C:\DNN\dnn10"),
        }
    }

    #[test]
    fn state_follows_the_app_pool() {
        assert_eq!(state_text(&site("Started", Some("Started"))), "Running");
        assert_eq!(state_text(&site("Started", Some("Stopped"))), "App pool stopped");
        assert_eq!(state_text(&site("Started", Some("Starting"))), "Starting…");
        assert_eq!(state_text(&site("Stopped", Some("Stopping"))), "Stopping…");
        assert_eq!(state_text(&site("Stopped", None)), "Stopped");
        assert_eq!(state_text(&site("Unknown", None)), "State unknown");
    }

    #[test]
    fn sql_column_words_each_case() {
        let mut p = ProjectState::from_site("dnn10".into(), site("Started", None));
        assert_eq!(sql(&p), "-");
        p.database_name = Some("dnn10".into());
        assert_eq!(sql(&p), "…");
        p.sql_reachable = Some(false);
        assert_eq!(sql(&p), "Offline");
        p.sql_reachable = Some(true);
        assert_eq!(sql(&p), "(none)");
        p.database_exists = true;
        assert_eq!(sql(&p), "Live");
        p.database_is_file = true;
        assert_eq!(sql(&p), "File");
    }

    #[test]
    fn database_column() {
        let mut p = ProjectState::from_site("x".into(), site("Started", None));
        assert_eq!(database(&p), "-");
        p.database_problem = Some("No web.config".into());
        assert_eq!(database(&p), "not set");
        assert_eq!(database_tip(&p).as_deref(), Some("No web.config"));
    }

    #[test]
    fn formats_sizes_like_dotnet() {
        assert_eq!(byte_size(512.0), "512 B");
        assert_eq!(byte_size(2048.0), "2 KB");
        assert_eq!(byte_size(1.5 * 1024.0 * 1024.0), "1.5 MB");
        assert_eq!(byte_size(3.0 * 1024.0 * 1024.0 * 1024.0), "3.00 GB");
        assert_eq!(group_thousands(1234567.891, 1), "1,234,567.9");
        assert_eq!(group_thousands(999.0, 0), "999");
        assert_eq!(decimal(1234.5, 2), "1234.50");
    }

    #[test]
    fn ago_words() {
        assert_eq!(ago(Duration::from_secs(30)), "just now");
        assert_eq!(ago(Duration::from_secs(60)), "1 minute ago");
        assert_eq!(ago(Duration::from_secs(5 * 3600)), "5 hours ago");
        assert_eq!(ago(Duration::from_secs(2 * 86400)), "2 days ago");
    }

    #[test]
    fn search_key_has_name_url_and_state() {
        let p = ProjectState::from_site("dnn10".into(), site("Started", None));
        let key = search_key(&p);
        assert!(key.contains("dnn10.dnndev.me") && key.contains("running") && key.contains(r"c:\dnn\dnn10"));
    }
}
