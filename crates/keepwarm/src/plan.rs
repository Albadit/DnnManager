//! Where a keep-warm request goes and how often - `KeepWarmPlan.cs`.

use std::net::IpAddr;
use std::time::Duration;

use dnnm_application::iis::IisSiteRuntime;

use crate::rules;
use crate::settings::{DEFAULT_PING_PATH, DEFAULT_WARM_UP_PATH, KeepWarmSettings, normalize_path, path_problem};

/// Where a keep-warm request to a site goes: this machine, on the binding a browser opens the site at, with that
/// binding's host name in the Host header - IIS routes it to the site whatever DNS says, and DNN finds its portal alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSiteTarget {
    /// "http" or "https".
    pub scheme: String,
    /// The address connected to: 127.0.0.1 for a binding on every address, else the binding's own.
    pub connect_host: String,
    pub port: u16,
    /// The host name, with the port when it isn't the scheme's own - e.g. `mysite.dnndev.me`.
    pub host_header: String,
}

impl LocalSiteTarget {
    pub fn new(scheme: &str, connect_host: &str, port: u16, host_header: &str) -> Self {
        Self { scheme: scheme.into(), connect_host: connect_host.into(), port, host_header: host_header.into() }
    }

    /// The address a request for `path_and_query` is sent to.
    pub fn uri_for(&self, path_and_query: &str) -> String {
        format!("{}://{}:{}{}", self.scheme, self.connect_host, self.port, path_and_query)
    }

    /// The page as people know it: `http://mysite.dnndev.me/KeepAlive.aspx`.
    pub fn display_url(&self, path_and_query: &str) -> String {
        format!("{}://{}{}", self.scheme, self.host_header, path_and_query)
    }

    /// The IP address [`Self::connect_host`] names (without IPv6 brackets).
    pub fn connect_ip(&self) -> Option<IpAddr> {
        self.connect_host.trim_matches(['[', ']']).parse().ok()
    }

    /// The target for `site`: its bindings in the order `browse_url` takes them, leaving out those that have no port
    /// or a wildcard host name (`*.example.com` is no Host header). None when none is left.
    pub fn for_site(site: &IisSiteRuntime) -> Option<Self> {
        let binding =
            site.bindings.iter().filter(|b| b.is_web() && b.port.is_some() && !b.host.contains('*')).min_by_key(
                |b| {
                    let kind = if b.is_https() && b.has_certificate {
                        0
                    } else if b.is_https() {
                        2
                    } else {
                        1
                    };
                    kind + if b.host.is_empty() { 3 } else { 0 }
                },
            )?;
        let scheme = if binding.is_https() { "https" } else { "http" };
        let port = binding.port?;
        let host = if binding.host.is_empty() { "localhost" } else { &binding.host };
        let default = if binding.is_https() { 443 } else { 80 };
        let host_header = if port == default { host.to_string() } else { format!("{host}:{port}") };
        Some(Self::new(scheme, &connect_address(&binding.address), port, &host_header))
    }
}

/// The address to connect to for a binding on `address`: loopback for all of them.
fn connect_address(address: &str) -> String {
    let a = address.trim();
    if a.is_empty() || a == "*" {
        return "127.0.0.1".into();
    }
    // IIS writes an IPv6 address in brackets already; a bare one gets them here.
    match a.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
        Ok(IpAddr::V4(ip)) => ip.to_string(),
        Err(_) => "127.0.0.1".into(),
    }
}

/// How a site is kept warm: the settings, and its IIS app pool's idle time-out - what the service requests, and what
/// the site's overview shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepWarmPlan {
    pub ping_minutes: u32,
    /// The time between two requests at most.
    pub interval: Duration,
    /// The app pool's idle time-out; None when it isn't known, zero when IIS never shuts the worker process down.
    pub idle_timeout: Option<Duration>,
    pub warm_up_path: String,
    pub ping_path: String,
    /// Where requests go; None when the site has no binding to request.
    pub target: Option<LocalSiteTarget>,
}

impl KeepWarmPlan {
    /// `idle_timeout` is the app pool's (`IisSiteRuntime.IdleTimeout` of the C# app - the port's runtime doesn't read
    /// it, so the caller passes what it knows).
    pub fn new(settings: &KeepWarmSettings, site: &IisSiteRuntime, idle_timeout: Option<Duration>) -> Self {
        Self {
            ping_minutes: settings.ping_minutes,
            interval: rules::interval(settings.ping_minutes, idle_timeout),
            idle_timeout,
            warm_up_path: fallback(&settings.warm_up_path, DEFAULT_WARM_UP_PATH),
            ping_path: fallback(&settings.ping_path, DEFAULT_PING_PATH),
            target: LocalSiteTarget::for_site(site),
        }
    }
}

// A page that can't be requested (never one of the installer's) - the built-in one instead.
fn fallback(path: &str, built_in: &str) -> String {
    if path_problem(path).is_none() { normalize_path(path) } else { built_in.to_string() }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dnnm_application::iis::IisBinding;
    use std::path::PathBuf;

    pub(crate) fn site(bindings: Vec<IisBinding>) -> IisSiteRuntime {
        IisSiteRuntime {
            id: 1,
            state: "Started".into(),
            app_pool: "shop".into(),
            app_pool_state: Some("Started".into()),
            worker_process_ids: vec![],
            bindings,
            physical_path: PathBuf::from(r"C:\DNN\shop"),
        }
    }

    fn http(host: &str, port: u16, address: &str) -> IisBinding {
        IisBinding {
            protocol: "http".into(),
            address: address.into(),
            port: Some(port),
            host: host.into(),
            has_certificate: false,
        }
    }

    #[test]
    fn target_is_this_machine_with_the_bindings_host_name() {
        let target = LocalSiteTarget::for_site(&site(vec![http("shop.dnndev.me", 80, "*")])).unwrap();
        assert_eq!(target, LocalSiteTarget::new("http", "127.0.0.1", 80, "shop.dnndev.me"));
        assert_eq!(target.uri_for("/KeepAlive.aspx"), "http://127.0.0.1:80/KeepAlive.aspx");
        assert_eq!(target.display_url("/KeepAlive.aspx"), "http://shop.dnndev.me/KeepAlive.aspx");
        assert_eq!(LocalSiteTarget::for_site(&site(vec![http("", 8080, "*")])).unwrap().host_header, "localhost:8080");
        assert_eq!(LocalSiteTarget::for_site(&site(vec![http("a", 80, "10.0.0.5")])).unwrap().connect_host, "10.0.0.5");
        assert_eq!(LocalSiteTarget::for_site(&site(vec![http("a", 80, "[::1]")])).unwrap().connect_host, "[::1]");
        assert_eq!(LocalSiteTarget::for_site(&site(vec![http("a", 80, "::1")])).unwrap().connect_host, "[::1]");
    }

    #[test]
    fn target_prefers_what_a_browser_opens_and_skips_what_cant_be_requested() {
        let https = IisBinding::parse("https", "*:443:shop.dnndev.me", true);
        assert_eq!(
            LocalSiteTarget::for_site(&site(vec![http("shop.dnndev.me", 80, "*"), https])),
            Some(LocalSiteTarget::new("https", "127.0.0.1", 443, "shop.dnndev.me"))
        );
        let wild = site(vec![http("*.dnndev.me", 80, "*"), http("shop.dnndev.me", 80, "*")]);
        assert_eq!(LocalSiteTarget::for_site(&wild).unwrap().host_header, "shop.dnndev.me");
        assert_eq!(LocalSiteTarget::for_site(&site(vec![IisBinding::parse("net.tcp", "808:*", false)])), None);
    }

    #[test]
    fn plan_takes_the_settings_within_the_idle_timeout() {
        let s = site(vec![http("shop.dnndev.me", 80, "*")]);
        let twenty = Some(Duration::from_secs(20 * 60));
        let plain = KeepWarmPlan::new(&KeepWarmSettings::default(), &s, twenty);
        assert_eq!(
            (plain.interval, plain.ping_path.as_str(), plain.warm_up_path.as_str()),
            (Duration::from_secs(300), "/KeepAlive.aspx", "/")
        );
        let settings = KeepWarmSettings {
            ping_minutes: 30,
            warm_up_path: "/Home".into(),
            ping_path: "/Install/Install.aspx".into(),
        };
        let changed = KeepWarmPlan::new(&settings, &s, twenty);
        assert_eq!(changed.interval, Duration::from_secs(8 * 60));
        assert_eq!(changed.warm_up_path, "/Home");
        assert_eq!(changed.ping_path, "/KeepAlive.aspx", "A page of the installer is never requested.");
    }
}
