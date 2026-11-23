//! Detection against a real database: the 0.x detection tests (survey
//! section 12) and Plan T's Review Focus cases. `FakeLookups` stands in for
//! the API where the request itself does not matter; the stub server is used
//! where it does (what was sent, nothing sent offline, the pace).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use archetypes::{Definition, DefinitionCard};
use fixture_tools::TestDir;
use mymtgo_api::{ArchetypeEstimate, EstimateCard, EstimateCardId, EstimateOutcome};
use store::Connection;
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, edit_archetypes};
use store::archetypes::DeckFilter;

use super::*;
use crate::api::session::testing::{TestApi, device_api};
use crate::api::stub::{Recorded, Reply, Stub};
use crate::archetypes::definitions::tests::{DEFINITION, FakeClock, NOW, pacer};
use crate::archetypes::definitions::{CheckReply, DefinitionReply, Pacer};
use mymtgo_api::archetypes::DEFINITIONS_CHECK_PATH as CHECK_PATH;

fn mtgo_card(id: u32, quantity: u32) -> EstimateCard {
    EstimateCard {
        id: EstimateCardId::Mtgo(id),
        quantity,
    }
}

/// The asked cards as `(id, copies)`, whichever platform the id is from;
/// cards asked by name are left out (see `FakeLookups::asked`).
fn pairs_of(cards: &[EstimateCard]) -> Vec<(u32, u32)> {
    cards
        .iter()
        .filter_map(|card| match card.id {
            EstimateCardId::Mtgo(id) | EstimateCardId::Arena(id) => Some((id, card.quantity)),
            EstimateCardId::Name(_) => None,
        })
        .collect()
}

/// Writes through `edit_archetypes` on its own connection, as the
/// tracker's writer does.
struct ConnWriter(Connection);

impl Writer for ConnWriter {
    fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
        edit_archetypes(&mut self.0, &edit).map_err(|error| error.to_string())
    }
}

/// The API as a test wants it, recording what was asked.
#[derive(Default)]
struct FakeLookups {
    offline: bool,
    /// The estimate's archetype; `None` is the server's "no archetype".
    answer: Option<ArchetypeEstimate>,
    /// The estimate fails (a 500, a dead token, no connection).
    fails: bool,
    /// The estimate fails only for cards holding this catalog id.
    fails_for: Option<u32>,
    /// The estimate refuses the cards (a 422).
    refuses: bool,
    /// A definition not in `definitions` fails instead of being gone.
    definitions_fail: bool,
    definitions: HashMap<String, Definition>,
    estimates: Vec<(String, Vec<(u32, u32)>)>,
    /// The same asks as the typed cards the API would be sent.
    asked: Vec<Vec<EstimateCard>>,
    definitions_asked: Vec<String>,
    /// The check's answer; without one, the `definitions` asked about
    /// are `changed`.
    check_answer: Option<archetypes::DefinitionsCheck>,
    /// The check answers 404.
    check_missing: bool,
    checks: Vec<Vec<(String, Option<String>)>>,
}

impl Lookups for FakeLookups {
    fn api_allowed(&self) -> bool {
        !self.offline
    }

    fn estimate(&mut self, format: &str, cards: &[EstimateCard]) -> EstimateOutcome {
        let pairs = pairs_of(cards);
        self.estimates.push((format.to_string(), pairs.clone()));
        self.asked.push(cards.to_vec());
        let fails_here = self
            .fails_for
            .is_some_and(|id| pairs.iter().any(|(card, _)| *card == id));
        if self.fails || fails_here {
            return EstimateOutcome::Failed("status 500".to_string());
        }
        if self.refuses {
            return EstimateOutcome::Refused("status 422".to_string());
        }
        match &self.answer {
            Some(answer) => EstimateOutcome::Answered(answer.clone()),
            None => EstimateOutcome::NoArchetype,
        }
    }

    fn definition(&mut self, uuid: &str) -> DefinitionReply {
        self.definitions_asked.push(uuid.to_string());
        match self.definitions.get(uuid) {
            Some(definition) => DefinitionReply::Found(Box::new(definition.clone())),
            None if self.definitions_fail => DefinitionReply::Failed,
            None => DefinitionReply::Gone,
        }
    }

    fn check(&mut self, versions: &[(String, Option<String>)]) -> CheckReply {
        self.checks.push(versions.to_vec());
        if self.check_missing {
            return CheckReply::NotDeployed;
        }
        if let Some(answer) = &self.check_answer {
            return CheckReply::Answer(answer.clone());
        }
        let changed = versions
            .iter()
            .filter_map(|(uuid, _)| self.definitions.get(uuid).cloned())
            .collect();
        CheckReply::Answer(archetypes::DefinitionsCheck {
            changed,
            ..archetypes::DefinitionsCheck::default()
        })
    }
}

fn estimate(uuid: &str, confidence: f64) -> Option<ArchetypeEstimate> {
    Some(ArchetypeEstimate {
        uuid: uuid.to_string(),
        confidence,
        deck_version_uuid: None,
    })
}

struct Db {
    _dir: TestDir,
    writer: ConnWriter,
    reader: Connection,
    /// The detector's clock, `NOW` until a test moves it.
    clock: std::rc::Rc<std::cell::Cell<i64>>,
    now: Box<dyn Fn() -> i64>,
    failed: FailedEstimates,
}

impl Db {
    fn new(label: &str) -> Db {
        let db = Db::open(TestDir::new(label));
        db.sql(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);",
        );
        db
    }

    /// The database `mymtgo.sqlite` in `dir`, as it is.
    fn open(dir: TestDir) -> Db {
        let path = dir.join("mymtgo.sqlite");
        let writer = store::open_writer(&path).expect("writer");
        let reader = store::open_reader(&path).expect("reader");
        let clock = std::rc::Rc::new(std::cell::Cell::new(NOW));
        let read = std::rc::Rc::clone(&clock);
        Db {
            _dir: dir,
            writer: ConnWriter(writer),
            reader,
            clock,
            now: Box::new(move || read.get()),
            failed: FailedEstimates::default(),
        }
    }

    fn sql(&self, sql: &str) {
        self.writer.0.execute_batch(sql).expect(sql);
    }

    fn number(&self, sql: &str) -> i64 {
        self.reader.query_row(sql, [], |row| row.get(0)).expect(sql)
    }

    /// A card row: `kind` as its type, colour red.
    fn card(&self, catalog_id: i64, oracle: &str, kind: &str) {
        self.sql(&format!(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
             VALUES ({catalog_id}, '{oracle}', 'Q{oracle}', '{kind}', 'R')"
        ));
    }

    /// Cards 100 to 109, each its own oracle id.
    fn ten_cards(&self) {
        for id in 100..110 {
            self.card(id, &format!("q-oracle-{id}"), "Creature");
        }
    }

    fn archetype(&self, uuid: &str, format: &str, source: &str) -> i64 {
        let definition = if source == "downloaded" {
            "'{}'"
        } else {
            "NULL"
        };
        self.sql(&format!(
            "INSERT INTO archetypes (uuid, name, format, source, definition, created_at, updated_at)
             VALUES ('{uuid}', 'Q{uuid}', '{format}', '{source}', {definition}, 1, 1)"
        ));
        self.number(&format!("SELECT id FROM archetypes WHERE uuid = '{uuid}'"))
    }

    /// List 1 of an archetype: four copies of each card.
    fn list(&self, archetype_id: i64, cards: &[i64]) {
        for card in cards {
            self.sql(&format!(
                "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
                 VALUES ({archetype_id}, 1, {card}, 4, 0)"
            ));
        }
    }

    /// A complete match in `format`, its game sync written.
    fn complete_match(&self, id: i64, format: &str) {
        self.sql(&format!(
            "INSERT INTO matches (id, token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, last_activity_at, revision, derived_version,
                 games_synced_at)
             VALUES ({id}, 'Q-match-{id}', 1, 1, 'log', '{format}', 'League', 'complete', 'win',
                 {id}, {id}, 1, 0, 1)"
        ));
    }

    /// A game: the local deck (`(catalog id, main, side)`) and the
    /// opponent's cards seen (`(catalog id, seen)`).
    fn game(&self, match_id: i64, number: i64, local: &[(i64, i64, i64)], seen: &[(i64, i64)]) {
        let game_id = match_id * 10 + number;
        self.sql(&format!(
            "INSERT INTO games (id, match_id, game_number, result, end_reason)
             VALUES ({game_id}, {match_id}, {number}, 'win', 'win')"
        ));
        for (card, main, side) in local {
            self.sql(&format!(
                "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty)
                 VALUES ({game_id}, 'local', {card}, {main}, {side})"
            ));
        }
        for (card, copies) in seen {
            self.sql(&format!(
                "INSERT INTO game_cards (game_id, side, catalog_id, seen)
                 VALUES ({game_id}, 'opponent', {card}, {copies})"
            ));
        }
    }

    /// A deck in this format with one version linked to the match.
    fn deck_for(&self, deck_id: i64, match_id: i64, archetype_id: Option<i64>) {
        let archetype = archetype_id.map_or("NULL".to_string(), |id| id.to_string());
        self.sql(&format!(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source,
                 archetype_id)
             VALUES ({deck_id}, 'Q-deck-{deck_id}', 'Qdeck', 'CMODERN', 1, 1, 'file', {archetype});
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES ({deck_id} * 10, {deck_id}, 'Q-sig-{deck_id}', 1);
             UPDATE matches SET deck_version_id = {deck_id} * 10 WHERE id = {match_id};"
        ));
    }

    fn seat(&self, match_id: i64, is_opponent: bool, archetype_id: i64, manual: bool) {
        self.sql(&format!(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES ({match_id}, {}, {archetype_id}, {}, {})",
            i64::from(is_opponent),
            if manual { "1.0" } else { "0.4" },
            i64::from(manual)
        ));
    }

    /// `(match, is_opponent, archetype, confidence, manual)` by match and seat.
    fn rows(&self) -> Vec<(i64, i64, i64, f64, i64)> {
        let mut stmt = self
            .reader
            .prepare(
                "SELECT match_id, is_opponent, archetype_id, confidence, manual
                 FROM match_archetypes ORDER BY match_id, is_opponent",
            )
            .expect("prepare");
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    fn marker(&self, match_id: i64) -> Option<i64> {
        self.reader
            .query_row(
                "SELECT archetypes_detected_at FROM matches WHERE id = ?1",
                [match_id],
                |row| row.get(0),
            )
            .expect("marker")
    }

    fn homebrew(&self) -> i64 {
        self.number("SELECT id FROM archetypes WHERE name = 'Homebrew'")
    }

    fn detector<'a>(&'a mut self, lookups: &'a mut dyn Lookups) -> Detector<'a> {
        Detector {
            reader: &self.reader,
            lookups,
            writer: &mut self.writer,
            now_ms: &*self.now,
            failed: &mut self.failed,
            logs_only: false,
        }
    }

    /// "Detect" on these matches.
    fn detect(&mut self, lookups: &mut dyn Lookups, match_ids: &[i64]) -> u32 {
        self.detector(lookups)
            .detect_matches(match_ids, &|| false)
            .expect("detect")
    }
}

/// A Modern match whose opponent showed 4 of cards 100 to 103.
fn opponent_plays_four(db: &Db, match_id: i64) {
    db.complete_match(match_id, "CMODERN");
    db.game(match_id, 1, &[], &[(100, 4), (101, 4), (102, 4), (103, 4)]);
}

/// A stub API: the estimate answers `estimate`, the check `check` (a 404
/// when `None`), the decklist `decklist`.
fn stub(estimate: &str, check: Option<String>, decklist: &str) -> Stub {
    let estimate = estimate.to_string();
    let decklist = decklist.to_string();
    Stub::start(move |request: &Recorded| {
        if request.target == "/api/archetypes/estimate" {
            Reply::json(200, &estimate)
        } else if request.target == CHECK_PATH {
            match &check {
                Some(body) => Reply::json(200, body),
                None => Reply::json(404, r#"{"message":"Not Found"}"#),
            }
        } else if request.target.ends_with("/decklist") {
            Reply::json(200, &decklist)
        } else {
            Reply::json(404, "{}")
        }
    })
}

fn targets(stub: &Stub) -> Vec<String> {
    stub.requests()
        .into_iter()
        .map(|request| request.target)
        .collect()
}

/// Detect with the real API client against the stub.
fn detect_with_api(db: &mut Db, test_api: &TestApi, pacer: &mut Pacer, match_ids: &[i64]) -> u32 {
    let mut lookups = ApiLookups::new(&test_api.state, pacer);
    db.detect(&mut lookups, match_ids)
}

// ---- 0.x DetermineMatchArchetypesManualTest ----

#[test]
fn preserves_a_manual_opponent_archetype_through_detection() {
    let mut db = Db::new("detect-manual-kept");
    let picked = db.archetype("q-picked", "modern", "manual");
    opponent_plays_four(&db, 1);
    db.seat(1, true, picked, true);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert_eq!(db.rows(), [(1, 1, picked, 1.0, 1)]);
    assert!(lookups.estimates.is_empty());
}

#[test]
fn recomputes_the_player_seat_when_only_the_opponent_is_manual() {
    let mut db = Db::new("detect-manual-player");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &[100]);
    let picked = db.archetype("q-picked", "modern", "manual");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[(100, 4, 0)], &[]);
    db.seat(1, true, picked, true);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert_eq!(db.rows(), [(1, 0, local, 1.0, 0), (1, 1, picked, 1.0, 1)]);
}

#[test]
fn recreates_a_detected_row_with_a_fresh_archetype() {
    let mut db = Db::new("detect-recreate");
    db.ten_cards();
    let fresh = db.archetype("q-fresh", "modern", "downloaded");
    db.list(fresh, &[100, 101, 102, 103]);
    let stale = db.archetype("q-stale", "modern", "downloaded");
    opponent_plays_four(&db, 1);
    db.seat(1, true, stale, false);
    db.detect(&mut FakeLookups::default(), &[1]);
    assert_eq!(db.rows(), [(1, 1, fresh, 1.0, 0)]);
}

// ---- 0.x DetermineMatchArchetypesMergedTest ----

#[test]
fn writes_the_parent_when_detection_names_a_merged_source() {
    let mut db = Db::new("detect-merged");
    db.ten_cards();
    let parent = db.archetype("q-parent", "modern", "downloaded");
    let source = db.archetype("q-source", "modern", "downloaded");
    db.list(source, &[100, 101, 102, 103]);
    db.sql(&format!(
        "UPDATE archetypes SET merged_into_id = {parent} WHERE id = {source}"
    ));
    opponent_plays_four(&db, 1);
    db.detect(&mut FakeLookups::default(), &[1]);
    assert_eq!(db.rows(), [(1, 1, parent, 1.0, 0)]);
}

#[test]
fn never_rewrites_older_rows_that_point_at_a_merged_source() {
    let mut db = Db::new("detect-merged-history");
    let parent = db.archetype("q-parent", "modern", "downloaded");
    let source = db.archetype("q-source", "modern", "downloaded");
    db.sql(&format!(
        "UPDATE archetypes SET merged_into_id = {parent} WHERE id = {source}"
    ));
    db.complete_match(1, "CMODERN");
    db.seat(1, true, source, false);
    db.sql("UPDATE matches SET archetypes_detected_at = 5 WHERE id = 1");
    db.complete_match(2, "CMODERN");
    db.game(2, 1, &[], &[]);
    db.detect(&mut FakeLookups::default(), &[2]);
    assert_eq!(db.rows(), [(1, 1, source, 0.4, 0)]);
}

// ---- 0.x FallbackAutoAssignTest ----

#[test]
fn gives_the_opponent_homebrew_when_local_and_api_detection_both_fail() {
    let mut db = Db::new("detect-homebrew");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[(999_999, 4)]);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    let homebrew = db.homebrew();
    assert_eq!(db.rows(), [(1, 1, homebrew, 0.0, 0)]);
    assert_eq!(lookups.estimates.len(), 1, "the API was asked first");
}

#[test]
fn an_arena_match_asks_by_arena_id() {
    let mut db = Db::new("detect-arena-ids");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[(100_087_654, 2), (126_519, 4)]);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert_eq!(
        lookups.asked,
        [vec![
            EstimateCard {
                id: EstimateCardId::Mtgo(126_519),
                quantity: 4
            },
            EstimateCard {
                id: EstimateCardId::Arena(87_654),
                quantity: 2
            },
        ]]
    );
}

/// The catalog id of `endstep_cards` row `row`.
fn endstep(row: u32) -> i64 {
    i64::from(cards::endstep_catalog_id(row))
}

impl Db {
    /// An `endstep_cards` row (its printing set `q<row>`) and its `cards`
    /// stub, as an Endstep sync writes them.
    fn endstep_card(&self, row: u32, name: &str) {
        self.sql(&format!(
            "INSERT INTO endstep_cards (id, name, set_code, first_seen_at)
                 VALUES ({row}, '{name}', 'q{row}', 1);
             INSERT INTO cards (catalog_id, name) VALUES ({}, '{name}');",
            endstep(row)
        ));
    }

    /// A downloaded Modern archetype whose one rule is `card` in the
    /// mainboard.
    fn rule_archetype(&mut self, uuid: &str, card: &str) -> i64 {
        let json = format!(
            r#"{{"uuid":"{uuid}","name":"Q{uuid}","format":"modern","color_identity":"R",
            "version":"q-v1","rank":[2,1,4,-17,0,0,0],
            "conditions":[{{"type":"InMainboard","cards":["{card}"]}}],"fallback":false,
            "common_cards":[],"min_overlap":0.1,"colour_fork":"41:Base",
            "cards":[{{"mtgo_id":501,"oracle_id":"q-oracle-a","name":"{card}","quantity":4,"sideboard":false}}]}}"#
        );
        let definition = archetypes::parse_definition(&json).expect("definition");
        self.writer
            .edit(ArchetypeEdit::EnsureDownloaded {
                definition: Box::new(definition),
                format: "modern".to_string(),
                now_ms: 1,
            })
            .expect("downloaded");
        self.number(&format!("SELECT id FROM archetypes WHERE uuid = '{uuid}'"))
    }
}

// Endstep plan 3: an Endstep card goes by its `endstep_cards` name, as
// `{"name", "quantity"}`, never by an id.
#[test]
fn endstep_cards_are_estimated_by_name() {
    let mut db = Db::new("detect-endstep-names");
    db.endstep_card(5, "Qend");
    db.complete_match(1, "Modern");
    db.game(1, 1, &[], &[(endstep(5), 4)]);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    let bodies: Vec<String> = stub
        .requests()
        .into_iter()
        .filter(|request| request.target == "/api/archetypes/estimate")
        .map(|request| String::from_utf8(request.body).expect("utf8"))
        .collect();
    assert_eq!(
        bodies,
        [r#"{"format":"Modern","cards":[{"name":"Qend","quantity":4}]}"#]
    );
    assert_eq!(db.marker(1), Some(NOW));
}

#[test]
fn two_printings_add_up_by_name() {
    let mut db = Db::new("detect-endstep-printings");
    db.endstep_card(5, "Qend");
    db.endstep_card(6, "Qend");
    db.endstep_card(8, "Qother");
    db.complete_match(1, "Modern");
    // Row 7 has no `endstep_cards` row: it names no card.
    db.game(
        1,
        1,
        &[],
        &[
            (126_519, 4),
            (endstep(5), 2),
            (endstep(6), 1),
            (endstep(7), 4),
            (endstep(8), 3),
        ],
    );
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    let named = |name: &str, quantity| EstimateCard {
        id: EstimateCardId::Name(name.to_string()),
        quantity,
    };
    assert_eq!(
        lookups.asked,
        [vec![
            mtgo_card(126_519, 4),
            named("Qend", 3),
            named("Qother", 3),
        ]]
    );
}

// A refused estimate (a 422) of an Endstep deck is no archetype: the local
// rules answer stands.
#[test]
fn a_rejected_estimate_keeps_the_local_answer() {
    let mut db = Db::new("detect-endstep-refused");
    db.endstep_card(5, "Qend");
    let rule = db.rule_archetype("q-rule", "Qend");
    db.complete_match(1, "Modern");
    db.game(1, 1, &[(endstep(5), 4, 0)], &[]);
    let stub = Stub::start(|request: &Recorded| {
        if request.target == "/api/archetypes/estimate" {
            Reply::json(422, r#"{"message":"The given data was invalid."}"#)
        } else {
            Reply::json(404, "{}")
        }
    });
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(targets(&stub), ["/api/archetypes/estimate"]);
    assert_eq!(db.rows(), [(1, 0, rule, 1.0, 0)]);
    assert_eq!(db.marker(1), Some(NOW));
}

/// The fixture Endstep match synced by the tracker into a new database.
fn synced_endstep_fixture(label: &str) -> Db {
    let dir = TestDir::new(label);
    crate::endstep_seed::sync_endstep_fixture(&dir.join("mymtgo.sqlite"), &dir, NOW);
    Db::open(dir)
}

// After the Endstep sync writes its game rows the match is due, and a run
// stores the local rules answer for it, its cards asked by name only.
#[test]
fn an_endstep_match_is_detected() {
    let mut db = synced_endstep_fixture("detect-endstep-fixture");
    let match_id = db.number("SELECT id FROM matches");
    assert_eq!(
        store::archetypes::due_matches(&db.reader, 10).expect("due"),
        [match_id]
    );
    let rule = db.rule_archetype("q-tron", "Urza's Tower");
    let mut lookups = FakeLookups::default();
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.marker(match_id), Some(NOW));
    let player: Vec<(i64, f64)> = db
        .rows()
        .into_iter()
        .filter(|row| row.1 == 0)
        .map(|row| (row.2, row.3))
        .collect();
    assert_eq!(player, [(rule, 1.0)]);
    assert!(!lookups.asked.is_empty());
    for cards in &lookups.asked {
        assert!(
            cards
                .iter()
                .all(|card| matches!(card.id, EstimateCardId::Name(_))),
            "only names are asked: {cards:?}"
        );
    }
}

#[test]
fn never_gives_the_player_homebrew() {
    let mut db = Db::new("detect-no-player-homebrew");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[(999_999, 4, 0)], &[]);
    db.detect(&mut FakeLookups::default(), &[1]);
    assert!(db.rows().is_empty());
    assert_eq!(db.marker(1), Some(NOW), "detected all the same");
}

// "does not dispatch DownloadArchetypeDecklists for fallback archetypes",
// read as "fallbacks fetch nothing".
#[test]
fn fallbacks_fetch_nothing() {
    let mut db = Db::new("detect-fallback-fetch");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[(999_999, 4)]);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert!(lookups.definitions_asked.is_empty());
}

#[test]
fn rerunning_detection_keeps_one_fallback_row_per_opponent() {
    let mut db = Db::new("detect-fallback-idempotent");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[(999_999, 4)]);
    db.detect(&mut FakeLookups::default(), &[1]);
    db.detect(&mut FakeLookups::default(), &[1]);
    assert_eq!(db.rows().len(), 1);
}

// ---- 0.x AggregateOpponentCardsTest ----

#[test]
fn sums_opponent_copies_across_games_and_caps_at_four() {
    let mut db = Db::new("detect-aggregate");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[(999, 4, 0)], &[(101, 3), (102, 1)]);
    db.game(1, 2, &[(999, 4, 0)], &[(101, 3), (102, 1)]);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    let opponent: Vec<&(String, Vec<(u32, u32)>)> = lookups
        .estimates
        .iter()
        .filter(|(_, cards)| !cards.contains(&(999, 4)))
        .collect();
    assert_eq!(
        opponent,
        [&("CMODERN".to_string(), vec![(101, 4), (102, 2)])],
        "the raw match format, summed and capped, the local deck apart"
    );
}

#[test]
fn no_opponent_cards_give_no_opponent_row_and_no_request() {
    let mut db = Db::new("detect-aggregate-empty");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[]);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert!(db.rows().is_empty());
    assert!(lookups.estimates.is_empty());
}

// ---- 0.x DetermineMatchArchetypesLimitedTest ----

fn detectable_match(db: &Db, format: &str) {
    db.ten_cards();
    let archetype = db.archetype("q-detectable", "modern", "downloaded");
    db.list(archetype, &[100, 101, 102, 103]);
    db.complete_match(1, format);
    db.game(1, 1, &[], &[(100, 4), (101, 4), (102, 4), (103, 4)]);
}

#[test]
fn skips_detection_for_a_limited_match() {
    let mut db = Db::new("detect-limited");
    detectable_match(&db, "DHOBHOBHOB");
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert!(db.rows().is_empty());
    assert_eq!(db.marker(1), Some(NOW), "marked detected without rows");
    assert!(lookups.estimates.is_empty());
}

#[test]
fn still_detects_a_constructed_match() {
    let mut db = Db::new("detect-constructed");
    detectable_match(&db, "CMODERN");
    db.detect(&mut FakeLookups::default(), &[1]);
    assert_eq!(db.rows().len(), 1);
}

// ---- 0.x DetermineDeckArchetypeTest, against the stub ----

/// A Modern match whose player deck is `cards` (four of each), the API
/// answering `estimate_body`.
fn player_match(db: &Db, cards: &[i64]) {
    db.complete_match(1, "CMODERN");
    let local: Vec<(i64, i64, i64)> = cards.iter().map(|card| (*card, 4, 0)).collect();
    db.game(1, 1, &local, &[]);
}

fn player_row(db: &Db) -> Option<(i64, f64)> {
    db.rows()
        .into_iter()
        .find(|row| row.1 == 0)
        .map(|row| (row.2, row.3))
}

// #489: online the API is asked first; without its answer the local match
// above the threshold stands.
#[test]
fn uses_the_local_match_above_the_threshold_when_the_api_has_none() {
    let mut db = Db::new("deck-detect-local");
    db.ten_cards();
    let burn = db.archetype("q-burn", "modern", "downloaded");
    db.list(burn, &[100, 101, 102, 103]);
    player_match(&db, &[100, 101, 102, 103]);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), Some((burn, 1.0)));
    assert_eq!(targets(&stub).len(), 1, "the estimate was asked");
}

#[test]
fn asks_the_api_when_the_local_confidence_is_too_low() {
    let mut db = Db::new("deck-detect-low");
    db.ten_cards();
    let burn = db.archetype("api-burn-uuid", "modern", "downloaded");
    db.list(burn, &[100]);
    player_match(&db, &[100, 200, 201, 202, 203, 204, 205, 206, 207, 208]);
    let stub = stub(
        r#"[{"uuid":"api-burn-uuid","confidence":0.85}]"#,
        None,
        "{}",
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), Some((burn, 0.85)));
    assert_eq!(targets(&stub), ["/api/archetypes/estimate"]);
}

// "resolves archetype_deck_id when API returns a known deck_version_uuid":
// v1 keeps no sample decks (ruling 9), so the held archetype is the answer.
#[test]
fn an_api_answer_with_a_known_deck_version_uses_the_held_archetype() {
    let mut db = Db::new("deck-detect-variant");
    let variant = db.archetype("api-uuid-variant", "modern", "downloaded");
    player_match(&db, &[999]);
    let stub = stub(
        r#"[{"uuid":"api-uuid-variant","confidence":0.92,"deck_version_uuid":"deck-version-uuid-xyz"}]"#,
        None,
        "{}",
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), Some((variant, 0.92)));
}

#[test]
fn an_api_answer_with_an_unknown_deck_version_still_counts() {
    let mut db = Db::new("deck-detect-unknown-variant");
    let held = db.archetype("api-uuid-unknown-variant", "modern", "downloaded");
    player_match(&db, &[999]);
    let stub = stub(
        r#"[{"uuid":"api-uuid-unknown-variant","confidence":0.8,"deck_version_uuid":"unknown-deck-uuid"}]"#,
        None,
        "{}",
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), Some((held, 0.8)));
}

#[test]
fn an_unknown_api_uuid_is_downloaded_instead_of_dropped() {
    let mut db = Db::new("deck-detect-new-uuid");
    player_match(&db, &[999]);
    let stub = stub(
        r#"[{"uuid":"q-new","confidence":0.9}]"#,
        Some(format!(
            r#"{{"changed":[{DEFINITION}],"moved":[],"retired":[]}}"#
        )),
        "{}",
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    let created = db.number("SELECT id FROM archetypes WHERE uuid = 'q-new'");
    assert_eq!(
        db.number("SELECT name = 'Qnew Storm' FROM archetypes WHERE uuid = 'q-new'"),
        1
    );
    assert_eq!(player_row(&db), Some((created, 0.9)));
}

#[test]
fn an_api_uuid_the_server_retired_gives_no_answer() {
    let mut db = Db::new("deck-detect-ghost");
    player_match(&db, &[999]);
    let stub = stub(
        r#"[{"uuid":"ghost-uuid","confidence":0.9}]"#,
        Some(r#"{"changed":[],"moved":[],"retired":["ghost-uuid"]}"#.to_string()),
        "{}",
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), None);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE uuid = 'ghost-uuid'"),
        0
    );
}

#[test]
fn asks_the_api_when_no_local_archetypes_exist() {
    let mut db = Db::new("deck-detect-no-local");
    let held = db.archetype("api-uuid", "modern", "downloaded");
    player_match(&db, &[100]);
    let stub = stub(r#"[{"uuid":"api-uuid","confidence":0.9}]"#, None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), Some((held, 0.9)));
    assert_eq!(targets(&stub), ["/api/archetypes/estimate"]);
}

// "returns archetype_deck_id when local estimation hits", as amended by
// #489: online the API is asked first; without its answer the local hit
// stands.
#[test]
fn a_local_hit_online_still_asks_the_api() {
    let mut db = Db::new("deck-detect-local-hit");
    db.card(1, "a", "Creature");
    let archetype = db.archetype("q-local", "modern", "downloaded");
    db.list(archetype, &[1]);
    player_match(&db, &[1]);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(player_row(&db), Some((archetype, 1.0)));
    assert_eq!(targets(&stub).len(), 1, "the estimate was asked");
}

#[test]
fn offline_a_local_hit_needs_no_request() {
    let mut db = Db::new("deck-detect-local-hit-offline");
    db.card(1, "a", "Creature");
    let archetype = db.archetype("q-local", "modern", "downloaded");
    db.list(archetype, &[1]);
    player_match(&db, &[1]);
    let mut lookups = FakeLookups {
        offline: true,
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    assert_eq!(player_row(&db), Some((archetype, 1.0)));
    assert!(lookups.estimates.is_empty());
}

// #489: the only held archetype of its colours (met once, its matches
// deleted) is not the answer when the API knows a better one.
#[test]
fn the_api_answer_beats_the_only_held_archetype() {
    let mut db = Db::new("deck-detect-api-first");
    db.card(1, "a", "Creature");
    let frog = db.archetype("q-frog", "modern", "downloaded");
    db.list(frog, &[1]);
    player_match(&db, &[1]);
    let definition = archetypes::parse_definition(DEFINITION).expect("definition");
    let mut lookups = FakeLookups {
        answer: estimate("q-new", 0.9),
        definitions: HashMap::from([("q-new".to_string(), definition)]),
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    let oculus = db.number("SELECT id FROM archetypes WHERE uuid = 'q-new'");
    assert_eq!(player_row(&db), Some((oculus, 0.9)));
}

#[test]
fn a_confident_manual_archetype_needs_no_request() {
    let mut db = Db::new("deck-detect-manual-local");
    db.card(1, "a", "Creature");
    let mine = db.archetype("q-mine", "modern", "manual");
    db.list(mine, &[1]);
    player_match(&db, &[1]);
    let mut lookups = FakeLookups {
        answer: estimate("q-other", 0.9),
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    assert_eq!(player_row(&db), Some((mine, 1.0)));
    assert!(lookups.estimates.is_empty());
}

#[test]
fn a_confident_manual_archetype_still_asks_the_api_for_the_opponent() {
    let mut db = Db::new("opponent-manual-asks");
    db.ten_cards();
    let mine = db.archetype("q-mine", "modern", "manual");
    db.list(mine, &[100, 101, 102, 103]);
    let storm = db.archetype("q-storm", "modern", "downloaded");
    opponent_plays_four(&db, 1);
    let mut lookups = FakeLookups {
        answer: estimate("q-storm", 1.0),
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    assert_eq!(db.rows(), vec![(1, 1, storm, 1.0, 0)]);
    assert_eq!(lookups.estimates.len(), 1);
}

#[test]
fn an_empty_estimate_still_takes_a_confident_manual_archetype_for_the_opponent() {
    let mut db = Db::new("opponent-manual-empty");
    db.ten_cards();
    let mine = db.archetype("q-mine", "modern", "manual");
    db.list(mine, &[100, 101, 102, 103]);
    opponent_plays_four(&db, 1);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert_eq!(db.rows(), vec![(1, 1, mine, 1.0, 0)]);
    assert_eq!(lookups.estimates.len(), 1);
}

// ---- 0.x ArchetypeFloorTest ----

/// Ten 4-ofs in the list; the player shows two of them (matched distinct
/// 2, below the evidence floor of 8).
fn thin_match(db: &Db) -> i64 {
    db.ten_cards();
    let archetype = db.archetype("q-floor", "modern", "downloaded");
    db.list(archetype, &(100..110).collect::<Vec<i64>>());
    player_match(db, &[100, 101]);
    archetype
}

#[test]
fn offline_a_sub_threshold_local_estimate_is_kept() {
    let mut db = Db::new("floor-offline-kept");
    let archetype = thin_match(&db);
    let mut lookups = FakeLookups {
        offline: true,
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    let (id, confidence) = player_row(&db).expect("a row");
    assert_eq!(id, archetype);
    assert!(confidence < 0.8, "{confidence}");
}

#[test]
fn offline_the_estimate_is_never_called() {
    let mut db = Db::new("floor-offline-never");
    thin_match(&db);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, true);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

#[test]
fn online_a_sub_threshold_estimate_still_asks_the_api() {
    let mut db = Db::new("floor-online-asks");
    thin_match(&db);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(targets(&stub), ["/api/archetypes/estimate"]);
}

#[test]
fn offline_without_candidates_the_player_gets_nothing() {
    let mut db = Db::new("floor-offline-none");
    db.complete_match(1, "CLEGACY");
    db.game(1, 1, &[(999_999, 4, 0)], &[]);
    let mut lookups = FakeLookups {
        offline: true,
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    assert!(db.rows().is_empty());
    assert!(lookups.estimates.is_empty());
}

#[test]
fn the_estimate_returns_nothing_offline_without_a_request() {
    let stub = stub(r#"[{"uuid":"q","confidence":1}]"#, None, "{}");
    let test_api = device_api(&stub.base_url, true);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    assert_eq!(
        lookups.estimate("CMODERN", &[mtgo_card(100, 4)]),
        EstimateOutcome::Failed("offline mode is on".to_string())
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

#[test]
fn the_estimate_still_asks_online() {
    let stub = stub(r#"[{"uuid":"q","confidence":1}]"#, None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    let outcome = lookups.estimate("CMODERN", &[mtgo_card(100, 4)]);
    assert!(
        matches!(&outcome, EstimateOutcome::Answered(answer) if answer.uuid == "q"),
        "{outcome:?}"
    );
    assert_eq!(targets(&stub), ["/api/archetypes/estimate"]);
}

#[test]
fn offline_a_queued_match_is_detected_without_the_api() {
    let mut db = Db::new("floor-offline-queue");
    thin_match(&db);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, true);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    let more = db
        .detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert!(!more);
    assert_eq!(db.marker(1), Some(NOW));
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

// ---- Failed and empty estimates (a 1.0.1 report: Modern opponents shown
// as an archetype they shared one card with) ----

/// The thin player list of `thin_match`, and the opponent showing the
/// same two cards.
fn thin_match_both_seats(db: &Db) -> i64 {
    let archetype = thin_match(db);
    db.game(1, 2, &[], &[(100, 4), (101, 4)]);
    archetype
}

/// A stub whose estimate answers 500 until `working` is set, then `body`.
fn flaky_estimate_stub(body: &str, working: Arc<std::sync::atomic::AtomicBool>) -> Stub {
    let body = body.to_string();
    Stub::start(move |request: &Recorded| {
        if request.target != "/api/archetypes/estimate" {
            Reply::json(404, "{}")
        } else if working.load(std::sync::atomic::Ordering::SeqCst) {
            Reply::json(200, &body)
        } else {
            Reply::json(500, r#"{"message":"Server Error"}"#)
        }
    })
}

// A failed estimate is no answer at all: nothing is written, the match
// stays due, and the next backfill with a working API labels it.
#[test]
fn a_failed_estimate_leaves_the_match_due_for_a_later_backfill() {
    let mut db = Db::new("estimate-failed-due");
    db.ten_cards();
    let burn = db.archetype("q-burn", "modern", "downloaded");
    db.list(burn, &[100, 101, 102, 103]);
    let storm = db.archetype("q-storm", "modern", "downloaded");
    opponent_plays_four(&db, 1);
    let working = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stub = flaky_estimate_stub(
        r#"[{"uuid":"q-storm","confidence":1}]"#,
        Arc::clone(&working),
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    let more = db
        .detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert!(!more, "no slice straight after a failure");
    assert!(db.rows().is_empty(), "{:?}", db.rows());
    assert_eq!(db.marker(1), None, "still due");

    working.store(true, std::sync::atomic::Ordering::SeqCst);
    db.clock.set(NOW + FAILED_MATCH_SKIP_MS);
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.rows(), vec![(1, 1, storm, 1.0, 0)]);
    assert_eq!(db.marker(1), Some(NOW + FAILED_MATCH_SKIP_MS));
}

// While the API fails, the backfill stops at the first failure instead of
// asking about every due match (and again at once).
#[test]
fn the_backfill_stops_at_the_first_failed_estimate() {
    let mut db = Db::new("estimate-failed-stops");
    three_matches(&db);
    db.sql("UPDATE matches SET archetypes_detected_at = NULL");
    let mut lookups = FakeLookups {
        fails: true,
        ..FakeLookups::default()
    };
    let more = db
        .detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert!(!more);
    assert_eq!(lookups.estimates.len(), 1);
    assert!(db.rows().is_empty());
    assert_eq!(
        (db.marker(1), db.marker(2), db.marker(3)),
        (None, None, None)
    );
}

// "Detect" by hand with a failing API writes nothing; the matches stay
// queued (markers cleared) for the backfill.
#[test]
fn a_detect_whose_estimate_fails_keeps_the_match_queued() {
    let mut db = Db::new("estimate-failed-detect");
    db.ten_cards();
    let burn = db.archetype("q-burn", "modern", "downloaded");
    db.list(burn, &[100, 101, 102, 103]);
    opponent_plays_four(&db, 1);
    db.seat(1, true, burn, false);
    db.sql("UPDATE matches SET archetypes_detected_at = 5");
    let mut lookups = FakeLookups {
        fails: true,
        ..FakeLookups::default()
    };
    assert_eq!(db.detect(&mut lookups, &[1]), 0);
    assert_eq!(db.marker(1), None);
}

// One match the API always fails on must not hold up the older ones: the
// backfill pauses after the failure, then passes over that match for a
// while, then asks about it again.
#[test]
fn a_match_whose_estimate_keeps_failing_does_not_hold_up_older_ones() {
    let mut db = Db::new("estimate-failed-skip");
    three_matches(&db);
    db.sql("UPDATE matches SET archetypes_detected_at = NULL");
    let homebrew = db.homebrew();
    // Match 3 (the newest, first in the queue) shows card 993.
    let mut lookups = FakeLookups {
        fails_for: Some(993),
        ..FakeLookups::default()
    };
    assert!(
        !db.detector(&mut lookups)
            .backfill(&|| false)
            .expect("backfill")
    );
    assert_eq!(lookups.estimates.len(), 1);
    assert!(db.rows().is_empty());

    // The pause: nothing is asked.
    db.clock.set(NOW + FAILED_ESTIMATE_PAUSE_MS - 1);
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(lookups.estimates.len(), 1);

    // After it, match 3 is passed over and the older two are detected.
    db.clock.set(NOW + FAILED_ESTIMATE_PAUSE_MS);
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(
        db.rows(),
        vec![(1, 1, homebrew, 0.0, 0), (2, 1, homebrew, 0.0, 0)]
    );
    assert_eq!(db.marker(3), None);

    // Once the skip ends, match 3 is asked about again.
    db.clock.set(NOW + FAILED_MATCH_SKIP_MS);
    lookups.fails_for = None;
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.rows().len(), 3);
    assert_eq!(db.marker(3), Some(NOW + FAILED_MATCH_SKIP_MS));
}

// The API named an archetype v1 does not hold and its definition could
// not be downloaded: the server's answer is not replaced by Homebrew for
// good; the match stays due.
#[test]
fn a_failed_definition_download_leaves_the_match_due() {
    let mut db = Db::new("definition-failed-due");
    opponent_plays_four(&db, 1);
    let mut lookups = FakeLookups {
        answer: estimate("q-new", 1.0),
        definitions_fail: true,
        ..FakeLookups::default()
    };
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert!(db.rows().is_empty(), "{:?}", db.rows());
    assert_eq!(db.marker(1), None);
    assert_eq!(lookups.definitions_asked, ["q-new"]);
}

// A definition the server says is gone is no archetype: Homebrew, done.
#[test]
fn a_gone_definition_gives_homebrew() {
    let mut db = Db::new("definition-gone");
    opponent_plays_four(&db, 1);
    let mut lookups = FakeLookups {
        answer: estimate("q-new", 1.0),
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    assert_eq!(db.rows(), vec![(1, 1, db.homebrew(), 0.0, 0)]);
    assert_eq!(db.marker(1), Some(NOW));
}

// A refused estimate (a 422) would be refused again: no archetype.
#[test]
fn a_refused_estimate_is_no_archetype() {
    let mut db = Db::new("estimate-refused");
    opponent_plays_four(&db, 1);
    let mut lookups = FakeLookups {
        refuses: true,
        ..FakeLookups::default()
    };
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.rows(), vec![(1, 1, db.homebrew(), 0.0, 0)]);
    assert_eq!(db.marker(1), Some(NOW));
}

// Cards without a catalog id give a request with no cards, which the
// server would refuse: nothing is asked, and it counts as no archetype.
#[test]
fn no_known_card_asks_nothing() {
    let mut db = Db::new("estimate-no-ids");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[(0, 4)]);
    let mut lookups = FakeLookups {
        fails: true,
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    assert!(lookups.estimates.is_empty());
    assert_eq!(db.marker(1), Some(NOW));
}

// The server named no archetype: a weak local score is not a label. The
// opponent gets Homebrew, the player no row, and the match is detected.
#[test]
fn an_empty_estimate_with_a_weak_local_score_gives_homebrew_and_no_player_row() {
    let mut db = Db::new("estimate-empty-weak");
    thin_match_both_seats(&db);
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    detect_with_api(&mut db, &test_api, &mut pacer, &[1]);
    assert_eq!(db.rows(), vec![(1, 1, db.homebrew(), 0.0, 0)]);
    assert_eq!(db.marker(1), Some(NOW));
    assert_eq!(targets(&stub).len(), 2, "both seats asked");
}

#[test]
fn offline_the_weak_local_score_still_labels_both_seats() {
    let mut db = Db::new("estimate-offline-weak");
    let archetype = thin_match_both_seats(&db);
    let mut lookups = FakeLookups {
        offline: true,
        ..FakeLookups::default()
    };
    db.detect(&mut lookups, &[1]);
    let rows = db.rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|row| row.2 == archetype && row.3 < 0.8));
    assert!(lookups.estimates.is_empty());
}

// ---- 0.x DeckArchetypeTest ----

/// A complete Modern deck: four of cards 100 and 101 and 52 of basic land
/// 900 in the maindeck, four of card 102 in the sideboard.
fn synced_deck(db: &Db, archetype_id: Option<i64>) {
    deck_with(db, "CMODERN", 52, archetype_id);
}

/// A deck in `format` holding `lands` copies of basic land 900 beside
/// four of cards 100 and 101 in the maindeck and four of 102 in the
/// sideboard; each call adds a newer version.
fn deck_with(db: &Db, format: &str, lands: u32, archetype_id: Option<i64>) {
    let signature = decks::signature(&[
        decks::DeckCard {
            catalog_id: 100,
            quantity: 4,
            sideboard: false,
        },
        decks::DeckCard {
            catalog_id: 101,
            quantity: 4,
            sideboard: false,
        },
        decks::DeckCard {
            catalog_id: 102,
            quantity: 4,
            sideboard: true,
        },
        decks::DeckCard {
            catalog_id: 900,
            quantity: lands,
            sideboard: false,
        },
    ]);
    let archetype = archetype_id.map_or("NULL".to_string(), |id| id.to_string());
    db.sql(&format!(
        "INSERT OR IGNORE INTO cards (catalog_id, oracle_id, name, type, color_identity)
             VALUES (900, 'q-oracle-mountain', 'QMountain', 'Basic Land - Mountain', 'R');
         INSERT OR IGNORE INTO decks (id, mtgo_id, name, format, created_at, updated_at, source,
             archetype_id)
             VALUES (1, 'Q-deck', 'Qdeck', '{format}', 1, 1, 'file', {archetype});
         INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES (1, '{signature}', (SELECT COUNT(*) + 1 FROM deck_versions));"
    ));
}

// MTGO saves a deck file while it is being built: a list short of a
// constructed maindeck is not guessed at, as the first answer would stick.
#[test]
fn a_deck_still_being_built_is_prefilled_once_complete() {
    let mut db = Db::new("prefill-partial");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &[100, 101]);
    deck_with(&db, "CMODERN", 51, None);
    let mut lookups = FakeLookups::default();
    assert!(!db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert!(lookups.estimates.is_empty(), "59 cards are not asked about");
    assert_eq!(
        db.number("SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"),
        1
    );

    deck_with(&db, "CMODERN", 52, None);
    assert!(db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(db.number("SELECT archetype_id FROM decks"), local);
}

// The API reads a format it has no archetypes for as Standard, so a
// Commander deck would be labelled with a Standard archetype.
#[test]
fn a_deck_in_a_format_without_archetypes_is_not_prefilled() {
    let mut db = Db::new("prefill-commander");
    db.archetype("q-standard", "standard", "downloaded");
    deck_with(&db, "CCOMMANDER", 92, None);
    let mut lookups = FakeLookups {
        answer: estimate("q-standard", 0.4),
        ..FakeLookups::default()
    };
    assert!(!db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert!(lookups.estimates.is_empty());
    assert_eq!(
        db.number("SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"),
        1
    );
}

#[test]
fn prefills_a_deck_without_an_archetype_on_sync() {
    let mut db = Db::new("prefill-api");
    let tron = db.archetype("tron-uuid", "modern", "downloaded");
    synced_deck(&db, None);
    let stub = stub(r#"[{"uuid":"tron-uuid","confidence":0.95}]"#, None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    assert!(db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(db.number("SELECT archetype_id FROM decks"), tron);
    let body: serde_json::Value =
        serde_json::from_slice(&stub.requests()[0].body).expect("json body");
    assert_eq!(
        body,
        serde_json::json!({"format": "CMODERN", "cards": [
            {"mtgo_id": 100, "quantity": 4}, {"mtgo_id": 101, "quantity": 4},
            {"mtgo_id": 900, "quantity": 52}]}),
        "the latest maindeck by catalog id"
    );
}

#[test]
fn does_not_overwrite_an_existing_archetype_on_sync() {
    let mut db = Db::new("prefill-kept");
    let existing = db.archetype("q-existing", "modern", "manual");
    synced_deck(&db, Some(existing));
    let mut lookups = FakeLookups {
        answer: estimate("q-existing", 1.0),
        ..FakeLookups::default()
    };
    assert!(!db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(db.number("SELECT archetype_id FROM decks"), existing);
    assert!(lookups.estimates.is_empty());
}

// #489: a prefill whose estimate was cut short by a waiting signal writes
// nothing, not the local answer (it is asked again).
#[test]
fn a_prefill_cut_short_writes_nothing() {
    let mut db = Db::new("prefill-cut-short");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &[100, 101]);
    synced_deck(&db, None);
    let mut lookups = SignalAfter {
        inner: FakeLookups::default(),
        stop_after: 1,
        cut: true,
        signal: std::rc::Rc::new(std::cell::Cell::new(false)),
    };
    assert!(!db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(
        db.number("SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"),
        1
    );
}

#[test]
fn a_prefill_without_the_api_answer_takes_the_local_one() {
    let mut db = Db::new("prefill-local-fallback");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &[100, 101]);
    synced_deck(&db, None);
    let mut lookups = FakeLookups::default();
    assert!(db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(db.number("SELECT archetype_id FROM decks"), local);
    assert_eq!(lookups.estimates.len(), 1);
}

#[test]
fn a_prefill_whose_estimate_failed_writes_nothing() {
    let mut db = Db::new("prefill-failed");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &[100, 101]);
    synced_deck(&db, None);
    let mut lookups = FakeLookups {
        fails: true,
        ..FakeLookups::default()
    };
    assert!(!db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(
        db.number("SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"),
        1
    );
    assert_eq!(lookups.estimates.len(), 1);
}

#[test]
fn a_prefill_after_an_empty_estimate_drops_a_weak_local_score() {
    let mut db = Db::new("prefill-empty-weak");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &(100..110).collect::<Vec<i64>>());
    synced_deck(&db, None);
    let mut lookups = FakeLookups::default();
    assert!(!db.detector(&mut lookups).prefill_deck(1).expect("prefill"));
    assert_eq!(
        db.number("SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"),
        1
    );
}

#[test]
fn a_deck_without_an_answer_gets_no_homebrew() {
    let mut db = Db::new("prefill-no-homebrew");
    synced_deck(&db, None);
    assert!(
        !db.detector(&mut FakeLookups::default())
            .prefill_deck(1)
            .expect("prefill")
    );
    assert_eq!(
        db.number("SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"),
        1
    );
}

#[test]
fn uses_the_deck_archetype_for_the_player_without_the_estimate() {
    let mut db = Db::new("detect-deck-archetype");
    let eldrazi = db.archetype("eldrazi-ramp-uuid", "modern", "manual");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[(123, 4, 0)], &[]);
    db.deck_for(1, 1, Some(eldrazi));
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert_eq!(player_row(&db), Some((eldrazi, 1.0)));
    assert!(lookups.estimates.is_empty());
}

// ---- 0.x TriggerArchetypeDetectionTest "flashes the queued count" ----

#[test]
fn the_deck_banner_queues_the_filtered_matches_and_answers_the_count() {
    let mut db = Db::new("detect-deck-banner");
    db.complete_match(1, "CMODERN");
    db.complete_match(2, "CMODERN");
    db.deck_for(1, 1, None);
    db.sql("UPDATE matches SET deck_version_id = 10, archetypes_detected_at = 5");
    let count = db
        .detector(&mut FakeLookups::default())
        .queue_deck(1, DeckFilter::NoOpponentArchetype)
        .expect("queue");
    assert_eq!(count, 2);
    assert_eq!((db.marker(1), db.marker(2)), (None, None), "queued");
    assert_eq!(
        store::archetypes::pending_archetype_count(&db.reader, 1).expect("pending"),
        2
    );
}

// ---- Plan T Review Focus ----

/// Review Focus 1: a hand-set seat stays through detection, a re-detection
/// and a definition change.
#[test]
fn a_manual_seat_survives_detection_redetection_and_a_definition_change() {
    let mut db = Db::new("detect-manual-survives");
    db.ten_cards();
    let detected = db.archetype("q-detected", "modern", "downloaded");
    db.list(detected, &[100, 101, 102, 103]);
    let picked = db.archetype("q-picked", "modern", "manual");
    opponent_plays_four(&db, 1);
    db.seat(1, true, picked, true);
    let mut lookups = FakeLookups::default();
    db.detect(&mut lookups, &[1]);
    assert_eq!(db.rows(), [(1, 1, picked, 1.0, 1)]);
    db.detect(&mut lookups, &[1]);
    let changed = Definition {
        uuid: "q-detected".to_string(),
        name: "Qdetected".to_string(),
        format: Some("modern".to_string()),
        cards: vec![DefinitionCard {
            mtgo_id: 100,
            arena_id: None,
            oracle_id: Some("q-oracle-100".to_string()),
            name: None,
            quantity: 4,
            sideboard: false,
        }],
        ..Definition::default()
    };
    // The change check: a changed definition of the archetype detection
    // would pick (match 1 has only hand-set rows, so its marker stays; the
    // detect and backfill below re-run detection anyway) and a server move
    // of the hand-set archetype.
    let chosen = db.archetype("q-chosen", "modern", "downloaded");
    let renamed = db.archetype("q-renamed", "modern", "downloaded");
    db.seat(1, false, chosen, true);
    let mut checker = FakeLookups {
        check_answer: Some(archetypes::DefinitionsCheck {
            changed: vec![changed],
            moved: vec![("q-chosen".to_string(), "q-renamed".to_string())],
            retired: Vec::new(),
        }),
        ..FakeLookups::default()
    };
    assert_eq!(
        db.detector(&mut checker)
            .check_definitions(&|| false)
            .expect("check"),
        CheckRun::Applied
    );
    db.detect(&mut lookups, &[1]);
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(
        db.rows(),
        [(1, 0, renamed, 1.0, 1), (1, 1, picked, 1.0, 1)],
        "both hand-set seats stand; the moved one follows its archetype"
    );
    assert!(lookups.estimates.is_empty());
}

/// Spec section 5: the change check sends every downloaded row with its
/// version and records when it ran; a moved uuid known only by uuid is
/// settled by the answer, so it is not asked about again (T4 review M7).
#[test]
fn the_change_check_sends_every_downloaded_version_and_settles_moves() {
    let mut db = Db::new("detect-change-check");
    let target = db.archetype("q-target", "modern", "downloaded");
    db.sql(
        "UPDATE archetypes SET definition_version = 'q-v0' WHERE uuid = 'q-target';
         INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES ('q-stub', 'q-stub', 'modern', 'downloaded', 1, 1);",
    );
    db.archetype("q-mine", "modern", "manual");
    opponent_plays_four(&db, 1);
    let stub = db.number("SELECT id FROM archetypes WHERE uuid = 'q-stub'");
    db.seat(1, true, stub, false);
    let mut lookups = FakeLookups {
        check_answer: Some(archetypes::DefinitionsCheck {
            moved: vec![("q-stub".to_string(), "q-target".to_string())],
            ..archetypes::DefinitionsCheck::default()
        }),
        ..FakeLookups::default()
    };
    db.detector(&mut lookups)
        .check_definitions(&|| false)
        .expect("check");
    assert_eq!(
        lookups.checks,
        [vec![
            ("q-stub".to_string(), None),
            ("q-target".to_string(), Some("q-v0".to_string()))
        ]],
        "downloaded rows only, by uuid"
    );
    assert_eq!(db.rows(), [(1, 1, target, 0.4, 0)]);
    assert_eq!(
        db.number(
            "SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'archetypes_checked_at'"
        ),
        NOW
    );
    // Nothing is known only by uuid any more: no further round asks.
    db.detector(&mut lookups)
        .fetch_missing_definitions(&|| false)
        .expect("missing");
    assert_eq!(lookups.checks.len(), 1);
}

/// Fails the `fail_at`-th definitions edit (1-based), as a missed deadline
/// or a quit between slices would; every other edit goes through.
struct FailingWriter<'a> {
    inner: &'a mut ConnWriter,
    fail_at: usize,
    seen: usize,
}

impl Writer for FailingWriter<'_> {
    fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
        if matches!(edit, ArchetypeEdit::ApplyDefinitions { .. }) {
            self.seen += 1;
            if self.seen == self.fail_at {
                return Err("the archetype edit missed its deadline".to_string());
            }
        }
        self.inner.edit(edit)
    }
}

/// T5 review I1: a later slice of the change check fails; the matches the
/// committed slice's definitions ask for detection again stay queued, and
/// the next check (which finds those versions applied) needs nothing more.
#[test]
fn a_failed_later_slice_keeps_the_earlier_slices_redetection() {
    let mut db = Db::new("detect-check-slice-fails");
    let held = db.archetype("q-held", "modern", "downloaded");
    db.sql("UPDATE archetypes SET definition_version = 'q-v1' WHERE uuid = 'q-held'");
    opponent_plays_four(&db, 1);
    db.seat(1, true, held, false);
    db.sql("UPDATE matches SET archetypes_detected_at = 5");
    let mut changed = vec![Definition {
        uuid: "q-held".to_string(),
        name: "Qheld".to_string(),
        format: Some("modern".to_string()),
        version: Some("q-v2".to_string()),
        ..Definition::default()
    }];
    for index in 0..DEFINITIONS_PER_EDIT {
        changed.push(Definition {
            uuid: format!("q-new-{index:03}"),
            name: format!("Qnew {index}"),
            format: Some("pauper".to_string()),
            version: Some("q-v1".to_string()),
            ..Definition::default()
        });
    }
    let answer = archetypes::DefinitionsCheck {
        changed,
        ..archetypes::DefinitionsCheck::default()
    };
    let mut lookups = FakeLookups {
        check_answer: Some(answer),
        ..FakeLookups::default()
    };
    let mut failing = FailingWriter {
        inner: &mut db.writer,
        fail_at: 2,
        seen: 0,
    };
    let mut detector = Detector {
        reader: &db.reader,
        lookups: &mut lookups,
        writer: &mut failing,
        now_ms: &|| NOW,
        failed: &mut FailedEstimates::default(),
        logs_only: false,
    };
    assert!(detector.check_definitions(&|| false).is_err());
    assert_eq!(db.marker(1), None, "queued with its definition");
    assert_eq!(
        db.number("SELECT definition_version = 'q-v2' FROM archetypes WHERE uuid = 'q-held'"),
        1
    );
    assert_eq!(
        db.number("SELECT COUNT(*) FROM settings WHERE key = 'archetypes_checked_at'"),
        0,
        "the check did not finish"
    );
    // The next check applies the rest; the committed slice plans nothing.
    db.sql("UPDATE matches SET archetypes_detected_at = 6");
    db.detector(&mut lookups)
        .check_definitions(&|| false)
        .expect("check again");
    assert_eq!(
        db.marker(1),
        Some(6),
        "nothing more to re-detect for q-held"
    );
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE uuid LIKE 'q-new-%'"),
        DEFINITIONS_PER_EDIT as i64
    );
}

/// Spec section 5: offline the check sends nothing and records nothing.
#[test]
fn no_check_while_offline() {
    let mut db = Db::new("detect-no-check-offline");
    db.archetype("q-held", "modern", "downloaded");
    let stub = stub("{}", Some("{}".to_string()), "{}");
    let test_api = device_api(&stub.base_url, true);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    let mut detector = db.detector(&mut lookups);
    assert_eq!(
        detector.check_definitions(&|| false).expect("check"),
        CheckRun::Offline
    );
    assert_eq!(
        detector
            .fetch_missing_definitions(&|| false)
            .expect("missing"),
        CheckRun::Applied,
        "nothing missing"
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM settings WHERE key = 'archetypes_checked_at'"),
        0
    );
}

/// At launch or wake the network is often not up yet: a check that cannot
/// reach MyMTGO is `Unreachable` (tried again soon), not a failure.
#[test]
fn a_check_that_cannot_reach_mymtgo_is_unreachable() {
    let mut db = Db::new("detect-check-unreachable");
    db.archetype("q-held", "modern", "downloaded");
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("address").port()
    };
    let test_api = device_api(&format!("http://127.0.0.1:{port}"), false);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    let mut detector = db.detector(&mut lookups);
    assert_eq!(
        detector.check_definitions(&|| false),
        Ok(CheckRun::Unreachable)
    );
}

/// Spec section 5 "Before the API has the endpoint": the check's 404 gives
/// rows without a definition their `/decklist` key list (cards only,
/// version null), and the check is not sent again until the next start.
#[test]
fn a_404_from_the_change_check_falls_back_to_the_decklist() {
    let mut db = Db::new("detect-check-404");
    db.sql(
        "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
         VALUES ('q-stub', 'q-stub', 'modern', 'downloaded', 1, 1)",
    );
    let stub = stub(
        "{}",
        None,
        r#"{"cards":[{"mtgo_id":501,"oracle_id":"q-oracle-a","name":"Qcard A","type":"Creature",
            "color_identity":"R","quantity":4,"sideboard":false}]}"#,
    );
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    let mut detector = db.detector(&mut lookups);
    assert_eq!(
        detector.check_definitions(&|| false).expect("check"),
        CheckRun::NotDeployed
    );
    assert_eq!(
        detector.check_definitions(&|| false).expect("again"),
        CheckRun::NotDeployed
    );
    assert_eq!(
        targets(&stub),
        [
            CHECK_PATH.to_string(),
            "/api/archetypes/q-stub/decklist".to_string()
        ],
        "one check, then no more"
    );
    assert_eq!(
        db.number(
            "SELECT definition IS NOT NULL AND definition_version IS NULL FROM archetypes
             WHERE uuid = 'q-stub'"
        ),
        1
    );
    assert_eq!(
        db.number("SELECT COUNT(*) FROM settings WHERE key = 'archetypes_checked_at'"),
        0,
        "no check happened"
    );
}

/// Review Focus 2: offline, detection decides locally and the stub sees no
/// connection at all, for a whole backfill.
#[test]
fn offline_detection_never_calls_the_estimate() {
    let mut db = Db::new("detect-offline-never");
    for id in 1..=3 {
        db.complete_match(id, "CMODERN");
        db.game(id, 1, &[(500 + id, 4, 0)], &[(600 + id, 2)]);
    }
    let stub = stub(r#"[{"uuid":"q","confidence":1}]"#, None, "{}");
    let test_api = device_api(&stub.base_url, true);
    let (mut pacer, _) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
    let homebrew = db.homebrew();
    assert_eq!(
        db.rows(),
        [
            (1, 1, homebrew, 0.0, 0),
            (2, 1, homebrew, 0.0, 0),
            (3, 1, homebrew, 0.0, 0)
        ]
    );
}

/// Review Focus 2: every API request after the first waits until 2 s have
/// passed since the last one ended; local-only matches wait for nothing.
#[test]
fn the_backfill_pauses_two_seconds_after_each_api_call() {
    let mut db = Db::new("detect-pace");
    db.ten_cards();
    // Manual, so the player decks of matches 4 and 5 are answered locally
    // online (#489; the opponent seat always asks).
    let local = db.archetype("q-local", "modern", "manual");
    db.list(local, &[100, 101, 102, 103]);
    // Matches 1 to 3 need the API (unknown opponent cards); 4 and 5 are a
    // local player deck with no opponent cards seen.
    for id in 1..=5 {
        db.complete_match(id, "CMODERN");
        if id <= 3 {
            db.game(id, 1, &[], &[(900 + id, 4)]);
        } else {
            let deck = [(100, 4, 0), (101, 4, 0), (102, 4, 0), (103, 4, 0)];
            db.game(id, 1, &deck, &[]);
        }
    }
    let stub = stub("[]", None, "{}");
    let test_api = device_api(&stub.base_url, false);
    let (mut pacer, clock): (Pacer, FakeClock) = pacer();
    let mut lookups = ApiLookups::new(&test_api.state, &mut pacer);
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(targets(&stub).len(), 3, "one estimate per unknown opponent");
    assert_eq!(
        *clock.sleeps.lock().expect("sleeps"),
        [2_000, 2_000],
        "a pause before every request but the first"
    );
    assert_eq!(db.rows().len(), 5);
}

/// Review Focus 5: a log match is not read before its game sync has
/// written its rows; it is detected once it has.
#[test]
fn detection_waits_for_the_game_sync() {
    let mut db = Db::new("detect-waits-sync");
    db.ten_cards();
    let archetype = db.archetype("q-a", "modern", "downloaded");
    db.list(archetype, &[100, 101, 102, 103]);
    opponent_plays_four(&db, 1);
    db.sql("UPDATE matches SET games_synced_at = NULL");
    let mut lookups = FakeLookups::default();
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.detect(&mut lookups, &[1]), 0, "Detect waits too");
    assert!(db.rows().is_empty());
    assert_eq!(db.marker(1), None, "still queued");
    db.sql("UPDATE matches SET games_synced_at = 1");
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.rows(), [(1, 1, archetype, 1.0, 0)]);
}

/// Spec 6.1 step 4: an API uuid v1 does not hold is downloaded (row,
/// definition, list 1) before the seat that names it is written.
#[test]
fn an_unheld_api_uuid_is_downloaded_before_it_is_written() {
    let db = Db::new("detect-unheld");
    opponent_plays_four(&db, 1);
    let order = Arc::new(Mutex::new(Vec::new()));
    struct Ordered {
        inner: ConnWriter,
        order: Arc<Mutex<Vec<String>>>,
    }
    impl Writer for Ordered {
        fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
            let name = match &edit {
                ArchetypeEdit::EnsureDownloaded { .. } => "download",
                ArchetypeEdit::SetDetections { .. } => "detections",
                _ => "other",
            };
            self.order.lock().expect("order").push(name.to_string());
            self.inner.edit(edit)
        }
    }
    let definition = archetypes::parse_definition(DEFINITION).expect("definition");
    let mut lookups = FakeLookups {
        answer: estimate("q-new", 0.5),
        definitions: HashMap::from([("q-new".to_string(), definition)]),
        ..FakeLookups::default()
    };
    let path = db._dir.join("mymtgo.sqlite");
    let mut writer = Ordered {
        inner: ConnWriter(store::open_writer(&path).expect("writer")),
        order: Arc::clone(&order),
    };
    let written = Detector {
        reader: &db.reader,
        lookups: &mut lookups,
        writer: &mut writer,
        now_ms: &|| NOW,
        failed: &mut FailedEstimates::default(),
        logs_only: false,
    }
    .detect_matches(&[1], &|| false)
    .expect("detect");
    assert_eq!(written, 1);
    let order: Vec<String> = order
        .lock()
        .expect("order")
        .iter()
        .filter(|name| *name != "other")
        .cloned()
        .collect();
    assert_eq!(order, ["download", "detections"]);
    let created = db.number("SELECT id FROM archetypes WHERE uuid = 'q-new'");
    assert_eq!(db.rows(), [(1, 1, created, 0.5, 0)]);
    assert_eq!(
        db.number(&format!(
            "SELECT COUNT(*) FROM archetype_cards WHERE archetype_id = {created} AND list = 1"
        )),
        1
    );
}

/// "Detect" on matches: each detected once, the count written answered.
#[test]
fn detect_on_matches_detects_each_once_and_answers_the_count() {
    let mut db = Db::new("detect-count");
    db.complete_match(1, "CMODERN");
    db.game(1, 1, &[], &[(999, 4)]);
    db.complete_match(2, "CMODERN");
    db.game(2, 1, &[], &[(998, 4)]);
    db.sql("UPDATE matches SET archetypes_detected_at = 5");
    let mut lookups = FakeLookups::default();
    assert_eq!(db.detect(&mut lookups, &[1, 2, 1, 99]), 2);
    assert_eq!(lookups.estimates.len(), 2, "each match once");
    assert_eq!((db.marker(1), db.marker(2)), (Some(NOW), Some(NOW)));
}

/// Spec 5 "Getting one": a downloaded row known only by uuid (a sync pull,
/// the 0.x import) gets its definition.
#[test]
fn rows_known_only_by_uuid_get_their_definitions() {
    let mut db = Db::new("detect-missing-definitions");
    db.sql(
        "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
         VALUES ('q-new', 'q-new', 'modern', 'downloaded', 1, 1)",
    );
    let definition = archetypes::parse_definition(DEFINITION).expect("definition");
    let mut lookups = FakeLookups {
        definitions: HashMap::from([("q-new".to_string(), definition)]),
        ..FakeLookups::default()
    };
    db.detector(&mut lookups)
        .fetch_missing_definitions(&|| false)
        .expect("fetch");
    assert_eq!(
        db.number("SELECT definition_version = 'q-v1' FROM archetypes WHERE uuid = 'q-new'"),
        1
    );
    let mut offline = FakeLookups {
        offline: true,
        ..FakeLookups::default()
    };
    db.sql(
        "UPDATE archetypes SET definition = NULL, definition_version = NULL WHERE uuid = 'q-new'",
    );
    db.detector(&mut offline)
        .fetch_missing_definitions(&|| false)
        .expect("fetch");
    assert!(offline.definitions_asked.is_empty(), "never offline");
}

/// T14 review C1: while a 0.x import runs, the backfill leaves imported
/// matches alone (their 0.x rows land at the import's last step); a log
/// match is still detected.
#[test]
fn the_backfill_skips_imported_matches_while_an_import_runs() {
    let mut db = Db::new("detect-import-running");
    db.ten_cards();
    let local = db.archetype("q-local", "modern", "downloaded");
    db.list(local, &[100, 101, 102, 103]);
    for id in 1..=2 {
        db.complete_match(id, "CMODERN");
        db.game(id, 1, &[], &[(100, 4), (101, 4), (102, 4), (103, 4)]);
    }
    db.sql("UPDATE matches SET source = 'imported', games_synced_at = NULL WHERE id = 2");
    let mut lookups = FakeLookups::default();
    let mut detector = db.detector(&mut lookups);
    detector.logs_only = true;
    detector.backfill(&|| false).expect("backfill");
    assert_eq!(db.marker(1), Some(NOW), "the log match is detected");
    assert_eq!(
        db.marker(2),
        None,
        "the imported match waits for the import"
    );
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill after the import");
    assert_eq!(db.marker(2), Some(NOW));
}

/// Asks `FakeLookups`, and after the `stop_after`-th estimate a signal
/// waits (the worker's `should_yield` says so). With `cut` that estimate's
/// paced wait was cut short by it: no answer, `interrupted`.
struct SignalAfter {
    inner: FakeLookups,
    stop_after: usize,
    cut: bool,
    signal: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Lookups for SignalAfter {
    fn api_allowed(&self) -> bool {
        true
    }

    fn estimate(&mut self, format: &str, cards: &[EstimateCard]) -> EstimateOutcome {
        if self.inner.estimates.len() + 1 == self.stop_after {
            self.signal.set(true);
            if self.cut {
                self.inner
                    .estimates
                    .push((format.to_string(), pairs_of(cards)));
                return EstimateOutcome::Failed("cut short".to_string());
            }
        }
        self.inner.estimate(format, cards)
    }

    fn definition(&mut self, uuid: &str) -> DefinitionReply {
        self.inner.definition(uuid)
    }

    fn check(&mut self, versions: &[(String, Option<String>)]) -> CheckReply {
        self.inner.check(versions)
    }

    fn interrupted(&self) -> bool {
        self.cut && self.signal.get()
    }
}

fn three_matches(db: &Db) {
    for id in 1..=3 {
        db.complete_match(id, "CMODERN");
        db.game(id, 1, &[], &[(990 + id, 4)]);
    }
    db.sql("UPDATE matches SET archetypes_detected_at = 5");
}

/// #300 F1: a bulk "Detect" stops for a waiting signal (another command,
/// or stopping): the matches done are written, the rest stay queued for
/// the backfill.
#[test]
fn a_bulk_detect_stops_for_a_waiting_signal() {
    let mut db = Db::new("detect-bulk-yields");
    three_matches(&db);
    let signal = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut lookups = SignalAfter {
        inner: FakeLookups::default(),
        stop_after: 1,
        cut: false,
        signal: std::rc::Rc::clone(&signal),
    };
    let waiting = std::rc::Rc::clone(&signal);
    let written = db
        .detector(&mut lookups)
        .detect_matches(&[1, 2, 3], &|| waiting.get())
        .expect("detect");
    assert_eq!(written, 1);
    assert_eq!(
        lookups.inner.estimates.len(),
        1,
        "no request after the signal"
    );
    assert_eq!(
        (db.marker(1), db.marker(2), db.marker(3)),
        (Some(NOW), None, None),
        "the rest queued"
    );
}

/// #300 F1: an estimate whose paced wait a signal cut short is no answer:
/// the match is not written (no Homebrew) and stays queued.
#[test]
fn a_cut_short_request_writes_nothing_for_its_match() {
    let mut db = Db::new("detect-cut-short");
    three_matches(&db);
    let signal = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut lookups = SignalAfter {
        inner: FakeLookups::default(),
        stop_after: 2,
        cut: true,
        signal: std::rc::Rc::clone(&signal),
    };
    let waiting = std::rc::Rc::clone(&signal);
    let written = db
        .detector(&mut lookups)
        .detect_matches(&[1, 2, 3], &|| waiting.get())
        .expect("detect");
    assert_eq!(written, 1);
    assert_eq!(
        (db.marker(1), db.marker(2), db.marker(3)),
        (Some(NOW), None, None)
    );
    assert!(
        db.rows().iter().all(|row| row.0 == 1),
        "nothing for match 2: {:?}",
        db.rows()
    );
    // The backfill takes the same care.
    db.sql("UPDATE matches SET archetypes_detected_at = NULL");
    db.sql("DELETE FROM match_archetypes");
    signal.set(false);
    lookups.inner.estimates.clear();
    let waiting = std::rc::Rc::clone(&signal);
    let more = db
        .detector(&mut lookups)
        .backfill(&|| waiting.get())
        .expect("backfill");
    assert!(more, "cut short: more is due");
    assert_eq!(db.rows().len(), 1, "{:?}", db.rows());
}

// A cut-short wait never reached the API, so it is not a failure: no
// pause, and the match it was for is not passed over.
#[test]
fn a_cut_short_request_is_not_recorded_as_a_failure() {
    let mut db = Db::new("detect-cut-short-not-failed");
    three_matches(&db);
    db.sql("UPDATE matches SET archetypes_detected_at = NULL");
    let signal = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut lookups = SignalAfter {
        inner: FakeLookups::default(),
        stop_after: 1,
        cut: true,
        signal: std::rc::Rc::clone(&signal),
    };
    let waiting = std::rc::Rc::clone(&signal);
    db.detector(&mut lookups)
        .backfill(&|| waiting.get())
        .expect("backfill");
    assert!(db.rows().is_empty());
    // At the same moment, with the signal handled: every match is asked.
    let mut lookups = FakeLookups::default();
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(lookups.estimates.len(), 3);
    assert_eq!(db.rows().len(), 3);
}

/// A writer that is not there (tracking restarting).
struct GoneWriter;

impl Writer for GoneWriter {
    fn edit(&mut self, _: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
        Err("tracking is restarting; try again".to_string())
    }

    fn ready(&self) -> bool {
        false
    }
}

/// #300 F2: without a writer nothing is asked, so no paced answer is
/// thrown away and asked again.
#[test]
fn nothing_is_asked_while_the_writer_is_gone() {
    let db = Db::new("detect-no-writer");
    three_matches(&db);
    db.sql("UPDATE matches SET archetypes_detected_at = NULL");
    let mut lookups = FakeLookups::default();
    let mut writer = GoneWriter;
    let mut detector = Detector {
        reader: &db.reader,
        lookups: &mut lookups,
        writer: &mut writer,
        now_ms: &|| NOW,
        failed: &mut FailedEstimates::default(),
        logs_only: false,
    };
    assert!(detector.backfill(&|| false).is_err());
    assert!(detector.detect_matches(&[1, 2], &|| false).is_err());
    assert!(lookups.estimates.is_empty());
}

// ---- The offline download's "Run detection" (2026-10-27 design section 3) ----

/// Every detected match asked again, offline, against a fuller pool: a
/// detected seat follows the better candidate, a hand-set seat and the
/// player's "no opponent archetype" choice stay.
#[test]
fn offline_redetection_keeps_hand_set_and_none_seats() {
    let mut db = Db::new("offline-redetect");
    db.ten_cards();
    let weak = db.archetype("q-weak", "modern", "manual");
    db.list(weak, &[100]);
    let better = db.archetype("q-better", "modern", "manual");
    db.list(better, &[100, 101, 102, 103, 104]);
    let deck: Vec<(i64, i64, i64)> = (100..105).map(|card| (card, 4, 0)).collect();
    let seen: Vec<(i64, i64)> = (100..105).map(|card| (card, 4)).collect();
    for id in [1, 2] {
        db.complete_match(id, "CMODERN");
        db.game(id, 1, &deck, &seen);
    }
    db.seat(1, false, weak, false);
    db.seat(1, true, weak, true);
    db.sql(
        "UPDATE matches SET opponent_archetype_none = 1 WHERE id = 2;
         UPDATE matches SET archetypes_detected_at = 1;",
    );

    let outcome = db
        .writer
        .edit(ArchetypeEdit::ClearAllDetected)
        .expect("cleared");
    assert_eq!(
        outcome,
        ArchetypeEditOutcome::Done {
            matches: vec![1, 2],
            decks: Vec::new(),
        }
    );
    let mut lookups = FakeLookups {
        offline: true,
        ..FakeLookups::default()
    };
    while db
        .detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill")
    {}
    let rows = db.rows();
    let player = rows
        .iter()
        .find(|row| row.0 == 1 && row.1 == 0)
        .expect("a player seat");
    assert_eq!((player.2, player.4), (better, 0), "{rows:?}");
    assert!(
        rows.contains(&(1, 1, weak, 1.0, 1)),
        "the hand-set opponent stays: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.0 == 2 && row.1 == 1),
        "the none choice stays: {rows:?}"
    );
    assert!(lookups.estimates.is_empty());
}

/// "Detect" on several matches writes each one as it is decided, so the
/// pages show each answer as it comes (and their spinners stop) instead of
/// all at the end, and reports each one in the order the page gave.
#[test]
fn detect_on_matches_writes_each_match_before_asking_the_next() {
    let db = Db::new("detect-each-written");
    three_matches(&db);
    let log = Arc::new(Mutex::new(Vec::new()));
    struct Logged {
        inner: ConnWriter,
        log: Arc<Mutex<Vec<String>>>,
    }
    impl Writer for Logged {
        fn edit(&mut self, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, String> {
            let entry = match &edit {
                ArchetypeEdit::SetDetections { results, .. } => {
                    let ids: Vec<String> = results.iter().map(|r| r.match_id.to_string()).collect();
                    format!("write {}", ids.join(","))
                }
                ArchetypeEdit::ClearDetected { match_ids } => format!("clear {match_ids:?}"),
                _ => "other".to_string(),
            };
            self.log.lock().expect("log").push(entry);
            self.inner.edit(edit)
        }
    }
    struct LoggedLookups {
        inner: FakeLookups,
        log: Arc<Mutex<Vec<String>>>,
    }
    impl Lookups for LoggedLookups {
        fn api_allowed(&self) -> bool {
            true
        }
        fn estimate(&mut self, format: &str, cards: &[EstimateCard]) -> EstimateOutcome {
            self.log.lock().expect("log").push("estimate".to_string());
            self.inner.estimate(format, cards)
        }
        fn definition(&mut self, uuid: &str) -> DefinitionReply {
            self.inner.definition(uuid)
        }
        fn check(&mut self, versions: &[(String, Option<String>)]) -> CheckReply {
            self.inner.check(versions)
        }
        fn interrupted(&self) -> bool {
            false
        }
    }
    let mut lookups = LoggedLookups {
        inner: FakeLookups::default(),
        log: Arc::clone(&log),
    };
    let path = db._dir.join("mymtgo.sqlite");
    let mut writer = Logged {
        inner: ConnWriter(store::open_writer(&path).expect("writer")),
        log: Arc::clone(&log),
    };
    let mut reported = Vec::new();
    let written = Detector {
        reader: &db.reader,
        lookups: &mut lookups,
        writer: &mut writer,
        now_ms: &|| NOW,
        failed: &mut FailedEstimates::default(),
        logs_only: false,
    }
    .detect_matches_reporting(&[3, 1, 2], &|| false, &mut |id| reported.push(id))
    .expect("detect");
    assert_eq!(written, 3);
    assert_eq!(
        *log.lock().expect("log"),
        [
            "clear [3, 1, 2]",
            "estimate",
            "write 3",
            "estimate",
            "write 1",
            "estimate",
            "write 2"
        ]
    );
    assert_eq!(reported, [3, 1, 2]);
}

/// A match not ready (here not there) writes nothing and the run goes on;
/// a failed estimate stops it: what was decided is written and reported,
/// the rest stay queued (markers cleared) and are not reported.
#[test]
fn detect_on_matches_skips_a_match_not_ready_and_stops_at_a_failed_estimate() {
    let mut db = Db::new("detect-each-stops");
    three_matches(&db);
    let mut lookups = FakeLookups {
        fails_for: Some(992),
        ..FakeLookups::default()
    };
    let mut reported = Vec::new();
    let written = db
        .detector(&mut lookups)
        .detect_matches_reporting(&[99, 1, 2, 3], &|| false, &mut |id| reported.push(id))
        .expect("detect");
    assert_eq!(written, 1);
    assert_eq!(reported, [1]);
    assert_eq!(lookups.estimates.len(), 2, "no request after the failure");
    assert_eq!(
        (db.marker(1), db.marker(2), db.marker(3)),
        (Some(NOW), None, None),
        "the rest queued"
    );
}

// ---- Commander formats (Arena Brawl spec section 6) ----

/// Two commander cards, resolved: 5001 (red) and 5002 (green and black,
/// stored out of order).
fn commander_cards(db: &Db) {
    db.sql(
        "INSERT INTO cards (catalog_id, oracle_id, name, color_identity)
             VALUES (5001, 'q-cmd-a', 'Qalpha', 'R'), (5002, 'q-cmd-b', 'Qbeta', 'GB');",
    );
}

/// A synced Historic Brawl match: the player's commander and the
/// opponent's (a catalog id each, `None` for no rows), a deck on the
/// match.
fn brawl_match(db: &Db, id: i64, player: Option<i64>, opponent: Option<i64>) {
    db.complete_match(id, "ARENA_HISTORIC_BRAWL");
    if let Some(card) = player {
        db.sql(&format!(
            "INSERT INTO match_commanders VALUES ({id}, 'player', 0, {card})"
        ));
    }
    if let Some(card) = opponent {
        db.sql(&format!(
            "INSERT INTO match_commanders VALUES ({id}, 'opponent', 0, {card})"
        ));
    }
}

fn commander_row(db: &Db, name: &str) -> i64 {
    db.number(&format!(
        "SELECT id FROM archetypes WHERE source = 'commander' AND name = '{name}'"
    ))
}

#[test]
fn a_brawl_match_gets_commander_archetypes() {
    let mut db = Db::new("brawl-detect");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    db.deck_for(1, 1, None);
    let mut lookups = FakeLookups::default();
    assert_eq!(db.detect(&mut lookups, &[1]), 1);
    let alpha = commander_row(&db, "Qalpha");
    let beta = commander_row(&db, "Qbeta");
    assert_eq!(db.rows(), vec![(1, 0, alpha, 1.0, 0), (1, 1, beta, 1.0, 0)]);
    assert!(lookups.estimates.is_empty(), "no estimate is asked");
    assert_eq!(db.number("SELECT archetype_id FROM decks"), alpha);
    assert_eq!(
        db.number(&format!(
            "SELECT COUNT(*) FROM archetypes WHERE id = {alpha} AND format = 'historic_brawl'
                 AND color_identity = 'R' AND definition IS NULL AND created_at = {NOW}
                 AND updated_at = {NOW}"
        )),
        1
    );
    // A two-colour commander stores the canonical comma form.
    assert_eq!(
        db.number(&format!(
            "SELECT COUNT(*) FROM archetypes WHERE id = {beta} AND color_identity = 'B,G'"
        )),
        1
    );
}

#[test]
fn a_second_match_with_the_same_commander_reuses_the_row() {
    let mut db = Db::new("brawl-reuse");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    brawl_match(&db, 2, Some(5001), Some(5002));
    db.detect(&mut FakeLookups::default(), &[1, 2]);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE source = 'commander'"),
        2
    );
    let rows = db.rows();
    assert_eq!(rows[0].2, rows[2].2);
    assert_eq!(rows[1].2, rows[3].2);
}

#[test]
fn a_renamed_commander_archetype_is_kept() {
    let mut db = Db::new("brawl-renamed");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    db.detect(&mut FakeLookups::default(), &[1]);
    let alpha = commander_row(&db, "Qalpha");
    db.sql(&format!(
        "UPDATE archetypes SET name = 'Mine' WHERE id = {alpha}"
    ));
    brawl_match(&db, 2, Some(5001), Some(5002));
    db.detect(&mut FakeLookups::default(), &[2]);
    assert_eq!(
        db.number(
            "SELECT archetype_id FROM match_archetypes WHERE match_id = 2 AND is_opponent = 0"
        ),
        alpha
    );
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE source = 'commander' AND name = 'Qalpha'"),
        0,
        "no second row under the old name"
    );
}

#[test]
fn a_merged_commander_archetype_resolves_to_the_target() {
    let mut db = Db::new("brawl-merged");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    db.detect(&mut FakeLookups::default(), &[1]);
    let alpha = commander_row(&db, "Qalpha");
    let target = db.archetype("q-target", "historic_brawl", "manual");
    db.sql(&format!(
        "UPDATE archetypes SET merged_into_id = {target} WHERE id = {alpha}"
    ));
    brawl_match(&db, 2, Some(5001), Some(5002));
    db.deck_for(2, 2, None);
    db.detect(&mut FakeLookups::default(), &[2]);
    assert_eq!(db.rows()[2].2, target);
    assert_eq!(db.number("SELECT archetype_id FROM decks"), target);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE source = 'commander'"),
        2
    );
}

#[test]
fn a_brawl_match_with_unresolved_commander_cards_waits() {
    let mut db = Db::new("brawl-waits");
    db.sql("INSERT INTO cards (catalog_id) VALUES (5001), (5002);");
    brawl_match(&db, 1, Some(5001), Some(5002));
    let mut lookups = FakeLookups::default();
    assert_eq!(db.detect(&mut lookups, &[1]), 0);
    assert_eq!(db.marker(1), None);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE source = 'commander'"),
        0
    );
    db.sql(
        "UPDATE cards SET oracle_id = 'q-cmd-a', name = 'Qalpha', color_identity = 'R'
             WHERE catalog_id = 5001;
         UPDATE cards SET oracle_id = 'q-cmd-b', name = 'Qbeta', color_identity = 'U'
             WHERE catalog_id = 5002;",
    );
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(db.marker(1), Some(NOW));
    assert_eq!(db.rows().len(), 2);
}

/// Match 633's shape: the opponent's commander is a card the API does not
/// know (`api_unknown_at`), the player's resolved. The player gets their
/// commander, the opponent seat stays empty and the match is detected.
#[test]
fn an_unknown_opponent_commander_does_not_block_the_player() {
    let mut db = Db::new("brawl-unknown-opponent");
    commander_cards(&db);
    db.sql("INSERT INTO cards (catalog_id, api_unknown_at) VALUES (5009, 7);");
    brawl_match(&db, 1, Some(5001), Some(5009));
    db.deck_for(1, 1, None);
    let mut lookups = FakeLookups::default();
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    let alpha = commander_row(&db, "Qalpha");
    assert_eq!(db.rows(), vec![(1, 0, alpha, 1.0, 0)]);
    assert_eq!(db.number("SELECT archetype_id FROM decks"), alpha);
    assert_eq!(db.marker(1), Some(NOW));
}

/// A commander card the cards worker has not looked up yet still waits.
#[test]
fn a_pending_opponent_commander_still_waits() {
    let mut db = Db::new("brawl-pending-opponent");
    commander_cards(&db);
    db.sql("INSERT INTO cards (catalog_id) VALUES (5009);");
    brawl_match(&db, 1, Some(5001), Some(5009));
    assert_eq!(db.detect(&mut FakeLookups::default(), &[1]), 0);
    assert_eq!(db.marker(1), None);
    assert!(db.rows().is_empty());
}

#[test]
fn a_hand_set_brawl_opponent_is_kept() {
    let mut db = Db::new("brawl-manual");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    let hand = db.archetype("q-hand", "historic_brawl", "manual");
    db.seat(1, true, hand, true);
    db.detect(&mut FakeLookups::default(), &[1]);
    let alpha = commander_row(&db, "Qalpha");
    assert_eq!(db.rows(), vec![(1, 0, alpha, 1.0, 0), (1, 1, hand, 1.0, 1)]);
}

#[test]
fn a_seat_without_commanders_gets_no_row() {
    let mut db = Db::new("brawl-no-opponent");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), None);
    db.detect(&mut FakeLookups::default(), &[1]);
    let alpha = commander_row(&db, "Qalpha");
    assert_eq!(db.rows(), vec![(1, 0, alpha, 1.0, 0)]);
    assert_eq!(db.marker(1), Some(NOW));
}

#[test]
fn detecting_the_same_brawl_match_again_is_a_no_op() {
    let mut db = Db::new("brawl-again");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    db.deck_for(1, 1, None);
    db.detect(&mut FakeLookups::default(), &[1]);
    let first = db.rows();
    let deck = db.number("SELECT archetype_id FROM decks");
    assert_eq!(db.detect(&mut FakeLookups::default(), &[1]), 1);
    assert_eq!(db.rows(), first);
    assert_eq!(db.number("SELECT archetype_id FROM decks"), deck);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE source = 'commander'"),
        2
    );
}

#[test]
fn two_matches_of_one_deck_in_a_slice_are_both_written() {
    let mut db = Db::new("brawl-same-deck");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    brawl_match(&db, 2, Some(5001), Some(5002));
    db.deck_for(1, 1, None);
    db.sql("UPDATE matches SET deck_version_id = 10 WHERE id = 2");
    let mut lookups = FakeLookups::default();
    let more = db
        .detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert!(!more);
    assert_eq!(db.marker(1), Some(NOW));
    assert_eq!(
        db.marker(2),
        Some(NOW),
        "the deck change is not a stale read"
    );
}

#[test]
fn unresolved_brawl_matches_do_not_starve_older_ones() {
    let mut db = Db::new("brawl-starve");
    db.sql("INSERT INTO cards (catalog_id) VALUES (5001);");
    for id in 100..112 {
        brawl_match(&db, id, Some(5001), None);
    }
    opponent_plays_four(&db, 1);
    db.ten_cards();
    let mut lookups = FakeLookups::default();
    db.detector(&mut lookups)
        .backfill(&|| false)
        .expect("backfill");
    assert_eq!(
        db.marker(1),
        Some(NOW),
        "the older Modern match is detected"
    );
    assert_eq!(db.marker(111), None);
}

#[test]
fn a_hand_set_opponent_commander_makes_no_row() {
    let mut db = Db::new("brawl-manual-no-row");
    commander_cards(&db);
    brawl_match(&db, 1, Some(5001), Some(5002));
    let hand = db.archetype("q-hand2", "historic_brawl", "manual");
    db.seat(1, true, hand, true);
    db.detect(&mut FakeLookups::default(), &[1]);
    assert_eq!(
        db.number("SELECT COUNT(*) FROM archetypes WHERE source = 'commander'"),
        1
    );
}
