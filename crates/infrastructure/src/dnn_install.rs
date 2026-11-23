//! The DNN version of a site's files, from `bin\DotNetNuke.dll` - "10.3.3", or None when there is no DNN.
//!
//! The WPF app reads the assembly version; this reads the file version's first three parts, which DNN's builds set to
//! the same number (a known, documented deviation for a hand-built DotNetNuke.dll whose two versions differ).

use std::path::Path;

pub fn dnn_version(site_directory: &Path) -> Option<String> {
    let dll = site_directory.join("bin").join("DotNetNuke.dll");
    if !dll.is_file() {
        return None;
    }
    // Windows' version API; when it can't read the file, its bytes.
    let version = file_version(&dll).or_else(|| version_in_bytes(&std::fs::read(&dll).ok()?));
    version.map(|(major, minor, build, _)| format!("{major}.{minor}.{build}"))
}

/// A file's version resource: major, minor, build, revision.
#[cfg(windows)]
pub fn file_version(path: &Path) -> Option<(u16, u16, u16, u16)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated path; the buffer is as large as Windows asked for, and the pointer
    // VerQueryValueW returns points into it and is read only while it lives.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wide.as_ptr()), None, size, buffer.as_mut_ptr().cast()).ok()?;
        let mut info: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let root: Vec<u16> = "\\".encode_utf16().chain(Some(0)).collect();
        if !VerQueryValueW(buffer.as_ptr().cast(), PCWSTR(root.as_ptr()), &mut info, &mut len).as_bool()
            || info.is_null()
            || (len as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let fixed = &*(info as *const VS_FIXEDFILEINFO);
        Some((
            (fixed.dwFileVersionMS >> 16) as u16,
            (fixed.dwFileVersionMS & 0xFFFF) as u16,
            (fixed.dwFileVersionLS >> 16) as u16,
            (fixed.dwFileVersionLS & 0xFFFF) as u16,
        ))
    }
}

/// Off Windows there is no version API: the file's own bytes are read ([`version_in_bytes`]) - kept per file while
/// its size and time stay the same, as the projects are read every few seconds and DotNetNuke.dll is megabytes.
#[cfg(not(windows))]
pub fn file_version(path: &Path) -> Option<(u16, u16, u16, u16)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Seen = HashMap<std::path::PathBuf, ((u64, Option<std::time::SystemTime>), Option<(u16, u16, u16, u16)>)>;
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok());
    if let Some((seen, version)) = SEEN.lock().ok()?.get_or_insert_default().get(path)
        && *seen == stamp
    {
        return *version;
    }
    let version = version_in_bytes(&std::fs::read(path).ok()?);
    SEEN.lock().ok()?.get_or_insert_default().insert(path.to_path_buf(), (stamp, version));
    version
}

/// The file version in a Windows program's or DLL's bytes: the fixed part of its version resource
/// (`VS_FIXEDFILEINFO`) - its signature 0xFEEF04BD, its structure version 1.0, then the version's high and low
/// halves. The last such block counts (the resources come last in the file).
pub fn version_in_bytes(bytes: &[u8]) -> Option<(u16, u16, u16, u16)> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let at = (0..bytes.len().saturating_sub(15))
        .rev()
        .find(|&at| word(at) == Some(0xFEEF04BD) && word(at + 4) == Some(0x0001_0000))?;
    let (high, low) = (word(at + 8)?, word(at + 12)?);
    Some(((high >> 16) as u16, (high & 0xFFFF) as u16, (low >> 16) as u16, (low & 0xFFFF) as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_version_resource_in_the_bytes() {
        let mut bytes = vec![0xBD, 0x04, 0xEF, 0xFE, 0, 0, 0, 0]; // a signature without the structure version: not it
        for word in [0xFEEF04BDu32, 0x0001_0000, (9 << 16) | 13, (9 << 16) | 2] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(version_in_bytes(&bytes), Some((9, 13, 9, 2)));
        assert_eq!(version_in_bytes(&[0xBD, 0x04, 0xEF, 0xFE]), None);
        assert_eq!(version_in_bytes(&[]), None);
    }

    /// The bytes give what Windows' own version API gives, for a .NET Framework assembly as DotNetNuke.dll is (not a
    /// system DLL: for kernel32.dll the API reports an older version to a program without a manifest).
    #[cfg(windows)]
    #[test]
    fn the_bytes_agree_with_windows() {
        let windir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
        let dll = windir.join("Microsoft.NET").join("Framework64").join("v4.0.30319").join("mscorlib.dll");
        assert!(file_version(&dll).is_some(), "{}", dll.display());
        assert_eq!(version_in_bytes(&std::fs::read(&dll).unwrap()), file_version(&dll));
    }

    #[cfg(windows)]
    #[test]
    fn reads_a_windows_file_version() {
        let (major, ..) = file_version(&crate::process::system32("kernel32.dll")).unwrap();
        assert!(major >= 6);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_without_dnn_has_no_version() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dnn_version(dir.path()), None);
    }
}
