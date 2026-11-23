//! Trimming the committed ranked Modern Bo3 capture to its one match: the
//! trimmed units fold, log and replay as the whole units do, and keep no
//! line of another match sharing a unit.

use endstep_match::{ReplayNames, TrimmedUnit, fold_match, game_log, match_record, trim_units};
use endstep_protocol::{Unit, decode_unit};

const UNIT_01: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/01-session-a7a2056a.jsonl");
const UNIT_02: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/02-match-329f3d80.jsonl");
const UNIT_03: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/03-session-e3f486f1.jsonl");
const UNIT_04: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/04-match-946164b1.jsonl");
const UNIT_05: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/05-session-1a930f54.jsonl");

const FORGE_GAME: &str = "00000000-0000-4000-8000-e0000000003e";
/// Far after every frame, so the fold's quiet rule never changes anything.
const NOW_MS: i64 = 1_900_000_000_000;

/// Every id of the other match, none of which may survive the trim.
const OTHER_IDS: [&str; 5] = [
    "q-other-game",
    "q-other-match",
    "q-other-lobby",
    "q-other-deck",
    "q-other-db",
];

/// A second match in the same units: its REST bodies (match, lobby, deck,
/// ranked result) and game socket frames (a state, a delta after it, an
/// event and an outgoing action after that).
fn other_match_lines() -> Vec<String> {
    let lines = [
        r#"{"at":1794310900001,"src":"rest","dir":"in","url":"/api/matches/q-other-match","body":{"id":"q-other-match","forgeGameId":"q-other-game","lobbyId":"q-other-lobby","dbId":"q-other-db","formatId":"Modern","myPlayerIndex":0,"participants":[{"userId":"00000000-0000-4000-8000-00000000000a","username":"Player"},{"userId":"00000000-0000-4000-8000-00000000000d","username":"Person9"}]}}"#,
        r#"{"at":1794310900002,"src":"rest","dir":"in","url":"/api/lobbies/q-other-lobby","body":{"id":"q-other-lobby","config":{"seatDecks":{"00000000-0000-4000-8000-00000000000a":"q-other-deck"}}}}"#,
        r#"{"at":1794310900003,"src":"rest","dir":"in","url":"/api/decks/q-other-deck","body":{"id":"q-other-deck","name":"Qother","cards":[{"name":"Island","quantity":60}]}}"#,
        r#"{"at":1794310900004,"src":"rest","dir":"in","url":"/api/ranked/results/q-other-db","body":{"status":"done","you":{"ratingBefore":1,"ratingAfter":2}}}"#,
        r#"{"at":1794310900005,"src":"game_ws","dir":"in","url":"wss://gw/q-other-game","body":{"type":"GAME_STATE","seq":990001,"payload":{"gameId":"q-other-game","turnNumber":1}}}"#,
        r#"{"at":1794310900006,"src":"game_ws","dir":"in","url":"wss://gw/q-other-game","body":{"type":"GAME_DELTA","seq":990002,"payload":{"patch":{"note":"q-other-game delta"}}}}"#,
        r#"{"at":1794310900007,"src":"game_ws","dir":"in","url":"wss://gw/q-other-game","body":{"type":"GAME_EVENT","seq":990003,"payload":{"gameId":"q-other-game","type":"TURN_PHASE","phase":"MAIN1"}}}"#,
        r#"{"at":1794310900008,"src":"game_ws","dir":"out","url":"wss://gw/q-other-game","body":{"type":"GAME_ACTION","payload":{"type":"KEEP_HAND","matchId":"q-other-match"}}}"#,
    ];
    lines.iter().map(|line| line.to_string()).collect()
}

/// `text` with the other match's lines put in before line `before` (the
/// header is line 0).
fn with_other_match(text: &str, before: usize) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (offset, line) in other_match_lines().into_iter().enumerate() {
        lines.insert(before + offset, line);
    }
    lines.join("\n") + "\n"
}

/// The first line from `from` on that is a state or event of this match:
/// the other match's lines go in before it, so the frames after them are
/// tied to this match's game again at once.
fn our_state_line(text: &str, from: usize) -> usize {
    text.lines()
        .enumerate()
        .skip(from)
        .find(|(_, line)| {
            line.contains(FORGE_GAME)
                && (line.contains("\"GAME_STATE\"") || line.contains("\"GAME_EVENT\""))
        })
        .map(|(index, _)| index)
        .expect("a state of this match")
}

/// The five fixture units, with the other match in units 01 (a session
/// unit) and 04 (a match unit).
fn shared_texts() -> Vec<String> {
    vec![
        with_other_match(UNIT_01, 1),
        UNIT_02.to_string(),
        UNIT_03.to_string(),
        with_other_match(UNIT_04, our_state_line(UNIT_04, 300)),
        UNIT_05.to_string(),
    ]
}

fn fixture_texts() -> Vec<String> {
    [UNIT_01, UNIT_02, UNIT_03, UNIT_04, UNIT_05]
        .iter()
        .map(|text| text.to_string())
        .collect()
}

fn decoded(texts: &[String]) -> Vec<Unit> {
    texts
        .iter()
        .map(|text| decode_unit(text).expect("unit decodes"))
        .collect()
}

/// The trimmed units as unit text again, decoded.
fn trimmed_units(trimmed: &[TrimmedUnit]) -> Vec<Unit> {
    trimmed
        .iter()
        .map(|unit| decode_unit(&unit.lines.join("\n")).expect("a trimmed unit decodes"))
        .collect()
}

/// Folding, the game log of every game and the replay record agree.
fn assert_same_match(trimmed: &[Unit], whole: &[Unit]) {
    assert_eq!(
        fold_match(FORGE_GAME, trimmed, NOW_MS),
        fold_match(FORGE_GAME, whole, NOW_MS)
    );
    for game_number in 1..=3 {
        let log = game_log(FORGE_GAME, whole, game_number);
        assert!(!log.is_empty(), "game {game_number} has a log");
        assert_eq!(game_log(FORGE_GAME, trimmed, game_number), log);
    }
    let names = ReplayNames::default();
    let record = match_record(FORGE_GAME, whole, &names);
    assert!(record.is_some());
    assert_eq!(match_record(FORGE_GAME, trimmed, &names), record);
}

#[test]
fn trimmed_units_fold_as_the_whole_units() {
    let texts = fixture_texts();
    let trimmed = trim_units(FORGE_GAME, &texts);
    assert_same_match(&trimmed_units(&trimmed), &decoded(&texts));
}

#[test]
fn another_match_in_shared_units_is_trimmed_away() {
    let shared = shared_texts();
    let trimmed = trim_units(FORGE_GAME, &shared);
    for unit in &trimmed {
        for line in &unit.lines {
            for id in OTHER_IDS {
                assert!(!line.contains(id), "{id} kept in {}: {line}", unit.unit_id);
            }
        }
    }
    // Folded as the whole shared units, and as the fixture without the
    // other match.
    let units = trimmed_units(&trimmed);
    assert_same_match(&units, &decoded(&shared));
    assert_same_match(&units, &decoded(&fixture_texts()));
}

#[test]
fn each_kept_unit_starts_with_its_header_and_keeps_line_order() {
    let texts = fixture_texts();
    let trimmed = trim_units(FORGE_GAME, &texts);
    let ids: Vec<&str> = trimmed.iter().map(|unit| unit.unit_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "a7a2056a-63ea-47e9-a198-028e1ebf8532",
            "329f3d80-56d9-4d83-a482-3c888f11fe9f",
            "e3f486f1-ab7a-411a-971c-671216111d2a",
            "946164b1-d1e2-4f5f-a509-d3569c3543ba",
        ],
        "unit 05 holds nothing of the match and is left out"
    );
    for (unit, text) in trimmed.iter().zip(&texts) {
        assert_eq!(unit.lines[0], text.lines().next().expect("a header"));
        // Every kept line is a line of the unit, in the unit's order.
        let mut source = text.lines();
        for line in &unit.lines {
            assert!(
                source.any(|candidate| candidate == line),
                "a kept line out of order in {}",
                unit.unit_id
            );
        }
    }
    // Lines the fold never reads (the site's queue and account bodies)
    // are not kept.
    let session = &trimmed[0].lines;
    assert!(!session.iter().any(|line| line.contains("/api/queue")));
    assert!(!session.iter().any(|line| line.contains("/api/auth/me")));
}

#[test]
fn a_unit_that_does_not_decode_is_left_out() {
    let texts = vec!["not a capture".to_string(), UNIT_02.to_string()];
    let trimmed = trim_units(FORGE_GAME, &texts);
    assert_eq!(trimmed.len(), 1);
    assert_eq!(trimmed[0].unit_id, "329f3d80-56d9-4d83-a482-3c888f11fe9f");
}

#[test]
fn a_malformed_line_is_dropped_without_shifting_the_lines_after_it() {
    let before = our_state_line(UNIT_04, 300);
    let mut lines: Vec<&str> = UNIT_04.lines().collect();
    lines.insert(before, "not json at all");
    lines.insert(before + 1, "   ");
    lines.insert(before + 2, "{\"at\":1,\"src\":\"game_ws\"");
    let broken = lines.join("\n") + "\n";
    let texts = vec![
        UNIT_01.to_string(),
        UNIT_02.to_string(),
        UNIT_03.to_string(),
        broken,
        UNIT_05.to_string(),
    ];
    let trimmed = trim_units(FORGE_GAME, &texts);
    let unit_04 = &trimmed[3];
    assert!(!unit_04.lines.iter().any(|line| line.contains("not json")));
    assert!(!unit_04.lines.iter().any(|line| line.trim().is_empty()));
    // Every kept line is what the fixture's own trim kept, in order.
    let plain = trim_units(FORGE_GAME, &fixture_texts());
    assert_eq!(unit_04.lines, plain[3].lines);
    assert_same_match(&trimmed_units(&trimmed), &decoded(&fixture_texts()));
}
