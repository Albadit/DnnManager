//! What the app knows of IIS: `IIisManager`, `IisSiteRuntime`, `IisBinding`, `IisServerState` and `IisDetails.cs`
//! (`IisSiteDetails`, `IisBindingDetails`, `IisCertificate`, `IisPoolDetails`, `IisPoolSettings`) of the C# app.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::project::SiteTraffic;

/// IIS's states of a site or an app pool, as IIS names them.
pub mod states {
    pub const STARTED: &str = "Started";
    pub const STARTING: &str = "Starting";
    pub const STOPPING: &str = "Stopping";
    pub const STOPPED: &str = "Stopped";
    pub const UNKNOWN: &str = "Unknown";
    /// Not an IIS site at all: a project's folder, listed where there is no IIS (macOS, Linux).
    pub const NOT_IN_IIS: &str = "NotInIis";

    /// Started - however it is capitalised.
    pub fn is_started(state: &str) -> bool {
        state.eq_ignore_ascii_case(STARTED)
    }
}

/// One binding of a site: protocol, address, port and host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisBinding {
    pub protocol: String,
    pub address: String,
    pub port: Option<u16>,
    pub host: String,
    pub has_certificate: bool,
}

impl IisBinding {
    /// Parses IIS's `bindingInformation` (`address:port:host`) for a protocol.
    pub fn parse(protocol: &str, information: &str, has_certificate: bool) -> Self {
        // The address can be an IPv6 literal in brackets, which holds colons: split from the right.
        let mut parts = information.rsplitn(3, ':');
        let host = parts.next().unwrap_or_default().to_string();
        let port = parts.next().and_then(|p| p.parse().ok());
        let address = parts.next().unwrap_or_default().to_string();
        Self { protocol: protocol.to_string(), address, port, host, has_certificate }
    }

    pub fn is_https(&self) -> bool {
        self.protocol.eq_ignore_ascii_case("https")
    }

    pub fn is_web(&self) -> bool {
        self.is_https() || self.protocol.eq_ignore_ascii_case("http")
    }

    /// The address a browser opens it at: its host name (localhost when it answers any), with the port when it isn't
    /// the protocol's own. None when it isn't a web binding.
    pub fn url(&self) -> Option<String> {
        let port = self.port.filter(|_| self.is_web())?;
        let scheme = if self.is_https() { "https" } else { "http" };
        let host = if self.host.is_empty() { "localhost" } else { &self.host };
        let default = if self.is_https() { 443 } else { 80 };
        Some(if port == default { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") })
    }
}

/// An IIS site as IIS has it configured and running - one row of the Projects table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisSiteRuntime {
    pub id: u64,
    /// "Started", "Starting", "Stopping", "Stopped" or "Unknown".
    pub state: String,
    pub app_pool: String,
    pub app_pool_state: Option<String>,
    /// The app pool's w3wp.exe processes - none until the site gets its first request.
    pub worker_process_ids: Vec<u32>,
    pub bindings: Vec<IisBinding>,
    /// The folder its root application serves, environment variables expanded.
    pub physical_path: PathBuf,
}

impl IisSiteRuntime {
    /// The ports of its bindings, lowest first.
    pub fn ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self.bindings.iter().filter_map(|b| b.port).collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    /// Where a browser opens it: an https binding with a certificate first, then http, then https without a
    /// certificate - each with a host name before one without. None for a site without web bindings.
    pub fn browse_url(&self) -> Option<String> {
        self.bindings
            .iter()
            .filter(|b| b.is_web())
            .min_by_key(|b| {
                let kind = if b.is_https() && b.has_certificate {
                    0
                } else if b.is_https() {
                    2
                } else {
                    1
                };
                kind + if b.host.is_empty() { 3 } else { 0 }
            })
            .and_then(IisBinding::url)
    }

    pub fn is_started(&self) -> bool {
        states::is_started(&self.state)
    }
}

/// The state of IIS itself (the W3SVC and WAS services) - the status bar's indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IisServerState {
    Running,
    Stopped,
    Starting,
    Stopping,
    NotInstalled,
    Unknown,
}

/// What IIS can be asked to do from the status bar's menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IisServerAction {
    Start,
    Stop,
    Restart,
}

/// Why IIS couldn't be read or changed - the message is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisError(pub String);

impl std::fmt::Display for IisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IisError {}

/// What IIS's configuration says about a site, in detail - its Details page's IIS tab: the site, every binding (with
/// its certificate), and its app pool with how it starts, recycles and fails.
#[derive(Debug, Clone, PartialEq)]
pub struct IisSiteDetails {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub physical_path: PathBuf,
    /// Where IIS keeps it: `MACHINE/WEBROOT/APPHOST/<site>` in applicationHost.config.
    pub config_path: String,
    pub server_auto_start: bool,
    /// The root application's preloadEnabled; None when IIS has no such setting.
    pub preload_enabled: Option<bool>,
    pub bindings: Vec<IisBindingDetails>,
    pub pool: Option<IisPoolDetails>,
}

/// One binding of a site, as its Details show it.
#[derive(Debug, Clone, PartialEq)]
pub struct IisBindingDetails {
    pub protocol: String,
    /// "*", "127.0.0.2", "[::1]".
    pub address: String,
    pub port: Option<u16>,
    /// Empty: any host name.
    pub host: String,
    pub binding_information: String,
    /// Server Name Indication: the certificate is chosen by host name (sslFlags 1).
    pub sni: bool,
    /// The thumbprint IIS has for an https binding (upper-case hex); None for none.
    pub certificate_hash: Option<String>,
    pub certificate_store: Option<String>,
    /// That certificate, from the store IIS names; None when there is no thumbprint or it isn't there.
    pub certificate: Option<IisCertificate>,
}

impl IisBindingDetails {
    /// A binding from IIS's `bindingInformation` (`address:port:host`), without its certificate yet.
    pub fn parse(protocol: &str, information: &str, sni: bool) -> Self {
        let parts: Vec<&str> = information.split(':').collect();
        // A web binding is "address:port:host"; other protocols (net.tcp "808:*") have their own form.
        let host = if parts.len() >= 3 { parts[parts.len() - 1] } else { "" };
        let host = if host == "*" { "" } else { host }.to_string();
        let port = if parts.len() >= 2 { parts[parts.len() - 2].parse().ok() } else { None };
        Self {
            protocol: protocol.to_string(),
            address: binding_address(information),
            port,
            host,
            binding_information: information.to_string(),
            sni,
            certificate_hash: None,
            certificate_store: None,
            certificate: None,
        }
    }
}

/// The address part of IIS's binding information ("127.0.0.2", "[::1]", "*").
pub fn binding_address(information: &str) -> String {
    // address:port:host - the address may be an IPv6 address in brackets, full of colons.
    let Some(last) = information.rfind(':').filter(|&i| i > 0) else { return "*".into() };
    match information[..last].rfind(':') {
        Some(port_colon) if port_colon > 0 => information[..port_colon].to_string(),
        _ => "*".into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisCertificate {
    pub subject: String,
    pub friendly_name: Option<String>,
    pub issuer: String,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
}

/// A site's app pool, in detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisPoolDetails {
    pub name: String,
    pub state: Option<String>,
    /// "v4.0", "v2.0" or [`NO_MANAGED_CODE`].
    pub runtime: String,
    /// "Integrated" or "Classic".
    pub pipeline: String,
    /// "ApplicationPoolIdentity", "NetworkService", "LocalService", "LocalSystem" or "SpecificUser".
    pub identity_type: String,
    /// The Windows account the worker process runs as - `IIS APPPOOL\<pool>` for ApplicationPoolIdentity.
    pub account: String,
    /// "OnDemand" or "AlwaysRunning".
    pub start_mode: String,
    pub idle_timeout: Duration,
    pub idle_timeout_action: Option<String>,
    /// Regular recycling; zero when it is off.
    pub recycle_interval: Duration,
    /// Recycled at these times of day.
    pub recycle_at: Vec<Duration>,
    /// Recycled above this much private memory; zero for no limit.
    pub private_memory_limit_kb: u64,
    pub virtual_memory_limit_kb: u64,
    pub queue_length: u64,
    pub enable_32bit: bool,
    pub rapid_fail_protection: bool,
    pub rapid_fail_max_crashes: u64,
    pub rapid_fail_interval: Duration,
    pub max_processes: u64,
    pub load_user_profile: bool,
    pub worker_process_ids: Vec<u32>,
}

/// The app pool settings a project's Details can change (Edit app pool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IisPoolSettings {
    /// "v4.0", or "" for No Managed Code.
    pub runtime: String,
    /// "Integrated" or "Classic".
    pub pipeline: String,
    pub enable_32bit: bool,
    /// "ApplicationPoolIdentity", "NetworkService", "LocalService" or "LocalSystem" - or "SpecificUser", which is kept
    /// as it is (its password isn't known here).
    pub identity: String,
    /// Zero: never shut down for being idle.
    pub idle_timeout: Duration,
    /// "OnDemand" or "AlwaysRunning".
    pub start_mode: String,
}

impl IisPoolSettings {
    pub fn from_pool(pool: &IisPoolDetails) -> Self {
        Self {
            runtime: if pool.runtime == NO_MANAGED_CODE { String::new() } else { pool.runtime.clone() },
            pipeline: pool.pipeline.clone(),
            enable_32bit: pool.enable_32bit,
            identity: pool.identity_type.clone(),
            idle_timeout: pool.idle_timeout,
            start_mode: pool.start_mode.clone(),
        }
    }
}

/// How the Details name an app pool without a .NET runtime.
pub const NO_MANAGED_CODE: &str = "No Managed Code";

/// What a method of [`IisManager`] says where its implementation can't do it (the C# interface's default).
pub const NOT_SUPPORTED: &str = "Not supported here.";

/// The Windows account an app pool runs as, by its identity type (`ProcessModelIdentityType`); a specific user is
/// `user_name`.
pub fn pool_account(pool: &str, identity_type: &str, user_name: &str) -> String {
    match identity_type {
        "SpecificUser" => user_name.to_string(),
        "NetworkService" => r"NT AUTHORITY\NETWORK SERVICE".into(),
        "LocalService" => r"NT AUTHORITY\LOCAL SERVICE".into(),
        "LocalSystem" => r"NT AUTHORITY\SYSTEM".into(),
        _ => format!(r"IIS APPPOOL\{pool}"),
    }
}

/// IIS, as the use cases and the monitor see it (`IIisManager`). Implemented in infrastructure.
pub trait IisManager: Send + Sync {
    fn is_available(&self) -> bool;
    fn server_state(&self) -> IisServerState;
    /// Every site in IIS, by name.
    fn site_runtimes(&self) -> Result<BTreeMap<String, IisSiteRuntime>, IisError>;
    /// Starts the site, and its app pool first when that is stopped.
    fn start_site(&self, site: &str) -> Result<(), IisError>;
    /// Stops the site, and its app pool when no other site uses it.
    fn stop_site(&self, site: &str) -> Result<(), IisError>;
    /// Recycles the site's app pool (a new worker process) and starts the site if it's stopped.
    fn restart_site(&self, site: &str) -> Result<(), IisError>;
    /// Recycles the site's app pool when it runs; a stopped pool stays stopped. Ok when there is nothing to recycle.
    fn recycle_app_pool(&self, site: &str) -> Result<(), IisError>;
    fn control_server(&self, action: IisServerAction) -> Result<(), IisError>;

    /// Creates the site with an app pool of its own (.NET v4.0, Integrated) named like it, bound to http `host` on
    /// `port` - removing a site of that name first. A pool of that name other sites use is joined as it is.
    fn create_site(&self, site: &str, physical_path: &Path, host: &str, port: u16) -> Result<(), IisError>;

    /// Stops the site and its pool, waits for the worker process to end (ended by force after 30 s), then removes
    /// both - the pool only when no other site uses it. Ok when there is no such site.
    fn remove_site(&self, site: &str) -> Result<(), IisError>;

    /// Stops the site as [`IisManager::stop_site`] does, and returns only once its worker process has ended - ended
    /// by force when it doesn't within `timeout`. Fails when the worker process still runs.
    fn stop_site_and_wait(&self, site: &str, _timeout: Duration) -> Result<(), IisError> {
        self.stop_site(site)
    }

    /// The folder IIS writes the site's request logs to (`…\W3SVC<id>`); None when there is no such site.
    fn log_directory(&self, site: &str) -> Option<PathBuf>;

    /// The Windows account the site runs as - what it signs in to SQL Server with under Windows authentication,
    /// e.g. `IIS APPPOOL\mysite`.
    fn app_pool_identity(&self, site: &str) -> String;

    /// Loads the user profile of the site's app pool identity and gives it that profile's environment - what a
    /// LocalDB database needs: LocalDB keeps every Windows account's instance in its profile.
    fn enable_user_profile(&self, site: &str) -> Result<(), IisError>;

    /// Every site's state, by name - empty when IIS can't be read (`GetSiteStates`).
    fn site_states(&self) -> BTreeMap<String, String> {
        self.site_runtimes()
            .map(|sites| sites.into_iter().map(|(name, s)| (name, s.state)).collect())
            .unwrap_or_default()
    }

    /// Every site's HTTP traffic since IIS started, from IIS's own counters; empty when they can't be read.
    fn site_traffic(&self) -> HashMap<String, SiteTraffic>;

    /// How many requests each site has served since IIS started; empty when the counters can't be read.
    fn requests_served(&self) -> HashMap<String, u64>;

    /// The site in detail - its bindings with their certificates, its app pool's settings; None when IIS has no
    /// such site or can't be read.
    fn site_details(&self, _site: &str) -> Option<IisSiteDetails> {
        None
    }

    /// Replaces the site's http bindings with `bindings` (host, port; an empty host answers any); its https and
    /// other bindings stay as they are.
    fn replace_http_bindings(&self, _site: &str, _bindings: &[(String, u16)]) -> Result<(), IisError> {
        Err(IisError(NOT_SUPPORTED.into()))
    }

    /// Changes the settings of the site's app pool - refused for a pool other sites use too.
    fn set_pool_settings(&self, _site: &str, _settings: &IisPoolSettings) -> Result<(), IisError> {
        Err(IisError(NOT_SUPPORTED.into()))
    }

    /// Renames the site - and its app pool, when it is named like the site and no other site uses it - and points
    /// its root at `physical_path`. The site and its pool are stopped first, and the worker process waited for; the
    /// caller starts it again.
    fn rename_site(&self, _site: &str, _new_name: &str, _physical_path: &Path) -> Result<(), IisError> {
        Err(IisError(NOT_SUPPORTED.into()))
    }

    /// Lets `identities` into the site's folder `path` and everything in it: its app pool may change it; IIS's
    /// shared groups (IIS_IUSRS, IUSR) may only read it.
    fn grant_permissions(&self, path: &Path, identities: &[String]) -> Result<(), IisError>;

    /// Deletes the Windows user profile made for the app pool's identity (`C:\Users\<pool>`), matched by the pool's
    /// SID. Ok when there is none, for a built-in pool, and while the pool is still in IIS (another site uses it).
    /// Call only after the pool's worker process has ended.
    fn remove_app_pool_profile(&self, pool: &str) -> Result<(), IisError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(bindings: Vec<IisBinding>) -> IisSiteRuntime {
        IisSiteRuntime {
            id: 1,
            state: "Started".into(),
            app_pool: "p".into(),
            app_pool_state: None,
            worker_process_ids: vec![],
            bindings,
            physical_path: PathBuf::new(),
        }
    }

    #[test]
    fn parses_binding_information() {
        let b = IisBinding::parse("http", "*:8080:dnn10.dnndev.me", false);
        assert_eq!((b.address.as_str(), b.port, b.host.as_str()), ("*", Some(8080), "dnn10.dnndev.me"));
        let v6 = IisBinding::parse("https", "[::1]:443:", true);
        assert_eq!((v6.address.as_str(), v6.port, v6.host.as_str()), ("[::1]", Some(443), ""));
    }

    #[test]
    fn url_drops_the_default_port_and_uses_localhost_for_any_host() {
        assert_eq!(IisBinding::parse("http", "*:80:a.me", false).url().as_deref(), Some("http://a.me"));
        assert_eq!(IisBinding::parse("HTTPS", "*:8443:", false).url().as_deref(), Some("https://localhost:8443"));
        assert_eq!(IisBinding::parse("net.tcp", "808:*", false).url(), None);
    }

    #[test]
    fn browse_url_prefers_certified_https_then_http_then_named_hosts() {
        let s = site(vec![
            IisBinding::parse("https", "*:443:", false),
            IisBinding::parse("http", "*:80:", false),
            IisBinding::parse("http", "*:80:a.me", false),
        ]);
        assert_eq!(s.browse_url().as_deref(), Some("http://a.me"));
        let s =
            site(vec![IisBinding::parse("http", "*:80:a.me", false), IisBinding::parse("https", "*:443:a.me", true)]);
        assert_eq!(s.browse_url().as_deref(), Some("https://a.me"));
        assert_eq!(site(vec![]).browse_url(), None);
    }

    #[test]
    fn binding_details_split_address_port_and_host() {
        let b = IisBindingDetails::parse("http", "127.0.0.2:80:shop", false);
        assert_eq!((b.address.as_str(), b.port, b.host.as_str()), ("127.0.0.2", Some(80), "shop"));
        let v6 = IisBindingDetails::parse("https", "[::1]:443:", true);
        assert_eq!((v6.address.as_str(), v6.port, v6.host.as_str(), v6.sni), ("[::1]", Some(443), "", true));
        let tcp = IisBindingDetails::parse("net.tcp", "808:*", false);
        assert_eq!((tcp.address.as_str(), tcp.port, tcp.host.as_str()), ("*", Some(808), ""));
        assert_eq!(binding_address("*:8080:a.me"), "*");
        assert_eq!(binding_address("nonsense"), "*");
    }

    #[test]
    fn pool_accounts_and_settings_from_a_pool() {
        assert_eq!(pool_account("shop", "ApplicationPoolIdentity", ""), r"IIS APPPOOL\shop");
        assert_eq!(pool_account("shop", "NetworkService", ""), r"NT AUTHORITY\NETWORK SERVICE");
        assert_eq!(pool_account("shop", "SpecificUser", r"PC\dev"), r"PC\dev");
        let pool = IisPoolDetails {
            name: "shop".into(),
            state: None,
            runtime: NO_MANAGED_CODE.into(),
            pipeline: "Integrated".into(),
            identity_type: "LocalSystem".into(),
            account: String::new(),
            start_mode: "AlwaysRunning".into(),
            idle_timeout: Duration::ZERO,
            idle_timeout_action: None,
            recycle_interval: Duration::ZERO,
            recycle_at: vec![],
            private_memory_limit_kb: 0,
            virtual_memory_limit_kb: 0,
            queue_length: 1000,
            enable_32bit: true,
            rapid_fail_protection: true,
            rapid_fail_max_crashes: 5,
            rapid_fail_interval: Duration::from_secs(300),
            max_processes: 1,
            load_user_profile: true,
            worker_process_ids: vec![],
        };
        let settings = IisPoolSettings::from_pool(&pool);
        assert_eq!(settings.runtime, "");
        assert_eq!((settings.identity.as_str(), settings.start_mode.as_str()), ("LocalSystem", "AlwaysRunning"));
        assert!(settings.enable_32bit);
    }

    #[test]
    fn ports_are_sorted_and_distinct() {
        let s = site(vec![
            IisBinding::parse("https", "*:443:a", false),
            IisBinding::parse("http", "*:80:a", false),
            IisBinding::parse("http", "*:80:b", false),
        ]);
        assert_eq!(s.ports(), vec![80, 443]);
    }
}
