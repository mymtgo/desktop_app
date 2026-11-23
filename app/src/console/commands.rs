//! The console's commands. Every one that reads the database or a file runs
//! off the IPC thread (`super::off_ipc`). Each query opens its own console
//! connection; errors come back as `ConsoleError`'s text for the page.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use super::saved::{self, SavedQuery};
use super::types::{ConsoleObject, ConsoleParam, ConsoleResult, ExportFormat};
use super::window::CONSOLE;
use super::{ConsoleState, off_ipc};
use crate::bridge::lock;
use crate::commands::Startup;
use crate::logging::Timer;

/// The most rows a run shows; the export has no cap.
const MAX_ROWS: usize = 1000;
/// How long a run may take.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an export may take.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);
/// The saved queries folder inside the app data folder.
const SAVED_FOLDER: &str = "console";

/// The database file, or why the app has none (startup failed).
fn db_path(startup: &Startup) -> Result<PathBuf, String> {
    match startup {
        Startup::Ready(state) => Ok(state.db_path.clone()),
        Startup::Failed { error, .. } => Err(error.clone()),
    }
}

/// The saved queries folder, or why the app has none.
fn saved_dir(startup: &Startup) -> Result<PathBuf, String> {
    match startup {
        Startup::Ready(state) => Ok(state.data_dir.join(SAVED_FOLDER)),
        Startup::Failed { error, .. } => Err(error.clone()),
    }
}

fn store_query(
    sql: String,
    params: Vec<ConsoleParam>,
    max_rows: Option<usize>,
    timeout: Duration,
) -> store::console::ConsoleQuery {
    store::console::ConsoleQuery {
        sql,
        params: params.into_iter().map(Into::into).collect(),
        max_rows,
        timeout,
    }
}

/// Runs one statement and returns at most 1000 rows; stops after 10 s or a
/// `console_cancel`.
#[tauri::command]
#[specta::specta]
pub async fn console_run(
    sql: String,
    params: Vec<ConsoleParam>,
    startup: State<'_, Startup>,
    console: State<'_, ConsoleState>,
) -> Result<ConsoleResult, String> {
    let _timer = Timer::start("console_run");
    let path = db_path(&startup)?;
    let cancel = console.start_run();
    let query = store_query(sql, params, Some(MAX_ROWS), RUN_TIMEOUT);
    off_ipc(move || {
        let conn = store::console::open_console(&path).map_err(|error| error.to_string())?;
        store::console::run_query(&conn, &query, cancel.flag())
            .map(ConsoleResult::from)
            .map_err(|error| error.to_string())
    })
    .await?
}

/// Stops the running query and the running export.
#[tauri::command]
#[specta::specta]
pub fn console_cancel(console: State<'_, ConsoleState>) {
    console.cancel();
}

/// The tables and views the console can read, the `v_*` views first.
#[tauri::command]
#[specta::specta]
pub async fn console_schema(startup: State<'_, Startup>) -> Result<Vec<ConsoleObject>, String> {
    let _timer = Timer::start("console_schema");
    let path = db_path(&startup)?;
    off_ipc(move || {
        let conn = store::console::open_console(&path).map_err(|error| error.to_string())?;
        let objects = store::console::schema(&conn).map_err(|error| error.to_string())?;
        Ok(objects.into_iter().map(ConsoleObject::from).collect())
    })
    .await?
}

/// The named parameters of `sql`, without their prefix, in order of first
/// use.
#[tauri::command]
#[specta::specta]
pub async fn console_parameters(
    sql: String,
    startup: State<'_, Startup>,
) -> Result<Vec<String>, String> {
    let path = db_path(&startup)?;
    off_ipc(move || {
        let conn = store::console::open_console(&path).map_err(|error| error.to_string())?;
        store::console::parameters(&conn, &sql).map_err(|error| error.to_string())
    })
    .await?
}

/// Asks where to save, then writes every row of the query there as CSV or
/// JSON and returns how many rows it wrote; `None` when the user closed the
/// save dialog. Stops after 60 s or a `console_cancel`; a failed export
/// leaves an existing file as it was. The count is a `u32` because the
/// bindings refuse a `u64`; an export never nears four billion rows.
#[tauri::command]
#[specta::specta]
pub async fn console_export(
    sql: String,
    params: Vec<ConsoleParam>,
    format: ExportFormat,
    app: AppHandle,
    startup: State<'_, Startup>,
    console: State<'_, ConsoleState>,
) -> Result<Option<u32>, String> {
    let _timer = Timer::start("console_export");
    let db = db_path(&startup)?;
    let saved_file = saved::file(&saved_dir(&startup)?);
    let cancel = console.start_export();
    let query = store_query(sql, params, None, EXPORT_TIMEOUT);
    off_ipc(move || {
        let Some(out) = choose_export_file(&app, format)? else {
            return Ok(None);
        };
        let target = canonical(&out);
        let refusal = export_refusal(
            &target,
            &canonical(&db),
            &canonical(&saved_file),
            CASE_INSENSITIVE_PATHS,
        );
        if let Some(message) = refusal {
            return Err(message.to_string());
        }
        let rows = export_to_file(&db, &query, format, &cancel, &out)?;
        Ok(Some(u32::try_from(rows).unwrap_or(u32::MAX)))
    })
    .await?
}

/// The export's file name and save dialog filter.
fn export_file_name(format: ExportFormat) -> (&'static str, &'static str, &'static str) {
    match format {
        ExportFormat::Csv => ("mymtgo-query.csv", "CSV file", "csv"),
        ExportFormat::Json => ("mymtgo-query.json", "JSON file", "json"),
    }
}

/// The save dialog over the console window; `None` when it was closed.
/// Blocks, so it runs off the IPC thread.
fn choose_export_file(app: &AppHandle, format: ExportFormat) -> Result<Option<PathBuf>, String> {
    let (file_name, filter, extension) = export_file_name(format);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Export query results")
        .set_file_name(file_name)
        .add_filter(filter, &[extension]);
    if let Some(window) = app.get_webview_window(CONSOLE) {
        dialog = dialog.set_parent(&window);
    }
    let Some(picked) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    picked
        .into_path()
        .map(Some)
        .map_err(|error| format!("The file could not be chosen: {error}"))
}

/// Whether the file system compares names without case: Windows and macOS
/// by default.
const CASE_INSENSITIVE_PATHS: bool = cfg!(any(windows, target_os = "macos"));

/// `path` with links and `..` resolved. A file that does not exist yet
/// (a new export) is its resolved folder joined with its name; a path whose
/// folder is missing too is kept as it is.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(found) = path.canonicalize() {
        return found;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path.to_path_buf();
    };
    match parent.canonicalize() {
        Ok(folder) => folder.join(name),
        Err(_) => path.to_path_buf(),
    }
}

/// Why an export may not be written to `target`, or `None` when it may.
/// Every path is already canonical (`canonical`). The database, its `-wal`,
/// `-shm` and `-journal` files and the saved queries file are refused;
/// `case_insensitive` compares the paths without case.
fn export_refusal(
    target: &Path,
    db: &Path,
    saved_file: &Path,
    case_insensitive: bool,
) -> Option<&'static str> {
    let key = |path: &Path| {
        let text = path.to_string_lossy().into_owned();
        if case_insensitive {
            text.to_lowercase()
        } else {
            text
        }
    };
    let target = key(target);
    let db = key(db);
    let database_files = [
        db.clone(),
        format!("{db}-wal"),
        format!("{db}-shm"),
        format!("{db}-journal"),
    ];
    if database_files.contains(&target) {
        return Some("Choose another file: that one is MyMTGO's database.");
    }
    if target == key(saved_file) {
        return Some("Choose another file: that one holds your saved queries.");
    }
    None
}

/// How many part file names (`<out>.part`, `<out>.1.part`, ...) the export
/// tries before giving up.
const PART_NAMES: u32 = 100;

/// Makes a new part file beside `out`, where the export is written until it
/// is complete: `<out>.part`, or `<out>.<n>.part` when that name is taken.
/// It is only ever made new (`create_new`), so a file of the user's own with
/// that name is never truncated or removed.
fn create_part(out: &Path) -> Result<(PathBuf, File), String> {
    for attempt in 0..PART_NAMES {
        let mut name = out.as_os_str().to_owned();
        if attempt > 0 {
            name.push(format!(".{attempt}"));
        }
        name.push(".part");
        let part = PathBuf::from(name);
        match File::options().write(true).create_new(true).open(&part) {
            Ok(file) => return Ok((part, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("The file could not be made: {error}")),
        }
    }
    Err("The file could not be made: too many part files beside it".to_string())
}

/// The export into a new part file (`create_part`), renamed to `out` once
/// complete, so a failed or cancelled export never touches an existing file
/// at `out`. The part file is removed when anything fails.
fn export_to_file(
    db: &Path,
    query: &store::console::ConsoleQuery,
    format: ExportFormat,
    cancel: &Arc<AtomicBool>,
    out: &Path,
) -> Result<u64, String> {
    let conn = store::console::open_console(db).map_err(|error| error.to_string())?;
    let (part, file) = create_part(out)?;
    let mut writer = BufWriter::new(file);
    let written = store::console::export(&conn, query, format.into(), cancel, &mut writer)
        .map_err(|error| error.to_string())
        .and_then(|rows| {
            writer
                .flush()
                .map(|()| rows)
                .map_err(|error| format!("The file could not be written: {error}"))
        });
    // Closed before the part file is renamed or removed (Windows refuses
    // either on an open file).
    drop(writer);
    let finished = written.and_then(|rows| {
        std::fs::rename(&part, out)
            .map(|()| rows)
            .map_err(|error| format!("The file could not be saved: {error}"))
    });
    if finished.is_err() {
        std::fs::remove_file(&part).ok();
    }
    finished
}

/// The saved queries, in the order they were first saved.
#[tauri::command]
#[specta::specta]
pub async fn console_saved_list(
    startup: State<'_, Startup>,
    console: State<'_, ConsoleState>,
) -> Result<Vec<SavedQuery>, String> {
    let Ok(dir) = saved_dir(&startup) else {
        return Ok(Vec::new());
    };
    let one_at_a_time = Arc::clone(&console.saved);
    off_ipc(move || {
        let _held = lock(&one_at_a_time);
        saved::load(&dir)
    })
    .await
}

/// Saves `query`, replacing the saved query of the same name, and returns
/// the list. The name is trimmed first (`saved_name`); `updatedAt` is set
/// to now; a blank name is refused.
#[tauri::command]
#[specta::specta]
pub async fn console_saved_put(
    query: SavedQuery,
    startup: State<'_, Startup>,
    console: State<'_, ConsoleState>,
) -> Result<Vec<SavedQuery>, String> {
    let name = saved_name(&query.name)?;
    let dir = saved_dir(&startup)?;
    let one_at_a_time = Arc::clone(&console.saved);
    let query = SavedQuery {
        name,
        updated_at: crate::wall_clock_ms(),
        ..query
    };
    off_ipc(move || change_saved(&one_at_a_time, &dir, |list| saved::put(list, query))).await?
}

/// Deletes the saved query called `name` and returns the list.
#[tauri::command]
#[specta::specta]
pub async fn console_saved_delete(
    name: String,
    startup: State<'_, Startup>,
    console: State<'_, ConsoleState>,
) -> Result<Vec<SavedQuery>, String> {
    let dir = saved_dir(&startup)?;
    let one_at_a_time = Arc::clone(&console.saved);
    off_ipc(move || change_saved(&one_at_a_time, &dir, |list| saved::delete(list, &name))).await?
}

/// The name a query is saved under: `name` without spaces around it, so
/// " Win rate" replaces "Win rate". A name that is only spaces is refused.
fn saved_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the query a name.".to_string());
    }
    Ok(name.to_string())
}

/// Reads the saved queries, changes them and writes them back, one change
/// at a time. A file that cannot be read is an error and is never written
/// over.
fn change_saved(
    one_at_a_time: &Mutex<()>,
    dir: &Path,
    change: impl FnOnce(Vec<SavedQuery>) -> Vec<SavedQuery>,
) -> Result<Vec<SavedQuery>, String> {
    let _held = lock(one_at_a_time);
    let list = saved::load_strict(dir)
        .map_err(|error| format!("The saved queries could not be read: {error}"))?;
    let list = change(list);
    saved::store(dir, &list)
        .map_err(|error| format!("The saved queries could not be written: {error}"))?;
    Ok(list)
}

/// Opens the folder holding the database in the file manager, through the
/// opener plugin's Rust side as `open_log_folder` does.
#[tauri::command]
#[specta::specta]
pub fn open_database_folder(app: AppHandle, startup: State<'_, Startup>) -> Result<(), String> {
    let _timer = Timer::start("open_database_folder");
    let path = db_path(&startup)?;
    let Some(folder) = path.parent() else {
        return Err("The database folder was not found.".to_string());
    };
    app.opener()
        .open_path(folder.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| {
            tracing::warn!(%error, "the database folder could not be opened");
            "The folder could not be opened.".to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::testing::Scratch;

    /// A migrated, empty database in `scratch`.
    fn database(scratch: &Scratch) -> PathBuf {
        let path = scratch.0.join("mymtgo.sqlite");
        drop(store::open_writer(&path).expect("make the database"));
        path
    }

    #[test]
    fn an_export_writes_the_file_and_counts_its_rows() {
        let scratch = Scratch::new();
        let db = database(&scratch);
        let out = scratch.0.join("out.csv");
        let query = store_query(
            "SELECT 1 AS a UNION ALL SELECT 2".to_string(),
            Vec::new(),
            None,
            EXPORT_TIMEOUT,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let rows = export_to_file(&db, &query, ExportFormat::Csv, &cancel, &out);
        assert_eq!(rows, Ok(2));
        assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), 3);
    }

    #[test]
    fn a_failed_export_leaves_no_file() {
        let scratch = Scratch::new();
        let db = database(&scratch);
        let out = scratch.0.join("out.json");
        let query = store_query(
            "SELECT * FROM no_such_table".to_string(),
            Vec::new(),
            None,
            EXPORT_TIMEOUT,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let result = export_to_file(&db, &query, ExportFormat::Json, &cancel, &out);
        assert!(result.is_err());
        assert!(!out.exists(), "the partial file is removed");
    }

    #[test]
    fn a_failed_export_keeps_an_existing_file() {
        let scratch = Scratch::new();
        let db = database(&scratch);
        let out = scratch.0.join("out.csv");
        std::fs::write(&out, "older export").unwrap();
        let query = store_query(
            "SELECT * FROM no_such_table".to_string(),
            Vec::new(),
            None,
            EXPORT_TIMEOUT,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(export_to_file(&db, &query, ExportFormat::Csv, &cancel, &out).is_err());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "older export");
        assert!(
            !scratch.0.join("out.csv.part").exists(),
            "no part file left"
        );
    }

    #[test]
    fn a_users_own_part_file_is_never_touched() {
        let scratch = Scratch::new();
        let db = database(&scratch);
        let out = scratch.0.join("out.csv");
        let theirs = scratch.0.join("out.csv.part");
        std::fs::write(&theirs, "their file").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let failing = store_query(
            "SELECT * FROM no_such_table".to_string(),
            Vec::new(),
            None,
            EXPORT_TIMEOUT,
        );
        assert!(export_to_file(&db, &failing, ExportFormat::Csv, &cancel, &out).is_err());
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "their file");
        assert!(
            !scratch.0.join("out.csv.1.part").exists(),
            "ours is removed"
        );

        let working = store_query(
            "SELECT 1 AS a".to_string(),
            Vec::new(),
            None,
            EXPORT_TIMEOUT,
        );
        assert_eq!(
            export_to_file(&db, &working, ExportFormat::Csv, &cancel, &out),
            Ok(1)
        );
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "their file");
        assert!(out.exists());
    }

    const DB_REFUSED: Option<&str> = Some("Choose another file: that one is MyMTGO's database.");
    const SAVED_REFUSED: Option<&str> =
        Some("Choose another file: that one holds your saved queries.");

    #[test]
    fn an_export_over_the_database_or_saved_queries_is_refused() {
        let db = Path::new("/data/mymtgo.sqlite");
        let saved_file = Path::new("/data/console/queries.json");
        let refusal = |target: &str| export_refusal(Path::new(target), db, saved_file, false);
        assert_eq!(refusal("/data/mymtgo.sqlite"), DB_REFUSED);
        assert_eq!(refusal("/data/mymtgo.sqlite-wal"), DB_REFUSED);
        assert_eq!(refusal("/data/mymtgo.sqlite-shm"), DB_REFUSED);
        assert_eq!(refusal("/data/mymtgo.sqlite-journal"), DB_REFUSED);
        assert_eq!(refusal("/data/console/queries.json"), SAVED_REFUSED);
        assert_eq!(refusal("/data/mymtgo.csv"), None);
        assert_eq!(refusal("/data/console/queries.csv"), None);
        assert_eq!(refusal("/data/MYMTGO.sqlite"), None, "case counts here");
    }

    #[test]
    fn a_case_insensitive_file_system_refuses_any_case() {
        let db = Path::new("/Data/mymtgo.sqlite");
        let saved_file = Path::new("/Data/console/queries.json");
        let refusal = |target: &str| export_refusal(Path::new(target), db, saved_file, true);
        assert_eq!(refusal("/data/MYMTGO.SQLITE"), DB_REFUSED);
        assert_eq!(refusal("/DATA/mymtgo.sqlite-WAL"), DB_REFUSED);
        assert_eq!(refusal("/data/Console/Queries.JSON"), SAVED_REFUSED);
        assert_eq!(refusal("/data/out.json"), None);
    }

    #[test]
    fn canonical_resolves_a_new_file_through_its_folder() {
        let scratch = Scratch::new();
        let db = database(&scratch);
        // A path that reaches the database through `..`, and a file that
        // does not exist yet beside it.
        let roundabout = scratch.0.join("console").join("..").join("mymtgo.sqlite");
        std::fs::create_dir_all(scratch.0.join("console")).unwrap();
        assert_eq!(canonical(&roundabout), canonical(&db));
        let wal = scratch
            .0
            .join("console")
            .join("..")
            .join("mymtgo.sqlite-wal");
        std::fs::remove_file(scratch.0.join("mymtgo.sqlite-wal")).ok();
        let saved_file = saved::file(&scratch.0.join("console"));
        assert_eq!(
            export_refusal(
                &canonical(&wal),
                &canonical(&db),
                &canonical(&saved_file),
                false
            ),
            DB_REFUSED
        );
        assert_eq!(
            export_refusal(
                &canonical(&scratch.0.join("console").join("queries.json")),
                &canonical(&db),
                &canonical(&saved_file),
                false
            ),
            SAVED_REFUSED
        );
        let elsewhere = scratch.0.join("export.csv");
        assert_eq!(
            export_refusal(
                &canonical(&elsewhere),
                &canonical(&db),
                &canonical(&saved_file),
                false
            ),
            None
        );
    }

    #[test]
    fn an_unreadable_saved_file_is_never_written_over() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("console");
        std::fs::create_dir_all(dir.join("queries.json")).unwrap();
        let lock = Mutex::new(());
        let query = SavedQuery {
            name: "A".to_string(),
            sql: "SELECT 1".to_string(),
            params: Vec::new(),
            updated_at: 5,
        };
        let result = change_saved(&lock, &dir, |list| saved::put(list, query));
        assert!(result.is_err());
        assert!(dir.join("queries.json").is_dir(), "left untouched");
    }

    #[test]
    fn a_saved_name_is_trimmed_and_a_blank_one_refused() {
        assert!(saved_name("").is_err());
        assert!(saved_name("  \t").is_err());
        assert_eq!(saved_name("Win rate"), Ok("Win rate".to_string()));
        assert_eq!(saved_name("  Win rate \t"), Ok("Win rate".to_string()));
    }

    #[test]
    fn saved_changes_go_through_the_file() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("console");
        let lock = Mutex::new(());
        let query = SavedQuery {
            name: "A".to_string(),
            sql: "SELECT 1".to_string(),
            params: Vec::new(),
            updated_at: 5,
        };
        let list = change_saved(&lock, &dir, |list| saved::put(list, query.clone()));
        assert_eq!(list, Ok(vec![query.clone()]));
        assert_eq!(saved::load(&dir), vec![query]);
        let list = change_saved(&lock, &dir, |list| saved::delete(list, "A"));
        assert_eq!(list, Ok(Vec::new()));
        assert_eq!(saved::load(&dir), Vec::new());
    }
}
