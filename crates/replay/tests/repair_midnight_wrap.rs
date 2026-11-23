//! Package `tests/Unit/RepairMidnightWrapTest.php`, ported, plus the v1
//! guarantee that time-of-day frames are never moved.

mod common;

use common::*;
use replay::{ReplayFrame, build_snapshot, repair_midnight_wrap};
use serde_json::json;

fn wrap_frames(timestamps: &[&str]) -> Vec<ReplayFrame> {
    timestamps
        .iter()
        .map(|timestamp| ReplayFrame {
            timestamp: timestamp.to_string(),
            content: json!({"Tag": timestamp}),
        })
        .collect()
}

fn times(frames: &[ReplayFrame]) -> Vec<&str> {
    frames
        .iter()
        .map(|frame| frame.timestamp.as_str())
        .collect()
}

#[test]
fn moves_frames_dated_before_the_real_start_to_the_end_a_day_later() {
    let frames = wrap_frames(&[
        "2026-09-26T00:00:10Z",
        "2026-09-26T00:00:39Z",
        "2026-09-26T23:48:07Z",
        "2026-09-26T23:59:57Z",
    ]);

    let repaired = repair_midnight_wrap(frames);

    assert_eq!(
        times(&repaired),
        vec![
            "2026-09-26T23:48:07Z",
            "2026-09-26T23:59:57Z",
            "2026-09-27T00:00:10Z",
            "2026-09-27T00:00:39Z",
        ]
    );
    assert_eq!(repaired[2].content, json!({"Tag": "2026-09-26T00:00:10Z"}));
}

#[test]
fn rolls_the_date_over_a_month_end() {
    let frames = wrap_frames(&["2026-09-30T00:00:05Z", "2026-09-30T23:58:00Z"]);

    assert_eq!(
        times(&repair_midnight_wrap(frames)),
        vec!["2026-09-30T23:58:00Z", "2026-10-01T00:00:05Z"]
    );
}

#[test]
fn leaves_a_correctly_dated_game_past_midnight_alone() {
    let frames = wrap_frames(&[
        "2026-09-26T23:48:07Z",
        "2026-09-26T23:59:57Z",
        "2026-09-27T00:00:10Z",
    ]);

    assert_eq!(repair_midnight_wrap(frames.clone()), frames);
}

#[test]
fn leaves_time_of_day_frames_from_a_desktop_share_alone() {
    let frames = wrap_frames(&["00:00:10", "23:48:07"]);

    assert_eq!(repair_midnight_wrap(frames.clone()), frames);
    assert_eq!(repair_midnight_wrap(Vec::new()), Vec::new());
}

#[test]
fn builds_snapshots_from_wrapped_bundles_in_play_order() {
    let frames = vec![
        timeline_row(1234, "2026-09-26T00:00:10Z"),
        timeline_row(1234, "2026-09-26T23:48:07Z"),
    ];

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(frames, Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    assert_eq!(
        times(&snapshot.games[0].frames),
        vec!["2026-09-26T23:48:07Z", "2026-09-27T00:00:10Z"]
    );
}

/// v1 frames carry the log line's `HH:MM:SS` (with or without milliseconds),
/// so a game crossing midnight is never reordered, and one time-of-day
/// frame among ISO ones stops the repair too.
#[test]
fn time_of_day_frames_are_never_wrapped() {
    let plain = wrap_frames(&["23:59:58", "00:00:01", "13:00:00"]);
    let millis = wrap_frames(&["23:59:58.120", "00:00:01.500", "13:00:00.000"]);
    let mixed = wrap_frames(&["2026-09-26T00:00:10Z", "23:48:07", "2026-09-26T23:59:57Z"]);

    assert_eq!(repair_midnight_wrap(plain.clone()), plain);
    assert_eq!(repair_midnight_wrap(millis.clone()), millis);
    assert_eq!(repair_midnight_wrap(mixed.clone()), mixed);
}

#[test]
fn an_offset_timestamp_is_read_in_its_own_zone() {
    // 23:30 at +02:00 is 21:30 UTC, under 12 hours after 10:00 UTC: no wrap.
    let frames = wrap_frames(&["2026-09-26T10:00:00+00:00", "2026-09-26T23:30:00+02:00"]);

    assert_eq!(repair_midnight_wrap(frames.clone()), frames);
}
