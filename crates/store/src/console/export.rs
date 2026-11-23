//! Streams a console query's full result to a writer as CSV or JSON, one
//! row at a time (spec section 5). No row cap applies.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rusqlite::Connection;

use super::run::{ConsoleError, ConsoleQuery, ConsoleValue, for_each_row};

/// The file formats the console can export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

/// Writes every row of `query` to `out` and returns how many rows it wrote.
/// A non-finite real (NaN, infinity) is written as an empty CSV field or a
/// JSON `null`, because JSON cannot hold it. A repeated column name keeps
/// its CSV header as it is, and becomes `name_2`, `name_3` and so on as a
/// JSON key, so no key is lost. Each row is written in small pieces, so a
/// caller writing to a file wraps it in a `BufWriter`.
pub fn export(
    conn: &Connection,
    query: &ConsoleQuery,
    format: ExportFormat,
    cancel: &Arc<AtomicBool>,
    out: &mut dyn Write,
) -> Result<u64, ConsoleError> {
    let out = RefCell::new(out);
    let columns: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let count = Cell::new(0u64);

    let result = for_each_row(
        conn,
        query,
        cancel,
        &mut |names| {
            *columns.borrow_mut() = match format {
                ExportFormat::Csv => names.to_vec(),
                ExportFormat::Json => unique_keys(names),
            };
            let mut out = out.borrow_mut();
            match format {
                ExportFormat::Csv => {
                    let line: Vec<String> = names.iter().map(|name| csv_field(name)).collect();
                    out.write_all(line.join(",").as_bytes())?;
                    out.write_all(b"\r\n")
                }
                ExportFormat::Json => out.write_all(b"["),
            }
        },
        &mut |row| {
            let mut out = out.borrow_mut();
            match format {
                ExportFormat::Csv => {
                    let line: Vec<String> = row.iter().map(csv_value).collect();
                    out.write_all(line.join(",").as_bytes())?;
                    out.write_all(b"\r\n")?;
                }
                ExportFormat::Json => {
                    if count.get() > 0 {
                        out.write_all(b",")?;
                    }
                    let names = columns.borrow();
                    out.write_all(json_object(&names, row).as_bytes())?;
                }
            }
            count.set(count.get() + 1);
            Ok(())
        },
    );
    result?;

    if format == ExportFormat::Json {
        out.borrow_mut()
            .write_all(b"]")
            .map_err(|error| ConsoleError::Sql(error.to_string()))?;
    }
    out.borrow_mut()
        .flush()
        .map_err(|error| ConsoleError::Sql(error.to_string()))?;
    Ok(count.get())
}

fn blob_text(size: usize) -> String {
    format!("<blob {size} bytes>")
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

fn csv_value(value: &ConsoleValue) -> String {
    match value {
        ConsoleValue::Null => String::new(),
        ConsoleValue::Integer(number) => number.to_string(),
        ConsoleValue::Real(number) if number.is_finite() => number.to_string(),
        ConsoleValue::Real(_) => String::new(),
        ConsoleValue::Text(text) => csv_field(text),
        ConsoleValue::Blob(size) => csv_field(&blob_text(*size)),
    }
}

fn json_text(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

fn json_value(value: &ConsoleValue) -> String {
    match value {
        ConsoleValue::Null => "null".to_string(),
        ConsoleValue::Integer(number) => number.to_string(),
        ConsoleValue::Real(number) => serde_json::Number::from_f64(*number)
            .map_or_else(|| "null".to_string(), |number| number.to_string()),
        ConsoleValue::Text(text) => json_text(text),
        ConsoleValue::Blob(size) => json_text(&blob_text(*size)),
    }
}

/// The column names as JSON keys: a name seen before gets the first free
/// suffix from `_2` up, so the second `match_id` is `match_id_2` (or
/// `match_id_3` when a column is already called `match_id_2`).
fn unique_keys(names: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        if !keys.contains(name) {
            keys.push(name.clone());
            continue;
        }
        let mut suffix = 2;
        loop {
            let candidate = format!("{name}_{suffix}");
            let later = names[index + 1..].contains(&candidate);
            if !keys.contains(&candidate) && !later {
                keys.push(candidate);
                break;
            }
            suffix += 1;
        }
    }
    keys
}

/// One row as a JSON object with its keys in column order.
fn json_object(names: &[String], row: &[ConsoleValue]) -> String {
    let fields: Vec<String> = names
        .iter()
        .zip(row)
        .map(|(name, value)| format!("{}:{}", json_text(name), json_value(value)))
        .collect();
    format!("{{{}}}", fields.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::open_console;
    use crate::test_support::db;
    use fixture_tools::TestDir;
    use std::time::Duration;

    fn console(label: &str) -> (TestDir, Connection) {
        let (dir, writer) = db(label);
        drop(writer);
        let conn = open_console(&dir.join("mymtgo.sqlite")).expect("console");
        (dir, conn)
    }

    fn export_text(conn: &Connection, sql: &str, format: ExportFormat) -> (String, u64) {
        let query = ConsoleQuery {
            sql: sql.to_string(),
            params: Vec::new(),
            max_rows: None,
            timeout: Duration::from_secs(10),
        };
        let mut out: Vec<u8> = Vec::new();
        let rows = export(
            conn,
            &query,
            format,
            &Arc::new(AtomicBool::new(false)),
            &mut out,
        )
        .expect("export");
        (String::from_utf8(out).expect("utf8"), rows)
    }

    #[test]
    fn csv_quotes_and_nulls() {
        let (_dir, conn) = console("export-csv");
        let (text, rows) = export_text(
            &conn,
            "SELECT 'a,b' AS x, 'say \"hi\"' AS y, 'l1' || char(10) || 'l2' AS z, NULL AS n",
            ExportFormat::Csv,
        );
        assert_eq!(
            text,
            "x,y,z,n\r\n\"a,b\",\"say \"\"hi\"\"\",\"l1\nl2\",\r\n"
        );
        assert_eq!(rows, 1);
    }

    #[test]
    fn json_shape() {
        let (_dir, conn) = console("export-json");
        let (text, rows) = export_text(&conn, "SELECT 1 x, 't' y, NULL n", ExportFormat::Json);
        assert_eq!(text, r#"[{"x":1,"y":"t","n":null}]"#);
        assert_eq!(rows, 1);
    }

    #[test]
    fn json_with_no_rows_is_an_empty_array() {
        let (_dir, conn) = console("export-json-empty");
        let (text, rows) = export_text(&conn, "SELECT 1 x WHERE 0", ExportFormat::Json);
        assert_eq!(text, "[]");
        assert_eq!(rows, 0);
    }

    #[test]
    fn blobs_and_non_finite_reals() {
        let (_dir, conn) = console("export-blob");
        let sql = "SELECT x'0102' AS b, 1e999 AS inf, 1.5 AS r";
        let (csv, _) = export_text(&conn, sql, ExportFormat::Csv);
        assert_eq!(csv, "b,inf,r\r\n<blob 2 bytes>,,1.5\r\n");
        let (json, _) = export_text(&conn, sql, ExportFormat::Json);
        assert_eq!(json, r#"[{"b":"<blob 2 bytes>","inf":null,"r":1.5}]"#);
    }

    #[test]
    fn json_suffixes_repeated_column_names_and_csv_keeps_them() {
        let (_dir, conn) = console("export-repeated");
        let sql = "SELECT 1 AS match_id, 2 AS match_id, 3 AS match_id_2, 4 AS match_id";
        let (json, _) = export_text(&conn, sql, ExportFormat::Json);
        assert_eq!(
            json,
            r#"[{"match_id":1,"match_id_3":2,"match_id_2":3,"match_id_4":4}]"#
        );
        let (csv, _) = export_text(&conn, sql, ExportFormat::Csv);
        assert_eq!(csv, "match_id,match_id,match_id_2,match_id\r\n1,2,3,4\r\n");
    }

    #[test]
    fn unique_keys_skip_a_taken_suffix() {
        let names: Vec<String> = ["a", "a_2", "a", "a"].map(String::from).to_vec();
        assert_eq!(unique_keys(&names), vec!["a", "a_2", "a_3", "a_4"]);
        let names: Vec<String> = ["id", "id", "id"].map(String::from).to_vec();
        assert_eq!(unique_keys(&names), vec!["id", "id_2", "id_3"]);
    }

    #[test]
    fn json_escapes_values_and_keys() {
        let (_dir, conn) = console("export-json-escape");
        // The column name holds a quote, a backslash, a tab and a non-ASCII
        // letter; the value a quote, a backslash, a tab, U+0001 and two
        // non-ASCII characters.
        let sql = "SELECT 'q\"b\\' || char(9) || char(1) || 'é✓' AS \"k\"\"\\\tü\"";
        let (json, _) = export_text(&conn, sql, ExportFormat::Json);
        assert_eq!(json, r#"[{"k\"\\\tü":"q\"b\\\t\u0001é✓"}]"#);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed[0]["k\"\\\tü"], "q\"b\\\t\u{1}é✓");
    }

    #[test]
    fn csv_quotes_a_carriage_return_and_a_header_that_needs_it() {
        let (_dir, conn) = console("export-csv-cr");
        let (text, _) = export_text(
            &conn,
            "SELECT 'a' || char(13) || 'b' AS \"x, y\", 1 AS \"say \"\"hi\"\"\"",
            ExportFormat::Csv,
        );
        assert_eq!(text, "\"x, y\",\"say \"\"hi\"\"\"\r\n\"a\rb\",1\r\n");
    }

    #[test]
    fn export_has_no_row_cap() {
        let (_dir, conn) = console("export-no-cap");
        let sql = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 1500) \
                   SELECT i FROM n";
        let query = ConsoleQuery {
            sql: sql.to_string(),
            params: Vec::new(),
            max_rows: Some(10),
            timeout: Duration::from_secs(10),
        };
        let mut out: Vec<u8> = Vec::new();
        let rows = export(
            &conn,
            &query,
            ExportFormat::Csv,
            &Arc::new(AtomicBool::new(false)),
            &mut out,
        )
        .expect("export");
        assert_eq!(rows, 1500);
    }
}
