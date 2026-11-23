//! Rate limits (#499): a 429 pauses every request to that path until its
//! `Retry-After` has passed, so the app backs off as the API (or
//! Cloudflare's rate-limit rules) asks instead of retrying on its own
//! timers. Like Laravel's `RateLimiter::tooManyAttempts` on the client
//! side: the app keeps one `RateLimits` and asks it before every send.

use chrono::DateTime;

/// The pause when a 429 carries no `Retry-After` that can be read.
pub const DEFAULT_PAUSE_MS: i64 = 60_000;

/// The longest pause one 429 sets, however long its `Retry-After`.
pub const MAX_PAUSE_MS: i64 = 3_600_000;

/// The shortest pause one 429 sets.
pub const MIN_PAUSE_MS: i64 = 1_000;

/// How long a 429 pauses its path, in milliseconds, from its `Retry-After`
/// header: whole seconds (`120`) or an HTTP date (`Wed, 21 Oct 2026
/// 07:28:00 GMT`), held between `MIN_PAUSE_MS` and `MAX_PAUSE_MS`.
/// `DEFAULT_PAUSE_MS` when there is none or it cannot be read.
pub fn pause_ms(retry_after: Option<&str>, now_ms: i64) -> i64 {
    let Some(value) = retry_after.map(str::trim).filter(|value| !value.is_empty()) else {
        return DEFAULT_PAUSE_MS;
    };
    let pause = if let Ok(seconds) = value.parse::<i64>() {
        seconds.saturating_mul(1_000)
    } else if let Ok(date) = DateTime::parse_from_rfc2822(value) {
        date.timestamp_millis() - now_ms
    } else {
        return DEFAULT_PAUSE_MS;
    };
    pause.clamp(MIN_PAUSE_MS, MAX_PAUSE_MS)
}

/// The paths paused by a 429, each until when.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimits {
    paused: Vec<(String, i64)>,
}

impl RateLimits {
    /// A 429 for `path` at `now_ms`: pause it for `pause_ms(retry_after)`.
    /// A later end replaces an earlier one, never the other way round.
    pub fn record(&mut self, path: &str, retry_after: Option<&str>, now_ms: i64) {
        let until = now_ms + pause_ms(retry_after, now_ms);
        for (paused, paused_until) in &mut self.paused {
            if paused == path {
                *paused_until = (*paused_until).max(until);
                return;
            }
        }
        self.paused.push((path.to_string(), until));
    }

    /// When `path` may be sent again, if it is paused at `now_ms`.
    pub fn paused_until(&self, path: &str, now_ms: i64) -> Option<i64> {
        self.paused
            .iter()
            .find(|(paused, until)| paused == path && *until > now_ms)
            .map(|(_, until)| *until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-21T07:28:00Z.
    const NOW: i64 = 1_792_567_680_000;

    #[test]
    fn retry_after_in_seconds_pauses_that_long() {
        assert_eq!(pause_ms(Some("120"), NOW), 120_000);
        assert_eq!(pause_ms(Some(" 5 "), NOW), 5_000);
    }

    #[test]
    fn retry_after_as_an_http_date_pauses_until_then() {
        assert_eq!(
            pause_ms(Some("Wed, 21 Oct 2026 07:30:00 GMT"), NOW),
            120_000
        );
    }

    #[test]
    fn a_missing_or_unreadable_retry_after_pauses_a_minute() {
        assert_eq!(pause_ms(None, NOW), DEFAULT_PAUSE_MS);
        assert_eq!(pause_ms(Some(""), NOW), DEFAULT_PAUSE_MS);
        assert_eq!(pause_ms(Some("soon"), NOW), DEFAULT_PAUSE_MS);
    }

    #[test]
    fn the_pause_is_held_between_a_second_and_an_hour() {
        assert_eq!(pause_ms(Some("0"), NOW), MIN_PAUSE_MS);
        assert_eq!(pause_ms(Some("-5"), NOW), MIN_PAUSE_MS);
        assert_eq!(pause_ms(Some("999999"), NOW), MAX_PAUSE_MS);
        assert_eq!(
            pause_ms(Some("Wed, 21 Oct 2026 07:00:00 GMT"), NOW),
            MIN_PAUSE_MS
        );
    }

    #[test]
    fn a_429_pauses_only_its_own_path_until_the_pause_ends() {
        let mut limits = RateLimits::default();
        limits.record("/api/cards", Some("30"), NOW);
        assert_eq!(limits.paused_until("/api/cards", NOW), Some(NOW + 30_000));
        assert_eq!(
            limits.paused_until("/api/cards", NOW + 29_999),
            Some(NOW + 30_000)
        );
        assert_eq!(limits.paused_until("/api/cards", NOW + 30_000), None);
        assert_eq!(limits.paused_until("/api/matches/report", NOW), None);
    }

    #[test]
    fn a_shorter_later_pause_never_cuts_a_longer_one() {
        let mut limits = RateLimits::default();
        limits.record("/api/cards", Some("600"), NOW);
        limits.record("/api/cards", Some("5"), NOW + 1_000);
        assert_eq!(
            limits.paused_until("/api/cards", NOW + 1_000),
            Some(NOW + 600_000)
        );
        limits.record("/api/cards", Some("3600"), NOW + 1_000);
        assert_eq!(
            limits.paused_until("/api/cards", NOW + 1_000),
            Some(NOW + 1_000 + 3_600_000)
        );
    }
}
