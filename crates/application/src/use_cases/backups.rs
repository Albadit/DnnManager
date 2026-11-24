//! A project's dated backups (`UseCases/ProjectBackups.cs`, `LocalSqlContainer.IsBackupFile`) and what Export,
//! Export for deployment and Restore backup see the site's files and its database through - the C#'s
//! `IProjectFileCopier`, `IBacpacService` and `IDatabaseProvisioner`, as far as these three use cases use them.
//!
//! A backup is a folder `<backups>\<project>\<project>_<yyyyMMdd_HHmmss>\` in `Documents\DnnManager\backups` - outside
//! the site, so IIS never serves it, a site export never includes it, and it is kept when the project is removed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::operation::{CancelToken, Reporter};
use crate::use_cases::portal_aliases::{AliasRow, Delocalized};

/// A backup's notes, when something says why it was made (an upgrade's stages do): its first line names it.
pub const NOTE_FILE: &str = "backup.txt";

/// A moment in Windows' time zone, to the second - what `DateTime.Now` gives the C# code. Ordered in time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Stamp {
    /// `yyyyMMdd_HHmmss` - a backup or deployment folder's name ends with it.
    pub fn folder_suffix(&self) -> String {
        format!("{:04}{:02}{:02}_{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }

    /// `yyyyMMddHHmmss` - the end of a temporary database's name.
    pub fn compact(&self) -> String {
        format!("{:04}{:02}{:02}{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }

    /// `yyyy-MM-dd HH:mm:ss` - the Restore backup menu's items.
    pub fn display(&self) -> String {
        format!("{} {:02}:{:02}:{:02}", self.date(), self.hour, self.minute, self.second)
    }

    /// `yyyy-MM-dd HH:mm` - Restore's title and toast.
    pub fn display_minutes(&self) -> String {
        format!("{} {:02}:{:02}", self.date(), self.hour, self.minute)
    }

    fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// The stamp a folder's name ends with (`<project>_20260928_154210`), as `DateTime.TryParseExact` reads it: None
    /// when the name is no longer than the stamp, or its end isn't a valid date and time.
    pub fn from_folder_name(name: &str) -> Option<Self> {
        const LEN: usize = "yyyyMMdd_HHmmss".len();
        if name.len() <= LEN || !name.is_char_boundary(name.len() - LEN) {
            return None;
        }
        let s = &name[name.len() - LEN..];
        let b = s.as_bytes();
        if b[8] != b'_' || !b[..8].iter().chain(&b[9..]).all(u8::is_ascii_digit) {
            return None;
        }
        let n = |range: std::ops::Range<usize>| s[range].parse::<u16>().ok();
        let stamp = Stamp {
            year: n(0..4)?,
            month: n(4..6)? as u8,
            day: n(6..8)? as u8,
            hour: n(9..11)? as u8,
            minute: n(11..13)? as u8,
            second: n(13..15)? as u8,
        };
        let valid = (1..=12).contains(&stamp.month)
            && stamp.day >= 1
            && stamp.day <= days_in_month(stamp.year, stamp.month)
            && stamp.hour < 24
            && stamp.minute < 60
            && stamp.second < 60
            && stamp.year >= 1;
        valid.then_some(stamp)
    }

    /// Seconds since 1970-01-01 00:00:00 of the same wall-clock time - to count days between two stamps.
    fn seconds(&self) -> i64 {
        let days = days_from_civil(self.year as i64, self.month as i64, self.day as i64);
        days * 86_400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
    }

    fn from_seconds(seconds: i64) -> Self {
        let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
        let (y, m, d) = civil_from_days(days);
        Stamp {
            year: y.clamp(1, 9999) as u16,
            month: m as u8,
            day: d as u8,
            hour: (rest / 3600) as u8,
            minute: (rest / 60 % 60) as u8,
            second: (rest % 60) as u8,
        }
    }

    /// This moment `days` days earlier (`now - TimeSpan.FromDays(days)`).
    pub fn minus_days(&self, days: u32) -> Self {
        Self::from_seconds(self.seconds() - days as i64 * 86_400)
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a calendar date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// One dated backup of a project (`ProjectBackup`): its folder holding the site's `.zip` and / or the database's
/// `.bacpac` (or `.bak`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBackup {
    pub project: String,
    pub folder: PathBuf,
    pub created: Stamp,
    pub site_zip: Option<PathBuf>,
    pub database: Option<PathBuf>,
    /// What it was made for - the first line of its [`NOTE_FILE`], e.g. "Before upgrading DNN 09.13.09 → 10.02.05".
    pub note: Option<String>,
}

impl ProjectBackup {
    /// Both halves are there, so it can be put back or imported as a new project.
    pub fn is_complete(&self) -> bool {
        self.site_zip.is_some() && self.database.is_some()
    }

    /// Its item in Restore backup: `2026-09-28 15:42:10  -  Before upgrading …`; its tooltip is the folder.
    pub fn menu_label(&self) -> String {
        match &self.note {
            Some(note) => format!("{}  -  {note}", self.created.display()),
            None => self.created.display(),
        }
    }
}

/// A new backup folder for a backup taken at `when` (not created yet): `<backups>\<project>_<yyyyMMdd_HHmmss>`.
pub fn new_folder(backup_directory: &Path, project: &str, when: Stamp) -> PathBuf {
    backup_directory.join(format!("{project}_{}", when.folder_suffix()))
}

/// The site zip's name inside a backup folder.
pub fn site_zip_name(project: &str) -> String {
    format!("{project}.zip")
}

/// The database file's name inside a backup folder, for `extension` ".bacpac" or ".bak".
pub fn database_name(project: &str, extension: &str) -> String {
    format!("{project}{extension}")
}

/// The extensions of hand-made copies like `web.config.bak` - those aren't database backups.
const COPIED_FILE_EXTENSIONS: &[&str] =
    &[".config", ".json", ".xml", ".txt", ".resources", ".resx", ".js", ".css", ".aspx", ".ascx", ".cs", ".dll"];

/// A `.bacpac` or a native `.bak` - not a copy like `web.config.bak` (`LocalSqlContainer.IsBackupFile`).
pub fn is_backup_file(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".bacpac") {
        return true;
    }
    let Some(stem) = name.strip_suffix(".bak") else { return false };
    let inner = stem.rfind('.').map(|i| &stem[i..]).unwrap_or_default();
    !COPIED_FILE_EXTENSIONS.contains(&inner)
}

/// The project's dated backups in `backup_directory`, newest first (`ProjectBackups.List`). A folder without a zip or
/// a database backup isn't one; a folder named by hand is dated by when it was made (`created_at` turns a file time
/// into Windows' local time).
pub fn list(backup_directory: &Path, project: &str, created_at: &dyn Fn(SystemTime) -> Stamp) -> Vec<ProjectBackup> {
    let Ok(entries) = std::fs::read_dir(backup_directory) else { return Vec::new() };
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let folder = entry.path();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map(|files| {
                files.flatten().filter(|f| f.file_type().is_ok_and(|t| t.is_file())).map(|f| f.path()).collect()
            })
            .unwrap_or_default();
        files.sort();
        let zip = files.iter().find(|f| has_extension(f, "zip")).cloned();
        // A .bacpac before a .bak.
        let database = files.iter().filter(|f| is_backup_file(f)).min_by_key(|f| !has_extension(f, "bacpac")).cloned();
        if zip.is_none() && database.is_none() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let created = Stamp::from_folder_name(&name)
            .unwrap_or_else(|| entry.metadata().and_then(|m| m.created()).map(created_at).unwrap_or_default());
        backups.push(ProjectBackup {
            project: project.to_string(),
            note: note(&folder),
            folder,
            created,
            site_zip: zip,
            database,
        });
    }
    backups.sort_by_key(|b| std::cmp::Reverse(b.created));
    backups
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

fn note(folder: &Path) -> Option<String> {
    let text = std::fs::read_to_string(folder.join(NOTE_FILE)).ok()?;
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// A path made absolute, without a trailing separator, in lower case - to compare folders as Windows does.
fn normal(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    full.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// `path` is in `folder`, at any depth - not `folder` itself (`SafePath.IsInside`).
pub fn is_inside(path: &Path, folder: &Path) -> bool {
    let (p, f) = (normal(path), normal(folder));
    !f.is_empty() && p.len() > f.len() + 1 && p.starts_with(&f) && p.as_bytes()[f.len()] == b'\\'
}

/// `path` is `folder` or in it (`SafePath.IsSameOrInside`).
pub fn is_same_or_inside(path: &Path, folder: &Path) -> bool {
    normal(path) == normal(folder) || is_inside(path, folder)
}

/// A server, database and login as the site's web.config names them (`SiteSqlServerConnection`) - an empty user is
/// Windows authentication as whoever runs DNN Manager. The password is never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct SqlLogin {
    pub server: String,
    pub database: String,
    pub user: String,
    pub password: String,
}

impl SqlLogin {
    pub fn new(server: &str, database: &str, user: &str, password: &str) -> Self {
        Self { server: server.into(), database: database.into(), user: user.into(), password: password.into() }
    }

    /// The same server and login, another database (`connection with { Database = … }`).
    pub fn with_database(&self, database: &str) -> Self {
        Self { database: database.into(), ..self.clone() }
    }

    pub fn uses_windows_authentication(&self) -> bool {
        self.user.is_empty()
    }
}

impl std::fmt::Debug for SqlLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlLogin")
            .field("server", &self.server)
            .field("database", &self.database)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// The site's files, as the backups see them (`IProjectFileCopier`, `BackupFilter`).
pub trait SiteFiles: Send + Sync {
    /// What the site's `_backup.filter` leaves out (`BackupFilter.Read`): empty when it has none; Err when it is there
    /// but can't be read.
    fn backup_filter(&self, site: &Path) -> Result<Vec<String>, String>;

    /// Zips `site` into `zip`, leaving out `excluded` (relative paths, folders with what is in them).
    fn create_zip(
        &self,
        site: &Path,
        zip: &Path,
        excluded: &[String],
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
}

/// SqlPackage, and where a site's database is exported from (`IBacpacService`, `LocalSqlContainer.ExportSourceOf`).
pub trait Bacpacs: Send + Sync {
    /// The database to export the site's data from - as its web.config names it, or the local SQL container's user for
    /// a database there; Err for a LocalDB file, which can't be exported.
    fn export_source(&self, project: &str, site: &Path) -> Result<SqlLogin, String>;

    /// Installs SqlPackage when it isn't there yet.
    fn ensure_available(&self, report: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String>;

    fn export(
        &self,
        source: &SqlLogin,
        bacpac: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;

    /// Imports `bacpac` as the new database `target.database` on `target.server`, signed in as `target`.
    fn import(
        &self,
        target: &SqlLogin,
        bacpac: &Path,
        report: &mut dyn Reporter,
        cancel: &CancelToken,
    ) -> Result<(), String>;
}

/// What the use cases do to whole databases (`IDatabaseProvisioner`). The database is the login's.
pub trait Databases: Send + Sync {
    fn exists(&self, database: &SqlLogin, cancel: &CancelToken) -> Result<bool, String>;
    /// Drops it, closing the connections still open to it. Ok when it isn't there.
    fn drop_database(&self, database: &SqlLogin) -> Result<(), String>;
    fn rename(&self, database: &SqlLogin, new_name: &str) -> Result<(), String>;
    /// Makes `aliases` portal 0's only aliases, the first the primary one.
    fn replace_portal_aliases(
        &self,
        database: &SqlLogin,
        aliases: &[String],
        cancel: &CancelToken,
    ) -> Result<(), String>;
    /// DNN's SSL setting on or off for every portal.
    fn set_ssl(&self, database: &SqlLogin, on: bool, cancel: &CancelToken) -> Result<(), String>;
    /// Every portal alias, and each portal's noted live primary alias
    /// ([`LIVE_PRIMARY_SETTING`](super::portal_aliases::LIVE_PRIMARY_SETTING)). Reads only.
    fn portal_aliases(&self, database: &SqlLogin, cancel: &CancelToken) -> Result<PortalAliases, String>;
    /// Makes the aliases as `plan` says - local ones removed, the live primary ones primary - and clears the noted
    /// live primaries, in one transaction.
    fn apply_live_aliases(&self, database: &SqlLogin, plan: &[Delocalized], cancel: &CancelToken) -> Result<(), String>;
}

/// A database's portal aliases, and each portal's noted live primary alias.
pub type PortalAliases = (Vec<AliasRow>, std::collections::HashMap<i32, String>);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn stamp(text: &str) -> Stamp {
        Stamp::from_folder_name(&format!("x_{text}")).unwrap()
    }

    #[test]
    fn stamps_read_and_write_as_dotnet_does() {
        let s = stamp("20260928_154210");
        assert_eq!((s.folder_suffix(), s.compact()), ("20260928_154210".into(), "20260928154210".into()));
        assert_eq!((s.display(), s.display_minutes()), ("2026-09-28 15:42:10".into(), "2026-09-28 15:42".into()));
        assert_eq!(Stamp::from_folder_name("20260928_154210"), None, "nothing before the stamp");
        assert_eq!(Stamp::from_folder_name("x_20260230_154210"), None, "no 30 February");
        assert_eq!(Stamp::from_folder_name("x_2026092a_154210"), None);
        assert_eq!(Stamp::from_folder_name("my backup"), None);
        assert!(stamp("20240229_000000").day == 29);
    }

    #[test]
    fn days_back_cross_months_and_years() {
        assert_eq!(stamp("20260305_101112").minus_days(7), stamp("20260226_101112"));
        assert_eq!(stamp("20260105_000000").minus_days(10), stamp("20251226_000000"));
        assert_eq!(stamp("20240301_000000").minus_days(1), stamp("20240229_000000"));
        assert_eq!(stamp("20260928_154210").minus_days(0), stamp("20260928_154210"));
    }

    #[test]
    fn database_backups_are_told_from_copies() {
        for yes in ["shop.bacpac", "SHOP.BAK", "shop_2026.bak"] {
            assert!(is_backup_file(Path::new(yes)), "{yes}");
        }
        for no in ["web.config.bak", "x.json.bak", "shop.zip", "bak"] {
            assert!(!is_backup_file(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn folders_are_compared_as_windows_does() {
        assert!(is_inside(Path::new(r"C:\DNN\shop\App_Data"), Path::new(r"c:\dnn\SHOP\")));
        assert!(!is_inside(Path::new(r"C:\DNN\shop"), Path::new(r"C:\DNN\shop")));
        assert!(!is_inside(Path::new(r"C:\DNN\shop2\x"), Path::new(r"C:\DNN\shop")));
        assert!(is_same_or_inside(Path::new(r"C:\DNN\shop\"), Path::new(r"C:\DNN\shop")));
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn backups_are_listed_newest_first_with_their_halves_and_notes() {
        let dir = tempfile::tempdir().unwrap();
        let make = |folder: &str, files: &[&str]| {
            let path = dir.path().join(folder);
            std::fs::create_dir_all(&path).unwrap();
            for file in files {
                std::fs::write(path.join(file), "x").unwrap();
            }
        };
        make("shop_20260101_090000", &["shop.zip", "shop.bak", "shop.bacpac"]);
        make("shop_20260301_090000", &["shop.zip"]);
        make("shop_20250101_090000", &["web.config.bak"]);
        make("by hand", &["shop.bacpac"]);
        std::fs::write(dir.path().join("shop_20260101_090000").join(NOTE_FILE), "\r\n  Before upgrading  \r\nmore")
            .unwrap();
        let backups = list(dir.path(), "shop", &|_| stamp("20200101_000000"));
        let names: Vec<_> =
            backups.iter().map(|b| b.folder.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["shop_20260301_090000", "shop_20260101_090000", "by hand"]);
        assert!(!backups[0].is_complete());
        let complete = &backups[1];
        assert!(complete.is_complete());
        assert!(complete.database.as_ref().unwrap().ends_with("shop.bacpac"), "a .bacpac before a .bak");
        assert_eq!(complete.menu_label(), "2026-01-01 09:00:00  -  Before upgrading");
        assert_eq!(backups[2].created, stamp("20200101_000000"));
        assert!(list(&dir.path().join("none"), "shop", &|_| Stamp::default()).is_empty());
        assert_eq!(
            new_folder(Path::new(r"D:\b\shop"), "shop", stamp("20260928_154210")),
            Path::new(r"D:\b\shop\shop_20260928_154210")
        );
    }
}
