//! Settings → Projects → Keep warm: `KeepWarmSettings` of `UserSettings.cs`, read from the `settings` rows
//! (`projects.keepWarm.pingMinutes`, `.pingPath`, `.warmUpPath`) through [`Settings`].

use dnnm_application::settings::Settings;
use url::Url;

pub const MIN_PING_MINUTES: u32 = 1;
pub const MAX_PING_MINUTES: u32 = 60;
/// The intervals the settings and a site's overview offer, in minutes.
pub const PING_INTERVALS: [u32; 8] = [1, 2, 5, 10, 15, 20, 30, 60];
pub const DEFAULT_WARM_UP_PATH: &str = "/";
pub const DEFAULT_PING_PATH: &str = "/KeepAlive.aspx";
pub const DEFAULT_PING_MINUTES: u32 = 5;

pub const PING_MINUTES_KEY: &str = "projects.keepWarm.pingMinutes";
pub const PING_PATH_KEY: &str = "projects.keepWarm.pingPath";
pub const WARM_UP_PATH_KEY: &str = "projects.keepWarm.warmUpPath";

/// How every site is kept warm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepWarmSettings {
    /// Minutes between two requests at most - fewer when the site's app pool shuts down sooner than twice that, and
    /// none while the site is in use anyway.
    pub ping_minutes: u32,
    /// The page requested when the site has no worker process: DNN starts up and the page is compiled.
    pub warm_up_path: String,
    /// The page requested to keep a running site warm - DNN's `KeepAlive.aspx` reads nothing from the database.
    pub ping_path: String,
}

impl Default for KeepWarmSettings {
    fn default() -> Self {
        Self {
            ping_minutes: DEFAULT_PING_MINUTES,
            warm_up_path: DEFAULT_WARM_UP_PATH.into(),
            ping_path: DEFAULT_PING_PATH.into(),
        }
    }
}

impl KeepWarmSettings {
    /// The settings as the rows have them, paths normalized (`ToAppOptions`). An interval out of range or not a
    /// number reads as the default - the WPF app's validation refuses to save one.
    pub fn from_settings(settings: &Settings) -> Self {
        let minutes = settings
            .get(PING_MINUTES_KEY)
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|m| minutes_problem(*m).is_none())
            .unwrap_or(DEFAULT_PING_MINUTES);
        Self {
            ping_minutes: minutes,
            warm_up_path: normalize_path(settings.get(WARM_UP_PATH_KEY).unwrap_or(DEFAULT_WARM_UP_PATH)),
            ping_path: normalize_path(settings.get(PING_PATH_KEY).unwrap_or(DEFAULT_PING_PATH)),
        }
    }
}

/// `path` as it is requested: trimmed, starting with a `/`. Empty stays empty (no value).
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') { trimmed.to_string() } else { format!("/{trimmed}") }
}

/// What is wrong with `path` as a page to request on the site; None when it can be requested. Never a page of DNN's
/// installer: requesting it can install or upgrade DNN.
pub fn path_problem(path: &str) -> Option<&'static str> {
    let p = normalize_path(path);
    if p.is_empty() {
        return Some("is required, e.g. /KeepAlive.aspx.");
    }
    if p.encode_utf16().count() > 2000 {
        return Some("can have at most 2000 characters.");
    }
    if p.starts_with("//") || p.contains("://") {
        return Some("must be a page of the site, e.g. /KeepAlive.aspx - not an address.");
    }
    if p.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("can't contain spaces.");
    }
    if p.contains('\\') {
        return Some("must use / between folders.");
    }
    if p.contains('#') {
        return Some("can't contain #.");
    }
    if p.split('?').next().unwrap_or_default().split('/').any(|segment| segment == "." || segment == "..") {
        return Some("can't contain . or .. folders.");
    }
    if is_installer_path(&p) || p.to_ascii_lowercase().contains("mode=") {
        return Some("can't be a page of DNN's installer - requesting it can install or upgrade DNN.");
    }
    None
}

/// Whether `path_and_query` is - or may be - a page of DNN's installer: a folder named Install anywhere in it, however
/// it is written (dot segments, %-escapes, trailing dots or spaces Windows ignores), or something that can't be read as
/// a path at all. Such a page is never requested.
pub fn is_installer_path(path_and_query: &str) -> bool {
    // Read as the path of one fixed address - not as a reference, where //Install/… would be a host named Install.
    let separator = if path_and_query.starts_with('/') { "" } else { "/" };
    let Ok(url) = Url::parse(&format!("http://localhost{separator}{path_and_query}")) else {
        return true;
    };
    let mut path = url.path().to_string();
    // Escapes inside escapes too (%2549 is %49 is I) - IIS decodes until nothing changes.
    for _ in 0..4 {
        let unescaped = unescape(&path);
        if unescaped == path {
            break;
        }
        path = unescaped;
    }
    path.replace('\\', "/")
        .split('/')
        .any(|segment| segment.trim_end_matches(['.', ' ']).eq_ignore_ascii_case("install"))
}

/// What is wrong with `minutes` as an interval; None when it is allowed.
pub fn minutes_problem(minutes: u32) -> Option<String> {
    if (MIN_PING_MINUTES..=MAX_PING_MINUTES).contains(&minutes) {
        None
    } else {
        Some(format!("must be between {MIN_PING_MINUTES} and {MAX_PING_MINUTES} minutes."))
    }
}

/// `Uri.UnescapeDataString`: every valid %XX decoded, anything else left as it is.
pub(crate) fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_pages_that_are_no_pages_of_the_site_or_of_the_installer() {
        for good in ["/KeepAlive.aspx", "KeepAlive.aspx", "/", "/Home?keepwarm=1", " /Default.aspx "] {
            assert_eq!(path_problem(good), None, "{good}");
        }
        for bad in [
            "",
            "  ",
            "http://shop.dnndev.me/",
            "//shop.dnndev.me/",
            "/a page",
            r"\KeepAlive.aspx",
            "/#top",
            "/Install/Install.aspx",
            "/install",
            "/INSTALL/UpgradeWizard.aspx",
            "/Default.aspx?mode=upgrade",
            "/./Install/Install.aspx",
            "/x/../Install/Install.aspx",
            "../Install/Install.aspx",
            "/%49nstall/Install.aspx",
            "/%2E/Install/Install.aspx",
            "/%2549nstall/Install.aspx",
            "/Install./Install.aspx",
            "/dnn/Install/Install.aspx",
        ] {
            assert!(path_problem(bad).is_some(), "{bad}");
        }
        assert_eq!(path_problem("/Installation"), None, "Only a folder named Install is the installer's.");
        assert_eq!(normalize_path(" KeepAlive.aspx "), "/KeepAlive.aspx");
        assert!(is_installer_path("//Install/InstallWizard.aspx"));
        assert!(minutes_problem(0).is_some() && minutes_problem(61).is_some() && minutes_problem(5).is_none());
    }

    #[test]
    fn reads_the_settings_rows() {
        let defaults = KeepWarmSettings::from_settings(&Settings::default());
        assert_eq!(defaults, KeepWarmSettings::default());
        let rows = [(PING_MINUTES_KEY, "30"), (PING_PATH_KEY, "KeepAlive.aspx"), (WARM_UP_PATH_KEY, " Home ")];
        let settings = Settings::from_rows(rows.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let read = KeepWarmSettings::from_settings(&settings);
        assert_eq!(
            (read.ping_minutes, read.ping_path.as_str(), read.warm_up_path.as_str()),
            (30, "/KeepAlive.aspx", "/Home")
        );
        let bad = Settings::from_rows([(PING_MINUTES_KEY.to_string(), "0".to_string())]);
        assert_eq!(KeepWarmSettings::from_settings(&bad).ping_minutes, 5);
    }
}
