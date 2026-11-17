//! SQL Server Express LocalDB (`Sql/LocalDbVersions.cs`, `Sql/LocalDbFiles.cs`).
//!
//! How it is reached: the C# connects with SqlClient, which asks LocalDB's instance API (`SqlUserInstance.dll`,
//! `LocalDBStartInstance`) to start the instance and hand it the instance's named pipe - LocalDB listens on nothing
//! else. This port does the same: it loads the newest installed LocalDB's instance API (its path from HKLM, checked to
//! be one only administrators can change when DNN Manager runs elevated), starts the instance, and speaks TDS over
//! that pipe with tiberius. Making a DNN Manager instance of another version stays `SqlLocalDB.exe create`, as in the
//! C#.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dnnm_application::operation::CancelToken;

use crate::client::{Conn, SqlError, block_on};
use crate::connection_strings::{self, for_server};
use crate::model::DatabaseConnection;
use crate::programs::Run;

// The newest database file version each LocalDB major opens - and gives a file it opens: attaching upgrades it.
const FILE_VERSIONS: [(i32, i32); 7] = [(11, 706), (12, 782), (13, 852), (14, 869), (15, 904), (16, 957), (17, 998)];

// Where a database file keeps its version: dbi_version on its boot page (page 9, after the 96-byte page header).
const VERSION_OFFSET: u64 = 0x12064;

const INSTALLED_KEY: &str = r"SOFTWARE\Microsoft\Microsoft SQL Server Local DB\Installed Versions";

/// The installed LocalDB majors (15 = 2019, 17 = 2025), each with its instance API DLL - from the registry.
pub fn installed() -> BTreeMap<i32, String> {
    let mut found = BTreeMap::new();
    for name in crate::programs::registry::local_machine_subkeys(INSTALLED_KEY) {
        let Some(major) = name.split('.').next().and_then(|m| m.parse::<i32>().ok()).filter(|m| *m > 0) else {
            continue;
        };
        let api = crate::programs::registry::local_machine(&format!(r"{INSTALLED_KEY}\{name}"), "InstanceAPIPath");
        found.insert(major, api.unwrap_or_default());
    }
    found
}

/// The newest file version `major` opens - any for a LocalDB newer than this list.
pub fn max_file_version(major: i32) -> i32 {
    if major > FILE_VERSIONS[FILE_VERSIONS.len() - 1].0 {
        return i32::MAX;
    }
    FILE_VERSIONS.iter().find(|(m, _)| *m == major).map_or(0, |(_, v)| *v)
}

/// The LocalDB major that makes files of `file_version` - the oldest that opens it (998 → 17).
pub fn major_for(file_version: i32) -> i32 {
    FILE_VERSIONS
        .iter()
        .find(|(_, v)| *v >= file_version)
        .map_or(FILE_VERSIONS[FILE_VERSIONS.len() - 1].0 + 1, |(m, _)| *m)
}

/// The LocalDB to open a file of `file_version` in, of those installed: the oldest that opens it. None when none does.
pub fn major_to_open(file_version: i32, installed: impl IntoIterator<Item = i32>) -> Option<i32> {
    installed.into_iter().filter(|m| max_file_version(*m) >= file_version).min()
}

/// A database file's version (611 = SQL Server 2005, 998 = 2025) from its header; None when the file is too short.
/// An error while SQL Server holds the file.
pub fn file_version_of(file: &Path) -> std::io::Result<Option<i32>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut stream = open_shared(file)?;
    if stream.metadata()?.len() < VERSION_OFFSET + 2 {
        return Ok(None);
    }
    stream.seek(SeekFrom::Start(VERSION_OFFSET))?;
    let mut bytes = [0u8; 2];
    stream.read_exact(&mut bytes)?;
    let version = i16::from_le_bytes(bytes);
    Ok((version > 0).then_some(i32::from(version)))
}

#[cfg(windows)]
fn open_shared(file: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, as the C#'s FileShare.ReadWrite | Delete.
    std::fs::OpenOptions::new().read(true).share_mode(0x7).open(file)
}

#[cfg(not(windows))]
fn open_shared(file: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(file)
}

/// SqlLocalDB.exe of the LocalDB whose instance API is `instance_api` - or the one on PATH.
pub fn sql_local_db_exe(instance_api: &str) -> String {
    // ...\170\LocalDB\Binn\SqlUserInstance.dll → ...\170\Tools\Binn\SqlLocalDB.exe
    let root = Path::new(instance_api).parent().and_then(Path::parent).and_then(Path::parent);
    match root.map(|r| r.join("Tools").join("Binn").join("SqlLocalDB.exe")) {
        Some(exe) if exe.is_file() => exe.display().to_string(),
        _ => "SqlLocalDB.exe".into(),
    }
}

/// The product name of a SQL Server major version (`DatabaseProvisioner.ProductName`).
pub fn product_name(major: i32) -> String {
    match major {
        m if m >= 17 => "SQL Server 2025".into(),
        16 => "SQL Server 2022".into(),
        15 => "SQL Server 2019".into(),
        14 => "SQL Server 2017".into(),
        13 => "SQL Server 2016".into(),
        12 => "SQL Server 2014".into(),
        11 => "SQL Server 2012".into(),
        m => format!("SQL Server {m}"),
    }
}

// ─── Reaching an instance ───────────────────────────────────────────────────

/// The named pipe of the current user's LocalDB instance `instance` (`MSSQLLocalDB`), started when it isn't running
/// - what SqlClient does for `(localdb)\…`.
pub(crate) async fn pipe_for(instance: &str) -> Result<String, SqlError> {
    let instance = instance.to_string();
    tokio::task::spawn_blocking(move || start_instance(&instance))
        .await
        .map_err(|e| SqlError::new(format!("LocalDB: {e}")))?
}

#[cfg(windows)]
fn start_instance(instance: &str) -> Result<String, SqlError> {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
    use windows::core::{HSTRING, PCWSTR, s};

    let installed = installed();
    let Some((_, api)) = installed.iter().rev().find(|(_, api)| !api.is_empty()) else {
        return Err(SqlError::new(
            "A network-related or instance-specific error occurred while establishing a connection to SQL Server. \
             (LocalDB: SQL Server Express LocalDB isn't installed.)",
        ));
    };
    // Loaded into an elevated process: only a copy administrators alone can change.
    if crate::programs::is_elevated()
        && crate::programs::trust::admin_only(Path::new(api)) != crate::programs::trust::Trust::AdminOnly
    {
        return Err(SqlError::new(format!(
            "LocalDB's instance API {api} could be changed by programs without administrator rights - DNN Manager doesn't load it."
        )));
    }
    type StartInstance = unsafe extern "system" fn(PCWSTR, u32, *mut u16, *mut u32) -> windows::core::HRESULT;
    unsafe {
        let module = LoadLibraryExW(&HSTRING::from(api.as_str()), None, LOAD_WITH_ALTERED_SEARCH_PATH)
            .map_err(|e| SqlError::new(format!("LocalDB's instance API can't be loaded: {e}")))?;
        let result = (|| {
            let start = GetProcAddress(module, s!("LocalDBStartInstance"))
                .ok_or_else(|| SqlError::new("LocalDB's instance API has no LocalDBStartInstance."))?;
            let start: StartInstance = std::mem::transmute(start);
            let mut buffer = vec![0u16; 261];
            let mut length = buffer.len() as u32;
            let name = HSTRING::from(instance);
            let hr = start(PCWSTR(name.as_ptr()), 0, buffer.as_mut_ptr(), &mut length);
            if hr.is_err() {
                return Err(SqlError::new(format!(
                    "A network-related or instance-specific error occurred while establishing a connection to SQL Server. \
                     (LocalDB: the instance {instance} didn't start - {})",
                    windows::core::Error::from(hr).message()
                )));
            }
            let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            let pipe = String::from_utf16_lossy(&buffer[..end]);
            Ok(pipe.strip_prefix("np:").unwrap_or(&pipe).to_string())
        })();
        let _ = FreeLibrary(module);
        result
    }
}

#[cfg(not(windows))]
fn start_instance(_instance: &str) -> Result<String, SqlError> {
    Err(SqlError::new("LocalDB is only available on Windows."))
}

// ─── A site's LocalDB file (LocalDbFiles) ───────────────────────────────────

// The site may have to be stopped a moment before LocalDB lets go of the file.
const OPEN_FOR: Duration = Duration::from_secs(60);

/// The database file's full path for a site in `site_directory`.
pub fn path_of(site_directory: &Path, connection: &DatabaseConnection) -> PathBuf {
    site_directory.join("App_Data").join(&connection.database)
}

/// Runs `work` on the site's database file - attached to DNN Manager's own LocalDB instance of the file's version
/// for the moment, the site stopped - then detaches it and gives the files their folder's permissions back.
pub fn with_database<T: Send + 'static, F: Future<Output = Result<T, SqlError>> + Send + 'static>(
    site_directory: &Path,
    connection: &DatabaseConnection,
    cancel: Option<&CancelToken>,
    work: impl FnOnce(Conn) -> F + Send + 'static,
) -> Result<T, SqlError> {
    let file = path_of(site_directory, connection);
    if !file.is_file() {
        return Err(SqlError::new(format!("The site's database file isn't there: {}", file.display())));
    }
    let until = Instant::now() + OPEN_FOR;
    let server = server_for(&connection.server, &file, until, cancel)?;
    let log = PathBuf::from(file.with_extension("").display().to_string() + "_log.ldf");
    // Attaching and detaching give a file permissions for the account that did it: the folder's first.
    let _ = reset_permissions(&file);
    let _ = reset_permissions(&log);
    let mut settings = for_server(&server, None, "", None, 60);
    settings.attach_file = Some(file.clone());
    let result = block_on(cancel, async move {
        loop {
            match Conn::open(&settings).await {
                Ok(conn) => return work(conn).await,
                // In use by the site's own instance - it closes a database a moment after the site lets go.
                Err(e) if matches!(e.number, Some(5120 | 1832 | 32)) && Instant::now() < until => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => return Err(e),
            }
        }
    });
    detach(&server, &file);
    let _ = reset_permissions(&file);
    let _ = reset_permissions(&log);
    result
}

/// The LocalDB to open `file` in: `configured` when it is the version the file needs, else DNN Manager's own instance
/// of that version - `(LocalDB)\DnnManager17`, made the first time.
fn server_for(configured: &str, file: &Path, until: Instant, cancel: Option<&CancelToken>) -> Result<String, SqlError> {
    if !connection_strings::is_local_db(configured) {
        return Ok(configured.to_string());
    }
    let installed = installed();
    if installed.is_empty() {
        return Ok(configured.to_string());
    }
    let file_version = loop {
        match file_version_of(file) {
            Ok(version) => break version,
            Err(_) if Instant::now() < until => {
                if cancel.is_some_and(CancelToken::is_cancelled) {
                    return Err(SqlError::new(dnnm_application::operation::CANCELLED));
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            Err(_) => break None,
        }
    };
    let Some(file_version) = file_version else { return Ok(configured.to_string()) };
    let Some(major) = major_to_open(file_version, installed.keys().copied()) else {
        return Err(SqlError::new(format!(
            "{} is a {} database - newer than any LocalDB on this PC (the newest is {}). Install that version's LocalDB to open it.",
            file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            product_name(major_for(file_version)),
            product_name(installed.keys().max().copied().unwrap_or_default())
        )));
    };
    if major_version_of(configured) == Some(major) {
        return Ok(configured.to_string());
    }
    let server = format!(r"(LocalDB)\DnnManager{major}");
    ensure_instance(&server, major, &installed[&major], cancel)?;
    Ok(server)
}

fn master(server: &str) -> crate::connection_strings::ConnectionSettings {
    for_server(server, Some("master"), "", None, 60)
}

/// The major version of the LocalDB instance `server`; None when it doesn't start.
fn major_version_of(server: &str) -> Option<i32> {
    let settings = master(server);
    block_on(None, async move {
        let mut conn = Conn::open(&settings).await?;
        conn.scalar("SELECT CAST(SERVERPROPERTY('ProductMajorVersion') AS int)", &[]).await
    })
    .ok()
    .and_then(|v| v.as_i64())
    .map(|v| v as i32)
}

/// The current user's LocalDB instance `server` at version `major` - made the first time. `SqlLocalDB create` answers
/// exit code 0 even when it fails, so what counts is that the instance then answers at that version.
fn ensure_instance(server: &str, major: i32, instance_api: &str, cancel: Option<&CancelToken>) -> Result<(), SqlError> {
    if major_version_of(server) == Some(major) {
        return Ok(());
    }
    let name = server.split_once('\\').map_or(server, |(_, n)| n);
    let exe = sql_local_db_exe(instance_api);
    let version = format!("{major}.0");
    let mut run = Run::new(&exe, &["create", name, &version]);
    run.cancel = cancel;
    run.keep_children = true;
    let output = run.run().map_err(SqlError::new)?;
    if major_version_of(server) != Some(major) {
        let text = if output.stderr.trim().is_empty() { &output.stdout } else { &output.stderr };
        let last = text.trim().lines().last().unwrap_or_default().trim().to_string();
        return Err(SqlError::new(format!(
            "Could not make the LocalDB instance {name} ({}): {last}",
            product_name(major)
        )));
    }
    Ok(())
}

/// Detaches the database whose data file is `file` from `server`, if it is attached there - never cancelled.
pub fn detach(server: &str, file: &Path) {
    let settings = master(server);
    let file = file.display().to_string();
    let _ = block_on(None, async move {
        let mut conn = Conn::open(&settings).await?;
        let name = conn
            .scalar(
                "SELECT TOP 1 DB_NAME(database_id) FROM sys.master_files WHERE type = 0 AND LOWER(physical_name) = LOWER(@P1)",
                &[file.as_str().into()],
            )
            .await?;
        let Some(name) = name.into_text() else { return Ok(()) };
        conn.execute(
            "DECLARE @db sysname = @P1; DECLARE @single nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@db) + N' SET SINGLE_USER WITH ROLLBACK IMMEDIATE'; EXEC (@single); \
             EXEC sp_detach_db @dbname = @db;",
            &[name.as_str().into()],
        )
        .await
        .map(|_| ())
    });
}

/// Gives `file` its folder's permissions back: what a detach set on it dropped, and inheriting again.
#[cfg(windows)]
pub fn reset_permissions(file: &Path) -> Result<(), String> {
    use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows::Win32::Security::{
        ACL, ACL_REVISION, DACL_SECURITY_INFORMATION, InitializeAcl, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::core::HSTRING;
    if !file.is_file() {
        return Ok(());
    }
    unsafe {
        let mut acl = vec![0u64; 8];
        let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
        InitializeAcl(acl_ptr, 64, ACL_REVISION).map_err(|e| e.to_string())?;
        let result = SetNamedSecurityInfoW(
            &HSTRING::from(file.as_os_str()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl_ptr),
            None,
        );
        if result.is_err() { Err(windows::core::Error::from(result.to_hresult()).message()) } else { Ok(()) }
    }
}

#[cfg(not(windows))]
pub fn reset_permissions(_file: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_opened_in_the_local_db_of_its_own_version() {
        let both = [15, 17];
        assert_eq!(
            major_to_open(998, both),
            Some(17),
            "A file the site's 2025 instance made: 2025, not the user's 2019."
        );
        assert_eq!(major_to_open(904, both), Some(15), "A 2019 file isn't upgraded to 2025 behind the site's back.");
        assert_eq!(
            major_to_open(611, both),
            Some(15),
            "DNN's shipped file (SQL Server 2005): the oldest that opens it."
        );
        assert_eq!(major_to_open(611, [17]), Some(17));
        assert_eq!(major_to_open(998, [15]), None, "Only 2019 here: nothing can open a 2025 file.");
        assert_eq!(major_to_open(1100, [17, 18]), Some(18), "A LocalDB newer than the list opens anything.");
    }

    #[test]
    fn a_file_version_names_the_local_db_that_made_it() {
        assert_eq!(major_for(998), 17);
        assert_eq!(major_for(904), 15);
        assert_eq!(major_for(611), 11);
        assert_eq!(major_for(1100), 18);
    }

    #[test]
    fn the_version_is_read_from_the_file_header() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Database.mdf");
        // dbi_version at 0x12064, as SQL Server writes it - 998 for a file LocalDB 2025 made.
        let mut bytes = vec![0u8; 0x12100];
        bytes[0x12064..0x12066].copy_from_slice(&998i16.to_le_bytes());
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(file_version_of(&file).unwrap(), Some(998));
        std::fs::write(&file, [0u8; 100]).unwrap();
        assert_eq!(file_version_of(&file).unwrap(), None, "Too short for a header: not a version.");
    }

    #[test]
    fn product_names() {
        assert_eq!(product_name(16), "SQL Server 2022");
        assert_eq!(product_name(18), "SQL Server 2025");
        assert_eq!(product_name(10), "SQL Server 10");
    }

    #[test]
    fn sql_local_db_exe_is_beside_the_instance_api_or_on_path() {
        assert_eq!(sql_local_db_exe(r"C:\nowhere\170\LocalDB\Binn\SqlUserInstance.dll"), "SqlLocalDB.exe");
    }

    #[test]
    fn resetting_a_files_permissions_lets_it_inherit_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Database.mdf");
        std::fs::write(&file, b"x").unwrap();
        reset_permissions(&file).unwrap();
        assert!(std::fs::read(&file).is_ok(), "the folder's rules let its owner in");
    }
}
