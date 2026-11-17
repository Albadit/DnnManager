//! The rules keep warm goes by, apart from the service that follows them - `KeepWarmRules.cs`.
//!
//! Why a local DNN site is slow after a while without visits: IIS shuts an app pool's worker process down after its
//! idle time-out (20 minutes by default), and the next request starts a new one - ASP.NET and DNN start up again, which
//! takes 3 to 10 seconds instead of some 30 ms. So keep warm requests a site before its time-out runs out, and warms it
//! up again as soon as its worker process is gone - with the site's own keep-alive page, which does no database work.

use std::time::Duration;

use url::Url;

use crate::model::{KeepWarmOutcomeKind, KeepWarmRequestKind};
use crate::settings::{is_installer_path, unescape};

/// An answer that takes longer than this came from a site that was cold: DNN had to start.
pub const SLOW_AFTER: Duration = Duration::from_secs(2);
/// The same while DNN Manager is in efficiency mode: a quick answer can be measured as a slow one.
pub const SLOW_AFTER_WHILE_THROTTLED: Duration = Duration::from_secs(5);
/// Never more often than this, whatever the app pool's idle time-out.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);
/// The interval is at most this part of the idle time-out.
pub const IDLE_TIMEOUT_SHARE: f64 = 0.4;
pub const PING_TIMEOUT: Duration = Duration::from_secs(30);
/// A cold DNN can take a while to start, longer after a rebuild.
pub const WARM_UP_TIMEOUT: Duration = Duration::from_secs(120);
/// After DNN Manager starts, sites without a worker process are warmed up only after this - Windows may still be starting.
pub const START_UP_DELAY: Duration = Duration::from_secs(60);
/// A worker process that ends this soon after a warm-up started it, failed (a site that keeps crashing).
pub const DIED_SOON_AFTER: Duration = Duration::from_secs(5 * 60);
/// Failures in a row after which a site isn't tried again until something changes.
pub const FAILURES_BEFORE_STOP: u32 = 4;
/// At most this many requests at once, for all sites together.
pub const MAX_IN_FLIGHT: usize = 2;
/// Redirects to the same site a warm-up follows.
pub const MAX_REDIRECTS: usize = 3;

const BACKOFFS: [Duration; 3] = [Duration::from_secs(60), Duration::from_secs(120), Duration::from_secs(300)];

/// The time between two requests: `ping_minutes`, or less when the app pool's idle time-out needs it. `idle_timeout`:
/// None when unknown, zero when IIS never shuts the worker process down for being idle.
pub fn interval(ping_minutes: u32, idle_timeout: Option<Duration>) -> Duration {
    let setting = Duration::from_secs(60 * u64::from(ping_minutes.max(1)));
    let Some(idle) = idle_timeout.filter(|i| !i.is_zero()) else {
        return setting;
    };
    let safe = idle.mul_f64(IDLE_TIMEOUT_SHARE);
    safe.min(setting).max(MIN_INTERVAL)
}

/// Whether a request may be skipped because the site served others since the last look: the next one then comes two
/// intervals after the last look at most - which has to stay within the idle time-out.
pub fn may_skip_when_in_use(site: &str, interval: Duration, idle_timeout: Option<Duration>) -> bool {
    match idle_timeout {
        None => true,
        Some(idle) if idle.is_zero() => true,
        Some(idle) => 2 * (interval + jitter(site, interval)) < idle,
    }
}

/// How long to wait after the `failures`-th failure in a row: 1, 2, then 5 minutes.
pub fn backoff(failures: u32) -> Duration {
    BACKOFFS[(failures.max(1) as usize - 1).min(BACKOFFS.len() - 1)]
}

/// A small offset of its own for each site (up to a tenth of the interval, at most 15 s), so sites switched on together
/// don't send at the same moment every time. The same for a site every time.
pub fn jitter(site: &str, interval: Duration) -> Duration {
    let range = (interval.as_secs_f64() * 1000.0 / 10.0).min(15_000.0);
    if range < 1.0 {
        return Duration::ZERO;
    }
    let mut hash: u32 = 2_166_136_261;
    for c in site.to_uppercase().encode_utf16() {
        hash = (hash ^ u32::from(c)).wrapping_mul(16_777_619);
    }
    Duration::from_millis(u64::from(hash % range as u32))
}

/// What a response means, and - for a warm-up - which redirect to follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub kind: KeepWarmOutcomeKind,
    pub reason: Option<String>,
    pub note: Option<String>,
    /// The same site's page to request next; None to stop here.
    pub follow_path: Option<String>,
}

impl Verdict {
    fn of(kind: KeepWarmOutcomeKind) -> Self {
        Self { kind, reason: None, note: None, follow_path: None }
    }

    fn installer_waiting() -> Self {
        Self { note: Some(INSTALLER_WAITING.into()), ..Self::of(KeepWarmOutcomeKind::Ok) }
    }
}

const INSTALLER_WAITING: &str = "DNN's installer is waiting - open the site to finish it";

/// What a response to a `kind` request for `requested` means. Any answer from the site keeps it warm - its worker
/// process and DNN run - except an error. DNN's installer is never followed into: requesting it can install or upgrade
/// DNN. `requested` is the address as people know it (scheme, host header, path); `location` the Location header.
pub fn classify(
    kind: KeepWarmRequestKind,
    requested: &Url,
    status: u16,
    location: Option<&str>,
    body_start: &str,
) -> Verdict {
    match status {
        200..=299 => {
            if body_start.to_ascii_lowercase().contains("installwizard") {
                Verdict::installer_waiting()
            } else {
                Verdict::of(KeepWarmOutcomeKind::Ok)
            }
        }
        300..=399 => {
            let Some(target) = location.and_then(|l| requested.join(l.trim()).ok()) else {
                return Verdict::of(KeepWarmOutcomeKind::Ok);
            };
            let query = target.query().unwrap_or_default();
            if target.path().to_ascii_lowercase().contains("errorpage") || query.to_ascii_lowercase().contains("error=")
            {
                return Verdict { reason: Some(error_page_reason(query)), ..Verdict::of(KeepWarmOutcomeKind::Failed) };
            }
            // Another site (or https): the site answered, nothing to follow.
            let same_site = target.scheme().eq_ignore_ascii_case(requested.scheme())
                && target.host_str().unwrap_or_default().eq_ignore_ascii_case(requested.host_str().unwrap_or_default())
                && target.port_or_known_default() == requested.port_or_known_default();
            if !same_site {
                return Verdict::of(KeepWarmOutcomeKind::Ok);
            }
            let path_and_query = path_and_query(&target);
            if is_installer_path(&path_and_query) {
                return Verdict::installer_waiting();
            }
            // Only a warm-up follows: the page it lands on is compiled too.
            if kind == KeepWarmRequestKind::WarmUp {
                Verdict { follow_path: Some(path_and_query), ..Verdict::of(KeepWarmOutcomeKind::Ok) }
            } else {
                Verdict::of(KeepWarmOutcomeKind::Ok)
            }
        }
        404 if kind == KeepWarmRequestKind::Ping => Verdict::of(KeepWarmOutcomeKind::PingPathMissing),
        // Not found, forbidden, sign in first… - the site answered, so it runs.
        400..=499 => Verdict::of(KeepWarmOutcomeKind::Ok),
        502..=504 => Verdict {
            reason: Some(if status == 503 {
                "the site answers 503 Service Unavailable - its app pool may be stopping or failing".into()
            } else {
                format!("the site answers HTTP {status}")
            }),
            ..Verdict::of(KeepWarmOutcomeKind::NotAnswering)
        },
        _ => Verdict {
            reason: Some(format!(r"the site answers HTTP {status} - see its logs in Portals\_default\Logs")),
            ..Verdict::of(KeepWarmOutcomeKind::Failed)
        },
    }
}

/// `Uri.PathAndQuery`.
pub(crate) fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    }
}

// DNN's error page says what went wrong in its query string (?status=500&error=…) - e.g. that the database didn't answer.
fn error_page_reason(query: &str) -> String {
    let error = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| k.eq_ignore_ascii_case("error"))
        .map(|(_, v)| v);
    let mut text = error.map(|e| unescape(&e.replace('+', " ")).trim().to_string()).unwrap_or_default();
    if text.encode_utf16().count() > 120 {
        text = format!("{}…", text.chars().take(120).collect::<String>());
    }
    if text.is_empty() {
        r"DNN shows its error page - see its logs in Portals\_default\Logs".into()
    } else {
        format!("DNN shows its error page: {text}")
    }
}

/// "12 ms", "3.4 s" - how long an answer took.
pub fn duration(elapsed: Duration) -> String {
    if elapsed < Duration::from_secs(1) {
        format!("{} ms", ((elapsed.as_secs_f64() * 1000.0).round_ties_even() as u64).max(1))
    } else {
        format!("{:.1} s", elapsed.as_secs_f64())
    }
}

/// "5 minutes", "1 minute", "45 seconds", "1.5 minutes".
pub fn span(span: Duration) -> String {
    if span < Duration::from_secs(60) {
        return format!("{} seconds", span.as_secs_f64().round_ties_even() as u64);
    }
    let minutes = span.as_secs_f64() / 60.0;
    if (minutes - minutes.round_ties_even()).abs() < 0.01 {
        let whole = minutes.round_ties_even() as u64;
        return if whole == 1 { "1 minute".into() } else { format!("{whole} minutes") };
    }
    let text = format!("{minutes:.1}");
    format!("{} minutes", text.strip_suffix(".0").unwrap_or(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeepWarmOutcomeKind as K;
    use KeepWarmRequestKind::{Ping, WarmUp};

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn interval_stays_well_within_the_idle_time_out() {
        assert_eq!(interval(5, Some(20 * MIN)), 5 * MIN, "IIS's default leaves the setting as it is.");
        assert_eq!(interval(30, Some(20 * MIN)), 8 * MIN, "Never more than 40% of the time-out.");
        assert_eq!(interval(5, Some(5 * MIN)), 2 * MIN);
        assert_eq!(interval(5, Some(Duration::from_secs(20))), MIN_INTERVAL, "Not more often than every 30 seconds.");
        assert_eq!(interval(5, Some(Duration::ZERO)), 5 * MIN, "An app pool that never idles out still gets them.");
        assert_eq!(interval(5, None), 5 * MIN, "Unknown: the setting.");
    }

    #[test]
    fn in_use_skip_only_when_the_next_request_still_comes_in_time() {
        assert!(may_skip_when_in_use("shop", 5 * MIN, Some(20 * MIN)));
        assert!(may_skip_when_in_use("shop", 5 * MIN, None));
        assert!(may_skip_when_in_use("shop", 5 * MIN, Some(Duration::ZERO)));
        let short = interval(5, Some(MIN));
        assert!(!may_skip_when_in_use("shop", short, Some(MIN)), "Two 30-second intervals don't fit in a minute.");
    }

    #[test]
    fn backoff_waits_longer_after_each_failure() {
        assert_eq!([backoff(1), backoff(2), backoff(3), backoff(10)], [MIN, 2 * MIN, 5 * MIN, 5 * MIN]);
    }

    #[test]
    fn jitter_is_the_sites_own_and_small() {
        let i = 5 * MIN;
        assert_eq!(jitter("shop", i), jitter("SHOP", i), "The same for a site every time.");
        for site in ["shop", "blog", "intranet", "a", "dnn10"] {
            assert!(jitter(site, i) < Duration::from_secs(15), "{site}");
        }
        assert!(jitter("shop", Duration::from_secs(30)) < Duration::from_secs(3), "At most a tenth of the interval.");
        // The same number as the C# FNV-1a over the upper-cased UTF-16 name.
        let mut hash: u32 = 2_166_136_261;
        for c in "SHOP".bytes() {
            hash = (hash ^ u32::from(c)).wrapping_mul(16_777_619);
        }
        assert_eq!(jitter("shop", i), Duration::from_millis(u64::from(hash % 15_000)));
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(duration(Duration::from_micros(12_400)), "12 ms");
        assert_eq!(duration(Duration::from_micros(100)), "1 ms");
        assert_eq!(duration(Duration::from_millis(3420)), "3.4 s");
        assert_eq!(span(MIN), "1 minute");
        assert_eq!(span(5 * MIN), "5 minutes");
        assert_eq!(span(Duration::from_secs(30)), "30 seconds");
        assert_eq!(span(Duration::from_secs(144)), "2.4 minutes");
        assert_eq!(span(Duration::from_secs(150)), "2.5 minutes");
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn any_answer_keeps_warm_but_not_an_error() {
        let u = url("http://shop.dnndev.me/KeepAlive.aspx");
        assert_eq!(classify(Ping, &u, 200, None, "").kind, K::Ok);
        assert_eq!(classify(Ping, &u, 401, None, "").kind, K::Ok, "Sign in first - but it runs.");
        assert_eq!(classify(Ping, &u, 403, None, "").kind, K::Ok);
        assert_eq!(classify(Ping, &u, 404, None, "").kind, K::PingPathMissing);
        assert_eq!(
            classify(WarmUp, &u, 404, None, "").kind,
            K::Ok,
            "A warm-up page that isn't there still had DNN answer."
        );
        assert_eq!(classify(Ping, &u, 500, None, "").kind, K::Failed);
        let down = classify(Ping, &u, 503, None, "");
        assert_eq!(down.kind, K::NotAnswering);
        assert_eq!(
            down.reason.as_deref(),
            Some("the site answers 503 Service Unavailable - its app pool may be stopping or failing")
        );
        let wizard = classify(WarmUp, &u, 200, None, "<form action=\"InstallWizard.aspx\">");
        assert_eq!((wizard.kind, wizard.note.as_deref()), (K::Ok, Some(INSTALLER_WAITING)));
    }

    #[test]
    fn follows_only_the_same_site_and_never_into_the_installer() {
        let home = url("http://shop.dnndev.me/");
        assert_eq!(classify(WarmUp, &home, 302, Some("/Home"), "").follow_path.as_deref(), Some("/Home"));
        assert_eq!(classify(Ping, &home, 302, Some("/Home"), "").follow_path, None, "A ping doesn't follow.");
        assert_eq!(classify(WarmUp, &home, 301, Some("https://shop.dnndev.me/"), "").follow_path, None);
        assert_eq!(classify(WarmUp, &home, 302, Some("http://www.example.com/"), "").follow_path, None);
        let installer = classify(WarmUp, &home, 302, Some("/Install/Install.aspx?mode=upgrade"), "");
        assert_eq!((installer.kind, installer.follow_path), (K::Ok, None));
        assert_eq!(classify(WarmUp, &home, 302, Some("/dnn/Install/Install.aspx?mode=upgrade"), "").follow_path, None);
        assert_eq!(
            classify(WarmUp, &home, 302, Some("/Installation"), "").follow_path.as_deref(),
            Some("/Installation")
        );
        for location in [
            "/\\Install/InstallWizard.aspx",
            "/.//Install/InstallWizard.aspx",
            "http://shop.dnndev.me//Install/InstallWizard.aspx",
        ] {
            assert_eq!(classify(WarmUp, &home, 302, Some(location), "").follow_path, None, "{location}");
        }
        let error = classify(
            WarmUp,
            &home,
            302,
            Some("/ErrorPage.aspx?status=500&error=Connection+To+The+Database+Failed"),
            "",
        );
        assert_eq!(error.kind, K::Failed);
        assert_eq!(error.reason.as_deref(), Some("DNN shows its error page: Connection To The Database Failed"));
        let bare = classify(WarmUp, &home, 302, Some("/ErrorPage.aspx"), "");
        assert_eq!(bare.reason.as_deref(), Some(r"DNN shows its error page - see its logs in Portals\_default\Logs"));
    }
}
