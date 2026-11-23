//! Helpers shared by the import tests: a 0.x database built from a schema,
//! the v1 database it imports into, and one-value queries.

#![allow(dead_code)]

use std::path::PathBuf;

use fixture_tools::TestDir;
use rusqlite::Connection;

/// A 0.x database in `dir` built from `schema`. Foreign keys are off: the
/// fixtures keep only some of 0.x's tables, so references can dangle.
pub fn legacy_db(dir: &TestDir, schema: &str) -> (PathBuf, Connection) {
    let path = dir.join("nativephp.sqlite");
    let conn = Connection::open(&path).expect("open 0.x test database");
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("no foreign keys");
    conn.execute_batch(schema).expect("0.x schema");
    (path, conn)
}

/// A fresh v1 database in `dir`.
pub fn v1_db(dir: &TestDir) -> Connection {
    store::open_writer(&dir.join("mymtgo.sqlite")).expect("v1 database")
}

pub fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect(sql)
}

pub fn text(conn: &Connection, sql: &str) -> Option<String> {
    conn.query_row(sql, [], |row| row.get(0)).expect(sql)
}

pub fn texts(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    stmt.query_map([], |row| row.get(0))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}
