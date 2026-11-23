//! Reading draft payloads (0.x `RepairJson::firstObject`).
//!
//! MTGO writes some draft payloads without their closing brackets (the
//! PendingPick message has no outer `}`). The repair rule lives in
//! `mtgo_protocol::json_repair` (tournament round info needs it too) and is
//! re-exported here, beside small typed readers for the fields the fold needs.

use chrono::{DateTime, NaiveDateTime};
use serde_json::Value;

pub use mtgo_protocol::json_repair::first_object;

/// A whole number field, from a JSON number or a numeric string.
pub fn as_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Like `as_u64`, for ids that fit in 32 bits (catalog ids, indexes). A
/// larger value is `None`; the fold reads that as "no card" (0), as it reads
/// every id the catalog could not hold (`cards::MAX_CATALOG_ID` is one
/// million, so no real card is lost).
pub fn as_u32(value: Option<&Value>) -> Option<u32> {
    as_u64(value).and_then(|number| u32::try_from(number).ok())
}

/// A string field.
pub fn as_string(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_string)
}

/// An MTGO payload time (`2026-08-22T12:13:09.1840000+01:00`) as UTC unix
/// milliseconds, truncated to the whole second as 0.x stored it. A time
/// without an offset is read as UTC (0.x's app timezone).
pub fn payload_time_ms(value: Option<&Value>) -> Option<i64> {
    let text = value?.as_str()?;
    let ms = match DateTime::parse_from_rfc3339(text) {
        Ok(at) => at.timestamp_millis(),
        Err(_) => NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
            .ok()?
            .and_utc()
            .timestamp_millis(),
    };
    Some(whole_second(ms))
}

/// Drop the milliseconds (0.x's `datetime` columns hold whole seconds).
pub fn whole_second(ms: i64) -> i64 {
    ms - ms.rem_euclid(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_above_32_bits_is_not_a_catalog_id() {
        assert_eq!(
            as_u32(Some(&serde_json::json!(4_294_967_295u64))),
            Some(u32::MAX)
        );
        assert_eq!(as_u32(Some(&serde_json::json!(4_294_967_296u64))), None);
        assert_eq!(as_u32(Some(&serde_json::json!("99999999999"))), None);
    }

    #[test]
    fn payload_times_are_utc_whole_seconds() {
        let value = serde_json::json!({
            "Offset": "2026-08-22T12:12:26.3150000+01:00",
            "Naive": "2026-08-22T11:12:26.9",
            "Bad": "soon",
        });
        let expected = DateTime::parse_from_rfc3339("2026-08-22T11:12:26Z")
            .expect("valid")
            .timestamp_millis();
        assert_eq!(payload_time_ms(value.get("Offset")), Some(expected));
        assert_eq!(payload_time_ms(value.get("Naive")), Some(expected));
        assert_eq!(payload_time_ms(value.get("Bad")), None);
        assert_eq!(payload_time_ms(value.get("Missing")), None);
    }

    #[test]
    fn numbers_read_from_numbers_or_numeric_strings() {
        let value = serde_json::json!({"A": 7, "B": "8", "C": -1, "D": null});
        assert_eq!(as_u64(value.get("A")), Some(7));
        assert_eq!(as_u64(value.get("B")), Some(8));
        assert_eq!(as_u64(value.get("C")), None);
        assert_eq!(as_u64(value.get("D")), None);
        assert_eq!(as_u32(value.get("A")), Some(7));
    }
}
