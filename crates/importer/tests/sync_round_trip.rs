//! Cross-device sync round trips (Plan Q task 11, 0.x `BundleRoundTripTest`):
//! a match or league built from v1's stored rows, read back as a pulled
//! bundle and imported into a fresh database through the 0.x import paths,
//! builds the same bytes again. Fake names start with Q.

use fixture_tools::TestDir;
use importer::bundle_read::{pulled_league, read_match_bundle};
use mymtgo_api::SyncType;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use store::sync::{SyncEdit, edit_sync, league_bundle_input, match_bundle_input};
use sync_bundles::league::{league_bundle, read_league_bundle};
use sync_bundles::match_bundle::match_bundle;
use sync_bundles::{bundle_hash, canonical_json, league_client_id};

const T: i64 = 1_787_133_605_000;
/// 4 of 101 and 4 of 102 main, 2 of 301 in the sideboard.
const SIGNATURE: &str = "MTAxOjQ6ZmFsc2V8MTAyOjQ6ZmFsc2V8MzAxOjI6dHJ1ZQ==";

fn db(dir: &TestDir) -> Connection {
    store::open_writer(&dir.join("mymtgo.sqlite")).expect("open")
}

/// The deck both devices have: "777", one version.
fn deck(conn: &Connection) {
    conn.execute_batch(&format!(
        "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
             cloud_sync_enabled)
         VALUES ('777', 'Qdeck', 'CPAUPER', 1, 1, 'file', 1);
         INSERT INTO deck_versions (deck_id, signature, modified_at)
         VALUES ((SELECT id FROM decks), '{SIGNATURE}', 1);
         INSERT INTO cards (catalog_id, oracle_id) VALUES (101, 'q-oracle-a'),
             (102, 'q-oracle-b'), (301, 'q-oracle-side'), (501, 'q-oracle-foe');"
    ))
    .expect("deck");
}

/// A match `token` of `source` with two games, played with the deck.
fn stored_match(conn: &Connection, token: &str, source: &str) -> i64 {
    conn.execute_batch(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)
             ON CONFLICT (username) DO NOTHING;
         INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0)
             ON CONFLICT (username) DO NOTHING;",
    )
    .expect("registries");
    let mtgo_match_id: Option<i64> = (source != "manual").then_some(285_746_550);
    conn.execute(
        "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source, format,
             match_type, state, outcome, started_at, ended_at, last_activity_at, revision,
             derived_version, deck_version_id, notes)
         VALUES (?1, ?2, (SELECT id FROM accounts), (SELECT id FROM opponents), ?3, 'CPAUPER',
             'Constructed', 'complete', 'win', ?4, ?5, ?5, 1, 1,
             (SELECT id FROM deck_versions), 'Qnote')",
        params![token, mtgo_match_id, source, T, T + 3_000_000],
    )
    .expect("match");
    let match_id = conn.last_insert_rowid();
    for number in [1, 2] {
        let mtgo_game_id: Option<i64> = (source != "manual").then_some(945_500_843 + number);
        conn.execute(
            "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason, on_play,
                 local_mulligans, opp_mulligans, local_die_roll, opp_die_roll, turn_count,
                 started_at, ended_at, local_starting_hand_size, opp_starting_hand_size)
             VALUES (?1, ?2, ?3, ?4, 'unknown', ?5, ?6, 0, NULL, NULL, ?7, ?8, ?9, ?10, NULL)",
            params![
                match_id,
                mtgo_game_id,
                number,
                if number == 1 { "win" } else { "loss" },
                number == 1,
                if number == 1 { 1 } else { 0 },
                (source != "manual").then_some(8 + number),
                T + (number - 1) * 1_000_000,
                T + number * 1_000_000,
                if number == 1 { Some(6) } else { None },
            ],
        )
        .expect("game");
    }
    match_id
}

fn game_id(conn: &Connection, match_id: i64, number: i64) -> i64 {
    conn.query_row(
        "SELECT id FROM games WHERE match_id = ?1 AND game_number = ?2",
        params![match_id, number],
        |row| row.get(0),
    )
    .expect("game id")
}

/// The bundle of a stored match (its archetypes from the rows).
fn built(conn: &Connection, token: &str, carry: Option<&sync_bundles::Carry>) -> Value {
    let match_id: i64 = conn
        .query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
            row.get(0)
        })
        .expect("match");
    let input = match_bundle_input(conn, match_id, chrono_tz::UTC)
        .expect("input")
        .expect("a match");
    match_bundle(&input, carry)
}

/// Pull `bundle` into `conn` as the sync run does.
fn pull(conn: &mut Connection, bundle: &Value) -> sync_bundles::Carry {
    let canonical = canonical_json(bundle);
    let (pulled, carry) = read_match_bundle(bundle).expect("reads");
    let edit = SyncEdit::PulledMatch {
        client_id: pulled.imported.token.clone(),
        pulled: Box::new(pulled),
        hash: bundle_hash(&canonical),
        carry: carry.to_json().to_string(),
        timezone: chrono_tz::UTC,
        now_ms: T + 9_000_000,
    };
    edit_sync(conn, &edit).expect("pulled");
    carry
}

/// Both seats as another device detected (the player) and hand-set (the
/// opponent) them.
fn with_archetypes(mut bundle: Value) -> Value {
    bundle["archetypes"] = json!([
        {"uuid": "aaaaaaaa-0000-4000-8000-000000000000", "confidence": "1.00", "manual": false,
         "player_username": "Qplayer", "player_is_player": true},
        {"uuid": "bbbbbbbb-0000-4000-8000-000000000000", "confidence": "0.50", "manual": true,
         "player_username": "Qfoe", "player_is_player": false},
    ]);
    bundle
}

// 0.x "produces an identical bundle from the imported match".
#[test]
fn produces_an_identical_bundle_from_the_imported_match() {
    let first = TestDir::new("sync-round-trip-log-a");
    let conn = db(&first);
    deck(&conn);
    let match_id = stored_match(&conn, "q-log", "log");
    let one = game_id(&conn, match_id, 1);
    let two = game_id(&conn, match_id, 2);
    conn.execute_batch(&format!(
        "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty, opening_hand,
             starting_hand, seen, `cast`, played, activated)
         VALUES ({one}, 'local', 101, 4, 0, 2, 2, 3, 1, 0, 0),
             ({one}, 'local', 102, 4, 0, 0, 0, 0, 0, 0, 0),
             ({one}, 'local', 301, 0, 2, 0, 0, 0, 0, 0, 0),
             ({one}, 'opponent', 501, 0, 0, 0, 0, 2, 1, 1, 0),
             ({two}, 'local', 101, 4, 0, 1, 1, 1, 0, 0, 0),
             ({two}, 'local', 102, 3, 1, 0, 0, 0, 0, 0, 0),
             ({two}, 'local', 301, 1, 1, 0, 0, 0, 0, 0, 0);
         UPDATE game_cards SET sided_in = 1 WHERE game_id = {two} AND catalog_id = 301;
         UPDATE game_cards SET sided_out = 1 WHERE game_id = {two} AND catalog_id = 102;
         INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
         VALUES ({one}, 'local', 101, 'kicked', 1);"
    ))
    .expect("rows");
    let bundle = with_archetypes(built(&conn, "q-log", None));

    let second = TestDir::new("sync-round-trip-log-b");
    let mut other = db(&second);
    deck(&other);
    let carry = pull(&mut other, &bundle);
    let source: String = other
        .query_row(
            "SELECT source FROM matches WHERE token = 'q-log'",
            [],
            |row| row.get(0),
        )
        .expect("pulled");
    assert_eq!(source, "imported", "a pulled match is not this device's");
    // Archetypes spec section 14: the seats land as rows (unknown uuids as
    // stubs) and the rebuild from rows gives the same bytes.
    let seats: Vec<(bool, String, bool)> = other
        .prepare(
            "SELECT x.is_opponent, a.uuid, x.manual FROM match_archetypes x
             JOIN archetypes a ON a.id = x.archetype_id ORDER BY x.is_opponent",
        )
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(
        seats,
        vec![
            (
                false,
                "aaaaaaaa-0000-4000-8000-000000000000".to_string(),
                false
            ),
            (
                true,
                "bbbbbbbb-0000-4000-8000-000000000000".to_string(),
                true
            ),
        ]
    );
    assert!(carry.is_empty(), "nothing rides in the carry");
    assert_eq!(
        canonical_json(&built(&other, "q-log", None)),
        canonical_json(&bundle)
    );
}

// 0.x BundleRoundTripTest: the golden log match's pulled bytes read into
// both seats, the opponent's by its username.
#[test]
fn reads_the_archetype_seats_of_a_pulled_match_bundle() {
    let golden: Value = serde_json::from_str(include_str!(
        "../../../fixtures/parity/sync_match_bundles.golden.json"
    ))
    .expect("golden");
    let case = &golden["bundles"][0];
    let bundle: Value =
        serde_json::from_str(case["canonical"].as_str().expect("canonical")).expect("bundle");
    let (pulled, _) = read_match_bundle(&bundle).expect("reads");
    let seats: Vec<(String, String, bool, bool)> = pulled
        .archetypes
        .iter()
        .map(|side| {
            (
                side.uuid.clone(),
                format!("{:.2}", side.confidence),
                side.manual,
                side.opponent,
            )
        })
        .collect();
    assert_eq!(
        seats,
        vec![
            (
                "aaaaaaaa-0000-4000-8000-000000000000".to_string(),
                "1.00".to_string(),
                false,
                false
            ),
            (
                "bbbbbbbb-0000-4000-8000-000000000000".to_string(),
                "0.50".to_string(),
                false,
                true
            ),
        ]
    );
    // Without games the username cannot tell: `player_is_player` does.
    let mut bare = bundle.clone();
    bare["players"] = json!([]);
    let (pulled, _) = read_match_bundle(&bare).expect("reads");
    let opponents: Vec<bool> = pulled.archetypes.iter().map(|side| side.opponent).collect();
    assert_eq!(opponents, vec![false, true]);
}

// 0.x "round-trips a manual match with its hand-entered opening hand".
#[test]
fn round_trips_a_manual_match_with_its_hand_entered_opening_hand() {
    let first = TestDir::new("sync-round-trip-manual-a");
    let conn = db(&first);
    deck(&conn);
    let match_id = stored_match(&conn, "q-manual", "manual");
    let one = game_id(&conn, match_id, 1);
    let two = game_id(&conn, match_id, 2);
    conn.execute_batch(&format!(
        "INSERT INTO game_cards (game_id, side, catalog_id, opening_hand, bottomed, starting_hand)
         VALUES ({one}, 'local', 101, 3, 0, 3), ({one}, 'local', 102, 4, 1, 3);
         UPDATE games SET local_opening_hand_size = 7 WHERE id = {one};
         INSERT INTO game_mulligan_cards (game_id, hand, catalog_id, quantity)
         VALUES ({one}, 1, 101, 4), ({one}, 1, 102, 3);
         INSERT INTO game_cards (game_id, side, catalog_id, main_qty)
         VALUES ({two}, 'local', 101, 4), ({two}, 'local', 102, 3), ({two}, 'local', 301, 1);
         INSERT INTO game_cards (game_id, side, catalog_id, seen)
         VALUES ({two}, 'opponent', 501, 2);"
    ))
    .expect("rows");
    let bundle = built(&conn, "q-manual", None);
    assert_eq!(bundle["match"]["manual"], json!(true));
    assert!(bundle["players"][1]["opening_hand_json"].is_object());

    let second = TestDir::new("sync-round-trip-manual-b");
    let mut other = db(&second);
    deck(&other);
    let carry = pull(&mut other, &bundle);
    let source: String = other
        .query_row(
            "SELECT source FROM matches WHERE token = 'q-manual'",
            [],
            |row| row.get(0),
        )
        .expect("pulled");
    assert_eq!(source, "manual");
    assert_eq!(
        canonical_json(&built(&other, "q-manual", Some(&carry))),
        canonical_json(&bundle)
    );
}

// 0.x "imports a limited match with no deck reference".
#[test]
fn imports_a_limited_match_with_no_deck_reference() {
    let first = TestDir::new("sync-round-trip-limited-a");
    let conn = db(&first);
    deck(&conn);
    stored_match(&conn, "q-limited", "log");
    let mut bundle = built(&conn, "q-limited", None);
    bundle["match"]["deck_mtgo_id"] = Value::Null;
    bundle["match"]["deck_version_signature"] = Value::Null;
    bundle["match"]["format"] = json!("DZNR");

    let second = TestDir::new("sync-round-trip-limited-b");
    let mut other = db(&second);
    pull(&mut other, &bundle);
    let (format, version): (String, Option<i64>) = other
        .query_row(
            "SELECT format, deck_version_id FROM matches WHERE token = 'q-limited'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("pulled");
    assert_eq!(format, "DZNR");
    assert_eq!(version, None);
}

// 0.x "marks an imported complete match as submitted so the stats pipeline
// does not re-report it".
#[test]
fn marks_an_imported_complete_match_as_submitted() {
    let first = TestDir::new("sync-round-trip-submitted-a");
    let conn = db(&first);
    deck(&conn);
    stored_match(&conn, "q-done", "log");
    let bundle = built(&conn, "q-done", None);
    let second = TestDir::new("sync-round-trip-submitted-b");
    let mut other = db(&second);
    deck(&other);
    pull(&mut other, &bundle);
    let status: String = other
        .query_row(
            "SELECT status FROM outbox WHERE kind = 'match_report' AND subject = 'q-done'",
            [],
            |row| row.get(0),
        )
        .expect("a report row");
    assert_eq!(status, "sent");
    // Even with a player archetype stored, it is not reported again.
    other
        .execute_batch(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('aaaaaaaa-1111-4111-8111-111111111111', 'Qarchetype', 'CMODERN',
                     'downloaded', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES ((SELECT id FROM matches WHERE token = 'q-done'), 0,
                     last_insert_rowid(), 1.0);",
        )
        .expect("player seat");
    let reportable = store::api_reads::pending_report_count(&other, i64::MAX).expect("count");
    assert_eq!(reportable, 0);
}

// 0.x "imports a pre-version-3 match bundle that carries no manual flag or
// opening hands".
#[test]
fn imports_a_pre_version_3_match_bundle_without_manual_flag_or_hands() {
    let first = TestDir::new("sync-round-trip-v2-a");
    let conn = db(&first);
    deck(&conn);
    stored_match(&conn, "q-old", "log");
    let mut bundle = built(&conn, "q-old", None);
    bundle["match"]
        .as_object_mut()
        .expect("match")
        .remove("manual");
    for player in bundle["players"].as_array_mut().expect("players") {
        player
            .as_object_mut()
            .expect("player")
            .remove("opening_hand_json");
    }
    let second = TestDir::new("sync-round-trip-v2-b");
    let mut other = db(&second);
    pull(&mut other, &bundle);
    let (source, games): (String, i64) = other
        .query_row(
            "SELECT source, (SELECT COUNT(*) FROM games) FROM matches WHERE token = 'q-old'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("pulled");
    assert_eq!((source.as_str(), games), ("imported", 2));
}

// 0.x "round-trips a draft league with picks and snapshots, re-linking the
// snapshot to its match".
#[test]
fn round_trips_a_draft_league_with_picks_and_snapshots() {
    let first = TestDir::new("sync-round-trip-league-a");
    let conn = db(&first);
    deck(&conn);
    stored_match(&conn, "q-draft-match", "log");
    conn.execute_batch(
        "INSERT INTO leagues (run_key, source, kind, state, token, name, format, set_code,
             mtgo_course_id, deck_id, deck_version_id, started_at, created_at, notes)
         VALUES ('q:run', 'log', 'draft', 'complete', 'q-run-token', 'Qleague DSK', 'DDSK',
             'DSK', 4242, (SELECT id FROM decks), (SELECT id FROM deck_versions), 1787133605000,
             1, 'Qnote');
         UPDATE matches SET league_id = (SELECT id FROM leagues);
         INSERT INTO drafts (draft_token, source, league_id, state, mtgo_draft_id, seat_index,
             started_at, ended_at)
         VALUES ('q-draft-token', 'log', (SELECT id FROM leagues), 'finished', 6123456, 3,
             1787133606000, 1787133700000);
         INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number, cards_available,
             reservations, picked_catalog_id, note, note_updated_at, shown_at)
         VALUES ((SELECT id FROM drafts), 1, 1, 1, '[5,6]',
                 '[{\"catalog_id\":6,\"at_ms\":1787133610000}]', 5, 'Qfirst', 1, 1787133609000),
             ((SELECT id FROM drafts), 2, 1, 2, '[7,8]', '[]', 8, NULL, NULL, NULL);
         INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at,
             event_file_id, event_offset)
         SELECT id, 'registered', '[{\"catalog_id\":5,\"quantity\":1,\"sideboard\":false}]',
             'q-sig', 1787133800000, 1, 1 FROM matches;",
    )
    .expect("league");
    let league_id: i64 = conn
        .query_row("SELECT id FROM leagues", [], |row| row.get(0))
        .expect("league id");
    let input = league_bundle_input(&conn, league_id)
        .expect("input")
        .expect("a league");
    let league = league_bundle(&input, None);
    let match_bundle = built(&conn, "q-draft-match", None);

    let second = TestDir::new("sync-round-trip-league-b");
    let mut other = db(&second);
    deck(&other);
    let (read, carry) = read_league_bundle(&league).expect("reads");
    let client_id = league_client_id(&read.token, read.started_at_ms);
    let pulled = pulled_league(&read, &client_id);
    let decks = pulled.decks.clone();
    edit_sync(
        &mut other,
        &SyncEdit::PulledLeague {
            client_id: client_id.clone(),
            pulled: Box::new(pulled),
            hash: bundle_hash(&canonical_json(&league)),
            carry: carry.to_json().to_string(),
            timezone: chrono_tz::UTC,
            now_ms: T,
        },
    )
    .expect("pulled league");
    let picks: Vec<(i64, Option<String>)> = other
        .prepare("SELECT ordinal, note FROM draft_picks ORDER BY ordinal")
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(picks, vec![(1, Some("Qfirst".to_string())), (2, None)]);
    // The snapshot comes once its match is here.
    pull(&mut other, &match_bundle);
    edit_sync(&mut other, &SyncEdit::PulledSnapshots { decks, now_ms: T }).expect("snapshots");
    let snapshot_match: String = other
        .query_row(
            "SELECT m.token FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id",
            [],
            |row| row.get(0),
        )
        .expect("snapshot");
    assert_eq!(snapshot_match, "q-draft-match");
    let stored = store::sync::synced_rows(&other, SyncType::League).expect("rows");
    assert!(stored.contains_key(&client_id));
}

// 0.x "imports a pre-amendment league bundle that carries no child keys".
#[test]
fn imports_a_pre_amendment_league_bundle_without_child_keys() {
    let bundle = json!({"league": {
        "token": "q-old-run", "name": "Qold", "format": "CMODERN",
        "started_at": "2026-08-19T10:00:05Z", "joined_at": null, "dropped_at": null,
        "completed_at": null, "state": "active", "notes": null, "manual": false,
        "deck_change_detected": false, "kind": "constructed", "set_code": null,
        "mtgo_course_id": null, "deck_mtgo_id": null, "deck_version_signature": null,
    }});
    let dir = TestDir::new("sync-round-trip-old-league");
    let mut conn = db(&dir);
    let (read, carry) = read_league_bundle(&bundle).expect("reads");
    let client_id = league_client_id(&read.token, read.started_at_ms);
    edit_sync(
        &mut conn,
        &SyncEdit::PulledLeague {
            client_id,
            pulled: Box::new(pulled_league(&read, "q-old-run_20260819T100005Z")),
            hash: "h".to_string(),
            carry: carry.to_json().to_string(),
            timezone: chrono_tz::UTC,
            now_ms: T,
        },
    )
    .expect("pulled");
    let tokens: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM leagues WHERE token = 'q-old-run'",
            [],
            |row| row.get(0),
        )
        .expect("league");
    assert_eq!(tokens, 1);
}

// Issue #313: a pulled bundle's `imported` flag is the match's exact
// history flag; a bundle without one (never written by 0.x or v1) is a
// tracked match.
#[test]
fn a_pulled_bundle_carries_its_imported_flag() {
    let dir = TestDir::new("sync-round-trip-imported-flag");
    let conn = db(&dir);
    deck(&conn);
    stored_match(&conn, "q-log", "log");
    let mut bundle = built(&conn, "q-log", None);
    let flag = |bundle: &Value| {
        read_match_bundle(bundle)
            .expect("reads")
            .0
            .imported
            .legacy_history
    };
    assert!(!flag(&bundle));
    bundle["match"]["imported"] = json!(true);
    assert!(flag(&bundle));
    bundle["match"]
        .as_object_mut()
        .expect("match")
        .remove("imported");
    assert!(!flag(&bundle));
}

// League self-management spec 6.1 and 6.3: a match the player deleted and
// took out of every league lands so on the other device, and its rebuild
// there gives the same bytes.
#[test]
fn a_deleted_match_round_trips_with_its_player_fields() {
    let first = TestDir::new("sync-round-trip-player-a");
    let conn = db(&first);
    deck(&conn);
    stored_match(&conn, "q-log", "log");
    conn.execute(
        "UPDATE matches SET hidden_at = ?1, no_league = 1, edited_at = ?1 WHERE token = 'q-log'",
        [T + 5],
    )
    .expect("deleted, no league");
    let bundle = built(&conn, "q-log", None);
    assert_eq!(bundle["match"]["hidden_at"], json!(T + 5));
    assert_eq!(bundle["match"]["no_league"], json!(true));

    let second = TestDir::new("sync-round-trip-player-b");
    let mut other = db(&second);
    deck(&other);
    pull(&mut other, &bundle);
    let columns: (Option<i64>, bool, Option<i64>) = other
        .query_row(
            "SELECT hidden_at, no_league, edited_at FROM matches WHERE token = 'q-log'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("pulled");
    assert_eq!(columns, (Some(T + 5), true, Some(T + 5)));
    assert_eq!(
        canonical_json(&built(&other, "q-log", None)),
        canonical_json(&bundle)
    );
}

// ---- A manual Endstep match across devices (cross-device sync spec 5.3) ----

/// This device's catalog id of the Endstep card `name`, made when new.
fn endstep_card(conn: &mut Connection, name: &str) -> u32 {
    let tx = conn.transaction().expect("tx");
    let id = store::endstep_cards::catalog_for_ref(
        &tx,
        &sync_bundles::EndstepCardRef {
            name: name.to_string(),
            set: "qqq".to_string(),
            number: "7".to_string(),
        },
        1,
    )
    .expect("a card");
    tx.commit().expect("commit");
    id
}

/// An Endstep account and its deck `endstep:qd`: 4 Qalpha and 4 Qbeta main,
/// 2 Qside in the sideboard. Returns the three catalog ids.
fn endstep_deck(conn: &mut Connection) -> (u32, u32, u32) {
    let alpha = endstep_card(conn, "Qalpha");
    let beta = endstep_card(conn, "Qbeta");
    let side = endstep_card(conn, "Qside");
    let card = |catalog_id: u32, quantity: u32, sideboard: bool| decks::DeckCard {
        catalog_id,
        quantity,
        sideboard,
    };
    let signature = decks::signature(&[
        card(alpha, 4, false),
        card(beta, 4, false),
        card(side, 2, true),
    ]);
    conn.execute_batch(
        "INSERT INTO accounts (username, first_seen_at, platform)
         VALUES ('endstep:qu1', 0, 'endstep');
         INSERT INTO opponents (username, first_seen_at, platform)
         VALUES ('endstep:qu2', 0, 'endstep');
         INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
             cloud_sync_enabled, account_id)
         VALUES ('endstep:qd', 'Qdeck', 'CMODERN', 1, 1, 'file', 1,
             (SELECT id FROM accounts));",
    )
    .expect("deck");
    conn.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at)
         VALUES ((SELECT id FROM decks), ?1, 1)",
        [signature],
    )
    .expect("version");
    (alpha, beta, side)
}

/// Every game row of the match `token` with its cards named, so two
/// devices with different Endstep ids compare.
fn named_rows(conn: &Connection, token: &str) -> Vec<String> {
    // `cards::ENDSTEP_ID_OFFSET` (the importer does not depend on `cards`).
    let offset = 200_000_000;
    let mut rows = Vec::new();
    for sql in [
        format!(
            "SELECT 'card ' || g.game_number || ':' || c.side || ':' || e.name || ':'
                 || c.main_qty || ':' || c.side_qty || ':' || c.opening_hand || ':'
                 || c.bottomed || ':' || c.starting_hand || ':' || c.seen || ':'
                 || c.sided_in || ':' || c.sided_out
             FROM game_cards c JOIN games g ON g.id = c.game_id
             JOIN matches m ON m.id = g.match_id
             JOIN endstep_cards e ON {offset} + e.id = c.catalog_id
             WHERE m.token = ?1"
        ),
        format!(
            "SELECT 'mulligan ' || g.game_number || ':' || u.hand || ':' || e.name || ':'
                 || u.quantity
             FROM game_mulligan_cards u JOIN games g ON g.id = u.game_id
             JOIN matches m ON m.id = g.match_id
             JOIN endstep_cards e ON {offset} + e.id = u.catalog_id
             WHERE m.token = ?1"
        ),
        "SELECT 'game ' || g.game_number || ':' || quote(g.local_opening_hand_size) || ':'
             || quote(g.local_starting_hand_size)
         FROM games g JOIN matches m ON m.id = g.match_id WHERE m.token = ?1"
            .to_string(),
    ] {
        let mut statement = conn.prepare(&sql).expect("prepare");
        let found = statement
            .query_map([token], |row| row.get::<_, String>(0))
            .expect("query");
        for row in found {
            rows.push(row.expect("row"));
        }
    }
    rows.sort();
    rows
}

#[test]
fn a_pulled_manual_endstep_match_keeps_its_detail() {
    let first = TestDir::new("sync-round-trip-endstep-manual-a");
    let mut conn = db(&first);
    let (alpha, beta, side) = endstep_deck(&mut conn);
    conn.execute(
        "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type, state,
             outcome, started_at, ended_at, last_activity_at, revision, derived_version,
             deck_version_id)
         VALUES ('q-es-manual', (SELECT id FROM accounts), (SELECT id FROM opponents), 'manual',
             'CMODERN', 'Constructed', 'complete', 'win', ?1, ?2, ?2, 1, 1,
             (SELECT id FROM deck_versions))",
        params![T, T + 3_000_000],
    )
    .expect("match");
    let match_id = conn.last_insert_rowid();
    for number in [1, 2] {
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                 local_mulligans, opp_mulligans, started_at, ended_at)
             VALUES (?1, ?2, 'win', 'unknown', 1, ?3, 0, ?4, ?5)",
            params![
                match_id,
                number,
                if number == 1 { 1 } else { 0 },
                T + (number - 1) * 1_000_000,
                T + number * 1_000_000,
            ],
        )
        .expect("game");
    }
    let one = game_id(&conn, match_id, 1);
    let two = game_id(&conn, match_id, 2);
    // A Qother card only the opponent showed, made last so its id differs.
    let other = endstep_card(&mut conn, "Qother");
    conn.execute_batch(&format!(
        "INSERT INTO game_cards (game_id, side, catalog_id, opening_hand, bottomed, starting_hand)
         VALUES ({one}, 'local', {alpha}, 3, 0, 3), ({one}, 'local', {beta}, 4, 1, 3);
         UPDATE games SET local_opening_hand_size = 7, local_starting_hand_size = 6
             WHERE id = {one};
         INSERT INTO game_mulligan_cards (game_id, hand, catalog_id, quantity)
         VALUES ({one}, 1, {alpha}, 4), ({one}, 1, {beta}, 3);
         INSERT INTO game_cards (game_id, side, catalog_id, main_qty)
         VALUES ({two}, 'local', {alpha}, 4), ({two}, 'local', {beta}, 3),
             ({two}, 'local', {side}, 1);
         INSERT INTO game_cards (game_id, side, catalog_id, seen)
         VALUES ({two}, 'opponent', {other}, 2);"
    ))
    .expect("rows");
    let bundle = built(&conn, "q-es-manual", None);
    assert_eq!(bundle["platform"], json!("endstep"));
    assert_eq!(bundle["match"]["manual"], json!(true));
    let sent = named_rows(&conn, "q-es-manual");
    assert!(
        sent.iter().any(|row| row.starts_with("mulligan")),
        "{sent:?}"
    );
    assert!(
        sent.iter().any(|row| row.contains(":opponent:Qother:")),
        "{sent:?}"
    );

    let second = TestDir::new("sync-round-trip-endstep-manual-b");
    let mut other_device = db(&second);
    // Cards in another order here, so every catalog id differs from A's.
    endstep_card(&mut other_device, "Qother");
    endstep_card(&mut other_device, "Qunrelated");
    other_device
        .execute_batch(
            "INSERT INTO accounts (username, first_seen_at, platform)
             VALUES ('endstep:qu1', 0, 'endstep');
             INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
                 account_id)
             VALUES ('endstep:qd', 'Qdeck', 'CMODERN', 1, 1, 'imported',
                 (SELECT id FROM accounts));",
        )
        .expect("the deck here");
    let carry = pull(&mut other_device, &bundle);
    assert_eq!(named_rows(&other_device, "q-es-manual"), sent);
    assert_eq!(
        canonical_json(&built(&other_device, "q-es-manual", Some(&carry))),
        canonical_json(&bundle),
        "the pulled match builds the same bytes"
    );
}
