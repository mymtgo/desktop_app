//! Loose readers for one value of a pulled bundle (or a 0.x bundle row).
//! The bundles were written by PHP, so a number can arrive as text and a
//! flag as a number: these read either, and anything else reads as none.

use serde_json::Value;

/// Text, or a number as its text.
pub fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// An integer, also from numeric text (ids and times can arrive as either).
pub fn int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A flag: `true`, or a non-zero integer. Anything else is `false`.
pub fn flag(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_i64().is_some_and(|n| n != 0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn text_reads_strings_and_numbers() {
        assert_eq!(text(&json!("abc")), Some("abc".to_string()));
        assert_eq!(text(&json!(42)), Some("42".to_string()));
        assert_eq!(text(&json!(1.5)), Some("1.5".to_string()));
        assert_eq!(text(&json!(null)), None);
        assert_eq!(text(&json!(true)), None);
    }

    #[test]
    fn int_reads_numbers_and_numeric_text() {
        assert_eq!(int(&json!(7)), Some(7));
        assert_eq!(int(&json!(" 12 ")), Some(12));
        assert_eq!(int(&json!("x")), None);
        assert_eq!(int(&json!(1.5)), None);
        assert_eq!(int(&json!(null)), None);
    }

    #[test]
    fn flag_reads_booleans_and_non_zero_integers() {
        assert!(flag(&json!(true)));
        assert!(flag(&json!(1)));
        assert!(!flag(&json!(0)));
        assert!(!flag(&json!(false)));
        assert!(!flag(&json!("1")));
        assert!(!flag(&json!(null)));
    }
}
