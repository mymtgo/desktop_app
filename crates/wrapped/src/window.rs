//! The counted year and when Wrapped is shown. The year is frozen at
//! 30 November: it counts from 1 January to 1 December (local midnights), and
//! Wrapped shows in December only.

use chrono::{Datelike, TimeZone};
use chrono_tz::Tz;

use crate::bundle::YearWindow;

/// The counted window of `year` in the player's timezone.
pub fn year_window(year: i32, tz: Tz) -> YearWindow {
    YearWindow {
        start_ms: local_midnight_ms(year, 1, 1, tz),
        end_ms: local_midnight_ms(year, 12, 1, tz),
    }
}

/// The Wrapped year shown at `now_ms`: December of a year shows that year
/// (owner, 2026-11-25: December only); any other month shows none.
pub fn wrapped_year(now_ms: i64, tz: Tz) -> Option<i32> {
    let now = tz.timestamp_millis_opt(now_ms).single()?;
    if now.month() == 12 {
        Some(now.year())
    } else {
        None
    }
}

/// The first instant of a local day. A day that starts inside a clock change
/// takes the earliest valid instant; one with no midnight at all (a skipped
/// hour) falls back to UTC midnight, which is never more than a day off.
fn local_midnight_ms(year: i32, month: u32, day: u32, tz: Tz) -> i64 {
    match tz.with_ymd_and_hms(year, month, day, 0, 0, 0).earliest() {
        Some(time) => time.timestamp_millis(),
        None => chrono::Utc
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .earliest()
            .map(|time| time.timestamp_millis())
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Los_Angeles;
    use chrono_tz::Europe::London;
    use chrono_tz::Pacific::Auckland;

    fn local_ms(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("an unambiguous local time")
            .timestamp_millis()
    }

    fn utc_ms(y: i32, mo: u32, d: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(y, mo, d, 0, 0, 0)
            .single()
            .expect("a utc time")
            .timestamp_millis()
    }

    #[test]
    fn the_window_runs_from_1_january_to_1_december_local() {
        let london = year_window(2026, London);
        assert_eq!(london.start_ms, utc_ms(2026, 1, 1));
        assert_eq!(london.end_ms, utc_ms(2026, 12, 1));

        // Auckland is UTC+13 in summer: its midnights come 13 hours earlier.
        let auckland = year_window(2026, Auckland);
        let hours_13 = 13 * 3_600_000;
        assert_eq!(auckland.start_ms, utc_ms(2026, 1, 1) - hours_13);
        assert_eq!(auckland.end_ms, utc_ms(2026, 12, 1) - hours_13);
    }

    #[test]
    fn wrapped_shows_from_1_december_to_31_december_local() {
        for tz in [Los_Angeles, Auckland] {
            assert_eq!(
                wrapped_year(local_ms(tz, 2026, 11, 30, 23, 59), tz),
                None,
                "{tz}"
            );
            assert_eq!(
                wrapped_year(local_ms(tz, 2026, 12, 1, 0, 0), tz),
                Some(2026),
                "{tz}"
            );
            assert_eq!(
                wrapped_year(local_ms(tz, 2026, 12, 31, 23, 59), tz),
                Some(2026),
                "{tz}"
            );
            assert_eq!(
                wrapped_year(local_ms(tz, 2027, 1, 1, 0, 0), tz),
                None,
                "{tz}"
            );
        }
    }

    #[test]
    fn mid_year_shows_nothing() {
        assert_eq!(
            wrapped_year(local_ms(London, 2026, 6, 15, 12, 0), London),
            None
        );
    }
}
