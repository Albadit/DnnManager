//! Everything read about a site for its Details (`ProjectSnapshot`, `FolderFacts`, `ProjectView.ReadAllAsync`):
//! each part from where it really is - IIS, the folder, web.config, bin, the database with web.config's connection.

use std::path::{Path, PathBuf};

use dnnm_application::iis::{IisManager, IisSiteDetails};
use dnnm_application::project::ProjectState;
use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind, is_local_db, read_database_connection};

use crate::assemblies::{self, AssemblyInspection};
use crate::database::DatabaseInspection;
use crate::folders::{self, FolderCheck};
use crate::format::LocalDateTime;
use crate::web_config::{self, WebConfigInspection};

/// How DNN Manager set the site up (`ProjectRecord` in dnnmanager.db) - read by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRecord {
    /// Automatic setup; false: DNN's installation wizard.
    pub automatic: bool,
    pub created: LocalDateTime,
}

/// What the project's folder says about how it is worked on: its git branch, its solutions, its backups.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderFacts {
    pub git_branch: Option<String>,
    pub solutions: Vec<String>,
    /// When each backup was made, newest first.
    pub backups: Vec<LocalDateTime>,
    /// Where the project's backups are kept.
    pub backup_directory: Option<PathBuf>,
}

/// Everything the tabs are made of. `project` is the row of the Projects table, as the monitor last read it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSnapshot {
    pub project: ProjectState,
    pub iis: Option<IisSiteDetails>,
    pub config: WebConfigInspection,
    pub folders: Vec<FolderCheck>,
    pub assemblies: AssemblyInspection,
    /// The product name of bin\DotNetNuke.dll ("DNN Platform").
    pub dnn_product: Option<String>,
    /// The database as web.config connects to it; None when it names none.
    pub connection: Option<DatabaseConnection>,
    /// What the database said; None while it is read, or when there is none to read.
    pub database: Option<DatabaseInspection>,
    pub database_read: bool,
    pub record: Option<SetupRecord>,
    pub folder: FolderFacts,
}

impl ProjectSnapshot {
    pub fn is_dnn(&self) -> bool {
        self.project.dnn_version.is_some()
    }

    /// Whether the database still has to be read (with [`crate::database::DatabaseInspector`]): web.config names
    /// one, and it isn't a LocalDB file the site runs itself.
    pub fn needs_database(&self) -> bool {
        self.connection.is_some() && !self.project.database_is_file && !self.database_read
    }

    /// The snapshot with what the database said.
    pub fn with_database(mut self, inspection: DatabaseInspection) -> Self {
        self.database = Some(inspection);
        self.database_read = true;
        self
    }
}

/// Reads this PC's part of a site - IIS, web.config, the folders, bin - as the C# does before the database: file
/// system work, so on a background thread. `backup_directory` is where the project's backups are kept.
pub fn read_local(
    project: &ProjectState,
    iis: &dyn IisManager,
    backup_directory: Option<&Path>,
    record: Option<SetupRecord>,
) -> ProjectSnapshot {
    let directory = &project.directory;
    let details = iis.site_details(&project.name);
    let config = web_config::inspect(directory);
    let account = details.as_ref().and_then(|d| d.pool.as_ref()).map(|p| p.account.clone());
    let folders = if directory.is_dir() { folders::inspect(directory, account.as_deref()) } else { Vec::new() };
    let assemblies =
        assemblies::inspect(directory, &config.binding_redirects, &config.code_bases, config.probing_path.as_deref());
    // A product name, not the address some builds put there.
    let dnn_product =
        product_name(&directory.join(r"bin\DotNetNuke.dll")).filter(|p| !p.trim().is_empty() && !p.contains("://"));
    let connection = database_of(directory);
    let database_read = connection.is_none() || project.database_is_file;
    ProjectSnapshot {
        project: project.clone(),
        iis: details,
        config,
        folders,
        assemblies,
        dnn_product,
        connection,
        database: None,
        database_read,
        record,
        folder: read_folder_facts(directory, backup_directory),
    }
}

/// The database the site's web.config names (`LocalSqlContainer.DatabaseOf`); None when it has none of its own yet -
/// DNN's shipped "SQL Server Express File".
pub fn database_of(directory: &Path) -> Option<DatabaseConnection> {
    let connection = read_database_connection(&directory.join("web.config")).ok()?;
    if connection.kind == DatabaseKind::LocalDbFile && !is_local_db(&connection.server) {
        return None;
    }
    Some(connection)
}

/// The git branch, solutions and backups (`ProjectDiagnostics.ReadFolderFacts`).
pub fn read_folder_facts(directory: &Path, backup_directory: Option<&Path>) -> FolderFacts {
    FolderFacts {
        git_branch: git_branch(directory),
        solutions: solutions(directory),
        backups: backup_directory.map(backups).unwrap_or_default(),
        backup_directory: backup_directory.map(Path::to_path_buf),
    }
}

fn solutions(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| {
            let lower = n.to_lowercase();
            lower.ends_with(".sln") || lower.ends_with(".slnx")
        })
        .collect();
    found.sort_by_key(|n| n.to_lowercase());
    found
}

/// The checked-out branch from `.git\HEAD`; None when the folder isn't a git repository.
fn git_branch(directory: &Path) -> Option<String> {
    let head = directory.join(".git").join("HEAD");
    if !head.is_file() {
        return directory.join(".git").is_dir().then(|| "(git repository)".to_string());
    }
    let Ok(text) = std::fs::read_to_string(&head) else { return Some("(unreadable)".into()) };
    let text = text.trim();
    Some(match text.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None if text.len() >= 7 => format!("(detached at {})", &text[..7]),
        None => "(unknown)".into(),
    })
}

/// Copies people make by hand (web.config.bak) - not database backups (`LocalSqlContainer.IsBackupFile`).
const COPIED_FILE_EXTENSIONS: [&str; 12] =
    [".config", ".json", ".xml", ".txt", ".resources", ".resx", ".js", ".css", ".aspx", ".ascx", ".cs", ".dll"];

fn is_backup_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.ends_with(".bacpac") {
        return true;
    }
    let Some(stem) = lower.strip_suffix(".bak") else { return false };
    let inner = stem.rfind('.').map(|i| &stem[i..]).unwrap_or_default();
    !COPIED_FILE_EXTENSIONS.contains(&inner)
}

/// The project's dated backups, newest first (`ProjectBackups.List`): each folder with a site zip or a database
/// backup, dated by its name (`<project>_20260928_154210`), else by when the folder was made.
pub fn backups(directory: &Path) -> Vec<LocalDateTime> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut found: Vec<LocalDateTime> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            std::fs::read_dir(e.path()).is_ok_and(|files| {
                files
                    .flatten()
                    .filter_map(|f| f.file_name().to_str().map(str::to_lowercase))
                    .any(|n| n.ends_with(".zip") || is_backup_file(&n))
            })
        })
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            stamp(&name).unwrap_or_else(|| {
                e.metadata().and_then(|m| m.created()).map(LocalDateTime::from_system_time).unwrap_or_default()
            })
        })
        .collect();
    found.sort_by(|a, b| b.cmp(a));
    found
}

/// "shop_20260928_154210" → 2026-09-28 15:42:10.
fn stamp(name: &str) -> Option<LocalDateTime> {
    let stamp = name.get(name.len().checked_sub(15)?..)?;
    if name.len() <= 15
        || stamp.as_bytes()[8] != b'_'
        || !stamp.chars().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit())
    {
        return None;
    }
    let n = |range: std::ops::Range<usize>| stamp[range].parse::<u16>().ok();
    let at = LocalDateTime::new(n(0..4)?, n(4..6)?, n(6..8)?, n(9..11)?, n(11..13)?, n(13..15)?);
    ((1..=12).contains(&at.month) && (1..=31).contains(&at.day) && at.hour < 24 && at.minute < 60 && at.second < 60)
        .then_some(at)
}

/// The ProductName of a file's version resource (`FileVersionInfo.ProductName`).
#[cfg(windows)]
pub fn product_name(file: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    use windows::core::HSTRING;
    let path = HSTRING::from(file.as_os_str());
    // SAFETY: the version block is read into a buffer of the size Windows gives; VerQueryValueW points into it.
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, block.as_mut_ptr() as *mut _).ok()?;
        let query = |sub: &str| -> Option<(*const u8, u32)> {
            let mut value = std::ptr::null_mut();
            let mut len = 0u32;
            VerQueryValueW(block.as_ptr() as *const _, &HSTRING::from(sub), &mut value, &mut len)
                .as_bool()
                .then_some((value as *const u8, len))
        };
        let (translations, len) = query(r"\VarFileInfo\Translation")?;
        let pairs = std::slice::from_raw_parts(translations as *const u16, (len / 2) as usize);
        let mut candidates: Vec<String> =
            pairs.as_chunks::<2>().0.iter().map(|p| format!("{:04x}{:04x}", p[0], p[1])).collect();
        candidates.extend(["040904b0".to_string(), "040904e4".to_string(), "000004b0".to_string()]);
        candidates.into_iter().find_map(|code| {
            let (text, len) = query(&format!(r"\StringFileInfo\{code}\ProductName"))?;
            let chars = std::slice::from_raw_parts(text as *const u16, len as usize);
            let value = String::from_utf16_lossy(chars).trim_end_matches('\0').to_string();
            (!value.is_empty()).then_some(value)
        })
    }
}

#[cfg(not(windows))]
pub fn product_name(_file: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_git_solutions_and_backups() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join(".git")).unwrap();
        std::fs::write(site.join(".git").join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        std::fs::write(site.join("b.slnx"), "").unwrap();
        std::fs::write(site.join("A.sln"), "").unwrap();
        let backups_dir = dir.path().join("backups");
        for (folder, file) in [
            ("shop_20260928_154210", "shop.zip"),
            ("shop_20261001_090000", "shop.bacpac"),
            ("copies", "web.config.bak"),
        ] {
            std::fs::create_dir_all(backups_dir.join(folder)).unwrap();
            std::fs::write(backups_dir.join(folder).join(file), "").unwrap();
        }
        let facts = read_folder_facts(&site, Some(&backups_dir));
        assert_eq!(facts.git_branch.as_deref(), Some("feature/x"));
        assert_eq!(facts.solutions, ["A.sln", "b.slnx"]);
        assert_eq!(
            facts.backups,
            [LocalDateTime::new(2026, 10, 1, 9, 0, 0), LocalDateTime::new(2026, 9, 28, 15, 42, 10)]
        );
        std::fs::write(site.join(".git").join("HEAD"), "0123456789abcdef").unwrap();
        assert_eq!(git_branch(&site).as_deref(), Some("(detached at 0123456)"));
        assert_eq!(git_branch(dir.path()), None);
    }

    #[test]
    fn backup_files_and_stamps() {
        assert!(is_backup_file("shop.bak") && is_backup_file("shop.bacpac"));
        assert!(!is_backup_file("web.config.bak") && !is_backup_file("x.zip"));
        assert_eq!(stamp("shop_20260928_154210"), Some(LocalDateTime::new(2026, 9, 28, 15, 42, 10)));
        assert_eq!(stamp("20260928_154210"), None);
        assert_eq!(stamp("shop_20261328_154210"), None);
    }

    #[test]
    fn reads_a_product_name() {
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let name = product_name(&Path::new(&windows).join(r"System32\kernel32.dll"));
        assert!(name.is_some_and(|n| n.contains("Windows")));
        assert_eq!(product_name(Path::new(r"C:\no\such.dll")), None);
    }
}
