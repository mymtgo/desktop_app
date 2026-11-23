//! Ports 0.x `ComputePickTimingsTest`.

mod common;

use common::{ELEVEN_AM, SECOND_MS, pick};
use limited::{
    DraftPickStats, Pick, PickTiming, Reservation, TimingSummary, draft_pick_stats, pick_timing,
    timing_summary,
};

fn reservations(count: usize) -> Vec<Reservation> {
    (0..count)
        .map(|index| Reservation {
            catalog_id: index as u32 + 1,
            at_ms: ELEVEN_AM,
        })
        .collect()
}

/// A pick shown at 11:00:00, picked `seconds` later, with a deadline a minute
/// after it was shown.
fn timed(ordinal: u32, pack: u32, seconds: i64, reserved: usize) -> Pick {
    Pick {
        pack_number: pack,
        shown_at_ms: Some(ELEVEN_AM),
        picked_at_ms: Some(ELEVEN_AM + seconds * SECOND_MS),
        deadline_at_ms: Some(ELEVEN_AM + 60 * SECOND_MS),
        reservations: reservations(reserved),
        ..pick(ordinal)
    }
}

#[test]
fn computes_elapsed_margin_and_indecision_for_a_pick() {
    let pick = Pick {
        shown_at_ms: Some(ELEVEN_AM),
        picked_at_ms: Some(ELEVEN_AM + 26 * SECOND_MS),
        deadline_at_ms: Some(ELEVEN_AM + 69 * SECOND_MS),
        reservations: reservations(2),
        ..pick(1)
    };

    assert_eq!(
        pick_timing(&pick),
        PickTiming {
            elapsed_seconds: Some(26),
            margin_seconds: Some(43),
            reservation_count: 2,
            indecisive: true,
        }
    );
}

#[test]
fn returns_nones_when_timestamps_are_missing() {
    assert_eq!(
        pick_timing(&pick(1)),
        PickTiming {
            elapsed_seconds: None,
            margin_seconds: None,
            reservation_count: 0,
            indecisive: false,
        }
    );
}

#[test]
fn a_pick_made_after_its_deadline_has_a_negative_margin() {
    let late = Pick {
        deadline_at_ms: Some(ELEVEN_AM + 10 * SECOND_MS),
        ..timed(1, 1, 12, 0)
    };

    assert_eq!(pick_timing(&late).margin_seconds, Some(-2));
}

#[test]
fn summarises_a_draft() {
    let picks = vec![
        timed(1, 1, 30, 2),
        timed(2, 1, 20, 1),
        timed(15, 2, 10, 0),
        timed(29, 3, 4, 3),
    ];

    assert_eq!(
        timing_summary(&picks),
        TimingSummary {
            avg_seconds: Some(16),
            avg_margin_seconds: Some(44),
            indecisive_count: 2,
            fastest_pack: Some(3),
            slowest_pack: Some(1),
        }
    );
}

#[test]
fn reports_no_average_margin_when_no_pick_has_a_deadline() {
    let picks = vec![Pick {
        shown_at_ms: Some(ELEVEN_AM),
        picked_at_ms: Some(ELEVEN_AM + 12 * SECOND_MS),
        ..pick(1)
    }];

    assert_eq!(timing_summary(&picks).avg_margin_seconds, None);
}

#[test]
fn draft_pick_stats_count_only_picked_picks() {
    let mut picks: Vec<Pick> = (1..=4).map(|ordinal| timed(ordinal, 1, 9, 0)).collect();
    picks[0].reservations = reservations(2);
    picks.push(pick(5));
    let draft = common::draft("q-draft", picks);

    assert_eq!(
        draft_pick_stats(&draft),
        Some(DraftPickStats {
            avg_seconds: Some(9),
            indecisive: 1,
            picks: 4,
        })
    );
    assert_eq!(
        draft_pick_stats(&common::draft("q-empty", vec![pick(1)])),
        None
    );
}
