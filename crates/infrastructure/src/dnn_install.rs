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
    file_version(&dll).map(|(major, minor, build, _)| format!("{major}.{minor}.{build}"))
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

#[cfg(not(windows))]
pub fn file_version(_path: &Path) -> Option<(u16, u16, u16, u16)> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn reads_a_windows_file_version() {
        let (major, ..) = file_version(&crate::process::system32("kernel32.dll")).unwrap();
        assert!(major >= 6);
    }

    #[test]
    fn a_folder_without_dnn_has_no_version() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dnn_version(dir.path()), None);
    }
}
