//! The records of `src/DnnManager.Domain/Models.cs`.

use std::path::PathBuf;

/// A DNN Manager project: a name, the folder its site serves and where its backups go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnProject {
    pub name: String,
    pub project_directory: PathBuf,
    pub backup_directory: PathBuf,
}

/// A DNN release on GitHub: its install package and, derived from it, its upgrade package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnRelease {
    pub version: String,
    pub tag_name: String,
    pub download_url: String,
    pub prerelease: bool,
    pub sha256: Option<String>,
    pub upgrade_sha256: Option<String>,
}

impl DnnRelease {
    /// The upgrade package next to the install package: `…_Install.zip` becomes `…_Upgrade.zip`.
    pub fn upgrade_url(&self) -> Option<String> {
        const INSTALL: &str = "_Install.zip";
        let url = &self.download_url;
        let cut = url.len().checked_sub(INSTALL.len())?;
        // ASCII-only suffix, so a byte cut is a char boundary whenever it matches.
        if url.is_char_boundary(cut) && url[cut..].eq_ignore_ascii_case(INSTALL) {
            Some(format!("{}_Upgrade.zip", &url[..cut]))
        } else {
            None
        }
    }
}

/// The releases of one source, and when they were saved (`dnn_releases.saved_utc`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DnnReleaseList {
    pub releases: Vec<DnnRelease>,
    pub saved_at: Option<String>,
}

/// Where a project's database lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub server: String,
    pub database_name: String,
    pub collation: String,
    pub port: u16,
    pub backup_directory: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(url: &str) -> DnnRelease {
        DnnRelease {
            version: "10.0.0".into(),
            tag_name: "v10.0.0".into(),
            download_url: url.into(),
            prerelease: false,
            sha256: None,
            upgrade_sha256: None,
        }
    }

    #[test]
    fn upgrade_url_replaces_install_suffix_case_insensitively() {
        assert_eq!(
            release("https://x/DNN_Platform_10.0.0_install.ZIP").upgrade_url().as_deref(),
            Some("https://x/DNN_Platform_10.0.0_Upgrade.zip")
        );
    }

    #[test]
    fn upgrade_url_is_none_without_install_suffix() {
        assert_eq!(release("https://x/DNN.zip").upgrade_url(), None);
        assert_eq!(release("zip").upgrade_url(), None);
    }
}
