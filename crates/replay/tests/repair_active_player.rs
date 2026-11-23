//! Package `tests/Unit/RepairActivePlayerTest.php`, ported.

mod common;

use common::*;
use replay::{ReplayFrame, build_snapshot, repair_active_player};
use serde_json::{Map, Value, json};

struct Seen {
    hands: [i64; 2],
    libraries: [i64; 2],
    stack: bool,
    active: Option<i64>,
}

const DEFAULT: Seen = Seen {
    hands: [7, 7],
    libraries: [53, 53],
    stack: false,
    active: Some(1),
};

/// Package `activeFrame`: seats 1 and 2, seat 1 recorded as the active
/// player throughout the way the sidecar records it. Null keys are left
/// out, as the package's `array_filter` does.
fn active_frame(turn: Option<i64>, step: Option<&str>, seen: Seen) -> ReplayFrame {
    let mut content = Map::new();
    if let Some(turn) = turn {
        content.insert("Turn".to_string(), json!(turn));
    }
    if let Some(step) = step {
        content.insert("Step".to_string(), json!(step));
    }
    if let Some(active) = seen.active {
        content.insert("ActivePlayer".to_string(), json!(active));
    }
    content.insert(
        "Players".to_string(),
        json!([
            {"Id": 1, "Name": "first.player", "HandCount": seen.hands[0], "LibraryCount": seen.libraries[0]},
            {"Id": 2, "Name": "second.player", "HandCount": seen.hands[1], "LibraryCount": seen.libraries[1]},
        ]),
    );
    let cards = if seen.stack {
        json!([{"Id": 10, "CatalogID": 1234, "Zone": "Stack", "Owner": 2}])
    } else {
        json!([])
    };
    content.insert("Cards".to_string(), cards);

    ReplayFrame {
        timestamp: "12:00:00".to_string(),
        content: Value::Object(content),
    }
}

fn plain(turn: Option<i64>, step: Option<&str>) -> ReplayFrame {
    active_frame(turn, step, DEFAULT)
}

fn active(turn: i64, step: &str, seat: i64) -> ReplayFrame {
    active_frame(
        Some(turn),
        Some(step),
        Seen {
            active: Some(seat),
            ..DEFAULT
        },
    )
}

fn counts(hands: [i64; 2], libraries: [i64; 2], stack: bool) -> Seen {
    Seen {
        hands,
        libraries,
        stack,
        ..DEFAULT
    }
}

fn active_players(frames: Vec<ReplayFrame>) -> Vec<Option<i64>> {
    repair_active_player(frames)
        .iter()
        .map(|frame| frame.content.get("ActivePlayer").and_then(Value::as_i64))
        .collect()
}

#[test]
fn hands_the_turn_over_when_the_step_goes_back_and_when_the_round_changes() {
    let frames = vec![
        plain(Some(1), None),
        plain(Some(1), Some("Upkeep")),
        plain(Some(1), Some("PreCombatMain")),
        plain(Some(1), Some("EndOfTurn")),
        plain(Some(1), Some("Upkeep")),
        plain(Some(1), Some("PostCombatMain")),
        plain(Some(2), Some("Upkeep")),
    ];

    assert_eq!(
        active_players(frames),
        [1, 1, 1, 1, 2, 2, 1].map(Some).to_vec()
    );
}

#[test]
fn counts_a_round_change_even_when_the_step_does_not_go_back() {
    let frames = vec![
        plain(Some(1), Some("Upkeep")),
        plain(Some(1), Some("EndOfTurn")),
        plain(Some(1), Some("PreCombatMain")),
        plain(Some(2), Some("PreCombatMain")),
    ];

    assert_eq!(active_players(frames), [1, 1, 2, 1].map(Some).to_vec());
}

#[test]
fn hands_a_main_phase_only_turn_over_on_the_next_player_drawing() {
    let frames = vec![
        active_frame(
            Some(1),
            Some("PreCombatMain"),
            counts([7, 6], [53, 53], false),
        ),
        active_frame(
            Some(1),
            Some("PreCombatMain"),
            counts([6, 6], [53, 53], false),
        ),
        active_frame(
            Some(1),
            Some("PreCombatMain"),
            counts([6, 7], [53, 52], false),
        ),
        active_frame(
            Some(1),
            Some("PreCombatMain"),
            counts([6, 8], [53, 51], false),
        ),
    ];

    assert_eq!(active_players(frames), [1, 1, 2, 2].map(Some).to_vec());
}

#[test]
fn ignores_a_draw_with_something_on_the_stack() {
    let frames = vec![
        active_frame(
            Some(1),
            Some("PreCombatMain"),
            counts([6, 6], [53, 53], true),
        ),
        active_frame(
            Some(1),
            Some("PreCombatMain"),
            counts([6, 7], [53, 52], true),
        ),
    ];

    assert_eq!(active_players(frames), [1, 1].map(Some).to_vec());
}

#[test]
fn ignores_a_draw_once_the_turn_has_gone_past_main_phase_1() {
    let frames = vec![
        plain(Some(1), Some("Upkeep")),
        plain(Some(1), Some("BeginCombat")),
        active_frame(
            Some(1),
            Some("CombatDamage"),
            counts([7, 8], [53, 52], false),
        ),
    ];

    assert_eq!(active_players(frames), [1, 1, 1].map(Some).to_vec());
}

#[test]
fn starts_from_the_recorded_player_who_went_first() {
    let frames = vec![
        active(1, "Upkeep", 2),
        active(1, "EndOfTurn", 2),
        active(1, "Upkeep", 2),
    ];

    assert_eq!(active_players(frames), [2, 2, 1].map(Some).to_vec());
}

#[test]
fn leaves_frames_whose_active_player_already_changes_alone() {
    let frames = vec![
        active(1, "Upkeep", 1),
        active(1, "EndOfTurn", 1),
        active(1, "Upkeep", 2),
        active(1, "PostCombatMain", 1),
    ];

    assert_eq!(repair_active_player(frames.clone()), frames);
}

#[test]
fn leaves_games_without_steps_alone() {
    let none = Seen {
        active: None,
        ..DEFAULT
    };
    let frames = vec![
        active_frame(None, None, none),
        active_frame(
            None,
            None,
            Seen {
                active: None,
                ..DEFAULT
            },
        ),
    ];

    assert_eq!(repair_active_player(frames.clone()), frames);
    assert_eq!(repair_active_player(Vec::new()), Vec::new());
}

#[test]
fn builds_snapshots_with_the_active_player_repaired() {
    let frames = vec![
        plain(Some(1), Some("Upkeep")),
        plain(Some(1), Some("EndOfTurn")),
        plain(Some(1), Some("Upkeep")),
    ];

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(frames, Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    let seats: Vec<Value> = snapshot.games[0]
        .frames
        .iter()
        .map(|frame| frame.content["ActivePlayer"].clone())
        .collect();
    assert_eq!(seats, vec![json!(1), json!(1), json!(2)]);
}
