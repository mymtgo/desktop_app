//! The time the supervisor gives the engine as `now` (#858). The app runs on
//! the real clock. A replay (the log player, a headless test) runs on the
//! log's time, so the time rules (abandon, retire, the quiet commit, the
//! league and tournament sync delays) judge the log, not the day the replay
//! runs.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Which time the tracker runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clock {
    /// The wall clock: the app, always.
    #[default]
    Real,
    /// The log's time: the newest event time read so far, plus the real time
    /// that has passed since that event was read. For replays only.
    Log,
}

/// A `Clock` and what the log clock remembers between ticks. It lives in the
/// supervisor, outside each engine, so a restarted engine keeps the log time.
#[derive(Debug)]
pub struct TrackerClock {
    clock: Clock,
    /// The newest event time read (0 before the first one).
    newest_ms: i64,
    /// When `newest_ms` was first seen.
    newest_seen: Instant,
    /// The last time handed out: the log clock never runs backwards.
    last_ms: i64,
}

impl TrackerClock {
    pub fn new(clock: Clock) -> Self {
        Self::starting_at(clock, Instant::now())
    }

    fn starting_at(clock: Clock, start: Instant) -> Self {
        Self {
            clock,
            newest_ms: 0,
            newest_seen: start,
            last_ms: 0,
        }
    }

    /// `now` in UTC unix milliseconds. `newest_log_ms` is the newest event
    /// time the engine has read (0 before the first; ignored by `Real`).
    pub fn now_ms(&mut self, newest_log_ms: i64) -> i64 {
        match self.clock {
            Clock::Real => wall_now_ms(),
            Clock::Log => self.log_now_ms(newest_log_ms, Instant::now()),
        }
    }

    /// The log clock as of the real instant `at`.
    ///
    /// Why the real time since the newest event is added: a log that stops
    /// growing is silent, and silence is how most time rules fire (the last
    /// event commits after 300 ms of quiet, a sync runs a second after the
    /// change that asked for it, a match is abandoned after an hour). With
    /// the newest event time alone, `now` would freeze when the log stops
    /// and those rules would never run. Live, MTGO writes as things happen,
    /// so the real time since the last line is the log's silence: a replay
    /// at real speed runs exactly as live did, and a fast replay only
    /// shortens the gaps between its events, never the time rules over them.
    fn log_now_ms(&mut self, newest_log_ms: i64, at: Instant) -> i64 {
        if newest_log_ms > self.newest_ms {
            self.newest_ms = newest_log_ms;
            self.newest_seen = at;
        }
        let since = at.saturating_duration_since(self.newest_seen).as_millis() as i64;
        self.last_ms = self.last_ms.max(self.newest_ms + since);
        self.last_ms
    }
}

/// The wall clock in UTC unix milliseconds.
pub fn wall_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const NEWEST: i64 = 1_787_400_000_000;

    fn after(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn the_log_clock_is_the_newest_event_time_when_it_is_read() {
        let start = Instant::now();
        let mut clock = TrackerClock::starting_at(Clock::Log, start);
        assert_eq!(clock.log_now_ms(NEWEST, after(start, 5_000)), NEWEST);
    }

    #[test]
    fn a_silent_log_moves_on_with_real_time() {
        let start = Instant::now();
        let mut clock = TrackerClock::starting_at(Clock::Log, start);
        clock.log_now_ms(NEWEST, start);
        assert_eq!(
            clock.log_now_ms(NEWEST, after(start, 1_500)),
            NEWEST + 1_500
        );
    }

    #[test]
    fn a_newer_event_moves_the_log_clock_to_its_time() {
        let start = Instant::now();
        let mut clock = TrackerClock::starting_at(Clock::Log, start);
        clock.log_now_ms(NEWEST, start);
        let hour_later = NEWEST + 60 * 60 * 1000;
        assert_eq!(clock.log_now_ms(hour_later, after(start, 10)), hour_later);
    }

    #[test]
    fn the_log_clock_never_runs_backwards() {
        let start = Instant::now();
        let mut clock = TrackerClock::starting_at(Clock::Log, start);
        clock.log_now_ms(NEWEST, start);
        // Ten real seconds of silence, then an event one second after the
        // last: the clock holds at ten seconds past instead of going back.
        assert_eq!(
            clock.log_now_ms(NEWEST, after(start, 10_000)),
            NEWEST + 10_000
        );
        assert_eq!(
            clock.log_now_ms(NEWEST + 1_000, after(start, 10_000)),
            NEWEST + 10_000
        );
        // An older engine figure (a restarted engine reads from 0) keeps
        // the newest time already seen.
        assert_eq!(clock.log_now_ms(0, after(start, 10_000)), NEWEST + 10_000);
    }

    #[test]
    fn before_any_event_the_log_clock_counts_real_time_from_zero() {
        let start = Instant::now();
        let mut clock = TrackerClock::starting_at(Clock::Log, start);
        assert_eq!(clock.log_now_ms(0, after(start, 2_000)), 2_000);
    }

    #[test]
    fn the_real_clock_ignores_the_log() {
        let mut clock = TrackerClock::new(Clock::Real);
        let before = wall_now_ms();
        let now = clock.now_ms(NEWEST);
        assert!(now >= before && now <= wall_now_ms());
    }
}
