//! The project menu's "Open with ▸" (`Pages/Projects/ProjectMenu.cs`'s `OpenWithMenu`, `FillOpenWith`, `OpenInIde`,
//! `OpenDatabase` and `LeavePasswordOnClipboard`): the code editors and IDEs found on this PC for the project's folder,
//! then SQL Server Management Studio for its database - nothing listed that isn't installed.
//!
//! - [`locate`]: what is installed (`Services/IdeLocator.cs`).
//! - [`unelevated`]: an editor starts as the signed-in user (`Unelevated.cs`).
//! - [`ssms`], [`connect_dialog`], [`secret_clipboard`]: SSMS on a database (`IdeLocator`, `SsmsConnectDialog.cs`,
//!   `SecretClipboard.cs`).
//!
//! Threading: nothing here may run on the UI thread. [`open_with_entries`] waits for vswhere the first time (looked up
//! once per run - [`warm_up`] starts that early, as the Projects page does) and reads the project's folder; [`open`]
//! starts programs and, for SSMS, signs in to the database and waits up to 90 s for its Connect dialog.
//!
//! The programs are Windows ones: on macOS and Linux nothing is looked for ([`other`]), so the menu is greyed.

#[cfg(windows)]
pub mod connect_dialog;
#[cfg(windows)]
pub mod locate;
#[cfg(windows)]
pub mod secret_clipboard;
#[cfg(windows)]
pub mod ssms;
#[cfg(windows)]
pub mod unelevated;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
pub use other::{locate, secret_clipboard};

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use dnnm_sql::address::SqlServerAddress;
use dnnm_sql::model::{DatabaseServerOptions, SiteSqlConnection};
use dnnm_sql::tester::SqlConnectionTester;

pub use locate::Ide;
#[cfg(windows)]
pub use unelevated::start_unelevated;

/// The submenu's title.
pub const OPEN_WITH: &str = "Open with…";
/// What the submenu holds until it is filled.
pub const READING: &str = "Reading…";
/// The tooltip of the greyed "Open with…" when nothing is installed.
pub const NOTHING_FOUND: &str = "No code editor, IDE or SQL Server Management Studio found on this PC.";
const NO_DATABASE: &str = "The site's web.config names no database.";

/// One line of the "Open with…" submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWithEntry {
    pub label: String,
    /// The program's path - or, for a greyed entry, why it is greyed (also shown when disabled, and the screen reader's
    /// help text).
    pub tooltip: Option<String>,
    pub kind: EntryKind,
}

/// What an entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// Opens the project's folder (or its only solution) in this editor - [`open`].
    Ide(Ide),
    /// The line between the editors and SSMS.
    Separator,
    /// Greyed: a copy of SSMS DNN Manager doesn't start as Administrator, or "Project database" when the site's
    /// web.config names none. The tooltip says why.
    Disabled,
    /// An SSMS, with its submenu: "Default  (local SQL Server, <user>)" and "Project  ([<db>])".
    Ssms(Vec<OpenWithEntry>),
    /// Opens SSMS on the local SQL Server (`project` false) or the project's database - [`open`].
    Database { ssms: Ide, project: bool },
}

/// Starts looking up what is installed, so the first right-click doesn't wait for vswhere (the Projects page does this
/// when it loads).
pub fn warm_up() {
    std::thread::spawn(|| {
        locate::installed();
        locate::management_studios();
    });
}

/// The greyed "Open with…"'s tooltip ([`NOTHING_FOUND`]) when no editor, IDE or SSMS is installed; None when it is
/// offered. Waits for the lookup the first time.
pub fn nothing_found() -> Option<&'static str> {
    (locate::installed().is_empty() && locate::management_studios().is_empty()).then_some(NOTHING_FOUND)
}

/// The submenu for a project in `project_dir`: the editors found (Visual Studio and Rider name the solution they open),
/// then each SSMS with its Default and Project entries. `project_database` is the database the site's web.config names
/// (None when it names none or can't be read), `database_tip` the Projects row's note on its database, `sql_user` the
/// local container's login (Settings → Database server).
pub fn open_with_entries(
    project_dir: &Path,
    project_database: Option<&str>,
    database_tip: Option<&str>,
    sql_user: &str,
) -> Vec<OpenWithEntry> {
    let studios: Vec<(Ide, Option<String>)> =
        locate::management_studios().iter().map(|s| (s.clone(), locate::refusal(s))).collect();
    entries(
        locate::installed(),
        locate::solution_for(project_dir).as_deref(),
        &studios,
        project_database.filter(|d| !d.is_empty()),
        database_tip,
        sql_user,
    )
}

/// [`open_with_entries`] from what was found: `studios` with their refusals.
pub fn entries(
    ides: &[Ide],
    solution: Option<&Path>,
    studios: &[(Ide, Option<String>)],
    project_database: Option<&str>,
    database_tip: Option<&str>,
    sql_user: &str,
) -> Vec<OpenWithEntry> {
    let solution_name = solution.and_then(|s| s.file_name()).map(|n| n.to_string_lossy().into_owned());
    let mut menu: Vec<OpenWithEntry> = ides
        .iter()
        .map(|ide| {
            let label = match (&solution_name, ide.opens_solution) {
                (Some(sln), true) => format!("{}  ({sln})", ide.name),
                _ => ide.name.clone(),
            };
            OpenWithEntry {
                label,
                tooltip: Some(ide.exe_path.display().to_string()),
                kind: EntryKind::Ide(ide.clone()),
            }
        })
        .collect();

    if studios.is_empty() {
        return menu;
    }
    if !menu.is_empty() {
        menu.push(OpenWithEntry { label: String::new(), tooltip: None, kind: EntryKind::Separator });
    }
    for (ssms, refusal) in studios {
        // A copy DNN Manager doesn't start as Administrator (others could change it): greyed, saying why - not a failure
        // on the click.
        if let Some(refusal) = refusal {
            menu.push(OpenWithEntry {
                label: ssms.name.clone(),
                tooltip: Some(refusal.clone()),
                kind: EntryKind::Disabled,
            });
            continue;
        }
        // Default: the local SQL Server as the container's user. Project: this project's database with its own login.
        let default = OpenWithEntry {
            label: format!("Default  (local SQL Server, {sql_user})"),
            tooltip: None,
            kind: EntryKind::Database { ssms: ssms.clone(), project: false },
        };
        // The database the site's web.config names - none to open when it names none (the tooltip says why).
        let project = match project_database {
            Some(db) => OpenWithEntry {
                label: format!("Project  ([{db}])"),
                tooltip: None,
                kind: EntryKind::Database { ssms: ssms.clone(), project: true },
            },
            None => OpenWithEntry {
                label: "Project database".into(),
                tooltip: Some(database_tip.unwrap_or(NO_DATABASE).to_string()),
                kind: EntryKind::Disabled,
            },
        };
        menu.push(OpenWithEntry {
            label: ssms.name.clone(),
            tooltip: Some(ssms.exe_path.display().to_string()),
            kind: EntryKind::Ssms(vec![default, project]),
        });
    }
    menu
}

/// What [`open`] needs of the project and DNN Manager's settings.
pub struct OpenContext<'a> {
    pub project_name: &'a str,
    pub project_dir: &'a Path,
    /// The local SQL Server as the container's user (`LocalSqlContainer.DefaultConnection`).
    pub default_connection: SiteSqlConnection,
    /// The project's database as its web.config has it (`LocalSqlContainer.SiteConnectionOf`); None when it has none.
    pub project_connection: Option<SiteSqlConnection>,
    /// The Projects row's note on its database (`ProjectRow.DatabaseTip`).
    pub database_tip: Option<&'a str>,
    /// The container's host (Settings → Docker's container IP).
    pub container_ip: &'a str,
    /// Settings → Database server: whether a server may be signed in to as the user without asking.
    pub database_server: &'a DatabaseServerOptions,
    /// Settings → SSMS remembers the password.
    pub remember_password: bool,
}

/// What [`open`] reports and asks through: the app's Output (ActivityLog), its prompt and the SQL tester.
pub trait OpenHost: Sync {
    fn info(&self, text: &str);
    fn warn(&self, text: &str);
    fn success(&self, text: &str);
    /// Asks `question` with the buttons `yes` and `no`; true for `yes`.
    fn confirm(&self, question: &str, yes: &str, no: &str) -> bool;
    fn tester(&self) -> &dyn SqlConnectionTester;
}

/// Runs `entry`. Err is the message for an error dialog ("Could not start …", "The project folder no longer exists: …");
/// what SSMS signing in reports goes to `host`. Blocks - call it off the UI thread. A separator or greyed entry does
/// nothing.
pub fn open(entry: &OpenWithEntry, context: &OpenContext<'_>, host: &dyn OpenHost) -> Result<(), String> {
    match &entry.kind {
        EntryKind::Ide(ide) => open_in_ide(ide, context.project_dir),
        EntryKind::Database { ssms, project } => {
            open_database(ssms, context, *project, host).map_err(|e| format!("Could not start {}: {e}", ssms.name))
        }
        EntryKind::Separator | EntryKind::Disabled | EntryKind::Ssms(_) => Ok(()),
    }
}

/// Opens `project_dir` (or its only solution file) in `ide` (`OpenInIde`, `IdeLocator.Open`) - as the signed-in user,
/// not with DNN Manager's administrator rights: an editor has no need of them, and one installed in the user's own
/// folders (VS Code's user installer) could have been changed by any program of theirs. Only when the desktop's shell
/// can't start it, and only administrators can change it, it is started directly.
#[cfg(not(windows))]
pub fn open_in_ide(ide: &Ide, _project_dir: &Path) -> Result<(), String> {
    Err(format!("Could not start {}: opening an editor from DNN Manager works only on Windows.", ide.name))
}

/// Opens `project_dir` (or its only solution file) in `ide` - Windows.
#[cfg(windows)]
pub fn open_in_ide(ide: &Ide, project_dir: &Path) -> Result<(), String> {
    if !project_dir.is_dir() {
        return Err(format!("The project folder no longer exists: {}", project_dir.display()));
    }
    let target: PathBuf = match locate::solution_for(project_dir) {
        Some(sln) if ide.opens_solution => sln,
        _ => project_dir.to_path_buf(),
    };
    let target_text = target.display().to_string();
    if start_unelevated(&ide.exe_path, &[&target_text], Some(project_dir)) {
        return Ok(());
    }
    let program = ide.exe_path.display().to_string();
    let resolved = dnnm_system::trusted::resolve(&program);
    if resolved.path.is_none() {
        let problem = resolved.problem(&program);
        return Err(format!("Could not start {}: {} couldn't be started as you. {problem}", ide.name, ide.name));
    }
    ssms::start_elevated(&ide.exe_path, &[&target_text], Some(project_dir))
        .map(drop)
        .map_err(|e| format!("Could not start {}: {e}", ide.name))
}

/// Opens SSMS signed in to the local SQL Server as the container's user (`project` false) or to the project's database
/// - the one its web.config uses. SSMS remembers the password when Settings says so. (`ProjectMenu.OpenDatabase`)
#[cfg(not(windows))]
fn open_database(_: &Ide, _: &OpenContext<'_>, _: bool, _: &dyn OpenHost) -> Result<(), String> {
    Err("SQL Server Management Studio runs only on Windows.".into())
}

/// Opens SSMS on the local SQL Server or the project's database - Windows.
#[cfg(windows)]
fn open_database(ssms: &Ide, context: &OpenContext<'_>, project: bool, host: &dyn OpenHost) -> Result<(), String> {
    // The project's database as its web.config has it (Windows authentication too) - not a guess.
    let mut database = if !project {
        context.default_connection.clone()
    } else if let Some(site) = &context.project_connection {
        site.clone()
    } else {
        host.warn(&format!(
            "'{}' has no database to open: {}.",
            context.project_name,
            context.database_tip.unwrap_or("its web.config names none")
        ));
        return Ok(());
    };
    let on_this_machine = SqlServerAddress::parse(&database.server).is_container_host(context.container_ip);

    // Windows authentication to a server web.config names - not on this PC, not the one in Settings - hands it your
    // Windows sign-in (the check below, and SSMS): asked once, naming it.
    if database.user.is_empty()
        && !SqlServerAddress::may_sign_in_as_user(&database.server, context.database_server)
        && !host.confirm(
            &SqlServerAddress::sign_in_question(&database.server, &format!("Opening it in {}", ssms.name)),
            &format!("Sign in to {}", database.server),
            "Cancel",
        )
    {
        return Ok(());
    }

    // SQL Server reports a missing database as the same "Login failed for user" as a wrong password, so check first:
    // open the server when only the database is missing, and say which of the two it is.
    let tester = host.tester();
    if !database.database.is_empty() && tester.test(&database, 5, None).is_err() {
        let master = SiteSqlConnection { database: "master".into(), ..database.clone() };
        match tester.test(&master, 5, None) {
            Ok(_) => {
                host.warn(&format!(
                    "Database [{}] doesn't exist on {} - {} opens the server instead. Restore a backup to put it back.",
                    database.database, database.server, ssms.name
                ));
                database.database.clear();
            }
            Err(error) => host.warn(&format!(
                "Can't log in to {} as '{}': {error} Check the password in the site's web.config (or Settings → Database server for the local container).",
                database.server, database.user
            )),
        }
    }

    let display_name = if !project {
        "Local SQL Server".to_string()
    } else if !database.database.is_empty() {
        format!("{} - {}", context.project_name, database.database)
    } else {
        context.project_name.to_string()
    };
    let target = if database.database.is_empty() {
        database.server.clone()
    } else {
        format!("[{}] on {}", database.database, database.server)
    };
    let as_user = if database.user.is_empty() { "Windows user" } else { database.user.as_str() };
    host.info(&format!("{}: {target} as {as_user}.", ssms.name));

    // SSMS has no password switch, and any connection switch makes it try to connect at once - failing with an error
    // when a password is needed. So for a SQL login in SSMS 21+ start it bare and fill its Connect dialog in; otherwise
    // pass what the command line takes.
    let sql_login = !database.user.is_empty() && !database.password.is_empty();
    if !sql_login || ssms.major_version < 21 {
        ssms::open_database(ssms, &database, on_this_machine, &display_name)?;
        if sql_login {
            leave_password_on_clipboard(ssms, &database, host);
        }
        return Ok(());
    }

    let remember = context.remember_password;
    // Add the connection to an SSMS that's already open rather than starting another one.
    let process = match ssms::find_running(ssms) {
        Some(running) if connect_dialog::open_in_running(&running) => {
            host.info(&format!("Adding the connection to the {} that's already open.", ssms.name));
            running
        }
        _ => ssms::start_management_studio(ssms)?,
    };
    let request = connect_dialog::SignIn {
        database: &database,
        trust_server_certificate: on_this_machine,
        display_name: &display_name,
        remember_password: remember,
        timeout: Duration::from_secs(90),
    };
    if connect_dialog::sign_in(&process, &request) {
        let remembered = if remember { " (password remembered there)." } else { "." };
        host.success(&format!("Signed in to {target} in {}{remembered}", ssms.name));
        return Ok(());
    }
    leave_password_on_clipboard(ssms, &database, host);
    Ok(())
}

#[cfg(windows)]
fn leave_password_on_clipboard(ssms: &Ide, database: &SiteSqlConnection, host: &dyn OpenHost) {
    if !secret_clipboard::set(&database.password) {
        host.warn(&format!(
            "Connect in {} to {} as '{}' - the clipboard was busy, so the password isn't on it.",
            ssms.name, database.server, database.user
        ));
        return;
    }
    host.info(&format!(
        "Connect in {} to {} as '{}' - the password is on the clipboard for {} seconds, kept out of the clipboard history (tick 'Remember Password').",
        ssms.name,
        database.server,
        database.user,
        secret_clipboard::CLEAR_AFTER.as_secs()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ide(name: &str, opens_solution: bool, major: u32) -> Ide {
        Ide {
            name: name.into(),
            exe_path: PathBuf::from(format!(r"C:\x\{name}.exe")),
            opens_solution,
            major_version: major,
        }
    }

    fn labels(entries: &[OpenWithEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.label.as_str()).collect()
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn editors_then_a_line_then_ssms_with_its_two_entries() {
        let ides = [ide("Visual Studio Community 2022", true, 17), ide("Visual Studio Code", false, 0)];
        let studios = [(ide("SQL Server Management Studio 21", false, 21), None)];
        let menu = entries(&ides, Some(Path::new(r"C:\Sites\shop\Shop.sln")), &studios, Some("shop"), None, "sa");

        assert_eq!(
            labels(&menu),
            ["Visual Studio Community 2022  (Shop.sln)", "Visual Studio Code", "", "SQL Server Management Studio 21"]
        );
        assert_eq!(menu[1].tooltip.as_deref(), Some(r"C:\x\Visual Studio Code.exe"));
        assert_eq!(menu[2].kind, EntryKind::Separator);
        let EntryKind::Ssms(submenu) = &menu[3].kind else { panic!("a submenu") };
        assert_eq!(labels(submenu), ["Default  (local SQL Server, sa)", "Project  ([shop])"]);
        assert!(matches!(submenu[1].kind, EntryKind::Database { project: true, .. }));
    }

    #[test]
    fn without_a_solution_the_name_stands_alone() {
        let menu = entries(&[ide("Rider", true, 0)], None, &[], None, None, "sa");
        assert_eq!(labels(&menu), ["Rider"]);
    }

    #[test]
    fn without_a_project_database_its_entry_is_greyed_saying_why() {
        let studios = [(ide("SSMS", false, 21), None)];
        let menu = entries(&[], None, &studios, None, None, "sa");
        assert_eq!(labels(&menu), ["SSMS"], "no line before SSMS when no editor is listed");
        let EntryKind::Ssms(submenu) = &menu[0].kind else { panic!("a submenu") };
        assert_eq!(submenu[1].label, "Project database");
        assert_eq!(submenu[1].kind, EntryKind::Disabled);
        assert_eq!(submenu[1].tooltip.as_deref(), Some("The site's web.config names no database."));

        let menu = entries(&[], None, &studios, None, Some("web.config can't be read."), "sa");
        let EntryKind::Ssms(submenu) = &menu[0].kind else { panic!("a submenu") };
        assert_eq!(submenu[1].tooltip.as_deref(), Some("web.config can't be read."));
    }

    #[test]
    fn an_ssms_others_can_change_is_greyed_with_the_reason() {
        let studios = [(ide("SSMS 20", false, 20), Some("DNN Manager doesn't start …".to_string()))];
        let menu = entries(&[], None, &studios, Some("shop"), None, "sa");
        assert_eq!(menu[0].kind, EntryKind::Disabled);
        assert_eq!(menu[0].tooltip.as_deref(), Some("DNN Manager doesn't start …"));
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn a_missing_project_folder_is_said_before_anything_starts() {
        let err = open_in_ide(&ide("Zed", false, 0), Path::new(r"C:\no\such\project")).unwrap_err();
        assert_eq!(err, r"The project folder no longer exists: C:\no\such\project");
    }

    #[cfg(windows)] // only the Windows-only tests use it
    struct Host {
        lines: std::sync::Mutex<Vec<String>>,
    }

    #[cfg(windows)]
    impl OpenHost for Host {
        fn info(&self, text: &str) {
            self.lines.lock().unwrap().push(text.into());
        }
        fn warn(&self, text: &str) {
            self.lines.lock().unwrap().push(text.into());
        }
        fn success(&self, text: &str) {
            self.lines.lock().unwrap().push(text.into());
        }
        fn confirm(&self, _: &str, _: &str, _: &str) -> bool {
            false
        }
        fn tester(&self) -> &dyn SqlConnectionTester {
            unreachable!("not asked")
        }
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn a_project_without_a_database_is_said_in_the_output_and_nothing_starts() {
        let host = Host { lines: Default::default() };
        let options = DatabaseServerOptions { kind: "container".into(), server: String::new() };
        let context = OpenContext {
            project_name: "shop",
            project_dir: Path::new(r"C:\Sites\shop"),
            default_connection: SiteSqlConnection::new("localhost", "", "sa", "pw"),
            project_connection: None,
            database_tip: None,
            container_ip: "localhost",
            database_server: &options,
            remember_password: false,
        };
        let entry = OpenWithEntry {
            label: "Project  ([shop])".into(),
            tooltip: None,
            kind: EntryKind::Database { ssms: ide("SSMS", false, 21), project: true },
        };
        assert_eq!(open(&entry, &context, &host), Ok(()));
        assert_eq!(*host.lines.lock().unwrap(), ["'shop' has no database to open: its web.config names none."]);
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn windows_sign_in_to_a_foreign_server_that_is_declined_starts_nothing() {
        let host = Host { lines: Default::default() };
        let options = DatabaseServerOptions { kind: "container".into(), server: String::new() };
        let context = OpenContext {
            project_name: "shop",
            project_dir: Path::new(r"C:\Sites\shop"),
            default_connection: SiteSqlConnection::new("localhost", "", "sa", "pw"),
            project_connection: Some(SiteSqlConnection::new("evil.example", "shop", "", "")),
            database_tip: None,
            container_ip: "localhost",
            database_server: &options,
            remember_password: false,
        };
        let entry = OpenWithEntry {
            label: String::new(),
            tooltip: None,
            kind: EntryKind::Database { ssms: ide("SSMS", false, 21), project: true },
        };
        assert_eq!(open(&entry, &context, &host), Ok(()));
        assert!(host.lines.lock().unwrap().is_empty());
    }

    // Reads this PC's installs (runs vswhere): not in a normal test run.
    #[test]
    #[ignore = "runs vswhere and reads this PC's Program Files"]
    fn lists_what_this_pc_has() {
        for entry in open_with_entries(Path::new(r"C:\"), Some("shop"), None, "sa") {
            println!("{:?} {:?}", entry.label, entry.tooltip);
        }
    }
}
