//! Unpacking a zip someone else made into a folder others can write to, with administrator rights
//! (`src/DnnManager.Infrastructure/Files/SafeZip.cs`, with the parts of `src/DnnManager.Application/SafePath.cs` it
//! needs): an entry lands strictly inside the folder or not at all - never on another file's stream
//! (`web.config::$DATA`), never through a link or junction, never into a file that has other names (hard links) - and a
//! zip that would fill the disk (a zip bomb) is refused before anything is written.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::format::n0;

/// What is left free on the disk after unpacking, at least.
pub const HEADROOM: u64 = 512 * 1024 * 1024;

/// The line an error about a link or junction ends with (`SafePath.LinkHint`).
pub const LINK_HINT: &str = "Hint: if the link is meant to be there, recreate it from an administrator command prompt (mklink /J) - or copy the files in instead of linking them.";

/// The folders already found to be no link, by their full path in capitals - an extraction asks for thousands of files
/// in the same few folders.
pub type CheckedFolders = HashSet<String>;

fn key(path: &Path) -> String {
    path.to_string_lossy().to_uppercase()
}

/// A full path without `.`, `..` or a trailing separator, each part as Windows reads it - trailing dots and spaces
/// dropped (`Path.GetFullPath`: `web.config.` is `web.config`, `...` is the folder itself). None when it isn't valid.
pub fn full(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\0') {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::ParentDir => {
                // Never above the drive.
                if out.parent().is_some() {
                    out.pop();
                }
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                let trimmed = name.trim_end_matches(['.', ' ']);
                if !trimmed.is_empty() {
                    out.push(trimmed);
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

fn same_or_inside(path: &Path, folder: &Path) -> bool {
    let mut parts = path.components();
    folder
        .components()
        .all(|f| parts.next().is_some_and(|p| key(p.as_os_str().as_ref()) == key(f.as_os_str().as_ref())))
}

fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// Whether `path` is there, and whether it is a link - one read of its attributes. One that can't be read isn't
/// trusted.
fn state(path: &Path) -> (bool, bool) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => (true, is_reparse_point(&meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, false),
        Err(_) => (true, true),
    }
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// `path` exists and is a link or junction itself (`SafePath.IsLink`).
pub fn is_link(path: &Path) -> bool {
    state(path).1
}

/// Whether `path` - or a folder on the way to it from `root`, the root included - is a link or junction. What doesn't
/// exist yet isn't one; a path outside the root counts as one (`SafePath.HasLink`).
pub fn has_link(root: &Path, path: &Path, checked: Option<&mut CheckedFolders>) -> bool {
    let (Some(top), Some(mut at)) = (full(root), full(path)) else { return true };
    if !same_or_inside(&at, &top) {
        return true;
    }
    let mut walked = Vec::new();
    loop {
        if checked.as_ref().is_some_and(|c| c.contains(&key(&at))) {
            break;
        }
        let (exists, link) = state(&at);
        if link {
            return true;
        }
        // Only a folder that is there is remembered: one made later could be made as a junction.
        if exists {
            walked.push(key(&at));
        }
        if same(&at, &top) || !at.pop() {
            break;
        }
    }
    if let Some(checked) = checked {
        checked.extend(walked);
    }
    false
}

/// Err when `has_link` - for a folder about to be written to (`SafePath.EnsureNoLink`).
pub fn ensure_no_link(root: &Path, path: &Path, checked: Option<&mut CheckedFolders>) -> Result<(), String> {
    if has_link(root, path, checked) {
        return Err(format!(
            "{} is reached through a link or junction - DNN Manager doesn't delete or write through those, with its administrator rights that could reach a folder outside the site.\r\n{LINK_HINT}",
            path.display()
        ));
    }
    Ok(())
}

/// `relative` under `root` - None when it would land outside it, on the root itself, or go through a link
/// (`SafePath.Under`).
fn under(root: &Path, relative: &str, checked: Option<&mut CheckedFolders>) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains([':', '\0']) || relative.starts_with(['/', '\\']) {
        return None;
    }
    let top = full(root)?;
    let target = full(&top.join(relative))?;
    if !same_or_inside(&target, &top) || same(&target, &top) {
        return None;
    }
    if has_link(&top, &target, checked) { None } else { Some(target) }
}

/// `path` with every part that exists spelled as on disk - a short name (`WEB~1.CON`) as its long one
/// (`SafePath.LongPath`).
pub fn long_path(path: &Path) -> PathBuf {
    let Some(full) = full(path) else { return path.to_path_buf() };
    let mut existing = full.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|n| n.to_os_string()) else { return full };
        rest.push(name);
        if !existing.pop() {
            return full;
        }
    }
    let mut resolved = long_name(&existing).unwrap_or(existing);
    while let Some(part) = rest.pop() {
        resolved.push(part);
    }
    resolved
}

#[cfg(windows)]
fn long_name(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::Win32::Storage::FileSystem::GetLongPathNameW;
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 32768];
    // SAFETY: a NUL-terminated path in, a buffer of its stated length out.
    let length = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) } as usize;
    (length > 0 && length < buffer.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length])))
}

#[cfg(not(windows))]
fn long_name(_path: &Path) -> Option<PathBuf> {
    None
}

/// Whether the file has other names (hard links) - writing into it would change that other file too
/// (`SafePath.IsHardLinked`). One that can't be read isn't trusted; one that isn't there has none.
#[cfg(windows)]
pub fn is_hard_linked(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };
    use windows::core::PCWSTR;
    if !path.is_file() {
        return false;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the handle is closed before returning.
    unsafe {
        let Ok(handle) = CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            None,
        ) else {
            return true;
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let read = GetFileInformationByHandle(handle, &mut info);
        let _ = CloseHandle(handle);
        read.is_err() || info.nNumberOfLinks > 1
    }
}

#[cfg(not(windows))]
pub fn is_hard_linked(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.nlink() > 1)
}

/// An entry's name with '/' between its folders - some zip tools write '\' (`SafeZip.Name`).
pub fn name(entry_name: &str) -> String {
    entry_name.replace('\\', "/")
}

/// A folder entry: its name ends with a separator (`SafeZip.IsFolder`).
pub fn is_folder(entry_name: &str) -> bool {
    name(entry_name).ends_with('/')
}

/// Where `relative_name` lands under `root`, every part spelled as on disk - or why it may not: outside the folder,
/// the folder itself, a stream (':'), or through a link or junction (`SafeZip.Target`).
pub fn target(root: &Path, relative_name: &str, checked: Option<&mut CheckedFolders>) -> Result<PathBuf, String> {
    if relative_name.contains(':') {
        return Err(format!(
            "The zip contains an unsafe path: {relative_name} (a ':' would write into another file's stream)."
        ));
    }
    match under(root, relative_name, checked) {
        Some(target) => Ok(long_path(&target)),
        None if through_link(root, relative_name) => Err(format!(
            "The folder has a link or junction on the way to {relative_name} - DNN Manager doesn't write through those.\r\n{LINK_HINT}"
        )),
        None => Err(format!("The zip contains an unsafe path: {relative_name}")),
    }
}

// Refused although it stays inside the folder: a link or junction on the way.
fn through_link(root: &Path, relative_name: &str) -> bool {
    if relative_name.starts_with(['/', '\\']) {
        return false;
    }
    match (full(&root.join(relative_name)), full(root)) {
        (Some(path), Some(top)) => same_or_inside(&path, &top) && !same(&path, &top) && has_link(&top, &path, None),
        _ => false,
    }
}

/// The path of `target` relative to `root`, '/' between its folders - to compare with a list of names (DNN's
/// upgradeExclude) after "./", "a/../" and short names are resolved (`SafeZip.Relative`).
pub fn relative(root: &Path, target: &Path) -> String {
    let root = long_path(root);
    let target = full(target).unwrap_or_else(|| target.to_path_buf());
    let skip = root.components().count();
    target.components().skip(skip).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// Writes an entry's content to `target` (from `target`): its folder made once it is clear of links, a link in the
/// file's place refused, a file with other names replaced rather than written into, a read-only one overwritten. The
/// entry may not hold more than its stated `size` (`SafeZip.Write`).
pub fn write(
    root: &Path,
    content: &mut dyn Read,
    size: u64,
    target: &Path,
    checked: &mut CheckedFolders,
) -> Result<(), String> {
    let folder = target.parent().ok_or_else(|| format!("The zip contains an unsafe path: {}", target.display()))?;
    if !checked.contains(&key(&full(folder).unwrap_or_else(|| folder.to_path_buf()))) {
        ensure_no_link(root, folder, Some(checked))?;
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        // Made now: checked again, so a junction put in its place in between isn't written through.
        ensure_no_link(root, folder, Some(checked))?;
    }
    if is_link(target) {
        return Err(format!(
            "{} is a link - DNN Manager doesn't write through those.\r\n{LINK_HINT}",
            target.display()
        ));
    }
    if target.is_file() {
        if let Ok(meta) = std::fs::metadata(target) {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(target, permissions);
        }
        // Another name of the same file elsewhere would be overwritten too: this name is taken away, a new file made.
        if is_hard_linked(target) {
            std::fs::remove_file(target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
    }
    let mut file = std::fs::File::create(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let copied = std::io::copy(&mut content.take(size.saturating_add(1)), &mut file)
        .map_err(|e| format!("{}: {e}", target.display()))?;
    if copied > size {
        drop(file);
        let _ = std::fs::remove_file(target);
        return Err(format!("{}: the zip entry holds more than its size says.", target.display()));
    }
    file.flush().map_err(|e| format!("{}: {e}", target.display()))
}

/// Err when entries of `total` bytes unpacked would leave less than `HEADROOM` free on the disk of `root`
/// (`SafeZip.EnsureFits`). Free space unknown: not refused for it.
pub fn ensure_fits_space(total: u64, free: Option<u64>, root: &Path) -> Result<(), String> {
    let Some(available) = free else { return Ok(()) };
    if total as i128 > available as i128 - HEADROOM as i128 {
        return Err(format!(
            "The zip unpacks to {} MB, but the disk of {} has {} MB free - it isn't unpacked (at least {} MB are kept free).",
            n0((total as f64 / 1_048_576.0).round() as i64),
            root.display(),
            n0((available as f64 / 1_048_576.0).round() as i64),
            HEADROOM / 1_048_576
        ));
    }
    Ok(())
}

/// `EnsureFits` for the sizes the zip states.
pub fn ensure_fits(sizes: impl IntoIterator<Item = u64>, root: &Path) -> Result<(), String> {
    let total = sizes.into_iter().fold(0u64, u64::saturating_add);
    ensure_fits_space(total, free_space(root), root)
}

/// The space free for this user on the drive of `path`; None when it can't be told.
#[cfg(windows)]
pub fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let full = full(path)?;
    let drive = full.components().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir));
    let drive: PathBuf = drive.collect();
    if drive.as_os_str().is_empty() {
        return None;
    }
    let wide: Vec<u16> = drive.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: a NUL-terminated path; one out value.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }.ok()?;
    Some(available)
}

#[cfg(not(windows))]
pub fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// A zip opened for unpacking entry by entry.
pub type Archive = zip::ZipArchive<std::fs::File>;

/// Opens a zip; Err(`Damaged`) when it isn't one - the caller tells a damaged package apart from other failures.
pub fn open(path: &Path) -> Result<Archive, ZipError> {
    let file = std::fs::File::open(path).map_err(|e| ZipError::Other(format!("{}: {e}", path.display())))?;
    zip::ZipArchive::new(file).map_err(|e| ZipError::Damaged(e.to_string()))
}

/// Why a zip couldn't be unpacked: the zip itself is damaged (`InvalidDataException`), or anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipError {
    Damaged(String),
    Other(String),
}

impl From<String> for ZipError {
    fn from(value: String) -> Self {
        ZipError::Other(value)
    }
}

impl ZipError {
    pub fn message(&self) -> &str {
        match self {
            ZipError::Damaged(m) | ZipError::Other(m) => m,
        }
    }
}

/// The names and stated sizes of a zip's file entries (not its folders), in the zip's order.
pub fn files(zip: &mut Archive) -> Result<Vec<(usize, String, u64)>, ZipError> {
    let mut list = Vec::new();
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index).map_err(|e| ZipError::Damaged(e.to_string()))?;
        let entry_name = name(&String::from_utf8_lossy(entry.name_raw()));
        if !is_folder(&entry_name) {
            list.push((index, entry_name, entry.size()));
        }
    }
    Ok(list)
}

/// Writes entry `index` of `zip` to `target` (from `target`) - `write` with the entry's content.
pub fn write_entry(
    root: &Path,
    zip: &mut Archive,
    index: usize,
    target: &Path,
    checked: &mut CheckedFolders,
) -> Result<(), ZipError> {
    let mut entry = zip.by_index(index).map_err(|e| ZipError::Damaged(e.to_string()))?;
    let size = entry.size();
    write(root, &mut entry, size, target, checked).map_err(ZipError::Other)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A zip with these entries, in a temporary folder of its own.
    pub fn zip(dir: &Path, entries: &[(&str, &str)]) -> PathBuf {
        let path = dir.join(format!("test-{}.zip", entries.len()));
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, content) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    #[test]
    fn a_zip_entry_on_another_files_stream_or_outside_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("web.config"), "the site's").unwrap();
        for bad in ["web.config::$DATA", "web.config:evil", "../outside.txt", "./", "a/../", "...", "/abs.txt"] {
            assert!(target(&root, bad, None).is_err(), "'{bad}' was allowed.");
        }
        assert_eq!(relative(&root, &target(&root, "./web.config", None).unwrap()), "web.config");
        assert_eq!(relative(&root, &target(&root, "x/../web.config", None).unwrap()), "web.config");
        assert_eq!(relative(&root, &target(&root, "web.config.", None).unwrap()), "web.config");
        assert!(target(&root, "web.config:evil", None).unwrap_err().contains("another file's stream"));
        assert_eq!(std::fs::read_to_string(root.join("web.config")).unwrap(), "the site's");
    }

    #[test]
    fn a_zip_that_unpacks_to_more_than_the_disk_has_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        ensure_fits_space(10, Some(HEADROOM + 100), dir.path()).unwrap();
        assert!(ensure_fits_space(u64::MAX / 2, Some(10 * 1024 * 1024 * 1024), dir.path()).is_err());
        assert!(ensure_fits_space(100, Some(HEADROOM + 50), dir.path()).is_err());
        ensure_fits_space(u64::MAX, None, dir.path()).unwrap();
        assert!(free_space(dir.path()).is_some_and(|f| f > 0) || cfg!(not(windows)));
    }

    #[test]
    fn writes_an_entry_and_replaces_a_read_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        std::fs::create_dir(&root).unwrap();
        let path = zip(dir.path(), &[("bin/a.dll", "new"), ("folder/", "")]);
        let mut archive = open(&path).unwrap();
        let entries = files(&mut archive).unwrap();
        assert_eq!(entries.len(), 1);
        let mut checked = CheckedFolders::new();
        let to = target(&root, &entries[0].1, Some(&mut checked)).unwrap();
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(&to, "old").unwrap();
        let mut permissions = std::fs::metadata(&to).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&to, permissions).unwrap();
        write_entry(&root, &mut archive, entries[0].0, &to, &mut checked).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("bin").join("a.dll")).unwrap(), "new");
    }

    #[test]
    fn a_damaged_zip_is_said_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.zip");
        std::fs::write(&path, "not a zip").unwrap();
        assert!(matches!(open(&path), Err(ZipError::Damaged(_))));
    }
}
