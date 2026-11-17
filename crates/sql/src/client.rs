//! DNN Manager's own connections to SQL Server - what Microsoft.Data.SqlClient does in the C# - with `tiberius` on a
//! small tokio runtime of this crate's own: every call blocks its (worker) thread until it is done, so callers stay
//! synchronous. TCP (a port, or a named instance through SQL Browser), named pipes (`np:`) and LocalDB, whose instance
//! only listens on a named pipe: the pipe's name comes from LocalDB's instance API, as SqlClient gets it.
//!
//! Deviations from SqlClient: no connection pooling (each call opens its own connection, as `Pooling=false`); a named
//! instance is reached over TCP through SQL Browser, never shared memory; an attached LocalDB file is attached with
//! `CREATE DATABASE … FOR ATTACH` (tiberius has no `AttachDbFilename`); a cancel or a command time-out drops the
//! connection instead of sending an attention - SQL Server then ends the session's statement; a statement's
//! informational messages (BACKUP's "10 percent processed") aren't surfaced by tiberius.

use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use dnnm_application::operation::{CANCELLED, CancelToken};
use tiberius::{AuthMethod, Client, ColumnData, Config, EncryptionLevel, Query, Row, SqlBrowser};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::Runtime;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::address::SqlServerAddress;
use crate::connection_strings::{ConnectionSettings, Encrypt, reachable};
use crate::sql_text;

/// SqlClient's default command time-out.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// What went wrong talking to SQL Server: the server's error number, when it was one of its errors, and its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlError {
    pub number: Option<u32>,
    pub message: String,
    pub cancelled: bool,
}

impl SqlError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { number: None, message: message.into(), cancelled: false }
    }

    fn cancelled() -> Self {
        Self { number: None, message: CANCELLED.into(), cancelled: true }
    }

    /// The message's first line, as the C#'s `FirstLine(ex.Message)`.
    pub fn first_line(&self) -> String {
        first_line(&self.message)
    }
}

impl std::fmt::Display for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or_default().trim().to_string()
}

fn from_tiberius(e: tiberius::error::Error, server: &str) -> SqlError {
    match e {
        tiberius::error::Error::Server(token) => {
            SqlError { number: Some(token.code()), message: token.message().to_string(), cancelled: false }
        }
        tiberius::error::Error::Io { message, .. } => SqlError::new(network_error(server, &message)),
        tiberius::error::Error::Tls(message) => SqlError::new(format!(
            "A connection was successfully established with {server}, but then an error occurred during the login process: {message} (the certificate chain may not be trusted)"
        )),
        other => SqlError::new(other.to_string()),
    }
}

fn network_error(server: &str, detail: &str) -> String {
    format!(
        "A network-related or instance-specific error occurred while establishing a connection to SQL Server. The server \
         was not found or was not accessible. Verify that the instance name is correct and that SQL Server is configured \
         to allow remote connections. ({server}: {detail})"
    )
}

/// The crate's runtime: two worker threads drive every connection; callers block on it from their own threads.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("dnnm-sql")
            .enable_all()
            .build()
            .expect("the SQL runtime")
    })
}

/// Runs `work` to its end - or until `cancel` is set, which drops it (closing its connection) and says "Cancelled.".
pub fn block_on<T: Send + 'static>(
    cancel: Option<&CancelToken>,
    work: impl Future<Output = Result<T, SqlError>> + Send + 'static,
) -> Result<T, SqlError> {
    let cancel = cancel.cloned();
    runtime().block_on(async move {
        let handle = tokio::spawn(work);
        let abort = handle.abort_handle();
        let watch = async {
            loop {
                if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::select! {
            joined = handle => joined.unwrap_or_else(|e| Err(SqlError::new(format!("The SQL work ended unexpectedly: {e}")))),
            () = watch => {
                abort.abort();
                Err(SqlError::cancelled())
            }
        }
    })
}

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// A value of a parameter (`@P1`, `@P2`… in the SQL text).
#[derive(Debug, Clone)]
pub enum Param {
    Text(String),
    NullableText(Option<String>),
    Bit(bool),
}

impl From<&str> for Param {
    fn from(value: &str) -> Self {
        Param::Text(value.to_string())
    }
}

impl From<&String> for Param {
    fn from(value: &String) -> Self {
        Param::Text(value.clone())
    }
}

impl From<bool> for Param {
    fn from(value: bool) -> Self {
        Param::Bit(value)
    }
}

/// A cell read back: what DNN Manager reads is numbers and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
    Other,
}

impl Value {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn into_text(self) -> Option<String> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    fn of(cell: &ColumnData<'static>) -> Self {
        match cell {
            ColumnData::U8(Some(n)) => Value::Int(i64::from(*n)),
            ColumnData::I16(Some(n)) => Value::Int(i64::from(*n)),
            ColumnData::I32(Some(n)) => Value::Int(i64::from(*n)),
            ColumnData::I64(Some(n)) => Value::Int(*n),
            ColumnData::Bit(Some(b)) => Value::Int(i64::from(*b)),
            ColumnData::String(Some(s)) => Value::Text(s.to_string()),
            ColumnData::U8(None)
            | ColumnData::I16(None)
            | ColumnData::I32(None)
            | ColumnData::I64(None)
            | ColumnData::Bit(None)
            | ColumnData::String(None)
            | ColumnData::Numeric(None)
            | ColumnData::Binary(None)
            | ColumnData::Guid(None) => Value::Null,
            ColumnData::Numeric(Some(n)) => Value::Int(n.int_part() as i64),
            _ => Value::Other,
        }
    }
}

/// The cells of a row, in order.
pub fn values(row: &Row) -> Vec<Value> {
    row.cells().map(|(_, cell)| Value::of(cell)).collect()
}

/// One connection, open.
pub struct Conn {
    client: Client<Compat<Box<dyn Io>>>,
    server: String,
}

impl Conn {
    /// Opens a connection as `settings` say: within its connect time-out, encrypted as decided there.
    pub async fn open(settings: &ConnectionSettings) -> Result<Conn, SqlError> {
        let server = settings.server.clone();
        let timeout = settings.connect_timeout.max(Duration::from_secs(1));
        let mut conn = match tokio::time::timeout(timeout, connect(settings)).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(SqlError::new(format!(
                    "Connection Timeout Expired. The timeout period elapsed while connecting to {server} ({} s).",
                    timeout.as_secs()
                )));
            }
        };
        if let Some(file) = &settings.attach_file {
            conn.attach(&file.display().to_string()).await?;
        }
        Ok(conn)
    }

    /// Runs `sql` with `params` and returns every row of its first result set.
    pub async fn rows(&mut self, sql: &str, params: &[Param]) -> Result<Vec<Row>, SqlError> {
        self.rows_within(sql, params, DEFAULT_COMMAND_TIMEOUT).await
    }

    pub async fn rows_within(&mut self, sql: &str, params: &[Param], limit: Duration) -> Result<Vec<Row>, SqlError> {
        let server = self.server.clone();
        let query = make_query(sql, params);
        let work = async {
            let stream = query.query(&mut self.client).await?;
            stream.into_first_result().await
        };
        within(limit, work).await.and_then(|r| r.map_err(|e| from_tiberius(e, &server)))
    }

    /// The first column of the first row; Value::Null when there is none.
    pub async fn scalar(&mut self, sql: &str, params: &[Param]) -> Result<Value, SqlError> {
        let rows = self.rows(sql, params).await?;
        Ok(rows.first().and_then(|r| values(r).into_iter().next()).unwrap_or(Value::Null))
    }

    /// Runs `sql`; how many rows it changed.
    pub async fn execute(&mut self, sql: &str, params: &[Param]) -> Result<u64, SqlError> {
        self.execute_within(sql, params, DEFAULT_COMMAND_TIMEOUT).await
    }

    /// As [`Self::execute`] with a time-out of its own (`None`: none, as `CommandTimeout = 0`).
    pub async fn execute_within(&mut self, sql: &str, params: &[Param], limit: Duration) -> Result<u64, SqlError> {
        let server = self.server.clone();
        let query = make_query(sql, params);
        let work = async { query.execute(&mut self.client).await.map(|r| r.total()) };
        within(limit, work).await.and_then(|r| r.map_err(|e| from_tiberius(e, &server)))
    }

    /// A batch without parameters, run as it is (not through sp_executesql): what it changes about the session - the
    /// database `USE` picks, a transaction it begins - stays.
    pub async fn batch(&mut self, sql: &str) -> Result<(), SqlError> {
        let server = self.server.clone();
        let work = async {
            let stream = self.client.simple_query(sql).await?;
            stream.into_results().await.map(|_| ())
        };
        within(DEFAULT_COMMAND_TIMEOUT, work).await.and_then(|r| r.map_err(|e| from_tiberius(e, &server)))
    }

    /// `SqlConnection.ChangeDatabase`.
    pub async fn change_database(&mut self, database: &str) -> Result<(), SqlError> {
        self.batch(&format!("USE {};", sql_text::identifier(database))).await
    }

    /// Attaches a LocalDB database file and makes it this connection's database - the one already attached for it
    /// when there is one (found by its file, as SqlClient names an attached database by its path).
    async fn attach(&mut self, file: &str) -> Result<(), SqlError> {
        let attached = self
            .scalar(
                "SELECT TOP 1 DB_NAME(database_id) FROM sys.master_files WHERE type = 0 AND LOWER(physical_name) = LOWER(@P1)",
                &[file.into()],
            )
            .await?;
        let name = match attached.into_text() {
            Some(name) => name,
            None => {
                let name = attached_name(file);
                let log = std::path::Path::new(file).with_extension("").display().to_string() + "_log.ldf";
                let log = std::path::Path::new(&log).is_file().then_some(log);
                self.execute_within(
                    "DECLARE @sql nvarchar(max) = N'CREATE DATABASE ' + QUOTENAME(@P1) + N' ON (FILENAME = N''' + REPLACE(@P2, N'''', N'''''') + N''')' \
                     + COALESCE(N', (FILENAME = N''' + REPLACE(@P3, N'''', N'''''') + N''')', N'') + N' FOR ATTACH'; EXEC (@sql);",
                    &[name.as_str().into(), file.into(), Param::NullableText(log)],
                    Duration::from_secs(60),
                )
                .await?;
                name
            }
        };
        self.change_database(&name).await
    }
}

/// The name an attached file's database gets: its path, as SqlClient names it - or, too long for a name, a short one.
fn attached_name(file: &str) -> String {
    if file.chars().count() <= 128 {
        return file.to_uppercase();
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    file.to_lowercase().hash(&mut hasher);
    format!("DnnManager_{:016x}", hasher.finish())
}

fn make_query(sql: &str, params: &[Param]) -> Query<'static> {
    let mut query = Query::new(sql.to_string());
    for param in params {
        match param.clone() {
            Param::Text(t) => query.bind(t),
            Param::NullableText(t) => query.bind(t),
            Param::Bit(b) => query.bind(b),
        }
    }
    query
}

async fn within<T>(limit: Duration, work: impl Future<Output = T>) -> Result<T, SqlError> {
    if limit.is_zero() {
        return Ok(work.await);
    }
    tokio::time::timeout(limit, work).await.map_err(|_| {
        SqlError::new("Execution Timeout Expired. The timeout period elapsed prior to completion of the operation or the server is not responding.")
    })
}

async fn connect(settings: &ConnectionSettings) -> Result<Conn, SqlError> {
    let server = reachable(&settings.server);
    let address = SqlServerAddress::parse(&server);
    let mut config = Config::new();
    if let Some(database) = &settings.database {
        config.database(database);
    }
    config.application_name("DNN Manager");
    config.encryption(match settings.encrypt {
        Encrypt::Mandatory => EncryptionLevel::Required,
        Encrypt::Optional => EncryptionLevel::Off,
    });
    if settings.trust_server_certificate {
        config.trust_cert();
    }
    if settings.integrated_security() {
        config.authentication(windows_auth()?);
    } else {
        config.authentication(AuthMethod::sql_server(&settings.user, &settings.password));
    }

    let stream: Box<dyn Io> = if address.is_local_db {
        config.host("localhost");
        // LocalDB's pipe is this PC's own and the sign-in is Windows authentication (no password goes over it):
        // without TLS, which LocalDB ends the pipe on when tiberius starts it there.
        config.encryption(EncryptionLevel::NotSupported);
        let pipe = crate::localdb::pipe_for(&address.instance).await?;
        open_pipe(&pipe, &settings.server).await?
    } else if address.protocol == "np" {
        let path = server.trim();
        let path = path.strip_prefix("np:").or_else(|| path.strip_prefix("NP:")).unwrap_or(path).trim();
        config.host(if address.host == "." { "localhost" } else { &address.host });
        open_pipe(path, &settings.server).await?
    } else if address.protocol == "lpc" || address.protocol == "admin" {
        return Err(SqlError::new(format!(
            "{server}: the {} protocol isn't supported - use the server's TCP address.",
            address.protocol
        )));
    } else {
        let host = match address.host.as_str() {
            "" | "." | "(local)" => "127.0.0.1".to_string(),
            h => h.to_string(),
        };
        config.host(&host);
        if !address.instance.is_empty() && address.port.is_none() {
            config.instance_name(&address.instance);
            let tcp = tokio::net::TcpStream::connect_named(&config).await.map_err(|e| from_tiberius(e, &server))?;
            let _ = tcp.set_nodelay(true);
            Box::new(tcp)
        } else {
            let port = address.port.unwrap_or(crate::address::DEFAULT_PORT);
            config.port(port);
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|e| SqlError::new(network_error(&server, &e.to_string())))?;
            let _ = tcp.set_nodelay(true);
            Box::new(tcp)
        }
    };

    match Client::connect(config.clone(), stream.compat_write()).await {
        Ok(client) => Ok(Conn { client, server }),
        // Azure SQL may send the client to another node.
        Err(tiberius::error::Error::Routing { host, port }) => {
            config.host(&host);
            config.port(port);
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|e| SqlError::new(network_error(&server, &e.to_string())))?;
            let stream: Box<dyn Io> = Box::new(tcp);
            let client = Client::connect(config, stream.compat_write()).await.map_err(|e| from_tiberius(e, &server))?;
            Ok(Conn { client, server })
        }
        Err(e) => Err(from_tiberius(e, &server)),
    }
}

#[cfg(windows)]
fn windows_auth() -> Result<AuthMethod, SqlError> {
    Ok(AuthMethod::Integrated)
}

#[cfg(not(windows))]
fn windows_auth() -> Result<AuthMethod, SqlError> {
    Err(SqlError::new("Windows authentication is only available on Windows."))
}

#[cfg(windows)]
async fn open_pipe(path: &str, server: &str) -> Result<Box<dyn Io>, SqlError> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    loop {
        match ClientOptions::new().open(path) {
            Ok(pipe) => return Ok(Box::new(pipe)),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(e) => return Err(SqlError::new(network_error(server, &format!("Named Pipes Provider: {e}")))),
        }
    }
}

#[cfg(not(windows))]
async fn open_pipe(_path: &str, server: &str) -> Result<Box<dyn Io>, SqlError> {
    Err(SqlError::new(network_error(server, "named pipes are only available on Windows")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_strings::for_server;

    #[test]
    fn a_cancel_ends_the_wait() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let r: Result<(), SqlError> = block_on(Some(&cancel), async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(())
        });
        assert!(r.unwrap_err().cancelled);
    }

    #[test]
    fn a_server_that_isnt_there_is_a_network_error_within_the_timeout() {
        // Port 1 on this PC: nothing listens there - refused at once, nothing is changed anywhere.
        let settings = for_server("127.0.0.1,1", Some("master"), "sa", Some("x"), 5);
        let started = std::time::Instant::now();
        let error = block_on(None, async move { Conn::open(&settings).await.map(|_| ()) }).unwrap_err();
        assert!(
            error.message.starts_with("A network-related or instance-specific error occurred"),
            "{}",
            error.message
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn the_first_line_of_a_message() {
        assert_eq!(first_line("A network-related error occurred.\nMore detail."), "A network-related error occurred.");
    }

    #[test]
    fn an_attached_file_is_named_after_its_path() {
        assert_eq!(attached_name(r"C:\sites\shop\App_Data\Database.mdf"), r"C:\SITES\SHOP\APP_DATA\DATABASE.MDF");
        assert!(attached_name(&"x".repeat(200)).starts_with("DnnManager_"));
    }

    /// Read-only probe of a SQL Server on this PC (`SELECT @@VERSION` and the database names) - needs one listening
    /// on localhost,1433 that takes Windows authentication or the sa password in DNNM_TEST_SA_PASSWORD.
    #[test]
    #[ignore = "needs a SQL Server on localhost,1433"]
    fn probe_localhost_1433() {
        let password = std::env::var("DNNM_TEST_SA_PASSWORD").ok();
        let user = if password.is_some() { "sa" } else { "" };
        let settings = for_server("localhost,1433", Some("master"), user, password.as_deref(), 5);
        let (version, names) = block_on(None, async move {
            let mut conn = Conn::open(&settings).await?;
            let version = conn.scalar("SELECT @@VERSION", &[]).await?;
            let names = conn.rows("SELECT name FROM sys.databases", &[]).await?.len();
            Ok((version, names))
        })
        .unwrap();
        assert!(version.as_text().unwrap().contains("SQL Server"));
        assert!(names >= 4);
    }

    /// Read-only probe of the user's LocalDB (`SELECT @@VERSION`) through its named pipe - starts the instance as a
    /// connection would.
    #[test]
    #[ignore = "needs SQL Server Express LocalDB"]
    fn probe_local_db() {
        let settings = for_server(r"(LocalDB)\MSSQLLocalDB", Some("master"), "", None, 60);
        let version = block_on(None, async move {
            let mut conn = Conn::open(&settings).await?;
            conn.scalar("SELECT @@VERSION", &[]).await
        })
        .unwrap();
        assert!(version.as_text().unwrap().contains("Express"), "{version:?}");
    }
}
