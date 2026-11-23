//! Reading JSON the way 0.x's PHP did: loosely. The API answers from SQL
//! columns without casts, so a number can arrive as `"3.0"` and an id as
//! `"126517"`. Each helper reads one field of an object and never fails:
//! a value of the wrong shape is `None`, as PHP's `?? null` would leave it.

use serde_json::Value;

/// A text field: a string as it is, a number as its text; anything else
/// (null, missing, a list) is `None`.
pub fn text(object: &Value, key: &str) -> Option<String> {
    match object.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A number field: a JSON number, or a string holding one (`"3.0"`).
pub fn number(object: &Value, key: &str) -> Option<f64> {
    match object.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// A whole number field, as PHP compares `"126517" == 126517`: a number or
/// a numeric string whose value is whole.
pub fn whole(object: &Value, key: &str) -> Option<i64> {
    whole_of(object.get(key)?)
}

/// A whole number from a value (see `whole`).
pub fn whole_of(value: &Value) -> Option<i64> {
    let number = match value {
        Value::Number(number) => {
            if let Some(whole) = number.as_i64() {
                return Some(whole);
            }
            number.as_f64()?
        }
        Value::String(text) => {
            let text = text.trim();
            if let Ok(whole) = text.parse::<i64>() {
                return Some(whole);
            }
            text.parse::<f64>().ok()?
        }
        _ => return None,
    };
    if number.fract() == 0.0 && number.abs() < 9.0e15 {
        Some(number as i64)
    } else {
        None
    }
}

/// PHP's `(bool)`: false for a missing value, null, `false`, `0`, `0.0`,
/// `""`, `"0"` and an empty list or object; true for anything else.
pub fn truthy(object: &Value, key: &str) -> bool {
    match object.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty() && text != "0",
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(fields)) => !fields.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fields_are_read_as_php_would() {
        let object = json!({
            "s": "q", "n": 3, "f": 2.5, "ns": " 4.0 ", "bad": "4x", "null": null,
            "list": [1], "zero": "0", "empty": "", "yes": "no"
        });
        assert_eq!(text(&object, "s").as_deref(), Some("q"));
        assert_eq!(text(&object, "n").as_deref(), Some("3"));
        assert_eq!(text(&object, "null"), None);
        assert_eq!(text(&object, "list"), None);
        assert_eq!(text(&object, "missing"), None);
        assert_eq!(number(&object, "ns"), Some(4.0));
        assert_eq!(number(&object, "f"), Some(2.5));
        assert_eq!(number(&object, "bad"), None);
        assert_eq!(whole(&object, "ns"), Some(4));
        assert_eq!(whole(&object, "f"), None);
        assert_eq!(whole(&object, "n"), Some(3));
        assert_eq!(whole(&object, "bad"), None);
        assert!(!truthy(&object, "zero"));
        assert!(!truthy(&object, "empty"));
        assert!(!truthy(&object, "null"));
        assert!(!truthy(&object, "missing"));
        assert!(truthy(&object, "yes"));
        assert!(truthy(&object, "n"));
        assert!(truthy(&object, "list"));
    }
}
