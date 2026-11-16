//! DNN Manager's own updates - `Updates/AppRelease.cs` (the newest release from GitHub), `Updates/UpdateDownloader.cs`
//! (the download, checked by size, SHA-256 and the version in the file) and `Updates/UpdateTarget.cs` (installed or
//! portable). Applying it is [`crate::update_helper`].
//!
//! Deviation: `Files/StalledRead.cs` ends a download when nothing arrives for 60 seconds; ureq has no per-read idle
//! limit, so a download gets 60 seconds to connect and answer and 30 minutes in all, and Cancel is checked between
//! reads.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::operation::{CANCELLED, CancelToken};
use dnnm_application::project::view::group_thousands;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(windows)]
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

pub const REPOSITORY: &str = "https://github.com/Albadit/DnnManager.NET";
pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/Albadit/DnnManager.NET/releases/latest";

/// A version as `System.Version` has it: major.minor[.build[.revision]]; a missing build or revision is None.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub build: Option<u32>,
    pub revision: Option<u32>,
}

impl Version {
    /// "1.7.0" - two to four numbers.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<u32> = text.trim().split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        Some(Self { major: parts[0], minor: parts[1], build: parts.get(2).copied(), revision: parts.get(3).copied() })
    }

    // By major, minor and build only.
    fn normalized(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.build.unwrap_or(0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if let Some(build) = self.build {
            write!(f, ".{build}")?;
        }
        if let Some(revision) = self.revision {
            write!(f, ".{revision}")?;
        }
        Ok(())
    }
}

/// "v1.7.0" or "1.7.0" - a pre-release suffix ("-rc.1") or build metadata ("+commit") is left out.
pub fn try_parse_version(tag: &str) -> Option<Version> {
    let text = tag.trim_start_matches(['v', 'V']);
    Version::parse(text.split(['-', '+']).next().unwrap_or_default())
}

/// Whether `latest` is newer than `current` - by major, minor and build only.
pub fn is_newer(latest: &Version, current: &Version) -> bool {
    latest.normalized() > current.normalized()
}

/// How this DNN Manager is updated: by running the new Setup over it, or by replacing the portable exe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UpdateKind {
    Installer,
    Portable,
}

/// A file of a release, with the SHA-256 GitHub computed for it (None when it gives none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppReleaseAsset {
    pub name: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub url: String,
}

/// A published release: its tag, version, page and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRelease {
    pub tag: String,
    pub version: Version,
    pub page_url: String,
    pub assets: Vec<AppReleaseAsset>,
}

/// This process's architecture as the release names it (`RuntimeInformation.ProcessArchitecture`).
pub fn architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => other,
    }
}

impl AppRelease {
    /// The file that updates this kind of DNN Manager: Setup for an installed one, the portable exe for a portable one -
    /// for `arch`, as the release workflow names them (`DnnManager_Setup-1.7.7-x64.exe`, `DnnManager_Portable-1.7.7-x64.exe`)
    /// or as releases up to 1.7.6 did (`DnnManagerSetup-…`, `DnnManager-…`). None when the release has none.
    pub fn asset_for(&self, kind: UpdateKind, arch: &str) -> Option<&AppReleaseAsset> {
        let prefixes: [&str; 2] = match kind {
            UpdateKind::Installer => ["DnnManager_Setup-", "DnnManagerSetup-"],
            UpdateKind::Portable => ["DnnManager_Portable-", "DnnManager-"],
        };
        let suffix = format!("-{}.exe", arch.to_lowercase());
        prefixes.iter().find_map(|prefix| {
            self.assets.iter().find(|a| {
                let name = a.name.to_lowercase();
                name.starts_with(&prefix.to_lowercase()) && name.ends_with(&suffix)
            })
        })
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: Option<String>,
    html_url: Option<String>,
    assets: Option<Vec<GhAsset>>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: Option<String>,
    #[serde(default)]
    size: u64,
    digest: Option<String>,
    browser_download_url: Option<String>,
}

/// A release from GitHub's JSON answer.
pub fn parse_release(json: &str) -> Result<AppRelease, String> {
    let release: GhRelease = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = release.tag_name.ok_or("GitHub's answer has no version")?;
    let version = try_parse_version(&tag).ok_or("GitHub's answer has no version")?;
    let assets = release
        .assets
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| {
            Some(AppReleaseAsset {
                name: a.name?,
                size: a.size,
                sha256: sha256_of(a.digest.as_deref()),
                url: a.browser_download_url?,
            })
        })
        .collect();
    let page_url = release.html_url.unwrap_or_else(|| format!("{REPOSITORY}/releases/tag/{tag}"));
    Ok(AppRelease { tag, version, page_url, assets })
}

// "sha256:677af2…" - GitHub's digest of the file.
fn sha256_of(digest: Option<&str>) -> Option<String> {
    let d = digest?;
    (d.len() > 7 && d[..7].eq_ignore_ascii_case("sha256:")).then(|| d[7..].to_lowercase())
}

fn agent(answer_within: Duration, body_within: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(answer_within))
        .timeout_recv_response(Some(answer_within))
        .timeout_recv_body(Some(body_within))
        .build()
        .into()
}

fn status_text(status: ureq::http::StatusCode) -> String {
    format!("{} {}", status.as_u16(), status.canonical_reason().unwrap_or_default())
}

fn transport_message(e: ureq::Error, timed_out: &str) -> String {
    match e {
        ureq::Error::Timeout(_) => timed_out.to_string(),
        other => other.to_string(),
    }
}

/// The newest release from `url` ([`LATEST_RELEASE_URL`]) - never a draft or a pre-release. The error says why in
/// words for the user.
pub fn get_latest(url: &str) -> Result<AppRelease, String> {
    let agent = agent(Duration::from_secs(100), Duration::from_secs(100));
    let mut response = agent
        .get(url)
        .header("User-Agent", "DnnManager")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| transport_message(e, "GitHub didn't answer in time"))?;
    if !response.status().is_success() {
        return Err(format!("GitHub answered {}", status_text(response.status())));
    }
    let json =
        response.body_mut().read_to_string().map_err(|e| transport_message(e, "GitHub didn't answer in time"))?;
    parse_release(&json)
}

/// `asset` of `release` in `folder`, checked; `progress` goes from 0 to 1. A file already there that passes the checks
/// is used as it is.
pub fn download(
    release: &AppRelease,
    asset: &AppReleaseAsset,
    folder: &Path,
    progress: &mut dyn FnMut(f64),
    cancel: &CancelToken,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let path = folder.join(&asset.name);
    if path.is_file() && problem(&path, release, asset).is_none() {
        progress(1.0);
        return Ok(path);
    }
    let mut partial = path.clone().into_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let result = fetch(asset, &partial, progress, cancel).and_then(|()| {
        if let Some(problem) = problem(&partial, release, asset) {
            return Err(format!("The download of {} isn't right: {problem}", asset.name));
        }
        std::fs::rename(&partial, &path).map_err(|e| e.to_string())
    });
    let _ = std::fs::remove_file(&partial);
    result.map(|()| path)
}

fn fetch(
    asset: &AppReleaseAsset,
    partial: &Path,
    progress: &mut dyn FnMut(f64),
    cancel: &CancelToken,
) -> Result<(), String> {
    let stopped = format!("The download of {} stopped answering", asset.name);
    let agent = agent(Duration::from_secs(60), Duration::from_secs(30 * 60));
    let mut response =
        agent.get(&asset.url).header("User-Agent", "DnnManager").call().map_err(|e| transport_message(e, &stopped))?;
    if !response.status().is_success() {
        return Err(format!("GitHub answered {} for {}", status_text(response.status()), asset.name));
    }
    let mut source = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut target = std::fs::File::create(partial).map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; 81920];
    let mut done = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(CANCELLED.into());
        }
        let read = source
            .read(&mut buffer)
            .map_err(|e| if e.kind() == std::io::ErrorKind::TimedOut { stopped.clone() } else { e.to_string() })?;
        if read == 0 {
            break;
        }
        target.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        done += read as u64;
        if asset.size > 0 {
            progress((done as f64 / asset.size as f64).min(1.0));
        }
    }
    target.sync_all().map_err(|e| e.to_string())
}

/// The SHA-256 of a file, in lower-case hex.
pub fn sha256_file(file: &mut dyn Read) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 81920];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// What is wrong with a downloaded file; None when it is the release's.
pub fn problem(path: &Path, release: &AppRelease, asset: &AppReleaseAsset) -> Option<String> {
    let length = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if length != asset.size {
        return Some(format!(
            "it is {} bytes, GitHub lists {}",
            group_thousands(length as f64, 0),
            group_thousands(asset.size as f64, 0)
        ));
    }
    // It is run with administrator rights: without GitHub's SHA-256 there is nothing to check it against.
    let Some(expected) = &asset.sha256 else {
        return Some("GitHub lists no SHA-256 for it, so it can't be checked".into());
    };
    let actual = std::fs::File::open(path).and_then(|mut f| sha256_file(&mut f));
    if actual.ok().as_ref() != Some(expected) {
        return Some("its SHA-256 isn't the one GitHub lists".into());
    }
    // Setup and the exe carry the release's version (ProductVersion "1.7.0", or "1.7.0+commit").
    #[cfg(windows)]
    let product = crate::win::version_string(path, "ProductVersion");
    // Off Windows a file has no ProductVersion to read.
    #[cfg(not(windows))]
    let product: Option<String> = None;
    version_problem(product.as_deref(), &release.version)
}

/// Why a file whose ProductVersion is `product` isn't `expected`; None when it is.
pub fn version_problem(product: Option<&str>, expected: &Version) -> Option<String> {
    match product.and_then(try_parse_version) {
        Some(v) if !is_newer(&v, expected) && !is_newer(expected, &v) => None,
        _ => Some(format!("it is version {}, not {expected}", product.unwrap_or("(none)"))),
    }
}

// ─── UpdateTarget ───────────────────────────────────────────────────────

/// Keep in step with `AppGuid` in `src\DnnManager.Installer\DnnManager.iss`.
#[cfg(windows)]
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{AD68C57A-D887-4297-A905-B6F28C1D66D1}_is1";

/// What an update replaces: the exe that runs now and, when it was installed, for whom (Setup's `/ALLUSERS` or
/// `/CURRENTUSER`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTarget {
    pub kind: UpdateKind,
    pub app_exe: PathBuf,
    pub all_users: bool,
}

impl UpdateTarget {
    /// Installed by Setup when the exe is in an installation's folder, portable when it is a single-file exe anywhere
    /// else (a Rust exe always is). None for a development build - it isn't updated, it is rebuilt.
    pub fn detect(exe: Option<&Path>, is_single_file: bool, installations: &[(PathBuf, bool)]) -> Option<Self> {
        let exe = exe.filter(|e| !e.as_os_str().is_empty())?;
        let folder = normalize(exe.parent()?);
        for (installed, all_users) in installations {
            if normalize(installed).eq_ignore_ascii_case(&folder) {
                return Some(Self { kind: UpdateKind::Installer, app_exe: exe.to_path_buf(), all_users: *all_users });
            }
        }
        is_single_file.then(|| Self { kind: UpdateKind::Portable, app_exe: exe.to_path_buf(), all_users: false })
    }

    /// [`detect`](Self::detect) with [`installed_locations`].
    pub fn detect_here(exe: Option<&Path>, is_single_file: bool) -> Option<Self> {
        Self::detect(exe, is_single_file, &installed_locations())
    }
}

fn normalize(folder: &Path) -> String {
    let full = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    full.display().to_string().trim_end_matches(['\\', '/']).to_string()
}

/// Where Setup installed DNN Manager - for the current user, and for all users.
#[cfg(not(windows))]
pub fn installed_locations() -> Vec<(PathBuf, bool)> {
    Vec::new()
}

/// Where Setup installed DNN Manager - for the current user, and for all users.
#[cfg(windows)]
pub fn installed_locations() -> Vec<(PathBuf, bool)> {
    [(HKEY_CURRENT_USER, false), (HKEY_LOCAL_MACHINE, true)]
        .into_iter()
        .filter_map(|(hive, all_users)| install_location(hive).map(|l| (PathBuf::from(l), all_users)))
        .collect()
}

#[cfg(windows)]
fn install_location(hive: HKEY) -> Option<String> {
    use std::ffi::c_void;
    use windows::Win32::System::Registry::{RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW};
    use windows::core::PCWSTR;
    let key = crate::win::wide(UNINSTALL_KEY);
    let name = crate::win::wide("InstallLocation");
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer's size in bytes is passed in.
    unsafe {
        RegGetValueW(
            hive,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(buffer.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(crate::win::from_wide(&buffer)).filter(|l| !l.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // GitHub's answer for a release, cut to what is read.
    const SAMPLE: &str = r#"{
      "url": "https://api.github.com/repos/Albadit/DnnManager.NET/releases/1",
      "html_url": "https://github.com/Albadit/DnnManager.NET/releases/tag/v1.8.7",
      "tag_name": "v1.8.7",
      "draft": false,
      "prerelease": false,
      "assets": [
        { "name": "DnnManager_Setup-1.8.7-x64.exe", "size": 52428800,
          "digest": "sha256:677AF2AA00000000000000000000000000000000000000000000000000000000",
          "browser_download_url": "https://github.com/Albadit/DnnManager.NET/releases/download/v1.8.7/DnnManager_Setup-1.8.7-x64.exe" },
        { "name": "DnnManager_Portable-1.8.7-x64.exe", "size": 70000000, "digest": null,
          "browser_download_url": "https://github.com/x/DnnManager_Portable-1.8.7-x64.exe" },
        { "name": "DnnManager-1.7.0-arm64.exe", "size": 1, "browser_download_url": "https://x/arm" },
        { "name": "no-url.exe", "size": 1 }
      ]
    }"#;

    #[test]
    fn parses_githubs_latest_release() {
        let release = parse_release(SAMPLE).unwrap();
        assert_eq!(release.tag, "v1.8.7");
        assert_eq!(release.version, Version { major: 1, minor: 8, build: Some(7), revision: None });
        assert_eq!(release.page_url, "https://github.com/Albadit/DnnManager.NET/releases/tag/v1.8.7");
        assert_eq!(release.assets.len(), 3, "an asset without a URL is left out");
        assert_eq!(
            release.assets[0].sha256.as_deref(),
            Some("677af2aa00000000000000000000000000000000000000000000000000000000")
        );
        assert_eq!(release.assets[1].sha256, None);

        assert_eq!(release.asset_for(UpdateKind::Installer, "x64").unwrap().name, "DnnManager_Setup-1.8.7-x64.exe");
        assert_eq!(release.asset_for(UpdateKind::Portable, "x64").unwrap().name, "DnnManager_Portable-1.8.7-x64.exe");
        assert_eq!(release.asset_for(UpdateKind::Portable, "arm64").unwrap().name, "DnnManager-1.7.0-arm64.exe");
        assert_eq!(release.asset_for(UpdateKind::Installer, "arm64"), None);
        assert_eq!(parse_release(r#"{"tag_name":"latest"}"#).unwrap_err(), "GitHub's answer has no version");
        assert!(parse_release("not json").is_err());
    }

    #[test]
    fn versions_compare_by_major_minor_and_build() {
        let v = |t| try_parse_version(t).unwrap();
        assert_eq!(v("v1.7.0-rc.1").to_string(), "1.7.0");
        assert_eq!(v("1.7.0+abc").to_string(), "1.7.0");
        assert!(try_parse_version("v1").is_none());
        assert!(is_newer(&v("1.8.0"), &v("1.7.9")));
        assert!(!is_newer(&v("1.7"), &v("1.7.0")), "1.7 is 1.7.0");
        assert!(!is_newer(&v("1.7.0.5"), &v("1.7.0")), "the revision doesn't count");
        assert_eq!(version_problem(Some("1.8.7+3f2a"), &v("1.8.7")), None);
        assert_eq!(version_problem(Some("1.8.6"), &v("1.8.7")).unwrap(), "it is version 1.8.6, not 1.8.7");
        assert_eq!(version_problem(None, &v("1.8.7")).unwrap(), "it is version (none), not 1.8.7");
    }

    #[test]
    fn a_download_is_checked_by_size_and_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.exe");
        std::fs::write(&file, b"abc").unwrap();
        let release = parse_release(SAMPLE).unwrap();
        let mut asset = AppReleaseAsset { name: "x.exe".into(), size: 4, sha256: None, url: String::new() };
        assert_eq!(problem(&file, &release, &asset).unwrap(), "it is 3 bytes, GitHub lists 4");
        asset.size = 3;
        assert_eq!(problem(&file, &release, &asset).unwrap(), "GitHub lists no SHA-256 for it, so it can't be checked");
        asset.sha256 = Some("00".into());
        assert_eq!(problem(&file, &release, &asset).unwrap(), "its SHA-256 isn't the one GitHub lists");
        asset.sha256 = Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into());
        assert_eq!(problem(&file, &release, &asset).unwrap(), "it is version (none), not 1.8.7");
    }

    #[test]
    fn tells_an_installed_dnn_manager_from_a_portable_one() {
        let installs = vec![(PathBuf::from(r"C:\Program Files\DNN Manager\"), true)];
        let installed =
            UpdateTarget::detect(Some(Path::new(r"C:\Program Files\DNN Manager\DnnManager.exe")), true, &installs);
        assert_eq!(installed.unwrap().kind, UpdateKind::Installer);
        let portable = UpdateTarget::detect(Some(Path::new(r"D:\Tools\DnnManager.exe")), true, &installs).unwrap();
        assert_eq!((portable.kind, portable.all_users), (UpdateKind::Portable, false));
        assert_eq!(UpdateTarget::detect(Some(Path::new(r"D:\src\bin\DnnManager.exe")), false, &installs), None);
        assert_eq!(UpdateTarget::detect(None, true, &installs), None);
        let _ = installed_locations();
        assert!(["x64", "arm64", "x86"].contains(&architecture()));
    }
}
