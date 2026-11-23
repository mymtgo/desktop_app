//! Offline mode as rules (07 spec section 5, survey section 5): whether the
//! app is offline, the cooldown after coming back online, its message, and
//! the seed from 0.x's kept settings.
//!
//! Like 0.x's `AppSettings::isOffline`, `isOfflineModeLocked` and
//! `UpdateOfflineModeController`, without the settings file: the rows are
//! passed in, and so are the clock and the timezone.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use chrono_tz::Tz;
use serde_json::Value;

/// Settings row: offline mode is on (JSON bool).
pub const OFFLINE_MODE: &str = "offline_mode";
/// Settings row: offline mode cannot be switched back on before this time
/// (JSON ms, or null).
pub const OFFLINE_MODE_LOCKED_UNTIL: &str = "offline_mode_locked_until";
/// The settings row the 0.x import keeps 0.x's preferences in (the app's
/// `legacy::LEGACY_SETTINGS`).
pub const KEPT_0X_SETTINGS: &str = "legacy_0x_settings";

/// Whether the app is offline. Fails closed: a read that failed, a v1 row
/// that does not parse, or kept 0.x settings that do not parse are offline.
/// v1's own row always wins; without one, 0.x's kept `offline_mode` decides
/// (cast loosely, as 0.x's `(bool)` did); with neither, the app is online.
pub fn read_offline(rows: Result<&[(String, String)], ()>) -> bool {
    let Ok(rows) = rows else {
        return true;
    };
    if let Some(value) = find(rows, OFFLINE_MODE) {
        return serde_json::from_str::<bool>(value).unwrap_or(true);
    }
    match find(rows, KEPT_0X_SETTINGS) {
        Some(kept) => kept_offline(kept).unwrap_or(true),
        None => false,
    }
}

/// 0.x's kept `offline_mode`: `None` when the kept settings are not a JSON
/// object, false when the key is missing.
fn kept_offline(kept: &str) -> Option<bool> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(kept) else {
        return None;
    };
    Some(map.get(OFFLINE_MODE).is_some_and(php_truthy))
}

/// PHP's `(bool)` cast of a decoded JSON value.
fn php_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !(text.is_empty() || text == "0"),
        Value::Array(list) => !list.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn find<'a>(rows: &'a [(String, String)], key: &str) -> Option<&'a str> {
    rows.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// The rows a 0.x import stores for offline mode: `offline_mode = true` when
/// 0.x was offline and v1 has no row of its own yet. A v1 row is never
/// replaced, and an online 0.x user needs no row (missing is online).
pub fn seed_offline_rows(stored: &[(String, String)], kept: &str) -> Vec<(String, String)> {
    if find(stored, OFFLINE_MODE).is_some() {
        return Vec::new();
    }
    if kept_offline(kept) == Some(true) {
        vec![(OFFLINE_MODE.to_string(), "true".to_string())]
    } else {
        Vec::new()
    }
}

/// The stored cooldown end, if the row holds a time.
pub fn read_locked_until(rows: &[(String, String)]) -> Option<i64> {
    find(rows, OFFLINE_MODE_LOCKED_UNTIL)
        .and_then(|value| serde_json::from_str::<Option<i64>>(value).ok())
        .flatten()
}

/// When the cooldown after coming back online ends: the start of the next
/// local day (0.x `now()->toLocal()->addDay()->startOfDay()`). A midnight
/// that daylight saving skips moves on by the gap, as PHP does.
pub fn lock_until(now_ms: i64, tz: Tz) -> i64 {
    // Only a time outside chrono's range has no local time.
    let Some(now) = tz.timestamp_millis_opt(now_ms).single() else {
        return now_ms;
    };
    let tomorrow = now.date_naive() + Duration::days(1);
    decks::stats::local_midnight(tomorrow, tz)
}

/// Whether offline mode is on cooldown. Fails open: no stored time is not
/// locked (0.x `isOfflineModeLocked`).
pub fn is_locked(locked_until: Option<i64>, now_ms: i64) -> bool {
    locked_until.is_some_and(|until| until > now_ms)
}

/// The refusal for switching offline mode on during the cooldown (0.x
/// `UpdateOfflineModeController`).
pub fn cooldown_message(locked_until: i64, now_ms: i64) -> String {
    format!(
        "Offline mode is on cooldown after coming back online. You can turn it back on {}.",
        human_diff(locked_until, now_ms)
    )
}

/// Carbon 3's `diffForHumans()` with its defaults, in UTC: the largest unit
/// that is not zero (years, months, weeks, days, hours, minutes, seconds),
/// truncated, then "from now" or "ago". Under a second reads "0 seconds".
pub fn human_diff(at_ms: i64, now_ms: i64) -> String {
    let (earlier, later, suffix) = if at_ms > now_ms {
        (now_ms, at_ms, "from now")
    } else {
        (at_ms, now_ms, "ago")
    };
    let parts = calendar_diff(utc(earlier), utc(later));
    let units = [
        (parts.years, "year"),
        (parts.months, "month"),
        (parts.days / 7, "week"),
        (parts.days % 7, "day"),
        (parts.hours, "hour"),
        (parts.minutes, "minute"),
        (parts.seconds, "second"),
    ];
    let (count, unit) = units
        .into_iter()
        .find(|(count, _)| *count > 0)
        .unwrap_or((0, "second"));
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} {suffix}")
}

fn utc(ms: i64) -> NaiveDateTime {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .naive_utc()
}

/// PHP's `DateTime::diff` parts between two times, `earlier <= later`.
struct DiffParts {
    years: i64,
    months: i64,
    days: i64,
    hours: i64,
    minutes: i64,
    seconds: i64,
}

fn calendar_diff(earlier: NaiveDateTime, later: NaiveDateTime) -> DiffParts {
    let mut years = i64::from(later.year() - earlier.year());
    let mut months = i64::from(later.month()) - i64::from(earlier.month());
    let mut days = i64::from(later.day()) - i64::from(earlier.day());
    let mut seconds = i64::from(later.num_seconds_from_midnight())
        - i64::from(earlier.num_seconds_from_midnight());
    if later.nanosecond() < earlier.nanosecond() {
        seconds -= 1;
    }
    if seconds < 0 {
        seconds += 86_400;
        days -= 1;
    }
    if days < 0 {
        days += days_in_month_before(later.date());
        months -= 1;
    }
    if months < 0 {
        months += 12;
        years -= 1;
    }
    DiffParts {
        years,
        months,
        days,
        hours: seconds / 3_600,
        minutes: seconds % 3_600 / 60,
        seconds: seconds % 60,
    }
}

/// The number of days in the month before `date`'s month.
fn days_in_month_before(date: NaiveDate) -> i64 {
    let first = date.with_day(1).unwrap_or(date);
    let last_of_previous = first - Duration::days(1);
    i64::from(last_of_previous.day())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz;

    fn rows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn offline(pairs: &[(&str, &str)]) -> bool {
        read_offline(Ok(&rows(pairs)))
    }

    /// Unix ms of a UTC time written `2026-10-09T15:00:00`.
    fn utc(text: &str) -> i64 {
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
            .expect("a time")
            .and_utc()
            .timestamp_millis()
    }

    // 0.x AppSettings::isOffline fails closed.
    #[test]
    fn an_unreadable_read_is_offline() {
        assert!(read_offline(Err(())));
    }

    #[test]
    fn missing_everywhere_is_online() {
        assert!(!offline(&[]));
        assert!(!offline(&[("close_to_tray", "true")]));
    }

    #[test]
    fn the_v1_row_decides_and_an_unparsable_one_is_offline() {
        assert!(offline(&[(OFFLINE_MODE, "true")]));
        assert!(!offline(&[(OFFLINE_MODE, "false")]));
        assert!(offline(&[(OFFLINE_MODE, "null")]));
        assert!(offline(&[(OFFLINE_MODE, "nonsense")]));
    }

    // 0.x RenameShareStatsToOfflineModeTest, as seeding from the kept 0.x
    // settings.
    #[test]
    fn a_0x_offline_user_stays_offline() {
        assert!(offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":true}"#)]));
        // 0.x casts loosely: `(bool) $value`.
        assert!(offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":1}"#)]));
        assert!(offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":"yes"}"#)]));
        assert!(!offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":false}"#)]));
        assert!(!offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":0}"#)]));
        assert!(!offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":"0"}"#)]));
        assert!(!offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":""}"#)]));
        assert!(!offline(&[(KEPT_0X_SETTINGS, r#"{"offline_mode":null}"#)]));
    }

    #[test]
    fn a_missing_0x_value_is_online_and_unreadable_kept_settings_are_offline() {
        assert!(!offline(&[(
            KEPT_0X_SETTINGS,
            r#"{"deck_card_size":"large"}"#
        )]));
        assert!(offline(&[(KEPT_0X_SETTINGS, "not json")]));
        assert!(offline(&[(KEPT_0X_SETTINGS, "[]")]));
    }

    #[test]
    fn an_existing_v1_row_wins_over_the_0x_value() {
        assert!(!offline(&[
            (KEPT_0X_SETTINGS, r#"{"offline_mode":true}"#),
            (OFFLINE_MODE, "false"),
        ]));
        assert!(offline(&[
            (OFFLINE_MODE, "true"),
            (KEPT_0X_SETTINGS, r#"{"offline_mode":false}"#),
        ]));
    }

    #[test]
    fn the_import_seeds_a_0x_offline_user_and_never_replaces_a_v1_row() {
        let kept_offline = r#"{"offline_mode":true}"#;
        assert_eq!(
            seed_offline_rows(&[], kept_offline),
            rows(&[(OFFLINE_MODE, "true")])
        );
        assert_eq!(
            seed_offline_rows(&[], r#"{"offline_mode":false}"#),
            Vec::new()
        );
        assert_eq!(seed_offline_rows(&[], "{}"), Vec::new());
        assert_eq!(
            seed_offline_rows(&rows(&[(OFFLINE_MODE, "false")]), kept_offline),
            Vec::new()
        );
    }

    #[test]
    fn the_lock_row_reads_as_ms_and_anything_else_as_none() {
        assert_eq!(
            read_locked_until(&rows(&[(OFFLINE_MODE_LOCKED_UNTIL, "1800000000000")])),
            Some(1_800_000_000_000)
        );
        assert_eq!(
            read_locked_until(&rows(&[(OFFLINE_MODE_LOCKED_UNTIL, "null")])),
            None
        );
        assert_eq!(
            read_locked_until(&rows(&[(OFFLINE_MODE_LOCKED_UNTIL, "\"2026\"")])),
            None
        );
        assert_eq!(read_locked_until(&[]), None);
    }

    // 0.x OfflineModeRejoinLockTest: "locks offline mode until tomorrow".
    #[test]
    fn the_lock_ends_at_the_start_of_the_next_local_day() {
        // 16:00 in London (BST) on the 9th: midnight on the 10th, 23:00 UTC.
        assert_eq!(
            lock_until(utc("2026-10-09T15:00:00"), Tz::Europe__London),
            utc("2026-10-09T23:00:00")
        );
        // 23:30 UTC is already the 10th in Tokyo (+09:00).
        assert_eq!(
            lock_until(utc("2026-10-09T23:30:00"), Tz::Asia__Tokyo),
            utc("2026-10-10T15:00:00")
        );
        // Just after midnight still locks until the next midnight.
        assert_eq!(
            lock_until(utc("2026-10-09T00:00:01"), Tz::UTC),
            utc("2026-10-10T00:00:00")
        );
    }

    #[test]
    fn a_midnight_skipped_by_daylight_saving_locks_until_the_first_hour_that_exists() {
        // Sao Paulo moved its clocks from 00:00 to 01:00 on 2018-11-04 (-02:00).
        assert_eq!(
            lock_until(utc("2018-11-03T15:00:00"), Tz::America__Sao_Paulo),
            utc("2018-11-04T03:00:00")
        );
        // Lord Howe's half hour gap is at 02:00, so its midnight is ordinary.
        assert_eq!(
            lock_until(utc("2026-10-03T03:00:00"), Tz::Australia__Lord_Howe),
            utc("2026-10-03T13:30:00")
        );
    }

    // 0.x isOfflineModeLocked fails open; "reports not locked when no lock
    // has ever been written"; "allows offline mode again once the lock has
    // expired".
    #[test]
    fn the_lock_holds_only_while_it_is_in_the_future() {
        let now = 1_800_000_000_000;
        assert!(!is_locked(None, now));
        assert!(!is_locked(Some(now - 1), now));
        assert!(!is_locked(Some(now), now));
        assert!(is_locked(Some(now + 1), now));
    }

    fn diff(ahead_ms: i64) -> String {
        let now = utc("2026-10-09T15:00:00");
        human_diff(now + ahead_ms, now)
    }

    // Carbon 3.14 `diffForHumans()` with its defaults: one part, the largest
    // unit that is not zero, truncated, seconds at the smallest.
    #[test]
    fn the_wait_reads_like_carbon_diff_for_humans() {
        let second = 1_000;
        let minute = 60 * second;
        let hour = 60 * minute;
        let day = 24 * hour;
        assert_eq!(diff(5 * hour), "5 hours from now");
        assert_eq!(diff(5 * hour + 59 * minute), "5 hours from now");
        assert_eq!(diff(hour), "1 hour from now");
        assert_eq!(diff(hour - 1), "59 minutes from now");
        assert_eq!(diff(minute), "1 minute from now");
        assert_eq!(diff(30 * second), "30 seconds from now");
        assert_eq!(diff(second), "1 second from now");
        assert_eq!(diff(400), "0 seconds from now");
        assert_eq!(diff(day), "1 day from now");
        assert_eq!(diff(day + 23 * hour), "1 day from now");
        assert_eq!(diff(8 * day), "1 week from now");
        assert_eq!(diff(-2 * hour), "2 hours ago");
    }

    #[test]
    fn longer_waits_use_calendar_months_and_years() {
        let now = utc("2026-10-09T15:00:00");
        assert_eq!(
            human_diff(utc("2026-11-09T15:00:00"), now),
            "1 month from now"
        );
        assert_eq!(
            human_diff(utc("2028-01-01T00:00:00"), now),
            "1 year from now"
        );
        // 2026-11-08 is not a whole month away: 4 weeks and 2 days.
        assert_eq!(
            human_diff(utc("2026-11-08T15:00:00"), now),
            "4 weeks from now"
        );
    }

    // 0.x UpdateOfflineModeController's flash error.
    #[test]
    fn the_cooldown_message_says_when_offline_mode_can_come_back() {
        let now = utc("2026-10-09T19:00:00");
        assert_eq!(
            cooldown_message(utc("2026-10-10T00:00:00"), now),
            "Offline mode is on cooldown after coming back online. You can turn it back on 5 hours from now."
        );
    }
}
