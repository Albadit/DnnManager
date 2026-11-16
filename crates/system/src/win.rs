//! Small Windows helpers the modules share: wide strings, handles, the process list, a file's version resource.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PCWSTR;

/// A NUL-terminated UTF-16 copy of `text`.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A NUL-terminated UTF-16 copy of `path`.
pub fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// The text up to the first NUL.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// A kernel handle closed when it goes.
pub struct Handle(pub HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: the handle was opened by this wrapper's owner and is closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// One running process as Windows lists it.
#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent: u32,
    /// The exe's file name, e.g. `Code.exe`.
    pub exe: String,
}

/// Every running process.
pub fn processes() -> Vec<ProcessEntry> {
    let mut list = Vec::new();
    // SAFETY: the snapshot handle is closed by `Handle`; PROCESSENTRY32W carries its own size.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return list };
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snapshot.0, &mut entry).is_err() {
            return list;
        }
        loop {
            list.push(ProcessEntry {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
                exe: from_wide(&entry.szExeFile),
            });
            if Process32NextW(snapshot.0, &mut entry).is_err() {
                break;
            }
        }
    }
    list
}

/// The program a process runs - read with the least access, so other users' and services' processes too.
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: the process handle is closed by `Handle`; the buffer's length is passed in and the result's read back.
    unsafe {
        let handle = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        QueryFullProcessImageNameW(handle.0, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut size)
            .ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..size as usize])))
    }
}

/// A string of a file's version resource (`ProductVersion`, `FileDescription`), in its first language.
pub fn version_string(file: &Path, name: &str) -> Option<String> {
    let path = wide_path(file);
    // SAFETY: the buffer is the size Windows asked for; VerQueryValueW points into it and is read before it goes.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, data.as_mut_ptr() as *mut c_void).ok()?;

        let mut translation: *mut c_void = std::ptr::null_mut();
        let mut length = 0u32;
        let query = wide(r"\VarFileInfo\Translation");
        let language =
            if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(query.as_ptr()), &mut translation, &mut length)
                .as_bool()
                && length >= 4
            {
                let pair = translation as *const u16;
                format!("{:04x}{:04x}", *pair, *pair.add(1))
            } else {
                "040904b0".to_string()
            };

        let query = wide(&format!(r"\StringFileInfo\{language}\{name}"));
        let mut value: *mut c_void = std::ptr::null_mut();
        if !VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(query.as_ptr()), &mut value, &mut length).as_bool()
            || length == 0
        {
            return None;
        }
        let text = std::slice::from_raw_parts(value as *const u16, length as usize);
        let text = from_wide(text).trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

/// A folder Windows knows (`Environment.GetFolderPath`): `FOLDERID_ProgramFiles`, `FOLDERID_Windows`, …
pub fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: SHGetKnownFolderPath returns a NUL-terminated string Windows allocated; it is copied, then freed.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const c_void));
        text.filter(|t| !t.is_empty()).map(PathBuf::from)
    }
}

/// A string value from the registry as written - `%variables%` not filled in; None when it isn't there.
pub fn registry_string(root: windows::Win32::System::Registry::HKEY, key: &str, name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
    let key = wide(key);
    let name = wide(name);
    let mut size = 0u32;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    // SAFETY: the first call asks for the size; the buffer is that big for the second.
    unsafe {
        if RegGetValueW(root, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()), flags, None, None, Some(&mut size)).is_err()
        {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        let mut size = (buffer.len() * 2) as u32;
        if RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            flags,
            None,
            Some(buffer.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
        .is_err()
        {
            return None;
        }
        Some(from_wide(&buffer))
    }
}

/// `at` in Windows' time zone: year, month, day, hour, minute, second.
pub fn local_parts(at: std::time::SystemTime) -> Option<(u16, u16, u16, u16, u16, u16)> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns from 1601; the Unix epoch is 11 644 473 600 s later.
    let since_unix = at.duration_since(std::time::UNIX_EPOCH).ok()?;
    let ticks = (since_unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(since_unix.subsec_nanos() / 100);
    let file_time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: plain values in and out.
    unsafe {
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some((local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_round_trip() {
        assert_eq!(from_wide(&wide("café")), "café");
    }

    #[test]
    fn lists_this_process_with_its_exe() {
        let me = std::process::id();
        assert!(processes().iter().any(|p| p.pid == me));
        let exe = exe_path(me).unwrap();
        assert_eq!(exe, std::env::current_exe().unwrap());
    }

    #[test]
    fn reads_a_windows_programs_version_resource() {
        let notepad = dnnm_infrastructure::process::system32("cmd.exe");
        assert!(version_string(&notepad, "ProductVersion").is_some());
    }
}
