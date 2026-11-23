//! Canonical JSON (0.x `App\Support\CanonicalJson`, survey section 12):
//! `json_encode(sortRecursively($v), JSON_UNESCAPED_SLASHES |
//! JSON_UNESCAPED_UNICODE)` byte for byte, so two devices hash the same row
//! the same way.
//!
//! The PHP rules reproduced here:
//! - maps are sorted with `ksort` (PHP 8 comparison: a key that is a
//!   canonical decimal integer is an integer key; integers compare
//!   numerically, two numeric strings numerically, anything else as bytes);
//! - a map whose sorted keys are exactly `0..n-1` is a list, and an empty map
//!   is `[]` (PHP has one array type);
//! - lists keep their order;
//! - no whitespace; `/` and non-ASCII stay as they are, except U+2028 and
//!   U+2029 (` `, ` `); `"`, `\` and control characters escape as
//!   PHP does (`\b`, `\f`, `\n`, `\r`, `\t`, else `\u00xx`);
//! - floats as `serialize_precision = -1` (shortest round trip, `1.0e+25`
//!   past 17 digits, `1.0e-5` below 0.0001, `1` for 1.0).
//!
//! Known gap: PHP sorts keys that compare equal (`"1"` and `"1.0"`) in their
//! insertion order, which a `serde_json` map does not keep; bundle keys are
//! field names and never tie.

use std::cmp::Ordering;
use std::fmt::Write;

use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

/// 0.x `sync_client.canonical_version`: the bundle shapes v1 builds.
pub const CANONICAL_VERSION: u32 = 4;

/// The canonical JSON of a value.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    encode(value, &mut out);
    out
}

/// The bundle hash: lowercase sha256 hex of the canonical bytes.
pub fn bundle_hash(canonical: &str) -> String {
    let digest = Sha256::digest(canonical.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn encode(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => match number.as_i64() {
            Some(int) => {
                let _ = write!(out, "{int}");
            }
            None => out.push_str(&php_number(number)),
        },
        Value::String(text) => escape(text, out),
        Value::Array(items) => encode_list(items.iter(), out),
        Value::Object(fields) => encode_map(fields, out),
    }
}

/// One map entry: its PHP key, what PHP makes of the key as a number (read
/// once, not on every comparison) and its value.
struct Entry<'a> {
    key: PhpKey<'a>,
    numeric: Option<Numeric>,
    value: &'a Value,
}

/// Whether a key is neither an integer key nor a numeric string: such keys
/// compare as bytes.
fn is_plain_key(key: &str) -> bool {
    matches!(PhpKey::from(key), PhpKey::Text(_)) && numeric(key).is_none()
}

fn encode_map(fields: &serde_json::Map<String, Value>, out: &mut String) {
    // Plain keys compare as bytes, a total order, so plain keys already in
    // byte order (a `serde_json` map without `preserve_order` iterates so)
    // are already sorted and go out as they are.
    let mut previous: Option<&str> = None;
    let mut sorted = !fields.is_empty();
    for key in fields.keys() {
        if !is_plain_key(key) || previous.is_some_and(|before| before.as_bytes() >= key.as_bytes())
        {
            sorted = false;
            break;
        }
        previous = Some(key);
    }
    if sorted {
        out.push('{');
        for (index, (key, value)) in fields.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            escape(key, out);
            out.push(':');
            encode(value, out);
        }
        out.push('}');
        return;
    }
    let mut entries: Vec<Entry> = fields
        .iter()
        .map(|(key, value)| {
            let key = PhpKey::from(key);
            let numeric = match key {
                PhpKey::Int(_) => None,
                PhpKey::Text(text) => numeric(text),
            };
            Entry {
                key,
                numeric,
                value,
            }
        })
        .collect();
    entries.sort_by(compare_entries);
    let is_list = entries
        .iter()
        .enumerate()
        .all(|(index, entry)| entry.key == PhpKey::Int(index as i64));
    if is_list {
        encode_list(entries.iter().map(|entry| entry.value), out);
        return;
    }
    out.push('{');
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        match entry.key {
            PhpKey::Int(int) => {
                let _ = write!(out, "\"{int}\"");
            }
            PhpKey::Text(text) => escape(text, out),
        }
        out.push(':');
        encode(entry.value, out);
    }
    out.push('}');
}

fn encode_list<'a>(items: impl Iterator<Item = &'a Value>, out: &mut String) {
    out.push('[');
    for (index, item) in items.enumerate() {
        if index > 0 {
            out.push(',');
        }
        encode(item, out);
    }
    out.push(']');
}

/// PHP's `json_encode` string escaping with `JSON_UNESCAPED_SLASHES |
/// JSON_UNESCAPED_UNICODE`.
fn escape(text: &str, out: &mut String) {
    out.push('"');
    // Most text needs no escape: `"`, `\\`, a control character, or the
    // lead byte of U+2028 and U+2029 (0xE2) sends it the long way.
    let plain = text
        .bytes()
        .all(|byte| byte >= 0x20 && byte != b'"' && byte != b'\\' && byte != 0xE2);
    if plain {
        out.push_str(text);
        out.push('"');
        return;
    }
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A JSON number as PHP prints it: integers as they are, floats as
/// `serialize_precision = -1`. An integer beyond PHP's range was a float
/// in PHP.
fn php_number(number: &Number) -> String {
    if let Some(int) = number.as_i64() {
        return int.to_string();
    }
    php_float(number.as_f64().unwrap_or(0.0))
}

/// PHP's `php_gcvt(value, 17, '.', 'e')` with mode 0 (shortest digits).
fn php_float(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    // Rust's `{:e}` is the shortest round-trip form: `d.ddde±x`.
    let shortest = format!("{:e}", value.abs());
    let (mantissa, exponent) = shortest.split_once('e').unwrap_or((&shortest, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let decpt = exponent + 1;
    let mut out = String::new();
    if value.is_sign_negative() {
        out.push('-');
    }
    let precision = 17;
    // PHP: `decpt < 0 ? decpt < -3 : decpt > precision`.
    if decpt < -3 || decpt > precision {
        // Exponential: `1.0e+25`, `1.5e-7`.
        let mut chars = digits.chars();
        out.push(chars.next().unwrap_or('0'));
        out.push('.');
        let rest: String = chars.collect();
        out.push_str(if rest.is_empty() { "0" } else { &rest });
        let shown = decpt - 1;
        out.push('e');
        out.push(if shown < 0 { '-' } else { '+' });
        out.push_str(&shown.abs().to_string());
    } else if decpt < 0 {
        // `0.000ddd`.
        out.push_str("0.");
        for _ in decpt..0 {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let decpt = decpt as usize;
        if decpt == 0 {
            out.push('0');
        }
        for index in 0..decpt {
            out.push(digits.chars().nth(index).unwrap_or('0'));
        }
        if digits.len() > decpt {
            out.push('.');
            out.push_str(&digits[decpt..]);
        }
    }
    out
}

/// A PHP array key: a canonical decimal integer string is an integer key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhpKey<'a> {
    Int(i64),
    Text(&'a str),
}

impl<'a> PhpKey<'a> {
    fn from(key: &'a str) -> PhpKey<'a> {
        let digits = key.strip_prefix('-').unwrap_or(key);
        let canonical = !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'))
            && key != "-0";
        if canonical && let Ok(int) = key.parse::<i64>() {
            return PhpKey::Int(int);
        }
        PhpKey::Text(key)
    }
}

/// What PHP's `is_numeric_string` makes of a string.
#[derive(Debug, Clone, Copy)]
enum Numeric {
    Long(i64),
    Double(f64),
}

/// PHP 8's numeric strings: optional leading and trailing whitespace, an
/// optional sign, digits with an optional fraction (or a fraction alone)
/// and an optional exponent. Integers that do not fit are doubles.
fn numeric(text: &str) -> Option<Numeric> {
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}');
    let body = text.trim_matches(is_space);
    let bytes = body.as_bytes();
    let mut index = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        index += 1;
    }
    let int_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let int_digits = index - int_start;
    let mut fraction_digits = 0;
    let mut is_double = false;
    if bytes.get(index) == Some(&b'.') {
        is_double = true;
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
            fraction_digits += 1;
        }
    }
    if int_digits == 0 && fraction_digits == 0 {
        return None;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        let mut exponent = index + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            is_double = true;
            index = exponent;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
        }
    }
    if index != bytes.len() {
        return None;
    }
    if !is_double && let Ok(long) = body.parse::<i64>() {
        return Some(Numeric::Long(long));
    }
    body.parse::<f64>().ok().map(Numeric::Double)
}

fn compare_numbers(a: Numeric, b: Numeric) -> Option<Ordering> {
    match (a, b) {
        (Numeric::Long(a), Numeric::Long(b)) => Some(a.cmp(&b)),
        (a, b) => {
            let (a, b) = (as_f64(a), as_f64(b));
            if a == b && !a.is_finite() {
                return None;
            }
            a.partial_cmp(&b)
        }
    }
}

fn as_f64(number: Numeric) -> f64 {
    match number {
        Numeric::Long(long) => long as f64,
        Numeric::Double(double) => double,
    }
}

/// PHP 8 `ksort` (SORT_REGULAR) key comparison, with each text key's
/// numeric reading taken from its entry.
fn compare_entries(a: &Entry, b: &Entry) -> Ordering {
    match (a.key, b.key) {
        (PhpKey::Int(x), PhpKey::Int(y)) => x.cmp(&y),
        (PhpKey::Text(x), PhpKey::Text(y)) => match (a.numeric, b.numeric) {
            (Some(m), Some(n)) => {
                compare_numbers(m, n).unwrap_or_else(|| x.as_bytes().cmp(y.as_bytes()))
            }
            _ => x.as_bytes().cmp(y.as_bytes()),
        },
        (PhpKey::Int(int), PhpKey::Text(text)) => compare_int_to_text(int, text, b.numeric),
        (PhpKey::Text(text), PhpKey::Int(int)) => {
            compare_int_to_text(int, text, a.numeric).reverse()
        }
    }
}

/// PHP 8 `compare_longs_to_string`: numerically when the string is numeric
/// (`numeric`, its reading), else the integer's digits against the string
/// as bytes.
fn compare_int_to_text(int: i64, text: &str, numeric: Option<Numeric>) -> Ordering {
    match numeric {
        Some(number) => compare_numbers(Numeric::Long(int), number).unwrap_or(Ordering::Equal),
        None => int.to_string().as_bytes().cmp(text.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn golden() -> Value {
        serde_json::from_str(include_str!(
            "../../../fixtures/parity/sync_canonical.golden.json"
        ))
        .expect("the canonical golden is JSON")
    }

    // 0.x CanonicalJsonTest: "is byte-stable regardless of key order and
    // nesting".
    #[test]
    fn is_byte_stable_regardless_of_key_order_and_nesting() {
        let a = json!({"b": 2, "a": {"y": [3, 1], "x": "ø/slash"}, "c": null});
        let b = json!({"c": null, "a": {"x": "ø/slash", "y": [3, 1]}, "b": 2});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(
            bundle_hash(&canonical_json(&a)),
            bundle_hash(&canonical_json(&b))
        );
    }

    // 0.x "does not reorder lists, only maps".
    #[test]
    fn does_not_reorder_lists_only_maps() {
        assert_eq!(canonical_json(&json!({"k": [2, 1]})), r#"{"k":[2,1]}"#);
    }

    // 0.x "emits no whitespace and leaves slashes and unicode unescaped".
    #[test]
    fn emits_no_whitespace_and_leaves_slashes_and_unicode_unescaped() {
        assert_eq!(
            canonical_json(&json!({"url": "a/b", "name": "ø"})),
            r#"{"name":"ø","url":"a/b"}"#
        );
    }

    // 0.x "hashes to lowercase sha256 hex of the encoded bytes".
    #[test]
    fn hashes_to_lowercase_sha256_hex_of_the_encoded_bytes() {
        let encoded = canonical_json(&json!({"token": "abc"}));
        assert_eq!(encoded, r#"{"token":"abc"}"#);
        // printf '{"token":"abc"}' | shasum -a 256
        assert_eq!(
            bundle_hash(&encoded),
            "9c5955952c12f09397bd7fcf56624bea656322913fd00d884661c3b126d17a28"
        );
    }

    // Every case 0.x's CanonicalJson encoded, byte for byte, with its hash.
    #[test]
    fn every_golden_case_matches_0x_byte_for_byte() {
        let golden = golden();
        let cases = golden["cases"].as_array().expect("cases");
        assert!(cases.len() >= 13);
        for case in cases {
            let name = case["name"].as_str().expect("name");
            let encoded = canonical_json(&case["input"]);
            assert_eq!(
                encoded,
                case["canonical"].as_str().expect("canonical"),
                "{name}"
            );
            assert_eq!(
                bundle_hash(&encoded),
                case["hash"].as_str().expect("hash"),
                "{name}"
            );
        }
        assert_eq!(golden["canonical_version"], json!(CANONICAL_VERSION));
    }

    #[test]
    fn php_ksort_orders_integer_and_string_keys() {
        assert_eq!(
            canonical_json(&json!({"b": 1, "10": 2, "9": 3, "a": 4, "1e1": 5, "01": 6})),
            r#"{"01":6,"9":3,"10":2,"1e1":5,"a":4,"b":1}"#
        );
        assert_eq!(canonical_json(&json!({"1": "a", "0": "b"})), r#"["b","a"]"#);
        assert_eq!(
            canonical_json(&json!({"1": "a", "2": "b"})),
            r#"{"1":"a","2":"b"}"#
        );
        assert_eq!(canonical_json(&json!({})), "[]");
        assert_eq!(canonical_json(&json!([])), "[]");
    }

    #[test]
    fn floats_follow_serialize_precision_minus_one() {
        for (value, expected) in [
            (1.0, "1"),
            (0.1, "0.1"),
            (1e25, "1.0e+25"),
            (0.00001, "1.0e-5"),
            (0.0001, "0.0001"),
            (-0.0, "-0"),
            (1e16, "10000000000000000"),
            (1e17, "1.0e+17"),
            (2.5, "2.5"),
            (-2.5e-300, "-2.5e-300"),
            (123.456e10, "1234560000000"),
        ] {
            assert_eq!(canonical_json(&json!(value)), expected, "{value}");
        }
    }

    #[test]
    fn strings_escape_as_php() {
        assert_eq!(
            canonical_json(&json!(
                "a\u{2028}b\u{2029}\u{1}\u{7f}\"\\\n\t\r\u{8}\u{c}/é"
            )),
            "\"a\\u2028b\\u2029\\u0001\u{7f}\\\"\\\\\\n\\t\\r\\b\\f/é\""
        );
    }
}
