//! Downloads a DNN package and puts it into a site (`src/DnnManager.Infrastructure/Github/DnnPackageInstaller.cs`,
//! `IDnnPackageInstaller`). With Keep downloaded DNN install packages on, the package is kept in
//! `Documents\DnnManager\packages\<owner>.<repo>\` and used again next time instead of downloading it; otherwise it is
//! downloaded into DNN Manager's own temporary folder and deleted after extracting - also when extracting fails.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dnnm_application::operation::{CANCELLED, CancelToken, Reporter};
use dnnm_domain::DnnRelease;
use sha2::{Digest, Sha256};
use xot::Node;

use crate::format::{n0, n1};
use crate::metadata::strong_name;
use crate::releases::is_release_source;
use crate::safe_zip::{self, Archive, CheckedFolders, ZipError};
use crate::version::Version;
use crate::xml::SiteXml;

/// How long a read of a download may get nothing before the download counts as stopped (`StalledRead.After`).
const STALLED_AFTER: Duration = Duration::from_secs(60);

/// What DNN's LocalUpgradeService leaves out when the package has no upgrade.json of its own.
const DEFAULT_UPGRADE_EXCLUDE: [&str; 6] = [
    "App_Data/Database.mdf",
    "Config/DotNetNuke.config",
    "Install/InstallWizard",
    "favicon.ico",
    "robots.txt",
    "web.config",
];

/// The version range DNN's AssemblyInstaller redirects from: everything.
pub const REDIRECT_FROM: &str = "0.0.0.0-32767.32767.32767.32767";

const ASSEMBLY_BINDING: &str = "urn:schemas-microsoft-com:asm.v1";

/// A transient line for the status bar (`IProgressReporter.Progress`), which the port's `Reporter` doesn't have.
pub type Progress<'a> = &'a mut dyn FnMut(&str);

/// Where packages go (`AppOptions.KeepDnnPackages`, `AppDataPaths.PackagesDirectory`, `PrivateTemp.Path`).
#[derive(Debug, Clone)]
pub struct PackageOptions {
    pub keep_packages: bool,
    /// `Documents\DnnManager\packages`.
    pub packages_directory: PathBuf,
    /// A folder only administrators can write - the files DNN Manager unpacks with administrator rights come from
    /// where only administrators can change them.
    pub private_temp: PathBuf,
}

/// `IDnnPackageInstaller`.
pub struct PackageInstaller {
    options: PackageOptions,
    agent: ureq::Agent,
}

/// Why getting or unpacking a package failed - the C# tells these apart by exception type.
enum Failure {
    /// `HttpRequestException`: the server answered with an error, or wasn't reached.
    Http(Option<u16>, String),
    /// `RefusedException`: not https, or not the file GitHub lists.
    Refused(String),
    /// `InvalidDataException`: the zip is damaged.
    Damaged(String),
    Other(String),
    Cancelled,
}

impl From<ZipError> for Failure {
    fn from(e: ZipError) -> Self {
        match e {
            ZipError::Damaged(m) => Failure::Damaged(m),
            ZipError::Other(m) if m == CANCELLED => Failure::Cancelled,
            ZipError::Other(m) => Failure::Other(m),
        }
    }
}

impl From<String> for Failure {
    fn from(m: String) -> Self {
        if m == CANCELLED { Failure::Cancelled } else { Failure::Other(m) }
    }
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Failure::Http(_, m) | Failure::Refused(m) | Failure::Damaged(m) | Failure::Other(m) => m.clone(),
            Failure::Cancelled => CANCELLED.into(),
        }
    }
}

/// A package on disk to unpack: kept (in Documents) or downloaded for this one use (deleted after it).
struct Package {
    path: PathBuf,
    kept: bool,
}

/// Deletes a package downloaded for one use - kept ones stay.
fn discard(package: &Option<Package>) {
    if let Some(Package { path, kept: false }) = package {
        let _ = std::fs::remove_file(path);
    }
}

impl PackageInstaller {
    pub fn new(options: PackageOptions) -> Self {
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_recv_response(Some(Duration::from_secs(100)))
            .timeout_per_read(Some(STALLED_AFTER))
            .user_agent("DnnManager-NET")
            .build()
            .new_agent();
        Self { options, agent }
    }

    /// The release's install package is kept from an earlier download, so installing it needs no download.
    pub fn is_kept(&self, release: &DnnRelease) -> bool {
        self.options.keep_packages
            && self.kept_path(&release.download_url, &format!("DNN_Platform_{}_Install.zip", release.version)).is_file()
    }

    /// Downloads (or takes the kept) install package and extracts it into `project_directory`.
    pub fn download_and_extract(
        &self,
        release: &DnnRelease,
        project_directory: &Path,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let mut package = None;
        let result = (|| {
            let what = format!("DNN {}", release.version);
            let name = format!("DNN_Platform_{}_Install.zip", release.version);
            package = Some(self.package(
                &release.download_url,
                release.sha256.as_deref(),
                &what,
                &name,
                reporter,
                progress,
                cancel,
            )?);
            let got = package.as_ref().expect("set");
            reporter.info("Extracting…");
            std::fs::create_dir_all(project_directory).map_err(|e| Failure::Other(e.to_string()))?;
            match extract_over(&got.path, project_directory, false, progress, cancel) {
                Err(Failure::Damaged(_)) if got.kept => {
                    // A damaged kept package would fail every time - drop it so the next try downloads it again.
                    let _ = std::fs::remove_file(&got.path);
                    return Err(Failure::Other(format!(
                        "The kept package {} is damaged and has been deleted - try again to download it.",
                        got.path.display()
                    )));
                }
                other => other?,
            };
            reporter.success("Extraction complete.");
            Ok(())
        })();
        // Downloaded for this one use: gone whether it worked or not.
        discard(&package);
        result.map_err(|failure| match failure {
            // Offline, most likely: only a kept package installs without a download.
            Failure::Http(_, message) => format!(
                "DNN {} could not be downloaded ({message}). {}",
                release.version,
                if self.options.keep_packages {
                    "Without internet only a version marked \"kept, no download\" can be set up."
                } else {
                    "To set up projects without internet, turn on Settings → DNN releases → Keep downloaded DNN install packages while online."
                }
            ),
            other => other.message(),
        })
    }

    /// Puts the release's upgrade package over the site - every file of it but `web.config`, which keeps the site's own
    /// settings. Kept like an install package, otherwise downloaded and deleted again.
    pub fn extract_upgrade(
        &self,
        release: &DnnRelease,
        project_directory: &Path,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let Some(url) = release.upgrade_url() else {
            return Err(format!(
                "DNN {}'s release has no upgrade package beside its install package.",
                release.version
            ));
        };
        let mut package = None;
        let result = (|| {
            let what = format!("DNN {}'s upgrade package", release.version);
            let name = format!("DNN_Platform_{}_Upgrade.zip", release.version);
            package = Some(self.package(
                &url,
                release.upgrade_sha256.as_deref(),
                &what,
                &name,
                reporter,
                progress,
                cancel,
            )?);
            let got = package.as_ref().expect("set");
            reporter.info("Putting the new files in…");
            let count = match extract_over(&got.path, project_directory, true, progress, cancel) {
                Err(Failure::Damaged(_)) if got.kept => {
                    let _ = std::fs::remove_file(&got.path);
                    return Err(Failure::Other(format!(
                        "The kept upgrade package {} is damaged and has been deleted - try again to download it.",
                        got.path.display()
                    )));
                }
                other => other?,
            };
            reporter.success(&format!("{} files of DNN {} are in.", n0(count as i64), release.version));
            Ok(())
        })();
        discard(&package);
        result.map_err(|failure| match failure {
            Failure::Http(Some(404), _) => format!("DNN {}'s release has no upgrade package ({url}).", release.version),
            Failure::Http(_, message) => {
                format!("DNN {}'s upgrade package could not be downloaded ({message}).", release.version)
            }
            other => other.message(),
        })
    }

    /// For a site on DNN 10.2 or newer: puts the release's install package over the site as DNN's own local upgrade
    /// does (`LocalUpgradeService.StartLocalUpgrade`) - each assembly in bin with a binding redirect to its version in
    /// web.config, everything else but what the package's `App_Data/Upgrade/upgrade.json` excludes (web.config among
    /// it).
    pub fn extract_local_upgrade(
        &self,
        release: &DnnRelease,
        project_directory: &Path,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let mut package = None;
        let result = (|| {
            let what = format!("DNN {}'s install package", release.version);
            let name = format!("DNN_Platform_{}_Install.zip", release.version);
            package = Some(self.package(
                &release.download_url,
                release.sha256.as_deref(),
                &what,
                &name,
                reporter,
                progress,
                cancel,
            )?);
            let got = package.as_ref().expect("set");
            reporter.info("Putting the new files in as DNN's own upgrade does: each assembly with its binding redirect, then the rest…");
            let (assemblies, files) = local_upgrade_over(&got.path, project_directory, reporter, progress, cancel)?;
            reporter.success(&format!(
                "{} assemblies (with their binding redirects) and {} other files of DNN {} are in.",
                n0(assemblies as i64),
                n0(files as i64),
                release.version
            ));
            Ok(())
        })();
        let kept = package.as_ref().is_some_and(|p| p.kept);
        let message = result.map_err(|failure| match failure {
            Failure::Http(_, message) => {
                format!("DNN {}'s install package could not be downloaded ({message}).", release.version)
            }
            Failure::Damaged(message) => {
                if let Some(p) = package.as_ref().filter(|p| p.kept) {
                    let _ = std::fs::remove_file(&p.path);
                }
                format!(
                    "DNN {}'s install package is damaged ({message}){}.",
                    release.version,
                    if kept { " and has been deleted - try again to download it" } else { "" }
                )
            }
            other => other.message(),
        });
        discard(&package);
        message
    }

    /// The package at `url` on disk: the kept one when it is there and still the file GitHub lists, or downloaded -
    /// kept when Keep downloaded DNN install packages is on, otherwise into the admin-only temporary folder.
    #[allow(clippy::too_many_arguments)]
    fn package(
        &self,
        url: &str,
        sha256: Option<&str>,
        what: &str,
        kept_name: &str,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<Package, Failure> {
        let keep = self.options.keep_packages;
        let zip_path = if keep {
            self.kept_path(url, kept_name)
        } else {
            let stem = kept_name.strip_suffix(".zip").unwrap_or(kept_name);
            self.options.private_temp.join(format!("DnnManager-{stem}-{}.zip", random_hex()))
        };
        if keep && zip_path.is_file() {
            if let Some(changed) = kept_mismatch(&zip_path, sha256) {
                return Err(Failure::Other(changed));
            }
            reporter.info(&format!("Using the kept {what} {} - no download needed.", zip_path.display()));
            return Ok(Package { path: zip_path, kept: true });
        }
        if let Some(folder) = zip_path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| Failure::Other(format!("{}: {e}", folder.display())))?;
        }
        self.download(url, what, &zip_path, sha256, reporter, progress, cancel)?;
        reporter.success(&if keep {
            format!("Downloaded and kept: {}", zip_path.display())
        } else {
            "Downloaded.".into()
        });
        Ok(Package { path: zip_path, kept: keep })
    }

    /// Downloads a package (`what`: "DNN 10.3.3") through a temporary file, so a failed or cancelled download never
    /// looks complete. Only from https (or this computer) - also every address it is sent on to - and, with GitHub's
    /// SHA-256 of the file, only when it matches.
    #[allow(clippy::too_many_arguments)]
    fn download(
        &self,
        url: &str,
        what: &str,
        zip_path: &Path,
        sha256: Option<&str>,
        reporter: &mut dyn Reporter,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), Failure> {
        if !is_release_source(url) {
            return Err(Failure::Refused(format!(
                "{what} isn't downloaded from {url}: it isn't https - only https addresses are."
            )));
        }
        reporter.info(&format!("Downloading {what}…"));
        let tmp = PathBuf::from(format!("{}.download", zip_path.display()));
        let result = self.download_to(url, what, &tmp, sha256, progress, cancel);
        match result {
            Ok(()) => std::fs::rename(&tmp, zip_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Failure::Other(format!("{}: {e}", zip_path.display()))
            }),
            Err(failure) => {
                let _ = std::fs::remove_file(&tmp);
                Err(failure)
            }
        }
    }

    fn download_to(
        &self,
        url: &str,
        what: &str,
        tmp: &Path,
        sha256: Option<&str>,
        progress: Progress,
        cancel: &CancelToken,
    ) -> Result<(), Failure> {
        let mut current = url.to_string();
        let mut response = None;
        for _ in 0..=10 {
            cancel.check()?;
            let answer = self.agent.get(&current).call().map_err(|e| Failure::Http(None, e.to_string()))?;
            let status = answer.status().as_u16();
            if (300..400).contains(&status) {
                let location = answer.headers().get("location").and_then(|l| l.to_str().ok()).map(str::to_string);
                let Some(next) = location.and_then(|l| url::Url::parse(&current).ok()?.join(&l).ok()) else {
                    return Err(Failure::Http(Some(status), format!("{status} without an address to go on to")));
                };
                // Sent on to an address that isn't https: anyone on the way could change the package.
                if redirect_not_https(next.as_str()).is_some() {
                    return Err(Failure::Refused(format!(
                        "{what} isn't downloaded: {url} sends it on to {next}, which isn't https - only https addresses are."
                    )));
                }
                current = next.to_string();
                continue;
            }
            response = Some(answer);
            break;
        }
        let Some(response) = response else {
            return Err(Failure::Http(None, "Too many redirects.".into()));
        };
        let status = response.status();
        if !status.is_success() {
            return Err(Failure::Http(
                Some(status.as_u16()),
                format!(
                    "Response status code does not indicate success: {} ({}).",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
            ));
        }
        let total =
            response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        let mut input = response.into_body().into_reader();
        let mut output = std::fs::File::create(tmp).map_err(|e| Failure::Other(format!("{}: {e}", tmp.display())))?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 81920];
        let mut done = 0u64;
        let mut next = Instant::now();
        loop {
            cancel.check()?;
            let read = match input.read(&mut buffer) {
                Ok(read) => read,
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut || e.to_string().to_lowercase().contains("timeout") =>
                {
                    return Err(Failure::Other(format!(
                        "The download stopped: nothing arrived for {} seconds. Try again.",
                        STALLED_AFTER.as_secs()
                    )));
                }
                Err(e) => return Err(Failure::Http(None, e.to_string())),
            };
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read]).map_err(|e| Failure::Other(format!("{}: {e}", tmp.display())))?;
            hash.update(&buffer[..read]);
            done += read as u64;
            if Instant::now() < next {
                continue;
            }
            next = Instant::now() + Duration::from_millis(250);
            let mb = |b: u64| b as f64 / 1_048_576.0;
            progress(&match total.filter(|t| *t > 0) {
                Some(total) => {
                    format!("Downloading {what}: {}% ({} of {} MB)", done * 100 / total, n1(mb(done)), n1(mb(total)))
                }
                None => format!("Downloading {what}: {} MB", n1(mb(done))),
            });
        }
        output.flush().map_err(|e| Failure::Other(e.to_string()))?;
        drop(output);
        if let Some(expected) = sha256
            && hex(&hash.finalize()) != expected
        {
            return Err(Failure::Refused(format!(
                "{what} from {url} isn't the file GitHub lists for the release (its SHA-256 differs) - it was changed on the way, and isn't used."
            )));
        }
        Ok(())
    }

    /// `packages\<owner>.<repo>\<asset>` for the package at `url` - per repository, since forks may ship different
    /// packages under one name; `fallback_name` when the URL names no zip.
    fn kept_path(&self, url: &str, fallback_name: &str) -> PathBuf {
        let mut folder = "other".to_string();
        let mut file_name = fallback_name.to_string();
        if let Ok(uri) = url::Url::parse(url) {
            // https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>
            let parts: Vec<&str> = uri.path().split('/').filter(|p| !p.is_empty()).collect();
            if parts.len() >= 2 {
                folder = format!("{}.{}", parts[0], parts[1]);
            }
            if let Some(last) = parts.last().filter(|p| p.to_ascii_lowercase().ends_with(".zip")) {
                file_name = last.to_string();
            }
        }
        self.options.packages_directory.join(file_name_safe(&folder)).join(file_name_safe(&file_name))
    }
}

/// `Path.GetInvalidFileNameChars` replaced with '_'.
fn file_name_safe(name: &str) -> String {
    name.chars().map(|c| if c < ' ' || "\"<>|:*?\\/".contains(c) { '_' } else { c }).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    hex(&bytes)
}

/// The address a download was sent on to when that isn't https (or this computer); None when it is
/// (`DnnPackageInstaller.RedirectNotHttps`).
pub fn redirect_not_https(target: &str) -> Option<String> {
    (!is_release_source(target)).then(|| target.to_string())
}

/// Why the kept package can't be used - it isn't the file GitHub lists any more (it is in Documents, which any program
/// of the user can change); it is deleted then, so the next try downloads it again. None when it matches, or when
/// GitHub gives no SHA-256 for it.
fn kept_mismatch(zip_path: &Path, sha256: Option<&str>) -> Option<String> {
    let expected = sha256?;
    let matches = std::fs::File::open(zip_path).ok().and_then(|mut file| {
        // sha2 0.11's hashers aren't io::Write: the file goes in in blocks.
        let mut hash = Sha256::new();
        let mut block = vec![0u8; 64 * 1024];
        loop {
            let read = std::io::Read::read(&mut file, &mut block).ok()?;
            if read == 0 {
                break;
            }
            hash.update(&block[..read]);
        }
        Some(hex(&hash.finalize()) == expected)
    });
    if matches == Some(true) {
        return None;
    }
    let _ = std::fs::remove_file(zip_path);
    Some(format!(
        "The kept package {} isn't the file GitHub lists for the release any more (its SHA-256 differs) - it has been deleted; try again to download it.",
        zip_path.display()
    ))
}

/// Reports "Putting the new files in: n of m" at most every 250 ms.
struct Pacer(Instant);

impl Pacer {
    fn new() -> Self {
        Self(Instant::now())
    }

    fn tick(&mut self, progress: &mut dyn FnMut(&str), count: usize, total: usize) {
        if Instant::now() < self.0 {
            return;
        }
        self.0 = Instant::now() + Duration::from_millis(250);
        progress(&format!("Putting the new files in: {} of {}", n0(count as i64), n0(total as i64)));
    }
}

/// Every entry's place under `root`: a package with one unsafe path writes nothing.
fn targets(
    root: &Path,
    entries: &[(usize, String, u64)],
    checked: &mut CheckedFolders,
) -> Result<Vec<(usize, PathBuf)>, Failure> {
    entries
        .iter()
        .map(|(index, name, _)| Ok((*index, safe_zip::target(root, name, Some(checked)).map_err(Failure::Other)?)))
        .collect()
}

/// Every file of the package over `directory`, overwriting - but, with `keep_web_config`, never web.config (compared
/// as the path it resolves to: "./web.config", "x/../web.config" and WEB~1.CON are web.config too). The number of files
/// written.
fn extract_over(
    zip_path: &Path,
    directory: &Path,
    keep_web_config: bool,
    progress: Progress,
    cancel: &CancelToken,
) -> Result<usize, Failure> {
    let mut zip = safe_zip::open(zip_path)?;
    let files = safe_zip::files(&mut zip)?;
    safe_zip::ensure_fits(files.iter().map(|f| f.2), directory).map_err(Failure::Other)?;
    let mut checked = CheckedFolders::new();
    let targets: Vec<_> = targets(directory, &files, &mut checked)?
        .into_iter()
        .filter(|(_, target)| {
            !keep_web_config || !safe_zip::relative(directory, target).eq_ignore_ascii_case("web.config")
        })
        .collect();
    let mut pacer = Pacer::new();
    for (count, (index, target)) in targets.iter().enumerate() {
        cancel.check()?;
        safe_zip::write_entry(directory, &mut zip, *index, target, &mut checked)?;
        pacer.tick(progress, count + 1, files.len());
    }
    Ok(targets.len())
}

/// DNN's `LocalUpgradeService.StartLocalUpgrade`, done from outside the site: bin's assemblies first - each copied in
/// and, when it is strong-named, web.config's binding redirect for it pointed at its version - then every other file
/// but the excluded ones (compared with the path each entry resolves to). The numbers written.
fn local_upgrade_over(
    zip_path: &Path,
    directory: &Path,
    reporter: &mut dyn Reporter,
    progress: Progress,
    cancel: &CancelToken,
) -> Result<(usize, usize), Failure> {
    let root =
        safe_zip::full(directory).ok_or_else(|| Failure::Other(format!("{} isn't a folder.", directory.display())))?;
    let mut zip = safe_zip::open(zip_path)?;
    let files = safe_zip::files(&mut zip)?;
    let (exclude, minimum) = upgrade_info(&mut zip, &files)?;
    if let (Some(minimum), Some(current)) = (minimum.as_deref(), dnnm_infrastructure::dnn_install::dnn_version(&root))
        && let (Some(needs), Some(have)) = (Version::parse(minimum), Version::parse(&current))
        && have < Version::new(needs.major, needs.minor, needs.build_or_zero())
    {
        return Err(Failure::Other(format!(
            "This package upgrades DNN {minimum} or newer only - the site runs {current}."
        )));
    }
    // As DNN compares them: the start of the path from the site's folder, '/' between its folders.
    let excluded: Vec<String> = exclude
        .iter()
        .map(|x| x.replace('\\', "/").trim_start_matches(['.', '/']).to_ascii_lowercase())
        .filter(|x| !x.is_empty())
        .collect();
    let is_excluded = |relative: &str| {
        let relative = relative.to_ascii_lowercase();
        excluded.iter().any(|x| relative.starts_with(x.as_str()))
    };

    safe_zip::ensure_fits(files.iter().map(|f| f.2), &root).map_err(Failure::Other)?;
    let mut checked = CheckedFolders::new();
    // Where each entry lands - resolved once, so "bin/./x.dll" is an assembly of bin like "bin/x.dll".
    let targets = targets(&root, &files, &mut checked)?;
    let (dlls, rest): (Vec<_>, Vec<_>) = targets.into_iter().partition(|(_, target)| {
        let relative = safe_zip::relative(&root, target);
        let folder = relative.rsplit_once('/').map(|(f, _)| f).unwrap_or("");
        folder.eq_ignore_ascii_case("bin") && relative.to_ascii_lowercase().ends_with(".dll")
    });

    // The assemblies, and the binding redirects that make the site use them.
    let web_config = root.join("web.config");
    let text =
        std::fs::read_to_string(&web_config).map_err(|e| Failure::Other(format!("{}: {e}", web_config.display())))?;
    let mut config = SiteXml::parse(&text).map_err(Failure::Other)?;
    let mut redirects = 0;
    for (index, target) in &dlls {
        cancel.check()?;
        safe_zip::write_entry(&root, &mut zip, *index, target, &mut checked)?;
        if let Some((name, token, version)) = strong_name(target)
            && set_binding_redirect(&mut config, &name, &token, &version).map_err(Failure::Other)?
        {
            redirects += 1;
        }
    }
    if redirects > 0 {
        safe_zip::ensure_no_link(&root, &web_config, None).map_err(Failure::Other)?;
        let text = config.to_text().map_err(Failure::Other)?;
        std::fs::write(&web_config, text).map_err(|e| Failure::Other(format!("{}: {e}", web_config.display())))?;
    }
    reporter.info(&format!(
        "{} assemblies in bin, {} binding redirects in web.config set to them.",
        n0(dlls.len() as i64),
        n0(redirects)
    ));

    // Everything else, as DNN unzips it.
    let rest: Vec<_> =
        rest.into_iter().filter(|(_, target)| !is_excluded(&safe_zip::relative(&root, target))).collect();
    let mut pacer = Pacer::new();
    for (count, (index, target)) in rest.iter().enumerate() {
        cancel.check()?;
        safe_zip::write_entry(&root, &mut zip, *index, target, &mut checked)?;
        pacer.tick(progress, count + 1, rest.len());
    }
    Ok((dlls.len(), rest.len()))
}

/// The package's `App_Data/Upgrade/upgrade.json`: what it excludes - with the site's web.config always on it, its
/// machineKey and connection are the site's - and the DNN it needs at least.
fn upgrade_info(zip: &mut Archive, files: &[(usize, String, u64)]) -> Result<(Vec<String>, Option<String>), Failure> {
    let mut exclude: Vec<String> = DEFAULT_UPGRADE_EXCLUDE.iter().map(|s| s.to_string()).collect();
    let Some((index, _, size)) = files.iter().find(|f| f.1.eq_ignore_ascii_case("App_Data/Upgrade/upgrade.json"))
    else {
        return Ok((exclude, None));
    };
    let mut text = String::new();
    zip.by_index(*index)
        .map_err(|e| Failure::Damaged(e.to_string()))?
        .take((*size).min(16 * 1024 * 1024))
        .read_to_string(&mut text)
        .map_err(|e| Failure::Other(e.to_string()))?;
    let json: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| Failure::Other(e.to_string()))?;
    if let Some(list) = json.get("upgradeExclude").and_then(|l| l.as_array()) {
        let mut seen = HashSet::new();
        exclude = list
            .iter()
            .filter_map(|e| e.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .chain(Some("web.config".to_string()))
            .filter(|s| seen.insert(s.to_ascii_lowercase()))
            .collect();
    }
    let minimum = json.get("minimumDnnVersion").and_then(|m| m.as_str()).map(str::to_string);
    Ok((exclude, minimum))
}

fn has_identity(config: &SiteXml, dependent: Node, name: &str, token: &str) -> bool {
    config.element(dependent, "assemblyIdentity", ASSEMBLY_BINDING).is_some_and(|identity| {
        config.attribute(identity, "name").is_some_and(|n| n.eq_ignore_ascii_case(name))
            && config.attribute(identity, "publicKeyToken").is_some_and(|t| t.eq_ignore_ascii_case(token))
    })
}

/// web.config's binding redirect for an assembly, as DNN's BindingRedirect.config merge sets it: in every
/// `dependentAssembly` with its name and token (case doesn't matter), in whichever `assemblyBinding` it is, the binding
/// redirect set - or added when it has none - to redirect every version to `version`; what else it holds (a `codeBase`,
/// its `culture`) stays. A new `dependentAssembly` only when there is none. True when it changed.
pub fn set_binding_redirect(config: &mut SiteXml, name: &str, token: &str, version: &Version) -> Result<bool, String> {
    let configuration = config.root().ok_or("web.config is empty.")?;
    let runtime = match config.element(configuration, "runtime", "") {
        Some(runtime) => runtime,
        None => config.append_element(configuration, "runtime", "")?,
    };
    let new_version = version.to_string();
    let matching: Vec<Node> = config
        .elements(runtime, "assemblyBinding", ASSEMBLY_BINDING)
        .into_iter()
        .flat_map(|b| config.elements(b, "dependentAssembly", ASSEMBLY_BINDING))
        .filter(|d| has_identity(config, *d, name, token))
        .collect();
    if matching.is_empty() {
        let binding = match config.element(runtime, "assemblyBinding", ASSEMBLY_BINDING) {
            Some(binding) => binding,
            None => {
                let binding = config.append_element(runtime, "assemblyBinding", ASSEMBLY_BINDING)?;
                let ns = config.xot.add_namespace(ASSEMBLY_BINDING);
                let prefix = config.xot.empty_prefix();
                config.xot.set_namespace(binding, prefix, ns);
                binding
            }
        };
        let dependent = config.append_element(binding, "dependentAssembly", ASSEMBLY_BINDING)?;
        let identity = config.append_element(dependent, "assemblyIdentity", ASSEMBLY_BINDING)?;
        config.set_attribute(identity, "name", name);
        config.set_attribute(identity, "publicKeyToken", token);
        let redirect = config.append_element(dependent, "bindingRedirect", ASSEMBLY_BINDING)?;
        config.set_attribute(redirect, "oldVersion", REDIRECT_FROM);
        config.set_attribute(redirect, "newVersion", &new_version);
        return Ok(true);
    }
    let mut changed = false;
    for dependent in matching {
        if let Some(redirect) = config.element(dependent, "bindingRedirect", ASSEMBLY_BINDING) {
            if config.attribute(redirect, "newVersion") == Some(new_version.as_str())
                && config.attribute(redirect, "oldVersion") == Some(REDIRECT_FROM)
            {
                continue;
            }
            config.set_attribute(redirect, "oldVersion", REDIRECT_FROM);
            config.set_attribute(redirect, "newVersion", &new_version);
        } else {
            // Right after its identity, where a redirect goes.
            let added = config.new_element("bindingRedirect", ASSEMBLY_BINDING);
            config.set_attribute(added, "oldVersion", REDIRECT_FROM);
            config.set_attribute(added, "newVersion", &new_version);
            match config.element(dependent, "assemblyIdentity", ASSEMBLY_BINDING) {
                Some(identity) => config.xot.insert_after(identity, added),
                None => config.xot.append(dependent, added),
            }
            .map_err(|e| e.to_string())?;
        }
        changed = true;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_zip::tests::zip;

    #[derive(Default)]
    struct Lines(Vec<String>);

    impl Reporter for Lines {
        fn step(&mut self, t: &str) {
            self.0.push(format!("## {t}"));
        }
        fn info(&mut self, t: &str) {
            self.0.push(t.into());
        }
        fn success(&mut self, t: &str) {
            self.0.push(format!("ok {t}"));
        }
        fn warn(&mut self, t: &str) {
            self.0.push(format!("warn {t}"));
        }
        fn fail(&mut self, t: &str) {
            self.0.push(format!("! {t}"));
        }
    }

    fn config(bindings: &str) -> SiteXml {
        SiteXml::parse(&format!(
            "<configuration><appSettings /><runtime><assemblyBinding xmlns=\"{ASSEMBLY_BINDING}\">{bindings}</assemblyBinding></runtime></configuration>"
        ))
        .unwrap()
    }

    fn redirects(config: &SiteXml) -> Vec<(String, String)> {
        let text = config.to_text().unwrap();
        let doc = roxmltree::Document::parse(&text).unwrap();
        doc.descendants()
            .filter(|n| n.has_tag_name((ASSEMBLY_BINDING, "bindingRedirect")))
            .map(|n| (n.attribute("oldVersion").unwrap().into(), n.attribute("newVersion").unwrap().into()))
            .collect()
    }

    #[test]
    fn a_binding_redirect_is_pointed_at_the_new_version_as_dnns_merge_does() {
        let mut c = config(
            "<dependentAssembly><assemblyIdentity name=\"AngleSharp\" publicKeyToken=\"E83494DCDC6D31EA\" culture=\"neutral\" />\
             <bindingRedirect oldVersion=\"0.0.0.0-0.17.1.0\" newVersion=\"0.17.1.0\" /></dependentAssembly>",
        );
        let v = Version::new4(1, 3, 0, 0);
        assert!(set_binding_redirect(&mut c, "AngleSharp", "e83494dcdc6d31ea", &v).unwrap());
        assert_eq!(redirects(&c), [(REDIRECT_FROM.to_string(), "1.3.0.0".to_string())]);
        // Already so: nothing to change.
        assert!(!set_binding_redirect(&mut c, "AngleSharp", "e83494dcdc6d31ea", &v).unwrap());
    }

    #[test]
    fn a_missing_binding_redirect_is_added_and_others_are_left_alone() {
        let mut c = config(
            "<dependentAssembly><assemblyIdentity name=\"Newtonsoft.Json\" publicKeyToken=\"30ad4fe6b2a6aeed\" />\
             <bindingRedirect oldVersion=\"0.0.0.0-32767.32767.32767.32767\" newVersion=\"13.0.0.0\" /></dependentAssembly>",
        );
        assert!(set_binding_redirect(&mut c, "MailKit", "4e064fe7c44a8f1b", &Version::new4(4, 8, 0, 0)).unwrap());
        let text = c.to_text().unwrap();
        let doc = roxmltree::Document::parse(&text).unwrap();
        let names: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name((ASSEMBLY_BINDING, "assemblyIdentity")))
            .map(|n| n.attribute("name").unwrap())
            .collect();
        assert_eq!(names, ["Newtonsoft.Json", "MailKit"]);
        assert_eq!(redirects(&c)[0].1, "13.0.0.0");
    }

    #[test]
    fn a_codebase_and_the_culture_stay_when_the_redirect_is_set() {
        let mut c = config(
            "<dependentAssembly><assemblyIdentity name=\"Imageflow.Net\" publicKeyToken=\"b4b1e5a3b9d5e5a1\" culture=\"neutral\" />\
             <codeBase version=\"0.10.0.0\" href=\"bin/Imageflow/0.10/Imageflow.Net.dll\" /></dependentAssembly>",
        );
        assert!(
            set_binding_redirect(&mut c, "Imageflow.Net", "b4b1e5a3b9d5e5a1", &Version::new4(0, 13, 0, 0)).unwrap()
        );
        let text = c.to_text().unwrap();
        assert!(text.contains("culture=\"neutral\""), "{text}");
        assert!(text.contains("<codeBase"), "The codeBase went.");
        // Right after the identity.
        assert!(text.find("<bindingRedirect").unwrap() < text.find("<codeBase").unwrap(), "{text}");
        assert_eq!(redirects(&c)[0].1, "0.13.0.0");
    }

    #[test]
    fn a_redirect_in_a_later_assembly_binding_is_set_there_not_added_again() {
        let mut c = SiteXml::parse(&format!(
            "<configuration><runtime><assemblyBinding xmlns=\"{ASSEMBLY_BINDING}\" /><assemblyBinding xmlns=\"{ASSEMBLY_BINDING}\">\
             <dependentAssembly><assemblyIdentity name=\"MailKit\" publicKeyToken=\"4e064fe7c44a8f1b\" />\
             <bindingRedirect oldVersion=\"0.0.0.0-4.0.0.0\" newVersion=\"4.0.0.0\" /></dependentAssembly></assemblyBinding></runtime></configuration>"
        ))
        .unwrap();
        assert!(set_binding_redirect(&mut c, "MailKit", "4e064fe7c44a8f1b", &Version::new4(4, 8, 0, 0)).unwrap());
        assert_eq!(redirects(&c), [(REDIRECT_FROM.to_string(), "4.8.0.0".to_string())]);
    }

    #[test]
    fn a_web_config_without_runtime_gets_one() {
        let mut c = SiteXml::parse(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<configuration>\r\n  <appSettings />\r\n</configuration>",
        )
        .unwrap();
        assert!(set_binding_redirect(&mut c, "Dnn.Sample", "0123456789abcdef", &Version::new4(1, 0, 0, 0)).unwrap());
        let text = c.to_text().unwrap();
        assert!(
            text.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<configuration>\r\n  <appSettings"),
            "{text}"
        );
        let doc = roxmltree::Document::parse(&text).unwrap();
        let runtime = doc.root_element().children().find(|n| n.has_tag_name("runtime")).unwrap();
        let binding = runtime.children().find(|n| n.has_tag_name((ASSEMBLY_BINDING, "assemblyBinding"))).unwrap();
        assert_eq!(binding.children().filter(|n| n.has_tag_name((ASSEMBLY_BINDING, "dependentAssembly"))).count(), 1);
    }

    fn installer(data: &Path) -> PackageInstaller {
        PackageInstaller::new(PackageOptions {
            keep_packages: true,
            packages_directory: data.join("packages"),
            private_temp: data.join("temp"),
        })
    }

    fn release(version: &str) -> DnnRelease {
        DnnRelease {
            version: version.into(),
            tag_name: format!("v{version}"),
            download_url: format!(
                "https://github.com/dnnsoftware/Dnn.Platform/releases/download/v{version}/DNN_Platform_{version}_Install.zip"
            ),
            prerelease: false,
            sha256: None,
            upgrade_sha256: None,
        }
    }

    #[test]
    fn a_package_with_a_path_outside_the_site_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join("bin")).unwrap();
        std::fs::write(site.join("web.config"), "<configuration>the site's own</configuration>").unwrap();
        std::fs::write(site.join("bin").join("DotNetNuke.dll"), "old").unwrap();
        let data = dir.path().join("data");
        let kept = data.join("packages").join("dnnsoftware.Dnn.Platform").join("DNN_Platform_10.3.3_Upgrade.zip");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::rename(zip(dir.path(), &[("bin/DotNetNuke.dll", "new"), ("../outside.txt", "never")]), &kept).unwrap();

        let error = installer(&data)
            .extract_upgrade(&release("10.3.3"), &site, &mut Lines::default(), &mut |_| {}, &CancelToken::new())
            .unwrap_err();
        assert!(error.contains("unsafe path"), "{error}");
        assert!(!dir.path().join("outside.txt").exists(), "A file landed outside the site's folder.");
        assert_eq!(std::fs::read_to_string(site.join("bin").join("DotNetNuke.dll")).unwrap(), "old");
    }

    #[test]
    fn an_upgrade_package_goes_in_without_web_config_and_a_kept_install_package_unpacks_whole() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("web.config"), "the site's").unwrap();
        let data = dir.path().join("data");
        let folder = data.join("packages").join("dnnsoftware.Dnn.Platform");
        std::fs::create_dir_all(&folder).unwrap();
        let entries = [("./web.config", "the package's"), ("Install/Install.aspx", "x")];
        std::fs::rename(zip(dir.path(), &entries), folder.join("DNN_Platform_10.3.3_Upgrade.zip")).unwrap();
        let installer = installer(&data);
        let mut lines = Lines::default();
        installer.extract_upgrade(&release("10.3.3"), &site, &mut lines, &mut |_| {}, &CancelToken::new()).unwrap();
        assert_eq!(std::fs::read_to_string(site.join("web.config")).unwrap(), "the site's");
        assert!(site.join("Install").join("Install.aspx").exists());
        assert!(lines.0.iter().any(|l| l.starts_with("Using the kept DNN 10.3.3's upgrade package")), "{:?}", lines.0);
        assert!(lines.0.iter().any(|l| l == "ok 1 files of DNN 10.3.3 are in."), "{:?}", lines.0);

        assert!(!installer.is_kept(&release("10.3.3")));
        std::fs::rename(zip(dir.path(), &[("web.config", "new site")]), folder.join("DNN_Platform_10.3.3_Install.zip"))
            .unwrap();
        assert!(installer.is_kept(&release("10.3.3")));
        let new_site = dir.path().join("new");
        installer
            .download_and_extract(&release("10.3.3"), &new_site, &mut lines, &mut |_| {}, &CancelToken::new())
            .unwrap();
        assert_eq!(std::fs::read_to_string(new_site.join("web.config")).unwrap(), "new site");
    }

    #[test]
    fn a_kept_package_that_isnt_githubs_file_is_deleted_and_a_damaged_one_too() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let folder = data.join("packages").join("dnnsoftware.Dnn.Platform");
        std::fs::create_dir_all(&folder).unwrap();
        let kept = folder.join("DNN_Platform_10.3.3_Install.zip");
        std::fs::write(&kept, "changed").unwrap();
        let mut r = release("10.3.3");
        r.sha256 = Some("a".repeat(64));
        let error = installer(&data)
            .download_and_extract(&r, &dir.path().join("site"), &mut Lines::default(), &mut |_| {}, &CancelToken::new())
            .unwrap_err();
        assert!(error.contains("isn't the file GitHub lists for the release any more"), "{error}");
        assert!(!kept.exists());

        std::fs::write(&kept, "not a zip").unwrap();
        let error = installer(&data)
            .download_and_extract(
                &release("10.3.3"),
                &dir.path().join("site"),
                &mut Lines::default(),
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert!(error.contains("is damaged and has been deleted"), "{error}");
        assert!(!kept.exists());
    }

    #[test]
    fn dnns_local_upgrade_leaves_out_what_is_excluded_however_the_zip_spells_it() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join("App_Data")).unwrap();
        let config = "<configuration><runtime /></configuration>";
        std::fs::write(site.join("web.config"), config).unwrap();
        std::fs::write(site.join("App_Data").join("Database.mdf"), "the site's data").unwrap();
        let package = zip(
            dir.path(),
            &[
                ("./web.config", "<configuration>the package's</configuration>"),
                ("x/../web.config", "<configuration>the package's</configuration>"),
                ("./App_Data/Database.mdf", "empty"),
                ("Resources/new.js", "new"),
            ],
        );
        let (_, files) =
            local_upgrade_over(&package, &site, &mut Lines::default(), &mut |_| {}, &CancelToken::new()).ok().unwrap();
        assert_eq!(std::fs::read_to_string(site.join("web.config")).unwrap(), config, "web.config was overwritten.");
        assert_eq!(std::fs::read_to_string(site.join("App_Data").join("Database.mdf")).unwrap(), "the site's data");
        assert_eq!(std::fs::read_to_string(site.join("Resources").join("new.js")).unwrap(), "new");
        assert_eq!(files, 1);
    }

    #[test]
    fn upgrade_json_excludes_its_own_list_and_web_config() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("web.config"), "<configuration />").unwrap();
        let package = zip(
            dir.path(),
            &[
                ("App_Data/Upgrade/upgrade.json", r#"{ "upgradeExclude": ["robots.txt"] }"#),
                ("robots.txt", "package"),
                ("favicon.ico", "package"),
                ("web.config", "package"),
            ],
        );
        local_upgrade_over(&package, &site, &mut Lines::default(), &mut |_| {}, &CancelToken::new()).ok().unwrap();
        assert!(!site.join("robots.txt").exists());
        assert!(site.join("favicon.ico").exists(), "Only the package's own list is left out.");
        assert_eq!(std::fs::read_to_string(site.join("web.config")).unwrap(), "<configuration />");
    }

    #[test]
    fn a_download_sent_on_to_an_address_that_isnt_https_is_said_so() {
        assert_eq!(redirect_not_https("http://evil.example/a.zip").as_deref(), Some("http://evil.example/a.zip"));
        assert_eq!(redirect_not_https("https://objects.githubusercontent.com/a.zip"), None);
    }

    #[test]
    fn a_package_not_from_https_isnt_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = release("10.3.3");
        r.download_url = "http://evil.example/DNN_Platform_10.3.3_Install.zip".into();
        let error = installer(dir.path())
            .download_and_extract(&r, &dir.path().join("site"), &mut Lines::default(), &mut |_| {}, &CancelToken::new())
            .unwrap_err();
        assert_eq!(
            error,
            "DNN 10.3.3 isn't downloaded from http://evil.example/DNN_Platform_10.3.3_Install.zip: it isn't https - only https addresses are."
        );
    }

    #[test]
    fn kept_packages_are_per_repository() {
        let installer = installer(Path::new(r"C:\data"));
        assert_eq!(
            installer.kept_path(&release("10.3.3").download_url, "x.zip"),
            Path::new(r"C:\data\packages\dnnsoftware.Dnn.Platform\DNN_Platform_10.3.3_Install.zip")
        );
        assert_eq!(installer.kept_path("not a url", "x.zip"), Path::new(r"C:\data\packages\other\x.zip"));
    }

    #[test]
    #[ignore = "needs the internet: downloads DNN 10.3.3's upgrade package from GitHub (about 40 MB)"]
    fn downloads_and_checks_a_real_package() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("web.config"), "x").unwrap();
        let mut options = installer(dir.path()).options;
        options.keep_packages = false;
        std::fs::create_dir_all(&options.private_temp).unwrap();
        PackageInstaller::new(options)
            .extract_upgrade(&release("10.3.3"), &site, &mut Lines::default(), &mut |_| {}, &CancelToken::new())
            .unwrap();
        assert!(site.join("bin").join("DotNetNuke.dll").exists());
    }
}
