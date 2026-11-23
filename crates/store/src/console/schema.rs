//! The console's schema listing (spec section 5.2): the `v_*` views with
//! their column help first, then the raw tables and views.

use rusqlite::Connection;

use super::run::ConsoleError;
use super::views;

/// Whether a listed object is a table or a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    View,
    Table,
}

/// One column of a listed object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleColumn {
    pub name: String,
    /// The declared type (`INTEGER`, `TEXT`); empty when SQLite has none,
    /// as for most view columns.
    pub decl_type: String,
    /// The one-line help of a `v_*` column; `None` for raw columns.
    pub description: Option<String>,
}

/// One table or view the console can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleObject {
    pub name: String,
    pub kind: ObjectKind,
    /// A raw table or view of the database, not one of the `v_*` views.
    pub raw: bool,
    pub columns: Vec<ConsoleColumn>,
}

/// Every object the console lists: the `v_*` views first, in `views::ALL`
/// order, then the database's own tables and views by name, leaving out
/// SQLite's internal `sqlite_` tables. Reads only `PRAGMA table_list` and
/// `table_info`, which the console's authorizer allows.
pub fn schema(conn: &Connection) -> Result<Vec<ConsoleObject>, ConsoleError> {
    let mut objects = Vec::new();
    for view in views::ALL {
        let mut columns = table_columns(conn, "temp", view.name)?;
        for column in &mut columns {
            column.description = view
                .columns
                .iter()
                .find(|(name, _)| *name == column.name)
                .map(|(_, description)| description.to_string());
        }
        objects.push(ConsoleObject {
            name: view.name.to_string(),
            kind: ObjectKind::View,
            raw: false,
            columns,
        });
    }
    let mut raw = Vec::new();
    for (name, kind) in main_objects(conn)? {
        let columns = table_columns(conn, "main", &name)?;
        raw.push(ConsoleObject {
            name,
            kind,
            raw: true,
            columns,
        });
    }
    raw.sort_by(|a, b| a.name.cmp(&b.name));
    objects.extend(raw);
    Ok(objects)
}

/// The main schema's tables and views, without SQLite's own.
fn main_objects(conn: &Connection) -> Result<Vec<(String, ObjectKind)>, ConsoleError> {
    let mut stmt = conn.prepare("PRAGMA main.table_list").map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>("name")?, row.get::<_, String>("type")?))
        })
        .map_err(sql_error)?;
    let mut found = Vec::new();
    for row in rows {
        let (name, kind) = row.map_err(sql_error)?;
        if name.starts_with("sqlite_") {
            continue;
        }
        let kind = match kind.as_str() {
            "table" => ObjectKind::Table,
            "view" => ObjectKind::View,
            // Virtual and shadow tables are an extension's internals.
            _ => continue,
        };
        found.push((name, kind));
    }
    Ok(found)
}

/// The columns of `name` in the schema `database` (`main` or `temp`), in
/// order, without descriptions.
fn table_columns(
    conn: &Connection,
    database: &str,
    name: &str,
) -> Result<Vec<ConsoleColumn>, ConsoleError> {
    let sql = format!("PRAGMA {database}.table_info({})", quoted(name));
    let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ConsoleColumn {
                name: row.get("name")?,
                decl_type: row.get("type")?,
                description: None,
            })
        })
        .map_err(sql_error)?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(sql_error)?);
    }
    Ok(columns)
}

/// `name` as an SQL identifier in double quotes.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn sql_error(error: rusqlite::Error) -> ConsoleError {
    ConsoleError::Sql(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::open_console;
    use crate::test_support::db;

    #[test]
    fn schema_puts_views_first_with_descriptions() {
        let (dir, writer) = db("console-schema");
        drop(writer);
        let conn = open_console(&dir.join("mymtgo.sqlite")).expect("console");
        let objects = schema(&conn).expect("schema");

        let first = objects.first().expect("objects");
        assert!(first.name.starts_with("v_"), "{}", first.name);
        assert!(!first.raw);
        assert_eq!(first.kind, ObjectKind::View);
        let views: Vec<&ConsoleObject> = objects.iter().filter(|object| !object.raw).collect();
        assert_eq!(views.len(), crate::console::views::ALL.len());
        for view in &views {
            assert!(!view.columns.is_empty(), "{}", view.name);
            for column in &view.columns {
                assert!(
                    column.description.is_some(),
                    "{}.{}",
                    view.name,
                    column.name
                );
            }
        }
        // Every view comes before every raw object.
        let first_raw = objects.iter().position(|object| object.raw).expect("raw");
        assert!(objects[first_raw..].iter().all(|object| object.raw));

        let matches = objects
            .iter()
            .find(|object| object.name == "matches")
            .expect("matches");
        assert!(matches.raw);
        assert_eq!(matches.kind, ObjectKind::Table);
        let token = matches
            .columns
            .iter()
            .find(|column| column.name == "token")
            .expect("token");
        assert_eq!(token.decl_type, "TEXT");
        assert_eq!(token.description, None);
        let listed = objects
            .iter()
            .find(|object| object.name == "listed_matches")
            .expect("listed_matches");
        assert_eq!(listed.kind, ObjectKind::View);
        assert!(
            objects
                .iter()
                .all(|object| !object.name.starts_with("sqlite_"))
        );

        let raw_names: Vec<&str> = objects[first_raw..]
            .iter()
            .map(|object| object.name.as_str())
            .collect();
        let mut sorted = raw_names.clone();
        sorted.sort_unstable();
        assert_eq!(raw_names, sorted);
    }
}
