//! The anonymised `mtgo_game_history` fixture (from 0.x's test fixture, names
//! replaced by same-length fakes). Every field of all 671 records was checked
//! against 0.x's parse of the original file (names mapped through the
//! anonymiser). 0.x read the start times with a UTC system timezone, so these
//! tests read them in UTC too.

use std::path::Path;

use chrono::NaiveDateTime;
use chrono_tz::Tz;
use mtgo_history::{HistoryRecord, merge_records, parse_history};

const ZONE: Tz = chrono_tz::UTC;

fn fixture() -> Vec<u8> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/history/mtgo_game_history");
    std::fs::read(path).expect("fixtures/history/mtgo_game_history exists")
}

fn records() -> Vec<HistoryRecord> {
    parse_history(&fixture(), ZONE).expect("the fixture parses")
}

fn utc_ms(text: &str) -> i64 {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%SZ")
        .expect("time")
        .and_utc()
        .timestamp_millis()
}

#[test]
fn the_fixture_has_671_records() {
    assert_eq!(records().len(), 671);
}

#[test]
fn the_first_record() {
    let first = &records()[0];
    assert_eq!(first.id, 278_248_231);
    assert_eq!(first.start_time_ms, Some(utc_ms("2025-05-06T20:04:56Z")));
    assert_eq!(first.opponents, vec![FIRST_OPPONENT.to_string()]);
    assert_eq!((first.game_wins, first.game_losses), (0, 2));
    assert_eq!(first.game_ids, vec![915_866_742, 915_867_768]);
    assert_eq!(first.format.as_deref(), Some("CMODERN"));
    assert_eq!(first.game_wins_to_win, Some(2));
    assert_eq!(first.round, 0);
}

#[test]
fn every_record_is_unranked_and_best_of_one_or_three() {
    for record in records() {
        assert_eq!(record.round, 0, "record {}", record.id);
        assert!(
            matches!(record.game_wins_to_win, Some(1) | Some(2)),
            "record {}",
            record.id
        );
    }
}

#[test]
fn six_records_have_no_opponents() {
    let empty = records()
        .iter()
        .filter(|record| record.opponents.is_empty())
        .count();
    assert_eq!(empty, 6);
}

#[test]
fn ids_are_unique_and_merging_the_file_twice_changes_nothing() {
    let records = records();
    let merged = merge_records(vec![records.clone(), records.clone()]);
    assert_eq!(merged, records);
}

#[test]
fn every_cut_of_the_fixture_is_an_error() {
    let bytes = fixture();
    let step = 997;
    let mut cut = 0;
    while cut < bytes.len() {
        assert!(parse_history(&bytes[..cut], ZONE).is_err(), "cut at {cut}");
        cut += step;
    }
}

/// The first record's opponent, as the anonymiser renamed it.
const FIRST_OPPONENT: &str = "Qmcxxxxxx";

/// The local player, as the anonymiser renamed it.
const LOCAL: &str = "Qzbxxxxxxx";

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn the_start_time_is_local_wall_clock_in_the_given_zone() {
    let london = parse_history(&fixture(), chrono_tz::Europe::London).expect("parses");
    // 20:04:56 on 6 May is British Summer Time, an hour ahead of UTC.
    assert_eq!(
        london[0].start_time_ms,
        Some(utc_ms("2025-05-06T19:04:56Z"))
    );
}

#[test]
fn spot_checks_through_the_file() {
    let records = records();

    let second = &records[1];
    assert_eq!(second.id, 278_248_597);
    assert_eq!(second.start_time_ms, Some(utc_ms("2025-05-06T20:18:19Z")));
    assert_eq!((second.game_wins, second.game_losses), (0, 0));
    assert_eq!(second.game_ids, vec![915_868_176]);
    assert_eq!(second.opponents, names(&[FIRST_OPPONENT]));
    assert!(second.match_winners.is_empty() && second.match_losers.is_empty());

    let hundredth = &records[100];
    assert_eq!(hundredth.id, 279_673_955);
    assert_eq!(
        hundredth.start_time_ms,
        Some(utc_ms("2025-07-02T17:02:40Z"))
    );
    assert_eq!((hundredth.game_wins, hundredth.game_losses), (0, 1));
    assert_eq!(hundredth.game_ids, vec![921_385_710]);
    assert_eq!(hundredth.opponents, names(&["Qsxxxxx"]));
    assert_eq!(hundredth.match_winners, names(&["Qsxxxxx"]));
    assert_eq!(hundredth.match_losers, names(&[LOCAL]));

    let pauper = &records[400];
    assert_eq!(pauper.id, 283_684_882);
    assert_eq!(pauper.start_time_ms, Some(utc_ms("2025-12-18T13:00:20Z")));
    assert_eq!(pauper.game_ids, vec![937_302_574, 937_302_802]);
    assert_eq!(pauper.format.as_deref(), Some("CPAUPER"));
    assert_eq!(pauper.description, "Qjxxxxxxxxxx");

    let last = &records[670];
    assert_eq!(last.id, 285_754_011);
    assert_eq!(last.start_time_ms, Some(utc_ms("2026-03-18T19:44:15Z")));
    assert_eq!((last.game_wins, last.game_losses), (2, 1));
    assert_eq!(last.game_ids, vec![945_530_736, 945_531_478, 945_531_682]);
    assert_eq!(last.match_winners, names(&[LOCAL]));
    assert_eq!(last.match_losers, names(&["Qkbxxxxxx"]));
}

#[test]
fn the_records_without_opponents() {
    let ids: Vec<u64> = records()
        .iter()
        .filter(|record| record.opponents.is_empty())
        .map(|record| record.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            279_600_083,
            279_773_869,
            279_982_004,
            281_715_117,
            281_732_421,
            281_732_426
        ]
    );
}

#[test]
fn formats_and_descriptions() {
    let records = records();
    let count = |code: &str| {
        records
            .iter()
            .filter(|record| record.format.as_deref() == Some(code))
            .count()
    };
    assert_eq!(
        (count("CMODERN"), count("CPAUPER"), count("CPIONEER")),
        (378, 255, 38)
    );
    let best_of_one = records
        .iter()
        .filter(|record| record.game_wins_to_win == Some(1))
        .count();
    assert_eq!(best_of_one, 6);
    let described = records
        .iter()
        .filter(|record| !record.description.is_empty())
        .count();
    assert_eq!(described, 141);
}
