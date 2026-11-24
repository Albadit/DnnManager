//! The shared SQL Server container, through `sqlcmd` inside it (`Sql/SqlServerService.cs`, `ISqlServerService`).
//! The password reaches sqlcmd as SQLCMDPASSWORD, which `docker exec -e` takes from docker's own environment - never
//! on a command line, where Docker keeps an exec's arguments. A query with a password in it goes on sqlcmd's standard
//! input. A BACKUP or RESTORE is tagged (`-H`) so a cancel can end it in SQL Server too (pkill, then KILL).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dnnm_application::operation::{CANCELLED, CancelToken};

use crate::docker::{is_valid_name, name_refusal};
use crate::model::{DatabaseConfig, DockerOptions};
use crate::programs::{ProcessResult, Run};
use dnnm_application::use_cases::portal_aliases::{self, AliasRow, LIVE_PRIMARY_SETTING, Localized};

use crate::sql_text::{dnn_tables, identifier, literal};

const BACKUP_DIR: &str = "/var/opt/mssql/backup";
// How long sqlcmd waits to sign in: a container that runs but doesn't answer mustn't hold an operation for good.
const LOGIN_TIMEOUT_SECONDS: &str = "15";

/// -x: no $(variables) in the query, -X: no :!! commands - names from a site's web.config or a backup are text.
/// -I: QUOTED_IDENTIFIER on, as for every other client (SQL Server refuses to change a table with a filtered index,
/// like DNN's Tabs, without it).
pub const SQLCMD_SWITCHES: [&str; 6] = ["-C", "-No", "-b", "-x", "-X", "-I"];

/// How long a cancelled BACKUP or RESTORE may take to stop inside SQL Server (a RESTORE rolls back first).
pub const STOP_WITHIN: Duration = Duration::from_secs(60);
// A file in the container removed after a cancel or a failure: bounded, never cancelled itself.
const CLEANUP_WITHIN: Duration = Duration::from_secs(30);
/// How long a drop waits before it is tried again while the database is still in use.
const DROP_RETRIES: [Duration; 4] =
    [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4), Duration::from_secs(8)];

/// `ISqlServerService`: the container named in the settings, signed in to as their user.
#[derive(Clone)]
pub struct SqlServerService {
    pub docker: DockerOptions,
}

#[derive(Default)]
struct Sqlcmd<'a> {
    database: Option<&'a str>,
    plain: bool,
    tag: Option<&'a str>,
    via_stdin: bool,
    timeout: Option<Duration>,
}

fn error_text(r: &ProcessResult) -> String {
    if r.stderr.is_empty() { r.stdout.clone() } else { r.stderr.clone() }
}

impl SqlServerService {
    pub fn new(docker: DockerOptions) -> Self {
        Self { docker }
    }

    // Checked where it goes on docker's command line: a name starting with '-' would be read as an option.
    fn container(&self) -> Result<&str, String> {
        if is_valid_name(&self.docker.container_name) {
            Ok(&self.docker.container_name)
        } else {
            Err(name_refusal(&self.docker.container_name, "container").unwrap_or_default())
        }
    }

    fn sqlcmd_args(&self, query: &str, o: &Sqlcmd<'_>) -> Result<Vec<String>, String> {
        let mut args = vec!["exec".to_string()];
        if o.via_stdin {
            args.push("-i".into());
        }
        args.extend(
            [
                "-e",
                "SQLCMDPASSWORD",
                self.container()?,
                "/opt/mssql-tools18/bin/sqlcmd",
                "-S",
                "localhost",
                "-U",
                &self.docker.sql_user,
                "-l",
                LOGIN_TIMEOUT_SECONDS,
            ]
            .map(str::to_string),
        );
        args.extend(SQLCMD_SWITCHES.map(str::to_string));
        if let Some(tag) = o.tag {
            args.extend(["-H".to_string(), tag.to_string()]);
        }
        // Only the values: no header, no padding.
        if o.plain {
            args.extend(["-h", "-1", "-W"].map(str::to_string));
        }
        if let Some(database) = o.database.filter(|d| !d.is_empty()) {
            args.extend(["-d".to_string(), database.to_string()]);
        }
        if o.via_stdin {
            args.extend(["-i".to_string(), "/dev/stdin".to_string()]);
        } else {
            args.extend(["-Q".to_string(), query.to_string()]);
        }
        Ok(args)
    }

    fn sqlcmd(&self, query: &str, o: Sqlcmd<'_>, cancel: Option<&CancelToken>) -> Result<ProcessResult, String> {
        let args = self.sqlcmd_args(query, &o)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut run = Run::new("docker", &args).env("SQLCMDPASSWORD", &self.docker.sa_password);
        run.cancel = cancel;
        run.timeout = o.timeout;
        if o.via_stdin {
            run = run.stdin_text(query);
        }
        run.run()
    }

    fn docker(
        &self,
        args: &[&str],
        cancel: Option<&CancelToken>,
        timeout: Option<Duration>,
    ) -> Result<ProcessResult, String> {
        let mut run = Run::new("docker", args);
        run.cancel = cancel;
        run.timeout = timeout;
        run.run()
    }

    /// A statement that can run for long (BACKUP, RESTORE) - and that a cancel stops in SQL Server too: ending docker
    /// here leaves sqlcmd and its statement running in the container.
    fn stoppable_sqlcmd(&self, query: &str, cancel: &CancelToken) -> Result<ProcessResult, String> {
        let tag = session_tag();
        let result = self.sqlcmd(query, Sqlcmd { tag: Some(&tag), ..Default::default() }, Some(cancel));
        if matches!(&result, Err(e) if e == CANCELLED) {
            self.stop_session(&tag);
        }
        result
    }

    /// Ends the session tagged `tag` and waits until it is gone (gone for two looks in a row); false when it was still
    /// there after [`STOP_WITHIN`]. Never cancelled itself: it is what a cancel does.
    pub fn stop_session(&self, tag: &str) -> bool {
        let until = Instant::now() + STOP_WITHIN;
        let mut gone = 0;
        let Ok(container) = self.container() else { return false };
        while Instant::now() < until {
            // Best effort - an image without pkill still has the session killed below.
            let _ = self.docker(&["exec", container, "pkill", "-f", tag], None, Some(Duration::from_secs(10)));
            let r = self.sqlcmd(
                &kill_session_sql(tag),
                Sqlcmd { plain: true, timeout: Some(Duration::from_secs(20)), ..Default::default() },
                None,
            );
            match r {
                Ok(r) if r.success() && sessions_left(&r.stdout) == 0 => {
                    gone += 1;
                    if gone >= 2 {
                        return true;
                    }
                }
                _ => gone = 0,
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        false
    }

    /// Removes `container_path` in the container - after a cancel or a failure too, so never cancelled; bounded.
    fn remove_in_container(&self, container_path: &str) {
        if let Ok(container) = self.container() {
            let _ = self.docker(&["exec", container, "rm", "-f", container_path], None, Some(CLEANUP_WITHIN));
        }
    }

    /// The objectQualifier of DNN's tables in `database`, found from its table `table`; None when it has none.
    fn qualifier(&self, database: &str, table: &str, cancel: &CancelToken) -> Result<Option<String>, String> {
        let sql = format!(
            "SET NOCOUNT ON;\nSELECT TOP 1 N'Q=' + LEFT(name, LEN(name) - LEN(N'{table}')) FROM sys.tables WHERE SCHEMA_NAME(schema_id) = 'dbo'\n\
             AND (name = N'{table}' OR name LIKE N'%[_]{table}') ORDER BY LEN(name);"
        );
        let r =
            self.sqlcmd(&sql, Sqlcmd { database: Some(database), plain: true, ..Default::default() }, Some(cancel))?;
        if !r.success() {
            return Err(error_text(&r));
        }
        let Some(line) = r.stdout.lines().map(str::trim).find(|l| l.starts_with("Q=")) else { return Ok(None) };
        let qualifier = &line[2..];
        if dnn_tables::is_qualifier(qualifier) {
            Ok(Some(qualifier.to_string()))
        } else {
            Err(format!("DNN's tables in [{database}] have a prefix that isn't DNN's ('{qualifier}')."))
        }
    }

    pub fn database_exists(&self, database: &str, cancel: &CancelToken) -> Result<bool, String> {
        let q = format!(
            "SET NOCOUNT ON; IF EXISTS (SELECT 1 FROM sys.databases WHERE name = N'{}') PRINT 'EXISTS'",
            literal(database)
        );
        let r = self.sqlcmd(&q, Sqlcmd::default(), Some(cancel))?;
        if !r.success() {
            return Err(error_text(&r));
        }
        Ok(r.stdout.contains("EXISTS"))
    }

    /// Creates the database by name only; the collation can't be quoted - only a plain name is let through.
    pub fn create_database(&self, db: &DatabaseConfig, cancel: &CancelToken) -> Result<(), String> {
        if !db.collation.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("'{}' isn't a collation name.", db.collation));
        }
        let sql = format!(
            "\nIF NOT EXISTS (SELECT name FROM sys.databases WHERE name = N'{}')\nBEGIN CREATE DATABASE {} COLLATE {}; END",
            literal(&db.database_name),
            identifier(&db.database_name),
            db.collation
        );
        let r = self.sqlcmd(&sql, Sqlcmd::default(), Some(cancel))?;
        if r.success() { Ok(()) } else { Err(r.stderr) }
    }

    /// Drops the database; tried again a little later while a BACKUP or RESTORE that is just ending holds it.
    pub fn drop_database(&self, database: &str, cancel: &CancelToken) -> Result<(), String> {
        let (l, q) = (literal(database), identifier(database));
        let sql = format!(
            "
IF EXISTS (SELECT name FROM sys.databases WHERE name = N'{l}')
BEGIN
  IF EXISTS (SELECT 1 FROM sys.databases WHERE name = N'{l}' AND state_desc = N'ONLINE')
    ALTER DATABASE {q} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
  BEGIN TRY
    DROP DATABASE {q};
  END TRY
  BEGIN CATCH
    IF EXISTS (SELECT 1 FROM sys.databases WHERE name = N'{l}' AND state_desc = N'ONLINE')
      ALTER DATABASE {q} SET MULTI_USER;
    THROW;
  END CATCH
END"
        );
        let mut waits = DROP_RETRIES.iter();
        loop {
            let r = self.sqlcmd(&sql, Sqlcmd::default(), Some(cancel))?;
            if r.success() {
                return Ok(());
            }
            match waits.next() {
                Some(wait) if is_in_use(&format!("{}{}", r.stderr, r.stdout)) => sleep(*wait, cancel)?,
                _ => return Err(error_text(&r)),
            }
        }
    }

    /// Backs `database` up in the container and copies it out to DNN Manager's own temporary folder; the path of
    /// the copy, which the caller moves on. A copy of the whole database: never kept in the container.
    pub fn backup_database_local(
        &self,
        database: &str,
        backup_file_name: &str,
        cancel: &CancelToken,
    ) -> Result<PathBuf, String> {
        if !safe_file_name(backup_file_name) {
            return Err(format!("'{backup_file_name}' isn't a file name."));
        }
        let container = self.container()?.to_string();
        let container_path = format!("{BACKUP_DIR}/{backup_file_name}");
        // Not %TEMP%: the caller restores it later, and nothing unelevated may swap it meanwhile.
        let host_tmp = crate::dirs::temp()?.join(backup_file_name);
        let mut handed_on = false;
        let result = (|| {
            self.docker(&["exec", &container, "mkdir", "-p", BACKUP_DIR], Some(cancel), None)?;
            let sql = format!(
                "BACKUP DATABASE {} TO DISK = N'{}' WITH INIT, FORMAT, COMPRESSION, STATS = 10;",
                identifier(database),
                literal(&container_path)
            );
            let r = self.stoppable_sqlcmd(&sql, cancel)?;
            if !r.success() {
                return Err(r.stderr);
            }
            // Streamed through docker (cat in the container) and written by DNN Manager - a docker that runs as the
            // user couldn't write DNN Manager's admin-only folder.
            let mut file = std::fs::File::create(&host_tmp).map_err(|e| format!("{}: {e}", host_tmp.display()))?;
            let mut run = Run::new("docker", &["exec", &container, "cat", &container_path]).cancel(cancel);
            run.output = Some(&mut file);
            let cp = run.run()?;
            if !cp.success() {
                return Err(cp.stderr);
            }
            handed_on = true;
            Ok(host_tmp.clone())
        })();
        self.remove_in_container(&container_path);
        if !handed_on {
            let _ = std::fs::remove_file(&host_tmp);
        }
        result
    }

    /// Restores `backup_file_path` into `db`'s database, its files moved to names made from the database's name.
    pub fn restore_database_local(
        &self,
        db: &DatabaseConfig,
        backup_file_path: &Path,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let bak_name = backup_file_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !safe_file_name(&bak_name) {
            return Err(format!("'{bak_name}' isn't a file name."));
        }
        let container = self.container()?.to_string();
        let container_path = format!("{BACKUP_DIR}/{bak_name}");
        let result = (|| {
            self.docker(&["exec", &container, "mkdir", "-p", BACKUP_DIR], Some(cancel), None)?;
            // Read by DNN Manager and streamed in. The path goes to the shell as an argument ($1), never into its script.
            let mut file =
                std::fs::File::open(backup_file_path).map_err(|e| format!("{}: {e}", backup_file_path.display()))?;
            let mut run =
                Run::new("docker", &["exec", "-i", &container, "sh", "-c", "cat > \"$1\"", "sh", &container_path])
                    .cancel(cancel);
            run.input = crate::programs::Input::Bytes(&mut file);
            let cp = run.run()?;
            if !cp.success() {
                return Err(cp.stderr);
            }

            let list = self.sqlcmd(
                &file_list_sql(&container_path),
                Sqlcmd { plain: true, ..Default::default() },
                Some(cancel),
            )?;
            if !list.success() {
                return Err(list.stderr);
            }
            let moves = moves(&db.database_name, &list.stdout);
            if moves.is_empty() {
                return Err("Could not read backup file list.".into());
            }
            let r = self.stoppable_sqlcmd(&restore_sql(&db.database_name, &container_path, &moves), cancel)?;
            if r.success() { Ok(()) } else { Err(r.stderr) }
        })();
        // The backup's copy in the container - removed whatever happened, a cancel included.
        self.remove_in_container(&container_path);
        result
    }

    /// Gives every portal a local address, primary, for a site answering at `site_alias` (with or without a port):
    /// portal 0 that one, the others one under it; the live aliases stay as the portals' other aliases and the live
    /// primary one is noted ([`portal_aliases::localize`]). What each portal got. DNN's objectQualifier is found from
    /// the tables; the changes are one transaction.
    pub fn remap_portal_aliases(
        &self,
        database: &str,
        hostname_suffix: &str,
        site_alias: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<Localized>, String> {
        let Some(q) = self.qualifier(database, "PortalAlias", cancel)? else {
            return Err(format!("[{database}] has no DNN PortalAlias table."));
        };
        let read = self.sqlcmd(
            &read_aliases_sql(&q),
            Sqlcmd { database: Some(database), plain: true, ..Default::default() },
            Some(cancel),
        )?;
        if !read.success() {
            return Err(error_text(&read));
        }
        let (rows, portals, has_primary) = parse_aliases(&read.stdout);
        let plan = portal_aliases::localize(&rows, &portals, hostname_suffix, site_alias);
        let r = self.sqlcmd(&localize_sql(database, &q, &plan, has_primary), Sqlcmd::default(), Some(cancel))?;
        if r.success() { Ok(plan) } else { Err(error_text(&r)) }
    }

    /// The SQL login `login` with `password` - made, or its password set - and its user in `database`, owner of that
    /// database and of nothing else. The password goes on sqlcmd's standard input, never on a command line.
    pub fn grant_site_login(
        &self,
        database: &str,
        login: &str,
        password: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let (ql, ll, qd, lp) = (identifier(login), literal(login), identifier(database), literal(password));
        let sql = format!(
            "
SET NOCOUNT ON;
IF SUSER_ID(N'{ll}') IS NULL
  CREATE LOGIN {ql} WITH PASSWORD = N'{lp}', CHECK_POLICY = OFF, DEFAULT_DATABASE = {qd};
ELSE
  ALTER LOGIN {ql} WITH PASSWORD = N'{lp}', DEFAULT_DATABASE = {qd};
USE {qd};
IF USER_ID(N'{ll}') IS NULL
  CREATE USER {ql} FOR LOGIN {ql};
ELSE
  ALTER USER {ql} WITH LOGIN = {ql};
ALTER ROLE db_owner ADD MEMBER {ql};"
        );
        let r = self.sqlcmd(&sql, Sqlcmd { via_stdin: true, ..Default::default() }, Some(cancel))?;
        // sqlcmd may echo the failing statement: the password never goes into a message.
        if r.success() { Ok(()) } else { Err(error_text(&r).replace(&lp, "********").replace(password, "********")) }
    }

    /// Drops the SQL login `login` (its sessions ended first); Ok when there is none.
    pub fn drop_login(&self, login: &str, cancel: &CancelToken) -> Result<(), String> {
        let ll = literal(login);
        let sql = format!(
            "
IF SUSER_ID(N'{ll}') IS NOT NULL
BEGIN
  DECLARE @kill nvarchar(max) = N'';
  SELECT @kill += N'KILL ' + CAST(session_id AS nvarchar(10)) + N';' FROM sys.dm_exec_sessions
    WHERE login_name = N'{ll}' AND session_id <> @@SPID;
  EXEC (@kill);
  DROP LOGIN {};
END",
            identifier(login)
        );
        let r = self.sqlcmd(&sql, Sqlcmd::default(), Some(cancel))?;
        if r.success() { Ok(()) } else { Err(error_text(&r)) }
    }

    /// Turns DNN's SSL off in `database` - the portal's SSL setting and pages marked secure; how many values changed.
    pub fn disable_ssl(&self, database: &str, cancel: &CancelToken) -> Result<i64, String> {
        let Some(q) = self.qualifier(database, "PortalSettings", cancel)? else {
            return Err(format!("[{database}] has no DNN PortalSettings table."));
        };
        let r = self.sqlcmd(&disable_ssl_sql(database, &q), Sqlcmd::default(), Some(cancel))?;
        if !r.success() {
            return Err(error_text(&r));
        }
        Ok(r.stdout.rfind("CHANGED=").and_then(|at| r.stdout[at + 8..].trim().parse().ok()).unwrap_or(0))
    }
}

fn sleep(duration: Duration, cancel: &CancelToken) -> Result<(), String> {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        cancel.check()?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// A backup's file name as it goes into a path in the container: no folder, nothing the shell or SQL reads.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A workstation name nothing else uses - DNN Manager's, and this statement's only.
pub fn session_tag() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random numbers");
    format!("dnnmanager-{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// KILLs the session tagged `tag`, if there is one, and says how many such are left (`LEFT=n`).
pub fn kill_session_sql(tag: &str) -> String {
    let t = literal(tag);
    format!(
        "
SET NOCOUNT ON;
DECLARE @s int = (SELECT MIN(session_id) FROM sys.dm_exec_sessions WHERE host_name = N'{t}' AND session_id <> @@SPID);
IF @s IS NOT NULL
BEGIN
  DECLARE @kill nvarchar(40) = N'KILL ' + CAST(@s AS nvarchar(10));
  -- One already being killed (rolling back) only says how far it is.
  BEGIN TRY EXEC (@kill); END TRY BEGIN CATCH END CATCH
END
SELECT N'LEFT=' + CAST(COUNT(*) AS nvarchar(10)) FROM sys.dm_exec_sessions WHERE host_name = N'{t}' AND session_id <> @@SPID;"
    )
}

/// The `LEFT=n` of [`kill_session_sql`]'s output; -1 when it isn't there.
pub fn sessions_left(output: &str) -> i32 {
    output.lines().map(str::trim).rfind(|l| l.starts_with("LEFT=")).and_then(|l| l[5..].parse().ok()).unwrap_or(-1)
}

/// Whether sqlcmd's error says the database is in use: 3702, 3101, 5061, 5052.
pub fn is_in_use(error: &str) -> bool {
    ["Msg 3702,", "Msg 3101,", "Msg 5061,", "Msg 5052,"].iter().any(|m| error.contains(m))
        || error.to_lowercase().contains("currently in use")
}

fn file_name_part(value: &str) -> String {
    value.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

fn file_list_sql(container_path: &str) -> String {
    format!(
        "
SET NOCOUNT ON;
DECLARE @t TABLE (LogicalName nvarchar(128), PhysicalName nvarchar(260), Type char(1),
 FileGroupName nvarchar(128), Size numeric(20,0), MaxSize numeric(20,0), FileID bigint,
 CreateLSN numeric(25,0), DropLSN numeric(25,0), UniqueId uniqueidentifier,
 ReadOnlyLSN numeric(25,0), ReadWriteLSN numeric(25,0), BackupSizeInBytes bigint,
 SourceBlockSize int, FileGroupID int, LogGroupGUID uniqueidentifier,
 DifferentialBaseLSN numeric(25,0), DifferentialBaseGUID uniqueidentifier,
 IsReadOnly bit, IsPresent bit, TDEThumbprint varbinary(32), SnapshotUrl nvarchar(360));
INSERT INTO @t EXEC('RESTORE FILELISTONLY FROM DISK = N''{}''');
SELECT LogicalName + '|' + Type FROM @t;",
        literal(&literal(container_path))
    )
}

/// The MOVE clauses for a backup's files (`logical|type` lines): the logical names come from someone else's file -
/// quoted as text, and only safe characters in the file names made from them.
fn moves(database: &str, file_list: &str) -> Vec<String> {
    file_list
        .lines()
        .map(str::trim)
        .filter_map(|line| line.split_once('|'))
        .map(|(logical, kind)| {
            let logical = logical.trim();
            let ext = if kind.trim() == "L" { "_log.ldf" } else { ".mdf" };
            let file = file_name_part(&format!("{database}_{logical}")) + ext;
            format!("MOVE N'{}' TO N'/var/opt/mssql/data/{file}'", literal(logical))
        })
        .collect()
}

fn restore_sql(database: &str, container_path: &str, moves: &[String]) -> String {
    let (l, q) = (literal(database), identifier(database));
    format!(
        "
IF EXISTS (SELECT 1 FROM sys.databases WHERE name = N'{l}' AND state_desc = N'ONLINE')
  ALTER DATABASE {q} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
BEGIN TRY
  RESTORE DATABASE {q} FROM DISK = N'{}' WITH REPLACE, {}, STATS = 10;
END TRY
BEGIN CATCH
  IF EXISTS (SELECT 1 FROM sys.databases WHERE name = N'{l}' AND state_desc = N'ONLINE')
    ALTER DATABASE {q} SET MULTI_USER;
  THROW;
END CATCH
IF EXISTS (SELECT 1 FROM sys.databases WHERE name = N'{l}')
  ALTER DATABASE {q} SET MULTI_USER;",
        literal(container_path),
        moves.join(", ")
    )
}

/// Prints the PortalAlias rows (`R=id|portal|primary|alias`), the portals (`P=id`) and whether the table has IsPrimary
/// (`C=1`, DNN 7.1 and later).
fn read_aliases_sql(q: &str) -> String {
    let pa = dnn_tables::name(q, "PortalAlias");
    let portals = dnn_tables::name(q, "Portals");
    let pa_text = literal(&pa);
    let row = |primary: &str| {
        format!(
            "SELECT N'R=' + CAST(PortalAliasID AS nvarchar(20)) + N'|' + CAST(PortalID AS nvarchar(20)) + N'|' + {primary} + N'|' + HTTPAlias FROM {pa}"
        )
    };
    format!(
        "SET NOCOUNT ON;
IF COL_LENGTH(N'{pa_text}', 'IsPrimary') IS NOT NULL
BEGIN
    SELECT N'C=1';
    EXEC(N'{}');
END
ELSE
    {};
SELECT N'P=' + CAST(PortalID AS nvarchar(20)) FROM {portals};",
        literal(&row("CAST(CAST(IsPrimary AS int) AS nvarchar(1))")),
        row("N'0'")
    )
}

/// The rows, the portals and whether there is IsPrimary, from [`read_aliases_sql`]'s output.
fn parse_aliases(stdout: &str) -> (Vec<AliasRow>, Vec<i32>, bool) {
    let (mut rows, mut portals, mut has_primary) = (Vec::new(), Vec::new(), false);
    for line in stdout.lines().map(str::trim) {
        if line == "C=1" {
            has_primary = true;
        } else if let Some(p) = line.strip_prefix("P=") {
            portals.extend(p.trim().parse::<i32>());
        } else if let Some(r) = line.strip_prefix("R=") {
            let mut parts = r.splitn(4, '|');
            let (Some(id), Some(portal), Some(primary), Some(alias)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if let (Ok(id), Ok(portal)) = (id.parse(), portal.parse()) {
                rows.push(AliasRow { id, portal, alias: alias.trim().to_string(), primary: primary == "1" });
            }
        }
    }
    (rows, portals, has_primary)
}

/// The changes of [`portal_aliases::localize`]'s plan, in one transaction.
fn localize_sql(database: &str, q: &str, plan: &[Localized], has_primary: bool) -> String {
    let pa = dnn_tables::name(q, "PortalAlias");
    let settings = dnn_tables::name(q, "PortalSettings");
    let setting = literal(LIVE_PRIMARY_SETTING);
    let mut sql = format!("USE {};\nSET NOCOUNT ON;\nSET XACT_ABORT ON;\nBEGIN TRANSACTION;\n", identifier(database));
    let removed: Vec<String> = plan.iter().flat_map(|l| &l.remove).map(|id| id.to_string()).collect();
    if !removed.is_empty() {
        sql += &format!("DELETE FROM {pa} WHERE PortalAliasID IN ({});\n", removed.join(", "));
    }
    for l in plan {
        let (alias, portal) = (literal(&l.alias), l.portal);
        if let Some(id) = l.rewrite {
            sql += &format!(
                "UPDATE {pa} SET HTTPAlias = N'{alias}', LastModifiedByUserID = -1, LastModifiedOnDate = GETDATE() WHERE PortalAliasID = {id};\n"
            );
        } else if !l.present {
            sql += &if has_primary {
                format!(
                    "INSERT INTO {pa} (PortalID, HTTPAlias, BrowserType, IsPrimary, CreatedByUserID, CreatedOnDate, LastModifiedByUserID, LastModifiedOnDate) \
                     VALUES ({portal}, N'{alias}', N'Normal', 1, -1, GETDATE(), -1, GETDATE());\n"
                )
            } else {
                format!(
                    "INSERT INTO {pa} (PortalID, HTTPAlias, CreatedByUserID, CreatedOnDate, LastModifiedByUserID, LastModifiedOnDate) \
                     VALUES ({portal}, N'{alias}', -1, GETDATE(), -1, GETDATE());\n"
                )
            };
        }
        if has_primary {
            sql += &format!(
                "UPDATE {pa} SET IsPrimary = CASE WHEN HTTPAlias = N'{alias}' THEN 1 ELSE 0 END WHERE PortalID = {portal};\n"
            );
        }
        if let Some(live) = &l.live_primary {
            let live = literal(live);
            sql += &format!(
                "IF EXISTS (SELECT 1 FROM {settings} WHERE PortalID = {portal} AND SettingName = N'{setting}')\n    \
                 UPDATE {settings} SET SettingValue = N'{live}', LastModifiedByUserID = -1, LastModifiedOnDate = GETDATE() WHERE PortalID = {portal} AND SettingName = N'{setting}';\n\
                 ELSE\n    \
                 INSERT INTO {settings} (PortalID, SettingName, SettingValue, CreatedByUserID, CreatedOnDate, LastModifiedByUserID, LastModifiedOnDate) \
                 VALUES ({portal}, N'{setting}', N'{live}', -1, GETDATE(), -1, GETDATE());\n"
            );
        }
    }
    sql + "COMMIT TRANSACTION;\n"
}

fn disable_ssl_sql(database: &str, q: &str) -> String {
    let settings = dnn_tables::name(q, "PortalSettings");
    let tabs = dnn_tables::name(q, "Tabs");
    format!(
        "
USE {};
SET NOCOUNT ON;
DECLARE @changed int = 0;
IF OBJECT_ID(N'{}') IS NOT NULL
BEGIN
    UPDATE {settings} SET SettingValue = N'0'
    WHERE SettingName = N'SSLSetup' AND ISNULL(SettingValue, N'') <> N'0';
    SET @changed += @@ROWCOUNT;
    UPDATE {settings} SET SettingValue = N'False'
    WHERE SettingName IN (N'SSLEnabled', N'SSLEnforced') AND ISNULL(SettingValue, N'') NOT IN (N'False', N'false');
    SET @changed += @@ROWCOUNT;
END
IF COL_LENGTH(N'{}', N'IsSecure') IS NOT NULL
BEGIN
    UPDATE {tabs} SET IsSecure = 0 WHERE IsSecure = 1;
    SET @changed += @@ROWCOUNT;
END
PRINT 'CHANGED=' + CAST(@changed AS nvarchar(10));",
        identifier(database),
        literal(&settings),
        literal(&tabs)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> SqlServerService {
        SqlServerService::new(DockerOptions { sa_password: "Secret-123".into(), ..Default::default() })
    }

    #[test]
    fn stopping_a_session_kills_it_by_its_tag_and_counts_what_is_left() {
        let tag = session_tag();
        assert!(tag.starts_with("dnnmanager-"));
        assert_ne!(tag, session_tag());
        let sql = kill_session_sql(&tag);
        assert!(sql.contains(&format!("host_name = N'{tag}'")));
        assert!(sql.contains("KILL "));
        assert!(sql.contains("session_id <> @@SPID"));
        assert_eq!(sessions_left("LEFT=0\r\n"), 0);
        assert_eq!(sessions_left("SPID 52: transaction rollback in progress.\r\nLEFT=1\r\n"), 1);
        assert_eq!(sessions_left("Sqlcmd: Error: login timeout expired"), -1);
    }

    #[test]
    fn sqlcmd_runs_with_quoted_identifiers_on() {
        assert!(SQLCMD_SWITCHES.contains(&"-I"));
    }

    #[test]
    fn a_drop_is_tried_again_only_while_the_database_is_in_use() {
        assert!(is_in_use(
            "Msg 3702, Level 16, State 3, Server x, Line 6\nCannot drop database \"a\" because it is currently in use."
        ));
        assert!(is_in_use("Msg 5061, Level 16, State 1"));
        assert!(!is_in_use("Msg 18456, Level 14, State 1\nLogin failed for user 'sa'."));
    }

    #[test]
    fn the_password_is_never_on_the_command_line() {
        let s = service();
        let args = s.sqlcmd_args("SELECT 1", &Sqlcmd::default()).unwrap();
        assert!(!args.iter().any(|a| a.contains("Secret-123")));
        assert!(args.windows(2).any(|w| w[0] == "-e" && w[1] == "SQLCMDPASSWORD"));
        // A query with a password in it goes on standard input.
        let stdin =
            s.sqlcmd_args("CREATE LOGIN … 'Secret-123'", &Sqlcmd { via_stdin: true, ..Default::default() }).unwrap();
        assert!(!stdin.iter().any(|a| a.contains("Secret-123")));
        assert_eq!(&stdin[stdin.len() - 2..], ["-i", "/dev/stdin"]);
        let tagged = s.sqlcmd_args("BACKUP …", &Sqlcmd { tag: Some("dnnmanager-x"), ..Default::default() }).unwrap();
        assert!(tagged.windows(2).any(|w| w[0] == "-H" && w[1] == "dnnmanager-x"));
    }

    #[test]
    fn a_container_name_that_could_be_an_option_is_refused() {
        let s = SqlServerService::new(DockerOptions { container_name: "--privileged".into(), ..Default::default() });
        assert!(s.sqlcmd_args("SELECT 1", &Sqlcmd::default()).unwrap_err().contains("can't be the container's name"));
        assert_eq!(s.database_exists("x", &CancelToken::new()).unwrap_err(), s.container().unwrap_err());
    }

    #[test]
    fn names_are_quoted_in_the_sql_text() {
        let restore =
            restore_sql("x]; DROP DATABASE y; --", "/var/opt/mssql/backup/a.bak", &["MOVE N'a' TO N'b'".into()]);
        assert!(restore.contains("RESTORE DATABASE [x]]; DROP DATABASE y; --]"));
        assert!(restore.contains("name = N'x]; DROP DATABASE y; --'"));
        let moved = moves("shop", "Shop's data|D\r\nlog/../x|L\r\n");
        assert_eq!(
            moved,
            [
                "MOVE N'Shop''s data' TO N'/var/opt/mssql/data/shop_Shop_s_data.mdf'",
                "MOVE N'log/../x' TO N'/var/opt/mssql/data/shop_log____x_log.ldf'"
            ]
        );
        let read = read_aliases_sql("dnn_");
        assert!(read.contains("FROM dbo.[dnn_PortalAlias]") && read.contains("FROM dbo.[dnn_Portals]"));
        let (rows, portals, has_primary) =
            parse_aliases("C=1\r\nR=7|0|1|shop.nl\r\nR=8|1|0|a.shop.nl/x|y\r\nR=bad\r\nP=0\r\nP=1\r\n");
        assert!(has_primary);
        assert_eq!(portals, [0, 1]);
        assert_eq!(rows[1], AliasRow { id: 8, portal: 1, alias: "a.shop.nl/x|y".into(), primary: false });
        let plan = portal_aliases::localize(&rows, &portals, "dnndev.me", "o'neil.dnndev.me");
        let write = localize_sql("shop", "dnn_", &plan, true);
        assert!(write.starts_with("USE [shop];") && write.trim_end().ends_with("COMMIT TRANSACTION;"));
        assert!(write.contains("VALUES (0, N'o''neil.dnndev.me', N'Normal', 1"));
        assert!(write.contains("VALUES (1, N'portal1.o''neil.dnndev.me'"), "a path alias names no portal");
        assert!(write.contains("WHERE PortalID = 0 AND SettingName = N'DnnManager_LivePrimaryAlias'"));
        assert!(write.contains("SettingValue = N'shop.nl'"));
        assert!(disable_ssl_sql("shop", "").contains("UPDATE dbo.[Tabs] SET IsSecure = 0"));
        assert!(file_list_sql("/b/o'x.bak").contains("N''/b/o''''x.bak''"));
    }

    #[test]
    fn a_backup_file_name_is_a_name_only() {
        assert!(safe_file_name("shop_20260101.bak"));
        assert!(!safe_file_name("../etc/passwd"));
        assert!(!safe_file_name("a b.bak"));
        assert!(!safe_file_name(".hidden"));
    }
}
