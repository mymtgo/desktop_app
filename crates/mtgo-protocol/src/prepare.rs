//! Turning raw events into prepared events: header, classification, UTC time, and
//! the local username known for the file at that point.

use chrono::NaiveDate;
use chrono_tz::Tz;

use crate::classify::{EventKind, Subjects, classify};
use crate::header::{Header, parse_header};
use crate::login::parse_login;
use crate::splitter::RawEvent;
use crate::time::{ClockState, DayClock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedEvent {
    pub file_id: u64,
    pub offset: u64,
    pub at_ms: i64,
    pub header: Header,
    pub kind: EventKind,
    pub subjects: Subjects,
    pub text: String,
    /// The local player's username as known for this file when the event was read.
    pub local_username: Option<String>,
}

impl PreparedEvent {
    /// Ordering key: events of a match are always processed in this order.
    pub fn key(&self) -> (u64, u64) {
        (self.file_id, self.offset)
    }
}

/// Per-log-file state needed to prepare events in byte order.
#[derive(Debug, Clone)]
pub struct FileContext {
    pub file_id: u64,
    clock: DayClock,
    local_username: Option<String>,
    local_login_id: Option<u64>,
    /// Events whose header could not be parsed.
    pub malformed: u64,
}

impl FileContext {
    pub fn new(
        file_id: u64,
        day_anchor: NaiveDate,
        tz: Tz,
        inherited_username: Option<String>,
    ) -> Self {
        Self {
            file_id,
            clock: DayClock::new(day_anchor, tz),
            local_username: inherited_username,
            local_login_id: None,
            malformed: 0,
        }
    }

    /// Continue a file where an earlier run stopped (restart after a crash or quit).
    pub fn resume(
        file_id: u64,
        tz: Tz,
        clock: ClockState,
        local_username: Option<String>,
        local_login_id: Option<u64>,
    ) -> Self {
        Self {
            file_id,
            clock: DayClock::resume(clock, tz),
            local_username,
            local_login_id,
            malformed: 0,
        }
    }

    pub fn clock_state(&self) -> ClockState {
        self.clock.state()
    }

    pub fn local_username(&self) -> Option<&str> {
        self.local_username.as_deref()
    }

    pub fn local_login_id(&self) -> Option<u64> {
        self.local_login_id
    }

    /// Prepare one event from a catch-up read. Returns `None` for events that are
    /// not whitelisted.
    pub fn prepare(&mut self, raw: &RawEvent) -> Option<PreparedEvent> {
        self.prepare_with(raw, None)
    }

    /// Prepare one event. `live_read_at_ms` is the read time when the event was
    /// appended while the file was being followed live (spec 5.4), so its date can
    /// be cross-checked. Every parseable event advances the clock, whitelisted or
    /// not, so midnight rollover is detected from the full stream.
    pub fn prepare_with(
        &mut self,
        raw: &RawEvent,
        live_read_at_ms: Option<i64>,
    ) -> Option<PreparedEvent> {
        let Some(header) = parse_header(&raw.text) else {
            self.malformed += 1;
            return None;
        };
        let at_ms = match live_read_at_ms {
            Some(read_at_ms) => self.clock.resolve_live(header.time, read_at_ms),
            None => self.clock.resolve(header.time),
        };
        let classified = classify(&header, &raw.text)?;

        if classified.kind == EventKind::Login
            && let Some(login) = parse_login(&raw.text)
        {
            self.local_username = Some(login.username);
            self.local_login_id = login.login_id;
        }

        Some(PreparedEvent {
            file_id: self.file_id,
            offset: raw.offset,
            at_ms,
            header,
            kind: classified.kind,
            subjects: classified.subjects,
            text: raw.text.clone(),
            local_username: self.local_username.clone(),
        })
    }
}

/// Prepare an event whose UTC time is already known (rebuild from storage, tests).
pub fn prepare_at(
    file_id: u64,
    offset: u64,
    at_ms: i64,
    text: &str,
    local_username: Option<String>,
) -> Option<PreparedEvent> {
    let header = parse_header(text)?;
    let classified = classify(&header, text)?;
    Some(PreparedEvent {
        file_id,
        offset,
        at_ms,
        header,
        kind: classified.kind,
        subjects: classified.subjects,
        text: text.to_string(),
        local_username,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splitter::split_complete;
    use chrono::NaiveDate;

    const LOG: &str = "12:23:25 [INF] (Login|MtGO Login Success) Username: Local_Name (6031972)\n12:23:26 [INF] (UI|not whitelisted)\n12:23:54 [INF] (Game Management|Match State Changed for b2723ac5-787a-42a0-8692-ca205448f6f2 from A to B)\n";

    #[test]
    fn prepares_whitelisted_events_and_tracks_the_login() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 22).expect("date");
        let mut ctx = FileContext::new(7, date, chrono_tz::UTC, None);
        let prepared: Vec<_> = split_complete(LOG.as_bytes())
            .iter()
            .filter_map(|e| ctx.prepare(e))
            .collect();
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].kind, EventKind::Login);
        assert_eq!(prepared[1].kind, EventKind::MatchStateChanged);
        assert_eq!(prepared[1].local_username.as_deref(), Some("Local_Name"));
        assert_eq!(prepared[1].key().0, 7);
        assert_eq!(ctx.local_login_id(), Some(6_031_972));
    }

    #[test]
    fn inherited_username_applies_before_any_login_line() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 22).expect("date");
        let mut ctx = FileContext::new(1, date, chrono_tz::UTC, Some("Carried".into()));
        let text = "12:00:00 [INF] (Game Management|Match State Changed for b2723ac5-787a-42a0-8692-ca205448f6f2 from A to B)";
        let event = ctx
            .prepare(&split_complete(text.as_bytes())[0])
            .expect("prepared");
        assert_eq!(event.local_username.as_deref(), Some("Carried"));
    }

    #[test]
    fn counts_events_with_unparseable_headers() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 22).expect("date");
        let mut ctx = FileContext::new(1, date, chrono_tz::UTC, None);
        let raw = RawEvent {
            offset: 0,
            len: 5,
            text: "12:00:00 [INF] no parens".into(),
        };
        assert_eq!(ctx.prepare(&raw), None);
        assert_eq!(ctx.malformed, 1);
    }

    #[test]
    fn a_resumed_context_continues_the_clock_and_username() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 22).expect("date");
        let mut first = FileContext::new(3, date, chrono_tz::UTC, None);
        let events = split_complete(LOG.as_bytes());
        first.prepare(&events[0]);

        let mut resumed = FileContext::resume(
            3,
            chrono_tz::UTC,
            first.clock_state(),
            first.local_username().map(str::to_string),
            first.local_login_id(),
        );
        let a = first.prepare(&events[2]).expect("prepared");
        let b = resumed.prepare(&events[2]).expect("prepared");
        assert_eq!(a, b);
        assert_eq!(resumed.local_login_id(), Some(6_031_972));
    }

    #[test]
    fn live_preparation_cross_checks_the_date_against_the_read_time() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 20).expect("date");
        let mut ctx = FileContext::new(1, date, chrono_tz::UTC, None);
        let text = "12:00:00 [INF] (Game Management|Match State Changed for b2723ac5-787a-42a0-8692-ca205448f6f2 from A to B)";
        let raw = &split_complete(text.as_bytes())[0];
        let read_at = NaiveDate::from_ymd_opt(2026, 8, 23)
            .and_then(|d| d.and_hms_opt(12, 0, 1))
            .expect("read time")
            .and_utc()
            .timestamp_millis();
        let event = ctx.prepare_with(raw, Some(read_at)).expect("prepared");
        assert_eq!(event.at_ms, read_at - 1000);
        assert_eq!(
            ctx.clock_state().date,
            NaiveDate::from_ymd_opt(2026, 8, 23).expect("date")
        );
    }

    #[test]
    fn a_resumed_context_rolls_the_date_across_midnight_like_the_original() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 22).expect("date");
        let late = "23:59:50 [INF] (Game Management|Match State Changed for b2723ac5-787a-42a0-8692-ca205448f6f2 from A to B)";
        let early = "00:00:05 [INF] (Game Management|Match State Changed for b2723ac5-787a-42a0-8692-ca205448f6f2 from B to C)";
        let mut first = FileContext::new(3, date, chrono_tz::UTC, None);
        first
            .prepare(&split_complete(late.as_bytes())[0])
            .expect("prepared");

        let mut resumed = FileContext::resume(
            3,
            chrono_tz::UTC,
            first.clock_state(),
            first.local_username().map(str::to_string),
            first.local_login_id(),
        );
        let raw = &split_complete(early.as_bytes())[0];
        let a = first.prepare(raw).expect("prepared");
        let b = resumed.prepare(raw).expect("prepared");
        assert_eq!(a, b);
        let expected = NaiveDate::from_ymd_opt(2026, 8, 23)
            .and_then(|d| d.and_hms_opt(0, 0, 5))
            .expect("time")
            .and_utc()
            .timestamp_millis();
        assert_eq!(b.at_ms, expected);
        assert_eq!(
            resumed.clock_state().date,
            NaiveDate::from_ymd_opt(2026, 8, 23).expect("date")
        );
    }
}
