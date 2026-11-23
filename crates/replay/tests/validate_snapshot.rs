//! Package `tests/Unit/ValidateReplaySnapshotTest.php`, ported, with every
//! rule's dotted path and message (survey section 2.6) and the two
//! fixtures: the package's `replaySnapshot()` and the API's
//! `shareSnapshot()`.

mod common;

use common::*;
use replay::{Snapshot, ValidationErrors, validate_snapshot};
use serde_json::{Value, json};

fn errors(snapshot: &Value) -> ValidationErrors {
    validate_snapshot(snapshot).expect_err("the snapshot should be refused")
}

fn messages(errors: &ValidationErrors, path: &str) -> Vec<String> {
    errors.get(path).cloned().unwrap_or_default()
}

fn remove(value: &mut Value, pointer: &str, key: &str) {
    value
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("an object at the pointer")
        .remove(key);
}

const CARD: &str = "/games/0/frames/0/content/Cards/0";

/// One rule case: a label, the change that breaks the snapshot, the path and
/// the message expected.
type RuleCase = (
    &'static str,
    Box<dyn Fn(&mut Value)>,
    &'static str,
    &'static str,
);

#[test]
fn accepts_a_valid_snapshot_and_returns_it_unchanged() {
    let snapshot = package_snapshot_value();

    assert_eq!(validate_snapshot(&snapshot), Ok(()));
    assert_eq!(snapshot, package_snapshot_value());
}

#[test]
fn accepts_a_card_with_no_image() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"][0]["frames"][0]["content"]["Cards"][0]["image"] = Value::Null;

    assert_eq!(validate_snapshot(&snapshot), Ok(()));
}

#[test]
fn accepts_a_frame_with_no_cards_and_an_empty_log() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"][0]["frames"][0]["content"]["Cards"] = json!([]);
    snapshot["games"][0]["log"] = json!([]);

    assert_eq!(validate_snapshot(&snapshot), Ok(()));
}

#[test]
fn rejects_a_snapshot_with_the_wrong_version() {
    let mut snapshot = package_snapshot_value();
    snapshot["version"] = json!(1);

    assert_eq!(
        messages(&errors(&snapshot), "version"),
        ["Unsupported snapshot version."]
    );
}

#[test]
fn rejects_a_snapshot_with_no_frames() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"][0]["frames"] = json!([]);

    assert_eq!(
        messages(&errors(&snapshot), "games.0.frames"),
        ["Must be a non-empty list."]
    );
}

#[test]
fn rejects_a_snapshot_with_missing_top_level_keys() {
    let cases = [
        ("version", "Unsupported snapshot version."),
        ("meta", "Required."),
        ("games", "Must be a non-empty list."),
    ];

    for (key, message) in cases {
        let mut snapshot = package_snapshot_value();
        remove(&mut snapshot, "", key);

        assert_eq!(messages(&errors(&snapshot), key), [message], "{key}");
    }
}

#[test]
fn rejects_a_match_with_no_games() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"] = json!([]);

    assert_eq!(
        messages(&errors(&snapshot), "games"),
        ["Must be a non-empty list."]
    );
}

#[test]
fn rejects_a_game_missing_its_own_keys() {
    let cases = [
        ("game_number", "Must be a positive integer."),
        ("won", "Must be true, false or null."),
        ("frames", "Must be a non-empty list."),
        ("log", "Must be a list."),
    ];

    for (key, message) in cases {
        let mut snapshot = package_snapshot_value();
        remove(&mut snapshot, "/games/0", key);

        assert_eq!(
            messages(&errors(&snapshot), &format!("games.0.{key}")),
            [message],
            "{key}"
        );
    }
}

#[test]
fn accepts_several_games() {
    let mut snapshot = package_snapshot_value();
    let mut second = snapshot["games"][0].clone();
    second["game_number"] = json!(2);
    second["won"] = Value::Null;
    snapshot["games"].as_array_mut().unwrap().push(second);

    assert_eq!(validate_snapshot(&snapshot), Ok(()));
}

#[test]
fn rejects_bad_meta() {
    let cases = [
        ("played_at", Value::Null, "Must be a string."),
        ("games_in_match", json!(0), "Must be a positive integer."),
        (
            "games_in_match",
            json!("three"),
            "Must be a positive integer.",
        ),
        ("format", json!(12), "Must be a string or null."),
    ];

    for (key, value, message) in cases {
        let mut snapshot = package_snapshot_value();
        snapshot["meta"][key] = value.clone();

        assert_eq!(
            messages(&errors(&snapshot), &format!("meta.{key}")),
            [message],
            "{key} = {value}"
        );
    }
}

#[test]
fn rejects_a_player_without_an_integer_id_or_string_name() {
    let path = "games.0.frames.0.content.Players.0";
    let cases = [
        (
            json!({"Id": "1", "Name": "local.player"}),
            "Id",
            "Must be an integer.",
        ),
        (json!({"Id": 1}), "Name", "Must be a string."),
    ];

    for (player, key, message) in cases {
        let mut snapshot = package_snapshot_value();
        snapshot["games"][0]["frames"][0]["content"]["Players"][0] = player;

        assert_eq!(
            messages(&errors(&snapshot), &format!("{path}.{key}")),
            [message],
            "{key}"
        );
    }
}

#[test]
fn rejects_a_card_missing_a_required_key() {
    let cases = [
        ("Id", "Must be an integer."),
        ("CatalogID", "Must be an integer."),
        ("Zone", "Must be a string."),
        ("Owner", "Must be an integer."),
    ];

    for (key, message) in cases {
        let mut snapshot = package_snapshot_value();
        remove(&mut snapshot, CARD, key);

        assert_eq!(
            messages(
                &errors(&snapshot),
                &format!("games.0.frames.0.content.Cards.0.{key}")
            ),
            [message],
            "{key}"
        );
    }
}

#[test]
fn rejects_a_card_image_that_is_not_a_remote_https_url() {
    let images = [
        "http://127.0.0.1:8100/storage/cards/ab.jpg",
        "http://localhost/storage/cards/ab.jpg",
        "/storage/cards/ab.jpg",
    ];

    for image in images {
        let mut snapshot = package_snapshot_value();
        snapshot["games"][0]["frames"][0]["content"]["Cards"][0]["image"] = json!(image);

        assert_eq!(
            messages(&errors(&snapshot), "games.0.frames.0.content.Cards.0.image"),
            ["Must be a remote https URL or null."],
            "{image}"
        );
    }
}

#[test]
fn rejects_a_log_entry_without_a_message() {
    let mut snapshot = package_snapshot_value();
    remove(&mut snapshot, "/games/0/log/0", "message");

    assert_eq!(
        messages(&errors(&snapshot), "games.0.log.0.message"),
        ["Must be a string."]
    );
}

#[test]
fn names_the_offending_path_in_the_error() {
    let mut snapshot = package_snapshot_value();
    remove(&mut snapshot, CARD, "Zone");

    let errors = errors(&snapshot);

    assert!(errors.contains_key("games.0.frames.0.content.Cards.0.Zone"));
    assert_eq!(errors.len(), 1);
}

#[test]
fn accepts_a_game_carrying_your_sideboard() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"][0]["sideboard"] = json!([
        {"catalog_id": 1234, "quantity": 2, "name": "Duress", "type": "Sorcery", "image": "https://cards.scryfall.io/d.jpg"},
        {"catalog_id": 5678, "quantity": 1, "name": null, "type": null, "image": null},
    ]);

    assert_eq!(validate_snapshot(&snapshot), Ok(()));
}

#[test]
fn rejects_a_malformed_sideboard() {
    let path = "games.0.sideboard";
    let cases = [
        (
            "not a list",
            json!({"a": 1}),
            path.to_string(),
            "Must be a list.",
        ),
        (
            "zero quantity",
            json!([{"catalog_id": 1, "quantity": 0, "name": null, "type": null, "image": null}]),
            format!("{path}.0.quantity"),
            "Must be a positive integer.",
        ),
        (
            "string id",
            json!([{"catalog_id": "1", "quantity": 1, "name": null, "type": null, "image": null}]),
            format!("{path}.0.catalog_id"),
            "Must be an integer.",
        ),
        (
            "local image",
            json!([{"catalog_id": 1, "quantity": 1, "name": null, "type": null, "image": "http://127.0.0.1/x.jpg"}]),
            format!("{path}.0.image"),
            "Must be a remote https URL or null.",
        ),
        (
            "numeric name",
            json!([{"catalog_id": 1, "quantity": 1, "name": 5, "type": null, "image": null}]),
            format!("{path}.0.name"),
            "Must be a string or null.",
        ),
    ];

    for (label, sideboard, error_path, message) in cases {
        let mut snapshot = package_snapshot_value();
        snapshot["games"][0]["sideboard"] = sideboard;

        assert_eq!(
            messages(&errors(&snapshot), &error_path),
            [message],
            "{label}"
        );
    }
}

#[test]
fn rejects_an_other_face_image_that_is_not_a_remote_https_url() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"][0]["frames"][0]["content"]["Cards"][0]["other_image"] =
        json!("http://127.0.0.1:8100/storage/cards/ab.jpg");

    assert_eq!(
        messages(
            &errors(&snapshot),
            "games.0.frames.0.content.Cards.0.other_image"
        ),
        ["Must be a remote https URL or null."]
    );
}

#[test]
fn accepts_a_remote_other_face_image() {
    let mut snapshot = package_snapshot_value();
    snapshot["games"][0]["frames"][0]["content"]["Cards"][0]["other_image"] =
        json!("https://cards.scryfall.io/front.jpg");

    assert_eq!(validate_snapshot(&snapshot), Ok(()));
}

/// The remaining rows of survey section 2.6, each with its path and
/// message, and the PHP shapes JSON maps onto.
#[test]
fn every_rule_names_its_path_and_message() {
    let cases: Vec<RuleCase> = vec![
        (
            "meta not an object",
            Box::new(|s| s["meta"] = json!("x")),
            "meta",
            "Required.",
        ),
        (
            "meta archetype missing",
            Box::new(|s| remove(s, "/meta", "local_archetype")),
            "meta.local_archetype",
            "Must be a string or null.",
        ),
        (
            "meta opponent archetype a number",
            Box::new(|s| s["meta"]["opponent_archetype"] = json!(1)),
            "meta.opponent_archetype",
            "Must be a string or null.",
        ),
        (
            "games_in_match a float",
            Box::new(|s| s["meta"]["games_in_match"] = json!(3.0)),
            "meta.games_in_match",
            "Must be a positive integer.",
        ),
        (
            "version a float",
            Box::new(|s| s["version"] = json!(2.0)),
            "version",
            "Unsupported snapshot version.",
        ),
        (
            "games an object",
            Box::new(|s| s["games"] = json!({"0": s["games"][0].clone()})),
            "games",
            "Must be a non-empty list.",
        ),
        (
            "game not an object",
            Box::new(|s| s["games"][0] = json!(7)),
            "games.0",
            "Must be an object.",
        ),
        (
            "game number zero",
            Box::new(|s| s["games"][0]["game_number"] = json!(0)),
            "games.0.game_number",
            "Must be a positive integer.",
        ),
        (
            "won a string",
            Box::new(|s| s["games"][0]["won"] = json!("yes")),
            "games.0.won",
            "Must be true, false or null.",
        ),
        (
            "frame timestamp a number",
            Box::new(|s| s["games"][0]["frames"][0]["timestamp"] = json!(12)),
            "games.0.frames.0.timestamp",
            "Must be a string.",
        ),
        (
            "frame content missing",
            Box::new(|s| remove(s, "/games/0/frames/0", "content")),
            "games.0.frames.0.content",
            "Required.",
        ),
        (
            "players an object",
            Box::new(|s| s["games"][0]["frames"][0]["content"]["Players"] = json!({})),
            "games.0.frames.0.content.Players",
            "Must be a list.",
        ),
        (
            "cards missing",
            Box::new(|s| remove(s, "/games/0/frames/0/content", "Cards")),
            "games.0.frames.0.content.Cards",
            "Must be a list.",
        ),
        (
            "log an object",
            Box::new(|s| s["games"][0]["log"] = json!({})),
            "games.0.log",
            "Must be a list.",
        ),
        (
            "log timestamp missing",
            Box::new(|s| remove(s, "/games/0/log/1", "timestamp")),
            "games.0.log.1.timestamp",
            "Must be a string.",
        ),
        (
            "sideboard null",
            Box::new(|s| s["games"][0]["sideboard"] = Value::Null),
            "games.0.sideboard",
            "Must be a list.",
        ),
        (
            "sideboard type missing",
            Box::new(|s| {
                s["games"][0]["sideboard"] =
                    json!([{"catalog_id": 1, "quantity": 1, "name": null, "image": null}])
            }),
            "games.0.sideboard.0.type",
            "Must be a string or null.",
        ),
        (
            "sideboard quantity missing",
            Box::new(|s| {
                s["games"][0]["sideboard"] = json!([{"catalog_id": 1, "name": null, "type": null}])
            }),
            "games.0.sideboard.0.quantity",
            "Must be a positive integer.",
        ),
    ];

    for (label, change, path, message) in cases {
        let mut snapshot = package_snapshot_value();
        change(&mut snapshot);

        assert_eq!(messages(&errors(&snapshot), path), [message], "{label}");
    }
}

#[test]
fn every_failure_is_collected_in_one_pass() {
    let mut snapshot = package_snapshot_value();
    snapshot["version"] = json!(3);
    snapshot["meta"]["played_at"] = json!(1);
    snapshot["games"][0]["frames"][0]["content"]["Cards"][0] = json!({});

    let errors = errors(&snapshot);

    let paths: Vec<&str> = errors.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "games.0.frames.0.content.Cards.0.CatalogID",
            "games.0.frames.0.content.Cards.0.Id",
            "games.0.frames.0.content.Cards.0.Owner",
            "games.0.frames.0.content.Cards.0.Zone",
            "meta.played_at",
            "version",
        ]
    );
}

#[test]
fn the_package_fixture_round_trips() {
    let value = package_snapshot_value();

    assert_eq!(validate_snapshot(&value), Ok(()));
    let snapshot: Snapshot = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), value);
}

#[test]
fn the_api_fixture_validates() {
    let value = api_snapshot_value();

    assert_eq!(validate_snapshot(&value), Ok(()));
    let snapshot: Snapshot = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), value);
}
