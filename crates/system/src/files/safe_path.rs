//! Paths in folders others can write to - a site's folder, the projects folder, Documents - for what DNN Manager does
//! there with its administrator rights (`DnnManager.Application/SafePath.cs`). A link or junction on the way would take
//! a delete, write, copy or permission change to wherever it points.
//!
//! Deviation: `dnnm_infrastructure::web_config::{is_inside, has_link}` are simpler checks (no root, nothing cached); the
//! zip, copy and temp-folder code here needs the C#'s exact rules - the root counts, a path outside counts as a link,
//! folders found clear are remembered - so `SafePath` is ported whole here.

use std::collections::HashSet;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle, GetLongPathNameW};
use windows::core::PCWSTR;

use crate::win::wide_path;

/// The line an error about a link or junction ends with: what to do about it.
pub const LINK_HINT: &str = "Hint: if the link is meant to be there, recreate it from an administrator command prompt (mklink /J) - or copy the files in instead of linking them.";

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Folders already found to be no link - for an extraction that asks for thousands of files in the same few folders.
/// Lower-cased full paths.
#[derive(Debug, Default, Clone)]
pub struct CheckedFolders(HashSet<String>);

impl CheckedFolders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, folder: &Path) -> bool {
        self.0.contains(&key(folder))
    }

    fn add(&mut self, folder: &Path) {
        self.0.insert(key(folder));
    }
}

fn key(path: &Path) -> String {
    path.display().to_string().to_lowercase()
}

/// A full path without its trailing separator (a drive keeps its own: `C:\`); None when it isn't valid.
pub fn full(path: &Path) -> Option<PathBuf> {
    let text = path.to_str()?;
    if text.is_empty() || text.contains('\0') {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    let text = absolute.to_str()?.to_string();
    let trimmed = text.trim_end_matches(['\\', '/']);
    // A drive ("C:\") keeps its separator.
    Some(PathBuf::from(if trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    }))
}

fn same(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

fn same_or_inside(path: &Path, folder: &Path) -> bool {
    if same(path, folder) {
        return true;
    }
    let folder = folder.display().to_string();
    // A drive ("C:\") already ends with its separator.
    let prefix = if folder.ends_with('\\') { folder } else { format!("{folder}\\") };
    let path = path.display().to_string();
    path.len() >= prefix.len() && path[..prefix.len()].eq_ignore_ascii_case(&prefix)
}

/// Whether `path` is `folder` itself or inside it - both made full first. False when either isn't a valid path.
pub fn is_same_or_inside(path: &Path, folder: &Path) -> bool {
    matches!((full(path), full(folder)), (Some(p), Some(f)) if same_or_inside(&p, &f))
}

/// Whether `path` is inside `folder` - not the folder itself.
pub fn is_inside(path: &Path, folder: &Path) -> bool {
    matches!((full(path), full(folder)), (Some(p), Some(f)) if same_or_inside(&p, &f) && !same(&p, &f))
}

/// Whether `path` is there, and whether it is a link - one read of its attributes. One that can't be read isn't
/// trusted either.
fn state(path: &Path) -> (bool, bool) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => (true, meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => (false, false),
        // ERROR_PATH_NOT_FOUND / ERROR_INVALID_NAME: not there either.
        Err(e) if matches!(e.raw_os_error(), Some(3 | 123)) => (false, false),
        Err(_) => (true, true),
    }
}

/// Whether `path` - or a folder on the way to it from `root`, the root included - is a link or junction. What doesn't
/// exist yet isn't one; a path outside the root counts as one (never trusted). `checked` remembers the folders already
/// found to be no link.
pub fn has_link(root: &Path, path: &Path, checked: Option<&mut CheckedFolders>) -> bool {
    let (Some(top), Some(mut at)) = (full(root), full(path)) else { return true };
    if !same_or_inside(&at, &top) {
        return true;
    }
    let mut walked = Vec::new();
    loop {
        if checked.as_ref().is_some_and(|c| c.contains(&at)) {
            break;
        }
        let (exists, link) = state(&at);
        if link {
            return true;
        }
        // Only a folder that is there is remembered: one made later could be made as a junction.
        if exists {
            walked.push(at.clone());
        }
        if same(&at, &top) {
            break;
        }
        at = at.parent().map_or_else(|| top.clone(), Path::to_path_buf);
    }
    // Remembered only once the whole way is known to be clear.
    if let Some(checked) = checked {
        walked.iter().for_each(|f| checked.add(f));
    }
    false
}

/// `relative` (a zip entry's name, say) under `root` - or None when it would land outside it ("..", a full path, a
/// drive, a stream) or on the root itself, or go through a link or junction. What comes back is strictly inside.
pub fn under(root: &Path, relative: &str, checked: Option<&mut CheckedFolders>) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains(':') || relative.contains('\0') || is_rooted(relative) {
        return None;
    }
    let top = full(root)?;
    let target = full(&top.join(relative.replace('/', "\\")))?;
    if !same_or_inside(&target, &top) || same(&target, &top) {
        return None;
    }
    (!has_link(&top, &target, checked)).then_some(target)
}

// `Path.IsPathRooted`: "\x", "/x" or a drive.
fn is_rooted(path: &str) -> bool {
    path.starts_with(['\\', '/']) || path.as_bytes().get(1) == Some(&b':')
}

/// An error when [`has_link`] - for a folder about to be emptied, written to or deleted.
pub fn ensure_no_link(root: &Path, path: &Path, checked: Option<&mut CheckedFolders>) -> std::io::Result<()> {
    if has_link(root, path, checked) {
        return Err(std::io::Error::other(format!(
            "{} is reached through a link or junction - DNN Manager doesn't delete or write through those, with its administrator rights that could reach a folder outside the site.\r\n{LINK_HINT}",
            path.display()
        )));
    }
    Ok(())
}

/// Whether `path` exists and is a link or junction itself. One that can't be read isn't trusted either.
pub fn is_link(path: &Path) -> bool {
    state(path).1
}

/// Whether the file `path` has other names (hard links) elsewhere - writing into it in place would change that other
/// file too. One that can't be read isn't trusted; one that isn't there has no other name.
pub fn is_hard_linked(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const SHARE_ALL: u32 = 0x7;
    let Ok(file) = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
    else {
        return true;
    };
    use std::os::windows::io::AsRawHandle;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is the open file's; the structure is filled by Windows.
    let ok = unsafe {
        GetFileInformationByHandle(windows::Win32::Foundation::HANDLE(file.as_raw_handle()), &mut info).is_ok()
    };
    !ok || info.nNumberOfLinks > 1
}

/// Deletes the folder `path` with everything in it, never following a link or junction in it - a link is deleted
/// itself, what it points to stays. Read-only files are deleted too.
pub fn delete_tree(path: &Path) -> std::io::Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok(()) };
    if meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Ok(());
    }
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        // Only the link: a junction is removed as a folder, not its target.
        return std::fs::remove_dir(path);
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let meta = std::fs::symlink_metadata(entry.path())?;
        if meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
            delete_tree(&entry.path())?;
        } else {
            clear_read_only(&entry.path(), &meta)?;
            std::fs::remove_file(entry.path())?;
        }
    }
    clear_read_only(path, &meta)?;
    std::fs::remove_dir(path)
}

pub(crate) fn clear_read_only(path: &Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
    if meta.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 {
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// `path` with every part that exists spelled as on disk - a short name (`WEB~1.CON`) as its long one - so two
/// spellings of one file compare equal. The parts that don't exist yet stay as they are.
pub fn long_path(path: &Path) -> PathBuf {
    let Some(full_path) = full(path) else { return path.to_path_buf() };
    let mut existing = full_path.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent() else { return full_path };
        rest.push(existing.file_name().map(|n| n.to_os_string()).unwrap_or_default());
        existing = parent.to_path_buf();
    }
    let name = wide_path(&existing);
    let mut buffer = vec![0u16; 32768];
    // SAFETY: the buffer's length is passed in.
    let length = unsafe { GetLongPathNameW(PCWSTR(name.as_ptr()), Some(&mut buffer)) } as usize;
    let mut resolved = if length > 0 && length < buffer.len() {
        PathBuf::from(String::from_utf16_lossy(&buffer[..length]))
    } else {
        existing
    };
    while let Some(part) = rest.pop() {
        resolved.push(part);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_whole_folders_ignoring_case_and_dots() {
        assert!(is_same_or_inside(Path::new(r"C:\DNN\Shop"), Path::new(r"c:\dnn")));
        assert!(is_same_or_inside(Path::new(r"C:\DNN\a\..\b"), Path::new(r"C:\DNN\")));
        assert!(!is_same_or_inside(Path::new(r"C:\DNN2\x"), Path::new(r"C:\DNN")));
        assert!(is_same_or_inside(Path::new(r"C:\x"), Path::new(r"C:\")));
        assert!(is_inside(Path::new(r"C:\DNN\x"), Path::new(r"C:\DNN")));
        assert!(!is_inside(Path::new(r"C:\DNN"), Path::new(r"C:\DNN\")));
    }

    #[test]
    fn a_name_that_leaves_the_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for bad in ["", "..\\x", "../x", r"C:\x", "\\x", "a:b", "web.config::$DATA", "./", "a/..", "x\0y"] {
            assert_eq!(under(root, bad, None), None, "{bad:?}");
        }
        assert_eq!(under(root, "a/b.txt", None), Some(full(&root.join(r"a\b.txt")).unwrap()));
    }

    #[test]
    fn a_junction_on_the_way_is_a_link_and_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        let target = dir.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let junction = root.join("bin");
        let made = std::process::Command::new(dnnm_infrastructure::process::system32("cmd.exe"))
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success());

        assert!(is_link(&junction));
        assert!(has_link(&root, &junction.join("x.dll"), None));
        assert_eq!(under(&root, "bin/x.dll", None), None);
        assert!(ensure_no_link(&root, &junction, None).unwrap_err().to_string().contains(LINK_HINT));
        assert!(has_link(&root, &dir.path().join("outside"), None), "outside the root: never trusted");

        let mut checked = CheckedFolders::new();
        std::fs::create_dir_all(root.join("App_Data")).unwrap();
        assert!(!has_link(&root, &root.join(r"App_Data\x"), Some(&mut checked)));
        assert!(checked.contains(&full(&root.join("App_Data")).unwrap()));

        // Deleting the site deletes the junction, not what it points to.
        std::fs::write(target.join("keep.txt"), "x").unwrap();
        delete_tree(&root).unwrap();
        assert!(!root.exists());
        assert!(target.join("keep.txt").exists());
    }

    #[test]
    fn a_hard_linked_file_is_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(!is_hard_linked(&file));
        std::fs::hard_link(&file, dir.path().join("b.txt")).unwrap();
        assert!(is_hard_linked(&file));
        assert!(!is_hard_linked(&dir.path().join("missing.txt")));
    }

    #[test]
    fn deletes_read_only_files_and_spells_short_names_long() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        let file = tree.join(r"sub\read-only.txt");
        std::fs::write(&file, "x").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert_eq!(long_path(&tree.join("not-there")).file_name().unwrap(), "not-there");
        delete_tree(&tree).unwrap();
        assert!(!tree.exists());
    }
}
