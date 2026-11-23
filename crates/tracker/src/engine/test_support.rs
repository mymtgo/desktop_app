//! Helpers shared by the engine's unit tests. The integration tests in
//! `tests/` keep their own copy in `tests/common/mod.rs`: `#[cfg(test)]`
//! code is not compiled into the library they link against.

use std::sync::Arc;

use fixture_tools::{Fixture, TestDir};
use mtgo_log::Env;

use super::Engine;
use crate::{TrackerConfig, TrackerEvent};

/// Most ticks `run_until_settled` gives each phase before failing the test.
const MAX_TICKS: usize = 20_000;

/// A tracker reading `logs/` in `dir`, with the fixture's timezone and day.
pub(crate) fn config(dir: &TestDir, fixture: &Fixture) -> TrackerConfig {
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

pub(crate) fn open(config: &TrackerConfig, now_ms: i64) -> Engine {
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
/// pushed and the final `now`.
pub(crate) fn run_until_settled(engine: &mut Engine, start_ms: i64) -> (Vec<TrackerEvent>, i64) {
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
