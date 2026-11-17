//! Microsoft's SqlPackage: a database to a `.bacpac` and back (`Sql/SqlPackageService.cs`, `IBacpacService`) - the
//! supported way to copy an Azure SQL Database, which can't make a native .bak. Installed on first use as a .NET tool
//! in DNN Manager's own tools folder (`%ProgramData%\DnnManager\tools`), which only administrators can change.
//!
//! As in the C#, the connection string - password included - is on SqlPackage's command line: Import and Export take
//! no publish profile, and the other ways (/SourcePassword, /TargetPassword) are command-line arguments too. SqlPackage
//! runs elevated, as DNN Manager does, so programs without administrator rights can't read it; its output is never
//! put in a message with the password in it ([`hide`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::operation::{CancelToken, Reporter};

use crate::client::{Conn, block_on};
use crate::connection_strings::for_server;
use crate::model::SiteSqlConnection;
use crate::programs::{self, Resolved, Run};

/// The version installed: a known one, not whatever nuget.org has as latest the day it is first needed. Raised by hand.
pub const PINNED_VERSION: &str = "170.5.96";

const INSTALL_HINT: &str = "SqlPackage was not found. Install the .NET SDK for all users and try again - DNN Manager then installs SqlPackage itself.";

// The .NET build of SqlPackage can't resolve a collation's custom-locale LCID with ICU: Windows' NLS instead.
const SQLPACKAGE_ENV: (&str, &str) = ("DOTNET_SYSTEM_GLOBALIZATION_USENLS", "true");

/// Finds the SqlPackage executable: DNN Manager's own tools folder, then a copy on the computer's PATH that only
/// administrators can change. Never one of the user's own (a global .NET tool in ~/.dotnet/tools).
fn resolve_exe() -> Option<PathBuf> {
    if let Ok(tools) = crate::dirs::tools() {
        let tool = tools.join("sqlpackage.exe");
        if tool.is_file() {
            return Some(tool);
        }
    }
    match programs::resolve("sqlpackage.exe") {
        Resolved::Path(path) if path.is_absolute() => Some(path),
        _ => None,
    }
}

/// Makes sure SqlPackage is there, installing it (one-time) with `dotnet tool install` when it isn't.
pub fn ensure_available(reporter: &mut dyn Reporter, cancel: &CancelToken) -> Result<(), String> {
    if resolve_exe().is_some() {
        return Ok(());
    }
    reporter.info("SqlPackage not found - installing it (one-time)…");
    let tools = crate::dirs::tools()?;
    reporter.info(&format!(
        "Running: dotnet tool install Microsoft.SqlPackage --version {PINNED_VERSION} --tool-path {}",
        tools.display()
    ));
    // Only from nuget.org, by a NuGet.Config of DNN Manager's own: the user's is theirs to change.
    let nuget_config = crate::dirs::temp()?.join("sqlpackage.nuget.config");
    std::fs::write(
        &nuget_config,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<configuration>\r\n  <packageSources>\r\n    <clear />\r\n    \
         <add key=\"nuget.org\" value=\"https://api.nuget.org/v3/index.json\" protocolVersion=\"3\" />\r\n  </packageSources>\r\n  \
         <packageSourceMapping>\r\n    <packageSource key=\"nuget.org\">\r\n      <package pattern=\"*\" />\r\n    </packageSource>\r\n  \
         </packageSourceMapping>\r\n</configuration>\r\n",
    )
    .map_err(|e| format!("Could not run 'dotnet' to install SqlPackage ({e}). Install the .NET SDK for all users (so 'dotnet' is on PATH), then retry."))?;
    let tool_path = tools.display().to_string();
    let config = nuget_config.display().to_string();
    let r = Run::new(
        "dotnet",
        &[
            "tool",
            "install",
            "Microsoft.SqlPackage",
            "--version",
            PINNED_VERSION,
            "--tool-path",
            &tool_path,
            "--configfile",
            &config,
        ],
    )
    .cancel(cancel)
    .run()?;
    // `install` exits non-zero when the tool is already present: what counts is whether it can be found now.
    if resolve_exe().is_some() {
        reporter.success("SqlPackage is ready.");
        return Ok(());
    }
    if r.exit_code == -1 && r.stdout.is_empty() {
        return Err(format!(
            "Could not run 'dotnet' to install SqlPackage ({}). Install the .NET SDK for all users (so 'dotnet' is on PATH), then retry.",
            r.stderr.trim()
        ));
    }
    Err(format!(
        "Automatic SqlPackage install did not succeed: {}. Install the .NET SDK for all users (dotnet in Program Files), then retry.",
        tail(&r.stderr, &r.stdout)
    ))
}

fn connection_string(server: &str, database: &str, user: &str, password: &str) -> String {
    for_server(server, Some(database), user, Some(password), 60).connection_string()
}

// SqlPackage accepts "host,port"; the optional "tcp:" prefix dropped.
fn normalize_server(server: &str) -> String {
    let s = server.trim();
    if s.len() >= 4 && s[..4].eq_ignore_ascii_case("tcp:") { s[4..].to_string() } else { s.to_string() }
}

fn run_sqlpackage(exe: &Path, args: &[String], cancel: &CancelToken) -> Result<crate::programs::ProcessResult, String> {
    let exe = exe.display().to_string();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Run::new(&exe, &args).env(SQLPACKAGE_ENV.0, SQLPACKAGE_ENV.1).cancel(cancel).run()
}

/// Exports `source` to `bacpac_path` on this PC.
pub fn export(
    source: &SiteSqlConnection,
    bacpac_path: &Path,
    reporter: &mut dyn Reporter,
    cancel: &CancelToken,
) -> Result<(), String> {
    let exe = resolve_exe().ok_or(INSTALL_HINT)?;
    let server = normalize_server(&source.server);
    let cs = connection_string(&server, &source.database, &source.user, &source.password);
    let args = vec![
        "/Action:Export".to_string(),
        format!("/SourceConnectionString:{cs}"),
        format!("/TargetFile:{}", bacpac_path.display()),
        "/OverwriteFiles:True".to_string(),
    ];
    reporter.info(&format!("Exporting [{}] from {server} (BACPAC)", source.database));
    reporter.info("This can take several minutes for a large database…");
    let r = match run_sqlpackage(&exe, &args, cancel) {
        Ok(r) => r,
        Err(cancelled) => {
            let _ = std::fs::remove_file(bacpac_path);
            return Err(cancelled);
        }
    };
    if !r.success() {
        let (err, out) = (hide(&r.stderr, &source.password), hide(&r.stdout, &source.password));
        // A partial or zero-byte .bacpac isn't left for a later import to pick.
        let _ = std::fs::remove_file(bacpac_path);
        return Err(format!(
            "BACPAC export failed: {}{}",
            tail(&err, &out),
            unseen_login_hint(&format!("{err}{out}"), &source.user)
        ));
    }
    let size = std::fs::metadata(bacpac_path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(bacpac_path);
        return Err("BACPAC export reported success but produced no usable file.".into());
    }
    reporter.success(&format!("Exported BACPAC ({} MB).", n1(size as f64 / 1024.0 / 1024.0)));
    Ok(())
}

/// Imports a `.bacpac` into `target_server`, creating `database_name` (SqlPackage fails if it exists).
pub fn import(
    target_server: &str,
    sa_user: &str,
    sa_password: &str,
    database_name: &str,
    bacpac_path: &Path,
    reporter: &mut dyn Reporter,
    cancel: &CancelToken,
) -> Result<(), String> {
    let exe = resolve_exe().ok_or(INSTALL_HINT)?;
    let cs = connection_string(target_server, database_name, sa_user, sa_password);
    let args = vec![
        "/Action:Import".to_string(),
        format!("/TargetConnectionString:{cs}"),
        format!("/SourceFile:{}", bacpac_path.display()),
    ];
    reporter.info(&format!("Importing BACPAC into [{database_name}]"));
    reporter.info("This can take several minutes…");
    let mut r = run_sqlpackage(&exe, &args, cancel)?;

    // A partially contained database can only be created with 'contained database authentication' on: switched on,
    // the empty database the failed import left dropped, and the import run once more.
    if !r.success() && needs_contained_authentication(&format!("{}{}", r.stderr, r.stdout)) {
        reporter.info(&format!(
            "The database has users with a password of their own (a contained database) - switching on \
             'contained database authentication' on {target_server}, which SQL Server needs for it"
        ));
        if enable_contained_authentication(target_server, sa_user, sa_password, database_name, cancel).is_ok() {
            r = run_sqlpackage(&exe, &args, cancel)?;
        }
    }
    if !r.success() {
        let (err, out) = (hide(&r.stderr, sa_password), hide(&r.stdout, sa_password));
        return Err(format!("BACPAC import failed: {}", import_error(&err, &out)));
    }
    reporter.success("BACPAC imported.");
    Ok(())
}

/// `value` as .NET's "N1" writes it in the user's number format: grouped thousands, one decimal.
fn n1(value: f64) -> String {
    let (decimal, group) = dnnm_infrastructure::locale::number_separators().unwrap_or(('.', ','));
    let text = format!("{value:.1}");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, "0"));
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(group);
        }
        grouped.push(c);
    }
    format!("{grouped}{decimal}{fraction}")
}

/// SQL Server's Msg 12824: the database is (partially) contained and the server has contained database authentication off.
pub fn needs_contained_authentication(output: &str) -> bool {
    output.contains("Msg 12824")
        || output.to_lowercase().contains("'contained database authentication' must be set to 1")
}

/// Switches on 'contained database authentication' and drops the empty database the failed import left - only when it
/// has no table.
fn enable_contained_authentication(
    server: &str,
    user: &str,
    password: &str,
    database: &str,
    cancel: &CancelToken,
) -> Result<(), String> {
    let settings = for_server(server, Some("master"), user, Some(password), 30);
    let database = database.to_string();
    block_on(Some(cancel), async move {
        let mut conn = Conn::open(&settings).await?;
        conn.batch("EXEC sp_configure 'contained database authentication', 1; RECONFIGURE;").await?;
        // The name goes in as a parameter - QUOTENAME builds the statement on the server.
        conn.execute_within(
            "DECLARE @db sysname = @P1; \
             IF DB_ID(@db) IS NOT NULL BEGIN \
               DECLARE @tables int; DECLARE @count nvarchar(max) = N'SELECT @n = COUNT(*) FROM ' + QUOTENAME(@db) + N'.sys.tables'; \
               EXEC sp_executesql @count, N'@n int OUTPUT', @n = @tables OUTPUT; \
               IF @tables = 0 BEGIN DECLARE @sql nvarchar(max) = N'DROP DATABASE ' + QUOTENAME(@db); EXEC (@sql); END \
             END",
            &[database.as_str().into()],
            Duration::from_secs(120),
        )
        .await
        .map(|_| ())
    })
    .map_err(|e| e.message)
}

/// What went wrong in an import: SQL Server's own message (the "Msg …" line) when there is one - the last lines of
/// SqlPackage's output are the script it ran, not why it failed.
pub fn import_error(err: &str, out: &str) -> String {
    let text = if err.trim().is_empty() { out } else { err };
    match text.lines().map(str::trim).find(|l| l.contains("Msg ") && l.contains("Level ")) {
        Some(line) => line[line.find("Msg ").unwrap_or(0)..].to_string(),
        None => tail(err, out),
    }
}

/// `text` with `password` blanked out - for what goes into a message.
pub fn hide(text: &str, password: &str) -> String {
    if password.is_empty() { text.to_string() } else { text.replace(password, "********") }
}

/// What to do when SqlPackage met a user whose login the exporting one can't see (SQL71501).
fn unseen_login_hint(output: &str, user: &str) -> String {
    if output.contains("SQL71501") && output.to_lowercase().contains("reference to login") {
        format!(
            " The database has a user whose login {} can't see - export it signed in as a sysadmin (or a login with \
             VIEW ANY DEFINITION), or drop that user if the site doesn't use it.",
            if user.is_empty() { "your Windows account" } else { user }
        )
    } else {
        String::new()
    }
}

fn tail(err: &str, out: &str) -> String {
    let text = if err.trim().is_empty() { out } else { err };
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() { "(no output)".into() } else { lines[lines.len().saturating_sub(3)..].join(" | ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    // As SqlPackage 170 wrote it, importing a .bacpac of an Azure SQL database into the local SQL container.
    const CONTAINED_OUTPUT: &str = "*** Error importing database:Could not import package.\r\n\
        Error SQL72014: Core Microsoft SqlClient Data Provider: Msg 12824, Level 16, State 1, Line 5 The sp_configure value \
        'contained database authentication' must be set to 1 in order to alter a contained database.  You may need to use \
        RECONFIGURE to set the value_in_use.\r\n\
        Error SQL72045: Script execution error.  The executed script:\r\n\
        IF EXISTS (SELECT 1\r\n           FROM   [master].[dbo].[sysdatabases]\r\n           WHERE  [name] = N'$(DatabaseName)')\r\n\
            BEGIN\r\n        ALTER DATABASE [$(DatabaseName)]\r\n            SET CONTAINMENT = PARTIAL \r\n\
                    WITH ROLLBACK IMMEDIATE;\r\n    END\r\n";

    #[test]
    fn a_contained_database_is_recognised() {
        assert!(needs_contained_authentication(CONTAINED_OUTPUT));
        assert!(!needs_contained_authentication(
            "Error SQL72014: Core Microsoft SqlClient Data Provider: Msg 1801, Level 16, State 3, Line 1 Database 'shop' already exists."
        ));
    }

    #[test]
    fn the_message_is_sql_servers_error_not_the_scripts_last_lines() {
        let message = import_error(CONTAINED_OUTPUT, "");
        assert!(message.starts_with("Msg 12824, Level 16"), "{message}");
        assert!(message.contains("'contained database authentication' must be set to 1"));
        assert!(!message.contains("ROLLBACK"));
    }

    #[test]
    fn without_a_sql_server_error_the_last_lines_are_the_message() {
        assert_eq!(import_error("", "zero\nfirst\nsecond\nthird\n"), "first | second | third");
        assert_eq!(tail("", ""), "(no output)");
    }

    #[test]
    fn the_password_never_goes_into_a_message() {
        assert_eq!(hide("Login failed: pw=S3cret! for sa", "S3cret!"), "Login failed: pw=******** for sa");
        assert_eq!(hide("x", ""), "x");
    }

    #[test]
    fn an_unseen_login_says_what_to_do() {
        let output = "Error SQL71501: User: [web] has an unresolved reference to Login [web].";
        assert!(unseen_login_hint(output, "").contains("your Windows account can't see"));
        assert_eq!(unseen_login_hint("other", "sa"), "");
    }

    #[test]
    fn sizes_are_written_with_one_decimal() {
        let (decimal, group) = dnnm_infrastructure::locale::number_separators().unwrap_or(('.', ','));
        assert_eq!(n1(1234.56), format!("1{group}234{decimal}6"));
        assert_eq!(n1(0.04), format!("0{decimal}0"));
    }

    #[test]
    fn the_server_is_given_without_tcp() {
        assert_eq!(normalize_server(" tcp:db.example.com,1433 "), "db.example.com,1433");
        let cs = connection_string("localhost,1433", "shop", "sa", "p;w");
        assert!(cs.starts_with("Data Source=127.0.0.1,1433;Encrypt=True;Trust Server Certificate=True;Connect Timeout=60;Initial Catalog=shop;"));
        assert!(cs.ends_with("Password=\"p;w\""));
    }
}
