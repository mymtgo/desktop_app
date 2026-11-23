//! What turns on `CARD_STATS_LIVE` (cards spec section 11): a live game's
//! card stats payload, from the log through the engine, the card
//! resolution and Q8's read, against the payload 0.x builds for the same
//! game (`fixtures/parity/card_stats_draft_hobbit.golden.json`, exported by
//! 0.x's `ExportParityGoldensTest` "exports the card stats golden").

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use common::*;
use fixture_tools::{Fixture, TestDir, load_fixture};
use mtgo_match::ABANDON_AFTER_MS;
use mymtgo_api::{CardStatsGame, card_stats_payload};
use serde_json::Value;
use store::api_reads::card_stats_candidates;
use store::{Connection, open_reader, open_writer};
use tracker::{Engine, TrackerConfig};

/// The constructed match of `draft_hobbit`: three games, sideboarding.
const MATCH: &str = "c0d8ff76-dc13-480b-bdf0-a228a6cd20f8";

fn golden() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/parity/card_stats_draft_hobbit.golden.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("golden json")
}

/// The fixture read by the engine, its games synced, and the constructed
/// match linked to a deck version (Q8 ships only deck-linked matches).
fn live(label: &str) -> (TestDir, TrackerConfig, Engine, i64) {
    let fixture: Fixture = load_fixture("draft_hobbit");
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
    let mut engine = open(&config, now);
    let (_, now) = run_until_settled(&mut engine, now);
    let conn = open_writer(&config.db_path).expect("writer");
    conn.execute(
        "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
         VALUES ('q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file')",
        [],
    )
    .expect("deck");
    let version: i64 = conn
        .query_row(
            "INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES ((SELECT id FROM decks WHERE mtgo_id = 'q-deck'), 'Q', 0) RETURNING id",
            [],
            |row| row.get(0),
        )
        .expect("version");
    conn.execute(
        "UPDATE matches SET deck_version_id = ?1 WHERE token = ?2",
        store::params![version, MATCH],
    )
    .expect("link");
    seat_archetypes(&conn, &golden());
    drop(conn);
    (dir, config, engine, now)
}

/// The match's archetype rows: the ones 0.x's payload names (fake uuids),
/// the player's a downloaded archetype and the opponent's the baseline's
/// fallback seed when it is one.
fn seat_archetypes(conn: &Connection, golden: &Value) {
    let payload = &golden["games"][0]["payload"];
    for (key, is_opponent) in [
        ("player_archetype_uuid", false),
        ("opponent_archetype_uuid", true),
    ] {
        let Some(uuid) = payload[key].as_str() else {
            continue;
        };
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, 'Qarchetype', 'CMODERN', 'downloaded', 0, 0)
             ON CONFLICT (uuid) DO NOTHING",
            [uuid],
        )
        .expect("archetype");
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES ((SELECT id FROM matches WHERE token = ?1), ?2,
                 (SELECT id FROM archetypes WHERE uuid = ?3), 1.0)",
            store::params![MATCH, is_opponent, uuid],
        )
        .expect("seat");
    }
}

/// The constructed match's games as Q8 reads them for queueing, by MTGO
/// game id.
fn games(conn: &Connection) -> BTreeMap<i64, CardStatsGame> {
    card_stats_candidates(conn, 500, i64::MAX)
        .expect("candidates")
        .into_iter()
        .filter(|game| game.match_token == MATCH)
        .map(|game| (game.mtgo_game_id, game))
        .collect()
}

/// An answer for every card the games name: `q-oracle-{id}`, except that
/// 38315 is another printing of 126126 (the golden's `oracle_rule`).
fn answers(conn: &Connection) -> store::cards::CardsUpdate {
    let mut stmt = conn
        .prepare("SELECT catalog_id FROM cards ORDER BY catalog_id")
        .expect("ids");
    let ids: Vec<u32> = stmt
        .query_map([], |row| row.get(0))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    let oracle = |id: u32| if id == 38_315 { 126_126 } else { id };
    let resolved = ids
        .iter()
        .map(|&id| mymtgo_api::Resolution {
            mtgo_id: id,
            card: mymtgo_api::CardAnswer {
                scryfall_id: Some(format!("q-scryfall-{id}")),
                oracle_id: Some(format!("q-oracle-{}", oracle(id))),
                name: Some(format!("Qcard {}", oracle(id))),
                ..mymtgo_api::CardAnswer::default()
            },
            is_token: false,
        })
        .collect();
    store::cards::CardsUpdate {
        tokens: Vec::new(),
        resolutions: mymtgo_api::Resolutions {
            resolved,
            unknown: Vec::new(),
        },
        unknown_at_ms: 0,
    }
}

/// Resolve every card the way the cards worker's answer is applied, then
/// let the re-syncs finish.
fn resolve(config: &TrackerConfig, engine: &mut Engine, now: i64) {
    let update = answers(&open_reader(&config.db_path).expect("reader"));
    assert!(!update.resolutions.resolved.is_empty());
    engine.resolve_cards(update).expect("resolved");
    run_until_settled(engine, now + 10_000);
}

// Readiness 1: population runs without sign-in, so a live game's cards
// resolve; before that, the payload leaves every card out (no oracle id) and
// the game is skipped; after it, every card carries its oracle id.
#[test]
fn a_live_game_payload_has_oracle_ids_once_cards_resolve() {
    let (_dir, config, mut engine, now) = live("card-stats-readiness-oracle");
    let before = games(&open_reader(&config.db_path).expect("reader"));
    assert_eq!(before.len(), 3, "the three games are candidates");
    for game in before.values() {
        assert_eq!(
            mymtgo_api::card_stats_skip_reason(game),
            Some("no player-side card stats"),
            "no card has an oracle id yet"
        );
    }

    resolve(&config, &mut engine, now);
    let after = games(&open_reader(&config.db_path).expect("reader"));
    assert_eq!(after.len(), 3);
    for game in after.values() {
        let payload = card_stats_payload(game).expect("a payload once cards resolve");
        let cards = payload["cards"].as_array().expect("cards");
        assert!(!cards.is_empty());
        for card in cards {
            let oracle = card["oracle_id"].as_str().expect("an oracle id");
            assert!(oracle.starts_with("q-oracle-"), "{oracle}");
        }
    }
}

/// The payload's cards by oracle id (0.x sends one row per oracle id), and
/// the oracle ids sent more than once.
fn by_oracle(payload: &Value) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut cards = BTreeMap::new();
    let mut twice = Vec::new();
    for card in payload["cards"].as_array().expect("cards") {
        let oracle = card["oracle_id"].as_str().expect("oracle").to_string();
        if cards.insert(oracle.clone(), card.clone()).is_some() {
            twice.push(oracle);
        }
    }
    (cards, twice)
}

/// The golden's fixture name in `fixtures/parity/allowlist.json`.
const FIXTURE: &str = "card_stats_draft_hobbit";

fn allowlist() -> Vec<Value> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/parity/allowlist.json");
    let text = std::fs::read_to_string(&path).expect("the allowlist");
    let list: Value = serde_json::from_str(&text).expect("the allowlist is JSON");
    list.as_array().expect("a list").clone()
}

/// The golden's allowlisted differences, as `(game id, field)`.
fn allowed() -> BTreeSet<(String, String)> {
    allowlist()
        .iter()
        .filter(|entry| entry["fixture"] == FIXTURE)
        .map(|entry| {
            let text = |key: &str| entry[key].as_str().expect(key).to_string();
            assert!(!text("reason").is_empty(), "every entry has a reason");
            (text("token"), text("field"))
        })
        .collect()
}

/// Every difference between 0.x's payloads and v1's, as `(game id, field)`
/// with a readable line.
fn differences(
    golden: &Value,
    games: &BTreeMap<i64, CardStatsGame>,
) -> Vec<((String, String), String)> {
    let mut found = Vec::new();
    for expected in golden["games"].as_array().expect("games") {
        let id = expected["mtgo_id"].as_i64().expect("game id");
        let token = id.to_string();
        let game = games.get(&id).unwrap_or_else(|| panic!("game {id}"));
        let payload = card_stats_payload(game).expect("payload");
        let want = &expected["payload"];
        let mut differ = |field: String, line: String| {
            found.push(((token.clone(), field), format!("game {id} {line}")));
        };
        for (key, value) in want.as_object().expect("payload object") {
            if key != "cards" && &payload[key] != value {
                differ(
                    key.clone(),
                    format!("{key}: 0.x {value}, v1 {}", payload[key]),
                );
            }
        }
        let (theirs, _) = by_oracle(want);
        let (ours, twice) = by_oracle(&payload);
        for oracle in twice {
            differ(
                format!("cards[{oracle}] (twice)"),
                format!("{oracle}: sent more than once by v1"),
            );
        }
        for (oracle, card) in &theirs {
            let Some(mine) = ours.get(oracle) else {
                differ(
                    format!("cards[{oracle}] (only in 0.x)"),
                    format!("{oracle}: only in 0.x"),
                );
                continue;
            };
            for (field, value) in card.as_object().expect("card") {
                if &mine[field] != value {
                    differ(
                        format!("cards[{oracle}].{field}"),
                        format!("{oracle}.{field}: 0.x {value}, v1 {}", mine[field]),
                    );
                }
            }
        }
        for oracle in ours.keys() {
            if !theirs.contains_key(oracle) {
                differ(
                    format!("cards[{oracle}] (only in v1)"),
                    format!("{oracle}: only in v1"),
                );
            }
        }
    }
    found
}

// Readiness 2: every counter 0.x sends, field by field, for the same live
// game (three games, sideboarding, two printings of one card). Every other
// difference is fixed or justified in `fixtures/parity/allowlist.json`
// (fixture `card_stats_draft_hobbit`), and an entry that no longer occurs
// fails; no zone counter is allowlisted, so they must be equal.
#[test]
fn the_card_stats_payload_matches_the_0x_golden() {
    let golden = golden();
    let (_dir, config, mut engine, now) = live("card-stats-readiness-golden");
    resolve(&config, &mut engine, now);
    let games = games(&open_reader(&config.db_path).expect("reader"));

    let allowed = allowed();
    let found = differences(&golden, &games);
    let unexplained: Vec<&String> = found
        .iter()
        .filter(|(key, _)| !allowed.contains(key))
        .map(|(_, line)| line)
        .collect();
    assert!(
        unexplained.is_empty(),
        "differences from 0.x that are not allowlisted:\n{}",
        unexplained
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let keys: BTreeSet<(String, String)> = found.into_iter().map(|(key, _)| key).collect();
    let stale: Vec<&(String, String)> = allowed.difference(&keys).collect();
    assert!(
        stale.is_empty(),
        "allowlist entries that no longer occur: {stale:?}"
    );
    for (_, field) in &allowed {
        for zone in [
            "hand_seen",
            "graveyard_seen",
            "exile_seen",
            "battlefield_seen",
            "discarded",
            "cast_turn",
            "has_zone_data",
        ] {
            assert!(
                !field.ends_with(&format!(".{zone}")),
                "zone counters must be equal"
            );
        }
    }
}

// The deviations of cards spec section 15 that this golden cannot see are
// recorded as notes (fixture `card_stats`).
#[test]
fn the_card_stats_deviations_are_noted_in_the_allowlist() {
    let notes: Vec<String> = allowlist()
        .iter()
        .filter(|entry| entry["fixture"] == "card_stats")
        .map(|entry| entry["field"].as_str().expect("field").to_string())
        .collect();
    for field in [
        "timeframe applied",
        "unresolved cards shown",
        "imported postboard rule",
        "community stats not ported",
        "0.x image files copied, never moved",
        "seen (sideboard card postboard)",
        "zone counters cap",
    ] {
        assert!(notes.iter().any(|note| note == field), "{field}");
    }
}
