//! Dev command: run the tracker's Arena source over one `Player.log` and
//! write the result into a database, then print the Arena runs it made.
//!
//! `cargo run -p tracker --bin arena-seed -- <Player.log> <database path>`
//!
//! The log is copied into a fresh temporary folder and the tracker reads only
//! that copy, so the real Arena log folder is never touched. The database is
//! the only other thing written (created when missing). To look at the result
//! in `tauri dev`, seed a copy of the app's database file and swap it in by
//! hand; this command never picks the app's own database.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mtgo_log::Env;
use tracker::{Engine, TrackerConfig};

/// Ticks to read the whole file, then a few rescans so the tracker settles.
const MAX_TICKS: usize = 10_000;
const RESCANS: usize = 3;
const RESCAN_STEP_MS: i64 = 2_000;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: arena-seed <Player.log> <database path>");
        return ExitCode::FAILURE;
    }
    match run(Path::new(&args[0]), Path::new(&args[1])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("arena-seed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(log: &Path, db_path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(log).map_err(|error| format!("could not read the log: {error}"))?;
    let folder = temp_folder()?;
    let copy = folder.join("Player.log");
    std::fs::write(&copy, &bytes).map_err(|error| format!("could not copy the log: {error}"))?;
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create the database folder: {error}"))?;
    }
    let result = seed(&folder, &copy, db_path);
    let _ = std::fs::remove_dir_all(&folder);
    result?;
    print_runs(db_path)
}

fn temp_folder() -> Result<PathBuf, String> {
    let folder = std::env::temp_dir().join(format!("mymtgo-arena-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not create a temporary folder: {error}"))?;
    Ok(folder)
}

fn seed(folder: &Path, copy: &Path, db_path: &Path) -> Result<(), String> {
    let config = TrackerConfig {
        db_path: db_path.to_path_buf(),
        timezone: chrono_tz::UTC,
        os_timezone: None,
        // Empty folders, so neither MTGO's logs nor the real Arena folder are searched.
        log_overrides: vec![folder.join("no-mtgo-logs")],
        arena_log_overrides: vec![copy.to_path_buf()],
        mtgo_data_folder: Some(folder.join("no-mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    let mut now = now_ms();
    let mut engine = Engine::open(config, Env::current(), Arc::default(), Vec::new(), now)
        .map_err(|error| format!("could not open the tracker: {error:?}"))?;
    for _ in 0..MAX_TICKS {
        let tick = engine
            .tick(now)
            .map_err(|error| format!("tick failed: {error:?}"))?;
        now += 1;
        if !tick.more {
            break;
        }
    }
    for _ in 0..RESCANS {
        now += RESCAN_STEP_MS;
        engine
            .tick(now)
            .map_err(|error| format!("tick failed: {error:?}"))?;
    }
    engine
        .finish()
        .map_err(|error| format!("could not finish: {error:?}"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// One line per Arena run: name, wins-losses of its completed linked matches,
/// and its state.
fn print_runs(db_path: &Path) -> Result<(), String> {
    let conn = store::open_reader(db_path).map_err(|error| format!("could not read: {error}"))?;
    let runs = store::arena_run_sync::arena_run_summaries(&conn)
        .map_err(|error| format!("could not read the runs: {error}"))?;
    println!("{} Arena run(s) in {}", runs.len(), db_path.display());
    for run in runs {
        println!("{} | {}-{} | {}", run.name, run.wins, run.losses, run.state);
    }
    Ok(())
}
