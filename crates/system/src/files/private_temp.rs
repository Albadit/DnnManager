//! `%ProgramData%\DnnManager\temp` - DNN Manager's own temporary folder, which only Administrators and SYSTEM may
//! change (`Files/PrivateTemp.cs`). DNN Manager runs elevated; what it writes and later reads back or runs (an update, a
//! database export, a task definition) must not be somewhere every program of the signed-in user can swap it - %TEMP%
//! is such a place. A folder of that name that someone else made first, or that is a link elsewhere, isn't used: a new
//! one is made beside it. Not elevated (the tests), there is no such boundary to keep, and the user's %TEMP% is used.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use windows::Win32::UI::Shell::FOLDERID_ProgramData;

use crate::acl::{Ace, Security, current_user_sid, rights, sids};
use crate::trusted::{is_elevated, is_reparse_point};
use crate::win::known_folder;

/// What is left in the temporary folder longer than this is from a run that didn't clean up after itself.
pub const KEPT_FOR: Duration = Duration::from_secs(24 * 60 * 60);

// A failure (the folder couldn't be made just now) isn't kept for the rest of the session - the next use tries again.
static TEMP: Mutex<Option<PathBuf>> = Mutex::new(None);
static TOOLS: Mutex<Option<PathBuf>> = Mutex::new(None);
static DOCKER: Mutex<Option<PathBuf>> = Mutex::new(None);

fn cached(cell: &Mutex<Option<PathBuf>>, make: impl FnOnce() -> std::io::Result<PathBuf>) -> std::io::Result<PathBuf> {
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = &*guard {
        return Ok(path.clone());
    }
    let path = make()?;
    *guard = Some(path.clone());
    Ok(path)
}

/// The folder, made (or checked) on first use.
pub fn path() -> std::io::Result<PathBuf> {
    cached(&TEMP, create)
}

/// Where DNN Manager installs the programs it runs as Administrator: only administrators can change them there.
pub fn tools_path() -> std::io::Result<PathBuf> {
    cached(&TOOLS, || beside_temp("tools", "DnnManager-tools"))
}

/// docker's configuration folder (`DOCKER_CONFIG`) for the docker DNN Manager runs as Administrator: its own, which only
/// administrators can change - not the user's `.docker`.
pub fn docker_config_path() -> std::io::Result<PathBuf> {
    cached(&DOCKER, || beside_temp("docker", "DnnManager-docker"))
}

// Beside the temporary folder, in the same folder only administrators can change - what was made there is theirs.
fn beside_temp(name: &str, unelevated: &str) -> std::io::Result<PathBuf> {
    let temp = path()?;
    let parent = temp.parent().map(Path::to_path_buf).unwrap_or(temp);
    if !is_elevated() {
        let folder = parent.join(unelevated);
        std::fs::create_dir_all(&folder)?;
        return Ok(folder);
    }
    secure_or_beside(&parent, name)
}

fn create() -> std::io::Result<PathBuf> {
    if !is_elevated() {
        let folder = std::env::temp_dir().join("DnnManager");
        std::fs::create_dir_all(&folder)?;
        return Ok(folder);
    }
    let program_data =
        known_folder(&FOLDERID_ProgramData).ok_or_else(|| std::io::Error::other("ProgramData isn't known"))?;
    let parent = secure_or_beside(&program_data, "DnnManager")?;
    let temp = secure_or_beside(&parent, "temp")?;
    // What an earlier run left behind (a crash, a cancelled export) - in the background, the first use doesn't wait.
    let sweep_folder = temp.clone();
    std::thread::spawn(move || sweep(&sweep_folder, SystemTime::now() - KEPT_FOR));
    Ok(temp)
}

/// `name` in `parent`, made or kept to Administrators and SYSTEM - or, when that one can't be trusted, one beside it:
/// an earlier run's `name-…` that is still only administrators', or a new one.
fn secure_or_beside(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    if let Some(own) = secure(&parent.join(name)) {
        return Ok(own);
    }
    if let Ok(entries) = std::fs::read_dir(parent) {
        let prefix = format!("{}-", name.to_lowercase());
        let mut earlier: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with(&prefix))
            .filter(|e| std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir() && !is_reparse_point(&m)))
            .map(|e| e.path())
            .collect();
        earlier.sort_by_key(|p| p.display().to_string().to_lowercase());
        if let Some(reused) = earlier.iter().find_map(|p| secure(p)) {
            return Ok(reused);
        }
    }
    secure(&parent.join(format!("{name}-{}", random_suffix()))).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("Could not make a folder only administrators can change under {}.", parent.display()),
        )
    })
}

fn random_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:08x}", hasher.finish() as u32)
}

/// Deletes what in `folder` was last written before `before` - files, then the folders left empty - never going into
/// a link or junction. Best effort: what is in use stays.
pub fn sweep(folder: &Path, before: SystemTime) {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    collect(folder, &mut files, &mut folders);
    for file in files {
        if std::fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|t| t < before)
            && let Ok(meta) = std::fs::symlink_metadata(&file)
        {
            let _ = super::safe_path::clear_read_only(&file, &meta).and_then(|()| std::fs::remove_file(&file));
        }
    }
    // Deepest first, so a folder emptied of its folders goes too.
    folders.sort_by_key(|f| std::cmp::Reverse(f.as_os_str().len()));
    for sub in folders {
        let old = std::fs::metadata(&sub).and_then(|m| m.modified()).is_ok_and(|t| t < before);
        if old && std::fs::read_dir(&sub).is_ok_and(|mut d| d.next().is_none()) {
            let _ = std::fs::remove_dir(&sub);
        }
    }
}

fn collect(folder: &Path, files: &mut Vec<PathBuf>, folders: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
        if is_reparse_point(&meta) {
            continue;
        }
        if meta.is_dir() {
            folders.push(entry.path());
            collect(&entry.path(), files, folders);
        } else {
            files.push(entry.path());
        }
    }
}

/// The rules of the folder: owned by Administrators, nothing inherited from ProgramData (where every user may create
/// files), Administrators and SYSTEM in full - and whoever Windows makes the owner may only read (OWNER RIGHTS).
pub fn rules() -> Security {
    Security::protected(
        Some(sids::ADMINISTRATORS),
        vec![
            Ace::full_control(sids::ADMINISTRATORS),
            Ace::full_control(sids::SYSTEM),
            Ace::allow(sids::OWNER_RIGHTS, rights::READ_AND_EXECUTE),
        ],
    )
}

/// `path`, made with - or set to - Administrators and SYSTEM only; None when it is there already and isn't theirs (or
/// is a link), so it can't be trusted.
fn secure(path: &Path) -> Option<PathBuf> {
    let me = current_user_sid();
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.is_dir() || is_reparse_point(&meta) {
            return None;
        }
        // Already closed with these rules (an earlier run's): what is in it was put there by an administrator.
        let security = Security::read(path).ok()?;
        let was_closed = made_by_us(&security, me.as_deref());
        if !is_trusted(security.owner.as_deref()) && !was_closed {
            return None;
        }
        rules().apply(path).ok()?;
        // Something put in it before it was closed (it may have let others in then) isn't trusted either.
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            let meta = std::fs::symlink_metadata(entry.path()).ok()?;
            let owner = Security::read(&entry.path()).ok()?.owner;
            if is_reparse_point(&meta) || (!is_trusted(owner.as_deref()) && !(was_closed && owner == me)) {
                return None;
            }
        }
        return Some(path.to_path_buf());
    }
    // Made by another program between the check and here, it keeps theirs as it is: checked again.
    let _ = rules().create_dir(path);
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() || is_reparse_point(&meta) {
        return None;
    }
    let security = Security::read(path).ok()?;
    if is_trusted(security.owner.as_deref()) {
        return Some(path.to_path_buf());
    }
    if !made_by_us(&security, me.as_deref()) {
        return None;
    }
    // Made by us, owned by us (Group Policy): Administrators are made its owner where Windows lets them be.
    let _ = rules().apply(path);
    Some(path.to_path_buf())
}

/// Made by DNN Manager though its owner is the signed-in user (the Group Policy "Default owner for objects created by
/// members of the Administrators group: Object creator"): exactly the rules DNN Manager gives it - nothing inherited,
/// only Administrators and SYSTEM, and OWNER RIGHTS limited to reading.
pub fn made_by_us(security: &Security, me: Option<&str>) -> bool {
    if me.is_none() || security.owner.as_deref() != me || !security.protected {
        return false;
    }
    security.aces.iter().any(|a| a.sid == sids::OWNER_RIGHTS)
        && security.aces.iter().all(|a| {
            a.sid == sids::ADMINISTRATORS
                || a.sid == sids::SYSTEM
                || (a.sid == sids::OWNER_RIGHTS
                    && a.allow
                    && a.file_rights() & !(rights::READ_AND_EXECUTE | rights::SYNCHRONIZE) == 0)
        })
}

/// Owned by Administrators, SYSTEM or TrustedInstaller.
pub fn is_trusted(owner: Option<&str>) -> bool {
    matches!(owner, Some(sids::ADMINISTRATORS | sids::SYSTEM | sids::TRUSTED_INSTALLER))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn a_folder_made_by_us_under_the_creator_owner_policy_is_told_apart() {
        let mut ours = rules();
        ours.owner = Some(ME.into());
        assert!(made_by_us(&ours, Some(ME)));
        assert!(!made_by_us(&ours, Some("S-1-5-21-9")), "someone else's");

        let mut open = ours.clone();
        open.aces.push(Ace::allow(sids::USERS, rights::READ_AND_EXECUTE));
        assert!(!made_by_us(&open, Some(ME)), "lets users in");

        let mut owner_writes = ours.clone();
        owner_writes.aces[2].mask = rights::MODIFY;
        assert!(!made_by_us(&owner_writes, Some(ME)), "the owner may change it");

        let mut inherits = ours;
        inherits.protected = false;
        assert!(!made_by_us(&inherits, Some(ME)));
    }

    #[test]
    fn only_administrators_system_and_trusted_installer_are_trusted_owners() {
        assert!(is_trusted(Some(sids::ADMINISTRATORS)));
        assert!(is_trusted(Some(sids::TRUSTED_INSTALLER)));
        assert!(!is_trusted(Some(ME)));
        assert!(!is_trusted(None));
    }

    #[test]
    fn sweeps_what_is_older_and_keeps_what_is_new() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(r"old\deep")).unwrap();
        std::fs::write(dir.path().join(r"old\deep\a.tmp"), "x").unwrap();
        std::fs::write(dir.path().join("new.tmp"), "x").unwrap();
        sweep(dir.path(), SystemTime::now() + Duration::from_secs(60));
        assert!(!dir.path().join(r"old\deep\a.tmp").exists());
        sweep(dir.path(), SystemTime::now() - Duration::from_secs(3600));
        assert!(dir.path().exists());
    }

    #[test]
    fn unelevated_the_folder_is_in_temp() {
        if !is_elevated() {
            let folder = path().unwrap();
            assert!(folder.ends_with("DnnManager"));
            assert!(folder.starts_with(std::env::temp_dir()));
        }
    }
}
