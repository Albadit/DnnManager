//! "Open with…" of a project's menu (`ProjectMenu.FillOpenWith` / `OpenInIde` / `OpenDatabase`): the editors and SQL
//! Server Management Studio found on this PC, and opening one - as an operation, so what signing SSMS in says goes to
//! the Output and its questions are the app's dialogs.

use std::path::{Path, PathBuf};

use dnnm_application::operation::Operation;
use dnnm_openwith::{OpenContext, OpenHost, OpenWithEntry};
use dnnm_sql::model::{DatabaseServerOptions, SiteSqlConnection};
use dnnm_sql::tester::{SqlClientTester, SqlConnectionTester};

use crate::Services;

pub use dnnm_openwith::{EntryKind, NOTHING_FOUND, OPEN_WITH, READING};

/// What to open, with what the menu showed for the project.
#[derive(Clone)]
pub struct OpenWithRequest {
    pub entry: OpenWithEntry,
    pub project_name: String,
    pub project_dir: PathBuf,
    /// The Projects row's note on its database (`ProjectRow.DatabaseTip`).
    pub database_tip: Option<String>,
}

/// The container's SQL login - the "Default  (local SQL Server, <user>)" entry names it.
pub fn container_user(services: &Services) -> String {
    services.settings.get("sqlServer.userName").map(str::trim).filter(|u| !u.is_empty()).unwrap_or("sa").to_string()
}

/// The submenu's entries - off the UI thread: it looks for editors and SSMS on the first call.
pub fn entries(services: &Services, project_dir: &Path, project_database: Option<&str>, database_tip: Option<&str>) -> Vec<OpenWithEntry> {
    dnnm_openwith::open_with_entries(project_dir, project_database, database_tip, &container_user(services))
}

/// The local SQL Server as the container's user (`LocalSqlContainer.DefaultConnection`).
fn default_connection(services: &Services) -> SiteSqlConnection {
    let host = services.settings.get("sqlServer.host").map(str::trim).filter(|h| !h.is_empty()).unwrap_or("localhost");
    let port = services.settings.get("sqlServer.port").map(str::trim).filter(|p| !p.is_empty()).unwrap_or("1433");
    let password = services
        .settings
        .get(dnnm_system::secrets::SA_PASSWORD_KEY)
        .and_then(|p| dnnm_system::secrets::unprotect(p).ok())
        .unwrap_or_default();
    SiteSqlConnection::new(&format!("{host},{port}"), "", &container_user(services), &password)
}

/// The project's database as its web.config has it (`LocalSqlContainer.SiteConnectionOf`).
fn project_connection(project_dir: &Path) -> Option<SiteSqlConnection> {
    let c = dnnm_infrastructure::web_config::read_database_connection(&project_dir.join("web.config")).ok()?;
    Some(if c.windows_authentication {
        SiteSqlConnection::new(&c.server, &c.database, "", "")
    } else {
        SiteSqlConnection::new(&c.server, &c.database, &c.user, &c.password)
    })
}

/// The operation's title: "Open with <what>".
pub fn title(entry: &OpenWithEntry) -> String {
    format!("Open with {}", entry.label)
}

/// Opens the entry (`ProjectMenu.OpenInIde` / `OpenDatabase`).
pub fn open_with(services: &Services, request: OpenWithRequest, op: &mut Operation) -> Result<(), String> {
    let database_server = DatabaseServerOptions {
        kind: services.settings.get("sqlServer.type").unwrap_or("container").to_string(),
        server: services.settings.get("sqlServer.server").unwrap_or_default().to_string(),
    };
    let container_ip = services.settings.get("sqlServer.host").unwrap_or("localhost").to_string();
    let context = OpenContext {
        project_name: &request.project_name,
        project_dir: &request.project_dir,
        default_connection: default_connection(services),
        project_connection: project_connection(&request.project_dir),
        database_tip: request.database_tip.as_deref(),
        container_ip: &container_ip,
        database_server: &database_server,
        remember_password: services.settings.get("ssms.rememberPassword").is_some_and(|v| v.eq_ignore_ascii_case("true")),
    };
    let host = OperationHost::new(op);
    let result = dnnm_openwith::open(&request.entry, &context, &host);
    host.flush(op);
    result
}

/// What `open` reports and asks, collected while it runs (it wants `Sync`) and handed to the operation after; its
/// questions go straight to the prompt.
struct OperationHost<'a> {
    lines: std::sync::Mutex<Vec<(u8, String)>>,
    prompt: &'a dyn dnnm_application::operation::Prompt,
    tester: SqlClientTester,
}

impl<'a> OperationHost<'a> {
    fn new(op: &Operation<'a>) -> Self {
        Self { lines: std::sync::Mutex::new(Vec::new()), prompt: op.prompt, tester: SqlClientTester }
    }

    fn push(&self, kind: u8, text: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push((kind, text.to_string()));
        }
    }

    fn flush(&self, op: &mut Operation) {
        let lines = self.lines.lock().map(|l| l.clone()).unwrap_or_default();
        for (kind, text) in lines {
            match kind {
                0 => op.report.info(&text),
                1 => op.report.warn(&text),
                _ => op.report.success(&text),
            }
        }
    }
}

impl OpenHost for OperationHost<'_> {
    fn info(&self, text: &str) {
        self.push(0, text);
    }
    fn warn(&self, text: &str) {
        self.push(1, text);
    }
    fn success(&self, text: &str) {
        self.push(2, text);
    }
    fn confirm(&self, question: &str, yes: &str, no: &str) -> bool {
        self.prompt.confirm(question, yes, no, false)
    }
    fn tester(&self) -> &dyn SqlConnectionTester {
        &self.tester
    }
}
