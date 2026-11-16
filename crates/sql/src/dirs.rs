//! DNN Manager's own folders under `%ProgramData%\DnnManager` that only administrators and SYSTEM may change
//! (`Files/PrivateTemp.cs`): `temp` (a backup copied out of the container, SqlPackage's NuGet.Config), `tools`
//! (SqlPackage, run as Administrator) and `docker` (docker's configuration). Not elevated, there is no such boundary
//! and the user's %TEMP% is used, as in the C#.
//!
//! Deviation: a folder of that name that someone else made first (or a link) is refused with an error - the C# makes
//! one beside it instead - and an old run's leftovers aren't swept.

use std::path::{Path, PathBuf};

use crate::programs::{is_elevated, known_folders, trust};

/// `%ProgramData%\DnnManager\temp` - files DNN Manager writes and reads back.
pub fn temp() -> Result<PathBuf, String> {
    if !is_elevated() {
        return make(std::env::temp_dir().join("DnnManager"));
    }
    secure(&secure(&known_folders::program_data().join("DnnManager"))?.join("temp"))
}

/// Where DNN Manager installs the programs it runs as Administrator (SqlPackage).
pub fn tools() -> Result<PathBuf, String> {
    if !is_elevated() {
        return make(std::env::temp_dir().join("DnnManager-tools"));
    }
    secure(&secure(&known_folders::program_data().join("DnnManager"))?.join("tools"))
}

/// docker's configuration folder (`DOCKER_CONFIG`) for the docker DNN Manager runs as Administrator.
pub fn docker_config() -> Result<PathBuf, String> {
    if !is_elevated() {
        return make(std::env::temp_dir().join("DnnManager-docker"));
    }
    secure(&secure(&known_folders::program_data().join("DnnManager"))?.join("docker"))
}

fn make(path: PathBuf) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// `path`, made with Administrators and SYSTEM only - or, when it is there, checked to be only theirs.
fn secure(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        create_admin_only(path)?;
    }
    match trust::folder_admin_only(path) {
        trust::Trust::AdminOnly => Ok(path.to_path_buf()),
        _ => Err(format!(
            "{} can be changed by programs without administrator rights (someone else made it) - DNN Manager doesn't use it. \
             Delete it and try again.",
            path.display()
        )),
    }
}

#[cfg(windows)]
fn create_admin_only(path: &Path) -> Result<(), String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::CreateDirectoryW;
    use windows::core::{HSTRING, w};
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // Owner Administrators; protected DACL: SYSTEM and Administrators full control, inherited by what is inside.
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(|e| e.message())?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let made = CreateDirectoryW(&HSTRING::from(path.as_os_str()), Some(&attributes));
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        made.or_else(|e| if path.is_dir() { Ok(()) } else { Err(format!("{}: {}", path.display(), e.message())) })
    }
}

#[cfg(not(windows))]
fn create_admin_only(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folders_are_there_to_use() {
        for folder in [temp(), tools(), docker_config()] {
            assert!(folder.unwrap().is_dir());
        }
    }
}
