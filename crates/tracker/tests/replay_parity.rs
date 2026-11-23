//! The replay snapshots v1 shares against the ones 0.x shares for the same
//! games (replays spec section 14, Plan V task 10): each parity log fixture
//! is read by the engine, every match's snapshot is built and prepared
//! (redaction and validation) as `share_replay` does, and the result is
//! compared as JSON values with `fixtures/parity/replays/{fixture}.golden.json`,
//! exported by 0.x's `ExportParityGoldensTest` "exports a replay golden"
//! through 0.x's `BuildReplaySnapshot` and the package's
//! `PrepareSharedSnapshot`.
//!
//! The golden's card rows (`Qcard {id}`, `Qtype`, a fake https image) and
//! archetype names are seeded into v1 as 0.x had them. A frame the golden
//! writes as `{"timestamp", "same_as_previous": true}` repeats the content
//! of the one before it. Every difference is fixed or justified in
//! `fixtures/parity/allowlist.json` (fixture `replays`), and an entry that
//! no longer occurs fails.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use common::*;
use fixture_tools::{Fixture, TestDir, load_fixture};
use mtgo_match::ABANDON_AFTER_MS;
use replay::{CardInfo, build_snapshot, prepare_shared_snapshot};
use serde_json::Value;
use store::replay::{card_infos, replay_inputs};
use store::{Connection, open_reader, open_writer, params};
use tracker::TrackerConfig;

/// The allowlist's fixture name for every replay golden.
const FIXTURE: &str = "replays";

/// The parity log fixtures, each with a replay golden.
const FIXTURES: [&str; 3] = ["draft_hobbit", "draft_incomplete", "league_join_drop"];

fn parity_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/parity")
        .join(name)
}

fn golden(fixture: &str) -> Value {
    let path = parity_path(&format!("replays/{fixture}.golden.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("golden json")
}

fn allowlist() -> Vec<Value> {
    let text = std::fs::read_to_string(parity_path("allowlist.json")).expect("the allowlist");
    let list: Value = serde_json::from_str(&text).expect("the allowlist is JSON");
    list.as_array().expect("a list").clone()
}

/// The allowlisted differences, as `(token, field)`; `*` matches any token.
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

/// The fixture read by the engine into a fresh database.
fn tracked(fixture_name: &str) -> (TestDir, TrackerConfig) {
    let fixture: Fixture = load_fixture(fixture_name);
    let dir = TestDir::new(&format!("replay-parity-{fixture_name}"));
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let now = fixture.meta.now_ms() + 3 * ABANDON_AFTER_MS;
    let mut engine = open(&config, now);
    run_until_settled(&mut engine, now);
    drop(engine);
    (dir, config)
}

/// The golden's card rows, as 0.x held them.
fn seed_cards(conn: &Connection, golden: &Value) {
    for id in golden["cards"].as_array().expect("cards") {
        let id = id.as_u64().expect("a catalog id");
        conn.execute(
            "INSERT INTO cards (catalog_id, name, type, image) VALUES (?1, ?2, 'Qtype', ?3)
             ON CONFLICT (catalog_id) DO UPDATE SET name = excluded.name,
                 type = excluded.type, image = excluded.image, local_image = NULL",
            params![
                id as i64,
                format!("Qcard {id}"),
                format!("https://cards.example/{id}.jpg")
            ],
        )
        .expect("card");
    }
}

/// An archetype row named `name`, made once.
fn archetype(conn: &Connection, name: &str) -> i64 {
    conn.execute(
        "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
         VALUES (?1, ?2, 'CMODERN', 'downloaded', 0, 0)
         ON CONFLICT (uuid) DO NOTHING",
        params![format!("q-replay-{name}"), name],
    )
    .expect("archetype");
    conn.query_row(
        "SELECT id FROM archetypes WHERE uuid = ?1",
        [format!("q-replay-{name}")],
        |row| row.get(0),
    )
    .expect("archetype id")
}

/// The match's archetype names as 0.x's snapshot names them: the deck the
/// match was played with carries the local one, the opponent seat the
/// other.
fn seed_archetypes(conn: &Connection, match_id: i64, meta: &Value) {
    if let Some(name) = meta["local_archetype"].as_str() {
        let archetype_id = archetype(conn, name);
        let deck_key = format!("q-replay-deck-{match_id}");
        conn.execute(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source, archetype_id)
             VALUES (?1, 'Qdeck', 'CMODERN', 0, 0, 'file', ?2)",
            params![deck_key, archetype_id],
        )
        .expect("deck");
        let version: i64 = conn
            .query_row(
                "INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES ((SELECT id FROM decks WHERE mtgo_id = ?1), 'Q', 0) RETURNING id",
                [&deck_key],
                |row| row.get(0),
            )
            .expect("version");
        conn.execute(
            "UPDATE matches SET deck_version_id = ?1 WHERE id = ?2",
            params![version, match_id],
        )
        .expect("link");
    }
    if let Some(name) = meta["opponent_archetype"].as_str() {
        let archetype_id = archetype(conn, name);
        conn.execute(
            "INSERT OR REPLACE INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES (?1, 1, ?2, 1.0)",
            params![match_id, archetype_id],
        )
        .expect("seat");
    }
}

fn match_id(conn: &Connection, token: &str) -> Option<i64> {
    conn.query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
        row.get(0)
    })
    .ok()
}

/// The snapshot v1 shares for the match (`app::replay::share::upload` up
/// to the request): built, then prepared. `None` when no game has frames.
fn shared_snapshot(conn: &Connection, match_id: i64, tz: chrono_tz::Tz) -> Option<Value> {
    let inputs = replay_inputs(conn, match_id, tz, &[])
        .expect("read")
        .expect("a match");
    let cards = |ids: &[u32]| -> BTreeMap<u32, CardInfo> { card_infos(conn, ids).expect("cards") };
    let snapshot = build_snapshot(inputs.snapshot_input(), &cards);
    if snapshot.games.is_empty() {
        return None;
    }
    let prepared = prepare_shared_snapshot(snapshot, &inputs.opponent_names)
        .unwrap_or_else(|failure| panic!("match {match_id}: {failure:?}"));
    Some(serde_json::from_slice(prepared.json()).expect("prepared json"))
}

/// The golden's frames with every `same_as_previous` frame given the
/// content of the one before it.
fn expand(frames: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for frame in frames {
        if frame["same_as_previous"] == true {
            let content = out.last().expect("a frame before")["content"].clone();
            out.push(serde_json::json!({"timestamp": frame["timestamp"], "content": content}));
        } else {
            out.push(frame.clone());
        }
    }
    out
}

/// Ruling 4 applied to 0.x's frames: a frame identical to the one before it
/// is dropped (the first one's time kept), except the game's last frame.
fn without_repeats(frames: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let last = index + 1 == frames.len();
        let repeat = index > 0 && frame["content"] == frames[index - 1]["content"];
        if !repeat || last {
            out.push(frame.clone());
        }
    }
    out
}

/// The first place two frame lists differ, as a readable line.
fn first_frame_difference(theirs: &[Value], ours: &[Value]) -> String {
    for (index, (a, b)) in theirs.iter().zip(ours).enumerate() {
        if a != b {
            return format!(
                "frame {index} (0.x at {}, v1 at {}): {}",
                a["timestamp"],
                b["timestamp"],
                first_difference(a, b, "frame")
            );
        }
    }
    format!("0.x {} frames, v1 {}", theirs.len(), ours.len())
}

/// The first path at which two JSON values differ.
fn first_difference(a: &Value, b: &Value, path: &str) -> String {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for key in keys {
                let (left, right) = (
                    x.get(key).unwrap_or(&Value::Null),
                    y.get(key).unwrap_or(&Value::Null),
                );
                if left != right || x.contains_key(key) != y.contains_key(key) {
                    return first_difference(left, right, &format!("{path}.{key}"));
                }
            }
            path.to_string()
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (index, (left, right)) in x.iter().zip(y).enumerate() {
                if left != right {
                    return first_difference(left, right, &format!("{path}[{index}]"));
                }
            }
            path.to_string()
        }
        _ => format!("{path}: 0.x {a}, v1 {b}"),
    }
}

/// Every difference between 0.x's snapshot and v1's, as `(token, field)`
/// with a readable line.
fn differences(token: &str, theirs: &Value, ours: &Value) -> Vec<((String, String), String)> {
    let mut found = Vec::new();
    let mut differ = |field: String, line: String| {
        found.push(((token.to_string(), field), format!("{token} {line}")));
    };
    if theirs["version"] != ours["version"] {
        differ(
            "version".to_string(),
            format!("version: 0.x {}, v1 {}", theirs["version"], ours["version"]),
        );
    }
    let top = |snapshot: &Value| -> BTreeSet<String> {
        snapshot
            .as_object()
            .expect("snapshot")
            .keys()
            .cloned()
            .collect()
    };
    let (their_top, our_top) = (top(theirs), top(ours));
    for key in their_top.union(&our_top) {
        if !their_top.contains(key) {
            differ(key.clone(), format!("{key}: only in v1"));
        } else if !our_top.contains(key) {
            differ(key.clone(), format!("{key}: only in 0.x"));
        }
    }
    for (key, value) in theirs["meta"].as_object().expect("meta") {
        if &ours["meta"][key] != value {
            let field =
                if key == "played_at" && !played_at_within_a_second(value, &ours["meta"][key]) {
                    "meta.played_at (beyond 1 s)".to_string()
                } else {
                    format!("meta.{key}")
                };
            differ(
                field,
                format!("meta.{key}: 0.x {value}, v1 {}", ours["meta"][key]),
            );
        }
    }
    let keys = |snapshot: &Value| -> BTreeSet<String> {
        snapshot["meta"]
            .as_object()
            .expect("meta")
            .keys()
            .cloned()
            .collect()
    };
    for key in keys(ours).difference(&keys(theirs)) {
        differ(format!("meta.{key}"), format!("meta.{key}: only in v1"));
    }

    let games = |snapshot: &Value| -> BTreeMap<u64, Value> {
        snapshot["games"]
            .as_array()
            .expect("games")
            .iter()
            .map(|game| (game["game_number"].as_u64().expect("number"), game.clone()))
            .collect()
    };
    let (their_games, our_games) = (games(theirs), games(ours));
    let numbers: BTreeSet<u64> = their_games
        .keys()
        .chain(our_games.keys())
        .copied()
        .collect();
    for number in numbers {
        let (Some(theirs), Some(ours)) = (their_games.get(&number), our_games.get(&number)) else {
            differ(
                format!("games[{number}]"),
                format!("game {number}: only in one snapshot"),
            );
            continue;
        };
        let fields: BTreeSet<&String> = theirs
            .as_object()
            .expect("game")
            .keys()
            .chain(ours.as_object().expect("game").keys())
            .collect();
        for field in fields {
            let (left, right) = (&theirs[field.as_str()], &ours[field.as_str()]);
            if field == "frames" {
                let all = expand(left.as_array().expect("frames"));
                let mine = right.as_array().expect("frames");
                if &all == mine {
                    continue;
                }
                let kept = without_repeats(&all);
                if &kept == mine {
                    differ(
                        "games[].frames (repeats dropped)".to_string(),
                        format!(
                            "game {number}: {} of 0.x's {} frames kept, repeats dropped",
                            mine.len(),
                            all.len()
                        ),
                    );
                } else {
                    differ(
                        format!("games[{number}].frames"),
                        format!(
                            "game {number} frames (0.x with repeats dropped): {}",
                            first_frame_difference(&kept, mine)
                        ),
                    );
                }
                continue;
            }
            if left == right {
                continue;
            }
            if field == "sideboard" && same_entries(left, right) {
                differ(
                    "games[].sideboard order".to_string(),
                    format!("game {number}: sideboard entries in another order"),
                );
                continue;
            }
            differ(
                format!("games[{number}].{field}"),
                format!("game {number} {}", first_difference(left, right, field)),
            );
        }
    }
    found
}

/// Whether v1's `played_at` is at or up to 1,000 ms before 0.x's (the
/// allowlisted difference: a v1 game starts at its first sign of life).
fn played_at_within_a_second(theirs: &Value, ours: &Value) -> bool {
    let parse = |value: &Value| {
        value
            .as_str()
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
    };
    let (Some(theirs), Some(ours)) = (parse(theirs), parse(ours)) else {
        return false;
    };
    let gap = (theirs - ours).num_milliseconds();
    (0..=1_000).contains(&gap)
}

/// Two lists holding the same entries in some order.
fn same_entries(a: &Value, b: &Value) -> bool {
    let sorted = |value: &Value| -> Option<Vec<String>> {
        let mut entries: Vec<String> = value.as_array()?.iter().map(Value::to_string).collect();
        entries.sort();
        Some(entries)
    };
    sorted(a).is_some() && sorted(a) == sorted(b)
}

// Review focus 2: the snapshot v1 shares equals the one 0.x shares for the
// same games, or differs only as allowlisted; no opponent name survives in
// either.
#[test]
fn replay_snapshots_match_0x_goldens() {
    let allowed = allowed();
    let mut found = Vec::new();
    let mut compared = 0;
    for fixture_name in FIXTURES {
        let golden = golden(fixture_name);
        assert_eq!(golden["fixture"], fixture_name);
        let (_dir, config) = tracked(fixture_name);
        let writer = open_writer(&config.db_path).expect("writer");
        seed_cards(&writer, &golden);
        for expected in golden["matches"].as_array().expect("matches") {
            let token = expected["token"].as_str().expect("token");
            let id = match_id(&writer, token)
                .unwrap_or_else(|| panic!("{fixture_name}: v1 has no match {token}"));
            if let Some(meta) = expected["snapshot"].get("meta") {
                seed_archetypes(&writer, id, meta);
            }
        }
        drop(writer);

        let reader = open_reader(&config.db_path).expect("reader");
        for expected in golden["matches"].as_array().expect("matches") {
            let token = expected["token"].as_str().expect("token");
            let id = match_id(&reader, token).expect("match");
            let ours = shared_snapshot(&reader, id, config.timezone);
            match (&expected["snapshot"], &ours) {
                (Value::Null, None) => {}
                (Value::Null, Some(_)) | (_, None) => found.push((
                    (token.to_string(), "snapshot".to_string()),
                    format!("{token}: one side has no game with frames"),
                )),
                (theirs, Some(ours)) => {
                    compared += 1;
                    let text = ours.to_string();
                    assert!(text.contains("\"Opponent\""), "{token}: redacted");
                    found.extend(differences(token, theirs, ours));
                }
            }
        }
        // Every v1 match with frames is in the golden.
        let mut stmt = reader
            .prepare("SELECT id, token FROM matches ORDER BY id")
            .expect("matches");
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        let tokens: BTreeSet<&str> = golden["matches"]
            .as_array()
            .expect("matches")
            .iter()
            .map(|one| one["token"].as_str().expect("token"))
            .collect();
        for (id, token) in rows {
            if !tokens.contains(token.as_str())
                && shared_snapshot(&reader, id, config.timezone).is_some()
            {
                found.push((
                    (token.clone(), "snapshot".to_string()),
                    format!("{token}: only v1 shares this match"),
                ));
            }
        }
    }
    assert!(compared >= 5, "the goldens hold {compared} shared matches");

    let matches = |key: &(String, String)| {
        allowed.contains(key) || allowed.contains(&("*".to_string(), key.1.clone()))
    };
    let unexplained: Vec<&String> = found
        .iter()
        .filter(|(key, _)| !matches(key))
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
    let stale: Vec<&(String, String)> = allowed
        .iter()
        .filter(|(token, field)| {
            !keys
                .iter()
                .any(|(seen, seen_field)| seen_field == field && (token == "*" || token == seen))
        })
        .filter(|(_, field)| !NOTES.contains(&field.as_str()))
        .collect();
    assert!(
        stale.is_empty(),
        "allowlist entries that no longer occur: {stale:?}"
    );
}

/// The spec section 16 deviations no golden can see, recorded as notes
/// (fixture `replays`).
const NOTES: [&str; 2] = [
    "log line on a dropped repeat's second",
    "old frame times trimmed to the time of day",
];

#[test]
fn the_replay_deviations_are_noted_in_the_allowlist() {
    let notes: Vec<String> = allowlist()
        .iter()
        .filter(|entry| entry["fixture"] == FIXTURE)
        .map(|entry| entry["field"].as_str().expect("field").to_string())
        .collect();
    for field in NOTES
        .iter()
        .copied()
        .chain(["games[].frames (repeats dropped)"])
    {
        assert!(notes.iter().any(|note| note == field), "{field}");
    }
}

// Replays spec section 12 on real games: `replay_inputs` and one game's
// decoration for every game of the parity fixtures' matches, p95 under
// 300 ms. Run in release:
// `cargo test --release -p tracker --test replay_parity -- --ignored --nocapture`.
#[test]
#[ignore = "speed budget: run in release with --ignored --nocapture"]
fn replay_inputs_under_300_ms_on_the_parity_fixtures() {
    use replay::{ImageMode, decorate_frames};
    use std::time::{Duration, Instant};

    let mut timings = Vec::new();
    let mut frames = 0;
    for fixture_name in FIXTURES {
        let golden = golden(fixture_name);
        let (_dir, config) = tracked(fixture_name);
        let writer = open_writer(&config.db_path).expect("writer");
        seed_cards(&writer, &golden);
        drop(writer);
        let reader = open_reader(&config.db_path).expect("reader");
        for expected in golden["matches"].as_array().expect("matches") {
            let id = match_id(&reader, expected["token"].as_str().expect("token")).expect("match");
            let games = replay_inputs(&reader, id, config.timezone, &[])
                .expect("read")
                .expect("a match")
                .games
                .len();
            for index in 0..games {
                for _ in 0..10 {
                    let started = Instant::now();
                    let inputs = replay_inputs(&reader, id, config.timezone, &[])
                        .expect("read")
                        .expect("a match");
                    let game = inputs.games[index].clone();
                    let mut ids: Vec<u32> = game
                        .frames
                        .iter()
                        .filter_map(|frame| frame.content["Cards"].as_array())
                        .flatten()
                        .filter_map(|card| card["CatalogID"].as_u64())
                        .map(|id| id as u32)
                        .collect();
                    ids.sort_unstable();
                    ids.dedup();
                    let cards = card_infos(&reader, &ids).expect("cards");
                    let shown = decorate_frames(
                        game.frames,
                        &inputs.local_username,
                        &cards,
                        ImageMode::App,
                    );
                    timings.push(started.elapsed());
                    frames = frames.max(shown.len());
                }
            }
        }
    }
    timings.sort();
    let p95 = timings[(timings.len() * 95).div_ceil(100) - 1];
    println!(
        "parity replay views: {} reads, largest game {frames} frames; min {:?}, median {:?}, p95 {p95:?} (budget 300ms)",
        timings.len(),
        timings[0],
        timings[timings.len() / 2]
    );
    assert!(p95 < Duration::from_millis(300), "p95 {p95:?}");
}

/// The fields `differences` reports for two small snapshots.
fn fields_of(theirs: Value, ours: Value) -> Vec<String> {
    differences("q-token", &theirs, &ours)
        .into_iter()
        .map(|((_, field), _)| field)
        .collect()
}

fn snapshot_at(played_at: &str) -> Value {
    serde_json::json!({"version": 1, "meta": {"played_at": played_at}, "games": []})
}

// FIX NOW 3: the allowlisted `played_at` difference is bounded to v1 at or
// up to a second before 0.x; anything else is reported apart and fails.
#[test]
fn played_at_is_allowlisted_only_up_to_a_second_earlier() {
    let theirs = || snapshot_at("2026-10-01T12:00:01.000Z");
    assert_eq!(
        fields_of(theirs(), snapshot_at("2026-10-01T12:00:00.000Z")),
        ["meta.played_at"]
    );
    assert_eq!(
        fields_of(theirs(), snapshot_at("2026-10-01T12:00:01.000Z")),
        Vec::<String>::new()
    );
    for beyond in [
        "2026-10-01T11:59:59.999Z",
        "2026-10-01T12:00:01.001Z",
        "not a time",
    ] {
        assert_eq!(
            fields_of(theirs(), snapshot_at(beyond)),
            ["meta.played_at (beyond 1 s)"],
            "{beyond}"
        );
    }
}

// FIX NOW 4: a top-level key on one side only is a difference.
#[test]
fn a_top_level_key_on_one_side_only_is_a_difference() {
    let base = || serde_json::json!({"version": 1, "meta": {}, "games": []});
    let mut theirs = base();
    theirs["old"] = serde_json::json!(true);
    let mut ours = base();
    ours["new"] = serde_json::json!(true);
    assert_eq!(fields_of(theirs, ours), ["new", "old"]);
}
