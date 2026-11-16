//! BACKUP DATABASE on a (possibly external) SQL Server - a clone's live source (`Sql/RemoteSqlBackupService.cs`,
//! `IRemoteSqlBackupService`). Returns the host-side path of the `.bak` when this PC can read it.
//!
//! Deviation: tiberius doesn't surface a statement's informational messages, so BACKUP's "10 percent processed" lines
//! aren't reported while it runs.

use std::path::Path;
use std::time::Duration;

use dnnm_application::operation::{CANCELLED, CancelToken, Reporter};

use crate::address::SqlServerAddress;
use crate::client::{Conn, SqlError, block_on};
use crate::connection_strings::{explained, for_server};
use crate::model::SiteSqlConnection;
use crate::sql_text::{identifier, literal};

/// What the server did, step by step - so the reporter (not shared with the runtime) is told on this thread.
enum Outcome {
    Done(String),
    Failed(String),
}

/// Backs `source`'s database up to `backup_server_path` (a path the SQL Server service can write to: a local path for a
/// server on this PC - its own backup folder is used then - or a UNC share readable from here).
pub fn backup(
    source: &SiteSqlConnection,
    backup_server_path: &str,
    reporter: &mut dyn Reporter,
    cancel: &CancelToken,
) -> Result<String, String> {
    let windows = source.user.is_empty();
    reporter.info(&format!(
        "Connecting to source SQL Server {} as {}…",
        source.server,
        if windows { "you (Windows authentication)" } else { source.user.as_str() }
    ));
    // Straight to the target database: Azure SQL logins are often contained users that don't exist in [master].
    let database = if source.database.trim().is_empty() { "master" } else { source.database.as_str() };
    let settings = for_server(&source.server, Some(database), &source.user, Some(&source.password), 30);
    let s = source.clone();
    let path = backup_server_path.to_string();

    // Connect and look first; the BACKUP itself is a second step, so its start is known for a cancel's clean-up.
    let looked = block_on(Some(cancel), async move {
        let mut conn = Conn::open(&settings).await?;
        let version = conn.scalar("SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128))", &[]).await?;
        let edition =
            conn.scalar("SELECT CAST(SERVERPROPERTY('EngineEdition') AS int)", &[]).await?.as_i64().unwrap_or(0);
        let exists = conn
            .scalar("SELECT COUNT(*) FROM sys.databases WHERE name = @P1", &[(&s.database).into()])
            .await?
            .as_i64()
            .unwrap_or(0);
        let folder = if SqlServerAddress::is_on_this_machine(&s.server) {
            conn.scalar("SELECT CAST(SERVERPROPERTY('InstanceDefaultBackupPath') AS nvarchar(260))", &[])
                .await?
                .into_text()
        } else {
            None
        };
        Ok((conn, server_version(version.as_text().unwrap_or_default()), edition, exists, folder))
    });
    let (conn, version, edition, exists, folder) = match looked {
        Ok(found) => found,
        Err(e) if e.cancelled => return Err(CANCELLED.into()),
        Err(e) => return Err(explained(&e.message)),
    };
    reporter.success(&format!("Connected. SQL Server version: {version}"));
    // Azure SQL Database (EngineEdition 5) does not support BACKUP DATABASE ... TO DISK.
    if edition == 5 {
        return Err("Source is Azure SQL Database, which does not support BACKUP DATABASE TO DISK. \
                    Cloning from Azure SQL needs a BACPAC export (SqlPackage) instead \u{2014} not yet supported."
            .into());
    }
    if exists == 0 {
        return Err(format!("Database [{}] not found on {}.", source.database, source.server));
    }
    // A server on this PC writes where its service account may: its own backup folder. Only a folder on this PC, by
    // its full path - DNN Manager reads the file from there and deletes it afterwards.
    let mut path = path;
    if let Some(folder) = folder.filter(|f| !f.is_empty()) {
        if !is_local_folder(&folder) {
            return Err(format!(
                "{} reports its backup folder as '{folder}' - not a folder on this PC. \
                 DNN Manager only reads a backup from a local folder given by its full path.",
                source.server
            ));
        }
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        path = Path::new(&folder).join(name).display().to_string();
    }

    // Express and LocalDB (EngineEdition 4) can't compress a backup.
    reporter.info(&format!("Backing up [{}] \u{2192} {path}", source.database));
    let sql = format!(
        "BACKUP DATABASE {} TO DISK = N'{}' WITH INIT, FORMAT,{} STATS = 10;",
        identifier(&source.database),
        literal(&path),
        if edition == 4 { "" } else { " COMPRESSION," }
    );
    let target = path.clone();
    let ran = block_on(Some(cancel), async move {
        let mut conn = conn;
        conn.execute_within(&sql, &[], Duration::ZERO).await?;
        check_visible(conn, target).await
    });
    match ran {
        Ok(Outcome::Done(path)) => {
            reporter.success("Source backup written.");
            Ok(path)
        }
        Ok(Outcome::Failed(message)) => {
            reporter.success("Source backup written.");
            Err(message)
        }
        Err(e) if e.cancelled => {
            // Cancelled, not failed: a backup cut off (or finished just before) is a copy of the source's whole
            // database - not left where it was written.
            remove_backup(source, &path);
            Err(CANCELLED.into())
        }
        Err(e) => Err(explained(&e.message)),
    }
}

/// After the BACKUP: the file must be reachable from this PC - otherwise it is removed on the server if the login may.
async fn check_visible(mut conn: Conn, path: String) -> Result<Outcome, SqlError> {
    if Path::new(&path).is_file() {
        return Ok(Outcome::Done(path));
    }
    let removed = conn.execute("EXEC master.dbo.xp_delete_file 0, @P1", &[path.as_str().into()]).await.is_ok();
    Ok(Outcome::Failed(format!(
        "Backup written on the SQL Server side but not visible at {path} from this machine. \
         If the source is remote, supply a UNC share path that both the SQL service and this host can access.{}",
        if removed {
            String::new()
        } else {
            format!(" The backup is still on the server at {path} - delete it there.")
        }
    )))
}

/// Deletes the backup at `path` after a cancel - from here when this PC sees it, otherwise on the server if the login
/// may. Best effort, and bounded: a cancel mustn't wait long for it.
fn remove_backup(source: &SiteSqlConnection, path: &str) {
    if Path::new(path).is_file() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let database = if source.database.trim().is_empty() { "master" } else { source.database.as_str() };
    let settings = for_server(&source.server, Some(database), &source.user, Some(&source.password), 10);
    let path = path.to_string();
    let _ = block_on(None, async move {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut conn = Conn::open(&settings).await?;
            conn.execute_within(
                "EXEC master.dbo.xp_delete_file 0, @P1",
                &[path.as_str().into()],
                Duration::from_secs(10),
            )
            .await
        })
        .await
        .map_err(|_| SqlError::new("timed out"))?
        .map(|_| ())
    });
}

/// SqlClient's `ServerVersion` form of a product version: "16.0.4255.2" → "16.00.4255".
fn server_version(product: &str) -> String {
    let parts: Vec<u32> = product.split('.').filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [major, minor, build, ..] => format!("{major:02}.{minor:02}.{build:04}"),
        _ => product.to_string(),
    }
}

/// Whether `folder` is a folder on this PC given by its full path - `D:\SQL\Backup`, not a share, a device path or a
/// relative path.
pub fn is_local_folder(folder: &str) -> bool {
    let b = folder.as_bytes();
    b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/')
        && !folder.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_backup_folder_is_only_used_when_it_is_local() {
        for (folder, used) in [
            (r"D:\SQL\Backup", true),
            (r"C:\Program Files\Microsoft SQL Server\MSSQL16.SQLEXPRESS\MSSQL\Backup", true),
            (r"\\fileserver\share\backups", false),
            (r"\\?\C:\backups", false),
            ("backups", false),
            ("C:backups", false),
            (r"C:\backups\..\Windows", false),
        ] {
            assert_eq!(is_local_folder(folder), used, "{folder}");
        }
    }

    #[test]
    fn the_version_is_said_as_sqlclient_says_it() {
        assert_eq!(server_version("16.0.4255.2"), "16.00.4255");
        assert_eq!(server_version("12.0.2000.8"), "12.00.2000");
    }
}
