//! Replays a real `mtgo.log` into a folder, event by event, so the tracker can be
//! developed and measured without MTGO (spec 9, dev tooling). Never prints log
//! content: real logs contain real player names.

pub mod cli;

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::NaiveTime;
use mtgo_protocol::{parse_header, split_complete};

/// The longest pause between two events, whatever the log says.
pub const MAX_GAP: Duration = Duration::from_secs(2);

/// One write: the destination now holds the source up to `end_offset`.
#[derive(Debug, Clone, Copy)]
pub struct Written {
    pub end_offset: u64,
    pub at: Instant,
}

/// Create `dest` (and its folder) empty, so a replay does not append to an
/// earlier one.
pub fn start_fresh(dest: &Path) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::File::create(dest)?;
    Ok(())
}

/// Append `source` to `dest` one event at a time, exactly (line endings
/// included). Pauses follow the header times divided by `speed`; 0 means none.
/// `dest` should start empty (see `start_fresh`): bytes already in it would
/// shift every offset the tracker sees.
pub fn play(
    source: &[u8],
    dest: &Path,
    speed: f64,
    mut on_write: impl FnMut(Written),
) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(dest)?;
    let events = split_complete(source);

    // Lines before the first header (a log that starts mid-event).
    let first = events
        .first()
        .map_or(source.len(), |event| event.offset as usize);
    if first > 0 {
        file.write_all(&source[..first])?;
        file.flush()?;
        on_write(Written {
            end_offset: first as u64,
            at: Instant::now(),
        });
    }

    let mut previous: Option<NaiveTime> = None;
    for event in &events {
        let time = parse_header(&event.text).map(|header| header.time);
        if speed > 0.0
            && let (Some(before), Some(after)) = (previous, time)
        {
            std::thread::sleep(gap(before, after, speed));
        }
        if time.is_some() {
            previous = time;
        }
        let start = event.offset as usize;
        // `split_complete` counts a missing final newline as one extra byte.
        let end = ((event.offset + event.len) as usize).min(source.len());
        file.write_all(&source[start..end])?;
        file.flush()?;
        on_write(Written {
            end_offset: end as u64,
            at: Instant::now(),
        });
    }
    Ok(())
}

/// The pause between two header times at `speed`. A wrap past midnight counts
/// forward; a small step back counts as no pause.
pub fn gap(before: NaiveTime, after: NaiveTime, speed: f64) -> Duration {
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let mut ms = (after - before).num_milliseconds();
    if ms < -DAY_MS / 2 {
        ms += DAY_MS;
    }
    if ms <= 0 || speed <= 0.0 {
        return Duration::ZERO;
    }
    Duration::from_millis((ms as f64 / speed) as u64).min(MAX_GAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::{TestDir, load_fixture};

    fn t(h: u32, m: u32, s: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, s).expect("time")
    }

    #[test]
    fn replays_a_fixture_byte_for_byte() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("player-fixture");
        let dest = dir.join("out/mtgo.log");
        let mut writes = Vec::new();
        play(&fixture.bytes, &dest, 0.0, |written| writes.push(written)).expect("play");
        assert_eq!(std::fs::read(&dest).expect("read"), fixture.bytes);
        assert_eq!(
            writes.last().expect("writes").end_offset,
            fixture.bytes.len() as u64
        );
        assert!(
            writes
                .windows(2)
                .all(|pair| pair[0].end_offset < pair[1].end_offset)
        );
    }

    #[test]
    fn a_log_without_a_final_newline_is_copied_exactly() {
        let source = b"orphan line\n12:00:00 [INF] (UI|a)\r\n12:00:01 [INF] (UI|b)";
        let dir = TestDir::new("player-tail");
        let dest = dir.join("mtgo.log");
        play(source, &dest, 0.0, |_| {}).expect("play");
        assert_eq!(std::fs::read(&dest).expect("read"), source);
    }

    #[test]
    fn start_fresh_empties_an_earlier_replay() {
        let source = b"12:00:00 [INF] (UI|a)\n12:00:01 [INF] (UI|b)\n";
        let dir = TestDir::new("player-fresh");
        let dest = dir.join("out/mtgo.log");
        start_fresh(&dest).expect("fresh");
        play(source, &dest, 0.0, |_| {}).expect("first play");
        start_fresh(&dest).expect("fresh again");
        assert_eq!(std::fs::read(&dest).expect("read"), b"");
        play(source, &dest, 0.0, |_| {}).expect("second play");
        assert_eq!(std::fs::read(&dest).expect("read"), source);
    }

    #[test]
    fn gaps_follow_header_times() {
        assert_eq!(gap(t(12, 0, 0), t(12, 0, 1), 1.0), Duration::from_secs(1));
        assert_eq!(
            gap(t(12, 0, 0), t(12, 0, 1), 10.0),
            Duration::from_millis(100)
        );
        assert_eq!(gap(t(23, 59, 59), t(0, 0, 1), 1.0), Duration::from_secs(2));
        assert_eq!(gap(t(12, 0, 5), t(12, 0, 1), 1.0), Duration::ZERO);
        assert_eq!(gap(t(12, 0, 0), t(13, 0, 0), 1.0), MAX_GAP);
        assert_eq!(gap(t(12, 0, 0), t(12, 0, 1), 0.0), Duration::ZERO);
    }
}
