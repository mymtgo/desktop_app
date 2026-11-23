//! Small lookups over `serde_json::Value`. Endstep adds fields freely and
//! sends some numbers as strings (`"0"`, `"7"`), so every read is lenient.

use serde_json::Value;

/// The string at `key`, when it is a JSON string.
pub(crate) fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// True when `key` holds JSON `true`, the number 1 or the text `"true"` or
/// `"1"` (any case, spaces around ignored), as Endstep sends some values as
/// strings. Anything else is false.
pub(crate) fn flag(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(on)) => *on,
        Some(Value::Number(number)) => number.as_f64() == Some(1.0),
        Some(Value::String(text)) => {
            let text = text.trim();
            text.eq_ignore_ascii_case("true") || text == "1"
        }
        _ => false,
    }
}

/// A whole number from a JSON number (a float is rounded) or a string
/// holding one.
pub(crate) fn i64_of(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().or_else(|| rounded(number.as_f64()?)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn u64_of(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| u64::try_from(rounded(number.as_f64()?)?).ok()),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn u32_of(value: Option<&Value>) -> Option<u32> {
    let number = u64_of(value)?;
    u32::try_from(number).ok()
}

/// The strings of a JSON array; anything else in it is skipped.
pub(crate) fn strings(value: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if let Some(text) = item.as_str() {
                out.push(text.to_string());
            }
        }
    }
    out
}

/// The numbers of a JSON array; anything else in it is skipped.
pub(crate) fn u32s(value: Option<&Value>) -> Vec<u32> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if let Some(number) = u32_of(Some(item)) {
                out.push(number);
            }
        }
    }
    out
}

/// A float rounded to the nearest whole number, when it fits an `i64`.
fn rounded(number: f64) -> Option<i64> {
    let whole = number.round();
    if whole.is_finite() && whole >= i64::MIN as f64 && whole <= i64::MAX as f64 {
        Some(whole as i64)
    } else {
        None
    }
}

/// The numbers of a JSON array, at most `limit` of them; anything else in it
/// is skipped.
pub(crate) fn u64s_limited(value: Option<&Value>, limit: usize) -> Vec<u64> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if out.len() >= limit {
                break;
            }
            if let Some(number) = u64_of(Some(item)) {
                out.push(number);
            }
        }
    }
    out
}

/// A whole number that fits an `i32`.
pub(crate) fn i32_of(value: Option<&Value>) -> Option<i32> {
    i32::try_from(i64_of(value)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_flag_is_a_boolean_or_its_text_or_number() {
        let value = json!({
            "bool_true": true,
            "bool_false": false,
            "text_true": "true",
            "text_upper": " TRUE ",
            "text_false": "false",
            "text_one": "1",
            "text_zero": "0",
            "number_one": 1,
            "number_zero": 0,
            "float_one": 1.0,
            "number_two": 2,
            "other_text": "yes",
            "null": null,
            "list": [true],
        });
        let on = [
            "bool_true",
            "text_true",
            "text_upper",
            "text_one",
            "number_one",
            "float_one",
        ];
        for key in on {
            assert!(flag(&value, key), "{key}");
        }
        let off = [
            "bool_false",
            "text_false",
            "text_zero",
            "number_zero",
            "number_two",
            "other_text",
            "null",
            "list",
            "missing",
        ];
        for key in off {
            assert!(!flag(&value, key), "{key}");
        }
    }
}
