//! The SQL plumbing every 0.x reader shares: which tables and columns a 0.x
//! database has, select lists that read a missing column as `NULL`, and the
//! loose value converters (a number stored as text still counts, anything
//! odd is `None`, never an error).
//!
//! Rows come two ways: typed readers name each column in a row struct
//! (`value` with the `as_*` converters), and short lookups take every row as
//! its values (`rows` with `text`, `int`, `real`, `flag` and `time`).

use std::collections::HashMap;

use chrono::NaiveDateTime;
use rusqlite::types::Value;
use rusqlite::{Connection, Row};

use crate::read::{ImportError, read_error};

/// The column names of `table` (empty when the table does not exist).
pub(crate) fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, ImportError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(read_error)?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(read_error)?);
    }
    Ok(columns)
}

/// Whether `present` (a table's columns) holds `column`.
pub(crate) fn has_column(present: &[String], column: &str) -> bool {
    present.iter().any(|name| name == column)
}

/// Whether `present` (a table's columns) holds every one of `columns`.
pub(crate) fn has_all(present: &[String], columns: &[&str]) -> bool {
    columns.iter().all(|column| has_column(present, column))
}

/// Whether `table` exists with every one of `columns`.
pub(crate) fn has_columns(
    conn: &Connection,
    table: &str,
    columns: &[&str],
) -> Result<bool, ImportError> {
    Ok(has_all(&table_columns(conn, table)?, columns))
}

/// `"column", "column"` for a SQL select list, each column prefixed with
/// `prefix` (for example `gp.`; empty for none).
pub(crate) fn column_list(prefix: &str, columns: &[&str]) -> String {
    let quoted: Vec<String> = columns
        .iter()
        .map(|column| format!("{prefix}\"{column}\""))
        .collect();
    quoted.join(", ")
}

/// `"column"` when the table has it, `NULL AS "column"` otherwise.
pub(crate) fn select_list(present: &[String], columns: &[&str]) -> String {
    let mut parts = Vec::new();
    for column in columns {
        if has_column(present, column) {
            parts.push(format!("\"{column}\""));
        } else {
            parts.push(format!("NULL AS \"{column}\""));
        }
    }
    parts.join(", ")
}

/// The `accounts` column of 0.x's soft delete.
pub(crate) const ACCOUNT_DELETED_AT: &str = "deleted_at";

/// ` WHERE <account not deleted>` for an `accounts` query (`alias` is the
/// table's alias with its dot, such as `a.`, or empty), or nothing when
/// `accounts` has no `deleted_at` column (`present` holds its columns): an
/// older 0.x deleted no account. A blank `deleted_at` is not deleted.
pub(crate) fn where_account_not_deleted(present: &[String], alias: &str) -> String {
    if has_column(present, ACCOUNT_DELETED_AT) {
        format!(" WHERE trim(COALESCE({alias}\"{ACCOUNT_DELETED_AT}\", '')) = ''")
    } else {
        String::new()
    }
}

/// One cell of a row, whatever its storage type.
pub(crate) fn value(row: &Row, index: usize) -> rusqlite::Result<Value> {
    row.get::<_, Value>(index)
}

/// A value as text: numbers are written out, blobs and NULL are `None`.
pub(crate) fn as_text(value: Value) -> Option<String> {
    match value {
        Value::Text(text) => Some(text),
        Value::Integer(number) => Some(number.to_string()),
        Value::Real(number) => Some(number.to_string()),
        Value::Null | Value::Blob(_) => None,
    }
}

/// A value as an integer: integers, and text that is one.
pub(crate) fn as_integer(value: Value) -> Option<i64> {
    match value {
        Value::Integer(number) => Some(number),
        Value::Text(text) => text.trim().parse().ok(),
        Value::Real(_) | Value::Null | Value::Blob(_) => None,
    }
}

/// A value as a flag: an integer (or text that is one), not 0 for true.
pub(crate) fn as_bool(value: Value) -> Option<bool> {
    as_integer(value).map(|number| number != 0)
}

/// A value as a real number: integers, reals, and text that is one.
pub(crate) fn as_real(value: Value) -> Option<f64> {
    match value {
        Value::Integer(number) => Some(number as f64),
        Value::Real(number) => Some(number),
        Value::Text(text) => text.trim().parse().ok(),
        Value::Null | Value::Blob(_) => None,
    }
}

/// An MTGO ID that is a whole number; `manual_*` and other text is `None`.
pub(crate) fn parse_id(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

/// `YYYY-MM-DD HH:MM:SS` as UTC unix ms. Also accepts a `T` separator, a
/// fractional second and a trailing `Z`.
pub(crate) fn parse_utc_ms(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    let without_zone = trimmed.strip_suffix('Z').unwrap_or(trimmed);
    let spaced = without_zone.replacen('T', " ", 1);
    let parsed = NaiveDateTime::parse_from_str(&spaced, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    Some(parsed.and_utc().timestamp_millis())
}

/// Rows of `sql`, each as its loosely read values.
pub(crate) fn rows(conn: &Connection, sql: &str) -> Result<Vec<Vec<Value>>, ImportError> {
    let mut stmt = conn.prepare(sql).map_err(read_error)?;
    let columns = stmt.column_count();
    let found = stmt
        .query_map([], |row| {
            let mut values = Vec::new();
            for index in 0..columns {
                values.push(value(row, index)?);
            }
            Ok(values)
        })
        .map_err(read_error)?;
    let mut all = Vec::new();
    for row in found {
        all.push(row.map_err(read_error)?);
    }
    Ok(all)
}

/// The value at `index` of a `rows` row as text (see `as_text`).
pub(crate) fn text(values: &[Value], index: usize) -> Option<String> {
    values.get(index).cloned().and_then(as_text)
}

/// The value at `index` of a `rows` row as an integer (see `as_integer`).
pub(crate) fn int(values: &[Value], index: usize) -> Option<i64> {
    values.get(index).cloned().and_then(as_integer)
}

/// The value at `index` of a `rows` row as a real number (see `as_real`).
pub(crate) fn real(values: &[Value], index: usize) -> Option<f64> {
    values.get(index).cloned().and_then(as_real)
}

/// The value at `index` of a `rows` row as a flag; anything unreadable is
/// false.
pub(crate) fn flag(values: &[Value], index: usize) -> bool {
    values
        .get(index)
        .cloned()
        .and_then(as_bool)
        .unwrap_or(false)
}

/// The value at `index` of a `rows` row as a time (see `parse_utc_ms`).
pub(crate) fn time(values: &[Value], index: usize) -> Option<i64> {
    text(values, index).as_deref().and_then(parse_utc_ms)
}

/// Each 0.x deck id's MTGO id, for the data 0.x keyed by deck id (guides,
/// matchup notes, overlay labels; v1 keys decks by MTGO id). Empty without
/// a usable `decks` table. A deck with a blank MTGO id is left out: the
/// deck import leaves such a deck out too, so nothing may match it.
pub(crate) fn deck_mtgo_ids(conn: &Connection) -> Result<HashMap<i64, String>, ImportError> {
    let mut ids = HashMap::new();
    if has_columns(conn, "decks", &["id", "mtgo_id"])? {
        for row in rows(conn, "SELECT id, mtgo_id FROM decks")? {
            if let (Some(id), Some(mtgo_id)) = (int(&row, 0), text(&row, 1))
                && !mtgo_id.trim().is_empty()
            {
                ids.insert(id, mtgo_id);
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_missing_column_is_selected_as_null() {
        let columns = present(&["id", "name"]);
        assert_eq!(
            select_list(&columns, &["name", "notes"]),
            "\"name\", NULL AS \"notes\""
        );
        assert!(has_all(&columns, &["id", "name"]));
        assert!(!has_all(&columns, &["id", "notes"]));
    }

    #[test]
    fn the_account_clause_needs_the_deleted_column() {
        assert_eq!(where_account_not_deleted(&present(&["id"]), "a."), "");
        assert_eq!(
            where_account_not_deleted(&present(&["id", "deleted_at"]), "a."),
            " WHERE trim(COALESCE(a.\"deleted_at\", '')) = ''"
        );
    }

    #[test]
    fn a_deck_with_a_blank_mtgo_id_is_left_out_of_the_lookup() {
        let conn = Connection::open_in_memory().expect("memory");
        conn.execute_batch(
            "CREATE TABLE decks (id INTEGER PRIMARY KEY, mtgo_id TEXT);
             INSERT INTO decks VALUES (1, 'q-deck'), (2, ''), (3, '  '), (4, NULL);",
        )
        .expect("decks");
        let ids = deck_mtgo_ids(&conn).expect("ids");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get(&1).map(String::as_str), Some("q-deck"));
    }
}
