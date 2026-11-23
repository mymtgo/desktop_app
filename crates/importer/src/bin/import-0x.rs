//! Dev command: import a 0.x database into a v1 database from the terminal.
//!
//! `cargo run --release -p importer --bin import-0x -- <0.x sqlite> <v1 sqlite>`
//!
//! Prints counts only, never player names. Settings are not written. Decks,
//! versions, cards and match links are written after the matches, leagues
//! after the decks, and Limited drafts, picks, notes and registered decks
//! after the leagues.
//!
//! `import-0x --compare-card-stats <0.x sqlite> <v1 sqlite>` writes nothing:
//! it opens both databases read only and prints how v1's card rows of each
//! imported game compare with 0.x's `card_game_stats` (counts only).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use importer::read_database;
use rusqlite::{Connection, OpenFlags};
use store::import::ImportCounts;
use store::writer::spawn_writer;

/// Matches per writer batch (importer spec section 7).
const BATCH: usize = 200;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let compare = args.first().map(String::as_str) == Some("--compare-card-stats");
    let paths = if compare { &args[1..] } else { &args[..] };
    if paths.len() != 2 {
        eprintln!("usage: import-0x [--compare-card-stats] <0.x sqlite> <v1 sqlite>");
        return ExitCode::from(2);
    }
    let (legacy, v1) = (PathBuf::from(&paths[0]), PathBuf::from(&paths[1]));
    let outcome = if compare {
        compare_card_stats(&legacy, &v1)
    } else {
        run(legacy, v1)
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("import failed: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Open both databases read only and print the card stats comparison.
fn compare_card_stats(legacy: &Path, v1: &Path) -> Result<(), String> {
    let open = |path: &Path| {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    let legacy = open(legacy)?;
    let v1 = open(v1)?;
    let report = importer::compare_card_stats::compare_card_stats(&v1, &legacy)
        .map_err(|error| error.to_string())?;
    for line in report.lines() {
        println!("{line}");
    }
    Ok(())
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

fn run(legacy: PathBuf, v1: PathBuf) -> Result<(), String> {
    let started = Instant::now();
    let read = read_database(&legacy).map_err(|error| error.to_string())?;
    let read_ms = started.elapsed().as_millis();
    println!("read {} matches in {read_ms} ms", read.matches.len());

    let writer = spawn_writer(&v1).map_err(|error| error.to_string())?;
    let now = now_ms();
    // Before the matches, as the app does.
    let accounts = writer
        .import_accounts(read.accounts.clone(), now)
        .map_err(|error| error.to_string())?;
    let mut total = ImportCounts::default();
    for batch in read.matches.chunks(BATCH) {
        let counts = writer
            .import(batch.to_vec(), now)
            .map_err(|error| error.to_string())?;
        total.added += counts.added;
        total.updated += counts.updated;
        total.unchanged += counts.unchanged;
        total.skipped_live += counts.skipped_live;
    }
    let matches_ms = started.elapsed().as_millis();

    // After the matches, so the links find their rows.
    let decks = match read.decks {
        Some(deck_read) => {
            let counts = writer
                .import_decks(deck_read.data, now)
                .map_err(|error| error.to_string())?;
            Some((
                counts,
                deck_read.skipped_limited,
                deck_read.skipped_versions,
                deck_read.skipped_duplicates,
            ))
        }
        None => None,
    };
    let decks_ms = started.elapsed().as_millis();

    // After the decks, so members and deck versions are found.
    let leagues = match read.leagues {
        Some(league_read) => {
            let counts = writer
                .import_leagues(league_read.leagues, now)
                .map_err(|error| error.to_string())?;
            Some((
                counts,
                league_read.skipped_deleted,
                league_read.skipped_unreadable,
                league_read.skipped_not_account,
            ))
        }
        None => None,
    };

    // After the leagues, so drafts and registered decks find their runs.
    let limited = match read.limited {
        Some(limited_read) => {
            let counts = writer
                .import_limited(limited_read.data, now)
                .map_err(|error| error.to_string())?;
            Some((
                counts,
                limited_read.drafts_skipped + limited_read.drafts_not_account,
                limited_read.decks_skipped,
            ))
        }
        None => None,
    };
    let total_ms = started.elapsed().as_millis();

    println!("added:      {}", total.added);
    println!("updated:    {}", total.updated);
    println!("unchanged:  {}", total.unchanged);
    println!("skipped:");
    println!("  live row exists:  {}", total.skipped_live);
    println!("  in progress:      {}", read.skipped.in_progress);
    println!("  no local player:  {}", read.skipped.no_local_player);
    println!("  not an account:   {}", read.skipped.not_account);
    println!("  duplicates:       {}", read.skipped.duplicate_token);
    println!("  bad start time:   {}", read.skipped.bad_time);
    match decks {
        Some((counts, skipped_limited, skipped_versions, skipped_duplicates)) => {
            println!(
                "cards:      {} added, {} updated, {} unchanged",
                counts.cards_added, counts.cards_updated, counts.cards_unchanged
            );
            println!(
                "decks:      {} added, {} updated, {} unchanged, {} Limited skipped, {} duplicate MTGO ids skipped",
                counts.decks_added,
                counts.decks_updated,
                counts.decks_unchanged,
                skipped_limited,
                skipped_duplicates
            );
            println!(
                "versions:   {} added, {} unchanged, {} unreadable skipped",
                counts.versions_added, counts.versions_unchanged, skipped_versions
            );
            println!(
                "links:      {} added, {} unchanged",
                counts.links_added, counts.links_unchanged
            );
        }
        None => println!("decks:      none (no deck tables in this 0.x database)"),
    }
    match leagues {
        Some((counts, skipped_deleted, skipped_unreadable, skipped_not_account)) => {
            println!(
                "leagues:    {} added, {} updated, {} unchanged, {} without an account",
                counts.added, counts.updated, counts.unchanged, counts.without_account
            );
            println!(
                "  skipped:  {skipped_deleted} deleted, {skipped_unreadable} unreadable, \
                 {skipped_not_account} not an account's"
            );
        }
        None => println!("leagues:    none (no leagues table in this 0.x database)"),
    }
    match limited {
        Some((counts, drafts_skipped, decks_skipped)) => {
            println!(
                "drafts:     {} added, {} updated, {} unchanged, {} from the log, {} skipped, \
                 {} notes added, {} notes without their pick yet",
                counts.drafts_added,
                counts.drafts_updated,
                counts.drafts_unchanged,
                counts.drafts_from_log,
                drafts_skipped,
                counts.notes_added,
                counts.notes_without_pick
            );
            println!(
                "registered: {} added, {} updated, {} unchanged, {} without a run, \
                 {} without a match, {} skipped",
                counts.decks_added,
                counts.decks_updated,
                counts.decks_unchanged,
                counts.decks_without_run,
                counts.decks_without_match,
                decks_skipped
            );
        }
        None => println!("drafts:     none (no Limited tables in this 0.x database)"),
    }
    println!(
        "accounts:   {} added, {} unchanged, {} login ids filled, {} login id clashes, \
         {} untracked, active account seeded: {}",
        accounts.added,
        accounts.unchanged,
        accounts.login_ids_filled,
        accounts.login_id_clashes,
        accounts.untracked,
        accounts.active_seeded
    );
    println!(
        "elapsed: {total_ms} ms (read {read_ms} ms, matches written by {matches_ms} ms, \
         decks by {decks_ms} ms)"
    );
    Ok(())
}
