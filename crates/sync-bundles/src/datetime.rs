//! Bundle datetimes: UTC `Y-m-d\TH:i:s\Z`, no fraction (0.x
//! `DeckBundleBuilder::datetime`).

use chrono::{DateTime, NaiveDateTime};

const FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// A unix millisecond time as a bundle datetime (milliseconds dropped).
pub fn bundle_datetime(ms: i64) -> String {
    let time = DateTime::from_timestamp_millis(ms.div_euclid(1_000) * 1_000).unwrap_or_default();
    time.format(FORMAT).to_string()
}

/// A bundle datetime as unix milliseconds; `None` when it is not one.
pub fn parse_bundle_datetime(text: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(text, FORMAT)
        .ok()
        .map(|time| time.and_utc().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 0.x "emits every datetime as UTC ISO-8601 seconds".
    #[test]
    fn bundle_datetimes_are_utc_seconds_with_a_z() {
        assert_eq!(bundle_datetime(1_787_133_605_999), "2026-08-19T10:00:05Z");
        assert_eq!(bundle_datetime(0), "1970-01-01T00:00:00Z");
        assert_eq!(bundle_datetime(-1), "1969-12-31T23:59:59Z");
        assert_eq!(
            parse_bundle_datetime("2026-08-19T10:00:05Z"),
            Some(1_787_133_605_000)
        );
        assert_eq!(parse_bundle_datetime("2026-08-19 10:00:05"), None);
        assert_eq!(parse_bundle_datetime("nope"), None);
    }
}
