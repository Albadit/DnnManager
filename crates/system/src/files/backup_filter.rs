//! A site's `_backup.filter` (`DnnManager.Application/UseCases/BackupFilter.cs`): the files and folders a backup leaves
//! out, one per line, in the format Azure App Service uses - paths from `D:\home`, so `\site\wwwroot\App_Data\Search`
//! is the site's `App_Data\Search`. A path without the `\site\wwwroot` prefix is taken as relative to the site root. A
//! folder leaves out everything in it.

use std::path::Path;

pub const FILE_NAME: &str = "_backup.filter";

const AZURE_SITE_ROOT: &str = r"site\wwwroot";

/// The paths to leave out, relative to `site_directory` (e.g. `App_Data\Search`) - empty when the site has no
/// `_backup.filter` or it can't be read.
pub fn read(site_directory: &Path) -> Vec<String> {
    std::fs::read_to_string(site_directory.join(FILE_NAME)).map(|text| parse(&text)).unwrap_or_default()
}

/// The paths a filter's text leaves out. Blank lines, the site root itself and paths leaving the site are ignored.
pub fn parse(text: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut entry = line.trim().replace('/', "\\").trim_matches('\\').to_string();
        let prefix = format!("{AZURE_SITE_ROOT}\\");
        if entry.len() > prefix.len() && entry[..prefix.len()].eq_ignore_ascii_case(&prefix) {
            entry = entry[prefix.len()..].trim_matches('\\').to_string();
        } else if entry.eq_ignore_ascii_case(AZURE_SITE_ROOT) {
            // The whole site.
            continue;
        }
        if entry.is_empty() || entry.split('\\').any(|part| part == "." || part == "..") {
            continue;
        }
        if !paths.iter().any(|p| p.eq_ignore_ascii_case(&entry)) {
            paths.push(entry);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_azure_paths_and_site_relative_ones() {
        let text = "\\site\\wwwroot\\App_Data\\Search\r\n/site/wwwroot/Portals/0/Cache/\r\nbin\\obj\r\n\r\n\\site\\wwwroot\r\n..\\outside\r\nApp_Data\\.\\x\r\napp_data\\search\r\n";
        assert_eq!(parse(text), vec![r"App_Data\Search", r"Portals\0\Cache", r"bin\obj"]);
    }

    #[test]
    fn a_site_without_a_filter_leaves_nothing_out() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).is_empty());
        std::fs::write(dir.path().join(FILE_NAME), "App_Data/Search").unwrap();
        assert_eq!(read(dir.path()), vec![r"App_Data\Search"]);
    }
}
