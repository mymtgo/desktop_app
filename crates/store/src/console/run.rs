//! Running one console statement (spec section 5.3).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, ErrorCode};

use super::connection::take_denial;

/// One cell of a console result. A blob is shown by its length only.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(usize),
}

/// A named parameter's value as the user typed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleParam {
    pub name: String,
    pub value: Option<String>,
}

/// One statement to run.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleQuery {
    pub sql: String,
    pub params: Vec<ConsoleParam>,
    pub max_rows: Option<usize>,
    pub timeout: Duration,
}

/// A finished query.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<ConsoleValue>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// Why a console query did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleError {
    MultipleStatements,
    /// The text holds no statement (empty, spaces or only comments).
    Empty,
    /// A positional parameter (`?` or `?1`): the console binds names only.
    Positional,
    NotAllowed(String),
    TimedOut(Duration),
    Cancelled,
    Sql(String),
}

impl fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsoleError::MultipleStatements => write!(f, "Run one statement at a time"),
            ConsoleError::Empty => write!(f, "Write a query to run"),
            ConsoleError::Positional => write!(f, "Use named parameters (:name)"),
            ConsoleError::NotAllowed(what) => write!(f, "Not allowed in the console: {what}"),
            ConsoleError::TimedOut(after) => write!(f, "Query stopped after {} s", after.as_secs()),
            ConsoleError::Cancelled => write!(f, "Query cancelled"),
            ConsoleError::Sql(message) => write!(f, "{message}"),
        }
    }
}

/// Runs `query` and keeps at most `max_rows` rows; `truncated` says more
/// rows were left unread. The statement stops early when `query.timeout`
/// passes or `cancel` is set.
pub fn run_query(
    conn: &Connection,
    query: &ConsoleQuery,
    cancel: &Arc<AtomicBool>,
) -> Result<ConsoleResult, ConsoleError> {
    let started = Instant::now();
    let mut columns = Vec::new();
    let mut rows: Vec<Vec<ConsoleValue>> = Vec::new();
    let mut truncated = false;
    stream(
        conn,
        query,
        cancel,
        &mut |names| {
            columns = names.to_vec();
            Ok(())
        },
        &mut |row| {
            if let Some(max_rows) = query.max_rows
                && rows.len() >= max_rows
            {
                truncated = true;
                return Ok(Flow::Stop);
            }
            rows.push(row.to_vec());
            Ok(Flow::Next)
        },
    )?;
    Ok(ConsoleResult {
        columns,
        rows,
        truncated,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// Runs `query` and hands every row to `on_row`, after the column names to
/// `on_columns`. Every row is streamed: `max_rows` is `run_query`'s cap and
/// is not applied here. An I/O error from either callback stops the query
/// and comes back as `ConsoleError::Sql` with its message.
pub fn for_each_row(
    conn: &Connection,
    query: &ConsoleQuery,
    cancel: &Arc<AtomicBool>,
    on_columns: &mut dyn FnMut(&[String]) -> std::io::Result<()>,
    on_row: &mut dyn FnMut(&[ConsoleValue]) -> std::io::Result<()>,
) -> Result<(), ConsoleError> {
    stream(conn, query, cancel, on_columns, &mut |row| {
        on_row(row).map(|()| Flow::Next)
    })
}

/// The named parameters of `sql` (`:name`, `@name` or `$name`) without their
/// prefix, each once, in order of first use. A positional parameter (`?`,
/// `?1`) is refused, as a run refuses it.
pub fn parameters(conn: &Connection, sql: &str) -> Result<Vec<String>, ConsoleError> {
    take_denial();
    let stmt = conn
        .prepare(sql)
        .map_err(|error| console_error(Failure::Sqlite(error), Duration::ZERO, false))?;
    let mut names: Vec<String> = Vec::new();
    for index in 1..=stmt.parameter_count() {
        let Some(name) = stmt.parameter_name(index).and_then(strip_prefix) else {
            return Err(ConsoleError::Positional);
        };
        if !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Whether the stream reads another row.
enum Flow {
    Next,
    Stop,
}

/// Why the inner run stopped.
enum Failure {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// A refusal made before the statement ran.
    Console(ConsoleError),
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Failure::Sqlite(error)
    }
}

/// Installs the deadline and cancel check, runs the statement, then removes
/// the check so a later run on the same connection starts clean.
fn stream(
    conn: &Connection,
    query: &ConsoleQuery,
    cancel: &Arc<AtomicBool>,
    on_columns: &mut dyn FnMut(&[String]) -> std::io::Result<()>,
    on_row: &mut dyn FnMut(&[ConsoleValue]) -> std::io::Result<Flow>,
) -> Result<(), ConsoleError> {
    let deadline = Instant::now() + query.timeout;
    let flag = Arc::clone(cancel);
    let installed = conn.progress_handler(
        PROGRESS_STEPS,
        Some(move || flag.load(Ordering::Relaxed) || Instant::now() > deadline),
    );
    let outcome = match installed {
        Ok(()) => step_rows(conn, query, on_columns, on_row),
        Err(error) => Err(Failure::Sqlite(error)),
    };
    let removed = conn.progress_handler(PROGRESS_STEPS, None::<fn() -> bool>);
    let cancelled = cancel.load(Ordering::Relaxed);
    outcome.map_err(|failure| console_error(failure, query.timeout, cancelled))?;
    removed.map_err(|error| console_error(Failure::Sqlite(error), query.timeout, cancelled))
}

/// How many SQLite virtual machine steps run between deadline checks.
const PROGRESS_STEPS: i32 = 1000;

fn step_rows(
    conn: &Connection,
    query: &ConsoleQuery,
    on_columns: &mut dyn FnMut(&[String]) -> std::io::Result<()>,
    on_row: &mut dyn FnMut(&[ConsoleValue]) -> std::io::Result<Flow>,
) -> Result<(), Failure> {
    take_denial();
    let mut stmt = conn.prepare(&query.sql)?;
    if is_empty(&stmt) {
        return Err(Failure::Console(ConsoleError::Empty));
    }
    let mut bindings = Vec::new();
    for index in 1..=stmt.parameter_count() {
        let Some(name) = stmt.parameter_name(index).and_then(strip_prefix) else {
            return Err(Failure::Console(ConsoleError::Positional));
        };
        let text = query
            .params
            .iter()
            .find(|param| param.name == name)
            .and_then(|param| param.value.as_deref());
        bindings.push((index, bind_value(text)));
    }
    for (index, value) in bindings {
        stmt.raw_bind_parameter(index, value)?;
    }

    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|name| name.to_string())
        .collect();
    on_columns(&columns).map_err(Failure::Io)?;

    let column_count = columns.len();
    let mut rows = stmt.raw_query();
    let mut values = Vec::with_capacity(column_count);
    while let Some(row) = rows.next()? {
        values.clear();
        for index in 0..column_count {
            values.push(console_value(row.get_ref(index)?));
        }
        match on_row(&values).map_err(Failure::Io)? {
            Flow::Next => {}
            Flow::Stop => break,
        }
    }
    Ok(())
}

/// A parameter's typed text as a SQLite value: empty or missing is NULL,
/// then an integer, then a finite real, else text.
fn bind_value(text: Option<&str>) -> Value {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Value::Null;
    };
    if let Ok(integer) = text.parse::<i64>() {
        return Value::Integer(integer);
    }
    if let Ok(real) = text.parse::<f64>()
        && real.is_finite()
    {
        return Value::Real(real);
    }
    Value::Text(text.to_string())
}

fn console_value(value: ValueRef<'_>) -> ConsoleValue {
    match value {
        ValueRef::Null => ConsoleValue::Null,
        ValueRef::Integer(integer) => ConsoleValue::Integer(integer),
        ValueRef::Real(real) => ConsoleValue::Real(real),
        ValueRef::Text(bytes) => ConsoleValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => ConsoleValue::Blob(bytes.len()),
    }
}

/// Whether `stmt` holds no statement: SQLite prepares empty text, spaces
/// or only comments to nothing, and such a statement has no SQL text.
fn is_empty(stmt: &rusqlite::Statement<'_>) -> bool {
    stmt.expanded_sql().is_none()
}

/// A parameter name without its `:`, `@` or `$`; `None` for positional
/// parameters (`?` has no name, `?1` keeps its `?`).
fn strip_prefix(name: &str) -> Option<&str> {
    name.strip_prefix([':', '@', '$'])
}

fn console_error(failure: Failure, timeout: Duration, cancelled: bool) -> ConsoleError {
    let error = match failure {
        Failure::Io(error) => return ConsoleError::Sql(error.to_string()),
        Failure::Console(error) => return error,
        Failure::Sqlite(error) => error,
    };
    match &error {
        rusqlite::Error::MultipleStatement => ConsoleError::MultipleStatements,
        rusqlite::Error::SqliteFailure(code, _) if code.code == ErrorCode::OperationInterrupted => {
            if cancelled {
                ConsoleError::Cancelled
            } else {
                ConsoleError::TimedOut(timeout)
            }
        }
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == ErrorCode::AuthorizationForStatementDenied =>
        {
            ConsoleError::NotAllowed(take_denial().unwrap_or_else(|| "this statement".to_string()))
        }
        // A denied function comes back as a plain SQL input error ("not
        // authorized to use function"), so any failure after a denial is one.
        _ => match take_denial() {
            Some(what) => ConsoleError::NotAllowed(what),
            None => ConsoleError::Sql(error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::open_console;
    use crate::test_support::db;
    use fixture_tools::TestDir;

    /// A migrated database reopened as a console connection.
    fn console(label: &str) -> (TestDir, Connection) {
        let (dir, writer) = db(label);
        drop(writer);
        let conn = open_console(&dir.join("mymtgo.sqlite")).expect("console");
        (dir, conn)
    }

    fn query(sql: &str) -> ConsoleQuery {
        ConsoleQuery {
            sql: sql.to_string(),
            params: Vec::new(),
            max_rows: None,
            timeout: Duration::from_secs(10),
        }
    }

    fn run(conn: &Connection, query: &ConsoleQuery) -> Result<ConsoleResult, ConsoleError> {
        run_query(conn, query, &Arc::new(AtomicBool::new(false)))
    }

    const RUNAWAY: &str =
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n) SELECT count(*) FROM n";

    #[test]
    fn select_returns_columns_and_rows() {
        let (_dir, conn) = console("console-select");
        let result = run(
            &conn,
            &query("SELECT 1 AS a, 'x' AS b, NULL AS c, x'0102' AS d"),
        )
        .expect("run");
        assert_eq!(result.columns, vec!["a", "b", "c", "d"]);
        assert_eq!(
            result.rows,
            vec![vec![
                ConsoleValue::Integer(1),
                ConsoleValue::Text("x".to_string()),
                ConsoleValue::Null,
                ConsoleValue::Blob(2),
            ]]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn a_value_past_the_length_limit_is_refused_not_allocated() {
        let (_dir, conn) = console("console-too-big");
        let fits = format!(
            "SELECT length(zeroblob({}))",
            crate::console::connection::MAX_VALUE_BYTES
        );
        assert_eq!(
            run(&conn, &query(&fits)).expect("at the limit").rows,
            vec![vec![ConsoleValue::Integer(i64::from(
                crate::console::connection::MAX_VALUE_BYTES
            ))]]
        );
        let error = run(&conn, &query("SELECT hex(zeroblob(500000000))")).expect_err("too big");
        assert!(
            matches!(&error, ConsoleError::Sql(message) if message.contains("too big")),
            "{error:?}"
        );
    }

    #[test]
    fn a_real_comes_back_as_a_real() {
        let (_dir, conn) = console("console-real");
        let result = run(&conn, &query("SELECT 1.5")).expect("run");
        assert_eq!(result.rows, vec![vec![ConsoleValue::Real(1.5)]]);
    }

    #[test]
    fn row_cap_sets_truncated() {
        let (_dir, conn) = console("console-cap");
        let sql = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 1500) \
                   SELECT i FROM n";
        let capped = run(
            &conn,
            &ConsoleQuery {
                max_rows: Some(1000),
                ..query(sql)
            },
        )
        .expect("capped");
        assert_eq!(capped.rows.len(), 1000);
        assert!(capped.truncated);

        let all = run(&conn, &query(sql)).expect("all");
        assert_eq!(all.rows.len(), 1500);
        assert!(!all.truncated);
    }

    #[test]
    fn a_cap_equal_to_the_row_count_is_not_truncated() {
        let (_dir, conn) = console("console-cap-exact");
        let result = run(
            &conn,
            &ConsoleQuery {
                max_rows: Some(2),
                ..query("SELECT 1 UNION ALL SELECT 2")
            },
        )
        .expect("run");
        assert_eq!(result.rows.len(), 2);
        assert!(!result.truncated);
    }

    #[test]
    fn writes_and_escapes_are_refused() {
        let (dir, conn) = console("console-refused");
        for sql in [
            "INSERT INTO settings(key, value) VALUES ('k','v')",
            "DELETE FROM matches",
        ] {
            let error = run(&conn, &query(sql)).expect_err(sql);
            assert!(
                matches!(error, ConsoleError::NotAllowed(_) | ConsoleError::Sql(_)),
                "{sql}: {error:?}"
            );
        }
        for sql in [
            "CREATE TEMP TABLE t(a)",
            "ATTACH ':memory:' AS x",
            "PRAGMA query_only = OFF",
            "PRAGMA writable_schema = ON",
        ] {
            let error = run(&conn, &query(sql)).expect_err(sql);
            assert!(
                matches!(error, ConsoleError::NotAllowed(_)),
                "{sql}: {error:?}"
            );
        }
        let error = run(&conn, &query("SELECT load_extension('x')")).expect_err("extension");
        assert_eq!(
            error,
            ConsoleError::NotAllowed("load_extension".to_string())
        );

        let copy = dir.join("x.db");
        let sql = format!("VACUUM INTO '{}'", copy.display());
        let error = run(&conn, &query(&sql)).expect_err("vacuum into");
        // VACUUM INTO attaches its target internally; the authorizer sees the
        // same ATTACH a user would write, so the refusal names ATTACH.
        assert_eq!(error, ConsoleError::NotAllowed("ATTACH".to_string()));
        assert!(!copy.exists());

        let result = run(&conn, &query("SELECT count(*) FROM matches")).expect("still reads");
        assert_eq!(result.rows, vec![vec![ConsoleValue::Integer(0)]]);
    }

    #[test]
    fn a_refusal_names_the_action() {
        let (_dir, conn) = console("console-refusal-name");
        let attach = run(&conn, &query("ATTACH ':memory:' AS x")).expect_err("attach");
        assert_eq!(attach, ConsoleError::NotAllowed("ATTACH".to_string()));
        assert_eq!(attach.to_string(), "Not allowed in the console: ATTACH");
        let pragma = run(&conn, &query("PRAGMA query_only = OFF")).expect_err("pragma");
        assert_eq!(
            pragma,
            ConsoleError::NotAllowed("PRAGMA query_only".to_string())
        );
    }

    #[test]
    fn table_info_is_allowed() {
        let (_dir, conn) = console("console-table-info");
        let result = run(&conn, &query("PRAGMA table_info(matches)")).expect("table_info");
        assert!(!result.rows.is_empty());
    }

    #[test]
    fn trailing_semicolon_and_comment_are_one_statement() {
        let (_dir, conn) = console("console-one-statement");
        let result = run(&conn, &query("SELECT 1;\n-- done\n")).expect("one statement");
        assert_eq!(result.rows, vec![vec![ConsoleValue::Integer(1)]]);
        let error = run(&conn, &query("SELECT 1; SELECT 2")).expect_err("two statements");
        assert_eq!(error, ConsoleError::MultipleStatements);
        assert_eq!(error.to_string(), "Run one statement at a time");
    }

    #[test]
    fn a_query_with_no_statement_asks_for_one() {
        let (_dir, conn) = console("console-empty");
        for sql in ["", "   \n", "-- only a comment", "/* note */ \n -- more\n"] {
            let error = run(&conn, &query(sql)).expect_err(sql);
            assert_eq!(error, ConsoleError::Empty, "{sql:?}");
            assert_eq!(error.to_string(), "Write a query to run");
        }
    }

    #[test]
    fn positional_parameters_are_refused() {
        let (_dir, conn) = console("console-positional");
        for sql in ["SELECT ?", "SELECT ?1", "SELECT :a, ?"] {
            let error = run(&conn, &query(sql)).expect_err(sql);
            assert_eq!(error, ConsoleError::Positional, "{sql}");
            assert_eq!(error.to_string(), "Use named parameters (:name)");
            assert_eq!(
                parameters(&conn, sql),
                Err(ConsoleError::Positional),
                "{sql}"
            );
        }
    }

    #[test]
    fn runaway_query_times_out() {
        let (_dir, conn) = console("console-timeout");
        let started = Instant::now();
        let error = run(
            &conn,
            &ConsoleQuery {
                timeout: Duration::from_millis(200),
                ..query(RUNAWAY)
            },
        )
        .expect_err("runaway");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(error, ConsoleError::TimedOut(Duration::from_millis(200)));
    }

    #[test]
    fn a_timeout_does_not_leak_into_the_next_run() {
        let (_dir, conn) = console("console-timeout-reset");
        let _ = run(
            &conn,
            &ConsoleQuery {
                timeout: Duration::from_millis(50),
                ..query(RUNAWAY)
            },
        );
        std::thread::sleep(Duration::from_millis(100));
        let sql = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 100000) \
                   SELECT count(*) FROM n";
        let result = run(&conn, &query(sql)).expect("later run");
        assert_eq!(result.rows, vec![vec![ConsoleValue::Integer(100_000)]]);
    }

    #[test]
    fn cancel_flag_cancels() {
        let (_dir, conn) = console("console-cancel");
        let error = run_query(&conn, &query(RUNAWAY), &Arc::new(AtomicBool::new(true)))
            .expect_err("cancelled");
        assert_eq!(error, ConsoleError::Cancelled);
        assert_eq!(error.to_string(), "Query cancelled");
    }

    #[test]
    fn timeout_message_names_the_seconds() {
        assert_eq!(
            ConsoleError::TimedOut(Duration::from_secs(10)).to_string(),
            "Query stopped after 10 s"
        );
    }

    #[test]
    fn parameters_are_listed_once_and_bound() {
        let (_dir, conn) = console("console-params");
        let sql = "SELECT :a, :b, :a";
        assert_eq!(parameters(&conn, sql).expect("names"), vec!["a", "b"]);
        let result = run(
            &conn,
            &ConsoleQuery {
                params: vec![
                    ConsoleParam {
                        name: "a".to_string(),
                        value: Some("5".to_string()),
                    },
                    ConsoleParam {
                        name: "b".to_string(),
                        value: Some(String::new()),
                    },
                ],
                ..query(sql)
            },
        )
        .expect("run");
        assert_eq!(
            result.rows,
            vec![vec![
                ConsoleValue::Integer(5),
                ConsoleValue::Null,
                ConsoleValue::Integer(5),
            ]]
        );
    }

    #[test]
    fn parameter_text_binds_as_real_or_text() {
        let (_dir, conn) = console("console-param-types");
        let result = run(
            &conn,
            &ConsoleQuery {
                params: vec![
                    ConsoleParam {
                        name: "r".to_string(),
                        value: Some("2.5".to_string()),
                    },
                    ConsoleParam {
                        name: "t".to_string(),
                        value: Some("Modern".to_string()),
                    },
                    ConsoleParam {
                        name: "n".to_string(),
                        value: Some("nan".to_string()),
                    },
                ],
                ..query("SELECT @r, $t, :n, :missing")
            },
        )
        .expect("run");
        assert_eq!(
            result.rows,
            vec![vec![
                ConsoleValue::Real(2.5),
                ConsoleValue::Text("Modern".to_string()),
                ConsoleValue::Text("nan".to_string()),
                ConsoleValue::Null,
            ]]
        );
    }

    #[test]
    fn for_each_row_streams_every_row() {
        let (_dir, conn) = console("console-stream");
        let mut columns = Vec::new();
        let mut rows = Vec::new();
        for_each_row(
            &conn,
            &ConsoleQuery {
                max_rows: Some(1),
                ..query("SELECT 1 AS a UNION ALL SELECT 2")
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |names| {
                columns = names.to_vec();
                Ok(())
            },
            &mut |row| {
                rows.push(row.to_vec());
                Ok(())
            },
        )
        .expect("stream");
        assert_eq!(columns, vec!["a"]);
        assert_eq!(
            rows,
            vec![
                vec![ConsoleValue::Integer(1)],
                vec![ConsoleValue::Integer(2)]
            ]
        );
    }

    #[test]
    fn a_write_error_from_the_caller_stops_the_stream() {
        let (_dir, conn) = console("console-stream-error");
        let error = for_each_row(
            &conn,
            &query("SELECT 1"),
            &Arc::new(AtomicBool::new(false)),
            &mut |_| Ok(()),
            &mut |_| Err(std::io::Error::other("disk full")),
        )
        .expect_err("io error");
        assert_eq!(error, ConsoleError::Sql("disk full".to_string()));
    }
}
