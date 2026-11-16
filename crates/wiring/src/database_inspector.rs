//! The Details' database read (`Infrastructure/Diagnostics/DatabaseInspector.cs`): `dnnm_diagnostics`'
//! [`DatabaseInspector`] with `dnnm-sql`'s client. It signs in with the site's own web.config connection - its login,
//! or Windows authentication as DNN Manager's user - never DNN Manager's settings, and only reads: SELECTs and server
//! properties, the queries of [`dnnm_diagnostics::database::queries`] word for word.
//!
//! Differs from the C#: the connection's application name is the client's own ("DNN Manager", not "DNN Manager
//! (Details)" - `dnnm-sql` sets it for every connection), and the host settings' parameters are named `@P1`… (tiberius')
//! instead of `@n0`….

use std::collections::BTreeMap;

use dnnm_diagnostics::database::{
    DatabaseFile, DatabaseInspection, DatabaseInspector, DatabaseProperties, DnnDatabaseFacts, DnnPortalDetails,
    SqlServerFacts, first_line, growth_text, max_size_text, part_note, queries,
};
use dnnm_diagnostics::format::LocalDateTime;
use dnnm_infrastructure::web_config::DatabaseConnection;
use dnnm_sql::client::{Conn, Param, SqlError, block_on};
use dnnm_sql::sql_text::dnn_tables;
use tiberius::ColumnData;

/// The C#'s time-out for connecting to the site's database from the Details.
const TIMEOUT_SECONDS: u64 = 8;

/// [`inspect_database`] as the diagnostics crate's trait.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlDatabaseInspector;

impl DatabaseInspector for SqlDatabaseInspector {
    fn inspect(&self, connection: &DatabaseConnection) -> DatabaseInspection {
        inspect_database(connection)
    }
}

/// Reads the site's database (`DatabaseInspector.InspectAsync`) for its Details' Database and DNN tabs. Blocks - up to
/// 8 s to connect, then a query per part: call it off the UI thread, and not for a LocalDB file (the site's own while
/// it runs; `ProjectSnapshot::needs_database` leaves those out). A connection that fails is `problem`; a part that
/// fails is a note, and the other parts still come.
pub fn inspect_database(connection: &DatabaseConnection) -> DatabaseInspection {
    let (user, password) = if connection.windows_authentication {
        ("", "")
    } else {
        (connection.user.as_str(), connection.password.as_str())
    };
    let settings = dnnm_sql::connection_strings::for_server(
        &connection.server,
        Some(&connection.database),
        user,
        Some(password),
        TIMEOUT_SECONDS,
    );
    block_on(None, async move {
        let mut conn = match Conn::open(&settings).await {
            Ok(conn) => conn,
            Err(e) => {
                return Ok(DatabaseInspection {
                    problem: Some(first_line(&e.message).to_string()),
                    ..Default::default()
                });
            }
        };
        let mut notes = Vec::new();
        let server = noted(&mut notes, "the SQL Server's properties", server_facts(&mut conn).await);
        let database = database_properties(&mut conn, &mut notes).await;
        let database = noted(&mut notes, "the database's properties", database);
        let dnn = dnn_facts(&mut conn, &mut notes).await;
        let dnn = noted(&mut notes, "DNN's tables", dnn).flatten();
        Ok(DatabaseInspection { problem: None, server, database, dnn, notes })
    })
    .unwrap_or_else(|e: SqlError| DatabaseInspection { problem: Some(e.first_line()), ..Default::default() })
}

/// One part of the inspection (`Try`): what goes wrong is a note - the other parts still come.
fn noted<T>(notes: &mut Vec<String>, what: &str, read: Result<T, SqlError>) -> Option<T> {
    read.map_err(|e| notes.push(part_note(what, &e.message))).ok()
}

async fn server_facts(conn: &mut Conn) -> Result<SqlServerFacts, SqlError> {
    let row = first_row(conn, queries::SERVER, &[]).await?;
    Ok(SqlServerFacts {
        instance: text(&row, 0),
        version: text(&row, 1),
        level: text(&row, 2),
        edition: text(&row, 3),
        collation: text(&row, 4),
        major_version: int(&row, 5).map(|v| v as i32),
    })
}

async fn database_properties(conn: &mut Conn, notes: &mut Vec<String>) -> Result<DatabaseProperties, SqlError> {
    let row = first_row(conn, queries::DATABASE, &[]).await?;
    let mut properties = DatabaseProperties {
        name: text(&row, 0).unwrap_or_default(),
        compatibility_level: int(&row, 1).map(|v| v as i32),
        collation: text(&row, 2),
        recovery_model: text(&row, 3),
        state: text(&row, 4),
        read_only: boolean(&row, 5),
        auto_shrink: boolean(&row, 6),
        auto_close: boolean(&row, 7),
        snapshot_isolation: text(&row, 8),
        read_committed_snapshot: boolean(&row, 9),
        page_verify: text(&row, 10),
        created: date(&row, 11),
        ..Default::default()
    };

    let files = all_rows(conn, queries::FILES, &[]).await.map(|rows| {
        rows.iter()
            .map(|row| DatabaseFile {
                name: text(row, 0).unwrap_or_default(),
                kind: text(row, 1).unwrap_or_default(),
                size_mb: number(row, 2).unwrap_or_default(),
                used_mb: number(row, 3),
                max_size: max_size_text(int(row, 4).unwrap_or_default() as i32),
                growth: growth_text(int(row, 5).unwrap_or_default() as i32, boolean(row, 6).unwrap_or_default()),
                physical_name: text(row, 7).unwrap_or_default(),
            })
            .collect()
    });
    properties.files = noted(notes, "the database's files", files).unwrap_or_default();

    // Not every login may look at its own connection (VIEW SERVER STATE) - then it isn't said.
    match all_rows(conn, queries::CONNECTION, &[]).await {
        Ok(rows) => {
            if let Some(row) = rows.first() {
                properties.connection_encrypted = text(row, 0);
                properties.signed_in_as = text(row, 1);
            }
        }
        Err(_) => {
            properties.signed_in_as =
                first_row(conn, queries::SIGNED_IN_AS, &[]).await.ok().and_then(|row| text(&row, 0));
        }
    }
    Ok(properties)
}

/// DNN's own tables (`DnnAsync`); None when the database has none.
async fn dnn_facts(conn: &mut Conn, notes: &mut Vec<String>) -> Result<Option<DnnDatabaseFacts>, SqlError> {
    // DNN's tables can carry an objectQualifier prefix (dnn_Portals): found from the Version table.
    let qualifier = first_row(conn, dnn_tables::QUALIFIER_SQL, &["Version".into()]).await?;
    let Some(q) = text(&qualifier, 0).filter(|q| dnn_tables::is_qualifier(q)) else { return Ok(None) };
    let mut facts = DnnDatabaseFacts { qualifier: (!q.is_empty()).then(|| q.clone()), ..Default::default() };

    let version = all_rows(conn, &with_tables(queries::VERSION, &q), &[]).await;
    if let Some(row) = noted(notes, "DNN's Version table", version).as_ref().and_then(|rows| rows.first()) {
        facts.version = text(row, 0);
        facts.upgraded = date(row, 1);
        facts.installed = date(row, 2);
    }

    let portals = portals(conn, &q).await;
    facts.portals = noted(notes, "the portals", portals).unwrap_or_default();
    facts.users = count(conn, notes, &with_tables(queries::USERS, &q), "the users").await;
    facts.roles = count(conn, notes, &with_tables(queries::ROLES, &q), "the roles").await;
    facts.desktop_modules = count(conn, notes, &with_tables(queries::MODULES, &q), "the modules").await;

    let schedule = first_row(conn, &with_tables(queries::SCHEDULE, &q), &[]).await;
    if let Some(row) = noted(notes, "the scheduler's jobs", schedule) {
        facts.schedule_items = int(&row, 0);
        facts.schedule_enabled = Some(int(&row, 1).unwrap_or_default());
    }

    let events = first_row(conn, &with_tables(queries::EVENT_LOG, &q), &[]).await;
    if let Some(row) = noted(notes, "the event log", events) {
        facts.event_log_records = int(&row, 0);
        facts.event_log_oldest = date(&row, 1);
        facts.event_log_newest = date(&row, 2);
    }

    let packages = all_rows(conn, &with_tables(queries::PACKAGES, &q), &[]).await.map(|rows| {
        let mut by_type = BTreeMap::new();
        for row in &rows {
            insert_ignoring_case(&mut by_type, text(row, 0).unwrap_or_default(), int(row, 1).unwrap_or_default());
        }
        by_type
    });
    facts.packages = noted(notes, "the installed extensions", packages).unwrap_or_default();

    let authentication = all_rows(conn, &with_tables(queries::AUTHENTICATION, &q), &[]).await.map(|rows| {
        rows.iter().map(|row| (text(row, 0).unwrap_or_default(), boolean(row, 1).unwrap_or_default())).collect()
    });
    facts.authentication_providers = noted(notes, "the authentication providers", authentication).unwrap_or_default();

    let names: Vec<Param> = queries::HOST_SETTING_NAMES.iter().map(|n| Param::from(*n)).collect();
    let placeholders = (1..=names.len()).map(|i| format!("@P{i}")).collect::<Vec<_>>().join(", ");
    let sql = format!("{}({placeholders})", with_tables(queries::HOST_SETTINGS, &q));
    let settings = all_rows(conn, &sql, &names).await.map(|rows| {
        let mut found = BTreeMap::new();
        for row in &rows {
            insert_ignoring_case(&mut found, text(row, 0).unwrap_or_default(), text(row, 1).unwrap_or_default());
        }
        found
    });
    facts.host_settings = noted(notes, "the host settings", settings).unwrap_or_default();
    Ok(Some(facts))
}

/// The portals with their aliases (`PortalsAsync`) - their name from PortalLocalization when this DNN has it, the
/// primary alias by IsPrimary when it has that.
async fn portals(conn: &mut Conn, q: &str) -> Result<Vec<DnnPortalDetails>, SqlError> {
    let t = |table: &str| dnn_tables::name(q, table);
    let columns = first_row(conn, &with_tables(queries::PORTAL_COLUMNS, q), &[]).await?;
    let localized = !matches!(columns.first(), None | Some(Cell::Null));
    let primary_flag = !matches!(columns.get(1), None | Some(Cell::Null));
    let culture = !matches!(columns.get(2), None | Some(Cell::Null));
    let name = if localized {
        format!(
            "(SELECT TOP 1 pl.PortalName FROM {} pl WHERE pl.PortalID = p.PortalID ORDER BY CASE WHEN pl.CultureCode = p.DefaultLanguage THEN 0 ELSE 1 END, pl.CultureCode)",
            t("PortalLocalization")
        )
    } else {
        "NULL".to_string()
    };
    let portals = all_rows(
        conn,
        &format!(
            "SELECT p.PortalID, {name}, p.DefaultLanguage, p.HomeDirectory, CASE WHEN p.ExpiryDate IS NOT NULL AND p.ExpiryDate < GETDATE() THEN 1 ELSE 0 END FROM {} p ORDER BY p.PortalID",
            t("Portals")
        ),
        &[],
    )
    .await?;
    let aliases = all_rows(
        conn,
        &format!(
            "SELECT PortalID, HTTPAlias, {}, {} FROM {} ORDER BY PortalID, {}PortalAliasID",
            if culture { "CultureCode" } else { "NULL" },
            if primary_flag { "CAST(IsPrimary AS int)" } else { "0" },
            t("PortalAlias"),
            if primary_flag { "IsPrimary DESC, " } else { "" }
        ),
        &[],
    )
    .await?;
    let aliases: Vec<(i64, String, Option<String>, bool)> = aliases
        .iter()
        .map(|row| {
            (int(row, 0).unwrap_or_default(), text(row, 1).unwrap_or_default(), text(row, 2), int(row, 3) == Some(1))
        })
        .collect();
    Ok(portals
        .iter()
        .map(|row| {
            let id = int(row, 0).unwrap_or_default();
            let list: Vec<_> = aliases.iter().filter(|a| a.0 == id).collect();
            let primary = list.iter().find(|a| a.3).or(list.first()).map(|a| a.1.clone());
            DnnPortalDetails {
                id: id as i32,
                name: text(row, 1).unwrap_or_else(|| format!("Portal {id}")),
                default_language: text(row, 2),
                home_directory: text(row, 3),
                expired: int(row, 4) == Some(1),
                other_aliases: list
                    .iter()
                    .filter(|a| primary.as_deref() != Some(a.1.as_str()))
                    .map(|a| (a.1.clone(), a.2.clone()))
                    .collect(),
                primary_alias: primary,
            }
        })
        .collect())
}

async fn count(conn: &mut Conn, notes: &mut Vec<String>, sql: &str, what: &str) -> Option<i64> {
    let read = first_row(conn, sql, &[]).await.map(|row| int(&row, 0).unwrap_or_default());
    noted(notes, what, read)
}

/// `template` with each `{Table}` as DNN's table of that name: `dbo.[<qualifier>Table]` (`DnnTables.Name`).
fn with_tables(template: &str, qualifier: &str) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        out.push_str(&rest[..open]);
        out.push_str(&dnn_tables::name(qualifier, &rest[open + 1..open + close]));
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// As a dictionary with `StringComparer.OrdinalIgnoreCase`: a name that differs only in case replaces the one there.
fn insert_ignoring_case<V>(map: &mut BTreeMap<String, V>, key: String, value: V) {
    let existing = map.keys().find(|k| k.to_lowercase() == key.to_lowercase()).cloned();
    map.insert(existing.unwrap_or(key), value);
}

// ─── Cells ───────────────────────────────────────────────────────────────────

/// A cell as the inspection reads it: numbers, text, bits and dates.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Null,
    Int(i64),
    Number(f64),
    Text(String),
    Bit(bool),
    Date(LocalDateTime),
    Other,
}

async fn all_rows(conn: &mut Conn, sql: &str, params: &[Param]) -> Result<Vec<Vec<Cell>>, SqlError> {
    let rows = conn.rows(sql, params).await?;
    Ok(rows.iter().map(|row| row.cells().map(|(_, data)| cell(data)).collect()).collect())
}

/// The first row; an error when there is none (the C# reads it as there).
async fn first_row(conn: &mut Conn, sql: &str, params: &[Param]) -> Result<Vec<Cell>, SqlError> {
    all_rows(conn, sql, params).await?.into_iter().next().ok_or_else(|| SqlError::new("The query returned no row."))
}

fn cell(data: &ColumnData<'static>) -> Cell {
    match data {
        ColumnData::U8(Some(n)) => Cell::Int(i64::from(*n)),
        ColumnData::I16(Some(n)) => Cell::Int(i64::from(*n)),
        ColumnData::I32(Some(n)) => Cell::Int(i64::from(*n)),
        ColumnData::I64(Some(n)) => Cell::Int(*n),
        ColumnData::F32(Some(n)) => Cell::Number(f64::from(*n)),
        ColumnData::F64(Some(n)) => Cell::Number(*n),
        ColumnData::Numeric(Some(n)) => Cell::Number(f64::from(*n)),
        ColumnData::Bit(Some(b)) => Cell::Bit(*b),
        ColumnData::String(Some(s)) => Cell::Text(s.to_string()),
        // datetime: days since 1900-01-01 and 1/300 s since midnight.
        ColumnData::DateTime(Some(d)) => {
            Cell::Date(date_time(i64::from(d.days()) + DAYS_TO_1900, u64::from(d.seconds_fragments()) / 300))
        }
        // smalldatetime: days since 1900-01-01 and minutes since midnight.
        ColumnData::SmallDateTime(Some(d)) => {
            Cell::Date(date_time(i64::from(d.days()) + DAYS_TO_1900, u64::from(d.seconds_fragments()) * 60))
        }
        // datetime2: days since 0001-01-01 and increments of 10^-scale s since midnight.
        ColumnData::DateTime2(Some(d)) => {
            let seconds = d.time().increments() / 10u64.pow(u32::from(d.time().scale()));
            Cell::Date(date_time(i64::from(d.date().days()) + DAYS_TO_0001, seconds))
        }
        ColumnData::Date(Some(d)) => Cell::Date(date_time(i64::from(d.days()) + DAYS_TO_0001, 0)),
        ColumnData::U8(None)
        | ColumnData::I16(None)
        | ColumnData::I32(None)
        | ColumnData::I64(None)
        | ColumnData::F32(None)
        | ColumnData::F64(None)
        | ColumnData::Numeric(None)
        | ColumnData::Bit(None)
        | ColumnData::String(None)
        | ColumnData::DateTime(None)
        | ColumnData::SmallDateTime(None)
        | ColumnData::DateTime2(None)
        | ColumnData::Date(None)
        | ColumnData::Guid(None)
        | ColumnData::Binary(None)
        | ColumnData::Xml(None) => Cell::Null,
        _ => Cell::Other,
    }
}

/// Days from 1970-01-01 back to 1900-01-01 and to 0001-01-01.
const DAYS_TO_1900: i64 = -25_567;
const DAYS_TO_0001: i64 = -719_162;

/// A date and time from days since 1970-01-01 and seconds since midnight (Howard Hinnant's civil_from_days).
fn date_time(days: i64, seconds: u64) -> LocalDateTime {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let seconds = seconds % 86_400;
    LocalDateTime::new(
        year as u16,
        month as u16,
        day as u16,
        (seconds / 3600) as u16,
        (seconds / 60 % 60) as u16,
        (seconds % 60) as u16,
    )
}

/// `Convert.ToString` of the cell; None for NULL.
fn text(row: &[Cell], i: usize) -> Option<String> {
    match row.get(i)? {
        Cell::Text(t) => Some(t.clone()),
        Cell::Int(n) => Some(n.to_string()),
        Cell::Number(n) => Some(n.to_string()),
        Cell::Bit(b) => Some(if *b { "True" } else { "False" }.into()),
        Cell::Date(d) => Some(dnnm_diagnostics::format::general(d)),
        Cell::Null | Cell::Other => None,
    }
}

fn int(row: &[Cell], i: usize) -> Option<i64> {
    match row.get(i)? {
        Cell::Int(n) => Some(*n),
        Cell::Number(n) => Some(*n as i64),
        Cell::Bit(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn number(row: &[Cell], i: usize) -> Option<f64> {
    match row.get(i)? {
        Cell::Number(n) => Some(*n),
        Cell::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// `Convert.ToBoolean` of the cell; None for NULL.
fn boolean(row: &[Cell], i: usize) -> Option<bool> {
    match row.get(i)? {
        Cell::Bit(b) => Some(*b),
        Cell::Int(n) => Some(*n != 0),
        _ => None,
    }
}

fn date(row: &[Cell], i: usize) -> Option<LocalDateTime> {
    match row.get(i)? {
        Cell::Date(d) => Some(*d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_get_dnns_qualifier() {
        assert_eq!(
            with_tables(queries::VERSION, "dnn_"),
            "SELECT TOP 1 CONCAT(Major, '.', Minor, '.', Build), CreatedDate, (SELECT MIN(CreatedDate) FROM dbo.[dnn_Version]) FROM dbo.[dnn_Version] ORDER BY Major DESC, Minor DESC, Build DESC"
        );
        assert_eq!(
            with_tables(queries::PORTAL_COLUMNS, ""),
            "SELECT OBJECT_ID('dbo.[PortalLocalization]'), COL_LENGTH('dbo.[PortalAlias]', 'IsPrimary'), COL_LENGTH('dbo.[PortalAlias]', 'CultureCode')"
        );
    }

    #[test]
    fn sql_servers_dates_and_numbers_are_read() {
        // 2026-11-16 is day 46,340 after 1900-01-01; 13:45:30 is 49,530 s, 14,859,000 fragments.
        let at = cell(&ColumnData::DateTime(Some(tiberius::time::DateTime::new(46_340, 14_859_000))));
        assert_eq!(at, Cell::Date(LocalDateTime::new(2026, 11, 16, 13, 45, 30)));
        assert_eq!(date_time(DAYS_TO_0001, 0), LocalDateTime::new(1, 1, 1, 0, 0, 0));
        assert_eq!(date_time(0, 86_399), LocalDateTime::new(1970, 1, 1, 23, 59, 59));
        assert_eq!(cell(&ColumnData::I32(None)), Cell::Null);
        let row = vec![Cell::Int(1), Cell::Bit(true), Cell::Number(10.5), Cell::Null];
        assert_eq!((text(&row, 0).as_deref(), boolean(&row, 0), boolean(&row, 1)), (Some("1"), Some(true), Some(true)));
        assert_eq!((number(&row, 2), int(&row, 2), text(&row, 3), number(&row, 9)), (Some(10.5), Some(10), None, None));
    }

    #[test]
    fn names_differing_in_case_are_one() {
        let mut map = BTreeMap::new();
        insert_ignoring_case(&mut map, "Module".to_string(), 1);
        insert_ignoring_case(&mut map, "module".to_string(), 2);
        assert_eq!(map, BTreeMap::from([("Module".to_string(), 2)]));
    }

    #[test]
    #[ignore = "needs a SQL Server that answers on localhost,1433 - read-only"]
    fn reads_a_real_database() {
        let c = dnnm_infrastructure::web_config::parse_connection_string(
            "Server=localhost,1433;Database=master;Integrated Security=True",
        )
        .unwrap();
        let found = inspect_database(&c);
        assert!(found.problem.is_none(), "{:?}", found.problem);
        assert!(found.server.is_some() && found.database.is_some());
    }

    #[test]
    fn a_server_that_isnt_there_is_the_problem() {
        let c = dnnm_infrastructure::web_config::parse_connection_string(
            "Server=127.0.0.1,1;Database=x;User ID=u;Password=p",
        )
        .unwrap();
        let found = inspect_database(&c);
        assert!(found.problem.is_some() && found.server.is_none() && found.notes.is_empty());
    }
}
