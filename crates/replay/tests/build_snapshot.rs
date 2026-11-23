//! Package `tests/Unit/BuildReplaySnapshotTest.php`, ported.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;

use common::*;
use replay::{CardInfo, LogEntry, Meta, ReplayFrame, SideboardEntry, build_snapshot, format_key};
use serde_json::{Value, json};

#[test]
fn builds_a_version_2_snapshot_with_head_metadata() {
    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![row()], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    assert_eq!(snapshot.version, 2);
    assert_eq!(
        snapshot.meta,
        Meta {
            format: Some("modern".to_string()),
            played_at: "2026-09-20T18:00:00+00:00".to_string(),
            games_in_match: 1,
            local_archetype: Some("Boros Energy".to_string()),
            opponent_archetype: None,
        }
    );
}

#[test]
fn marks_the_local_player_and_fills_card_fields_in_every_frame() {
    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![row()], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );
    let content = &snapshot.games[0].frames[0].content;

    assert_eq!(content["Players"][0]["IsLocal"], json!(true));
    assert_eq!(content["Players"][1]["IsLocal"], json!(false));
    assert_eq!(
        content["Cards"][0]["name"],
        json!("Ragavan, Nimble Pilferer")
    );
    assert_eq!(content["Cards"][0]["type"], json!("Legendary Creature"));
    assert_eq!(
        content["Cards"][0]["image"],
        json!("https://cards.scryfall.io/a.jpg")
    );
}

#[test]
fn drops_an_image_that_is_not_a_remote_https_url() {
    let frame = timeline_row(5678, "2026-09-20T18:00:01+00:00");
    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![frame], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );
    let card = first_card(&snapshot.games[0].frames[0]);

    assert_eq!(card["image"], Value::Null);
    assert_eq!(card["name"], json!("Island"));
}

#[test]
fn leaves_unknown_cards_with_null_fields() {
    let frame = timeline_row(999, "2026-09-20T18:00:01+00:00");
    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![frame], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );
    let card = first_card(&snapshot.games[0].frames[0])
        .as_object()
        .unwrap();

    assert_eq!(card.get("name"), Some(&Value::Null));
    assert_eq!(card.get("type"), Some(&Value::Null));
    assert_eq!(card.get("image"), Some(&Value::Null));
}

#[test]
fn names_a_card_the_catalog_cannot_resolve_with_the_name_mtgo_gave_it() {
    let mut frame = timeline_row(999, "2026-09-20T18:00:01+00:00");
    frame.content["Cards"][0]["Name"] = json!("Eldrazi Spawn");

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![frame], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );
    let card = first_card(&snapshot.games[0].frames[0])
        .as_object()
        .unwrap();

    assert_eq!(card.get("name"), Some(&json!("Eldrazi Spawn")));
    assert!(!card.contains_key("Name"));
}

#[test]
fn keeps_a_hidden_card_nameless_even_when_mtgo_sent_a_name() {
    let mut frame = timeline_row(0, "2026-09-20T18:00:01+00:00");
    frame.content["Cards"][0]["Name"] = json!("Ragavan, Nimble Pilferer");

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![frame], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    assert_eq!(
        first_card(&snapshot.games[0].frames[0])["name"],
        Value::Null
    );
}

#[test]
fn skips_frameless_games_but_keeps_their_place_in_the_numbering() {
    let snapshot = build_snapshot(
        match_input(
            vec![
                game_input(vec![], Some(false), vec![]),
                game_input(vec![row()], Some(true), vec![]),
            ],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    // Two games played, only game 2 has frames, so it keeps number 2.
    assert_eq!(snapshot.games.len(), 1);
    assert_eq!(snapshot.games[0].game_number, 2);
    assert_eq!(snapshot.games[0].won, Some(true));
    assert_eq!(snapshot.meta.games_in_match, 2);
}

#[test]
fn passes_the_log_through_untouched() {
    let log = vec![LogEntry {
        timestamp: "18:00:01".to_string(),
        message: "Opp_Name casts Ragavan, Nimble Pilferer.".to_string(),
    }];
    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![row()], Some(true), log.clone())],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    assert_eq!(snapshot.games[0].log, log);
}

#[test]
fn asks_the_resolver_once_per_game_with_each_catalog_id_once() {
    let calls: RefCell<Vec<Vec<u32>>> = RefCell::new(Vec::new());
    let resolver = |ids: &[u32]| {
        calls.borrow_mut().push(ids.to_vec());
        BTreeMap::new()
    };
    let frames = vec![
        timeline_row(1234, "2026-09-20T18:00:01+00:00"),
        timeline_row(1234, "2026-09-20T18:00:01+00:00"),
        timeline_row(5678, "2026-09-20T18:00:01+00:00"),
    ];

    build_snapshot(
        match_input(
            vec![game_input(frames, Some(true), vec![])],
            Some("CMODERN"),
        ),
        &resolver,
    );

    assert_eq!(calls.into_inner(), vec![vec![1234, 5678]]);
}

#[test]
fn reduces_format_codes_to_the_archetype_key() {
    let cases = [
        ("constructed code", Some("CMODERN"), Some("modern")),
        ("mixed case code", Some("CModern"), Some("modern")),
        ("bare name", Some("Modern"), Some("modern")),
        (
            "commander is a name, not a code",
            Some("Commander"),
            Some("commander"),
        ),
        ("null", None, None),
    ];

    for (label, code, expected) in cases {
        assert_eq!(format_key(code), expected.map(str::to_string), "{label}");
    }
}

#[test]
fn writes_a_null_format_when_there_is_none() {
    let snapshot = build_snapshot(
        match_input(vec![game_input(vec![row()], Some(true), vec![])], None),
        &card_resolver,
    );

    assert_eq!(snapshot.meta.format, None);
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(
        json["meta"].as_object().unwrap().get("format"),
        Some(&Value::Null)
    );
}

#[test]
fn carries_your_sideboard_as_the_game_began_with_card_fields_filled() {
    let mut game = game_input(vec![row()], Some(true), vec![]);
    game.sideboard = vec![(1234, 2), (5678, 1)];

    let snapshot = build_snapshot(match_input(vec![game], Some("CMODERN")), &card_resolver);

    assert_eq!(
        snapshot.games[0].sideboard,
        Some(vec![
            SideboardEntry {
                catalog_id: 1234,
                quantity: 2,
                name: Some("Ragavan, Nimble Pilferer".to_string()),
                type_line: Some("Legendary Creature".to_string()),
                image: Some("https://cards.scryfall.io/a.jpg".to_string()),
            },
            SideboardEntry {
                catalog_id: 5678,
                quantity: 1,
                name: Some("Island".to_string()),
                type_line: Some("Basic Land".to_string()),
                image: None,
            },
        ])
    );
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(
        json["games"][0]["sideboard"][1],
        json!({"catalog_id": 5678, "quantity": 1, "name": "Island", "type": "Basic Land", "image": null})
    );
}

#[test]
fn leaves_the_sideboard_out_when_the_game_has_none_recorded() {
    let snapshot = build_snapshot(
        match_input(
            vec![
                game_input(vec![row()], Some(true), vec![]),
                game_input(vec![row()], Some(true), vec![]),
            ],
            Some("CMODERN"),
        ),
        &card_resolver,
    );

    let json = serde_json::to_value(&snapshot).unwrap();
    assert!(
        !json["games"][0]
            .as_object()
            .unwrap()
            .contains_key("sideboard")
    );
    assert!(
        !json["games"][1]
            .as_object()
            .unwrap()
            .contains_key("sideboard")
    );
}

#[test]
fn resolves_sideboard_cards_the_timeline_never_shows() {
    let asked: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    let resolver = |ids: &[u32]| {
        *asked.borrow_mut() = ids.to_vec();
        BTreeMap::new()
    };
    let mut game = game_input(vec![row()], Some(true), vec![]);
    game.sideboard = vec![(999, 1)];

    build_snapshot(match_input(vec![game], Some("CMODERN")), &resolver);

    let asked = asked.into_inner();
    assert!(asked.contains(&999));
    assert!(asked.contains(&1234));
}

fn meadow(other_image: &str) -> CardInfo {
    CardInfo {
        other_image: Some(other_image.to_string()),
        ..card(
            "Witch-Blessed Meadow",
            "Land",
            "https://cards.scryfall.io/back.jpg",
        )
    }
}

#[test]
fn carries_the_other_face_of_a_double_faced_card_when_the_resolver_knows_it() {
    let resolver =
        |_: &[u32]| BTreeMap::from([(4321, meadow("https://cards.scryfall.io/front.jpg"))]);
    let frame = timeline_row(4321, "2026-09-20T18:00:01+00:00");

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![frame], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &resolver,
    );

    assert_eq!(
        first_card(&snapshot.games[0].frames[0])["other_image"],
        json!("https://cards.scryfall.io/front.jpg")
    );
}

#[test]
fn leaves_other_image_off_single_faced_cards_and_drops_one_that_is_not_remote() {
    let resolver = |_: &[u32]| {
        BTreeMap::from([
            (
                1234,
                card(
                    "Ragavan, Nimble Pilferer",
                    "Legendary Creature",
                    "https://cards.scryfall.io/a.jpg",
                ),
            ),
            (4321, meadow("http://127.0.0.1:8100/front.jpg")),
        ])
    };
    let frames = vec![
        timeline_row(1234, "2026-09-20T18:00:01+00:00"),
        timeline_row(4321, "2026-09-20T18:00:01+00:00"),
    ];

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(frames, Some(true), vec![])],
            Some("CMODERN"),
        ),
        &resolver,
    );

    for frame in &snapshot.games[0].frames {
        assert!(
            !first_card(frame)
                .as_object()
                .unwrap()
                .contains_key("other_image")
        );
    }
}

/// Plan V: every key MTGO or 0.x's dense recorder wrote reaches the viewer,
/// in the snapshot and in the app viewer's frames.
#[test]
fn a_frame_keeps_every_key_mtgo_wrote() {
    let content = json!({
        "Turn": 3, "Phase": "Combat", "Step": "DeclareAttackers", "ActivePlayer": 1,
        "Priority": 2, "Unknown": {"Nested": [1, 2]},
        "Players": [
            {"Id": 1, "Name": "local.player", "Life": 18, "HandCount": 4, "LibraryCount": 50,
             "TimeLeft": 1500, "Pool": "RR"},
            {"Id": 2, "Name": "Opp_Name", "Life": 20, "HandCount": 6, "LibraryCount": 51,
             "TimeLeft": 1490},
        ],
        "Cards": [{"Id": 10, "CatalogID": 1234, "Zone": "Battlefield", "ActualZone": "Battlefield",
                   "Owner": 1, "Controller": 1, "Tapped": true, "Power": 1, "Toughness": 1,
                   "Counters": [{"Kind": "+1/+1", "Count": 1}], "Attacking": true,
                   "Blocking": false, "Damage": 0}],
    });
    let frame = ReplayFrame {
        timestamp: "18:00:01".to_string(),
        content: content.clone(),
    };

    let snapshot = build_snapshot(
        match_input(
            vec![game_input(vec![frame.clone()], Some(true), vec![])],
            Some("CMODERN"),
        ),
        &card_resolver,
    );
    let app = replay::decorate_frames(
        vec![frame],
        "local.player",
        &card_resolver(&[]),
        replay::ImageMode::App,
    );

    for written in [&snapshot.games[0].frames[0].content, &app[0].content] {
        for (key, value) in content.as_object().unwrap() {
            if key == "Players" || key == "Cards" {
                continue;
            }
            assert_eq!(written.get(key), Some(value), "{key}");
        }
        for (index, player) in content["Players"].as_array().unwrap().iter().enumerate() {
            for (key, value) in player.as_object().unwrap() {
                assert_eq!(
                    written["Players"][index].get(key),
                    Some(value),
                    "player {key}"
                );
            }
        }
        for (key, value) in content["Cards"][0].as_object().unwrap() {
            assert_eq!(written["Cards"][0].get(key), Some(value), "card {key}");
        }
    }
}
