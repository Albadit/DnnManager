//! Opening a website or a folder for the user. DNN Manager runs elevated; what it opens for the user must not
//! (`ElevatedStart.Explorer`): Explorer hands the request to the user's own, unelevated Explorer, which opens the
//! folder - or the address in the user's default browser.

use std::path::Path;
use std::process::Command;

use crate::process::system32;

fn explorer(argument: &std::ffi::OsStr) -> Result<(), String> {
    let windows = system32("..").canonicalize().unwrap_or_else(|_| system32(".."));
    Command::new(windows.join("explorer.exe"))
        .arg(argument)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Explorer can't be started: {e}"))
}

/// Opens an http(s) address in the user's browser. Anything else is refused: a site's binding is untrusted input.
pub fn open_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) || url.contains(['"', '\n', '\r']) {
        return Err(format!("Not a web address: {url}"));
    }
    explorer(url.as_ref())
}

/// Opens a folder in File Explorer.
pub fn open_folder(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("The folder isn't there: {}", path.display()));
    }
    explorer(path.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_isnt_a_web_address() {
        assert!(open_url("file:///c:/windows").is_err());
        assert!(open_url("calc.exe").is_err());
        assert!(open_url("http://x\" /select").is_err());
    }
}
