//! mtgo.log lines carry only `HH:MM:SS`. `DayClock` rebuilds full UTC instants from a
//! per-file day anchor and timezone, rolling the date forward when the time of day
//! wraps around midnight. Feed it every event's time in byte order.
//!
//! Time of day alone cannot see a silent gap of 12 to 24 hours (08:00 followed by
//! 07:00 the next day looks like a small step back). Events read live, moments after
//! MTGO wrote them, are cross-checked against their read time (`resolve_live`).

use chrono::{
    DateTime, Days, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone,
};
use chrono_tz::Tz;

/// How far a live event may sit from its read time before its date is re-anchored.
const LIVE_DRIFT_MS: i64 = 12 * 60 * 60 * 1000;

/// Everything needed to continue a clock later (persisted with the file cursor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockState {
    /// The running local date.
    pub date: NaiveDate,
    /// Time of day of the last event, if any.
    pub last: Option<NaiveTime>,
}

#[derive(Debug, Clone)]
pub struct DayClock {
    tz: Tz,
    date: NaiveDate,
    last: Option<NaiveTime>,
}

impl DayClock {
    pub fn new(day_anchor: NaiveDate, tz: Tz) -> Self {
        Self {
            tz,
            date: day_anchor,
            last: None,
        }
    }

    pub fn resume(state: ClockState, tz: Tz) -> Self {
        Self {
            tz,
            date: state.date,
            last: state.last,
        }
    }

    pub fn state(&self) -> ClockState {
        ClockState {
            date: self.date,
            last: self.last,
        }
    }

    /// UTC unix milliseconds for the next event's time of day.
    pub fn resolve(&mut self, time: NaiveTime) -> i64 {
        if let Some(last) = self.last
            && last.signed_duration_since(time) > Duration::hours(12)
        {
            self.date = self.date.succ_opt().unwrap_or(self.date);
        }
        self.last = Some(time);
        self.instant(time)
    }

    /// Like `resolve`, for an event written moments before `read_at_ms`. If the
    /// running date puts it more than 12 hours from its read time, the date is
    /// re-anchored to the read time's local date, stepping back a day when that
    /// would put the event more than 12 hours after the read (0.x's rule).
    pub fn resolve_live(&mut self, time: NaiveTime, read_at_ms: i64) -> i64 {
        let at = self.resolve(time);
        if (at - read_at_ms).abs() <= LIVE_DRIFT_MS {
            return at;
        }
        let Some(read_at) = DateTime::from_timestamp_millis(read_at_ms) else {
            return at;
        };
        self.date = read_at.with_timezone(&self.tz).date_naive();
        let mut at = self.instant(time);
        if at > read_at_ms + LIVE_DRIFT_MS {
            self.date = self.date.pred_opt().unwrap_or(self.date);
            at = self.instant(time);
        }
        at
    }

    fn instant(&self, time: NaiveTime) -> i64 {
        let local = NaiveDateTime::new(self.date, time);
        local_to_utc_ms(local, self.tz).unwrap_or_else(|| local.and_utc().timestamp_millis())
    }
}

/// A local wall-clock time in `tz` as UTC unix milliseconds, the way 0.x's
/// `Carbon::parse($text, $tz)` (PHP `DateTime`) reads it. A time the clocks
/// skip (inside a DST gap) is moved on by the real length of the gap: 01:30
/// in London's one hour gap is 02:30 BST, 02:15 in Lord Howe's half hour gap
/// is 02:45. A time that happens twice takes the earlier instant. `None` only
/// for a time outside chrono's range.
pub fn local_to_utc_ms(wall_clock: NaiveDateTime, tz: Tz) -> Option<i64> {
    let instant = match tz.from_local_datetime(&wall_clock) {
        LocalResult::Single(at) => at,
        LocalResult::Ambiguous(earliest, _) => earliest,
        LocalResult::None => past_the_gap(wall_clock, tz)?,
    };
    Some(instant.timestamp_millis())
}

/// A wall-clock time that does not exist (inside a DST gap) moved on by the
/// real length of the gap, as PHP does. The gap is the offset change read a
/// day either side.
fn past_the_gap(wall_clock: NaiveDateTime, tz: Tz) -> Option<DateTime<Tz>> {
    let day = Duration::days(1);
    let before = tz
        .offset_from_utc_datetime(&(wall_clock.checked_sub_signed(day)?))
        .fix();
    let after = tz
        .offset_from_utc_datetime(&(wall_clock.checked_add_signed(day)?))
        .fix();
    let gap = i64::from(after.local_minus_utc()) - i64::from(before.local_minus_utc());
    let gap = if gap > 0 { gap } else { 3600 };
    tz.from_local_datetime(&(wall_clock.checked_add_signed(Duration::seconds(gap))?))
        .earliest()
}

/// The local date a file's first event belongs to, worked back from the file's
/// last write (spec 5.4 rule 1, issue #48). MTGO rewrites mtgo.log in place when
/// it starts and Windows keeps the old creation time, so the creation date can be
/// days out; the last line was written at the last write. `times` are the file's
/// event times of day in byte order. `None` when there are none.
pub fn day_anchor_from_last_write(
    times: &[NaiveTime],
    last_write_ms: i64,
    tz: Tz,
) -> Option<NaiveDate> {
    let last = *times.last()?;
    let written = DateTime::from_timestamp_millis(last_write_ms)?
        .with_timezone(&tz)
        .naive_local();
    // The last line's date: the write's, or the day before when the line's time
    // is far after the write's (written before midnight, flushed after).
    let mut date = written.date();
    if last.signed_duration_since(written.time()) > Duration::hours(12) {
        date = date.pred_opt()?;
    }
    // Step back over each midnight between the first line and the last, using
    // the same rule as `DayClock::resolve`.
    let mut rollovers: u64 = 0;
    for pair in times.windows(2) {
        if pair[0].signed_duration_since(pair[1]) > Duration::hours(12) {
            rollovers += 1;
        }
    }
    date.checked_sub_days(Days::new(rollovers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};

    fn t(h: u32, m: u32, s: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, s).expect("time")
    }

    fn day(y: i32, mo: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, mo, d).expect("date")
    }

    fn london_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        chrono_tz::Europe::London
            .with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("london")
            .timestamp_millis()
    }

    #[test]
    fn a_file_rewritten_today_is_dated_today_whatever_its_creation_date() {
        // The owner's log (#48): created on 30/09, rewritten at 16:17 on 07/10.
        let times = [t(16, 17, 47), t(16, 18, 50)];
        let written = london_ms(2026, 10, 7, 16, 18, 57);
        assert_eq!(
            day_anchor_from_last_write(&times, written, chrono_tz::Europe::London),
            Some(day(2026, 10, 7))
        );
    }

    #[test]
    fn a_file_that_crossed_midnight_starts_the_day_before_its_last_write() {
        let times = [t(23, 50, 0), t(23, 59, 0), t(0, 10, 0)];
        let written = london_ms(2026, 10, 8, 0, 10, 30);
        assert_eq!(
            day_anchor_from_last_write(&times, written, chrono_tz::Europe::London),
            Some(day(2026, 10, 7))
        );
    }

    #[test]
    fn a_line_written_before_midnight_and_flushed_after_keeps_its_day() {
        let times = [t(23, 59, 59)];
        let written = london_ms(2026, 10, 8, 0, 0, 2);
        assert_eq!(
            day_anchor_from_last_write(&times, written, chrono_tz::Europe::London),
            Some(day(2026, 10, 7))
        );
    }

    #[test]
    fn a_file_with_no_events_has_no_anchor_from_its_last_write() {
        let written = london_ms(2026, 10, 7, 16, 0, 0);
        assert_eq!(
            day_anchor_from_last_write(&[], written, chrono_tz::Europe::London),
            None
        );
    }

    fn utc_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("utc")
            .timestamp_millis()
    }

    #[test]
    fn combines_anchor_date_with_time_in_utc() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        assert_eq!(
            clock.resolve(t(12, 23, 24)),
            utc_ms(2026, 8, 22, 12, 23, 24)
        );
    }

    #[test]
    fn rolls_over_at_midnight() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        clock.resolve(t(23, 59, 50));
        assert_eq!(clock.resolve(t(0, 0, 5)), utc_ms(2026, 8, 23, 0, 0, 5));
    }

    #[test]
    fn small_backwards_steps_stay_on_the_same_day() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        clock.resolve(t(12, 0, 5));
        assert_eq!(clock.resolve(t(12, 0, 1)), utc_ms(2026, 8, 22, 12, 0, 1));
    }

    #[test]
    fn applies_the_timezone() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::Europe::London);
        assert_eq!(clock.resolve(t(12, 0, 0)), utc_ms(2026, 8, 22, 11, 0, 0));
    }

    #[test]
    fn dst_gap_shifts_forward_by_the_gap() {
        let mut clock = DayClock::new(day(2026, 3, 29), chrono_tz::Europe::London);
        assert_eq!(clock.resolve(t(1, 30, 0)), utc_ms(2026, 3, 29, 1, 30, 0));
        // Lord Howe's gap is half an hour: 02:15 (+10:30) reads as 02:45 (+11:00).
        let mut clock = DayClock::new(day(2026, 10, 4), chrono_tz::Australia::Lord_Howe);
        assert_eq!(clock.resolve(t(2, 15, 0)), utc_ms(2026, 10, 3, 15, 45, 0));
    }

    fn wall_clock(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        day(y, mo, d).and_hms_opt(h, mi, 0).expect("time")
    }

    #[test]
    fn local_to_utc_ms_reads_a_wall_clock_as_php_does() {
        let london = chrono_tz::Europe::London;
        let lord_howe = chrono_tz::Australia::Lord_Howe;
        // An ordinary time.
        assert_eq!(
            local_to_utc_ms(wall_clock(2026, 7, 1, 12, 0), london),
            Some(utc_ms(2026, 7, 1, 11, 0, 0))
        );
        // London's one hour gap: 01:30 is 02:30 BST.
        assert_eq!(
            local_to_utc_ms(wall_clock(2026, 3, 29, 1, 30), london),
            Some(utc_ms(2026, 3, 29, 1, 30, 0))
        );
        // Lord Howe's half hour gap: 02:15 is 02:45 (+11:00).
        assert_eq!(
            local_to_utc_ms(wall_clock(2026, 10, 4, 2, 15), lord_howe),
            Some(utc_ms(2026, 10, 3, 15, 45, 0))
        );
        // A repeated time takes the earlier instant: 01:30 BST, and 01:45
        // Lord Howe daylight time (+11:00).
        assert_eq!(
            local_to_utc_ms(wall_clock(2026, 10, 25, 1, 30), london),
            Some(utc_ms(2026, 10, 25, 0, 30, 0))
        );
        assert_eq!(
            local_to_utc_ms(wall_clock(2026, 4, 5, 1, 45), lord_howe),
            Some(utc_ms(2026, 4, 4, 14, 45, 0))
        );
    }

    #[test]
    fn dst_overlap_takes_the_earlier_instant() {
        let mut clock = DayClock::new(day(2026, 10, 25), chrono_tz::Europe::London);
        assert_eq!(clock.resolve(t(1, 30, 0)), utc_ms(2026, 10, 25, 0, 30, 0));
    }

    #[test]
    fn a_live_event_after_a_silent_overnight_gap_takes_the_read_date() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        clock.resolve(t(8, 0, 0));
        let read = utc_ms(2026, 8, 23, 7, 0, 1);
        assert_eq!(
            clock.resolve_live(t(7, 0, 0), read),
            utc_ms(2026, 8, 23, 7, 0, 0)
        );
    }

    #[test]
    fn a_live_event_later_in_the_day_after_a_silent_day_takes_the_read_date() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        clock.resolve(t(20, 0, 0));
        let read = utc_ms(2026, 8, 23, 21, 0, 1);
        assert_eq!(
            clock.resolve_live(t(21, 0, 0), read),
            utc_ms(2026, 8, 23, 21, 0, 0)
        );
    }

    #[test]
    fn a_live_event_read_just_after_midnight_keeps_its_own_day() {
        let mut clock = DayClock::new(day(2026, 8, 20), chrono_tz::UTC);
        clock.resolve(t(23, 0, 0));
        let read = utc_ms(2026, 8, 23, 0, 0, 2);
        assert_eq!(
            clock.resolve_live(t(23, 59, 58), read),
            utc_ms(2026, 8, 22, 23, 59, 58)
        );
        // The rollover rule carries on from the re-anchored date.
        assert_eq!(clock.resolve(t(0, 0, 5)), utc_ms(2026, 8, 23, 0, 0, 5));
    }

    #[test]
    fn a_live_event_close_to_its_read_time_is_left_alone() {
        let mut clock = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        clock.resolve(t(12, 0, 0));
        let read = utc_ms(2026, 8, 22, 12, 0, 2);
        assert_eq!(
            clock.resolve_live(t(12, 0, 1), read),
            utc_ms(2026, 8, 22, 12, 0, 1)
        );
    }

    #[test]
    fn live_dating_applies_the_timezone() {
        let mut clock = DayClock::new(day(2026, 8, 20), chrono_tz::Europe::London);
        clock.resolve(t(9, 0, 0));
        // 06:00:01 UTC is 07:00:01 in London (BST).
        let read = utc_ms(2026, 8, 23, 6, 0, 1);
        assert_eq!(
            clock.resolve_live(t(7, 0, 0), read),
            utc_ms(2026, 8, 23, 6, 0, 0)
        );
    }

    #[test]
    fn a_resumed_clock_continues_where_it_stopped() {
        let mut first = DayClock::new(day(2026, 8, 22), chrono_tz::UTC);
        first.resolve(t(23, 0, 0));
        let mut resumed = DayClock::resume(first.state(), chrono_tz::UTC);
        assert_eq!(resumed.state(), first.state());
        assert_eq!(resumed.resolve(t(0, 0, 1)), first.resolve(t(0, 0, 1)));
        assert_eq!(resumed.resolve(t(0, 0, 1)), utc_ms(2026, 8, 23, 0, 0, 1));
    }

    #[test]
    fn live_re_anchoring_into_a_dst_gap_uses_the_shifted_instant() {
        let mut clock = DayClock::new(day(2026, 3, 20), chrono_tz::Europe::London);
        // 01:30 local does not exist on 2026-03-29; the real instant is 01:30 UTC.
        let read = utc_ms(2026, 3, 29, 1, 30, 1);
        assert_eq!(
            clock.resolve_live(t(1, 30, 0), read),
            utc_ms(2026, 3, 29, 1, 30, 0)
        );
        assert_eq!(clock.state().date, day(2026, 3, 29));
    }

    #[test]
    fn live_re_anchoring_into_a_dst_overlap_takes_the_earlier_instant() {
        let mut clock = DayClock::new(day(2026, 10, 20), chrono_tz::Europe::London);
        // 01:30 local happens twice on 2026-10-25; the earlier one is 00:30 UTC.
        let read = utc_ms(2026, 10, 25, 0, 30, 1);
        assert_eq!(
            clock.resolve_live(t(1, 30, 0), read),
            utc_ms(2026, 10, 25, 0, 30, 0)
        );
        assert_eq!(clock.state().date, day(2026, 10, 25));
    }
}
