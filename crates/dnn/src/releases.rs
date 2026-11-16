//! The DNN releases of a GitHub repository (`src/DnnManager.Infrastructure/Github/GitHubDnnReleaseService.cs`,
//! `IDnnReleaseService`). Each list GitHub returns is saved in `dnnmanager.db` (`dnn_releases`) in place of the one
//! before, so New project still offers the versions - and sets up the kept packages - when GitHub can't be reached.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use dnnm_application::operation::{CANCELLED, CancelToken};
use dnnm_domain::{DnnRelease, DnnReleaseList};
use regex::Regex;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use crate::version::Version;

/// Pages of 100 releases read at most - 2 000 releases, far more than any DNN source has.
const MAX_RELEASE_PAGES: usize = 20;

/// HttpClient's own default: what a request to GitHub may take.
const API_TIMEOUT: Duration = Duration::from_secs(100);

/// A GitHub answer: its status, its `Link` header and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAnswer {
    pub status: u16,
    pub link: Option<String>,
    pub body: String,
}

/// How the release service reaches GitHub - a stand-in in the tests. Err: GitHub couldn't be reached.
pub trait ReleaseHttp: Send + Sync {
    fn get(&self, url: &str, cancel: &CancelToken) -> Result<HttpAnswer, String>;
}

/// GitHub over HTTPS (`HttpClient` with DNN Manager's User-Agent).
#[derive(Debug, Clone)]
pub struct GitHubHttp {
    agent: ureq::Agent,
}

impl Default for GitHubHttp {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(API_TIMEOUT))
            .http_status_as_error(false)
            .user_agent("DnnManager-NET")
            .build()
            .new_agent();
        Self { agent }
    }
}

impl ReleaseHttp for GitHubHttp {
    fn get(&self, url: &str, cancel: &CancelToken) -> Result<HttpAnswer, String> {
        cancel.check()?;
        let mut response = self.agent.get(url).call().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let link = response.headers().get_all("link").iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>();
        let link = (!link.is_empty()).then(|| link.join(","));
        let body =
            response.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().map_err(|e| e.to_string())?;
        cancel.check()?;
        Ok(HttpAnswer { status, link, body })
    }
}

/// `IDnnReleaseService` over GitHub's releases API, with the list saved for offline use.
pub struct ReleaseService<H: ReleaseHttp = GitHubHttp> {
    http: H,
    store: ReleaseStore,
    apis: Vec<String>,
}

impl<H: ReleaseHttp> ReleaseService<H> {
    /// `apis`: the release sources of Settings (`GitHubReleaseApis`).
    pub fn new(http: H, store: ReleaseStore, apis: Vec<String>) -> Self {
        Self { http, store, apis }
    }

    pub fn known_release_apis(&self) -> &[String] {
        &self.apis
    }

    /// The release of that version, or the latest release - offline, from the releases saved at the last lookup.
    pub fn get_release(
        &self,
        api_url: &str,
        version: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<DnnRelease, String> {
        let version = version.filter(|v| !v.trim().is_empty());
        let Some(version) = version else {
            let list = self.list_releases(api_url, cancel)?;
            // The latest means the latest release - a pre-release only when the repository has nothing else.
            return list
                .releases
                .iter()
                .find(|r| !r.prerelease)
                .or(list.releases.first())
                .cloned()
                .ok_or_else(|| "No suitable DNN release found.".into());
        };
        match self.lookup(api_url, version, cancel) {
            Ok(found) => found,
            Err(_) if cancel.is_cancelled() => Err(CANCELLED.into()),
            Err(error) => {
                // Offline: the release as saved at the last lookup.
                let wanted = version.trim_start_matches('v');
                let known = self.store.load(api_url).and_then(|saved| {
                    saved
                        .releases
                        .into_iter()
                        .find(|r| r.version.eq_ignore_ascii_case(wanted) || r.tag_name.eq_ignore_ascii_case(version))
                });
                known.ok_or_else(|| format!("GitHub can't be reached: {error}"))
            }
        }
    }

    /// Outer Err: GitHub wasn't reached; inner: its answer.
    fn lookup(&self, api_url: &str, version: &str, cancel: &CancelToken) -> Result<Result<DnnRelease, String>, String> {
        // DNN tags its releases "v10.0.1"; a tag without the "v" works too.
        for tag in [format!("v{}", version.trim_start_matches('v')), version.to_string()] {
            let answer = self.http.get(&format!("{api_url}/tags/{tag}"), cancel)?;
            if answer.status == 404 {
                continue;
            }
            if !(200..300).contains(&answer.status) {
                return Err(format!("Response status code does not indicate success: {}.", answer.status));
            }
            let json: Value = serde_json::from_str(&answer.body).map_err(|e| e.to_string())?;
            if json.is_null() {
                break;
            }
            let release = GhRelease::from(&json);
            return Ok(match release.install_asset() {
                Some(asset) => Ok(release.to_release(asset)),
                None => Err(format!("Release {} has no DNN Install ZIP asset.", release.tag_name)),
            });
        }
        Ok(Err(format!("Release {version} not found.")))
    }

    /// The releases of `api_url` that have a DNN install package, pre-releases included (no drafts), highest version
    /// first - a release before a pre-release of the same version. When GitHub can't be reached, the releases saved at
    /// the last lookup (`saved_at`), if any.
    pub fn list_releases(&self, api_url: &str, cancel: &CancelToken) -> Result<DnnReleaseList, String> {
        match self.fetch_all(api_url, cancel) {
            Ok(releases) if releases.is_empty() => Err("Empty release list.".into()),
            Ok(releases) => {
                let usable = usable(releases);
                self.store.save(api_url, &usable);
                Ok(DnnReleaseList { releases: usable, saved_at: None })
            }
            Err(_) if cancel.is_cancelled() => Err(CANCELLED.into()),
            Err(error) => self.store.load(api_url).ok_or(error),
        }
    }

    fn fetch_all(&self, api_url: &str, cancel: &CancelToken) -> Result<Vec<GhRelease>, String> {
        // GitHub returns 30 releases a page by default, at most 100 - and DNN has more than 100 with its release
        // candidates: every page is read (GitHub's Link header names the next), so the oldest versions are there too.
        let mut releases = Vec::new();
        let mut url = Some(format!("{api_url}{}per_page=100", if api_url.contains('?') { "&" } else { "?" }));
        let mut page = 0;
        while let Some(current) = url.filter(|_| page < MAX_RELEASE_PAGES) {
            let answer = self.http.get(&current, cancel)?;
            if !(200..300).contains(&answer.status) {
                return Err(format!("Response status code does not indicate success: {}.", answer.status));
            }
            let json: Value = serde_json::from_str(&answer.body).map_err(|e| e.to_string())?;
            releases.extend(json.as_array().into_iter().flatten().map(GhRelease::from));
            url = answer.link.as_deref().and_then(next_page);
            page += 1;
        }
        Ok(releases)
    }
}

/// The releases with an install package, highest version first: GitHub lists by release date, and a 9.x patch can come
/// out after a 10.x release. A release before a pre-release of the same version ("10.1.0" before "10.1.0-rc1"); tags
/// that aren't a version number keep GitHub's order, after the numbered ones.
fn usable(releases: Vec<GhRelease>) -> Vec<DnnRelease> {
    let mut list: Vec<(DnnRelease, usize, Option<Version>)> = releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| r.install_asset().map(|asset| r.to_release(asset)))
        .enumerate()
        .map(|(index, release)| {
            let number = version_number(&release.version);
            (release, index, number)
        })
        .collect();
    list.sort_by(|a, b| {
        b.2.is_some()
            .cmp(&a.2.is_some())
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.prerelease.cmp(&b.0.prerelease))
            .then(a.1.cmp(&b.1))
    });
    list.into_iter().map(|(release, _, _)| release).collect()
}

/// The number part of a version - `10.1.0` of `10.1.0-rc1` - or None when it doesn't start with one.
fn version_number(version: &str) -> Option<Version> {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+){1,3}").expect("valid"));
    NUMBER.find(version).and_then(|m| Version::parse(m.as_str()))
}

/// The next page's address from GitHub's `Link` header (`<…>; rel="next"`); None on the last page
/// (`GitHubDnnReleaseService.NextPage`).
pub fn next_page(link: &str) -> Option<String> {
    for part in link.split(',') {
        let pieces: Vec<&str> = part.split(';').collect();
        if pieces.len() < 2 || !pieces[1..].iter().any(|p| p.trim().eq_ignore_ascii_case("rel=\"next\"")) {
            continue;
        }
        let target = pieces[0].trim();
        if target.len() >= 2 && target.starts_with('<') && target.ends_with('>') {
            return Some(target[1..target.len() - 1].to_string());
        }
    }
    None
}

/// A DNN release source: https, or http only to this computer (a mirror on localhost) - over plain http anyone on the
/// way could change the package that becomes the site (`SettingRules.IsReleaseSource`).
pub fn is_release_source(source: &str) -> bool {
    let Ok(url) = url::Url::parse(source) else { return false };
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

struct GhAsset {
    name: String,
    browser_download_url: String,
    // "sha256:677af2…" - GitHub's digest of the file; none on assets uploaded before GitHub kept them.
    digest: Option<String>,
}

impl GhAsset {
    fn sha256(&self) -> Option<String> {
        let d = self.digest.as_deref()?;
        (d.len() == 7 + 64 && d.is_char_boundary(7) && d[..7].eq_ignore_ascii_case("sha256:"))
            .then(|| d[7..].to_ascii_lowercase())
    }
}

struct GhRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<GhAsset>,
}

impl From<&Value> for GhRelease {
    fn from(json: &Value) -> Self {
        let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        Self {
            tag_name: text(json, "tag_name"),
            prerelease: json.get("prerelease").and_then(Value::as_bool).unwrap_or(false),
            draft: json.get("draft").and_then(Value::as_bool).unwrap_or(false),
            assets: json
                .get("assets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|a| GhAsset {
                    name: text(a, "name"),
                    browser_download_url: text(a, "browser_download_url"),
                    digest: a.get("digest").and_then(Value::as_str).map(str::to_string),
                })
                .collect(),
        }
    }
}

impl GhRelease {
    fn install_asset(&self) -> Option<&GhAsset> {
        static INSTALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"DNN_Platform.*Install\.zip$").expect("valid"));
        self.assets.iter().find(|a| INSTALL.is_match(&a.name))
    }

    /// The release as DNN Manager uses it: its install package, and the SHA-256s GitHub gives for it and its upgrade
    /// package.
    fn to_release(&self, install: &GhAsset) -> DnnRelease {
        let mut release = DnnRelease {
            version: self.tag_name.trim_start_matches('v').to_string(),
            tag_name: self.tag_name.clone(),
            download_url: install.browser_download_url.clone(),
            prerelease: self.prerelease,
            sha256: install.sha256(),
            upgrade_sha256: None,
        };
        release.upgrade_sha256 = release.upgrade_url().and_then(|url| {
            self.assets.iter().find(|a| a.browser_download_url.eq_ignore_ascii_case(&url)).and_then(GhAsset::sha256)
        });
        release
    }
}

/// The releases saved for offline use: `dnn_releases` of `dnnmanager.db`, shared with the WPF app.
#[derive(Debug, Clone)]
pub struct ReleaseStore {
    path: PathBuf,
}

impl ReleaseStore {
    pub fn new(database_file: impl Into<PathBuf>) -> Self {
        Self { path: database_file.into() }
    }

    fn open(&self) -> Result<Connection, String> {
        // Through AppDatabase first: a new database gets the WPF app's tables and schema version, not just this one.
        dnnm_infrastructure::app_db::AppDatabase::new(&self.path).state("", "")?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(&self.path, flags).map_err(|e| e.to_string())?;
        connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
        Ok(connection)
    }

    /// Saves the list whole, in place of the one before - with GitHub's SHA-256s, so a kept package used offline is
    /// checked as it is online. Best effort: it's only for offline use.
    pub fn save(&self, api_url: &str, releases: &[DnnRelease]) {
        let _ = self.try_save(api_url, releases);
    }

    fn try_save(&self, api_url: &str, releases: &[DnnRelease]) -> Result<(), String> {
        let mut connection = self.open()?;
        // One list or the other, never half.
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM dnn_releases WHERE api = ?1", params![api_url]).map_err(|e| e.to_string())?;
        // DateTime.Now.ToString("O"): seven digits of the second, and the offset.
        let now = chrono::Local::now();
        let now = format!(
            "{}.{:07}{}",
            now.format("%Y-%m-%dT%H:%M:%S"),
            chrono::Timelike::nanosecond(&now) % 1_000_000_000 / 100,
            now.format("%:z")
        );
        for (i, r) in releases.iter().enumerate() {
            tx.execute(
                "INSERT INTO dnn_releases (api, position, version, tag, url, prerelease, saved_utc, sha256, upgrade_sha256) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![api_url, i as i64, r.version, r.tag_name, r.download_url, r.prerelease, now, r.sha256, r.upgrade_sha256],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// The list saved for `api_url`, with when it was saved; None when there is none (or it can't be read).
    pub fn load(&self, api_url: &str) -> Option<DnnReleaseList> {
        let connection = self.open().ok()?;
        let mut statement = connection
            .prepare(
                "SELECT version, tag, url, prerelease, saved_utc, sha256, upgrade_sha256 FROM dnn_releases \
                 WHERE api = ?1 ORDER BY position",
            )
            .ok()?;
        let rows = statement
            .query_map(params![api_url], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })
            .ok()?;
        let mut releases = Vec::new();
        let mut saved_at = None;
        for row in rows {
            let (version, tag, url, prerelease, saved, sha, upgrade_sha) = row.ok()?;
            // The saved list is in Documents, which any program of the user can change: an address that isn't https
            // (to GitHub, or this computer) isn't downloaded from.
            if !is_release_source(&url) {
                continue;
            }
            releases.push(DnnRelease {
                version,
                tag_name: tag,
                download_url: url,
                prerelease: prerelease != 0,
                sha256: sha256_of(sha),
                upgrade_sha256: sha256_of(upgrade_sha),
            });
            saved_at.get_or_insert(saved);
        }
        (!releases.is_empty()).then_some(DnnReleaseList { releases, saved_at })
    }
}

/// A saved SHA-256 - 64 hex digits, lowercase - or None when there is none (or it isn't one).
fn sha256_of(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const API: &str = "https://api.github.com/repos/dnnsoftware/Dnn.Platform/releases";

    const RELEASES_JSON: &str = r#"[
      { "tag_name": "v10.0.1", "prerelease": false, "draft": false,
        "assets": [ { "name": "DNN_Platform_10.0.1_Install.zip",
                      "browser_download_url": "https://github.com/dnnsoftware/Dnn.Platform/releases/download/v10.0.1/DNN_Platform_10.0.1_Install.zip" } ] },
      { "tag_name": "v9.13.9", "prerelease": false, "draft": false,
        "assets": [ { "name": "DNN_Platform_9.13.9_Install.zip",
                      "browser_download_url": "https://github.com/dnnsoftware/Dnn.Platform/releases/download/v9.13.9/DNN_Platform_9.13.9_Install.zip" } ] }
    ]"#;

    /// GitHub: the releases given - or, offline, no answer at all.
    struct Stub {
        online: bool,
        json: String,
    }

    impl ReleaseHttp for Stub {
        fn get(&self, url: &str, _: &CancelToken) -> Result<HttpAnswer, String> {
            if !self.online {
                return Err("No such host is known. (api.github.com:443)".into());
            }
            let path = url.split('?').next().unwrap();
            Ok(if path.ends_with("/releases") {
                HttpAnswer { status: 200, link: None, body: self.json.clone() }
            } else {
                HttpAnswer { status: 404, link: None, body: String::new() }
            })
        }
    }

    fn service(dir: &std::path::Path, online: bool, json: &str) -> ReleaseService<Stub> {
        ReleaseService::new(
            Stub { online, json: json.into() },
            ReleaseStore::new(dir.join("dnnmanager.db")),
            vec![API.into()],
        )
    }

    fn saved_rows(dir: &std::path::Path) -> i64 {
        Connection::open(dir.join("dnnmanager.db"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM dnn_releases", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn offline_the_releases_saved_at_the_last_lookup_are_offered() {
        let dir = tempfile::tempdir().unwrap();
        let online = service(dir.path(), true, RELEASES_JSON).list_releases(API, &CancelToken::new()).unwrap();
        assert_eq!(online.saved_at, None, "From GitHub, not the saved list.");
        assert_eq!(saved_rows(dir.path()), 2, "Saved for offline use.");

        let offline = service(dir.path(), false, "").list_releases(API, &CancelToken::new()).unwrap();
        assert!(offline.saved_at.is_some());
        assert_eq!(offline.releases.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["10.0.1", "9.13.9"]);
        assert_eq!(online.releases[1], offline.releases[1], "The same release - its download address too.");
    }

    #[test]
    fn offline_a_chosen_version_and_the_latest_come_from_the_saved_releases() {
        let dir = tempfile::tempdir().unwrap();
        service(dir.path(), true, RELEASES_JSON).list_releases(API, &CancelToken::new()).unwrap();
        let offline = service(dir.path(), false, "");
        let cancel = CancelToken::new();
        assert_eq!(offline.get_release(API, Some("9.13.9"), &cancel).unwrap().tag_name, "v9.13.9");
        assert_eq!(offline.get_release(API, None, &cancel).unwrap().version, "10.0.1");
        let unknown = offline.get_release(API, Some("8.0.0"), &cancel).unwrap_err();
        assert_eq!(unknown, "GitHub can't be reached: No such host is known. (api.github.com:443)");
    }

    #[test]
    fn never_reached_offline_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert!(service(dir.path(), false, "").list_releases(API, &CancelToken::new()).is_err());
    }

    #[test]
    fn the_saved_releases_keep_githubs_sha256s_for_offline_use() {
        let (install, upgrade) = ("a".repeat(64), "b".repeat(64));
        let json = format!(
            r#"[ {{ "tag_name": "v10.0.1", "prerelease": false, "draft": false, "assets": [
              {{ "name": "DNN_Platform_10.0.1_Install.zip", "digest": "sha256:{install}",
                "browser_download_url": "https://github.com/dnnsoftware/Dnn.Platform/releases/download/v10.0.1/DNN_Platform_10.0.1_Install.zip" }},
              {{ "name": "DNN_Platform_10.0.1_Upgrade.zip", "digest": "sha256:{upgrade}",
                "browser_download_url": "https://github.com/dnnsoftware/Dnn.Platform/releases/download/v10.0.1/DNN_Platform_10.0.1_Upgrade.zip" }} ] }} ]"#
        );
        let dir = tempfile::tempdir().unwrap();
        let online = service(dir.path(), true, &json).list_releases(API, &CancelToken::new()).unwrap();
        assert_eq!(online.releases[0].sha256.as_deref(), Some(install.as_str()));
        let offline = service(dir.path(), false, "").list_releases(API, &CancelToken::new()).unwrap();
        assert_eq!(offline.releases[0].sha256.as_deref(), Some(install.as_str()));
        assert_eq!(offline.releases[0].upgrade_sha256.as_deref(), Some(upgrade.as_str()));

        // Changed in the database (Documents, the user's to change) to something that isn't a SHA-256: none, not that.
        Connection::open(dir.path().join("dnnmanager.db"))
            .unwrap()
            .execute("UPDATE dnn_releases SET sha256 = 'not-a-hash'", [])
            .unwrap();
        assert_eq!(
            service(dir.path(), false, "").list_releases(API, &CancelToken::new()).unwrap().releases[0].sha256,
            None
        );
    }

    #[test]
    fn highest_version_first_a_release_before_its_pre_release_and_no_drafts() {
        let asset = |v: &str| {
            format!(
                r#"[{{ "name": "DNN_Platform_{v}_Install.zip", "browser_download_url": "https://x/DNN_Platform_{v}_Install.zip" }}]"#
            )
        };
        let release = |tag: &str, pre: bool, draft: bool, v: &str| {
            format!(r#"{{ "tag_name": "{tag}", "prerelease": {pre}, "draft": {draft}, "assets": {} }}"#, asset(v))
        };
        let json = format!(
            "[{}, {}, {}, {}, {}, {}]",
            release("v9.13.10", false, false, "9.13.10"),
            release("v10.1.0-rc1", true, false, "10.1.0-rc1"),
            release("next", false, false, "next"),
            release("v10.1.0", false, false, "10.1.0"),
            release("v10.2.0", false, true, "10.2.0"),
            r#"{ "tag_name": "v10.0.0", "prerelease": false, "draft": false, "assets": [] }"#
        );
        let dir = tempfile::tempdir().unwrap();
        let list = service(dir.path(), true, &json).list_releases(API, &CancelToken::new()).unwrap();
        let tags: Vec<_> = list.releases.iter().map(|r| r.tag_name.as_str()).collect();
        assert_eq!(tags, ["v10.1.0", "v10.1.0-rc1", "v9.13.10", "next"]);
    }

    #[test]
    fn githubs_next_page_is_read_from_its_link_header() {
        assert_eq!(
            next_page(
                "<https://api.github.com/repositories/1/releases?per_page=100&page=2>; rel=\"next\", \
                 <https://api.github.com/repositories/1/releases?per_page=100&page=3>; rel=\"last\""
            )
            .as_deref(),
            Some("https://api.github.com/repositories/1/releases?per_page=100&page=2")
        );
        assert_eq!(
            next_page("<https://api.github.com/repositories/1/releases?per_page=100&page=1>; rel=\"prev\""),
            None
        );
    }

    #[test]
    fn a_release_source_is_https_or_this_computer() {
        assert!(is_release_source("https://github.com/x"));
        assert!(is_release_source("http://localhost:8080/x"));
        assert!(is_release_source("http://127.0.0.1/x"));
        assert!(!is_release_source("http://evil.example/x"));
        assert!(!is_release_source("ftp://github.com/x"));
        assert!(!is_release_source("not a url"));
    }

    #[test]
    #[ignore = "needs the internet: GitHub's real releases of DNN"]
    fn reads_dnns_releases_from_github() {
        let dir = tempfile::tempdir().unwrap();
        let service = ReleaseService::new(GitHubHttp::default(), ReleaseStore::new(dir.path().join("db")), vec![]);
        let list = service.list_releases(API, &CancelToken::new()).unwrap();
        assert!(list.releases.iter().any(|r| r.version == "10.3.3"));
    }
}
