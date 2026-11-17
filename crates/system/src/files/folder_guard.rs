//! Keeps the projects folder (`C:\DNN`) to SYSTEM, Administrators and you (`Files/ProjectsFolderGuard.cs`). A folder
//! made at the root of C: inherits "Authenticated Users: Modify" - every account on the PC could read each site's
//! web.config (its database password) and change its bin folder. Each site's folder keeps the rights DNN Manager gives
//! IIS on it and no more: what older versions gave (Full control for IIS_IUSRS, IUSR and the app pool) is lowered.
//! The caller runs [`secure`] in the background at the start and whenever the projects folder changes.
//!
//! [`projects_folder_problem`] is `SettingRules.ProjectsFolderProblem` from the C# Application - ported here because
//! the guard can't run without it; it belongs in `dnnm-application` once the settings rules are ported there.

use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::{
    FOLDERID_Documents, FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_ProgramData, FOLDERID_ProgramFiles,
    FOLDERID_ProgramFilesCommon, FOLDERID_ProgramFilesCommonX86, FOLDERID_ProgramFilesX86, FOLDERID_Windows,
};

use super::safe_path::{full, has_link, is_same_or_inside, long_path};
use crate::acl::{Ace, Security, current_user_sid, flags, rights, sids};
use crate::trusted::is_reparse_point;
use crate::win::known_folder;

// The groups everyone is in: none of them may read or change the projects folder.
const EVERYONE: [&str; 4] = [sids::EVERYONE, sids::AUTHENTICATED_USERS, sids::USERS, sids::INTERACTIVE];

// The folders Windows keeps at the top of every drive for itself.
const WINDOWS_DRIVE_FOLDERS: [&str; 8] = [
    "$Recycle.Bin",
    "System Volume Information",
    "Recovery",
    "Config.Msi",
    "$WinREAgent",
    "$WINDOWS.~BT",
    "$Windows.~WS",
    "$SysReset",
];

/// Why `folder` can't be the projects folder, or None when it can (`SettingRules.ProjectsFolderProblem`).
pub fn projects_folder_problem(folder: &str) -> Option<String> {
    let path = Path::new(folder);
    let fully_qualified =
        folder.len() >= 3 && folder.as_bytes()[1] == b':' && matches!(folder.as_bytes()[2], b'\\' | b'/')
            || folder.starts_with(r"\\");
    let Some(full_path) = (!folder.trim().is_empty() && fully_qualified).then(|| full(path)).flatten() else {
        return Some("must be a full path, e.g. C:\\DNN.".into());
    };
    // Compared by its long name: a short one (C:\Users\RUNNER~1\..., as TEMP can be) would look like another account's
    // folder next to the known folders, which are spelled long.
    let full_path = long_path(&full_path);
    const USE_OWN: &str = " - use a folder of its own, e.g. C:\\DNN.";
    let text = full_path.display().to_string();
    if text.len() <= 3 && text.ends_with(['\\', ':']) {
        return Some(format!("can't be a drive{USE_OWN}"));
    }
    // A share or a device path: the folders below can't be told apart there.
    if text.starts_with(r"\\") {
        return Some("must be a folder on this computer, e.g. C:\\DNN.".into());
    }
    let system = [
        &FOLDERID_Windows,
        &FOLDERID_ProgramFiles,
        &FOLDERID_ProgramFilesX86,
        &FOLDERID_ProgramFilesCommon,
        &FOLDERID_ProgramFilesCommonX86,
        &FOLDERID_ProgramData,
    ];
    if system.iter().filter_map(|id| known_folder(id)).any(|f| is_same_or_inside(&full_path, &f)) {
        return Some(format!(
            "can't be a system folder or a folder in one (Windows, Program Files, ProgramData){USE_OWN}"
        ));
    }
    let first_folder = text.get(3..).unwrap_or_default().split('\\').next().unwrap_or_default();
    if WINDOWS_DRIVE_FOLDERS.iter().any(|f| f.eq_ignore_ascii_case(first_folder)) {
        return Some(format!("can't be in {first_folder}, a folder Windows keeps for itself{USE_OWN}"));
    }
    // DNN Manager's own data (Documents\DnnManager) - its database, backups and logs aren't projects.
    let profile = known_folder(&FOLDERID_Profile);
    let data_roots = [
        known_folder(&FOLDERID_Documents).map(|d| d.join("DnnManager")),
        profile.as_ref().map(|p| p.join(r"Documents\DnnManager")),
    ];
    for data in data_roots.into_iter().flatten() {
        if is_same_or_inside(&full_path, &data) {
            return Some(format!(
                "can't be DNN Manager's own data folder ({}) or a folder in it{USE_OWN}",
                data.display()
            ));
        }
    }
    if let Some(profile) = profile {
        if full(&profile).is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(full_path.as_os_str())) {
            return Some(format!("can't be your user folder itself{USE_OWN}"));
        }
        if let Some(local) = known_folder(&FOLDERID_LocalAppData)
            && (is_same_or_inside(&full_path, &local.join("Programs"))
                || is_same_or_inside(&full_path, &local.join(r"Microsoft\WindowsApps")))
        {
            return Some(format!(
                "can't be a folder of programs installed for you (AppData\\Local\\Programs) or a folder in one{USE_OWN}"
            ));
        }
        if let Some(users) = profile.parent() {
            if full(users).is_some_and(|u| u.as_os_str().eq_ignore_ascii_case(full_path.as_os_str())) {
                return Some(format!("can't be {} itself, the folder of every account{USE_OWN}", users.display()));
            }
            if is_same_or_inside(&full_path, users) && !is_same_or_inside(&full_path, &profile) {
                return Some(format!(
                    "can't be in another account's folder in {} (another user's, Public or Default) - only in yours{USE_OWN}",
                    users.display()
                ));
            }
        }
    }
    None
}

/// Makes `folder` - or sets it to - SYSTEM, Administrators and the signed-in user only, when it lets everyone in now.
/// True when it is kept so. Never gives the user rights they didn't have: never a drive, a system folder or anything in
/// one, nothing reached through a link, and of a folder that is already there only one the user owns - or, made by an
/// administrator, one that holds only DNN sites (everyone's groups are then only taken out).
pub fn secure(folder: &str) -> bool {
    if let Some(problem) = projects_folder_problem(folder) {
        log::warn!("The projects folder {folder} is left as it is: it {problem}");
        return false;
    }
    match secure_checked(folder) {
        Ok(kept) => kept,
        Err(e) => {
            log::warn!("Could not keep the projects folder {folder} to administrators and you: {e}");
            false
        }
    }
}

fn secure_checked(folder: &str) -> std::io::Result<bool> {
    let Some(full_path) = full(Path::new(folder)) else { return Ok(false) };
    if through_link(&full_path) {
        return Ok(false);
    }
    let me = current_user_sid().ok_or_else(|| std::io::Error::other("the signed-in user can't be read"))?;
    let Ok(meta) = std::fs::symlink_metadata(&full_path) else {
        std::fs::create_dir_all(full_path.parent().unwrap_or(&full_path))?;
        let _ = rules(&me).create_dir(&full_path);
        // Made by another program in between, it would have been kept as it was: only ours counts.
        let meta = std::fs::symlink_metadata(&full_path)?;
        return Ok(!is_reparse_point(&meta) && !lets_everyone_in(&Security::read(&full_path)?));
    };
    if is_reparse_point(&meta) {
        return Ok(false);
    }
    let security = Security::read(&full_path)?;
    if lets_everyone_in(&security) {
        if security.owner.as_deref() == Some(me.as_str()) {
            rules(&me).apply(&full_path)?;
            log::info!("The projects folder {} is now only for SYSTEM, Administrators and you", full_path.display());
        } else if holds_only_dnn_sites(&full_path) {
            // Made by an administrator - an older DNN Manager made C:\DNN so: everyone's groups are taken out and
            // nothing else changes.
            without_everyone(&security).apply(&full_path)?;
            log::info!("The projects folder {} no longer lets every user in", full_path.display());
        } else {
            log::warn!(
                "The projects folder {} lets every user in and isn't yours: DNN Manager doesn't change the rights of a folder an administrator made - choose a folder of your own (it makes one when it isn't there)",
                full_path.display()
            );
            return Ok(false);
        }
    }
    tighten_sites(&full_path);
    Ok(true)
}

fn tighten_sites(projects: &Path) {
    let Ok(entries) = std::fs::read_dir(projects) else { return };
    for site in entries.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(site.path()) else { continue };
        if !meta.is_dir() || is_reparse_point(&meta) {
            continue;
        }
        match tighten_site(&site.path()) {
            Ok(true) => log::info!(
                "The site folder {}: IIS_IUSRS and IUSR now only read it, its app pool no longer changes its rights",
                site.path().display()
            ),
            Ok(false) => {}
            Err(e) => log::warn!("Could not limit IIS's rights on {}: {e}", site.path().display()),
        }
    }
}

/// Lowers what IIS's identities may do in `site` (its own rules only). True when something changed.
pub fn tighten_site(site: &Path) -> std::io::Result<bool> {
    let mut security = Security::read(site)?;
    let changed = tightened(&mut security);
    if changed {
        security.owner = None;
        security.apply(site)?;
    }
    Ok(changed)
}

/// IIS_IUSRS (every site's app pool) and IUSR to reading, an app pool's own identity to Modify (changing files, not
/// their rights). True when a rule was lowered.
pub fn tightened(security: &mut Security) -> bool {
    let mut changed = false;
    for ace in security.aces.iter_mut().filter(|a| a.allow && !a.is_inherited()) {
        let limit = if ace.sid == sids::IIS_IUSRS || ace.sid == sids::IUSR {
            rights::READ_AND_EXECUTE
        } else if ace.sid.starts_with(sids::APP_POOL_PREFIX) {
            rights::MODIFY
        } else {
            continue;
        };
        if ace.mask & !limit & !rights::SYNCHRONIZE != 0 {
            ace.mask = limit;
            changed = true;
        }
    }
    changed
}

/// Every folder in it is a DNN site (its bin holds DotNetNuke.dll) - DNN Manager's projects folder, not a program's.
fn holds_only_dnn_sites(folder: &Path) -> bool {
    let sites: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir() && !is_reparse_point(&m)))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    !sites.is_empty() && sites.iter().all(|s| s.join(r"bin\DotNetNuke.dll").is_file())
}

/// The folder's rules - its own and those it inherits, kept as its own from now on - without everyone's groups.
pub fn without_everyone(current: &Security) -> Security {
    let aces = current
        .aces
        .iter()
        .filter(|a| !(a.allow && EVERYONE.contains(&a.sid.as_str())))
        .map(|a| Ace { flags: a.flags & !flags::INHERITED, ..a.clone() })
        .collect();
    Security::protected(None, aces)
}

/// Whether one of the groups everyone is in may do anything with the folder.
pub fn lets_everyone_in(security: &Security) -> bool {
    security.allows_any_of(&EVERYONE)
}

/// SYSTEM, Administrators and the signed-in user, each in full - for the folder and everything in it.
pub fn rules(me: &str) -> Security {
    let mut aces = vec![Ace::full_control(sids::SYSTEM), Ace::full_control(sids::ADMINISTRATORS)];
    if me != sids::SYSTEM && me != sids::ADMINISTRATORS {
        aces.push(Ace::full_control(me));
    }
    Security::protected(None, aces)
}

/// Whether a folder on the way to `full_path` - from its drive - is a link or junction.
fn through_link(full_path: &Path) -> bool {
    match (full_path.parent(), full_path.ancestors().last()) {
        (Some(parent), Some(drive)) => has_link(drive, parent, None),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POOL: &str = "S-1-5-82-1-2-3-4-5";

    #[test]
    fn refuses_drives_system_folders_and_relative_paths() {
        assert_eq!(projects_folder_problem("DNN").unwrap(), "must be a full path, e.g. C:\\DNN.");
        assert_eq!(projects_folder_problem("").unwrap(), "must be a full path, e.g. C:\\DNN.");
        assert_eq!(
            projects_folder_problem(r"C:\").unwrap(),
            "can't be a drive - use a folder of its own, e.g. C:\\DNN."
        );
        assert!(projects_folder_problem(r"C:\Windows\x").unwrap().starts_with("can't be a system folder"));
        assert!(projects_folder_problem(r"C:\Program Files\DNN").unwrap().starts_with("can't be a system folder"));
        // Spelled short, where the drive has 8.3 names, it is still Program Files.
        if Path::new(r"C:\PROGRA~1").is_dir() {
            assert!(projects_folder_problem(r"C:\PROGRA~1\DNN").unwrap().starts_with("can't be a system folder"));
        }
        assert!(projects_folder_problem(r"C:\$Recycle.Bin\x").unwrap().starts_with("can't be in $Recycle.Bin"));
        assert_eq!(
            projects_folder_problem(r"\\server\share\dnn").unwrap(),
            "must be a folder on this computer, e.g. C:\\DNN."
        );
        assert_eq!(projects_folder_problem(r"C:\DNN"), None);
        let profile = known_folder(&FOLDERID_Profile).unwrap();
        assert!(
            projects_folder_problem(&profile.display().to_string()).unwrap().starts_with("can't be your user folder")
        );
        let documents = known_folder(&FOLDERID_Documents).unwrap().join(r"DnnManager\sites");
        assert!(
            projects_folder_problem(&documents.display().to_string())
                .unwrap()
                .starts_with("can't be DNN Manager's own data")
        );
        let other = profile.parent().unwrap().join(r"Public\DNN");
        assert!(
            projects_folder_problem(&other.display().to_string())
                .unwrap()
                .starts_with("can't be in another account's folder")
        );
    }

    #[test]
    fn lowers_what_older_versions_gave_iis_and_nothing_else() {
        let mut security = Security {
            owner: None,
            protected: false,
            aces: vec![
                Ace::full_control(sids::IIS_IUSRS),
                Ace::full_control(sids::IUSR),
                Ace::full_control(POOL),
                Ace::full_control(sids::USERS),
                Ace { flags: flags::INHERITED, ..Ace::full_control(sids::IUSR) },
            ],
        };
        assert!(tightened(&mut security));
        let masks: Vec<u32> = security.aces.iter().map(|a| a.mask).collect();
        assert_eq!(
            masks,
            vec![
                rights::READ_AND_EXECUTE,
                rights::READ_AND_EXECUTE,
                rights::MODIFY,
                rights::FULL_CONTROL,
                rights::FULL_CONTROL
            ]
        );
        assert!(!tightened(&mut security), "already as DNN Manager gives it");
    }

    #[test]
    fn takes_everyones_groups_out_and_keeps_the_rest_as_its_own() {
        let current = Security {
            owner: Some(sids::ADMINISTRATORS.into()),
            protected: false,
            aces: vec![
                Ace { flags: flags::INHERITED, ..Ace::allow(sids::AUTHENTICATED_USERS, rights::MODIFY) },
                Ace { flags: flags::INHERITED | flags::CONTAINER_INHERIT, ..Ace::full_control(sids::SYSTEM) },
                Ace::allow(sids::USERS, rights::READ_AND_EXECUTE),
            ],
        };
        assert!(lets_everyone_in(&current));
        let next = without_everyone(&current);
        assert!(next.protected);
        assert!(!lets_everyone_in(&next));
        assert_eq!(next.aces.len(), 1);
        assert!(!next.aces[0].is_inherited());
        assert!(!lets_everyone_in(&rules("S-1-5-21-1")));
    }

    #[test]
    fn a_folder_of_its_own_in_temp_is_kept_to_the_user() {
        // %TEMP% is in the user's profile: a folder there is the user's own, made only for them.
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("DNN");
        let kept = secure(&projects.display().to_string());
        assert!(kept);
        assert!(!lets_everyone_in(&Security::read(&projects).unwrap()));
    }

    #[test]
    fn a_folder_in_the_own_profile_spelled_short_is_allowed() {
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        use windows::core::PCWSTR;
        let dir = tempfile::tempdir().unwrap();
        let wide: Vec<u16> = dir.path().as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
        let mut buffer = vec![0u16; 32768];
        // SAFETY: the buffer's length is passed in.
        let length = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) } as usize;
        assert!(length > 0 && length < buffer.len());
        let short = PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("DNN");
        // Where 8.3 names are off, or no part needs one, it is the long spelling: still allowed.
        assert_eq!(projects_folder_problem(&short.display().to_string()), None, "{}", short.display());
    }
}
