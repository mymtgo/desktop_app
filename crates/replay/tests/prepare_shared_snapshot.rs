//! Package `tests/Unit/PrepareSharedSnapshotTest.php`, ported, plus the
//! owner's no-size-limit rule (replays spec ruling 14).

mod common;

use common::*;
use replay::{LogEntry, PrepareFailure, ShareFailure, prepare_shared_snapshot, validate_snapshot};
use serde_json::{Value, json};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn prepared_value(json: &[u8]) -> Value {
    serde_json::from_slice(json).expect("a prepared snapshot is JSON")
}

#[test]
fn redacts_every_opponent_in_frames_and_log() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].frames[0].content["Players"]
        .as_array_mut()
        .unwrap()
        .push(json!({"Id": 3, "Name": "Second_Opp"}));
    snapshot.games[0].log.push(LogEntry {
        timestamp: "2026-09-20T18:00:03+00:00".to_string(),
        message: "Second_Opp joined.".to_string(),
    });

    let prepared = prepare_shared_snapshot(snapshot, &names(&["Opp_Name", "Second_Opp"])).unwrap();
    let encoded = String::from_utf8(prepared.json().to_vec()).unwrap();

    assert!(!encoded.contains("Opp_Name"));
    assert!(!encoded.contains("Second_Opp"));
    assert_eq!(
        prepared_value(prepared.json())["games"][0]["frames"][0]["content"]["Players"][0]["Name"],
        json!("local.player")
    );
}

#[test]
fn ignores_blank_opponent_names() {
    let prepared =
        prepare_shared_snapshot(package_snapshot(), &names(&["", "  ", "Opp_Name"])).unwrap();

    let encoded = String::from_utf8(prepared.json().to_vec()).unwrap();
    assert!(!encoded.contains("Opp_Name"));
}

#[test]
fn rejects_a_snapshot_the_site_would_refuse() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].frames[0].content["Cards"][0]["image"] = json!("http://127.0.0.1/x.jpg");

    let failure = prepare_shared_snapshot(snapshot, &names(&["Opp_Name"])).unwrap_err();

    assert!(matches!(failure, PrepareFailure::Invalid(_)));
    assert_eq!(ShareFailure::from(failure), ShareFailure::Invalid);
}

/// The spec logs a validation failure (0.x `ShareReplay` logs the
/// `ValidationException`), so the refusal carries every failing path.
#[test]
fn a_refused_snapshot_reports_the_failing_path() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].frames[0].content["Cards"][0]["image"] = json!("http://127.0.0.1/x.jpg");
    snapshot.games[0].frames[0].content["Cards"][0]
        .as_object_mut()
        .unwrap()
        .remove("Zone");

    let Err(PrepareFailure::Invalid(errors)) =
        prepare_shared_snapshot(snapshot, &names(&["Opp_Name"]))
    else {
        panic!("the snapshot should be refused as invalid");
    };

    assert_eq!(
        errors.get("games.0.frames.0.content.Cards.0.image"),
        Some(&vec!["Must be a remote https URL or null.".to_string()])
    );
    assert_eq!(
        errors.get("games.0.frames.0.content.Cards.0.Zone"),
        Some(&vec!["Must be a string.".to_string()])
    );
    assert_eq!(errors.len(), 2);
}

#[test]
fn a_name_that_survives_is_never_prepared() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].log[0].message = "xOpp_Name casts a spell.".to_string();

    let failure = prepare_shared_snapshot(snapshot, &names(&["Opp_Name"])).unwrap_err();

    assert_eq!(failure, PrepareFailure::NameSurvived);
    assert_eq!(ShareFailure::from(failure), ShareFailure::NameSurvived);
}

#[test]
fn the_prepared_json_is_the_redacted_snapshot() {
    let prepared = prepare_shared_snapshot(package_snapshot(), &names(&["Opp_Name"])).unwrap();
    let value = prepared_value(prepared.json());

    assert_eq!(validate_snapshot(&value), Ok(()));
    let mut expected = package_snapshot_value();
    expected["games"][0]["frames"][0]["content"]["Players"][1]["Name"] = json!("Opponent");
    expected["games"][0]["log"][0]["message"] = json!("Opponent casts Ragavan, Nimble Pilferer.");
    expected["games"][0]["log"][1]["message"] = json!("local.player attacks Opponent's face.");
    assert_eq!(value, expected);
    assert_eq!(prepared.into_json(), serde_json::to_vec(&value).unwrap());
}

/// The server's limit is the server's (owner, 2026-10-09; ruling 14): a
/// valid snapshot over 20 MiB prepares, and the server's 413 decides.
#[test]
fn no_size_limit_is_checked_before_upload() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].log.push(LogEntry {
        timestamp: "2026-09-20T18:00:03+00:00".to_string(),
        message: "x ".repeat(11 * 1024 * 1024),
    });

    let prepared = prepare_shared_snapshot(snapshot, &names(&["Opp_Name"])).unwrap();

    assert!(prepared.json().len() > 20 * 1024 * 1024);
}
