//! `state-size`: how much per-match state the projector holds over a long
//! session (issue #49). Replays MTGO log files, oldest first, through the
//! projector with the engine's finalize and retire steps on log time, and
//! prints the footprint every 50,000 events and after each file.
//!   cargo run --release -p tracker --bin state-size -- mtgo-1.log mtgo-2.log
//! Each file is dated one day after the one before (MTGO starts a new log
//! daily); only the time between events matters here. Storage lookups are not
//! answered, so held events are an upper bound.

use std::process::ExitCode;

use chrono::{Days, NaiveDate};
use mtgo_match::{Footprint, Projector};
use mtgo_protocol::{EventKind, FileContext, split_complete};

const REPORT_EVERY: usize = 50_000;
const SWEEP_EVERY: usize = 1_000;

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: state-size <mtgo.log>... (oldest first)");
        return ExitCode::FAILURE;
    }
    let first_day = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
    let mut projector = Projector::new(Vec::new());
    let mut events = 0usize;
    let mut first_ms: Option<i64> = None;
    let mut newest_ms = 0i64;
    println!(
        "point | events | log hours | matches | results | revisions | match ids | game ids | held events | retired | asked | dropped ids"
    );
    for (index, path) in paths.iter().enumerate() {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("could not read {path}: {error}");
                return ExitCode::FAILURE;
            }
        };
        let day = first_day
            .checked_add_days(Days::new(index as u64))
            .unwrap_or(first_day);
        let mut ctx = FileContext::new(index as u64 + 1, day, chrono_tz::UTC, None);
        for raw in split_complete(&bytes).iter() {
            let Some(event) = ctx.prepare(raw) else {
                continue;
            };
            first_ms.get_or_insert(event.at_ms);
            newest_ms = newest_ms.max(event.at_ms);
            if event.kind == EventKind::Login
                && let Some(name) = &event.local_username
            {
                projector.add_account(name);
            }
            projector.apply(&event);
            projector.take_lookups();
            events += 1;
            if events.is_multiple_of(SWEEP_EVERY) {
                projector.finalize(newest_ms);
                projector.retire_quiet(newest_ms);
            }
            if events.is_multiple_of(REPORT_EVERY) {
                let hours = log_hours(first_ms, newest_ms);
                print_row(
                    &format!("{events} events"),
                    events,
                    hours,
                    projector.footprint(),
                );
            }
        }
        projector.finalize(newest_ms);
        projector.retire_quiet(newest_ms);
        print_row(
            path,
            events,
            log_hours(first_ms, newest_ms),
            projector.footprint(),
        );
    }
    ExitCode::SUCCESS
}

fn log_hours(first_ms: Option<i64>, newest_ms: i64) -> f64 {
    let first = first_ms.unwrap_or(newest_ms);
    (newest_ms - first) as f64 / 3_600_000.0
}

fn print_row(point: &str, events: usize, hours: f64, f: Footprint) {
    println!(
        "{point} | {events} | {hours:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {}",
        f.matches,
        f.results,
        f.revisions,
        f.match_ids,
        f.game_ids,
        f.held_events,
        f.retired,
        f.asked,
        f.dropped_ids
    );
}
