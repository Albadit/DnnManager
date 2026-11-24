//! Site databases on any SQL Server (`Sql/DatabaseProvisioner.cs`, `DatabaseProvisioner.Edits.cs`): Test connection's
//! checks (the ones DNN's own installation wizard runs, and what DNN Manager needs), creating the database, letting the
//! site's Windows identity own it, dropping, renaming, and DNN's portal aliases and SSL setting.
//!
//! Every method blocks until done; a cancel ends it with "Cancelled." (the C#'s OperationCanceledException), never with
//! the "Could not …" of a failure.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use dnnm_application::operation::{CANCELLED, CancelToken};
use dnnm_application::use_cases::backups::PortalAliases;
use dnnm_application::use_cases::portal_aliases::{AliasRow, Delocalized, LIVE_PRIMARY_SETTING};

use crate::address::SqlServerAddress;
use crate::client::{Conn, Param, SqlError, Value, block_on, values};
use crate::connection_strings::for_app;
use crate::localdb::{self, product_name};
use crate::model::*;
use crate::sql_text::{dnn_tables, escape_like};

const CONNECT_TIMEOUT_SECONDS: u64 = 10;
// LocalDB may have to create and start the instance first.
const LOCAL_DB_CONNECT_TIMEOUT_SECONDS: u64 = 60;
const LONG_COMMAND: std::time::Duration = std::time::Duration::from_secs(120);

/// `IDatabaseProvisioner`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DatabaseProvisioner;

fn passed(name: &str, detail: impl Into<String>) -> DatabaseCheck {
    DatabaseCheck { name: name.into(), outcome: CheckOutcome::Passed, detail: detail.into() }
}

fn failed(name: &str, detail: impl Into<String>) -> DatabaseCheck {
    DatabaseCheck { name: name.into(), outcome: CheckOutcome::Failed, detail: detail.into() }
}

fn warning(name: &str, detail: impl Into<String>) -> DatabaseCheck {
    DatabaseCheck { name: name.into(), outcome: CheckOutcome::Warning, detail: detail.into() }
}

// Login failed, untrusted domain, account disabled, password expired / must change.
fn is_login_failure(e: &SqlError) -> bool {
    matches!(e.number, Some(18456 | 18452 | 18470 | 18487 | 18488))
}

/// The C#'s `Environment.UserDomainName\UserName`.
fn windows_user() -> String {
    format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default())
}

/// A failure as the C# words it - or "Cancelled." for a cancel.
fn fail(e: SqlError, wording: impl FnOnce(&str) -> String) -> String {
    if e.cancelled { CANCELLED.to_string() } else { wording(&e.first_line()) }
}

async fn open(connection: &DatabaseConnection, database: &str) -> Result<Conn, SqlError> {
    let timeout = if connection.kind == DatabaseKind::LocalDbFile {
        LOCAL_DB_CONNECT_TIMEOUT_SECONDS
    } else {
        CONNECT_TIMEOUT_SECONDS
    };
    Conn::open(&for_app(connection, Some(database), timeout, None)).await
}

async fn int(conn: &mut Conn, sql: &str, params: &[Param]) -> Result<Option<i64>, SqlError> {
    Ok(conn.scalar(sql, params).await?.as_i64())
}

/// The qualifier of DNN's table `table` ("" or "dnn_"), None when the database has no such table (`DnnTables.QualifierAsync`).
pub(crate) async fn qualifier(conn: &mut Conn, table: &str) -> Result<Option<String>, SqlError> {
    Ok(conn
        .scalar(dnn_tables::QUALIFIER_SQL, &[table.into()])
        .await?
        .into_text()
        .filter(|q| dnn_tables::is_qualifier(q)))
}

/// The DNN version recorded in the database, or None when it holds no DNN.
async fn dnn_version(conn: &mut Conn) -> Result<Option<String>, SqlError> {
    let Some(q) = qualifier(conn, "Version").await? else { return Ok(None) };
    Ok(conn
        .scalar(
            &format!("SELECT TOP 1 CONCAT(Major, '.', Minor, '.', Build) FROM dbo.[{q}Version] ORDER BY Major DESC, Minor DESC, Build DESC"),
            &[],
        )
        .await?
        .into_text())
}

impl DatabaseProvisioner {
    /// Test connection: is the server there, can we sign in, is it new enough, and can the database be created (or is
    /// it usable as it is). Every problem is a failed check with a message that says what to do; Err only on a cancel.
    pub fn check(
        &self,
        connection: &DatabaseConnection,
        options: &DatabaseCheckOptions,
        cancel: Option<&CancelToken>,
    ) -> Result<DatabaseCheckReport, String> {
        let c = connection.clone();
        let o = options.clone();
        let result = if connection.kind == DatabaseKind::LocalDbFile {
            check_local_db(&c, &o, cancel)
        } else {
            block_on(cancel, async move {
                let mut checks = Vec::new();
                match check_server(&c, &o, &mut checks).await {
                    Ok(()) => Ok(checks),
                    Err(e) if e.cancelled => Err(e),
                    Err(e) => {
                        checks.push(failed("Test", e.first_line()));
                        Ok(checks)
                    }
                }
            })
        };
        match result {
            Ok(checks) => Ok(DatabaseCheckReport { checks }),
            Err(e) if e.cancelled => Err(CANCELLED.into()),
            Err(e) => Ok(DatabaseCheckReport { checks: vec![failed("Test", e.first_line())] }),
        }
    }

    pub fn database_exists(
        &self,
        connection: &DatabaseConnection,
        cancel: Option<&CancelToken>,
    ) -> Result<bool, String> {
        let c = connection.clone();
        block_on(cancel, async move {
            let mut conn = open(&c, "master").await?;
            Ok(int(&mut conn, "SELECT CASE WHEN DB_ID(@P1) IS NULL THEN 0 ELSE 1 END", &[(&c.database).into()]).await?
                == Some(1))
        })
        .map_err(|e| fail(e, str::to_string))
    }

    /// Creates the (empty) database, with `collation` when given.
    pub fn create_database(
        &self,
        connection: &DatabaseConnection,
        collation: Option<&str>,
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        if let Some(collation) = collation
            && (collation.is_empty() || !collation.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
        {
            return Err(format!("'{collation}' isn't a collation name."));
        }
        let c = connection.clone();
        let collation = collation.map(str::to_string);
        block_on(cancel, async move {
            let mut conn = open(&c, "master").await?;
            // Creating can take a while on a busy server (or Azure's provisioning).
            conn.execute_within(
                "DECLARE @sql nvarchar(max) = N'CREATE DATABASE ' + QUOTENAME(@P1) + COALESCE(N' COLLATE ' + @P2, N''); EXEC (@sql);",
                &[(&c.database).into(), Param::NullableText(collation)],
                LONG_COMMAND,
            )
            .await
            .map(|_| ())
        })
        .map_err(|e| fail(e, |m| format!("Could not create [{}] on {}: {m}", connection.database, connection.server)))
    }

    /// Lets the Windows login `windows_login` (`IIS APPPOOL\site`) own the database: the login and its database user
    /// made when missing, and added to db_owner.
    pub fn grant_site_access(
        &self,
        connection: &DatabaseConnection,
        windows_login: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        let c = connection.clone();
        let login = windows_login.to_string();
        block_on(cancel, async move {
            let mut conn = open(&c, "master").await?;
            conn.execute(
                "IF SUSER_ID(@P1) IS NULL BEGIN DECLARE @sql nvarchar(max) = N'CREATE LOGIN ' + QUOTENAME(@P1) + N' FROM WINDOWS'; EXEC (@sql); END",
                &[login.as_str().into()],
            )
            .await?;
            conn.change_database(&c.database).await?;
            // The login may be in the database already - as its owner (dbo) or under another name; a sysadmin owns
            // every database anyway.
            conn.execute(
                "DECLARE @login sysname = @P1; \
                 DECLARE @user sysname = (SELECT name FROM sys.database_principals WHERE sid = SUSER_SID(@login)); \
                 IF @user = N'dbo' OR IS_SRVROLEMEMBER('sysadmin', @login) = 1 RETURN; \
                 IF @user IS NULL BEGIN DECLARE @create nvarchar(max) = N'CREATE USER ' + QUOTENAME(@login) + N' FOR LOGIN ' + QUOTENAME(@login); EXEC (@create); SET @user = @login; END; \
                 IF IS_ROLEMEMBER('db_owner', @user) = 0 BEGIN DECLARE @role nvarchar(max) = N'ALTER ROLE db_owner ADD MEMBER ' + QUOTENAME(@user); EXEC (@role); END;",
                &[login.as_str().into()],
            )
            .await
            .map(|_| ())
        })
        .map_err(|e| fail(e, |m| format!("Could not give {windows_login} access to [{}]: {m}", connection.database)))
    }

    /// Drops the database, closing the connections still open to it.
    pub fn drop_database(&self, connection: &DatabaseConnection, cancel: Option<&CancelToken>) -> Result<(), String> {
        let c = connection.clone();
        block_on(cancel, async move {
            let mut conn = open(&c, "master").await?;
            // Azure SQL Database (EngineEdition 5) has no single-user mode. A drop that fails puts the database back to
            // everyone - left single-user, the site's next connection would take it.
            conn.execute_within(
                "DECLARE @db sysname = @P1; \
                 IF DB_ID(@db) IS NOT NULL BEGIN \
                 DECLARE @azure bit = CASE WHEN CAST(SERVERPROPERTY('EngineEdition') AS int) = 5 THEN 1 ELSE 0 END; \
                 IF @azure = 0 BEGIN DECLARE @single nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@db) + N' SET SINGLE_USER WITH ROLLBACK IMMEDIATE'; EXEC (@single); END \
                 BEGIN TRY DECLARE @drop nvarchar(max) = N'DROP DATABASE ' + QUOTENAME(@db); EXEC (@drop); END TRY \
                 BEGIN CATCH IF @azure = 0 AND DB_ID(@db) IS NOT NULL BEGIN DECLARE @multi nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@db) + N' SET MULTI_USER'; EXEC (@multi); END; THROW; END CATCH \
                 END",
                &[(&c.database).into()],
                LONG_COMMAND,
            )
            .await
            .map(|_| ())
        })
        .map_err(|e| fail(e, |m| format!("Could not drop [{}] on {}: {m}", connection.database, connection.server)))
    }

    /// The site's DNN host accounts (superusers), oldest first.
    pub fn list_host_accounts(
        &self,
        connection: &DatabaseConnection,
        cancel: Option<&CancelToken>,
    ) -> Result<Vec<DnnHostAccount>, String> {
        let c = connection.clone();
        block_on(cancel, async move {
            let mut conn = open(&c, &c.database).await?;
            let Some(q) = qualifier(&mut conn, "Users").await? else { return Ok(Vec::new()) };
            let deleted = !matches!(conn.scalar(&format!("SELECT COL_LENGTH('dbo.[{q}Users]', 'IsDeleted')"), &[]).await?, Value::Null);
            let rows = conn
                .rows(
                    &format!(
                        "SELECT UserID, Username, ISNULL(Email, '') FROM dbo.[{q}Users] WHERE IsSuperUser = 1{} ORDER BY UserID",
                        if deleted { " AND IsDeleted = 0" } else { "" }
                    ),
                    &[],
                )
                .await?;
            Ok(rows
                .iter()
                .map(|r| {
                    let v = values(r);
                    DnnHostAccount {
                        user_id: v.first().and_then(Value::as_i64).unwrap_or_default() as i32,
                        user_name: v.get(1).and_then(Value::as_text).unwrap_or_default().to_string(),
                        email: v.get(2).and_then(Value::as_text).unwrap_or_default().to_string(),
                    }
                })
                .collect())
        })
        .map_err(|e| fail(e, str::to_string))
    }

    /// Renames the database to `new_name`, closing the connections still open to it.
    pub fn rename_database(
        &self,
        connection: &DatabaseConnection,
        new_name: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        let c = connection.clone();
        let new = new_name.to_string();
        block_on(cancel, async move {
            let mut conn = open(&c, "master").await?;
            conn.execute_within(
                "DECLARE @db sysname = @P1, @new sysname = @P2;                  IF DB_ID(@new) IS NOT NULL THROW 50000, N'A database of that name exists already.', 1;                  DECLARE @azure bit = CASE WHEN CAST(SERVERPROPERTY('EngineEdition') AS int) = 5 THEN 1 ELSE 0 END;                  IF @azure = 0 BEGIN DECLARE @single nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@db) + N' SET SINGLE_USER WITH ROLLBACK IMMEDIATE'; EXEC (@single); END                  BEGIN TRY                    DECLARE @rename nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@db) + N' MODIFY NAME = ' + QUOTENAME(@new); EXEC (@rename);                    IF @azure = 0 BEGIN DECLARE @multi nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@new) + N' SET MULTI_USER'; EXEC (@multi); END                  END TRY BEGIN CATCH                    IF @azure = 0 BEGIN DECLARE @back nvarchar(max) = N'ALTER DATABASE ' + QUOTENAME(@db) + N' SET MULTI_USER'; EXEC (@back); END; THROW;                  END CATCH",
                &[(&c.database).into(), new.as_str().into()],
                LONG_COMMAND,
            )
            .await
            .map(|_| ())
        })
        .map_err(|e| fail(e, |m| format!("Could not rename [{}] to [{new_name}] on {}: {m}", connection.database, connection.server)))
    }

    /// Follows changed host names in DNN's portal aliases: every alias `from` (or `from/child`) becomes `to`; each of
    /// `added` no portal has yet becomes an alias of portal 0. A LocalDB file database is opened through
    /// `site_directory` - the site must be stopped. How many aliases changed.
    pub fn update_portal_aliases(
        &self,
        connection: &DatabaseConnection,
        site_directory: &Path,
        renamed: &[(String, String)],
        added: &[String],
        cancel: Option<&CancelToken>,
    ) -> Result<i64, String> {
        let renamed: Vec<(String, String)> =
            renamed.iter().filter(|(f, t)| !f.eq_ignore_ascii_case(t)).cloned().collect();
        let added = added.to_vec();
        let work = move |conn: Conn| update_aliases(conn, renamed, added);
        let result = if connection.kind == DatabaseKind::LocalDbFile {
            localdb::with_database(site_directory, connection, cancel, work)
        } else {
            let c = connection.clone();
            block_on(cancel, async move {
                let conn = open(&c, &c.database).await?;
                work(conn).await
            })
        };
        match result {
            Ok(inner) => inner,
            Err(e) => Err(fail(e, |m| format!("Could not update the portal aliases: {m}"))),
        }
    }

    /// Makes `aliases` portal 0's only aliases, the first the primary one; child portals keep theirs.
    pub fn replace_portal_aliases(
        &self,
        connection: &DatabaseConnection,
        aliases: &[String],
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        if aliases.is_empty() {
            return Ok(());
        }
        let c = connection.clone();
        let aliases = aliases.to_vec();
        let result = block_on(cancel, async move {
            let mut conn = open(&c, &c.database).await?;
            let Some(q) = qualifier(&mut conn, "PortalAlias").await? else {
                return Ok(Err("The database has no DNN portal aliases.".to_string()));
            };
            conn.batch("BEGIN TRANSACTION;").await?;
            let outcome: Result<(), SqlError> = async {
                // Portal 0's own aliases - not its child portals' (host/child), nor other portals'.
                conn.execute(
                    &format!("DELETE FROM dbo.[{q}PortalAlias] WHERE PortalID = 0 AND CHARINDEX(N'/', HTTPAlias) = 0"),
                    &[],
                )
                .await?;
                for (i, alias) in aliases.iter().enumerate() {
                    insert_alias(&mut conn, &q, alias, i == 0).await?;
                }
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => conn.batch("COMMIT TRANSACTION;").await.map(Ok),
                Err(e) => {
                    let _ = conn.batch("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;").await;
                    Err(e)
                }
            }
        });
        match result {
            Ok(inner) => inner,
            Err(e) => Err(fail(e, |m| format!("Could not set the portal aliases: {m}"))),
        }
    }

    /// Every portal alias, and each portal's noted live primary alias ([`LIVE_PRIMARY_SETTING`]). Reads only.
    pub fn portal_aliases(
        &self,
        connection: &DatabaseConnection,
        cancel: Option<&CancelToken>,
    ) -> Result<PortalAliases, String> {
        let c = connection.clone();
        let result = block_on(cancel, async move {
            let mut conn = open(&c, &c.database).await?;
            let Some(q) = qualifier(&mut conn, "PortalAlias").await? else {
                return Ok(Err("The database has no DNN portal aliases.".to_string()));
            };
            let primary = if has_is_primary(&mut conn, &q).await? { "CAST(IsPrimary AS int)" } else { "0" };
            let rows = conn
                .rows(
                    &format!(
                        "SELECT CAST(PortalAliasID AS bigint), CAST(PortalID AS int), HTTPAlias, {primary} FROM dbo.[{q}PortalAlias]"
                    ),
                    &[],
                )
                .await?;
            let aliases = rows
                .iter()
                .filter_map(|r| {
                    let v = values(r);
                    Some(AliasRow {
                        id: v.first()?.as_i64()?,
                        portal: i32::try_from(v.get(1)?.as_i64()?).ok()?,
                        alias: v.get(2)?.clone().into_text()?,
                        primary: v.get(3).and_then(Value::as_i64) == Some(1),
                    })
                })
                .collect();
            let noted = conn
                .rows(
                    &format!(
                        "IF OBJECT_ID(N'dbo.[{q}PortalSettings]') IS NOT NULL \
                         SELECT CAST(PortalID AS int), SettingValue FROM dbo.[{q}PortalSettings] WHERE SettingName = @P1"
                    ),
                    &[LIVE_PRIMARY_SETTING.into()],
                )
                .await?
                .iter()
                .filter_map(|r| {
                    let v = values(r);
                    Some((i32::try_from(v.first()?.as_i64()?).ok()?, v.get(1)?.clone().into_text()?))
                })
                .collect();
            Ok(Ok((aliases, noted)))
        });
        match result {
            Ok(inner) => inner,
            Err(e) => Err(fail(e, |m| format!("Could not read the portal aliases: {m}"))),
        }
    }

    /// The aliases as `plan` says - local ones removed, the live primary ones primary - and the noted live primaries
    /// cleared, in one transaction.
    pub fn apply_live_aliases(
        &self,
        connection: &DatabaseConnection,
        plan: &[Delocalized],
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        let c = connection.clone();
        let plan = plan.to_vec();
        let result = block_on(cancel, async move {
            let mut conn = open(&c, &c.database).await?;
            let Some(q) = qualifier(&mut conn, "PortalAlias").await? else {
                return Ok(Err("The database has no DNN portal aliases.".to_string()));
            };
            let has_primary = has_is_primary(&mut conn, &q).await?;
            conn.batch("BEGIN TRANSACTION;").await?;
            let outcome: Result<(), SqlError> = async {
                let removed: Vec<String> = plan.iter().flat_map(|d| &d.remove).map(|id| id.to_string()).collect();
                if !removed.is_empty() {
                    conn.execute(
                        &format!("DELETE FROM dbo.[{q}PortalAlias] WHERE PortalAliasID IN ({})", removed.join(", ")),
                        &[],
                    )
                    .await?;
                }
                for d in plan.iter().filter(|_| has_primary) {
                    if let Some(id) = d.primary {
                        conn.execute(
                            &format!(
                                "UPDATE dbo.[{q}PortalAlias] SET IsPrimary = CASE WHEN PortalAliasID = {id} THEN 1 ELSE 0 END WHERE PortalID = {}",
                                d.portal
                            ),
                            &[],
                        )
                        .await?;
                    }
                }
                conn.execute(
                    &format!(
                        "IF OBJECT_ID(N'dbo.[{q}PortalSettings]') IS NOT NULL \
                         DELETE FROM dbo.[{q}PortalSettings] WHERE SettingName = @P1"
                    ),
                    &[LIVE_PRIMARY_SETTING.into()],
                )
                .await?;
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => conn.batch("COMMIT TRANSACTION;").await.map(Ok),
                Err(e) => {
                    let _ = conn.batch("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;").await;
                    Err(e)
                }
            }
        });
        match result {
            Ok(inner) => inner,
            Err(e) => Err(fail(e, |m| format!("Could not set the live portal aliases: {m}"))),
        }
    }

    /// Switches DNN's SSL setting on or off for every portal (DNN 10: SSLSetup; DNN 9: SSLEnabled and SSLEnforced).
    pub fn set_ssl(
        &self,
        connection: &DatabaseConnection,
        on: bool,
        cancel: Option<&CancelToken>,
    ) -> Result<(), String> {
        let c = connection.clone();
        let result = block_on(cancel, async move {
            let mut conn = open(&c, &c.database).await?;
            let settings = qualifier(&mut conn, "PortalSettings").await?;
            let portals = qualifier(&mut conn, "Portals").await?;
            let (Some(settings), Some(portals)) = (settings, portals) else {
                return Ok(Err("The database has no DNN portal settings.".to_string()));
            };
            let version = qualifier(&mut conn, "Version").await?.unwrap_or_default();
            let dnn10 =
                int(&mut conn, &format!("SELECT MAX(Major) FROM dbo.[{version}Version]"), &[]).await?.unwrap_or(0)
                    >= 10;
            let values: Vec<(&str, &str)> = if dnn10 {
                vec![("SSLSetup", if on { "1" } else { "0" })]
            } else {
                let v = if on { "True" } else { "False" };
                vec![("SSLEnabled", v), ("SSLEnforced", v)]
            };
            for (name, value) in values {
                conn.execute(
                    &format!(
                        "UPDATE dbo.[{settings}PortalSettings] SET SettingValue = @P2 WHERE SettingName = @P1; \
                         INSERT INTO dbo.[{settings}PortalSettings] (PortalID, SettingName, SettingValue, CreatedByUserID, CreatedOnDate, LastModifiedByUserID, LastModifiedOnDate) \
                         SELECT p.PortalID, @P1, @P2, -1, GETDATE(), -1, GETDATE() FROM dbo.[{portals}Portals] p \
                         WHERE NOT EXISTS (SELECT 1 FROM dbo.[{settings}PortalSettings] s WHERE s.PortalID = p.PortalID AND s.SettingName = @P1);"
                    ),
                    &[name.into(), value.into()],
                )
                .await?;
            }
            Ok(Ok(()))
        });
        match result {
            Ok(inner) => inner,
            Err(e) => {
                Err(fail(e, |m| format!("Could not switch DNN's SSL setting {}: {m}", if on { "on" } else { "off" })))
            }
        }
    }
}

async fn update_aliases(
    mut conn: Conn,
    renamed: Vec<(String, String)>,
    added: Vec<String>,
) -> Result<Result<i64, String>, SqlError> {
    let conn = &mut conn;
    let Some(q) = qualifier(conn, "PortalAlias").await? else {
        return Ok(Err("The database has no DNN portal aliases.".to_string()));
    };
    conn.batch("BEGIN TRANSACTION;").await?;
    let mut changed = 0i64;
    let outcome: Result<(), SqlError> = async {
        for (from, to) in &renamed {
            // host and host/child; an alias the new name has already (in that portal) is dropped instead of doubled.
            changed += conn
                .execute(
                    &format!(
                        "DELETE a FROM dbo.[{q}PortalAlias] a WHERE (a.HTTPAlias = @P1 OR a.HTTPAlias LIKE @P2) AND EXISTS                          (SELECT 1 FROM dbo.[{q}PortalAlias] b WHERE b.PortalID = a.PortalID AND b.HTTPAlias = @P3 + SUBSTRING(a.HTTPAlias, LEN(@P1) + 1, 4000));                          UPDATE dbo.[{q}PortalAlias] SET HTTPAlias = @P3 + SUBSTRING(HTTPAlias, LEN(@P1) + 1, 4000)                          WHERE HTTPAlias = @P1 OR HTTPAlias LIKE @P2;"
                    ),
                    &[from.into(), Param::Text(escape_like(from) + "/%"), to.into()],
                )
                .await? as i64;
        }
        for alias in &added {
            changed += insert_alias(conn, &q, alias, false).await?;
        }
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            conn.batch("COMMIT TRANSACTION;").await?;
            Ok(Ok(changed))
        }
        Err(e) => {
            let _ = conn.batch("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;").await;
            Err(e)
        }
    }
}

/// Adds `alias` to portal 0 unless a portal has it already; 1 when added.
/// Whether PortalAlias has IsPrimary (DNN 7.1 and later).
async fn has_is_primary(conn: &mut Conn, q: &str) -> Result<bool, SqlError> {
    Ok(!matches!(conn.scalar(&format!("SELECT COL_LENGTH('dbo.[{q}PortalAlias]', 'IsPrimary')"), &[]).await?, Value::Null))
}

async fn insert_alias(conn: &mut Conn, q: &str, alias: &str, primary: bool) -> Result<i64, SqlError> {
    // DNN 7.1 and later have BrowserType and IsPrimary; an older schema gets the columns every version has.
    let full = !matches!(
        conn.scalar(&format!("SELECT COL_LENGTH('dbo.[{q}PortalAlias]', 'IsPrimary')"), &[]).await?,
        Value::Null
    );
    let insert = if full {
        format!(
            "INSERT INTO dbo.[{q}PortalAlias] (PortalID, HTTPAlias, BrowserType, IsPrimary, CreatedByUserID, CreatedOnDate, LastModifiedByUserID, LastModifiedOnDate) \
             VALUES (0, @P1, N'Normal', @P2, -1, GETDATE(), -1, GETDATE());"
        )
    } else {
        format!(
            "INSERT INTO dbo.[{q}PortalAlias] (PortalID, HTTPAlias, CreatedByUserID, CreatedOnDate, LastModifiedByUserID, LastModifiedOnDate) \
             VALUES (0, @P1, -1, GETDATE(), -1, GETDATE());"
        )
    };
    let sql = format!("IF NOT EXISTS (SELECT 1 FROM dbo.[{q}PortalAlias] WHERE HTTPAlias = @P1) {insert}");
    let params: Vec<Param> = if full { vec![alias.into(), primary.into()] } else { vec![alias.into()] };
    Ok(conn.execute(&sql, &params).await? as i64)
}

async fn check_server(
    c: &DatabaseConnection,
    options: &DatabaseCheckOptions,
    checks: &mut Vec<DatabaseCheck>,
) -> Result<(), SqlError> {
    if c.server.trim().is_empty() {
        checks.push(failed("Server", r"Enter the SQL Server - e.g. .\SQLEXPRESS, localhost or localhost,1433."));
        return Ok(());
    }
    if !c.uses_windows_authentication() && c.user.trim().is_empty() {
        checks.push(failed("Sign in", "Enter the SQL Server login's user name."));
        return Ok(());
    }
    if !options.server_only && c.database.trim().is_empty() {
        checks.push(failed("Database", "Enter the database's name."));
        return Ok(());
    }

    let mut conn = match Conn::open(&for_app(c, Some("master"), CONNECT_TIMEOUT_SECONDS, None)).await {
        Ok(conn) => conn,
        Err(e) if e.cancelled => return Err(e),
        Err(e) if is_login_failure(&e) => {
            checks.push(passed("Server reachable", c.server.clone()));
            checks.push(failed(
                "Sign in",
                if c.uses_windows_authentication() {
                    format!(
                        "SQL Server doesn't accept {} - give that Windows account a login, or use SQL Server authentication.",
                        windows_user()
                    )
                } else {
                    format!(
                        "Login failed for '{}' - check the user name and password, and that the server allows SQL Server authentication.",
                        c.user
                    )
                },
            ));
            return Ok(());
        }
        Err(e) => {
            checks.push(failed(
                "Server reachable",
                format!(
                    "Can't reach {}: {} Check the server name, that SQL Server is running, and that it accepts TCP/IP connections.",
                    c.server,
                    e.first_line()
                ),
            ));
            return Ok(());
        }
    };

    let facts = conn
        .rows(
            "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)), CAST(SERVERPROPERTY('EngineEdition') AS int), \
             CAST(SERVERPROPERTY('Edition') AS nvarchar(128)), SUSER_SNAME()",
            &[],
        )
        .await?;
    let facts = facts.first().map(values).unwrap_or_default();
    let text = |i: usize| facts.get(i).and_then(Value::as_text).unwrap_or_default().to_string();
    let (version, edition, login) = (text(0), text(2), text(3));
    let azure = facts.get(1).and_then(Value::as_i64) == Some(5);
    let major = version.split('.').next().and_then(|m| m.parse::<i32>().ok()).unwrap_or(0);
    checks.push(passed(
        "Server reachable",
        if azure {
            format!("Azure SQL Database ({version})")
        } else {
            format!("{} {edition} ({version})", product_name(major))
        },
    ));
    checks.push(passed("Signed in", format!("as {login}")));
    if !azure && major < options.minimum_major_version {
        checks.push(failed(
            "Version",
            format!(
                "{} is too old for this DNN version - it needs {} or later.",
                product_name(major),
                product_name(options.minimum_major_version)
            ),
        ));
        return Ok(());
    }

    let may_create_sql = "SELECT HAS_PERMS_BY_NAME(NULL, NULL, 'CREATE ANY DATABASE')";
    if options.server_only {
        let may_create = int(&mut conn, may_create_sql, &[]).await? == Some(1);
        checks.push(if may_create {
            passed("Create databases", format!("{login} may create databases - new projects get theirs here."))
        } else {
            failed(
                "Create databases",
                format!("{login} may not create databases - give it the dbcreator role, or create each project's database first with {login} as its owner."),
            )
        });
        return Ok(());
    }

    let db = &c.database;
    let exists =
        int(&mut conn, "SELECT CASE WHEN DB_ID(@P1) IS NULL THEN 0 ELSE 1 END", &[db.into()]).await? == Some(1);
    if !exists {
        if azure {
            checks.push(failed(
                "Database",
                format!("[{db}] doesn't exist - create it in Azure first, with {login} as its owner."),
            ));
            return Ok(());
        }
        let can_create = int(&mut conn, may_create_sql, &[]).await? == Some(1);
        checks.push(if can_create {
            passed("Database", format!("[{db}] doesn't exist yet - it will be created."))
        } else {
            failed(
                "Database",
                format!("[{db}] doesn't exist, and {login} may not create databases - create it first with {login} as its owner, or use a login with the dbcreator role."),
            )
        });
    } else {
        if let Err(e) = conn.change_database(db).await {
            if e.cancelled {
                return Err(e);
            }
            checks.push(failed(
                "Database",
                format!("[{db}] exists, but {login} can't open it - make {login} its owner (db_owner)."),
            ));
            return Ok(());
        }
        let dnn = dnn_version(&mut conn).await?;
        let tables = int(&mut conn, "SELECT COUNT(*) FROM sys.tables WHERE is_ms_shipped = 0", &[]).await?.unwrap_or(0);
        checks.push(match dnn {
            Some(version) if options.for_new_install => failed(
                "Database",
                format!("[{db}] already holds a DNN site (DNN {version}) - choose another database, or remove that site first."),
            ),
            Some(version) => passed("Database", format!("[{db}] holds DNN {version}.")),
            None if tables > 0 && options.for_new_install => {
                failed("Database", format!("[{db}] isn't empty ({tables} tables) - DNN is installed into an empty database."))
            }
            None => passed("Database", format!("[{db}] exists and is empty - DNN will be installed into it.")),
        });

        // DNN's own permission check: its install creates tables, procedures and roles.
        let owner = int(
            &mut conn,
            "SELECT CASE WHEN IS_ROLEMEMBER('db_owner') = 1 OR IS_SRVROLEMEMBER('sysadmin') = 1 THEN 1 ELSE 0 END",
            &[],
        )
        .await?
            == Some(1);
        if !owner {
            checks.push(failed(
                "Permissions",
                format!("{login} isn't an owner (db_owner) of [{db}] - DNN creates tables, procedures and roles, so it needs db_owner."),
            ));
            return Ok(());
        }
        let ticks = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or_default() as u64;
        let table = format!("FakeTable_{ticks:016x}");
        match conn
            .execute(&format!("CREATE TABLE [dbo].[{table}]([fakeColumn] [int] NULL); SELECT * FROM [dbo].[{table}]; DROP TABLE [dbo].[{table}];"), &[])
            .await
        {
            Ok(_) => checks.push(passed("Permissions", "db_owner - can create and drop tables.")),
            Err(e) if e.cancelled => return Err(e),
            Err(e) => {
                checks.push(failed("Permissions", format!("{login} can't create a table in [{db}]: {}", e.first_line())));
                return Ok(());
            }
        }
    }

    // With Windows authentication the site signs in as its app pool's identity, which needs a login of its own.
    if let Some(site_login) = &options.site_login
        && c.uses_windows_authentication()
        && !azure
    {
        if !SqlServerAddress::is_on_this_machine(&c.server) {
            checks.push(warning(
                "Site login",
                format!(
                    "On another server the site signs in as this computer's account ({}\\{}$) - give it db_owner on [{db}] there.",
                    std::env::var("USERDOMAIN").unwrap_or_default(),
                    std::env::var("COMPUTERNAME").unwrap_or_default()
                ),
            ));
            return Ok(());
        }
        let login_exists =
            int(&mut conn, "SELECT CASE WHEN SUSER_ID(@P1) IS NULL THEN 0 ELSE 1 END", &[site_login.into()]).await?
                == Some(1);
        let may_create =
            int(&mut conn, "SELECT HAS_PERMS_BY_NAME(NULL, NULL, 'ALTER ANY LOGIN')", &[]).await? == Some(1);
        checks.push(if login_exists || may_create {
            passed(
                "Site login",
                if login_exists {
                    format!("{site_login} exists - it will be made db_owner of [{db}].")
                } else {
                    format!("{site_login} will be created and made db_owner of [{db}].")
                },
            )
        } else {
            failed(
                "Site login",
                format!("{login} may not create the login {site_login} the site signs in as - use a SQL Server administrator, or SQL Server authentication."),
            )
        });
    }
    Ok(())
}

fn check_local_db(
    c: &DatabaseConnection,
    options: &DatabaseCheckOptions,
    cancel: Option<&CancelToken>,
) -> Result<Vec<DatabaseCheck>, SqlError> {
    let mut checks = Vec::new();
    let Some(installed) = localdb::installed().keys().max().copied() else {
        checks.push(failed(
            "LocalDB",
            "SQL Server Express LocalDB isn't installed - install it (SQL Server Express setup → LocalDB, or Visual Studio), or use SQL Server.",
        ));
        return Ok(checks);
    };
    if installed < options.minimum_major_version {
        checks.push(failed(
            "LocalDB",
            format!(
                "LocalDB {} is too old for this DNN version - it needs {} or later.",
                product_name(installed),
                product_name(options.minimum_major_version)
            ),
        ));
        return Ok(checks);
    }
    checks.push(passed("LocalDB", format!("{} Express LocalDB is installed.", product_name(installed))));

    // Proves LocalDB starts on this PC; the site gets an instance of its own under its app pool identity.
    let server_connection = DatabaseConnection { kind: DatabaseKind::SqlServer, ..c.clone() };
    let settings = for_app(&server_connection, Some("master"), LOCAL_DB_CONNECT_TIMEOUT_SECONDS, None);
    match block_on(cancel, async move { Conn::open(&settings).await.map(|_| ()) }) {
        Ok(()) => checks.push(passed("Signed in", format!("as {} to {}.", windows_user(), c.server))),
        Err(e) if e.cancelled => return Err(e),
        Err(e) => {
            checks.push(failed("Signed in", format!("LocalDB didn't start: {}", e.first_line())));
            return Ok(checks);
        }
    }
    checks.push(warning(
        "Site database",
        format!(
            "App_Data\\{} runs in the LocalDB instance of the site's app pool identity - fine for trying things out; \
             DNN Manager opens it only while the site is stopped (to check the install or change the host password), and can't back it up as a .bacpac. \
             Use SQL Server for anything you keep.",
            c.database
        ),
    ));
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(server: &str, database: &str) -> DatabaseConnection {
        DatabaseConnection::new(DatabaseKind::SqlServer, server, database, SqlAuthentication::Sql).with_login("sa", "x")
    }

    #[test]
    fn what_is_missing_is_said_before_connecting() {
        let p = DatabaseProvisioner;
        let report = p.check(&connection("", "shop"), &DatabaseCheckOptions::new(true), None).unwrap();
        assert_eq!(
            report.problem().unwrap(),
            r"Server: Enter the SQL Server - e.g. .\SQLEXPRESS, localhost or localhost,1433."
        );
        let report =
            p.check(&connection("db", "shop").with_login("", ""), &DatabaseCheckOptions::new(true), None).unwrap();
        assert_eq!(report.problem().unwrap(), "Sign in: Enter the SQL Server login's user name.");
        let report = p.check(&connection("db", ""), &DatabaseCheckOptions::new(true), None).unwrap();
        assert_eq!(report.problem().unwrap(), "Database: Enter the database's name.");
    }

    #[test]
    fn a_server_that_isnt_there_fails_the_reachable_check() {
        let report = DatabaseProvisioner
            .check(&connection("127.0.0.1,1", "shop"), &DatabaseCheckOptions::new(true), None)
            .unwrap();
        let problem = report.problem().unwrap();
        assert!(problem.starts_with("Server reachable: Can't reach 127.0.0.1,1: A network-related"), "{problem}");
        assert!(problem.ends_with("and that it accepts TCP/IP connections."));
    }

    #[test]
    fn a_collation_that_isnt_a_name_is_refused_before_connecting() {
        let error = DatabaseProvisioner
            .create_database(&connection("db.invalid", "shop"), Some("Latin1_General_CI_AS; DROP DATABASE x"), None)
            .unwrap_err();
        assert_eq!(error, "'Latin1_General_CI_AS; DROP DATABASE x' isn't a collation name.");
    }

    #[test]
    fn a_failure_is_worded_as_the_csharp_and_a_cancel_stays_a_cancel() {
        let error = DatabaseProvisioner.drop_database(&connection("127.0.0.1,1", "shop"), None).unwrap_err();
        assert!(error.starts_with("Could not drop [shop] on 127.0.0.1,1: A network-related"), "{error}");
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            DatabaseProvisioner.drop_database(&connection("127.0.0.1,1", "shop"), Some(&cancel)).unwrap_err(),
            CANCELLED
        );
    }

    #[test]
    fn nothing_to_replace_is_done_at_once() {
        assert_eq!(DatabaseProvisioner.replace_portal_aliases(&connection("db.invalid", "shop"), &[], None), Ok(()));
    }
}
