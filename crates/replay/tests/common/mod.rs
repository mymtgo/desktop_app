//! Helpers shared by the ported package tests (package `tests/Unit/*.php`).
//! The package's own fakes `local.player` and `Opp_Name` are kept.

#![allow(dead_code)]

use std::collections::BTreeMap;

use replay::{CardInfo, GameInput, LogEntry, ReplayFrame, SnapshotInput};
use serde_json::{Value, json};

/// 2026-09-20T18:00:00+00:00 as UTC unix milliseconds.
pub const PLAYED_AT_MS: i64 = 1_789_927_200_000;

/// Package `buildTimelineRow`: local.player (Id 1) against Opp_Name (Id 2)
/// and a single card of the given catalog id.
pub fn timeline_row(catalog_id: u32, timestamp: &str) -> ReplayFrame {
    ReplayFrame {
        timestamp: timestamp.to_string(),
        content: json!({
            "Players": [{"Id": 1, "Name": "local.player"}, {"Id": 2, "Name": "Opp_Name"}],
            "Cards": [{"Id": 10, "CatalogID": catalog_id, "Zone": "Battlefield", "Owner": 2}],
        }),
    }
}

/// `timeline_row` with the package's default card and time.
pub fn row() -> ReplayFrame {
    timeline_row(1234, "2026-09-20T18:00:01+00:00")
}

/// Package `buildMatchInput`.
pub fn match_input(games: Vec<GameInput>, format: Option<&str>) -> SnapshotInput {
    SnapshotInput {
        format: format.map(str::to_string),
        played_at_ms: PLAYED_AT_MS,
        local_archetype: Some("Boros Energy".to_string()),
        opponent_archetype: None,
        games,
    }
}

/// Package `buildGameInput`.
pub fn game_input(frames: Vec<ReplayFrame>, won: Option<bool>, log: Vec<LogEntry>) -> GameInput {
    GameInput {
        won,
        local_username: "local.player".to_string(),
        frames,
        log,
        sideboard: Vec::new(),
    }
}

pub fn card(name: &str, type_line: &str, image: &str) -> CardInfo {
    CardInfo {
        name: Some(name.to_string()),
        type_line: Some(type_line.to_string()),
        image: Some(image.to_string()),
        local_image: None,
        other_image: None,
    }
}

/// Package `buildCardResolver`.
pub fn card_resolver(_ids: &[u32]) -> BTreeMap<u32, CardInfo> {
    BTreeMap::from([
        (
            1234,
            card(
                "Ragavan, Nimble Pilferer",
                "Legendary Creature",
                "https://cards.scryfall.io/a.jpg",
            ),
        ),
        (
            5678,
            card("Island", "Basic Land", "http://127.0.0.1:8100/local.jpg"),
        ),
    ])
}

/// The first card of a frame.
pub fn first_card(frame: &ReplayFrame) -> &Value {
    &frame.content["Cards"][0]
}

/// The package fixture `replaySnapshot()` (`tests/Fixtures.php`) as JSON.
pub fn package_snapshot_value() -> Value {
    serde_json::from_str(include_str!("../fixtures/package_snapshot.json"))
        .expect("the package fixture is JSON")
}

/// The package fixture as a typed snapshot.
pub fn package_snapshot() -> replay::Snapshot {
    serde_json::from_value(package_snapshot_value()).expect("the package fixture is a snapshot")
}

/// the API's `shareSnapshot()` (`tests/Feature/Replays/ShareReplayTest.php`)
/// as JSON: a complete v2 snapshot the server accepts.
pub fn api_snapshot_value() -> Value {
    serde_json::from_str(include_str!("../fixtures/api_snapshot.json"))
        .expect("the api fixture is JSON")
}
