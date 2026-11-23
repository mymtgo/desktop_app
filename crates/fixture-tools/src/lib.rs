//! Development tooling: fixture loading, anonymisation, synthetic events, measurement.

pub use mtgo_protocol::anonymise;
pub mod dat;
pub mod history;
pub mod synth;
pub mod testdir;
pub mod timing;

pub use testdir::TestDir;

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use mtgo_protocol::{FileContext, PreparedEvent, split_complete};
use serde::{Deserialize, Serialize};

/// Sidecar metadata for a fixture log (`<name>.meta.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureMeta {
    /// Local date the log's first event belongs to, `YYYY-MM-DD`.
    pub date: String,
    /// IANA timezone the log was written in.
    pub timezone: String,
    /// The (fake) local player name.
    pub local_username: String,
    /// RFC 3339 UTC instant used as `now` for finalize rules.
    pub now: String,
}

impl FixtureMeta {
    pub fn day_anchor(&self) -> NaiveDate {
        NaiveDate::parse_from_str(&self.date, "%Y-%m-%d").expect("fixture date is YYYY-MM-DD")
    }

    pub fn tz(&self) -> Tz {
        self.timezone
            .parse()
            .expect("fixture timezone is an IANA name")
    }

    pub fn now_ms(&self) -> i64 {
        DateTime::parse_from_rfc3339(&self.now)
            .expect("fixture now is RFC 3339")
            .timestamp_millis()
    }
}

pub struct Fixture {
    pub name: String,
    pub bytes: Vec<u8>,
    pub meta: FixtureMeta,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Every fixture log name (without `.log`), sorted.
pub fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures_dir().join("logs"))
        .expect("fixtures/logs exists")
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.strip_suffix(".log").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// The bytes of an anonymised game log fixture, `fixtures/gamelogs/<name>.dat`.
pub fn load_game_log(name: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join("gamelogs").join(format!("{name}.dat")))
        .expect("game log fixture exists")
}

pub fn load_fixture(name: &str) -> Fixture {
    let dir = fixtures_dir().join("logs");
    let bytes = std::fs::read(dir.join(format!("{name}.log"))).expect("fixture log exists");
    let meta_text = std::fs::read_to_string(dir.join(format!("{name}.meta.json")))
        .expect("fixture meta exists");
    let meta = serde_json::from_str(&meta_text).expect("fixture meta is valid JSON");
    Fixture {
        name: name.to_string(),
        bytes,
        meta,
    }
}

/// Sidecar metadata for an Arena fixture log (`fixtures/arena/<name>.meta.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArenaFixtureMeta {
    /// Minutes the machine that wrote the log was ahead of UTC.
    pub utc_offset_minutes: i32,
    /// Whether the log header's dates are DD/MM (true) or MM/DD (false).
    pub day_first: bool,
    /// The (fake) local display name, tag included.
    pub local_display_name: String,
    /// The (fake) local user id.
    pub local_user_id: String,
}

pub struct ArenaFixture {
    pub name: String,
    pub bytes: Vec<u8>,
    pub meta: ArenaFixtureMeta,
}

pub fn arena_fixture(name: &str) -> ArenaFixture {
    let dir = fixtures_dir().join("arena");
    let bytes = std::fs::read(dir.join(format!("{name}.log"))).expect("arena fixture log exists");
    let meta_text = std::fs::read_to_string(dir.join(format!("{name}.meta.json")))
        .expect("arena fixture meta exists");
    let meta = serde_json::from_str(&meta_text).expect("arena fixture meta is valid JSON");
    ArenaFixture {
        name: name.to_string(),
        bytes,
        meta,
    }
}

/// The fixture's whitelisted events, prepared as the live pipeline would (file id 1).
pub fn prepared_events(fixture: &Fixture) -> Vec<PreparedEvent> {
    let mut ctx = FileContext::new(1, fixture.meta.day_anchor(), fixture.meta.tz(), None);
    split_complete(&fixture.bytes)
        .iter()
        .filter_map(|raw| ctx.prepare(raw))
        .collect()
}

#[cfg(test)]
mod arena_tests {
    use super::*;

    fn contains(bytes: &[u8], needle: &str) -> bool {
        let needle = needle.as_bytes();
        bytes.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn arena_fixture_loads() {
        let fixture = arena_fixture("bo3-loss-and-match-concede");
        assert!(contains(&fixture.bytes, "DETAILED LOGS: ENABLED"));
        assert!(contains(&fixture.bytes, "Traditional_Explorer_Ladder"));
        assert_eq!(fixture.meta.local_display_name, "Player#00001");
        assert!(fixture.meta.day_first);
    }
}
