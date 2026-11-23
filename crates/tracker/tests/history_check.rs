//! MTGO's game history checks the stored outcomes (#454).

use std::sync::Arc;

use chrono_tz::UTC;
use fixture_tools::{TestDir, fixtures_dir};
use mtgo_history::{HistoryRecord, parse_history};
use mtgo_log::Env;
use store::{Connection, params};
use tracker::engine::HISTORY_CHECK_EVERY_MS;
use tracker::{Engine, TrackerConfig};

fn history_bytes() -> Vec<u8> {
    std::fs::read(fixtures_dir().join("history/mtgo_game_history")).expect("history fixture")
}

/// A fixture record whose single winner is not also a loser.
fn decided_record() -> (HistoryRecord, String) {
    let records = parse_history(&history_bytes(), UTC).expect("parse");
    records
        .into_iter()
        .find_map(|record| {
            let winner = record.match_winners.first()?.clone();
            (!record.match_losers.contains(&winner)).then_some((record, winner))
        })
        .expect("a decided record")
}

fn config(dir: &TestDir) -> TrackerConfig {
    TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: UTC,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    }
}

fn open(dir: &TestDir, now_ms: i64) -> Engine {
    Engine::open(
        config(dir),
        Env::current(),
        Arc::default(),
        Vec::new(),
        now_ms,
    )
    .expect("open engine")
}

/// A stored log match the derivation called a draw, with `match_id`.
fn store_draw(dir: &TestDir, username: &str, match_id: u64, token: &str, corrected: bool) {
    let conn = Connection::open(dir.join("data/mymtgo.sqlite")).expect("open db");
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .expect("timeout");
    conn.execute(
        "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0)
         ON CONFLICT (username) DO NOTHING",
        [username],
    )
    .expect("account");
    conn.execute(
        "INSERT INTO matches (token, mtgo_match_id, account_id, source, format, match_type,
             state, outcome, started_at, last_activity_at, revision, derived_version,
             corrected_at)
         SELECT ?1, ?2, id, 'log', 'CMODERN', 'Modern', 'complete', 'draw', 10, 12, 3, 5, ?4
         FROM accounts WHERE username = ?3",
        params![token, match_id as i64, username, corrected.then_some(9)],
    )
    .expect("match");
}

fn stored(dir: &TestDir, token: &str) -> (String, Option<String>, i64) {
    let conn = Connection::open(dir.join("data/mymtgo.sqlite")).expect("open db");
    conn.query_row(
        "SELECT outcome, history_outcome, revision FROM matches WHERE token = ?1",
        [token],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .expect("row")
}

#[test]
fn mtgo_s_verdict_replaces_a_derived_draw_once_and_leaves_corrections_alone() {
    let (record, winner) = decided_record();
    let dir = TestDir::new("history-check");
    std::fs::create_dir_all(dir.join("logs")).expect("logs");
    let mut engine = open(&dir, 1_000);
    store_draw(&dir, &winner, record.id, "q-left", false);
    store_draw(&dir, &winner, record.id, "q-fixed", true);
    dir.write("mtgo-data/Qaccount/mtgo_game_history", &history_bytes());

    assert_eq!(engine.check_history(2_000).expect("check"), 1);
    engine.flush().expect("flush");
    assert_eq!(
        stored(&dir, "q-left"),
        ("win".to_string(), Some("win".to_string()), 4)
    );
    assert_eq!(stored(&dir, "q-fixed"), ("draw".to_string(), None, 3));

    // The files did not change: nothing is read or written again.
    assert_eq!(
        engine
            .check_history(2_000 + HISTORY_CHECK_EVERY_MS)
            .expect("check"),
        0
    );
    engine.flush().expect("flush");
    assert_eq!(stored(&dir, "q-left").2, 4);
}

#[test]
fn a_tick_runs_the_check_once_caught_up_and_then_every_interval() {
    let (record, winner) = decided_record();
    let dir = TestDir::new("history-check-tick");
    std::fs::create_dir_all(dir.join("logs")).expect("logs");
    let mut engine = open(&dir, 1_000);
    store_draw(&dir, &winner, record.id, "q-left", false);
    engine.tick(1_500).expect("tick");
    engine.flush().expect("flush");
    // No history file yet: the draw stays.
    assert_eq!(stored(&dir, "q-left").1, None);

    dir.write("mtgo-data/Qaccount/mtgo_game_history", &history_bytes());
    engine.tick(1_600).expect("tick");
    engine.flush().expect("flush");
    assert_eq!(stored(&dir, "q-left").1, None, "not due yet");

    engine.tick(1_500 + HISTORY_CHECK_EVERY_MS).expect("tick");
    engine.flush().expect("flush");
    assert_eq!(stored(&dir, "q-left").0, "win");
}
