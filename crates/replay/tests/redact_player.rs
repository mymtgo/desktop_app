//! Package `tests/Unit/RedactPlayerTest.php`, ported, plus the byte-wise
//! matching PHP's `preg_replace` (no `/u`) and `stripos` do.

mod common;

use common::*;
use replay::{RedactError, Snapshot, assert_gone, redact_player};
use serde_json::json;

const LOG_SURVIVED: &str = "A log line still contains the redacted username.";
const NAME_SURVIVED: &str = "A player name still contains the redacted username.";

fn with_message(message: &str) -> Snapshot {
    let mut snapshot = package_snapshot();
    snapshot.games[0].log[0].message = message.to_string();
    snapshot
}

fn redacted_message(message: &str, username: &str) -> Result<String, RedactError> {
    let redacted = redact_player(with_message(message), username)?;
    Ok(redacted.games[0].log[0].message.clone())
}

#[test]
fn renames_the_player_in_every_frame() {
    let mut snapshot = package_snapshot();
    let frame = snapshot.games[0].frames[0].clone();
    snapshot.games[0].frames.push(frame);

    let redacted = redact_player(snapshot, "Opp_Name").unwrap();

    assert_eq!(redacted.games[0].frames.len(), 2);
    for frame in &redacted.games[0].frames {
        assert_eq!(frame.content["Players"][1]["Name"], json!("Opponent"));
        assert_eq!(frame.content["Players"][0]["Name"], json!("local.player"));
    }
}

#[test]
fn replaces_whole_token_mentions_in_the_log_keeping_punctuation() {
    let redacted = redact_player(package_snapshot(), "Opp_Name").unwrap();

    assert_eq!(
        redacted.games[0].log[0].message,
        "Opponent casts Ragavan, Nimble Pilferer."
    );
    assert_eq!(
        redacted.games[0].log[1].message,
        "local.player attacks Opponent's face."
    );
}

#[test]
fn replaces_a_name_that_ends_a_sentence_or_sits_in_punctuation() {
    let cases = [
        (
            "full stop",
            "local.player casts Thoughtseize targeting Opp_Name.",
            "local.player casts Thoughtseize targeting Opponent.",
        ),
        (
            "ellipsis",
            "Waiting for Opp_Name...",
            "Waiting for Opponent...",
        ),
        ("dash after", "Opp_Name - conceded", "Opponent - conceded"),
        ("parentheses", "(Opp_Name)", "(Opponent)"),
    ];

    for (label, message, expected) in cases {
        assert_eq!(
            redacted_message(message, "Opp_Name"),
            Ok(expected.to_string()),
            "{label}"
        );
    }
}

#[test]
fn replaces_a_dotted_username_without_eating_its_dots() {
    let mut snapshot = with_message("Targeting opp.name.");
    snapshot.games[0].frames[0].content["Players"][1]["Name"] = json!("opp.name");

    let redacted = redact_player(snapshot, "opp.name").unwrap();

    assert_eq!(redacted.games[0].log[0].message, "Targeting Opponent.");
}

#[test]
fn replaces_a_hyphenated_username_without_eating_its_hyphens() {
    let mut snapshot = with_message("Targeting Q-foe.");
    snapshot.games[0].frames[0].content["Players"][1]["Name"] = json!("Q-foe");

    let redacted = redact_player(snapshot, "Q-foe").unwrap();

    assert_eq!(redacted.games[0].log[0].message, "Targeting Opponent.");

    let mut glued = with_message("Q-foe-x attacks");
    glued.games[0].frames[0].content["Players"][1]["Name"] = json!("Q-foe");
    assert_eq!(
        redact_player(glued, "Q-foe"),
        Err(RedactError::NameSurvived(LOG_SURVIVED.to_string()))
    );
}

#[test]
fn matches_the_username_case_insensitively() {
    let mut snapshot = with_message("opp_name casts a spell.");
    snapshot.games[0].frames[0].content["Players"][1]["Name"] = json!("OPP_NAME");

    let redacted = redact_player(snapshot, "Opp_Name").unwrap();

    assert_eq!(redacted.games[0].log[0].message, "Opponent casts a spell.");
    assert_eq!(
        redacted.games[0].frames[0].content["Players"][1]["Name"],
        json!("Opponent")
    );
}

#[test]
fn leaves_card_fields_alone_when_a_card_name_contains_the_username() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].frames[0].content["Cards"][0]["name"] = json!("Opp_Name's Folly");

    let redacted = redact_player(snapshot, "Opp_Name").unwrap();

    assert_eq!(
        redacted.games[0].frames[0].content["Cards"][0]["name"],
        json!("Opp_Name's Folly")
    );
}

#[test]
fn refuses_when_the_username_survives_glued_to_other_text() {
    for message in ["xOpp_Name casts a spell.", "Opp_Name123 casts a spell."] {
        assert_eq!(
            redact_player(with_message(message), "Opp_Name"),
            Err(RedactError::NameSurvived(LOG_SURVIVED.to_string())),
            "{message}"
        );
    }
}

#[test]
fn does_not_touch_other_snapshot_keys() {
    let snapshot = package_snapshot();

    let redacted = redact_player(snapshot.clone(), "Opp_Name").unwrap();

    assert_eq!(redacted.meta, snapshot.meta);
    assert_eq!(redacted.games[0].won, snapshot.games[0].won);
    assert_eq!(redacted.version, snapshot.version);
}

#[test]
fn redacts_every_game_of_the_match() {
    let mut snapshot = package_snapshot();
    let mut second = snapshot.games[0].clone();
    second.game_number = 2;
    snapshot.games.push(second);

    let redacted = redact_player(snapshot, "Opp_Name").unwrap();

    assert_eq!(
        redacted.games[1].frames[0].content["Players"][1]["Name"],
        json!("Opponent")
    );
    assert_eq!(
        redacted.games[1].log[0].message,
        "Opponent casts Ragavan, Nimble Pilferer."
    );
}

#[test]
fn rejects_an_empty_username() {
    assert_eq!(
        redact_player(package_snapshot(), "  "),
        Err(RedactError::EmptyUsername)
    );
    assert_eq!(
        redact_player(package_snapshot(), "\t\n"),
        Err(RedactError::EmptyUsername)
    );
}

/// The four neighbour conditions of the package's pattern
/// `(?<![A-Za-z0-9_])(?<![A-Za-z0-9_][.\-])USER(?![A-Za-z0-9_])(?![.\-][A-Za-z0-9_])`.
#[test]
fn every_neighbour_condition_of_the_package_pattern_holds() {
    let replaced = [
        ("-Qfoe at the start", "-Opponent at the start"),
        ("Qfoe.-x", "Opponent.-x"),
        ("Qfoe-", "Opponent-"),
        (
            "Qfoe and qfoe and QFOE",
            "Opponent and Opponent and Opponent",
        ),
        ("Qfoe", "Opponent"),
        (" .Qfoe", " .Opponent"),
    ];
    for (message, expected) in replaced {
        assert_eq!(
            redacted_message(message, "Qfoe"),
            Ok(expected.to_string()),
            "{message}"
        );
    }

    let glued = [
        "a.Qfoe is glued through a joiner",
        "a-Qfoe is glued through a joiner",
        "Qfoe.a is glued through a joiner",
        "Qfoe-a is glued through a joiner",
        "QfoeQfoe touches itself",
        "_Qfoe",
        "Qfoe9",
    ];
    for message in glued {
        assert_eq!(
            redacted_message(message, "Qfoe"),
            Err(RedactError::NameSurvived(LOG_SURVIVED.to_string())),
            "{message}"
        );
    }
}

/// PHP's `preg_replace` without `/u` and `stripos` work on bytes and fold
/// ASCII letters only: a non-ASCII letter must match exactly, and a byte of a
/// non-ASCII character is never a word character.
#[test]
fn a_non_ascii_username_is_matched_bytewise_as_php_does() {
    let mut snapshot = with_message("QFOé casts a spell; éQfoé watches.");
    snapshot.games[0].frames[0].content["Players"][1]["Name"] = json!("qfoé");
    snapshot.games[0].log[1].message = "QfoÉ is not the same name.".to_string();

    let redacted = redact_player(snapshot, "Qfoé").unwrap();

    assert_eq!(
        redacted.games[0].log[0].message,
        "Opponent casts a spell; éOpponent watches."
    );
    assert_eq!(
        redacted.games[0].frames[0].content["Players"][1]["Name"],
        json!("Opponent")
    );
    // `É` and `é` differ in bytes, so PHP leaves this line alone and
    // `stripos` does not see the name in it either.
    assert_eq!(
        redacted.games[0].log[1].message,
        "QfoÉ is not the same name."
    );
}

#[test]
fn the_username_is_trimmed_as_php_trims_it() {
    let redacted = redact_player(package_snapshot(), " Opp_Name\n").unwrap();

    assert_eq!(
        redacted.games[0].log[0].message,
        "Opponent casts Ragavan, Nimble Pilferer."
    );
}

#[test]
fn a_player_name_that_only_contains_the_username_is_refused() {
    let mut snapshot = package_snapshot();
    snapshot.games[0].frames[0].content["Players"][1]["Name"] = json!("Opp_Name2");

    assert_eq!(
        redact_player(snapshot.clone(), "Opp_Name"),
        Err(RedactError::NameSurvived(NAME_SURVIVED.to_string()))
    );
    assert_eq!(
        assert_gone(&snapshot, "opp_name"),
        Err(RedactError::NameSurvived(NAME_SURVIVED.to_string()))
    );
    assert_eq!(assert_gone(&snapshot, "Qfoe"), Ok(()));
}
