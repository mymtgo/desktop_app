//! Dev command: write every Arena match with stored game messages as a
//! `.mtgr` file, read from a database opened read-only.
//!
//! `cargo run -p store --bin arena-mtgr -- <db path> <out dir>`
//!
//! Prints one line per match: its token, then its games, bytes, build
//! milliseconds, the lines naming a card `?` (decision 0048) and the tokens
//! queued for art; or the grpIds whose card data lacks a face name.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use store::arena_replay::{ArenaMatchRead, arena_match_text, arena_tokens_with_messages};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: arena-mtgr <db path> <out dir>");
        return ExitCode::FAILURE;
    }
    match run(&args[0], &PathBuf::from(&args[1])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("arena-mtgr: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(db_path: &str, out_dir: &PathBuf) -> Result<(), String> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("open {db_path}: {error}"))?;
    std::fs::create_dir_all(out_dir).map_err(|error| format!("create out dir: {error}"))?;
    let tokens = arena_tokens_with_messages(&conn).map_err(|error| error.to_string())?;
    for token in tokens {
        let started = Instant::now();
        let read = arena_match_text(&conn, &token).map_err(|error| error.to_string())?;
        let build_ms = started.elapsed().as_millis();
        match read {
            ArenaMatchRead::Ready(written) => {
                let path = out_dir.join(format!("{token}.mtgr"));
                std::fs::write(&path, &written.text)
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                println!(
                    "{token} games {} bytes {} build {build_ms} ms unknown {} tokens queued {}",
                    written.games.len(),
                    written.text.len(),
                    unknown_lines(&written.text),
                    written.tokens_to_fetch.len()
                );
            }
            ArenaMatchRead::MissingCardData(grp_ids) => {
                let ids: Vec<String> = grp_ids.iter().map(u32::to_string).collect();
                println!("{token} missing face names: {}", ids.join(", "));
            }
            ArenaMatchRead::NotFound => println!("{token} not found"),
        }
    }
    Ok(())
}

/// The lines naming a card `?`: `new card`, `identify` and deck entries.
fn unknown_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let named_unknown = line.contains(" new card #") && line.contains(" ? owner ");
            named_unknown
                || line.contains(" identify ? ")
                || line.contains(" main ") && line.contains(" ? src ")
                || line.contains(" side ") && line.contains(" ? src ")
        })
        .count()
}
