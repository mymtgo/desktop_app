//! Helpers shared by the tracker's integration tests.
#![allow(dead_code)] // each test file uses a different subset

use std::collections::BTreeMap;
use std::sync::Arc;

use fixture_tools::{Fixture, TestDir, prepared_events};
use mtgo_log::Env;
use mtgo_match::{MatchView, Projector};
use mtgo_protocol::EventKind;
use store::{open_reader, read};
use tracker::history::History;
use tracker::{Engine, Status, TrackerConfig, TrackerEvent};

/// Most ticks `run_until_settled` gives each phase before failing the test.
const MAX_TICKS: usize = 20_000;

pub fn config(dir: &TestDir, fixture: &Fixture) -> TrackerConfig {
    TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: fixture.meta.tz(),
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: Some(fixture.meta.day_anchor()),
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    }
}

pub fn open(config: &TrackerConfig, now_ms: i64) -> Engine {
    Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        now_ms,
    )
    .expect("open engine")
}

/// Tick until every byte is read, the provisional last event has committed
/// and the game syncs are done, then commit. Returns the events the ticks
/// pushed and the final `now`. The engine's unit tests have the same helper
/// in `engine::test_support`.
pub fn run_until_settled(engine: &mut Engine, start_ms: i64) -> (Vec<TrackerEvent>, i64) {
    let mut now = start_ms;
    let mut events = Vec::new();
    let mut read_all = false;
    for _ in 0..MAX_TICKS {
        let tick = engine.tick(now).expect("tick");
        events.extend(tick.events);
        now += 1;
        if !tick.more {
            read_all = true;
            break;
        }
    }
    assert!(read_all, "the log was not read within {MAX_TICKS} ticks");
    // Past the quiet period, so the provisional last event commits.
    for _ in 0..3 {
        now += 1_000;
        events.extend(engine.tick(now).expect("tick").events);
    }
    // The game syncs go one slice at a time, each sent once the last is
    // answered (the supervisor ticks again at once).
    let mut synced = false;
    for _ in 0..MAX_TICKS {
        engine.flush().expect("flush");
        now += 1;
        let tick = engine.tick(now).expect("tick");
        events.extend(tick.events);
        if !tick.syncing {
            synced = true;
            break;
        }
    }
    assert!(
        synced,
        "the game syncs did not finish within {MAX_TICKS} ticks"
    );
    engine.flush().expect("flush");
    (events, now)
}

/// What the projector alone makes of the fixture (Login lines add accounts, as the
/// engine does).
pub fn expected(fixture: &Fixture, now_ms: i64) -> Vec<MatchView> {
    let mut projector = Projector::new(vec![]);
    for event in prepared_events(fixture) {
        if event.kind == EventKind::Login
            && let Some(name) = &event.local_username
        {
            projector.add_account(name);
        }
        projector.apply(&event);
    }
    projector.finalize(now_ms);
    projector.views().into_iter().cloned().collect()
}

/// The last pushed view of every match still visible, in the projector's order.
pub fn last_views(events: &[TrackerEvent]) -> Vec<MatchView> {
    let mut latest: BTreeMap<String, MatchView> = BTreeMap::new();
    for event in events {
        match event {
            TrackerEvent::MatchUpdated { view, .. } => {
                latest.insert(view.token.clone(), view.clone());
            }
            TrackerEvent::MatchRemoved { token } => {
                latest.remove(token);
            }
            _ => {}
        }
    }
    let mut views: Vec<MatchView> = latest.into_values().collect();
    views.sort_by(|a, b| {
        a.started_at_ms
            .cmp(&b.started_at_ms)
            .then_with(|| a.token.cmp(&b.token))
    });
    views
}

pub fn rebuilt_from_storage(
    config: &TrackerConfig,
    tokens: &[String],
    now_ms: i64,
) -> Vec<MatchView> {
    let reader = open_reader(&config.db_path).expect("reader");
    let mut history = History::default();
    let mut views = Vec::new();
    for token in tokens {
        let mut projector = Projector::new(read::known_accounts(&reader).expect("accounts"));
        for event in history.match_events(&reader, token).expect("history") {
            projector.apply(&event);
        }
        projector.finalize(now_ms);
        views.extend(projector.view(token).cloned());
    }
    views
}

pub fn last_status(events: &[TrackerEvent]) -> Option<Status> {
    events.iter().rev().find_map(|event| match event {
        TrackerEvent::Status(status) => Some(status.clone()),
        _ => None,
    })
}

/// Events stored, counting archived ones and the `InPlayoffs` lines kept in
/// their own table.
pub fn stored_event_count(config: &TrackerConfig) -> i64 {
    let reader = open_reader(&config.db_path).expect("reader");
    let rows: i64 = reader
        .query_row("SELECT COUNT(*) FROM log_events", [], |row| row.get(0))
        .expect("count");
    let archived: i64 = reader
        .query_row(
            "SELECT COALESCE(SUM(event_count), 0) FROM match_event_archives",
            [],
            |row| row.get(0),
        )
        .expect("sum");
    let player_states: i64 = reader
        .query_row("SELECT COUNT(*) FROM tournament_player_states", [], |row| {
            row.get(0)
        })
        .expect("count");
    rows + archived + player_states
}

/// The committed cursor of the active log file, once the database exists.
pub fn committed_cursor(config: &TrackerConfig) -> Option<i64> {
    let reader = open_reader(&config.db_path).ok()?;
    reader
        .query_row(
            "SELECT cursor_offset FROM log_files WHERE sealed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .ok()
}
