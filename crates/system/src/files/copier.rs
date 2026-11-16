//! Copying, zipping and unzipping a site's files (`Files/ProjectFileCopier.cs`): never through a link or junction,
//! never outside the target, a zip written beside its place and moved in, progress at most four times a second.
//!
//! Deviation: the C# `IProgressReporter.Progress` (the status line) has no counterpart on the port's `Reporter`, so
//! the progress lines go to a `progress` callback of their own.

use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use dnnm_application::operation::{CANCELLED, CancelToken, Reporter};
use dnnm_application::project::view::group_thousands;

use super::safe_path::{self, CheckedFolders, LINK_HINT};
use super::safe_zip;

/// A sharing or lock violation, or a file a process has mapped (`FileInUse.Is`): what may pass once the process that
/// holds it lets go - not access denied, a full disk or a damaged file.
pub fn is_file_in_use(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32 | 33 | 1224))
}

/// Why an extraction stopped. `InUse` is the C#'s rethrown exception: the caller may end what holds the file and try
/// again (Restore backup does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    InUse(String),
    Failed(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

/// Rate-limits progress lines to four a second: more often only costs, and nobody reads it faster.
pub struct ProgressThrottle {
    next: Option<Instant>,
}

impl ProgressThrottle {
    const INTERVAL: Duration = Duration::from_millis(250);

    pub fn new() -> Self {
        Self { next: None }
    }

    /// Whether a line is due now - checked before the line is built.
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        if self.next.is_some_and(|next| now < next) {
            return false;
        }
        self.next = Some(now + Self::INTERVAL);
        true
    }
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new()
    }
}

fn megabytes(bytes: u64) -> String {
    group_thousands(bytes as f64 / 1024.0 / 1024.0, 1)
}

fn count(n: usize) -> String {
    group_thousands(n as f64, 0)
}

/// Why `dest` can't be copied into - it, or a folder on the way to it, is a link or junction; None when it can.
fn destination_link(dest: &Path) -> Option<String> {
    let full = safe_path::full(dest)?;
    let drive = full.ancestors().last()?.to_path_buf();
    safe_path::has_link(&drive, &full, None).then(|| {
        format!(
            "{} is a link or junction, or reached through one - DNN Manager doesn't copy into those.\r\n{LINK_HINT}",
            dest.display()
        )
    })
}

/// Copies the folder `src` into `dest` - every file, hidden and system ones too; links and junctions in it are named,
/// not followed.
pub fn copy(
    src: &Path,
    dest: &Path,
    report: &mut dyn Reporter,
    progress: &mut (dyn FnMut(&str) + Send),
    cancel: &CancelToken,
) -> Result<(), String> {
    // Not into a link or junction either: with administrator rights the copy would land wherever it points.
    if let Some(link) = destination_link(dest) {
        return Err(link);
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    if src.as_os_str().to_string_lossy().trim().is_empty() {
        return Err("Local source path is empty.".into());
    }
    if !src.is_dir() {
        return Err(format!("Source folder does not exist: {}", src.display()));
    }
    report.info(&format!("Copying files from {}", src.display()));
    // Not through a link: with administrator rights, the copy would read wherever one points into a folder the user
    // can read.
    if safe_path::is_link(src) {
        return Err(format!(
            "{} is a link or junction - DNN Manager copies a site's own folder, not what a link points to.\r\n{LINK_HINT}",
            src.display()
        ));
    }
    if let Some(link) = destination_link(dest) {
        return Err(link);
    }
    let (files, links) = walk(src).map_err(|e| e.to_string())?;
    // Said, not left out silently: the copy lacks what they point to.
    if !links.is_empty() {
        let shown: Vec<&str> = links.iter().take(10).map(String::as_str).collect();
        report.warn(&format!(
            "Not copied: {} link(s) or junction(s) - DNN Manager doesn't copy what they point to with its administrator rights: {}{}\r\nHint: copy what they point to into the new project yourself, or recreate the links there.",
            links.len(),
            shown.join(", "),
            if links.len() > 10 { ", …" } else { "" }
        ));
    }
    let total = files.len();
    // Every destination folder first, once each - then the files, a few at a time: a DNN site is tens of thousands of
    // small files, and copying them one after the other waits on each file's own round trip, not the disk.
    let mut checked = CheckedFolders::new();
    let mut made = HashSet::new();
    for (_, rel, _) in &files {
        let folder = dest.join(rel).parent().map(Path::to_path_buf).unwrap_or_else(|| dest.to_path_buf());
        if made.insert(folder.display().to_string().to_lowercase()) {
            safe_path::ensure_no_link(dest, &folder, Some(&mut checked)).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        }
    }

    let next = AtomicUsize::new(0);
    let copied = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let throttle = Mutex::new((ProgressThrottle::new(), progress));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                loop {
                    if cancel.is_cancelled() || failure.lock().unwrap().is_some() {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((file, rel, size)) = files.get(i) else { return };
                    let target = dest.join(rel);
                    // A file already there that is a link, or has other names, is replaced - not written through.
                    let result = (|| {
                        if safe_path::is_link(&target) || safe_path::is_hard_linked(&target) {
                            std::fs::remove_file(&target)?;
                        }
                        std::fs::copy(file, &target)
                    })();
                    if let Err(e) = result {
                        failure.lock().unwrap().get_or_insert(e.to_string());
                        return;
                    }
                    let done = copied.fetch_add(1, Ordering::Relaxed) + 1;
                    bytes.fetch_add(*size, Ordering::Relaxed);
                    let mut guard = throttle.lock().unwrap();
                    let (throttle, progress) = &mut *guard;
                    if throttle.due() {
                        progress(&format!("{done}/{total}  {rel}"));
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    cancel.check()?;
    let (n, b) = (copied.load(Ordering::Relaxed), bytes.load(Ordering::Relaxed));
    report.success(&format!("Copied {} files ({} MB).", count(n), megabytes(b)));
    report.fact("Files copied", &format!("{} · {} MB", count(n), megabytes(b)));
    Ok(())
}

/// A file found by [`walk`]: its path, its path relative to the folder walked, and its size.
pub type WalkedFile = (PathBuf, String, u64);

/// Every file under `src` (hidden and system ones too) with its path relative to it and its size, and - relative to
/// it - the links and junctions in it, which aren't gone into.
pub fn walk(src: &Path) -> std::io::Result<(Vec<WalkedFile>, Vec<String>)> {
    let mut files = Vec::new();
    let mut links = Vec::new();
    let mut folders = vec![src.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)?;
            let rel = relative_to(src, &path);
            if crate::trusted::is_reparse_point(&meta) {
                links.push(rel);
            } else if meta.is_dir() {
                folders.push(path);
            } else {
                files.push((path, rel, meta.len()));
            }
        }
    }
    Ok((files, links))
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).map(|p| p.display().to_string()).unwrap_or_else(|_| path.display().to_string())
}

/// The site root inside a zip: the folder of its shallowest web.config ("" when there is none, or it is at the top).
fn site_root(names: &[String]) -> Option<String> {
    names
        .iter()
        .filter(|n| safe_zip::file_name(n).eq_ignore_ascii_case("web.config"))
        .min_by_key(|n| n.matches('/').count())
        .map(|n| n[..n.len() - "web.config".len()].to_string())
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.is_char_boundary(prefix.len())
        && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Unpacks `zip_path` into `dest`: the site found by its web.config (what is outside that folder is skipped), every
/// entry strictly inside `dest`, through no link, and the whole checked against the free space first.
pub fn extract_zip(
    zip_path: &Path,
    dest: &Path,
    report: &mut dyn Reporter,
    progress: &mut dyn FnMut(&str),
    cancel: &CancelToken,
) -> Result<(), ExtractError> {
    let failed = |m: String| ExtractError::Failed(m);
    if !zip_path.is_file() {
        return Err(failed(format!("Zip file not found: {}", zip_path.display())));
    }
    let file = File::open(zip_path).map_err(|e| failed(e.to_string()))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|_| failed(format!("Not a valid .zip file: {}", zip_path.display())))?;

    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i).map_err(|e| failed(format!("A file in the zip is damaged: {e}")))?;
        let raw = entry.name().map_err(|e| failed(format!("A file in the zip has an unreadable name: {e}")))?.to_string();
        if !safe_zip::is_folder(&raw) {
            entries.push((i, safe_zip::name(&raw), entry.size()));
        }
    }
    if entries.is_empty() {
        return Err(failed("The zip is empty.".into()));
    }
    let names: Vec<String> = entries.iter().map(|e| e.1.clone()).collect();
    let root = match site_root(&names) {
        None => {
            report.info("No web.config in the zip - extracting it as it is. It may not be a DNN site.");
            String::new()
        }
        Some(root) => {
            if !root.is_empty() {
                report.info(&format!("Site root in the zip: {root}"));
            }
            root
        }
    };
    let in_root: Vec<_> = entries.iter().filter(|e| starts_with_ignore_case(&e.1, &root)).collect();
    let skipped = entries.len() - in_root.len();

    let classify = |e: std::io::Error| {
        if is_file_in_use(&e) {
            ExtractError::InUse(e.to_string())
        } else if e.kind() == std::io::ErrorKind::InvalidData {
            ExtractError::Failed(format!("A file in the zip is damaged: {e}"))
        } else {
            ExtractError::Failed(e.to_string())
        }
    };
    // Too big for the disk (a zip bomb): refused before the first file.
    safe_zip::ensure_fits(in_root.iter().map(|e| e.2).sum(), dest).map_err(classify)?;
    std::fs::create_dir_all(dest).map_err(classify)?;
    let mut checked = CheckedFolders::new();
    let mut throttle = ProgressThrottle::new();
    let (mut n, mut bytes) = (0usize, 0u64);
    for (index, name, size) in &in_root {
        cancel.check().map_err(ExtractError::Failed)?;
        let mut entry = zip.by_index(*index).map_err(|e| failed(format!("A file in the zip is damaged: {e}")))?;
        // Strictly inside the project ("zip slip"), on no other file's stream (':'), and through no link or junction.
        safe_zip::extract_entry(dest, name, &mut entry, *size, &mut checked, Some(&name[root.len()..]))
            .map_err(classify)?;
        n += 1;
        bytes += size;
        if throttle.due() {
            progress(&format!("{n}/{}  {name}", in_root.len()));
        }
    }
    if skipped > 0 {
        report.info(&format!("Skipped {skipped} file(s) outside the site root."));
    }
    report.success(&format!("Extracted {} files ({} MB).", count(n), megabytes(bytes)));
    report.fact("Files extracted", &format!("{} · {} MB", count(n), megabytes(bytes)));
    Ok(())
}

/// Deletes the files in `directory` that came after the backup `zip_path` and aren't in it - leaving out
/// `excluded_paths`. Returns how many were deleted.
pub fn remove_files_not_in_zip(
    zip_path: &Path,
    directory: &Path,
    excluded_paths: &[String],
    report: &mut dyn Reporter,
    cancel: &CancelToken,
) -> Result<usize, String> {
    let invalid = || format!("Not a valid .zip file: {}", zip_path.display());
    let file = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| invalid())?;
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let raw = zip.by_index_raw(i).map_err(|_| invalid())?.name().map_err(|_| invalid())?.to_string();
        if !safe_zip::is_folder(&raw) {
            names.push(safe_zip::name(&raw));
        }
    }
    // The site root in the zip, as `extract_zip` finds it.
    let root = site_root(&names).unwrap_or_default();
    let in_zip: HashSet<String> = names
        .iter()
        .filter(|n| starts_with_ignore_case(n, &root))
        .map(|n| n[root.len()..].replace('/', "\\").to_lowercase())
        .collect();

    // Only what came after the zip: a file the backup skipped (in use while it was made) is older, and stays.
    let made_at = std::fs::metadata(zip_path).and_then(|m| m.modified()).map_err(|e| e.to_string())?;
    let excluded = excluded_set(excluded_paths);
    let mut removed = 0;
    let mut failed = Vec::new();
    for (file, rel) in files_to_zip(directory, &excluded, cancel)? {
        cancel.check()?;
        let created = std::fs::metadata(&file).and_then(|m| m.created()).unwrap_or(SystemTime::UNIX_EPOCH);
        if in_zip.contains(&rel.to_lowercase()) || created <= made_at {
            continue;
        }
        let deleted = std::fs::symlink_metadata(&file)
            .and_then(|m| safe_path::clear_read_only(&file, &m))
            .and_then(|()| std::fs::remove_file(&file));
        match deleted {
            Ok(()) => removed += 1,
            Err(_) => failed.push(rel),
        }
    }
    if !failed.is_empty() {
        report.warn(&format!(
            "Could not delete {} file(s) added since the backup: {}{}",
            failed.len(),
            failed.iter().take(5).cloned().collect::<Vec<_>>().join(", "),
            if failed.len() > 5 { ", …" } else { "" }
        ));
    }
    Ok(removed)
}

fn excluded_set(paths: &[String]) -> HashSet<String> {
    paths.iter().map(|p| p.replace('/', "\\").trim_matches('\\').to_lowercase()).collect()
}

/// Zips the folder `src` into `zip_path`, leaving out `excluded_paths` (relative, e.g. `App_Data\Search`). Written
/// beside the target and moved into place, so a failed or cancelled export leaves no half zip. Files that can't be
/// read (in use) are skipped and named.
pub fn create_zip(
    src: &Path,
    zip_path: &Path,
    excluded_paths: &[String],
    report: &mut dyn Reporter,
    progress: &mut dyn FnMut(&str),
    cancel: &CancelToken,
) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!("Folder not found: {}", src.display()));
    }
    let excluded = excluded_set(excluded_paths);
    let mut tmp = zip_path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let (tmp_full, zip_full) = (safe_path::full(&tmp), safe_path::full(zip_path));
    // Never zip the zip itself when it is being written inside the folder.
    let files: Vec<(PathBuf, String)> = files_to_zip(src, &excluded, cancel)?
        .into_iter()
        .filter(|(f, _)| {
            let f = safe_path::full(f);
            f != tmp_full && f != zip_full
        })
        .collect();

    let result = write_zip(&tmp, &files, progress, cancel).and_then(|outcome| {
        std::fs::rename(&tmp, zip_path).map_err(|e| e.to_string())?;
        Ok(outcome)
    });
    let (n, bytes, skipped) = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if !skipped.is_empty() {
        report.warn(&format!(
            "Skipped {} file(s) that couldn't be read: {}{}",
            skipped.len(),
            skipped.iter().take(5).cloned().collect::<Vec<_>>().join(", "),
            if skipped.len() > 5 { ", …" } else { "" }
        ));
    }
    report.success(&format!("Zipped {} files ({} MB) into {}", count(n), megabytes(bytes), zip_path.display()));
    report.fact("Files zipped", &format!("{} · {} MB", count(n), megabytes(bytes)));
    Ok(())
}

fn write_zip(
    tmp: &Path,
    files: &[(PathBuf, String)],
    progress: &mut dyn FnMut(&str),
    cancel: &CancelToken,
) -> Result<(usize, u64, Vec<String>), String> {
    use std::os::windows::fs::OpenOptionsExt;
    let out = File::create(tmp).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(out);
    let mut throttle = ProgressThrottle::new();
    let (mut n, mut bytes, mut skipped) = (0usize, 0u64, Vec::new());
    for (file, rel) in files {
        if cancel.is_cancelled() {
            return Err(CANCELLED.into());
        }
        // Shared read: the running site keeps some files (logs, caches) open.
        const SHARE_ALL: u32 = 0x7;
        let input = std::fs::OpenOptions::new().read(true).share_mode(SHARE_ALL).open(file);
        let Ok(mut input) = input else {
            skipped.push(rel.clone());
            continue;
        };
        let meta = input.metadata().map_err(|e| e.to_string())?;
        // Fastest: a site's DLLs, images and packages hardly get smaller - more only takes longer.
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1))
            .large_file(meta.len() >= 0xFFFF_FFFF)
            .last_modified_time(zip_time(meta.modified().ok()));
        zip.start_file(rel.replace('\\', "/"), options).map_err(|e| e.to_string())?;
        if std::io::copy(&mut input, &mut zip).is_err() {
            // What was written of it stays in the zip as a damaged entry: the C#'s entry is left the same way.
            skipped.push(rel.clone());
            continue;
        }
        n += 1;
        bytes += meta.len();
        if throttle.due() {
            progress(&format!("{n}/{}  {rel}", files.len()));
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok((n, bytes, skipped))
}

// A zip keeps times from 1980 to 2107 only: one outside is kept at that end, rather than failing the export.
fn zip_time(at: Option<SystemTime>) -> zip::DateTime {
    let first = zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap_or_default();
    let last = zip::DateTime::from_date_and_time(2107, 12, 31, 23, 59, 58).unwrap_or_default();
    let Some((y, mo, d, h, mi, s)) = at.and_then(crate::win::local_parts) else { return first };
    if y < 1980 {
        return first;
    }
    if y > 2107 {
        return last;
    }
    zip::DateTime::from_date_and_time(y, mo as u8, d as u8, h as u8, mi as u8, s as u8).unwrap_or(first)
}

/// Every file under `root` with its path relative to it, leaving out the `excluded` relative paths (lower case). An
/// excluded folder isn't walked at all; links and junctions aren't gone into; unreadable folders are skipped.
fn files_to_zip(
    root: &Path,
    excluded: &HashSet<String>,
    cancel: &CancelToken,
) -> Result<Vec<(PathBuf, String)>, String> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        cancel.check()?;
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut subfolders = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            // A junction would zip its target or loop.
            if crate::trusted::is_reparse_point(&meta) {
                continue;
            }
            let rel = relative_to(root, &path);
            if excluded.contains(&rel.to_lowercase()) {
                continue;
            }
            if meta.is_dir() {
                subfolders.push(path);
            } else {
                found.push((path, rel));
            }
        }
        folders.extend(subfolders);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Lines;

    fn site(dir: &Path) -> PathBuf {
        let src = dir.join("site");
        std::fs::create_dir_all(src.join(r"App_Data\Search")).unwrap();
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("web.config"), "<configuration/>").unwrap();
        std::fs::write(src.join(r"bin\DotNetNuke.dll"), "MZ").unwrap();
        std::fs::write(src.join(r"App_Data\Search\index.dat"), "x").unwrap();
        src
    }

    #[test]
    fn copies_a_folder_and_says_how_much() {
        let dir = tempfile::tempdir().unwrap();
        let src = site(dir.path());
        let dest = dir.path().join("copy");
        let mut lines = Lines(vec![]);
        copy(&src, &dest, &mut lines, &mut |_| {}, &CancelToken::new()).unwrap();
        assert!(dest.join(r"bin\DotNetNuke.dll").is_file());
        assert!(dest.join(r"App_Data\Search\index.dat").is_file());
        assert!(lines.0.iter().any(|l| l.contains("Copied 3 files")), "{:?}", lines.0);
        assert_eq!(
            copy(&dir.path().join("missing"), &dest, &mut lines, &mut |_| {}, &CancelToken::new()).unwrap_err(),
            format!("Source folder does not exist: {}", dir.path().join("missing").display())
        );
    }

    #[test]
    fn zips_without_the_excluded_folder_and_unzips_the_site_root() {
        let dir = tempfile::tempdir().unwrap();
        let src = site(dir.path());
        let zip_path = dir.path().join("backup.zip");
        let mut lines = Lines(vec![]);
        create_zip(&src, &zip_path, &["App_Data/Search".into()], &mut lines, &mut |_| {}, &CancelToken::new()).unwrap();
        assert!(zip_path.is_file());
        assert!(!dir.path().join("backup.zip.tmp").exists());

        let dest = dir.path().join("restored");
        extract_zip(&zip_path, &dest, &mut lines, &mut |_| {}, &CancelToken::new()).unwrap();
        assert!(dest.join("web.config").is_file());
        assert!(dest.join(r"bin\DotNetNuke.dll").is_file());
        assert!(!dest.join(r"App_Data\Search").exists());

        // A file added after the backup goes; one in the zip stays.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(dest.join("new.txt"), "x").unwrap();
        let removed = remove_files_not_in_zip(&zip_path, &dest, &[], &mut lines, &CancelToken::new()).unwrap();
        assert_eq!(removed, 1);
        assert!(!dest.join("new.txt").exists());
        assert!(dest.join("web.config").exists());
    }

    #[test]
    fn a_zip_with_the_site_in_a_folder_extracts_that_folder_and_refuses_a_slip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("x.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            for name in ["wwwroot/web.config", "wwwroot/bin/a.dll", "readme.txt"] {
                zip.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut zip, b"x").unwrap();
            }
            zip.finish().unwrap();
        }
        let dest = dir.path().join("site");
        let mut lines = Lines(vec![]);
        extract_zip(&zip_path, &dest, &mut lines, &mut |_| {}, &CancelToken::new()).unwrap();
        assert!(dest.join(r"bin\a.dll").is_file());
        assert!(!dest.join("readme.txt").exists());
        assert!(lines.0.iter().any(|l| l.contains("Site root in the zip: wwwroot/")));
        assert!(lines.0.iter().any(|l| l.contains("Skipped 1 file(s) outside the site root.")));

        let evil = dir.path().join("evil.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&evil).unwrap());
            zip.start_file("../evil.dll", zip::write::SimpleFileOptions::default()).unwrap();
            zip.finish().unwrap();
        }
        let err = extract_zip(&evil, &dir.path().join("other"), &mut lines, &mut |_| {}, &CancelToken::new());
        assert_eq!(err, Err(ExtractError::Failed("The zip contains an unsafe path: ../evil.dll".into())));
        assert!(!dir.path().join("evil.dll").exists());

        std::fs::write(dir.path().join("bad.zip"), "not a zip").unwrap();
        let err = extract_zip(&dir.path().join("bad.zip"), &dest, &mut lines, &mut |_| {}, &CancelToken::new());
        assert!(matches!(err, Err(ExtractError::Failed(m)) if m.starts_with("Not a valid .zip file")));
    }

    #[test]
    fn the_throttle_lets_one_line_through_per_quarter_second() {
        let mut throttle = ProgressThrottle::new();
        assert!(throttle.due());
        assert!(!throttle.due());
    }

    #[test]
    fn zip_times_are_kept_inside_what_a_zip_can_hold() {
        assert_eq!(zip_time(Some(SystemTime::UNIX_EPOCH)).year(), 1980);
        assert_eq!(zip_time(None).year(), 1980);
    }
}
