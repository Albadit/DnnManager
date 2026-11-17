//! Unpacking a zip someone else made - a backup, a DNN package - into a folder others can write to, with DNN Manager's
//! administrator rights (`Files/SafeZip.cs`). Every extraction goes through [`extract_entry`]: an entry lands strictly
//! inside the folder or not at all - never on another file's stream, never through a link or junction, never into a
//! file that has other names - and [`ensure_fits`] refuses a zip that would fill the disk before anything is written.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use dnnm_application::project::view::group_thousands;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::PCWSTR;

use super::safe_path::{self, CheckedFolders, LINK_HINT};
use crate::win::wide_path;

/// What is left free on the disk after unpacking, at least.
pub const HEADROOM: u64 = 512 * 1024 * 1024;

/// An entry's name with '/' between its folders - some zip tools write '\'.
pub fn name(raw: &str) -> String {
    raw.replace('\\', "/")
}

/// A folder entry: its name ends with a separator.
pub fn is_folder(raw: &str) -> bool {
    name(raw).ends_with('/')
}

/// The file name part of an entry's name (`ZipArchiveEntry.Name`).
pub fn file_name(raw: &str) -> String {
    let n = name(raw);
    n.rsplit('/').next().unwrap_or_default().to_string()
}

fn io_error(message: String) -> std::io::Error {
    std::io::Error::other(message)
}

/// Where `relative_name` lands under `root`, spelled as on disk - or an error saying why it may not: outside the
/// folder, the folder itself, a stream (':'), or through a link or junction.
pub fn target(root: &Path, relative_name: &str, checked: Option<&mut CheckedFolders>) -> std::io::Result<PathBuf> {
    if relative_name.contains(':') {
        return Err(io_error(format!(
            "The zip contains an unsafe path: {relative_name} (a ':' would write into another file's stream)."
        )));
    }
    match safe_path::under(root, relative_name, checked) {
        Some(target) => Ok(safe_path::long_path(&target)),
        None if through_link(root, relative_name) => Err(io_error(format!(
            "The folder has a link or junction on the way to {relative_name} - DNN Manager doesn't write through those.\r\n{LINK_HINT}"
        ))),
        None => Err(io_error(format!("The zip contains an unsafe path: {relative_name}"))),
    }
}

// Refused by `under` although it stays inside the folder: a link or junction on the way.
fn through_link(root: &Path, relative_name: &str) -> bool {
    if relative_name.starts_with(['\\', '/']) || relative_name.as_bytes().get(1) == Some(&b':') {
        return false;
    }
    let Some(path) = safe_path::full(&root.join(relative_name.replace('/', "\\"))) else { return false };
    safe_path::is_inside(&path, root) && safe_path::has_link(root, &path, None)
}

/// The path of `target` relative to `root`, with '/' between its folders - to compare with a list of names.
pub fn relative(root: &Path, target: &Path) -> String {
    let root = safe_path::long_path(root);
    let (root_text, target_text) = (root.display().to_string(), target.display().to_string());
    let prefix = format!("{}\\", root_text.trim_end_matches('\\'));
    if target_text.len() >= prefix.len() && target_text[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        target_text[prefix.len()..].replace('\\', "/")
    } else {
        target_text.replace('\\', "/")
    }
}

/// Writes an entry's `content` (of the `size` the zip says) to `target` (from [`target`]): its folder made once it is
/// clear of links, a link in the file's place refused, a file with other names replaced rather than written into, a
/// read-only one overwritten. More data than the zip says is refused.
pub fn write(
    root: &Path,
    content: &mut dyn Read,
    size: u64,
    target: &Path,
    checked: &mut CheckedFolders,
) -> std::io::Result<()> {
    let folder = target.parent().ok_or_else(|| io_error(format!("{} has no folder", target.display())))?;
    if !checked.contains(folder) {
        safe_path::ensure_no_link(root, folder, Some(checked))?;
        std::fs::create_dir_all(folder)?;
        // Made now (only folders that are there are remembered): checked again, so a junction put in its place in
        // between isn't written through.
        safe_path::ensure_no_link(root, folder, Some(checked))?;
    }
    if safe_path::is_link(target) {
        return Err(io_error(format!(
            "{} is a link - DNN Manager doesn't write through those.\r\n{LINK_HINT}",
            target.display()
        )));
    }
    if let Ok(meta) = std::fs::symlink_metadata(target) {
        safe_path::clear_read_only(target, &meta)?;
        // Another name of the same file elsewhere would be overwritten too: this name is taken away, and a new file made.
        if safe_path::is_hard_linked(target) {
            std::fs::remove_file(target)?;
        }
    }
    let mut file = std::fs::File::create(target)?;
    let copied = std::io::copy(&mut content.take(size + 1), &mut file)?;
    if copied > size {
        drop(file);
        let _ = std::fs::remove_file(target);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} holds more than the zip says it does", target.display()),
        ));
    }
    file.flush()
}

/// [`target`] and [`write`] in one: the entry `raw_name` under `root` as `relative_name` (its own name when None).
/// Returns where it was written.
pub fn extract_entry(
    root: &Path,
    raw_name: &str,
    content: &mut dyn Read,
    size: u64,
    checked: &mut CheckedFolders,
    relative_name: Option<&str>,
) -> std::io::Result<PathBuf> {
    let own = name(raw_name);
    let target = target(root, relative_name.unwrap_or(&own), Some(checked))?;
    write(root, content, size, &target, checked)?;
    Ok(target)
}

/// An error when entries of `total` bytes unpacked would leave less than [`HEADROOM`] free on the disk of `root` - a
/// zip bomb, or a package too big for the disk, fails before the first file instead of filling it.
pub fn ensure_fits(total: u64, root: &Path) -> std::io::Result<()> {
    ensure_fits_on(total, free_space(root), root)
}

pub(crate) fn ensure_fits_on(total: u64, free: Option<u64>, root: &Path) -> std::io::Result<()> {
    let Some(available) = free else { return Ok(()) };
    if total as i128 > available as i128 - HEADROOM as i128 {
        const MB: f64 = 1_048_576.0;
        return Err(io_error(format!(
            "The zip unpacks to {} MB, but the disk of {} has {} MB free - it isn't unpacked (at least {} MB are kept free).",
            group_thousands(total as f64 / MB, 0),
            root.display(),
            group_thousands(available as f64 / MB, 0),
            HEADROOM / 1_048_576
        )));
    }
    Ok(())
}

fn free_space(root: &Path) -> Option<u64> {
    let full = safe_path::full(root)?;
    let drive = full.ancestors().last()?.to_path_buf();
    let name = wide_path(&drive);
    let mut available = 0u64;
    // SAFETY: a NUL-terminated path and a value to fill.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(name.as_ptr()), Some(&mut available), None, None) }.ok()?;
    Some(available)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_use_slashes_and_folders_end_with_one() {
        assert_eq!(name(r"bin\x.dll"), "bin/x.dll");
        assert!(is_folder(r"App_Data\"));
        assert!(!is_folder("web.config"));
        assert_eq!(file_name(r"site\web.config"), "web.config");
    }

    #[test]
    fn refuses_unsafe_entries_with_the_c_sharp_messages() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            target(dir.path(), "web.config::$DATA", None).unwrap_err().to_string(),
            "The zip contains an unsafe path: web.config::$DATA (a ':' would write into another file's stream)."
        );
        assert_eq!(
            target(dir.path(), "../evil.dll", None).unwrap_err().to_string(),
            "The zip contains an unsafe path: ../evil.dll"
        );
        let ok = target(dir.path(), "bin/x.dll", None).unwrap();
        assert_eq!(relative(dir.path(), &ok), "bin/x.dll");
    }

    #[test]
    fn writes_an_entry_and_refuses_more_than_it_says() {
        let dir = tempfile::tempdir().unwrap();
        let mut checked = CheckedFolders::new();
        let written = extract_entry(dir.path(), "a/b.txt", &mut &b"hello"[..], 5, &mut checked, None).unwrap();
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "hello");
        let err = extract_entry(dir.path(), "a/c.txt", &mut &b"too long"[..], 3, &mut checked, None).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!dir.path().join(r"a\c.txt").exists());
    }

    #[test]
    fn a_zip_bigger_than_the_disk_is_refused_before_anything_is_written() {
        let root = Path::new(r"C:\DNN\shop");
        assert!(ensure_fits_on(100, Some(HEADROOM + 100), root).is_ok());
        let err = ensure_fits_on(101, Some(HEADROOM + 100), root).unwrap_err().to_string();
        assert!(err.starts_with("The zip unpacks to 0 MB, but the disk of C:\\DNN\\shop has 512 MB free"), "{err}");
        assert!(ensure_fits_on(u64::MAX, None, root).is_ok(), "free space unknown: not refused");
        assert!(free_space(Path::new(r"C:\")).is_some());
    }
}
