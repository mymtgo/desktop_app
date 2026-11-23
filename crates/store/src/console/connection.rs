//! The console connection (spec section 5.1): opened read-only, the views
//! created as temp views, then an authorizer that allows only reading.

use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::{Connection, OpenFlags};

use super::views;
use crate::StoreError;

/// The longest string or blob the console connection can make or read, in
/// bytes (SQLite's default is 1 GB). The progress handler only runs between
/// VM steps, so without this one call such as `hex(zeroblob(500000000))`
/// could allocate that much in the app process before the timeout fires.
pub(crate) const MAX_VALUE_BYTES: i32 = 64 * 1024 * 1024;

/// The `PRAGMA`s the schema pane needs. Each only reads the schema.
const ALLOWED_PRAGMAS: [&str; 3] = ["table_info", "table_xinfo", "table_list"];

thread_local! {
    /// What the authorizer denied last on this thread. SQLite runs the
    /// authorizer while a statement is prepared, on the preparing thread, so
    /// the runner reads it back right after a refused prepare.
    static LAST_DENIAL: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Opens the database at `path` for the console: read-only (SQLite refuses
/// every write to the file), the console views created as temp views, then
/// the authorizer installed so user SQL can only read.
pub fn open_console(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
    for view in views::ALL {
        conn.execute_batch(&format!("CREATE TEMP VIEW {} AS {}", view.name, view.sql))?;
    }
    conn.authorizer(Some(authorize))?;
    Ok(conn)
}

/// Takes what the authorizer denied last on this thread, leaving nothing.
pub(crate) fn take_denial() -> Option<String> {
    LAST_DENIAL.with(|denial| denial.borrow_mut().take())
}

fn authorize(context: AuthContext<'_>) -> Authorization {
    if is_allowed(&context.action) {
        Authorization::Allow
    } else {
        let what = describe(&context.action);
        LAST_DENIAL.with(|denial| *denial.borrow_mut() = Some(what));
        Authorization::Deny
    }
}

/// The allow-list: reading, functions, recursive queries and the schema
/// `PRAGMA`s. Everything else is denied.
fn is_allowed(action: &AuthAction<'_>) -> bool {
    match action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Recursive => true,
        // `load_extension` is off anyway (rusqlite never enables extension
        // loading); denied here too so it never depends on that.
        AuthAction::Function { function_name } => !is_load_extension(function_name),
        // The value of these is a table name (`PRAGMA table_info(matches)`),
        // so any value is allowed.
        AuthAction::Pragma { pragma_name, .. } => ALLOWED_PRAGMAS
            .iter()
            .any(|allowed| pragma_name.eq_ignore_ascii_case(allowed)),
        _ => false,
    }
}

fn is_load_extension(function_name: &str) -> bool {
    function_name.eq_ignore_ascii_case("load_extension")
}

/// The denied action as the user would write it, for the error message.
fn describe(action: &AuthAction<'_>) -> String {
    let text = match action {
        AuthAction::Pragma { pragma_name, .. } => {
            return format!("PRAGMA {}", pragma_name.to_ascii_lowercase());
        }
        AuthAction::Function { function_name } if is_load_extension(function_name) => {
            "load_extension"
        }
        AuthAction::Insert { .. } => "INSERT",
        AuthAction::Update { .. } => "UPDATE",
        AuthAction::Delete { .. } => "DELETE",
        AuthAction::Attach { .. } => "ATTACH",
        AuthAction::Detach { .. } => "DETACH",
        AuthAction::CreateTable { .. } => "CREATE TABLE",
        AuthAction::CreateTempTable { .. } => "CREATE TEMP TABLE",
        AuthAction::CreateIndex { .. } => "CREATE INDEX",
        AuthAction::CreateTempIndex { .. } => "CREATE TEMP INDEX",
        AuthAction::CreateTrigger { .. } => "CREATE TRIGGER",
        AuthAction::CreateTempTrigger { .. } => "CREATE TEMP TRIGGER",
        AuthAction::CreateView { .. } => "CREATE VIEW",
        AuthAction::CreateTempView { .. } => "CREATE TEMP VIEW",
        AuthAction::CreateVtable { .. } => "CREATE VIRTUAL TABLE",
        AuthAction::DropTable { .. } => "DROP TABLE",
        AuthAction::DropTempTable { .. } => "DROP TEMP TABLE",
        AuthAction::DropIndex { .. } => "DROP INDEX",
        AuthAction::DropTempIndex { .. } => "DROP TEMP INDEX",
        AuthAction::DropTrigger { .. } => "DROP TRIGGER",
        AuthAction::DropTempTrigger { .. } => "DROP TEMP TRIGGER",
        AuthAction::DropView { .. } => "DROP VIEW",
        AuthAction::DropTempView { .. } => "DROP TEMP VIEW",
        AuthAction::DropVtable { .. } => "DROP VIRTUAL TABLE",
        AuthAction::AlterTable { .. } => "ALTER TABLE",
        AuthAction::Reindex { .. } => "REINDEX",
        AuthAction::Analyze { .. } => "ANALYZE",
        AuthAction::Transaction { .. } => "transactions",
        AuthAction::Savepoint { .. } => "savepoints",
        _ => "this statement",
    };
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;

    #[test]
    fn the_console_connection_cannot_write_the_file() {
        let (dir, writer) = db("console-open");
        drop(writer);
        let conn = open_console(&dir.join("mymtgo.sqlite")).expect("console");
        let error = conn
            .execute("INSERT INTO settings(key, value) VALUES ('k', 'v')", [])
            .expect_err("write refused");
        assert!(matches!(error, rusqlite::Error::SqliteFailure(..)));
        assert_eq!(take_denial(), Some("INSERT".to_string()));
        assert_eq!(take_denial(), None);
    }

    #[test]
    fn schema_pragmas_are_allowed_in_any_case() {
        for name in ["table_info", "TABLE_XINFO", "table_list"] {
            let action = AuthAction::Pragma {
                pragma_name: name,
                pragma_value: Some("matches"),
            };
            assert!(is_allowed(&action), "{name}");
        }
        let action = AuthAction::Pragma {
            pragma_name: "journal_mode",
            pragma_value: None,
        };
        assert!(!is_allowed(&action));
    }
}
