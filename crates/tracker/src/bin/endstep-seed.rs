//! Dev command: store a folder of Endstep capture units (`.jsonl` files) in a
//! database, run the tracker's Endstep sync over them, then print the
//! Endstep matches it made.
//!
//! `cargo run -p tracker --bin endstep-seed -- <folder of .jsonl units> <database path>`
//!
//! The files are only read. The database is the only thing written
//! (created when missing). To look at the result in `tauri dev`, seed a copy
//! of the app's database file and swap it in by hand; this command never
//! picks the app's own database.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mtgo_log::Env;
use sha2::{Digest, Sha256};
use store::endstep_captures::NewCapture;
use tracker::{Engine, TrackerConfig};

/// Ticks to fold every unit, then a few more so the deck links commit.
const MAX_TICKS: usize = 10_000;
const SETTLE_TICKS: usize = 3;
const SETTLE_STEP_MS: i64 = 2_000;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: endstep-seed <folder of .jsonl units> <database path>");
        return ExitCode::FAILURE;
    }
    match run(Path::new(&args[0]), Path::new(&args[1])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("endstep-seed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(folder: &Path, db_path: &Path) -> Result<(), String> {
    let captures = read_captures(folder)?;
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create the database folder: {error}"))?;
    }
    let empty = temp_folder()?;
    let result = seed(&empty, captures, db_path);
    let _ = std::fs::remove_dir_all(&empty);
    result?;
    print_matches(db_path)
}

/// Every `.jsonl` file in `folder`, by name, as a capture to store.
fn read_captures(folder: &Path) -> Result<Vec<NewCapture>, String> {
    let entries =
        std::fs::read_dir(folder).map_err(|error| format!("could not read the folder: {error}"))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("could not read the folder: {error}"))?
            .path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut captures = Vec::new();
    for path in paths {
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let file_hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let text =
            String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
        let parsed = endstep_capture::parse_capture(&text)
            .map_err(|error| format!("{} is not a capture: {error}", path.display()))?;
        captures.push(NewCapture {
            header: parsed.header,
            frame_count: parsed.frame_count,
            text,
            file_hash,
        });
    }
    Ok(captures)
}

/// An empty folder for the MTGO and Arena logs, so no real log is read.
fn temp_folder() -> Result<PathBuf, String> {
    let folder = std::env::temp_dir().join(format!("mymtgo-endstep-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not create a temporary folder: {error}"))?;
    Ok(folder)
}

fn seed(empty: &Path, captures: Vec<NewCapture>, db_path: &Path) -> Result<(), String> {
    let config = TrackerConfig {
        db_path: db_path.to_path_buf(),
        timezone: chrono_tz::UTC,
        os_timezone: None,
        log_overrides: vec![empty.join("no-mtgo-logs")],
        arena_log_overrides: vec![empty.join("no-arena/Player.log")],
        mtgo_data_folder: Some(empty.join("no-mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: false,
        endstep_enabled: true,
    };
    let mut now = now_ms();
    let mut engine = Engine::open(config, Env::current(), Arc::default(), Vec::new(), now)
        .map_err(|error| format!("could not open the tracker: {error:?}"))?;
    let report = engine
        .store_endstep_captures(captures, now)
        .map_err(|error| format!("could not store the captures: {error:?}"))?;
    println!(
        "captures: {} added, {} replaced, {} unchanged",
        report.added, report.replaced, report.unchanged
    );
    for _ in 0..MAX_TICKS {
        let tick = engine
            .tick(now)
            .map_err(|error| format!("tick failed: {error:?}"))?;
        now += 1;
        if !tick.more {
            break;
        }
    }
    for _ in 0..SETTLE_TICKS {
        now += SETTLE_STEP_MS;
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

/// One line per Endstep match: token, format, result and score, games.
fn print_matches(db_path: &Path) -> Result<(), String> {
    let conn = store::open_reader(db_path).map_err(|error| format!("could not read: {error}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT m.token, COALESCE(m.format, '?'), m.outcome,
                 (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'win'),
                 (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'loss'),
                 (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id)
             FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE a.platform = 'endstep' ORDER BY m.started_at, m.id",
        )
        .map_err(|error| format!("could not read the matches: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(format!(
                "{} | {} {} {}-{}, {} games",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?
            ))
        })
        .map_err(|error| format!("could not read the matches: {error}"))?;
    let mut lines = Vec::new();
    for row in rows {
        lines.push(row.map_err(|error| format!("could not read a match: {error}"))?);
    }
    println!("{} Endstep match(es) in {}", lines.len(), db_path.display());
    for line in lines {
        println!("{line}");
    }
    Ok(())
}
