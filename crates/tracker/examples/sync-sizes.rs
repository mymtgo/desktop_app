//! Dev command: how big the cross-device sync bundles of a database's Arena
//! and Endstep rows are, and (with `--pull`) how long a second device takes
//! to import and read them.
//!
//! `cargo run --release -p tracker --example sync-sizes -- <database path> [--pull <scratch database path>]`
//!
//! For every Arena and Endstep match it builds the match bundle as the sync
//! runner does (no carry), and for every Arena and Endstep deck the deck
//! bundle, then prints each one's canonical JSON size and gzip size
//! (`gzip_blob`, the size `COMPRESSED_PER_BLOB` limits), largest first, and
//! per kind the count, largest and median.
//!
//! With `--pull`, a new database is made at the scratch path (it must not
//! exist yet) and every match bundle is imported into it as a pulled match
//! (`SyncEdit::PulledMatch`, the pull's own transaction), timed. Then, on
//! that database, the match page's reads of each pulled Arena match
//! (`game_numbers_with_messages`, `arena_match_text`) and one Endstep resync
//! batch (which derives every pulled Endstep match again) are timed.
//!
//! The source database is only read.

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mymtgo_api::{COMPRESSED_PER_BLOB, Tier, gzip_blob};
use store::sync::SyncEdit;
use sync_bundles::match_bundle::match_bundle_owned;
use sync_bundles::{BundlePlatform, bundle_hash, canonical_json, deck_bundle};

/// The size the brief asks to flag: a match bundle above this calls for a
/// new limit.
const FLAG_BYTES: usize = 768 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (db_path, pull_path) = match args.as_slice() {
        [db] => (db.clone(), None),
        [db, flag, pull] if flag == "--pull" => (db.clone(), Some(pull.clone())),
        _ => {
            eprintln!("usage: sync-sizes <database path> [--pull <scratch database path>]");
            return ExitCode::FAILURE;
        }
    };
    match run(Path::new(&db_path), pull_path.as_deref().map(Path::new)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sync-sizes: {message}");
            ExitCode::FAILURE
        }
    }
}

/// One built bundle: what it is, its client id and its canonical JSON.
struct Built {
    kind: String,
    client_id: String,
    canonical: String,
    gzip_bytes: usize,
}

fn run(db_path: &Path, pull_path: Option<&Path>) -> Result<(), String> {
    let conn = store::open_reader(db_path).map_err(|error| format!("could not read: {error}"))?;
    let mut built = match_bundles(&conn)?;
    built.extend(deck_bundles(&conn)?);
    print_sizes(&built);
    if let Some(pull_path) = pull_path {
        let matches: Vec<&Built> = built
            .iter()
            .filter(|item| item.kind.ends_with("match"))
            .collect();
        pull_and_time(pull_path, &matches)?;
    }
    Ok(())
}

/// Every Arena and Endstep match's bundle, as the sync runner builds it.
fn match_bundles(conn: &store::Connection) -> Result<Vec<Built>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id, a.platform FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE a.platform IN ('arena', 'endstep') ORDER BY m.id",
        )
        .map_err(|error| format!("could not read the matches: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("could not read the matches: {error}"))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|error| format!("could not read a match: {error}"))?);
    }
    let mut built = Vec::new();
    for (match_id, platform) in ids {
        let input = store::sync::match_bundle_input(conn, match_id, chrono_tz::UTC)
            .map_err(|error| format!("match {match_id}: {error}"))?;
        let Some(input) = input else {
            continue;
        };
        let client_id = input.token.clone();
        let canonical = canonical_json(&match_bundle_owned(input, None));
        built.push(built_row(format!("{platform} match"), client_id, canonical));
    }
    Ok(built)
}

/// Every Arena and Endstep deck's bundle (supporter tier, so limited decks
/// too).
fn deck_bundles(conn: &store::Connection) -> Result<Vec<Built>, String> {
    let candidates = store::sync::deck_candidates(conn, Tier::Supporter)
        .map_err(|error| format!("could not read the decks: {error}"))?;
    let mut built = Vec::new();
    for candidate in candidates {
        let platform = match candidate.platform {
            BundlePlatform::Arena => "arena",
            BundlePlatform::Endstep => "endstep",
            BundlePlatform::Mtgo => continue,
        };
        let input = store::sync::deck_bundle_input(conn, &candidate)
            .map_err(|error| format!("deck {}: {error}", candidate.deck_id))?;
        let canonical = canonical_json(&deck_bundle(&input, None));
        built.push(built_row(
            format!("{platform} deck"),
            candidate.client_id.clone(),
            canonical,
        ));
    }
    Ok(built)
}

fn built_row(kind: String, client_id: String, canonical: String) -> Built {
    let gzip_bytes = gzip_blob(&canonical).len();
    Built {
        kind,
        client_id,
        canonical,
        gzip_bytes,
    }
}

/// Every bundle largest (gzip) first, then a summary per kind.
fn print_sizes(built: &[Built]) {
    let mut order: Vec<&Built> = built.iter().collect();
    order.sort_by_key(|item| std::cmp::Reverse(item.gzip_bytes));
    println!("kind | client id | canonical bytes | gzip bytes");
    for item in &order {
        println!(
            "{} | {} | {} | {}",
            item.kind,
            item.client_id,
            item.canonical.len(),
            item.gzip_bytes
        );
    }
    println!();
    println!("kind | count | largest canonical | median canonical | largest gzip | median gzip");
    for kind in ["arena match", "endstep match", "arena deck", "endstep deck"] {
        let raw: Vec<usize> = built
            .iter()
            .filter(|item| item.kind == kind)
            .map(|item| item.canonical.len())
            .collect();
        let gz: Vec<usize> = built
            .iter()
            .filter(|item| item.kind == kind)
            .map(|item| item.gzip_bytes)
            .collect();
        let raw = Summary::of(&raw);
        let gz = Summary::of(&gz);
        println!(
            "{kind} | {} | {} | {} | {} | {}",
            raw.count,
            kib(raw.largest),
            kib(raw.median),
            kib(gz.largest),
            kib(gz.median)
        );
    }
    let largest_match = built
        .iter()
        .filter(|item| item.kind.ends_with("match"))
        .map(|item| item.gzip_bytes)
        .max()
        .unwrap_or(0);
    println!();
    println!(
        "largest match gzip {} (flag at {}, limit COMPRESSED_PER_BLOB {}): {}",
        kib(largest_match),
        kib(FLAG_BYTES),
        kib(COMPRESSED_PER_BLOB as usize),
        if largest_match > FLAG_BYTES {
            "above the flag"
        } else {
            "under the flag"
        }
    );
}

/// Count, largest and median (the lower middle value for an even count) of
/// some sizes; all zero when there are none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    count: usize,
    largest: usize,
    median: usize,
}

impl Summary {
    fn of(values: &[usize]) -> Summary {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let median = if sorted.is_empty() {
            0
        } else {
            sorted[(sorted.len() - 1) / 2]
        };
        Summary {
            count: sorted.len(),
            largest: sorted.last().copied().unwrap_or(0),
            median,
        }
    }
}

fn kib(bytes: usize) -> String {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
}

fn ms(elapsed: Duration) -> String {
    format!("{:.1} ms", elapsed.as_secs_f64() * 1000.0)
}

/// Import every match bundle into a new database at `pull_path`, timed,
/// then time the pulled matches' reads and one Endstep resync there.
fn pull_and_time(pull_path: &Path, matches: &[&Built]) -> Result<(), String> {
    if pull_path.exists() {
        return Err(format!(
            "{} exists; give a new path for the scratch database",
            pull_path.display()
        ));
    }
    let mut conn =
        store::open_writer(pull_path).map_err(|error| format!("could not create: {error}"))?;
    println!();
    println!("pull | kind | client id | gzip bytes | import time");
    let mut pulled_arena: Vec<String> = Vec::new();
    let mut pulled_endstep = 0usize;
    let pull_started = Instant::now();
    for item in matches {
        let edit = pulled_edit(item)?;
        let started = Instant::now();
        store::sync::edit_sync(&mut conn, &edit)
            .map_err(|error| format!("could not import {}: {error}", item.client_id))?;
        let elapsed = started.elapsed();
        println!(
            "pull | {} | {} | {} | {}",
            item.kind,
            item.client_id,
            item.gzip_bytes,
            ms(elapsed)
        );
        if item.kind == "arena match" {
            pulled_arena.push(item.client_id.clone());
        } else {
            pulled_endstep += 1;
        }
    }
    // The runner's last pull step: the Arena runs folded once.
    let started = Instant::now();
    store::sync::edit_sync(
        &mut conn,
        &SyncEdit::ArenaRunsAfterPull { now_ms: now_ms() },
    )
    .map_err(|error| format!("the Arena run fold failed: {error}"))?;
    let fold = started.elapsed();
    println!();
    println!("arena runs fold after the pull: {}", ms(fold));
    println!(
        "whole pull of {} match(es), fold included: {}",
        matches.len(),
        ms(pull_started.elapsed())
    );
    time_arena_reads(&conn, &pulled_arena)?;
    time_endstep_resync(&mut conn, pulled_endstep)
}

/// A built match bundle as the runner's pull edit (`pulled_match`).
fn pulled_edit(item: &Built) -> Result<SyncEdit, String> {
    let bundle: serde_json::Value = serde_json::from_str(&item.canonical)
        .map_err(|error| format!("{} does not read: {error}", item.client_id))?;
    let (pulled, carry) = importer::bundle_read::read_match_bundle(&bundle)
        .map_err(|error| format!("{} does not read: {error:?}", item.client_id))?;
    Ok(SyncEdit::PulledMatch {
        client_id: item.client_id.clone(),
        pulled: Box::new(pulled),
        hash: bundle_hash(&item.canonical),
        carry: carry.to_json().to_string(),
        timezone: chrono_tz::UTC,
        now_ms: now_ms(),
    })
}

/// The match page's reads of each pulled Arena match, timed.
fn time_arena_reads(conn: &store::Connection, tokens: &[String]) -> Result<(), String> {
    println!();
    println!("read | client id | game numbers | game numbers time | match text | match text time");
    for token in tokens {
        let started = Instant::now();
        let numbers = store::arena_games::game_numbers_with_messages(conn, token)
            .map_err(|error| format!("{token}: {error}"))?;
        let numbers_time = started.elapsed();
        let started = Instant::now();
        let text = store::arena_replay::arena_match_text(conn, token)
            .map_err(|error| format!("{token}: {error}"))?;
        let text_time = started.elapsed();
        let outcome = match text {
            store::arena_replay::ArenaMatchRead::Ready(_) => "ready",
            store::arena_replay::ArenaMatchRead::MissingCardData(_) => "missing card data",
            store::arena_replay::ArenaMatchRead::NotFound => "not found",
        };
        println!(
            "read | {token} | {} | {} | {outcome} | {}",
            numbers.len(),
            ms(numbers_time),
            ms(text_time)
        );
    }
    Ok(())
}

/// One Endstep resync batch, which derives every pulled Endstep match again.
fn time_endstep_resync(conn: &mut store::Connection, pulled: usize) -> Result<(), String> {
    let batch = store::write::WriteBatch {
        endstep_resync: true,
        now_ms: now_ms(),
        ..Default::default()
    };
    let started = Instant::now();
    store::write::commit(conn, &batch).map_err(|error| format!("resync failed: {error}"))?;
    println!();
    println!(
        "endstep resync of {pulled} pulled match(es): {}",
        ms(started.elapsed())
    );
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::Summary;

    #[test]
    fn a_summary_counts_and_takes_the_largest_and_lower_median() {
        assert_eq!(
            Summary::of(&[30, 10, 20, 40]),
            Summary {
                count: 4,
                largest: 40,
                median: 20
            }
        );
        assert_eq!(
            Summary::of(&[5, 1, 3]),
            Summary {
                count: 3,
                largest: 5,
                median: 3
            }
        );
    }

    #[test]
    fn a_summary_of_nothing_is_zero() {
        assert_eq!(
            Summary::of(&[]),
            Summary {
                count: 0,
                largest: 0,
                median: 0
            }
        );
    }
}
