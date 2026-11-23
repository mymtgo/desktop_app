//! Per-game detail import tests (Plan O, issue #177): each game's deck list
//! and opponent's cards seen (`game_player.deck_json`), its frames
//! (`game_timelines`) and the match's decoded game log (`game_logs`). 0.x
//! databases are built from `fixtures/0x/schema.sql` plus fake rows; fake
//! names start with Q.

mod common;

use std::path::{Path, PathBuf};

use common::v1_db;
use fixture_tools::TestDir;
use importer::compare_card_stats::{CompareReport, compare_card_stats};
use importer::{DetailReader, read_database};
use rusqlite::{Connection, params};
use store::game_sync::{FrameText, game_log, read_game_cards, timeline};
use store::import::import_matches;
use store::imported_detail::{
    DetailCard, DetailChange, DetailLogEntry, ImportedCardStat, ImportedDetail, ImportedGameDetail,
    ImportedHand, import_detail,
};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

const STARTED: &str = "2026-01-02 03:04:05";
const STARTED_MS: i64 = 1_767_323_045_000;
const ENDED: &str = "2026-01-02 03:34:05";

const TOKEN: &str = "q-detail-token";

/// A local deck: 4 of 4001 main, 2 of 4003 in the sideboard.
const LOCAL_DECK: &str = r#"[{"mtgo_id":4001,"quantity":4,"sideboard":false},{"mtgo_id":4003,"quantity":2,"sideboard":true}]"#;
/// Cards the opponent showed.
const OPPONENT_CARDS: &str = r#"[{"mtgo_id":5001,"quantity":1,"sideboard":false},{"mtgo_id":5002,"quantity":2,"sideboard":false}]"#;

const FRAME_A: &str = r#"{"Cards":[{"Id":1,"CatalogID":4001,"Zone":"Hand"}],"Players":[]}"#;
const FRAME_B: &str = r#"{"Cards":[{"Id":1,"CatalogID":4001,"Zone":"Battlefield"}],"Players":[]}"#;

fn legacy_db(dir: &TestDir, schema: &str) -> (PathBuf, Connection) {
    let path = dir.join("nativephp.sqlite");
    let conn = Connection::open(&path).expect("open 0.x test database");
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("no foreign keys");
    conn.execute_batch(schema).expect("0.x schema");
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');",
    )
    .expect("players");
    (path, conn)
}

fn add_match(conn: &Connection, id: i64, token: &str, mtgo_id: &str) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won, games_lost,
             started_at, ended_at, state)
         VALUES (?1, ?2, ?3, 'CMODERN', 'Constructed', 2, 1, ?4, ?5, 'complete')",
        params![id, token, mtgo_id, STARTED, ENDED],
    )
    .expect("match");
}

fn add_game(conn: &Connection, id: i64, match_id: i64, mtgo_id: &str, started: &str) {
    conn.execute(
        "INSERT INTO games (id, match_id, mtgo_id, won, started_at) VALUES (?1, ?2, ?3, 1, ?4)",
        params![id, match_id, mtgo_id, started],
    )
    .expect("game");
}

fn add_seat(conn: &Connection, game_id: i64, player_id: i64, is_local: bool, deck: Option<&str>) {
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local, deck_json)
         VALUES (?1, ?2, 1, ?3, ?4)",
        params![game_id, player_id, is_local, deck],
    )
    .expect("game_player");
}

fn add_frame(conn: &Connection, id: i64, game_id: i64, timestamp: &str, content: &str) {
    conn.execute(
        "INSERT INTO game_timelines (id, game_id, timestamp, content) VALUES (?1, ?2, ?3, ?4)",
        params![id, game_id, timestamp, content],
    )
    .expect("frame");
}

fn add_log(conn: &Connection, token: &str, entries: &str) {
    conn.execute(
        "INSERT INTO game_logs (match_token, file_path, decoded_entries) VALUES (?1, 'Q.dat', ?2)",
        params![token, entries],
    )
    .expect("game log");
}

fn frame(time: &str, text: &str) -> FrameText {
    FrameText {
        time: time.to_string(),
        text: text.to_string(),
    }
}

fn card(mtgo_id: u32, quantity: u32, sideboard: bool) -> DetailCard {
    DetailCard {
        mtgo_id,
        quantity,
        sideboard,
    }
}

/// The detail of 0.x match `match_id` with this token.
fn read_detail(path: &Path, match_id: i64, token: &str) -> ImportedDetail {
    DetailReader::open(path)
        .expect("open")
        .read(match_id, token, "Qlocal")
        .expect("read")
}

/// Match 1 (TOKEN): game 1 (Game ID 7001) with decks and frames, game 2
/// (7002) with decks only, and a game log.
fn full_match(conn: &Connection) {
    add_match(conn, 1, TOKEN, "4242");
    add_game(conn, 10, 1, "7001", STARTED);
    add_game(conn, 11, 1, "7002", "2026-01-02 03:20:00");
    for game in [10, 11] {
        add_seat(conn, game, 1, true, Some(LOCAL_DECK));
        add_seat(conn, game, 2, false, Some(OPPONENT_CARDS));
    }
    // Inserted out of order: frames go by timestamp, then id. A A B A keeps
    // A B A: only a repeat of the frame just before is dropped.
    add_frame(conn, 4, 10, "2026-01-02 03:04:09", FRAME_A);
    add_frame(conn, 1, 10, "2026-01-02 03:04:06", FRAME_A);
    add_frame(conn, 3, 10, "2026-01-02 03:04:08", FRAME_B);
    add_frame(conn, 2, 10, "2026-01-02 03:04:06", FRAME_A);
    add_log(
        conn,
        TOKEN,
        r#"[{"timestamp":"2026-01-02T05:04:05+02:00","message":"Qlocal rolled a 6."},
            {"timestamp":"2026-01-02T03:04:07.250Z","message":"Qlocal chooses to play first."}]"#,
    );
}

fn full_detail() -> ImportedDetail {
    ImportedDetail {
        games: vec![
            ImportedGameDetail {
                mtgo_game_id: Some(7001),
                game_number: 1,
                local_deck: vec![card(4001, 4, false), card(4003, 2, true)],
                opponent_cards: vec![card(5001, 1, false), card(5002, 2, false)],
                // Each kept frame with its time of day (replays spec ruling 5).
                frames: vec![
                    frame("03:04:06", FRAME_A),
                    frame("03:04:08", FRAME_B),
                    frame("03:04:09", FRAME_A),
                ],
                // 0.x's column default.
                local_starting_hand_size: Some(7),
                opp_starting_hand_size: Some(7),
                ..ImportedGameDetail::default()
            },
            ImportedGameDetail {
                mtgo_game_id: Some(7002),
                game_number: 2,
                local_deck: vec![card(4001, 4, false), card(4003, 2, true)],
                opponent_cards: vec![card(5001, 1, false), card(5002, 2, false)],
                frames: Vec::new(),
                local_starting_hand_size: Some(7),
                opp_starting_hand_size: Some(7),
                ..ImportedGameDetail::default()
            },
        ],
        log: vec![
            DetailLogEntry {
                at_ms: STARTED_MS,
                message: "Qlocal rolled a 6.".to_string(),
            },
            DetailLogEntry {
                at_ms: STARTED_MS + 2_250,
                message: "Qlocal chooses to play first.".to_string(),
            },
        ],
    }
}

#[test]
fn reads_each_game_s_decks_and_frames_and_the_match_log() {
    let dir = TestDir::new("detail-read-all");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    full_match(&conn);
    // Another match's rows never mix in.
    add_match(&conn, 2, "q-other", "4243");
    add_game(&conn, 20, 2, "8001", STARTED);
    add_seat(&conn, 20, 1, true, Some(OPPONENT_CARDS));
    add_frame(&conn, 9, 20, "2026-01-02 03:04:07", FRAME_B);
    add_log(
        &conn,
        "q-other",
        r#"[{"timestamp":"2026-01-02T03:04:05Z","message":"Q other"}]"#,
    );

    assert_eq!(read_detail(&path, 1, TOKEN), full_detail());
    assert_eq!(
        read_detail(&path, 99, "q-unknown"),
        ImportedDetail::default()
    );
}

#[test]
fn games_are_numbered_as_the_match_import_numbers_them() {
    let dir = TestDir::new("detail-read-numbers");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    // Game order is start time, then id; a repeated Game ID is dropped as
    // the match import drops it, and numbers stay as read.
    add_game(&conn, 12, 1, "7003", "2026-01-02 03:30:00");
    add_game(&conn, 10, 1, "7001", STARTED);
    add_game(&conn, 11, 1, "7001", "2026-01-02 03:20:00");
    for game in [10, 11, 12] {
        add_seat(&conn, game, 1, true, Some(LOCAL_DECK));
    }
    let detail = read_detail(&path, 1, TOKEN);
    let numbers: Vec<(Option<u64>, u32)> = detail
        .games
        .iter()
        .map(|game| (game.mtgo_game_id, game.game_number))
        .collect();
    assert_eq!(numbers, vec![(Some(7001), 1), (Some(7003), 3)]);

    let read = read_database(&path).expect("read");
    let imported: Vec<(Option<i64>, u32)> = read.matches[0]
        .games
        .iter()
        .map(|game| (game.mtgo_game_id, game.game_number))
        .collect();
    assert_eq!(imported, vec![(Some(7001), 1), (Some(7003), 3)]);
}

#[test]
fn broken_json_skips_only_that_piece() {
    let dir = TestDir::new("detail-read-broken");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    add_seat(&conn, 10, 1, true, Some("not json"));
    // Rows without a readable id are skipped; a missing quantity is one
    // copy (0.x showed it so); text numbers and a text sideboard flag read.
    add_seat(
        &conn,
        10,
        2,
        false,
        Some(
            r#"[{"quantity":3},{"mtgo_id":"5001"},{"mtgo_id":5002,"quantity":"2","sideboard":"true"},
                {"mtgo_id":5003,"quantity":0},"junk"]"#,
        ),
    );
    add_frame(&conn, 1, 10, "2026-01-02 03:04:06", FRAME_A);
    add_frame(&conn, 2, 10, "2026-01-02 03:04:07", "{broken");
    add_frame(&conn, 3, 10, "2026-01-02 03:04:08", "[1,2]");
    // Broken frames are skipped before repeats are found; a repeat keeps
    // the first frame's time.
    add_frame(&conn, 4, 10, "2026-01-02 03:04:09", FRAME_A);
    // Written twice (a JSON string holding JSON) is read through.
    let twice = serde_json::to_string(FRAME_B).expect("json string");
    add_frame(&conn, 5, 10, "2026-01-02 03:04:10", &twice);
    // The longest log row is broken: the next readable one is used. A line
    // with an unreadable time or message is skipped.
    let cut_off = format!(
        r#"[{{"timestamp":"2026-01-02T03:04:05Z","message":"Q cut off {}"#,
        "q".repeat(400)
    );
    add_log(&conn, TOKEN, &cut_off);
    add_log(
        &conn,
        TOKEN,
        r#"[{"timestamp":"not a time","message":"Q no time"},
            {"timestamp":"2026-01-02T03:04:06+00:00","message":7},
            {"timestamp":"2026-01-02T03:04:07+00:00","message":"Q kept"}]"#,
    );

    let detail = read_detail(&path, 1, TOKEN);
    assert_eq!(
        detail,
        ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7001),
                game_number: 1,
                local_deck: Vec::new(),
                opponent_cards: vec![card(5001, 1, false), card(5002, 2, true)],
                frames: vec![frame("03:04:06", FRAME_A), frame("03:04:10", FRAME_B)],
                local_starting_hand_size: Some(7),
                opp_starting_hand_size: Some(7),
                ..ImportedGameDetail::default()
            }],
            log: vec![DetailLogEntry {
                at_ms: STARTED_MS + 2_000,
                message: "Q kept".to_string(),
            }],
        }
    );
}

#[test]
fn an_unreadable_log_is_no_log() {
    let dir = TestDir::new("detail-read-no-log");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_log(&conn, TOKEN, "{not a list");
    add_log(&conn, TOKEN, r#"{"timestamp":"2026-01-02T03:04:05Z"}"#);
    assert_eq!(read_detail(&path, 1, TOKEN), ImportedDetail::default());
}

#[test]
fn an_old_0x_without_timelines_logs_or_deck_lists_reads_empty_games() {
    let dir = TestDir::new("detail-read-old");
    let old_schema = SCHEMA
        .lines()
        .filter(|line| !line.contains("game_timelines") && !line.contains("game_logs"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(r#""deck_json" text, "#, "");
    assert!(!old_schema.contains("deck_json"));
    let (path, conn) = legacy_db(&dir, &old_schema);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local) VALUES (10, 1, 1, 1)",
        [],
    )
    .expect("seat");
    assert_eq!(
        read_detail(&path, 1, TOKEN),
        ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7001),
                game_number: 1,
                local_starting_hand_size: Some(7),
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        }
    );
}

// Replays spec section 10: 0.x frame times come in three shapes. Time of
// day (`HH:MM:SS`, `HH:MM:SS.mmm`) is kept; an older `Y-m-d H:i:s` row keeps
// its time part (ruling 5). Frames still go by the stored text, then id.
#[test]
fn the_three_timestamp_shapes_are_stored_normalised() {
    let dir = TestDir::new("detail-read-time-shapes");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    add_seat(&conn, 10, 1, true, None);
    add_frame(&conn, 1, 10, "03:04:06", FRAME_A);
    add_frame(&conn, 2, 10, "03:04:07.250", FRAME_B);
    add_frame(&conn, 3, 10, "2026-01-02 03:04:09", FRAME_A);
    let detail = read_detail(&path, 1, TOKEN);
    // Text order: "03:..." before "2026-...".
    assert_eq!(
        detail.games[0].frames,
        vec![
            frame("03:04:06", FRAME_A),
            frame("03:04:07.250", FRAME_B),
            frame("03:04:09", FRAME_A),
        ]
    );

    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    import_detail(&mut v1, TOKEN, &detail).expect("detail");
    let games = game_ids(&v1);
    assert_eq!(
        timeline(&v1, games[0]).expect("frames"),
        Some(detail.games[0].frames.clone())
    );
}

// Replays spec section 4 (ruling 4): the import drops a repeated frame with
// the one shared rule (`replay::drop_repeats`), so a game keeps its last
// frame even when it repeats, and every kept frame has its time.
#[test]
fn an_imported_game_keeps_its_last_frame_when_it_repeats() {
    let dir = TestDir::new("detail-read-last-frame");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    add_seat(&conn, 10, 1, true, None);
    add_frame(&conn, 1, 10, "03:04:06", FRAME_A);
    add_frame(&conn, 2, 10, "03:04:07", FRAME_B);
    add_frame(&conn, 3, 10, "03:04:08", FRAME_B);
    add_frame(&conn, 4, 10, "03:04:09", FRAME_B);
    let detail = read_detail(&path, 1, TOKEN);
    assert_eq!(
        detail.games[0].frames,
        vec![
            frame("03:04:06", FRAME_A),
            frame("03:04:07", FRAME_B),
            frame("03:04:09", FRAME_B),
        ]
    );

    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    import_detail(&mut v1, TOKEN, &detail).expect("detail");
    let games = game_ids(&v1);
    let stored = timeline(&v1, games[0])
        .expect("frames")
        .expect("a timeline");
    assert_eq!(stored, detail.games[0].frames);
    let (frame_count, raw, times): (i64, Vec<u8>, Vec<u8>) = v1
        .query_row(
            "SELECT frame_count, raw, times FROM game_timelines",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("the row");
    let unpack = |blob: Vec<u8>| -> Vec<String> {
        serde_json::from_slice(&zstd_decode(&blob)).expect("a JSON list")
    };
    let raw = unpack(raw);
    let times = unpack(times);
    assert_eq!(frame_count, 3);
    assert_eq!(raw.len(), times.len(), "times line up with raw");
    assert_eq!(times, vec!["03:04:06", "03:04:07", "03:04:09"]);
}

fn zstd_decode(blob: &[u8]) -> Vec<u8> {
    zstd::decode_all(blob).expect("zstd")
}

#[test]
fn an_imported_match_gets_its_detail_and_a_rerun_writes_nothing() {
    let dir = TestDir::new("detail-import");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    full_match(&conn);
    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");

    assert_eq!(read.match_ids.get(TOKEN), Some(&1));
    let mut reader = DetailReader::open(&path).expect("open");
    let detail = reader.read(1, TOKEN, "Qlocal").expect("read");
    assert_eq!(
        import_detail(&mut v1, TOKEN, &detail)
            .expect("store")
            .change,
        DetailChange::Added
    );
    let games = game_ids(&v1);
    let first = read_game_cards(&v1, games[0]).expect("rows");
    assert_eq!(first.cards.len(), 4, "two local cards, two opponent cards");
    assert_eq!(
        timeline(&v1, games[0]).expect("frames"),
        Some(full_detail().games[0].frames.clone())
    );
    let match_id: i64 = v1
        .query_row("SELECT id FROM matches", [], |row| row.get(0))
        .expect("match");
    assert_eq!(
        game_log(&v1, match_id).expect("log").map(|log| log.len()),
        Some(2)
    );

    let before = all_rows(&v1);
    let again = reader.read(1, TOKEN, "Qlocal").expect("read again");
    assert_eq!(
        import_detail(&mut v1, TOKEN, &again).expect("rerun").change,
        DetailChange::Unchanged
    );
    assert_eq!(all_rows(&v1), before, "a rerun writes nothing");
}

/// The v1 game ids, in game order.
fn game_ids(v1: &Connection) -> Vec<i64> {
    let mut stmt = v1
        .prepare("SELECT id FROM games ORDER BY game_number")
        .expect("prepare");
    let rows = stmt.query_map([], |row| row.get(0)).expect("query");
    rows.map(|row| row.expect("row")).collect()
}

/// Every per-game row and game log, as text.
fn all_rows(v1: &Connection) -> Vec<String> {
    let mut rows = Vec::new();
    for table in [
        "game_cards",
        "game_card_casts",
        "game_mulligan_cards",
        "game_timelines",
        "game_logs",
        "games",
    ] {
        let mut stmt = v1
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .expect("prepare");
        let columns = stmt.column_count();
        let found = stmt
            .query_map([], |row| {
                let mut values = Vec::new();
                for index in 0..columns {
                    let value: rusqlite::types::Value = row.get(index)?;
                    values.push(format!("{value:?}"));
                }
                Ok(values.join("|"))
            })
            .expect("query");
        for row in found {
            rows.push(format!("{table}: {}", row.expect("row")));
        }
    }
    rows
}

/// Game 10 of match 1 with card stats rows: a local card with counters and
/// casts (oracle `q-oracle-bolt`, two printings in v1), an opponent card,
/// and a card v1 has no printing of.
fn card_stats_match(conn: &Connection) {
    add_match(conn, 1, TOKEN, "4242");
    add_game(conn, 10, 1, "7001", STARTED);
    add_seat(conn, 10, 1, true, None);
    add_seat(conn, 10, 2, false, None);
    conn.execute_batch(
        "INSERT INTO card_game_stats (oracle_id, game_id, deck_version_id, quantity, kept, seen,
             won, cast, played, activated, kicked, free_cast, sided_in, pregame_played,
             opponent, cast_turn, hand_seen, has_zone_data)
         VALUES ('q-oracle-bolt', 10, 1, 4, 2, 3, 1, 2, 1, 0, 1, NULL, 1, 0, 0, 2, 2, 1),
                ('q-oracle-goyf', 10, 1, 0, 0, 2, 1, 1, NULL, NULL, NULL, 1, 0, 1, 1, NULL, 0, 0),
                ('q-oracle-none', 10, 1, 1, 1, 1, 1, 0, NULL, NULL, NULL, NULL, 0, 0, 0, NULL, 0, 0);",
    )
    .expect("card stats");
}

fn v1_with_cards(dir: &TestDir) -> Connection {
    let v1 = v1_db(dir);
    v1.execute_batch(
        "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
             (4011, 'q-oracle-bolt', 'Qbolt'), (4010, 'q-oracle-bolt', 'Qbolt'),
             (5001, 'q-oracle-goyf', 'Qgoyf');",
    )
    .expect("cards");
    v1
}

#[test]
fn card_game_stats_import_maps_oracle_ids_and_counters() {
    let dir = TestDir::new("detail-card-stats");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    card_stats_match(&conn);
    let detail = read_detail(&path, 1, TOKEN);
    let stats = &detail.games[0].card_stats;
    assert_eq!(stats.len(), 3);
    assert_eq!(
        stats[0],
        ImportedCardStat {
            oracle_id: "q-oracle-bolt".to_string(),
            opponent: false,
            quantity: 4,
            kept: 2,
            seen: 3,
            sided_in: true,
            cast: 2,
            played: Some(1),
            activated: Some(0),
            cast_turn: Some(2),
            hand_seen: 2,
            has_zone_data: true,
            methods: vec![("kicked".to_string(), 1)],
            ..ImportedCardStat::default()
        }
    );

    let mut v1 = v1_with_cards(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    let import = import_detail(&mut v1, TOKEN, &detail).expect("store");
    assert_eq!(import.change, DetailChange::Added);
    assert_eq!(import.card_stats_skipped, 1, "no v1 card has the oracle");
    let cards = read_game_cards(&v1, game_ids(&v1)[0]).expect("rows");
    let bolt = cards
        .row(store::game_sync::Side::Local, 4010)
        .expect("the lowest catalog id of the oracle");
    assert_eq!(
        (
            bolt.main_qty,
            bolt.starting_hand,
            bolt.seen,
            bolt.cast,
            bolt.played
        ),
        (4, 2, 3, 2, Some(1))
    );
    assert_eq!(
        (bolt.sided_in, bolt.cast_turn, bolt.hand_seen),
        (1, Some(2), 2)
    );
    let goyf = cards
        .row(store::game_sync::Side::Opponent, 5001)
        .expect("opponent");
    assert_eq!((goyf.seen, goyf.cast, goyf.pregame_played), (2, 1, 1));
    let methods: Vec<(u32, String, u32)> = cards
        .casts
        .iter()
        .map(|cast| (cast.catalog_id, cast.method.clone(), cast.count))
        .collect();
    assert_eq!(
        methods,
        vec![
            (4010, "kicked".to_string(), 1),
            (5001, "free_cast".to_string(), 1)
        ]
    );

    let before = all_rows(&v1);
    let again = read_detail(&path, 1, TOKEN);
    assert_eq!(
        import_detail(&mut v1, TOKEN, &again).expect("rerun").change,
        DetailChange::Unchanged
    );
    assert_eq!(all_rows(&v1), before, "a rerun adds nothing");
}

// Cards spec section 13: the dev-only report of v1's card rows against
// 0.x's `card_game_stats`, per imported game, counts only.
#[test]
fn compare_card_stats_counts_differences_without_names() {
    let dir = TestDir::new("detail-compare-card-stats");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    card_stats_match(&conn);
    let detail = read_detail(&path, 1, TOKEN);
    let mut v1 = v1_with_cards(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    import_detail(&mut v1, TOKEN, &detail).expect("store");

    let report = compare_card_stats(&v1, &conn).expect("compare");
    assert_eq!(
        report,
        CompareReport {
            games: 1,
            games_with_differences: 1,
            rows_equal: 2,
            only_in_0x: 1,
            ..CompareReport::default()
        },
        "bolt and goyf as imported; 0.x's row for an oracle v1 has no card of"
    );

    // v1 counts a cast more, and shows a card 0.x had no row for.
    let game = game_ids(&v1)[0];
    v1.execute(
        "UPDATE game_cards SET \"cast\" = \"cast\" + 1, hand_seen = 0 WHERE catalog_id = 4010",
        [],
    )
    .expect("a changed row");
    v1.execute(
        "INSERT INTO game_cards (game_id, side, catalog_id, seen) VALUES (?1, 'opponent', 6000, 1)",
        [game],
    )
    .expect("a row only v1 has");
    let report = compare_card_stats(&v1, &conn).expect("compare");
    assert_eq!(
        report,
        CompareReport {
            games: 1,
            games_with_differences: 1,
            rows_equal: 1,
            rows_differing: 1,
            differing_by_column: [("cast".to_string(), 1), ("hand_seen".to_string(), 1)]
                .into_iter()
                .collect(),
            only_in_v1: 1,
            only_in_0x: 1,
            ..CompareReport::default()
        }
    );
    // An alternative cast count that differs is found under its 0.x column.
    v1.execute(
        "UPDATE game_card_casts SET count = count + 1 WHERE method = 'kicked'",
        [],
    )
    .expect("a changed alternative cast");
    let report = compare_card_stats(&v1, &conn).expect("compare");
    assert_eq!(report.differing_by_column.get("kicked"), Some(&1));
    assert_eq!(report.differing_by_column.get("free_cast"), None);
    assert_eq!(
        report.rows_differing, 1,
        "the row differs in cast, hand_seen and kicked"
    );
    let printed = report.lines().join("\n");
    for name in ["Qbolt", "Qgoyf", "Qlocal", "Qopponent", "q-oracle", TOKEN] {
        assert!(!printed.contains(name), "{name} in {printed}");
    }
    assert!(printed.contains("cast"), "{printed}");
}

// S3 review I1: a repeated Game ID is dropped and the games keep the
// import's numbers (1 and 3), so the third game is compared, not missing.
#[test]
fn compare_card_stats_numbers_games_as_the_import_does() {
    let dir = TestDir::new("detail-compare-repeated-game");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    card_stats_match(&conn);
    add_game(&conn, 11, 1, "7001", "2026-01-02 03:10:05");
    add_game(&conn, 12, 1, "7002", "2026-01-02 03:20:05");
    add_seat(&conn, 12, 1, true, None);
    add_seat(&conn, 12, 2, false, None);
    conn.execute_batch(
        "INSERT INTO card_game_stats (oracle_id, game_id, deck_version_id, quantity, kept, seen,
             won, cast, played, activated, kicked, free_cast, sided_in, pregame_played,
             opponent, cast_turn, hand_seen, has_zone_data)
         VALUES ('q-oracle-bolt', 12, 1, 4, 1, 2, 0, 1, 1, 0, NULL, NULL, 0, 0, 0, 3, 1, 1);",
    )
    .expect("third game stats");
    let detail = read_detail(&path, 1, TOKEN);
    let mut v1 = v1_with_cards(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    import_detail(&mut v1, TOKEN, &detail).expect("store");

    let report = compare_card_stats(&v1, &conn).expect("compare");
    assert_eq!(
        (report.games, report.games_missing_in_v1),
        (2, 0),
        "games 1 and 3 are both compared"
    );
    assert_eq!(
        report.rows_equal, 3,
        "bolt and goyf in game 1, bolt in game 3"
    );
}

#[test]
fn opening_hand_json_and_deck_json_become_rows() {
    let dir = TestDir::new("detail-hand-json");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local, deck_json,
             opening_hand_json, starting_hand_size, mulligan_count)
         VALUES (10, 1, 1, 1, ?1, ?2, 6, 1), (10, 2, 2, 0, ?3, NULL, 7, 0)",
        params![
            LOCAL_DECK,
            r#"{"kept":[4001,4001,4001,4003,4004,4004],"bottomed":[4004],"mulligans":[[4001,4001,4001,4001,4004,4004,4004]]}"#,
            OPPONENT_CARDS
        ],
    )
    .expect("seats");
    let detail = read_detail(&path, 1, TOKEN);
    let game = &detail.games[0];
    assert_eq!(
        game.local_hand,
        Some(ImportedHand {
            starting: vec![4001, 4001, 4001, 4003, 4004, 4004],
            bottomed: vec![4004],
            mulligans: vec![vec![4001, 4001, 4001, 4001, 4004, 4004, 4004]],
        })
    );
    assert_eq!(
        (game.local_starting_hand_size, game.opp_starting_hand_size),
        (Some(6), Some(7))
    );

    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    assert_eq!(
        import_detail(&mut v1, TOKEN, &detail)
            .expect("store")
            .change,
        DetailChange::Added
    );
    let cards = read_game_cards(&v1, game_ids(&v1)[0]).expect("rows");
    assert_eq!(
        (cards.opening_hand_size, cards.starting_hand_size),
        (Some(7), Some(6))
    );
    assert_eq!(
        (cards.opp_opening_hand_size, cards.opp_starting_hand_size),
        (Some(7), Some(7))
    );
    let local = |id| {
        cards
            .row(store::game_sync::Side::Local, id)
            .expect("row")
            .clone()
    };
    assert_eq!(
        (
            local(4001).main_qty,
            local(4001).opening_hand,
            local(4001).starting_hand
        ),
        (4, 3, 3)
    );
    assert_eq!(
        (
            local(4004).opening_hand,
            local(4004).bottomed,
            local(4004).starting_hand
        ),
        (3, 1, 2)
    );
    assert_eq!((local(4003).side_qty, local(4003).starting_hand), (2, 1));
    let seen: Vec<(u32, u32)> = cards
        .cards
        .iter()
        .filter(|row| row.side == store::game_sync::Side::Opponent)
        .map(|row| (row.catalog_id, row.seen))
        .collect();
    assert_eq!(seen, vec![(5001, 1), (5002, 2)]);
    assert_eq!(cards.mulligans, vec![(1, 4001, 4), (1, 4004, 3)]);
    let before = all_rows(&v1);
    assert_eq!(
        import_detail(&mut v1, TOKEN, &detail)
            .expect("rerun")
            .change,
        DetailChange::Unchanged
    );
    assert_eq!(all_rows(&v1), before);
}

#[test]
fn the_read_names_the_0x_row_of_each_kept_match() {
    let dir = TestDir::new("detail-read-match-ids");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    // The first row with the token is still in progress, so the second is
    // the one imported: its detail is the one to read.
    add_match(&conn, 1, "q-twice", "4242");
    conn.execute("UPDATE matches SET state = 'in_progress' WHERE id = 1", [])
        .expect("in progress");
    add_match(&conn, 2, "q-twice", "4243");
    add_game(&conn, 20, 2, "8001", STARTED);
    add_seat(&conn, 20, 1, true, Some(LOCAL_DECK));
    let read = read_database(&path).expect("read");
    assert_eq!(read.matches.len(), 1);
    assert_eq!(read.match_ids.get("q-twice"), Some(&2));
    assert_eq!(read.match_ids.len(), 1);
}

/// Issue #190 item 2: with no seat marked local, the seat with the local
/// player's name is the local one, so the player's own cards never become
/// the opponent's.
#[test]
fn without_a_local_seat_the_local_player_s_named_seat_is_the_local_one() {
    let dir = TestDir::new("detail-read-no-local-seat");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    add_seat(&conn, 10, 1, false, Some(LOCAL_DECK));
    add_seat(&conn, 10, 2, false, Some(OPPONENT_CARDS));
    let detail = read_detail(&path, 1, TOKEN);
    assert_eq!(
        (
            detail.games[0].local_deck.clone(),
            detail.games[0].opponent_cards.clone()
        ),
        (
            vec![card(4001, 4, false), card(4003, 2, true)],
            vec![card(5001, 1, false), card(5002, 2, false)]
        )
    );
}

/// Issue #190 item 7: a log row with no readable line loses to a shorter
/// one with readable lines.
#[test]
fn the_longest_log_row_wins_only_with_a_readable_line() {
    let dir = TestDir::new("detail-read-log-unreadable-lines");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    let unreadable = format!(
        r#"[{{"timestamp":"not a time","message":"Q {}"}}]"#,
        "q".repeat(400)
    );
    add_log(&conn, TOKEN, &unreadable);
    add_log(
        &conn,
        TOKEN,
        r#"[{"timestamp":"2026-01-02T03:04:07+00:00","message":"Q kept"}]"#,
    );
    assert_eq!(
        read_detail(&path, 1, TOKEN).log,
        vec![DetailLogEntry {
            at_ms: STARTED_MS + 2_000,
            message: "Q kept".to_string(),
        }]
    );
}

/// Issue #190 item 7: a match imported before 0.x had its detail gains it on
/// a later run.
#[test]
fn an_imported_match_gains_its_detail_on_a_later_run() {
    let dir = TestDir::new("detail-import-later");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, TOKEN, "4242");
    add_game(&conn, 10, 1, "7001", STARTED);
    add_seat(&conn, 10, 1, true, None);
    add_seat(&conn, 10, 2, false, None);
    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 10).expect("matches");
    let first = DetailReader::open(&path)
        .expect("open")
        .read(1, TOKEN, "Qlocal")
        .expect("read");
    let change = import_detail(&mut v1, TOKEN, &first).expect("store").change;
    assert_eq!(change, DetailChange::Added, "the hand sizes alone");
    conn.execute(
        "UPDATE game_player SET deck_json = ?1 WHERE is_local = 1",
        [LOCAL_DECK],
    )
    .expect("a deck list later");
    add_log(
        &conn,
        TOKEN,
        r#"[{"timestamp":"2026-01-02T03:04:07+00:00","message":"Qlocal rolled a 6."}]"#,
    );
    import_matches(&mut v1, &read_database(&path).expect("read").matches, 20).expect("rerun");
    let later = DetailReader::open(&path)
        .expect("open")
        .read(1, TOKEN, "Qlocal")
        .expect("read");
    assert_eq!(
        import_detail(&mut v1, TOKEN, &later).expect("store").change,
        DetailChange::Updated
    );
    let games = game_ids(&v1);
    let rows = read_game_cards(&v1, games[0]).expect("rows");
    assert!(
        rows.row(store::game_sync::Side::Local, 4001).is_some(),
        "the deck list arrived: {rows:?}"
    );
    let match_id: i64 = v1
        .query_row("SELECT id FROM matches", [], |row| row.get(0))
        .expect("match");
    assert_eq!(
        game_log(&v1, match_id).expect("log").map(|log| log.len()),
        Some(1)
    );
}
