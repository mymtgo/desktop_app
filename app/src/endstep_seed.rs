//! Test seeds: an Endstep account, opponent and complete match stored
//! through the real write path (`store::write::commit`), and the fixture
//! Endstep match synced by the tracker.

use std::sync::Arc;

use fixture_tools::TestDir;
use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome, Resolution};
use store::arena::{PlatformAccount, PlatformOpponent};
use store::write::{MatchWrite, WriteBatch, commit};
use store::{Connection, Platform};

/// The Endstep match `token` with one game; returns that game's row id (the
/// newest game row, so call it on a database without other games).
pub(crate) fn seed_endstep_match(conn: &mut Connection, token: &str) -> i64 {
    let view = MatchView {
        token: token.to_string(),
        mtgo_match_id: None,
        local_username: "endstep:u1".to_string(),
        opponent: Some("endstep:u2".to_string()),
        format: Some("CMODERN".to_string()),
        match_type: Some("Constructed".to_string()),
        tournament_event_id: None,
        tournament_round: None,
        league_token: None,
        game_structure: None,
        state: MatchState::Complete,
        resolution: Some(Resolution::Signal),
        outcome: Outcome::Win,
        tentative_outcome: None,
        wins: 1,
        losses: 0,
        started_at_ms: 1_000,
        ended_at_ms: Some(90_000),
        last_activity_ms: 90_000,
        games: vec![GameView {
            mtgo_game_id: Some(7_001),
            game_number: 1,
            result: GameResult::Win,
            end_reason: EndReason::Win,
            on_play: Some(true),
            local_mulligans: Some(0),
            opp_mulligans: Some(0),
            local_die_roll: None,
            opp_die_roll: None,
            turn_count: Some(7),
            started_at_ms: Some(1_000),
            ended_at_ms: Some(90_000),
        }],
    };
    commit(
        conn,
        &WriteBatch {
            platform_accounts: vec![PlatformAccount {
                platform: Platform::Endstep,
                username: "endstep:u1".to_string(),
                user_id: None,
                display_name: Some("Player".to_string()),
                seen_at_ms: 1,
            }],
            platform_opponents: vec![PlatformOpponent {
                platform: Platform::Endstep,
                username: "endstep:u2".to_string(),
                display_name: "Opponent".to_string(),
            }],
            matches: vec![MatchWrite::Upsert {
                view: Box::new(view),
                revision: 1,
            }],
            now_ms: 100_000,
            ..WriteBatch::default()
        },
    )
    .expect("endstep match");
    conn.query_row("SELECT MAX(id) FROM games", [], |row| row.get(0))
        .expect("endstep game")
}

/// The Endstep match id (the stored token) of the fixture match.
pub(crate) const ENDSTEP_FIXTURE_TOKEN: &str = "00000000-0000-4000-8000-e0000000003a";

/// The fixture Endstep match (`fixtures/endstep/ranked-modern-bo3`, a Bo3
/// on Endstep with the Tron deck) synced by the tracker into a new database
/// at `db_path`, ticking from `now_ms` until nothing is left.
pub(crate) fn sync_endstep_fixture(db_path: &std::path::Path, dir: &TestDir, now_ms: i64) {
    let config = tracker::TrackerConfig {
        db_path: db_path.to_path_buf(),
        timezone: chrono_tz::UTC,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: false,
        endstep_enabled: true,
    };
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/endstep/ranked-modern-bo3");
    let scan = crate::endstep::import::scan_inbox(&fixture, true);
    assert_eq!(scan.captures.len(), 5, "the fixture units");
    let mut engine = tracker::Engine::open(
        config,
        mtgo_log::Env::current(),
        Arc::default(),
        Vec::new(),
        now_ms,
    )
    .expect("open engine");
    engine
        .store_endstep_captures(scan.captures, now_ms)
        .expect("store captures");
    let mut now = now_ms;
    for _ in 0..1_000 {
        let tick = engine.tick(now).expect("tick");
        now += 1;
        if !tick.more {
            break;
        }
    }
    for _ in 0..3 {
        now += 1_000;
        engine.tick(now).expect("tick");
    }
    engine.flush().expect("flush");
}
