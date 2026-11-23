//! Importer tests: 0.x databases are built from `fixtures/0x/schema.sql` (the
//! 0.x schema, no data) plus fake rows. Fake names start with Q.

mod common;

use std::path::{Path, PathBuf};

use common::{count, legacy_db, v1_db};
use fixture_tools::TestDir;
use importer::{
    ImportError, Os, Skipped, candidate_paths, filter_settings, read_database, settings_path,
};
use rusqlite::{Connection, params};
use store::import::{ImportCounts, ImportedAccount, ImportedGame, ImportedMatch, import_matches};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

const STARTED: &str = "2026-01-02 03:04:05";
const STARTED_MS: i64 = 1_767_323_045_000;
const ENDED: &str = "2026-01-02 03:34:05";
const ENDED_MS: i64 = 1_767_324_845_000;

/// The 0.x schema without one column (and the indexes that use it).
fn schema_without(column_definition: &str, column: &str) -> String {
    let removed = SCHEMA.replace(column_definition, "");
    assert_ne!(removed, SCHEMA, "the column definition is in the schema");
    let mut kept = String::new();
    for line in removed.lines() {
        let indexes_column = line.starts_with("CREATE")
            && line.contains(" INDEX ")
            && (line.contains(&format!("\"{column}\")"))
                || line.contains(&format!("\"{column}\",")));
        if !indexes_column {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

/// A 0.x database at `<dir>/nativephp.sqlite` built from `schema`.
fn add_player(conn: &Connection, id: i64, username: &str) {
    conn.execute(
        "INSERT INTO players (id, username) VALUES (?1, ?2)",
        params![id, username],
    )
    .expect("player");
}

/// A match with the required columns only; tests set the others with UPDATE.
fn add_match(conn: &Connection, id: i64, token: &str, mtgo_id: &str, state: &str) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won, games_lost,
             started_at, ended_at, state)
         VALUES (?1, ?2, ?3, 'CMODERN', 'Constructed', 2, 1, ?4, ?5, ?6)",
        params![id, token, mtgo_id, STARTED, ENDED, state],
    )
    .expect("match");
}

fn add_game(
    conn: &Connection,
    id: i64,
    match_id: i64,
    mtgo_id: &str,
    won: Option<i64>,
    started: &str,
) {
    conn.execute(
        "INSERT INTO games (id, match_id, mtgo_id, won, started_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, match_id, mtgo_id, won, started],
    )
    .expect("game");
}

fn add_seat(conn: &Connection, game_id: i64, player_id: i64, is_local: bool) {
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local) VALUES (?1, ?2, 1, ?3)",
        params![game_id, player_id, is_local],
    )
    .expect("game_player");
}

/// A complete match `id` with one won game (id `id * 10`) between Qlocal (1)
/// and Qopponent (2). The players must exist.
fn add_simple_match(conn: &Connection, id: i64, token: &str, mtgo_id: &str) {
    add_match(conn, id, token, mtgo_id, "complete");
    add_game(conn, id * 10, id, &(id * 10).to_string(), Some(1), STARTED);
    add_seat(conn, id * 10, 1, true);
    add_seat(conn, id * 10, 2, false);
}

fn add_two_players(conn: &Connection) {
    add_player(conn, 1, "Qlocal");
    add_player(conn, 2, "Qopponent");
}

fn tokens(matches: &[ImportedMatch]) -> Vec<String> {
    matches.iter().map(|found| found.token.clone()).collect()
}

fn only_match(path: &Path) -> ImportedMatch {
    let result = read_database(path).expect("read");
    assert_eq!(result.matches.len(), 1, "one match: {:?}", result.matches);
    result.matches[0].clone()
}

#[test]
fn candidate_paths_follow_the_nativephp_layout() {
    let windows = candidate_paths(
        Some(Path::new("C:/Users/Q")),
        Some(Path::new("C:/Users/Q/AppData/Roaming")),
        Os::Windows,
    );
    assert_eq!(
        windows,
        vec![
            PathBuf::from("C:/Users/Q/AppData/Roaming/mymtgo/database/database.sqlite"),
            PathBuf::from("C:/Users/Q/AppData/Roaming/mymtgo/database/nativephp.sqlite"),
            PathBuf::from("C:/Users/Q/AppData/Roaming/com.mymtgo.app/database/database.sqlite"),
            PathBuf::from("C:/Users/Q/AppData/Roaming/com.mymtgo.app/database/nativephp.sqlite"),
        ]
    );
    let windows_no_appdata = candidate_paths(Some(Path::new("C:/Users/Q")), None, Os::Windows);
    assert_eq!(
        windows_no_appdata[0],
        PathBuf::from("C:/Users/Q/AppData/Roaming/mymtgo/database/database.sqlite")
    );

    let linux = candidate_paths(Some(Path::new("/home/q")), None, Os::Linux);
    assert_eq!(
        linux,
        vec![
            PathBuf::from("/home/q/.config/mymtgo/database/database.sqlite"),
            PathBuf::from("/home/q/.config/mymtgo/database/nativephp.sqlite"),
            PathBuf::from("/home/q/.config/com.mymtgo.app/database/database.sqlite"),
            PathBuf::from("/home/q/.config/com.mymtgo.app/database/nativephp.sqlite"),
        ]
    );
    let linux_xdg = candidate_paths(
        Some(Path::new("/home/q")),
        Some(Path::new("/xdg")),
        Os::Linux,
    );
    assert_eq!(
        linux_xdg[0],
        PathBuf::from("/xdg/mymtgo/database/database.sqlite"),
        "XDG_CONFIG_HOME wins over ~/.config"
    );

    let mac = candidate_paths(Some(Path::new("/Users/q")), None, Os::MacOs);
    assert_eq!(
        mac,
        vec![
            PathBuf::from("/Users/q/Library/Application Support/mymtgo/database/database.sqlite"),
            PathBuf::from("/Users/q/Library/Application Support/mymtgo/database/nativephp.sqlite"),
            PathBuf::from(
                "/Users/q/Library/Application Support/com.mymtgo.app/database/database.sqlite"
            ),
            PathBuf::from(
                "/Users/q/Library/Application Support/com.mymtgo.app/database/nativephp.sqlite"
            ),
        ]
    );
}

#[test]
fn the_config_folder_alone_is_enough_without_a_home_folder() {
    let windows = candidate_paths(None, Some(Path::new("C:/Q/Roaming")), Os::Windows);
    assert_eq!(
        windows[0],
        PathBuf::from("C:/Q/Roaming/mymtgo/database/database.sqlite")
    );
    let linux = candidate_paths(None, Some(Path::new("/xdg")), Os::Linux);
    assert_eq!(
        linux[0],
        PathBuf::from("/xdg/mymtgo/database/database.sqlite")
    );
    assert!(candidate_paths(None, None, Os::Windows).is_empty());
    assert!(candidate_paths(None, Some(Path::new("/xdg")), Os::MacOs).is_empty());
}

#[test]
fn settings_path_sits_beside_the_database() {
    assert_eq!(
        settings_path(Path::new(
            "/home/q/.config/mymtgo/database/nativephp.sqlite"
        )),
        Some(PathBuf::from(
            "/home/q/.config/mymtgo/storage/app/private/settings.json"
        ))
    );
    assert_eq!(settings_path(Path::new("nativephp.sqlite")), None);
}

#[test]
fn maps_a_complete_match_with_two_games() {
    let dir = TestDir::new("importer-map");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_match(&conn, 1, "tok-full", "123456", "complete");
    conn.execute(
        "UPDATE matches SET outcome = 'draw', notes = 'Q kept a risky hand',
             tournament_event_id = 77, tournament_round = 3 WHERE id = 1",
        [],
    )
    .expect("optional columns");
    // Inserted out of order: game 10 started second.
    add_game(&conn, 10, 1, "901", Some(0), "2026-01-02 03:20:00");
    add_game(&conn, 11, 1, "900", Some(1), "2026-01-02 03:05:00");
    conn.execute_batch(
        "UPDATE games SET ended_at = '2026-01-02T03:19:59.500', turn_count = 8 WHERE id = 11;
         UPDATE games SET ended_at = '2026-01-02 03:33:00', turn_count = 11 WHERE id = 10;
         INSERT INTO game_player (game_id, player_id, instance_id, is_local, on_play, dice_roll, mulligan_count)
             VALUES (11, 1, 1, 1, 1, 17, 0), (11, 2, 2, 0, 0, 4, 1),
                    (10, 2, 2, 0, 1, NULL, 2), (10, 1, 1, 1, 0, NULL, 1);",
    )
    .expect("games");

    let found = only_match(&path);
    assert_eq!(
        found,
        ImportedMatch {
            token: "tok-full".to_string(),
            mtgo_match_id: Some(123_456),
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: Some(77),
            tournament_round: Some(3),
            state: "complete".to_string(),
            outcome: "draw".to_string(),
            started_at_ms: STARTED_MS,
            ended_at_ms: Some(ENDED_MS),
            note: Some("Q kept a risky hand".to_string()),
            games: vec![
                ImportedGame {
                    mtgo_game_id: Some(900),
                    game_number: 1,
                    result: "win".to_string(),
                    on_play: Some(true),
                    local_mulligans: Some(0),
                    opp_mulligans: Some(1),
                    local_die_roll: Some(17),
                    opp_die_roll: Some(4),
                    turn_count: Some(8),
                    started_at_ms: Some(1_767_323_100_000),
                    ended_at_ms: Some(1_767_323_999_500),
                },
                ImportedGame {
                    mtgo_game_id: Some(901),
                    game_number: 2,
                    result: "loss".to_string(),
                    on_play: Some(false),
                    local_mulligans: Some(1),
                    opp_mulligans: Some(2),
                    local_die_roll: None,
                    opp_die_roll: None,
                    turn_count: Some(11),
                    started_at_ms: Some(1_767_324_000_000),
                    ended_at_ms: Some(1_767_324_780_000),
                },
            ],
            manual: None,
            games_won_imported: None,
            games_lost_imported: None,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        }
    );
}

#[test]
fn ended_and_abandoned_states_copy_over() {
    let dir = TestDir::new("importer-states");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-ended", "1");
    add_simple_match(&conn, 2, "tok-abandoned", "2");
    conn.execute_batch(
        "UPDATE matches SET state = 'ended' WHERE id = 1;
         UPDATE matches SET state = 'abandoned', ended_at = 'garbage' WHERE id = 2;
         UPDATE games SET started_at = 'garbage' WHERE id = 20;",
    )
    .expect("states");
    let result = read_database(&path).expect("read");
    let states: Vec<(String, Option<i64>)> = result
        .matches
        .iter()
        .map(|found| (found.state.clone(), found.ended_at_ms))
        .collect();
    assert_eq!(
        states,
        vec![
            ("ended".to_string(), Some(ENDED_MS)),
            ("abandoned".to_string(), None)
        ],
        "an end time that does not parse becomes None"
    );
    assert_eq!(result.matches[1].games[0].started_at_ms, None);
}

#[test]
fn outcome_falls_back_to_the_game_score_when_the_column_is_missing_or_null() {
    for (label, schema) in [
        ("null", SCHEMA.to_string()),
        (
            "missing",
            schema_without("\"outcome\" varchar, ", "outcome"),
        ),
    ] {
        let dir = TestDir::new(&format!("importer-outcome-{label}"));
        let (path, conn) = legacy_db(&dir, &schema);
        add_two_players(&conn);
        add_simple_match(&conn, 1, "tok-won", "1");
        add_simple_match(&conn, 2, "tok-lost", "2");
        add_simple_match(&conn, 3, "tok-level", "3");
        add_simple_match(&conn, 4, "tok-bad", "4");
        conn.execute_batch(
            "UPDATE matches SET games_won = 1, games_lost = 2 WHERE id = 2;
             UPDATE matches SET games_won = 1, games_lost = 1 WHERE id = 3;
             UPDATE matches SET games_won = 0, games_lost = 0 WHERE id = 4;",
        )
        .expect("scores");
        if label == "null" {
            conn.execute("UPDATE matches SET outcome = 'nonsense' WHERE id = 4", [])
                .expect("bad outcome");
        }
        let outcomes: Vec<String> = read_database(&path)
            .expect("read")
            .matches
            .iter()
            .map(|found| found.outcome.clone())
            .collect();
        assert_eq!(
            outcomes,
            vec!["win", "loss", "unknown", "unknown"],
            "outcome column {label}"
        );
    }
}

#[test]
fn skips_in_progress_matches_matches_without_a_local_player_and_duplicate_tokens() {
    let dir = TestDir::new("importer-skips");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-a", "1");
    conn.execute("UPDATE matches SET state = 'in_progress' WHERE id = 1", [])
        .expect("in progress");
    add_simple_match(&conn, 2, "tok-b", "2");
    conn.execute("UPDATE matches SET state = 'started' WHERE id = 2", [])
        .expect("started");
    add_match(&conn, 3, "tok-c", "3", "complete");
    add_match(&conn, 4, "tok-d", "4", "complete");
    add_game(&conn, 40, 4, "40", Some(1), STARTED);
    add_seat(&conn, 40, 2, false);
    add_simple_match(&conn, 5, "tok-e", "5");
    add_simple_match(&conn, 6, "tok-e", "6");
    add_simple_match(&conn, 7, "tok-f", "7");
    conn.execute(
        "UPDATE matches SET started_at = 'not a time' WHERE id = 7",
        [],
    )
    .expect("bad time");
    // The first tok-a was skipped as in progress, so this one is not a duplicate.
    add_simple_match(&conn, 8, "tok-a", "8");

    let result = read_database(&path).expect("read");
    assert_eq!(tokens(&result.matches), vec!["tok-e", "tok-a"]);
    assert_eq!(
        result.matches[0].mtgo_match_id,
        Some(5),
        "the first tok-e wins"
    );
    assert_eq!(
        result.skipped,
        Skipped {
            in_progress: 2,
            no_local_player: 2,
            duplicate_token: 1,
            bad_time: 1,
            ..Skipped::default()
        }
    );
}

#[test]
fn a_second_row_with_a_match_id_already_read_is_skipped_as_a_duplicate() {
    let dir = TestDir::new("importer-duplicate-match-id");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    // 0.x's log capture and its history import of the same MTGO match.
    add_simple_match(&conn, 1, "tok-captured", "4242");
    add_simple_match(&conn, 2, "tok-history", "4242");
    // Matches without a numeric match ID are never duplicates of each other.
    add_simple_match(&conn, 3, "tok-manual-a", "manual_a");
    add_simple_match(&conn, 4, "tok-manual-b", "manual_b");
    add_simple_match(&conn, 5, "tok-other", "4343");

    let result = read_database(&path).expect("read");
    assert_eq!(
        tokens(&result.matches),
        vec!["tok-captured", "tok-manual-a", "tok-manual-b", "tok-other"],
        "the first row with match ID 4242 wins"
    );
    assert_eq!(
        result.skipped,
        Skipped {
            duplicate_token: 1,
            ..Skipped::default()
        }
    );
}

#[test]
fn a_blank_token_is_skipped_and_counted() {
    let dir = TestDir::new("importer-blank-token");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "", "1");
    add_simple_match(&conn, 2, "   ", "2");
    add_simple_match(&conn, 3, "tok-kept", "3");

    let result = read_database(&path).expect("read");
    assert_eq!(tokens(&result.matches), vec!["tok-kept"]);
    assert_eq!(
        result.skipped,
        Skipped {
            blank_token: 2,
            ..Skipped::default()
        }
    );
}

#[test]
fn a_repeated_game_id_within_a_match_is_dropped_and_counted() {
    let dir = TestDir::new("importer-duplicate-game");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_match(&conn, 1, "tok-a", "1", "complete");
    for (game_id, mtgo_id) in [(10, "70"), (11, "70"), (12, "71")] {
        add_game(&conn, game_id, 1, mtgo_id, Some(1), STARTED);
        add_seat(&conn, game_id, 1, true);
        add_seat(&conn, game_id, 2, false);
    }

    let result = read_database(&path).expect("read");
    let games: Vec<(u32, Option<i64>)> = result.matches[0]
        .games
        .iter()
        .map(|game| (game.game_number, game.mtgo_game_id))
        .collect();
    assert_eq!(
        games,
        vec![(1, Some(70)), (3, Some(71))],
        "the first game with Game ID 70 is kept; numbers stay as read"
    );
    assert_eq!(
        result.skipped,
        Skipped {
            duplicate_games: 1,
            ..Skipped::default()
        }
    );
}

#[test]
fn manual_and_odd_ids_become_null_match_ids() {
    let dir = TestDir::new("importer-odd-ids");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-manual", "manual_8f3a2c");
    add_simple_match(&conn, 2, "tok-mixed", "12ab");
    add_simple_match(&conn, 3, "tok-empty", "");
    add_simple_match(&conn, 4, "tok-number", "42");
    conn.execute("UPDATE games SET mtgo_id = 'manual_g' WHERE id = 40", [])
        .expect("odd game id");
    let result = read_database(&path).expect("read");
    let ids: Vec<Option<i64>> = result
        .matches
        .iter()
        .map(|found| found.mtgo_match_id)
        .collect();
    assert_eq!(ids, vec![None, None, None, Some(42)]);
    assert_eq!(result.matches[0].games[0].mtgo_game_id, Some(10));
    assert_eq!(result.matches[3].games[0].mtgo_game_id, None);
}

#[test]
fn unknown_format_becomes_null() {
    let dir = TestDir::new("importer-unknown-format");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-unknown", "1");
    add_simple_match(&conn, 2, "tok-empty", "2");
    conn.execute_batch(
        "UPDATE matches SET format = 'Unknown', match_type = 'Unknown' WHERE id = 1;
         UPDATE matches SET format = '', match_type = '' WHERE id = 2;",
    )
    .expect("formats");
    let result = read_database(&path).expect("read");
    for found in &result.matches {
        assert_eq!(found.format, None, "{}", found.token);
        assert_eq!(found.match_type, None, "{}", found.token);
    }
    assert_eq!(result.matches.len(), 2);
}

#[test]
fn a_null_won_is_unknown() {
    let dir = TestDir::new("importer-null-won");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-null-won", "1");
    conn.execute("UPDATE games SET won = NULL", [])
        .expect("null won");
    let found = only_match(&path);
    assert_eq!(found.games[0].result, "unknown");
}

#[test]
fn a_match_without_an_opponent_row_has_no_opponent() {
    let dir = TestDir::new("importer-no-opponent");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_match(&conn, 1, "tok-solo", "1", "complete");
    add_game(&conn, 10, 1, "10", Some(1), STARTED);
    add_seat(&conn, 10, 1, true);
    let found = only_match(&path);
    assert_eq!(found.opponent, None);
    assert_eq!(found.local_username, "Qlocal");
    assert_eq!(found.games[0].opp_mulligans, None);
}

// Issue #313: 0.x's `matches.imported` flag is carried as it is. A 0.x
// older than the flag also predates its match history import (both came in
// one commit), so every match it holds was tracked from the log.
#[test]
fn the_0x_imported_flag_is_carried() {
    let dir = TestDir::new("importer-imported-flag");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-tracked", "1");
    add_simple_match(&conn, 2, "tok-history", "2");
    conn.execute("UPDATE matches SET imported = 1 WHERE id = 2", [])
        .expect("flag");
    let flags: Vec<(String, bool)> = read_database(&path)
        .expect("read")
        .matches
        .into_iter()
        .map(|found| (found.token, found.legacy_history))
        .collect();
    assert_eq!(
        flags,
        vec![
            ("tok-tracked".to_string(), false),
            ("tok-history".to_string(), true),
        ]
    );

    let old = TestDir::new("importer-imported-flag-old");
    let schema = schema_without(
        ", \"imported\" tinyint(1) not null default ('0')",
        "imported",
    );
    let (path, conn) = legacy_db(&old, &schema);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-old", "1");
    assert!(!only_match(&path).legacy_history);
}

#[test]
fn a_database_without_mulligan_count_still_reads_with_no_mulligans() {
    let dir = TestDir::new("importer-no-mulligan-count");
    let schema = schema_without("\"mulligan_count\" integer, ", "mulligan_count");
    let (path, conn) = legacy_db(&dir, &schema);
    add_two_players(&conn);
    add_match(&conn, 1, "tok-old", "1", "complete");
    add_game(&conn, 10, 1, "10", Some(1), STARTED);
    add_seat(&conn, 10, 1, true);
    add_seat(&conn, 10, 2, false);
    let found = only_match(&path);
    assert_eq!(found.games[0].local_mulligans, None);
    assert_eq!(found.games[0].opp_mulligans, None);
}

#[test]
fn a_missing_required_column_is_reported_as_too_old() {
    let dir = TestDir::new("importer-too-old");
    let (path, _conn) = legacy_db(&dir, &schema_without("\"won\" tinyint(1), ", "won"));
    let error = read_database(&path).expect_err("too old");
    assert_eq!(error, ImportError::TooOld("games.won".to_string()));
    assert_eq!(
        error.to_string(),
        "This 0.x database is too old to import. Update MyMTGO 0.x, start it once, then try again."
    );
}

#[test]
fn a_v1_database_is_not_a_legacy_database() {
    let dir = TestDir::new("importer-v1-file");
    drop(v1_db(&dir));
    let error = read_database(&dir.join("mymtgo.sqlite")).expect_err("not 0.x");
    assert_eq!(error, ImportError::NotLegacy);
    assert_eq!(error.to_string(), "This file is not a MyMTGO 0.x database.");
}

#[test]
fn an_empty_sqlite_file_is_not_a_legacy_database() {
    let dir = TestDir::new("importer-empty-file");
    let path = dir.join("empty.sqlite");
    std::fs::write(&path, b"").expect("empty file");
    let error = read_database(&path).expect_err("not 0.x");
    assert_eq!(error, ImportError::NotLegacy);
}

#[test]
fn a_missing_file_is_an_open_error() {
    let dir = TestDir::new("importer-missing");
    let error = read_database(&dir.join("nativephp.sqlite")).expect_err("no file");
    assert!(matches!(error, ImportError::Open(_)), "{error:?}");
    assert!(
        !dir.join("nativephp.sqlite").exists(),
        "a read-only open never creates the file"
    );
}

#[test]
fn reads_a_database_another_connection_holds_open_in_wal_mode() {
    let dir = TestDir::new("importer-wal");
    let (path, writer) = legacy_db(&dir, SCHEMA);
    let mode: String = writer
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .expect("wal");
    assert_eq!(mode, "wal");
    writer
        .execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .expect("no checkpoints");
    add_two_players(&writer);
    add_simple_match(&writer, 1, "tok-wal", "1");
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    assert!(
        std::fs::metadata(&wal).expect("wal file").len() > 0,
        "the rows are only in the WAL"
    );

    let found = only_match(&path);
    assert_eq!(found.token, "tok-wal");
    assert_eq!(found.games.len(), 1);

    // 0.x closed: the last connection checkpoints and removes the WAL.
    drop(writer);
    assert_eq!(only_match(&path).token, "tok-wal");
}

#[test]
fn filter_settings_keeps_preferences_and_drops_secrets_and_internal_state() {
    let input = r#"{
        "log_path": "C:/Q/mtgo.log",
        "log_data_path": "C:/Q/data",
        "card_images_path": "C:/Q/images",
        "local_images": true,
        "card_stats_trust": 3,
        "offline_mode": false,
        "debug_mode": true,
        "dashboard_layout": ["a", "b"],
        "deck_card_size": "compact",
        "decks_per_page": 24,
        "hide_archived_decks": true,
        "decks_grouped_by_archetype": false,
        "league_window": {"x": 1},
        "opponent_window": {"x": 2},
        "deck_window": {"x": 3},
        "draft_notes_window": {"x": 4},
        "game_overlay": true,
        "overlay_background_path": "C:/Q/bg.png",
        "overlay_opacity": 0.8,
        "overlay_deck_labels": {"1": "Q label"},
        "overlay_publish_error": "Q failed",
        "overlay_background_remote": {"url": "https://example.invalid/q.png", "sha1": "q"},
        "overlay_last_published_at": 5,
        "api_key": "secret",
        "api_key_expires_at": "later",
        "sync_token": "secret",
        "archetype_cache": {},
        "watcher_active": true,
        "whats_new_seen_version": "0.9",
        "donation_prompt_seen": true,
        "offline_mode_locked_until": 5,
        "device_id": "q-device",
        "app_server_url": "https://example.invalid",
        "autostart_enabled": true,
        "system_tz": "Europe/London",
        "something_new": 1
    }"#;
    let filtered: serde_json::Value =
        serde_json::from_str(&filter_settings(input).expect("filter")).expect("json");
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
        "log_path": "C:/Q/mtgo.log",
        "log_data_path": "C:/Q/data",
        "card_images_path": "C:/Q/images",
        "local_images": true,
        "card_stats_trust": 3,
        "offline_mode": false,
        "debug_mode": true,
        "dashboard_layout": ["a", "b"],
        "deck_card_size": "compact",
        "decks_per_page": 24,
        "hide_archived_decks": true,
        "league_window": {"x": 1},
        "opponent_window": {"x": 2},
        "deck_window": {"x": 3},
        "draft_notes_window": {"x": 4},
        "game_overlay": true,
        "overlay_background_path": "C:/Q/bg.png",
        "overlay_opacity": 0.8,
        "overlay_deck_labels": {"1": "Q label"}
    }"#,
    )
    .expect("expected json");
    assert_eq!(filtered, expected);

    assert!(matches!(
        filter_settings("not json"),
        Err(ImportError::Read(_))
    ));
    assert!(matches!(
        filter_settings("[1, 2]"),
        Err(ImportError::Read(_))
    ));
}

/// A fresh v1 database (migrated) at `<dir>/mymtgo.sqlite`.
#[test]
fn a_second_import_changes_nothing() {
    let dir = TestDir::new("importer-rerun");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-one", "1");
    add_simple_match(&conn, 2, "tok-two", "manual_2");
    conn.execute("UPDATE matches SET notes = 'Q note' WHERE id = 1", [])
        .expect("note");
    let mut v1 = v1_db(&dir);

    let first = import_matches(&mut v1, &read_database(&path).expect("read").matches, 10)
        .expect("first import");
    assert_eq!(
        first,
        ImportCounts {
            added: 2,
            ..ImportCounts::default()
        }
    );
    let second = import_matches(&mut v1, &read_database(&path).expect("read").matches, 20)
        .expect("second import");
    assert_eq!(
        second,
        ImportCounts {
            unchanged: 2,
            ..ImportCounts::default()
        }
    );
    assert_eq!(count(&v1, "SELECT MAX(revision) FROM matches"), 1);
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM games"), 2);
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM matches WHERE notes IS NOT NULL"),
        1
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM matches WHERE source = 'imported'"
        ),
        2
    );
}

#[test]
fn live_rows_win_by_token_and_by_match_id() {
    let dir = TestDir::new("importer-live-wins");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_simple_match(&conn, 1, "tok-live", "1");
    add_simple_match(&conn, 2, "tok-history", "555");
    add_simple_match(&conn, 3, "tok-new", "3");
    let mut v1 = v1_db(&dir);
    v1.execute_batch(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlive', 1);
         INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
             started_at, last_activity_at, revision, derived_version)
         VALUES ('tok-live', 9, 1, 'log', 'complete', 'loss', 10, 20, 3, 1),
                ('tok-other-live', 555, 1, 'log', 'complete', 'loss', 10, 20, 3, 1);",
    )
    .expect("live rows");

    let counts =
        import_matches(&mut v1, &read_database(&path).expect("read").matches, 10).expect("import");
    assert_eq!(
        counts,
        ImportCounts {
            added: 1,
            skipped_live: 2,
            ..ImportCounts::default()
        }
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM matches WHERE source = 'log' AND outcome = 'loss' AND revision = 3"
        ),
        2,
        "the live rows are untouched"
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM matches WHERE token = 'tok-new' AND source = 'imported'"
        ),
        1
    );
}

fn add_account(conn: &Connection, id: i64, username: &str, deleted: bool) {
    let deleted_at = if deleted {
        Some("2026-01-01 00:00:00")
    } else {
        None
    };
    conn.execute(
        "INSERT INTO accounts (id, username, deleted_at) VALUES (?1, ?2, ?3)",
        params![id, username, deleted_at],
    )
    .expect("account");
}

/// Deck `id` of account `account_id` with version `id * 10`, and match
/// `match_id` played with that version.
fn add_deck_of(conn: &Connection, id: i64, account_id: i64, match_id: i64) {
    conn.execute_batch(&format!(
        "INSERT INTO decks (id, mtgo_id, name, format, account_id)
             VALUES ({id}, 'q-deck-{id}', 'Qdeck', 'CMODERN', {account_id});
         INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES ({version}, {id}, '', '2026-01-01 00:00:00');
         UPDATE matches SET deck_version_id = {version} WHERE id = {match_id};",
        version = id * 10
    ))
    .expect("deck");
}

/// `(token, local, opponent, games, won, lost)` of each read match.
type Shape = (
    String,
    String,
    Option<String>,
    usize,
    Option<i64>,
    Option<i64>,
);

fn shapes(matches: &[ImportedMatch]) -> Vec<Shape> {
    matches
        .iter()
        .map(|found| {
            (
                found.token.clone(),
                found.local_username.clone(),
                found.opponent.clone(),
                found.games.len(),
                found.games_won_imported,
                found.games_lost_imported,
            )
        })
        .collect()
}

fn shape(token: &str, local: &str, opponent: Option<&str>, games: usize) -> Shape {
    (
        token.to_string(),
        local.to_string(),
        opponent.map(str::to_string),
        games,
        None,
        None,
    )
}

#[test]
fn the_local_seat_and_the_opponent_may_be_on_any_game() {
    let dir = TestDir::new("importer-later-seat");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_match(&conn, 1, "tok-later", "1", "complete");
    add_game(&conn, 10, 1, "10", Some(1), "2026-01-02 03:05:00");
    add_game(&conn, 11, 1, "11", Some(0), "2026-01-02 03:20:00");
    add_game(&conn, 12, 1, "12", Some(1), "2026-01-02 03:30:00");
    // Game 1 has no seats, game 2 only the local one, game 3 both.
    add_seat(&conn, 11, 1, true);
    add_seat(&conn, 12, 2, false);
    add_seat(&conn, 12, 1, true);

    let result = read_database(&path).expect("read");
    assert_eq!(
        shapes(&result.matches),
        vec![shape("tok-later", "Qlocal", Some("Qopponent"), 3)]
    );
    let game_2 = &result.matches[0].games[1];
    assert_eq!(game_2.opp_mulligans, None, "game 2 has no opponent seat");
    assert_eq!(result.skipped, Skipped::default());
}

#[test]
fn without_a_local_seat_the_deck_s_account_is_the_local_player() {
    let dir = TestDir::new("importer-deck-account");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qdeckowner", false);
    add_account(&conn, 2, "Qsecond", false);
    // A gameless match on Qdeckowner's deck.
    add_match(&conn, 1, "tok-gameless", "1", "complete");
    add_deck_of(&conn, 1, 1, 1);
    // An opponent seat only: the opponent still comes over.
    add_match(&conn, 2, "tok-opponent-only", "2", "complete");
    add_game(&conn, 20, 2, "20", Some(1), STARTED);
    add_seat(&conn, 20, 2, false);
    add_deck_of(&conn, 2, 2, 2);
    // A local seat wins over the deck's account (when the seat is one of
    // the 0.x accounts, issue #337).
    add_account(&conn, 3, "Qlocal", false);
    add_simple_match(&conn, 3, "tok-seat", "3");
    add_deck_of(&conn, 3, 2, 3);
    // The deck owner's own seat flagged non-local is not the opponent.
    add_player(&conn, 3, "Qdeckowner");
    add_match(&conn, 4, "tok-own-seat", "4", "complete");
    add_game(&conn, 40, 4, "40", Some(1), STARTED);
    add_seat(&conn, 40, 3, false);
    add_deck_of(&conn, 4, 1, 4);

    let result = read_database(&path).expect("read");
    let mut gameless = shape("tok-gameless", "Qdeckowner", None, 0);
    (gameless.4, gameless.5) = (Some(2), Some(1));
    assert_eq!(
        shapes(&result.matches),
        vec![
            gameless,
            shape("tok-opponent-only", "Qsecond", Some("Qopponent"), 1),
            shape("tok-seat", "Qlocal", Some("Qopponent"), 1),
            shape("tok-own-seat", "Qdeckowner", None, 1),
        ]
    );
    assert_eq!(result.skipped, Skipped::default());
    assert_eq!(
        result.match_ids.get("tok-gameless"),
        Some(&1),
        "a newly kept match finds its detail"
    );
}

#[test]
fn without_a_seat_or_a_deck_the_only_account_is_the_local_player() {
    let dir = TestDir::new("importer-only-account");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    // A deleted account beside it would leave no only account
    // (`a_deleted_account_means_there_is_no_only_account`).
    add_account(&conn, 2, "Qonly", false);
    // 0.x ImportMatchesTest "creates match without games when no game log
    // available": a 0-2 history import with no games and no deck.
    add_match(&conn, 1, "tok-history", "99999999", "complete");
    conn.execute_batch(
        "UPDATE matches SET format = 'CPAUPER', games_won = 0, games_lost = 2,
             outcome = 'loss' WHERE id = 1;
         UPDATE matches SET deck_version_id = 404 WHERE id = 1;",
    )
    .expect("a gameless history import");
    // A 0-0 abandoned match with no games, no deck and format Unknown.
    add_match(&conn, 2, "tok-abandoned", "2", "abandoned");
    conn.execute(
        "UPDATE matches SET format = 'Unknown', games_won = 0, games_lost = 0, ended_at = NULL
         WHERE id = 2",
        [],
    )
    .expect("an abandoned match");

    let result = read_database(&path).expect("read");
    let mut history = shape("tok-history", "Qonly", None, 0);
    (history.4, history.5) = (Some(0), Some(2));
    let mut abandoned = shape("tok-abandoned", "Qonly", None, 0);
    (abandoned.4, abandoned.5) = (Some(0), Some(0));
    assert_eq!(shapes(&result.matches), vec![history, abandoned]);
    let found = &result.matches[0];
    assert_eq!(found.mtgo_match_id, Some(99_999_999));
    assert_eq!(found.format, Some("CPAUPER".to_string()));
    assert_eq!(found.outcome, "loss");
    assert_eq!(result.matches[1].format, None);
    assert_eq!(result.matches[1].state, "abandoned");
    assert_eq!(result.skipped, Skipped::default());

    let mut v1 = v1_db(&dir);
    let first = import_matches(&mut v1, &result.matches, 10).expect("import");
    assert_eq!(first.added, 2);
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE a.username = 'Qonly' AND m.source = 'imported'
               AND NOT EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)"
        ),
        2
    );
    assert_eq!(
        count(
            &v1,
            "SELECT games_lost_imported FROM matches WHERE token = 'tok-history'"
        ),
        2
    );
    let again =
        import_matches(&mut v1, &read_database(&path).expect("read").matches, 20).expect("rerun");
    assert_eq!(
        again,
        ImportCounts {
            unchanged: 2,
            ..ImportCounts::default()
        }
    );
}

#[test]
fn two_accounts_and_no_seat_or_deck_is_still_skipped() {
    let dir = TestDir::new("importer-two-accounts");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qfirst", false);
    add_account(&conn, 2, "Qsecond", false);
    add_match(&conn, 1, "tok-gameless", "1", "complete");
    add_match(&conn, 2, "tok-opponent-only", "2", "complete");
    add_game(&conn, 20, 2, "20", Some(1), STARTED);
    add_seat(&conn, 20, 2, false);
    // A deck without an account does not resolve one either.
    add_match(&conn, 3, "tok-no-owner", "3", "complete");
    add_deck_of(&conn, 3, 1, 3);
    conn.execute("UPDATE decks SET account_id = NULL", [])
        .expect("no owner");

    let result = read_database(&path).expect("read");
    assert!(result.matches.is_empty(), "{:?}", result.matches);
    assert_eq!(
        result.skipped,
        Skipped {
            no_local_player: 3,
            ..Skipped::default()
        }
    );
    assert!(result.match_ids.is_empty());
}

#[test]
fn every_account_but_deleted_ones_is_read_with_its_login_id_and_creation_time() {
    let dir = TestDir::new("importer-accounts");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    conn.execute_batch(
        "INSERT INTO accounts (id, username, login_id, created_at, deleted_at)
         VALUES (1, 'Qmain', 1234, '2026-01-02 03:04:05', NULL),
                (2, 'Qgone', 5678, '2026-01-02 03:04:05', '2026-01-03 00:00:00'),
                (3, 'Qbare', NULL, NULL, NULL),
                (4, '   ', 91, NULL, NULL),
                (5, 'Qodd', 'not a number', 'garbage', NULL);",
    )
    .expect("accounts");

    let accounts = read_database(&path).expect("read").accounts;
    assert_eq!(
        accounts,
        vec![
            ImportedAccount {
                username: "Qmain".to_string(),
                login_id: Some(1234),
                created_at_ms: Some(STARTED_MS),
                tracked: true,
                active_since_ms: None,
            },
            ImportedAccount {
                username: "Qbare".to_string(),
                login_id: None,
                created_at_ms: None,
                tracked: true,
                active_since_ms: None,
            },
            ImportedAccount {
                username: "Qodd".to_string(),
                login_id: None,
                created_at_ms: None,
                tracked: true,
                active_since_ms: None,
            },
        ],
        "a deleted account and a blank name are left out"
    );
}

#[test]
fn an_old_0x_without_login_ids_or_accounts_reads_what_it_has() {
    let dir = TestDir::new("importer-accounts-old");
    let (path, conn) = legacy_db(&dir, &schema_without(", \"login_id\" integer", "login_id"));
    conn.execute(
        "INSERT INTO accounts (id, username) VALUES (1, 'Qmain')",
        [],
    )
    .expect("account");
    let accounts = read_database(&path).expect("read").accounts;
    assert_eq!(
        accounts,
        vec![ImportedAccount {
            username: "Qmain".to_string(),
            login_id: None,
            created_at_ms: None,
            tracked: true,
            active_since_ms: None,
        }]
    );

    let bare = TestDir::new("importer-accounts-none");
    let (path, conn) = legacy_db(&bare, SCHEMA);
    conn.execute_batch("DROP TABLE accounts;")
        .expect("no accounts table");
    assert!(read_database(&path).expect("read").accounts.is_empty());
}

// Issue #310: 0.x's tracked flag and its active account (the time 0.x
// last updated the row, else its creation time, else 0).
#[test]
fn the_tracked_flag_and_the_active_account_are_read() {
    let dir = TestDir::new("importer-accounts-tracked");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    conn.execute_batch(
        "INSERT INTO accounts (id, username, active, tracked, created_at, updated_at)
         VALUES (1, 'Qmain', 1, 1, '2026-01-02 03:04:05', '2026-01-02 03:34:05'),
                (2, 'Qquiet', 0, 0, NULL, NULL),
                (3, 'Qundated', '1', '0', '2026-01-02 03:04:05', NULL),
                (4, 'Qbare', 1, 1, NULL, NULL);",
    )
    .expect("accounts");
    let read: Vec<(String, bool, Option<i64>)> = read_database(&path)
        .expect("read")
        .accounts
        .into_iter()
        .map(|account| (account.username, account.tracked, account.active_since_ms))
        .collect();
    assert_eq!(
        read,
        vec![
            ("Qmain".to_string(), true, Some(ENDED_MS)),
            ("Qquiet".to_string(), false, None),
            ("Qundated".to_string(), false, Some(STARTED_MS)),
            ("Qbare".to_string(), true, Some(0)),
        ]
    );
}

#[test]
fn an_old_0x_without_tracked_or_active_tracks_every_account() {
    let dir = TestDir::new("importer-accounts-untracked-old");
    let schema = schema_without(", \"active\" tinyint(1) not null default '0'", "active");
    let schema = schema
        .replace(", \"tracked\" tinyint(1) not null default '1'", "")
        .replace(", \"updated_at\" datetime, \"login_id\"", ", \"login_id\"");
    let (path, conn) = legacy_db(&dir, &schema);
    conn.execute(
        "INSERT INTO accounts (id, username) VALUES (1, 'Qmain')",
        [],
    )
    .expect("account");
    let accounts = read_database(&path).expect("read").accounts;
    assert_eq!(
        accounts
            .iter()
            .map(|account| (account.tracked, account.active_since_ms))
            .collect::<Vec<_>>(),
        vec![(true, None)]
    );
}

// 0.x AppSettings::autostartEnabled: `(bool) $this->get('autostart_enabled', false)`.
#[test]
fn the_0x_launch_at_login_is_read_as_php_casts_it() {
    use importer::settings::launch_at_login;
    assert!(launch_at_login(r#"{"autostart_enabled":true}"#));
    assert!(launch_at_login(r#"{"autostart_enabled":1}"#));
    assert!(launch_at_login(r#"{"autostart_enabled":"1"}"#));
    assert!(!launch_at_login(r#"{"autostart_enabled":false}"#));
    assert!(!launch_at_login(r#"{"autostart_enabled":0}"#));
    assert!(!launch_at_login(r#"{"autostart_enabled":"0"}"#));
    assert!(!launch_at_login(r#"{"autostart_enabled":""}"#));
    assert!(!launch_at_login(r#"{"autostart_enabled":null}"#));
    assert!(!launch_at_login(r#"{"deck_card_size":"large"}"#));
    assert!(!launch_at_login("not json"));
}

/// Issue #190 item 1: with one deleted and one active 0.x account, a match
/// with no seat or deck could be the deleted account's, so it is skipped,
/// not put on the active one. A deleted only account is no fallback either.
#[test]
fn a_deleted_account_means_there_is_no_only_account() {
    let dir = TestDir::new("importer-only-account-deleted");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qdeleted", true);
    add_account(&conn, 2, "Qactive", false);
    add_match(&conn, 1, "tok-gameless", "1", "complete");
    let result = read_database(&path).expect("read");
    assert!(result.matches.is_empty(), "{:?}", result.matches);
    assert_eq!(result.skipped.no_local_player, 1);

    let alone = TestDir::new("importer-only-account-deleted-alone");
    let (path, conn) = legacy_db(&alone, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qdeleted", true);
    add_match(&conn, 1, "tok-gameless", "1", "complete");
    let result = read_database(&path).expect("read");
    assert!(result.matches.is_empty(), "{:?}", result.matches);
    assert_eq!(result.skipped.no_local_player, 1);
}

/// Issue #190 item 9: the deck of a deleted 0.x account gives no local
/// player (0.x leaves a deleted account out of a deck's account), so the
/// import never registers a deleted account through it.
#[test]
fn a_deleted_account_s_deck_gives_no_local_player() {
    let dir = TestDir::new("importer-deck-account-deleted");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qdeleted", true);
    add_account(&conn, 2, "Qactive", false);
    add_match(&conn, 1, "tok-gameless", "1", "complete");
    add_deck_of(&conn, 1, 1, 1);
    let result = read_database(&path).expect("read");
    assert!(result.matches.is_empty(), "{:?}", result.matches);
    assert_eq!(result.skipped.no_local_player, 1);
}

/// Issue #162: 0.x named the local player of a manual match "You" when MTGO
/// gave no username. The match's deck account (else 0.x's only account) is
/// the real local player; without either, "You" stays.
#[test]
fn a_manual_match_s_you_is_the_deck_s_account() {
    let dir = TestDir::new("importer-manual-you");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_player(&conn, 1, "You");
    add_player(&conn, 2, "Qopponent");
    add_account(&conn, 1, "Qreal", false);
    add_account(&conn, 2, "Qsecond", false);
    // A manual match on Qreal's deck, and one with no deck.
    add_simple_match(&conn, 1, "tok-manual-deck", "manual_a");
    add_deck_of(&conn, 1, 1, 1);
    add_simple_match(&conn, 2, "tok-manual-bare", "manual_b");
    // A tracked match: a player really named "You" is no placeholder; not
    // being a 0.x account, it gives way to the deck's account (issue #337).
    add_simple_match(&conn, 3, "tok-tracked", "3");
    add_deck_of(&conn, 3, 1, 3);
    conn.execute("UPDATE matches SET manual = 1 WHERE id IN (1, 2)", [])
        .expect("manual");

    let result = read_database(&path).expect("read");
    assert_eq!(
        shapes(&result.matches),
        vec![
            shape("tok-manual-deck", "Qreal", Some("Qopponent"), 1),
            shape("tok-manual-bare", "You", Some("Qopponent"), 1),
            shape("tok-tracked", "Qreal", Some("Qopponent"), 1),
        ]
    );
    let manual = result.matches[0].manual.as_ref().expect("a manual match");
    assert!(manual.is_empty(), "nothing recorded: {manual:?}");

    // With one 0.x account, a manual match with no deck is that account's.
    conn.execute("DELETE FROM accounts WHERE id = 2", [])
        .expect("one account");
    let result = read_database(&path).expect("read");
    assert_eq!(result.matches[1].local_username, "Qreal");
}

/// Issue #190 item 2: with no seat marked local, the local player's own
/// seat gives the local die roll and mulligans, never the opponent's.
#[test]
fn without_a_local_seat_the_local_player_s_own_seat_is_the_local_one() {
    let dir = TestDir::new("importer-own-seat-numbers");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qlocal", false);
    add_match(&conn, 1, "tok-own-seat", "1", "complete");
    add_game(&conn, 10, 1, "10", Some(1), STARTED);
    add_seat(&conn, 10, 1, false);
    add_seat(&conn, 10, 2, false);
    conn.execute_batch(
        "UPDATE game_player SET dice_roll = 6, mulligan_count = 1, on_play = 1
             WHERE player_id = 1;
         UPDATE game_player SET dice_roll = 3, mulligan_count = 0, on_play = 0
             WHERE player_id = 2;",
    )
    .expect("seat numbers");
    let found = only_match(&path);
    assert_eq!(found.local_username, "Qlocal");
    assert_eq!(found.opponent.as_deref(), Some("Qopponent"));
    let game = &found.games[0];
    assert_eq!(
        (
            game.local_die_roll,
            game.opp_die_roll,
            game.local_mulligans,
            game.opp_mulligans,
            game.on_play
        ),
        (Some(6), Some(3), Some(1), Some(0), Some(true))
    );
}

/// Review fix (#162): a 0.x manual match stored on the "You" placeholder by
/// an earlier import moves to the real account on a rerun once 0.x gives
/// one, whether stored as manual or (before Plan L) as imported, and the
/// placeholder account goes once nothing uses it.
#[test]
fn a_rerun_moves_you_matches_to_the_real_account_and_drops_the_placeholder() {
    let dir = TestDir::new("importer-manual-you-rerun");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_player(&conn, 1, "You");
    add_player(&conn, 2, "Qopponent");
    add_simple_match(&conn, 1, "tok-manual", "manual_a");
    add_simple_match(&conn, 2, "tok-old-import", "manual_b");
    conn.execute("UPDATE matches SET manual = 1", [])
        .expect("manual");
    // No 0.x account yet: the first import keeps "You".
    let mut v1 = v1_db(&dir);
    import_matches(&mut v1, &read_database(&path).expect("read").matches, 10).expect("first");
    v1.execute(
        "UPDATE matches SET source = 'imported' WHERE token = 'tok-old-import'",
        [],
    )
    .expect("stored as plain imported before Plan L");
    let owners = |v1: &Connection| {
        let mut stmt = v1
            .prepare(
                "SELECT m.token || '=' || a.username FROM matches m
                 JOIN accounts a ON a.id = m.account_id ORDER BY m.token",
            )
            .expect("prepare");
        let found: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        found
    };
    assert_eq!(owners(&v1), vec!["tok-manual=You", "tok-old-import=You"]);

    add_account(&conn, 1, "Qreal", false);
    let again =
        import_matches(&mut v1, &read_database(&path).expect("read").matches, 20).expect("rerun");
    assert_eq!(again.updated, 2, "{again:?}");
    assert_eq!(
        owners(&v1),
        vec!["tok-manual=Qreal", "tok-old-import=Qreal"]
    );
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM accounts WHERE username = 'You'"),
        0,
        "the placeholder is gone"
    );
    let before = count(&v1, "SELECT COUNT(*) FROM matches");
    let third =
        import_matches(&mut v1, &read_database(&path).expect("read").matches, 30).expect("third");
    assert_eq!(third.updated, 0, "{third:?}");
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM matches"), before);
}

/// Review fix (#162): a 0.x with one active account beside a deleted one
/// gives a manual "You" match that account, never a "You" account.
#[test]
fn a_manual_you_match_takes_the_only_active_account_beside_a_deleted_one() {
    let dir = TestDir::new("importer-manual-you-deleted");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_player(&conn, 1, "You");
    add_player(&conn, 2, "Qopponent");
    add_account(&conn, 1, "Qgone", true);
    add_account(&conn, 2, "Qreal", false);
    add_simple_match(&conn, 1, "tok-manual", "manual_a");
    conn.execute("UPDATE matches SET manual = 1", [])
        .expect("manual");
    assert_eq!(only_match(&path).local_username, "Qreal");
}

/// Issue #337: with accounts in 0.x, only matches whose local player is one
/// of them are imported. A deleted account is not one (0.x shows none); a
/// match without a local seat still takes its deck's account; a skipped
/// match still counts under its own reason first.
#[test]
fn only_matches_of_0x_accounts_are_imported() {
    let dir = TestDir::new("importer-not-account");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_player(&conn, 3, "Qstranger");
    add_player(&conn, 4, "Qgone");
    add_account(&conn, 1, "Qlocal", false);
    add_account(&conn, 2, "Qgone", true);
    add_simple_match(&conn, 1, "tok-own", "1");
    // Qstranger's match, and one by the deleted account.
    for (id, token, player) in [(2, "tok-stranger", 3), (3, "tok-gone", 4)] {
        add_match(&conn, id, token, &id.to_string(), "complete");
        add_game(&conn, id * 10, id, &(id * 10).to_string(), Some(1), STARTED);
        add_seat(&conn, id * 10, player, true);
        add_seat(&conn, id * 10, 2, false);
    }
    // No local seat: the deck's account is the local player.
    add_match(&conn, 4, "tok-deck-only", "4", "complete");
    add_deck_of(&conn, 4, 1, 4);
    // Qstranger's match still in progress counts as in progress.
    add_match(&conn, 5, "tok-stranger-live", "5", "started");
    add_game(&conn, 50, 5, "50", None, STARTED);
    add_seat(&conn, 50, 3, true);
    // Two account rows (one deleted): no only account to fall back on.
    add_match(&conn, 6, "tok-nobody", "6", "complete");
    // Tournaments: 77 only Qstranger played, 78 both.
    conn.execute_batch(
        "UPDATE matches SET tournament_event_id = 77 WHERE id = 2;
         UPDATE matches SET tournament_event_id = 78 WHERE id IN (1, 3);",
    )
    .expect("tournaments");

    let result = read_database(&path).expect("read");
    assert_eq!(
        shapes(&result.matches),
        vec![
            shape("tok-own", "Qlocal", Some("Qopponent"), 1),
            (
                "tok-deck-only".to_string(),
                "Qlocal".to_string(),
                None,
                0,
                Some(2),
                Some(1)
            ),
        ]
    );
    assert_eq!(
        result.skipped,
        Skipped {
            in_progress: 1,
            no_local_player: 1,
            not_account: 2,
            ..Skipped::default()
        }
    );
    let mut left_out: Vec<String> = result.not_account.tokens.iter().cloned().collect();
    left_out.sort();
    assert_eq!(left_out, vec!["tok-gone", "tok-stranger"]);
    assert!(result.not_account.holds_tournament(77));
    assert!(!result.not_account.holds_tournament(78), "Qlocal played 78");

    // Without an account 0.x shows (only deleted rows), 0.x scopes nothing
    // by account: every local player is imported, as before #337.
    conn.execute("UPDATE accounts SET deleted_at = '2026-01-01 00:00:00'", [])
        .expect("all deleted");
    let result = read_database(&path).expect("read");
    assert_eq!(
        tokens(&result.matches),
        vec!["tok-own", "tok-stranger", "tok-gone"]
    );
    assert_eq!(result.skipped.not_account, 0);
    assert!(result.not_account.tokens.is_empty());
}

/// Issue #337: importing a 0.x database with another player's match, twice,
/// registers only 0.x's accounts, and the second run changes nothing.
#[test]
fn a_rerun_keeps_only_the_0x_accounts() {
    let dir = TestDir::new("importer-not-account-rerun");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_player(&conn, 3, "Qstranger");
    add_account(&conn, 1, "Qlocal", false);
    add_simple_match(&conn, 1, "tok-own", "1");
    add_match(&conn, 2, "tok-stranger", "2", "complete");
    add_game(&conn, 20, 2, "20", Some(1), STARTED);
    add_seat(&conn, 20, 3, true);
    add_seat(&conn, 20, 2, false);

    let mut v1 = v1_db(&dir);
    let accounts_of = |v1: &Connection| {
        let mut stmt = v1
            .prepare("SELECT username FROM accounts ORDER BY username")
            .expect("prepare");
        let found: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        found
    };
    for (run, now_ms) in [(1, 10), (2, 20)] {
        let read = read_database(&path).expect("read");
        store::import::import_accounts(&mut v1, &read.accounts, now_ms).expect("accounts");
        let counts = import_matches(&mut v1, &read.matches, now_ms).expect("matches");
        let expected = if run == 1 {
            ImportCounts {
                added: 1,
                ..ImportCounts::default()
            }
        } else {
            ImportCounts {
                unchanged: 1,
                ..ImportCounts::default()
            }
        };
        assert_eq!(counts, expected, "run {run}");
        assert_eq!(read.skipped.not_account, 1, "run {run}");
        assert_eq!(accounts_of(&v1), vec!["Qlocal"], "run {run}");
        assert_eq!(count(&v1, "SELECT COUNT(*) FROM matches"), 1);
    }
}

/// Issue #337: a deck of a deleted 0.x account imports with no account, so
/// the deck import never registers an account 0.x does not show.
#[test]
fn a_deleted_account_s_deck_has_no_account() {
    let dir = TestDir::new("importer-deck-of-deleted");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_account(&conn, 1, "Qlocal", false);
    add_account(&conn, 2, "Qgone", true);
    add_simple_match(&conn, 1, "tok-own", "1");
    add_deck_of(&conn, 1, 1, 1);
    add_simple_match(&conn, 2, "tok-other", "2");
    add_deck_of(&conn, 2, 2, 2);
    let read = read_database(&path).expect("read");
    let decks = read.decks.expect("decks").data.decks;
    let owners: Vec<(String, Option<String>)> = decks
        .iter()
        .map(|deck| (deck.mtgo_id.clone(), deck.account.clone()))
        .collect();
    assert_eq!(
        owners,
        vec![
            ("q-deck-1".to_string(), Some("Qlocal".to_string())),
            ("q-deck-2".to_string(), None),
        ]
    );
}

/// Issue #337 (owner ruling): a match whose local seat is not a 0.x account
/// but whose deck is a real account's is kept under that account, as 0.x
/// listed it (`forAccount`). A deleted account's deck does not save it.
#[test]
fn a_match_on_a_real_account_s_deck_is_kept_under_that_account() {
    let dir = TestDir::new("importer-not-account-deck");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_two_players(&conn);
    add_player(&conn, 3, "Qstranger");
    add_account(&conn, 1, "Qowner", false);
    add_account(&conn, 2, "Qgone", true);
    for (id, token) in [
        (1, "tok-owner-deck"),
        (2, "tok-gone-deck"),
        (3, "tok-no-deck"),
    ] {
        add_match(&conn, id, token, &id.to_string(), "complete");
        add_game(&conn, id * 10, id, &(id * 10).to_string(), Some(1), STARTED);
        add_seat(&conn, id * 10, 3, true);
        add_seat(&conn, id * 10, 2, false);
    }
    add_deck_of(&conn, 1, 1, 1);
    add_deck_of(&conn, 2, 2, 2);
    conn.execute(
        "UPDATE game_player SET dice_roll = 5, mulligan_count = 1 WHERE player_id = 3",
        [],
    )
    .expect("the seat's numbers");

    let result = read_database(&path).expect("read");
    assert_eq!(
        shapes(&result.matches),
        vec![shape("tok-owner-deck", "Qowner", Some("Qopponent"), 1)]
    );
    let game = &result.matches[0].games[0];
    assert_eq!(
        (game.local_die_roll, game.local_mulligans),
        (Some(5), Some(1)),
        "the local seat is still the local one"
    );
    assert_eq!(result.skipped.not_account, 2);
    let mut left_out: Vec<String> = result.not_account.tokens.iter().cloned().collect();
    left_out.sort();
    assert_eq!(left_out, vec!["tok-gone-deck", "tok-no-deck"]);

    let mut v1 = v1_db(&dir);
    store::import::import_accounts(&mut v1, &result.accounts, 10).expect("accounts");
    import_matches(&mut v1, &result.matches, 10).expect("matches");
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM accounts WHERE username = 'Qstranger'"
        ),
        0
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE m.token = 'tok-owner-deck' AND a.username = 'Qowner'"
        ),
        1
    );
}
