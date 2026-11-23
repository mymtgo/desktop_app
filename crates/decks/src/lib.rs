//! Deck rules ported from the 0.x app: MTGO deck file parsing, version
//! signatures, `deck_used` parsing, colour identity, format labels and `.dek`
//! export. Pure: no filesystem, clock, network or database. Callers pass in
//! the file text, the timezone and card lookups.

pub mod deck_used;
pub mod dek;
pub mod file;
pub mod format;
pub mod identity;
pub mod signature;
pub mod stats;

pub use deck_used::{first_json, parse_deck_used, parse_registered_deck, parse_tournament_deck};
pub use dek::{DekCard, INVALID_DEK, dek_file, parse_dek};
pub use file::{DeckFile, MAX_DEK_BYTES, is_deck_file_name, parse_deck_file};
pub use format::{
    FormatMatch, FormatShare, LIMITED_LABEL, UNKNOWN_FORMAT_LABEL, display_format,
    format_breakdown, is_limited, is_limited_match_format,
};
pub use identity::{CardInfo, color_identity, normalize_manual_identity};
pub use signature::{base64_decode, base64_encode, decode_signature, oracle_key, signature};

/// One row of a deck list: a printing (MTGO catalog id), how many, and
/// whether it sits in the sideboard. The same printing can appear twice
/// (main and side), and 0.x never merges duplicate rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckCard {
    pub catalog_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
}

/// The most copies of one card a deck row may hold. A real deck has at most a
/// few hundred basic lands; a crafted file's larger counts are clamped to
/// this so sums over a deck cannot overflow.
pub const MAX_CARD_QUANTITY: u32 = 1000;

/// A parsed card quantity, clamped to [`MAX_CARD_QUANTITY`].
pub(crate) fn clamp_quantity(quantity: u32) -> u32 {
    quantity.min(MAX_CARD_QUANTITY)
}

/// PHP's `filter_var($value, FILTER_VALIDATE_BOOL)`: `true`, `1`, `yes` and
/// `on` (any case, surrounding whitespace ignored) are true; anything else is
/// false. 0.x reads every sideboard flag this way.
pub(crate) fn php_bool(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower == "true" || lower == "1" || lower == "yes" || lower == "on"
}

/// PHP truthiness of a string: empty and `"0"` are false.
pub(crate) fn php_truthy(value: &str) -> bool {
    !value.is_empty() && value != "0"
}

/// PHP's `is_numeric` for a string: optional leading whitespace, an optional
/// sign, digits with an optional fraction and exponent, optional trailing
/// whitespace. Hex, `inf` and `nan` are not numeric.
pub fn php_is_numeric(text: &str) -> bool {
    let trimmed = text.trim_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let rest = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || fraction.is_some_and(|part| !digits(part)) {
        return false;
    }
    if whole.is_empty() && fraction.is_none_or(str::is_empty) {
        return false;
    }
    match exponent {
        None => true,
        Some(exp) => {
            let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
            !exp.is_empty() && digits(exp)
        }
    }
}

/// PHP's `(int)` cast of a string, kept to what a card count can hold: the
/// number the text starts with (after whitespace), cut toward zero, fraction
/// and exponent included (`"4.9"` is 4, `"1e1"` is 10, `"1e3x"` is 1000,
/// `"12abc"` is 12), or 0 when it starts with no number (`"abc"` is 0).
/// `None` when the result is negative or too big for a `u32`.
pub fn php_int(text: &str) -> Option<u32> {
    let trimmed = text.trim_start_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let number = leading_number(trimmed);
    if number.is_empty() {
        return Some(0);
    }
    let whole = number.parse::<f64>().ok()?.trunc();
    if whole < 0.0 || whole > f64::from(u32::MAX) {
        return None;
    }
    Some(whole as u32)
}

/// The longest number at the start of `text`, as PHP reads one: an optional
/// sign, digits with an optional `.` and fraction (at least one digit in
/// all), then an optional exponent (`e`, optional sign, digits). Empty when
/// the text does not start with a number.
fn leading_number(text: &str) -> &str {
    let bytes = text.as_bytes();
    let digits_from = |mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        at
    };
    let mut end = usize::from(text.starts_with(['+', '-']));
    let whole_start = end;
    end = digits_from(end);
    let mut digit_count = end - whole_start;
    if bytes.get(end) == Some(&b'.') {
        let fraction_end = digits_from(end + 1);
        digit_count += fraction_end - (end + 1);
        end = fraction_end;
    }
    if digit_count == 0 {
        return "";
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let exponent_end = digits_from(exponent);
        if exponent_end > exponent {
            end = exponent_end;
        }
    }
    &text[..end]
}

/// PHP 8.4's `round($value, $places)` (`PHP_ROUND_HALF_UP`): halves go
/// away from zero, and a half is judged on the decimal the float stands for,
/// so `1.005` gives `1.01` (although `1.005 * 100` is `100.49999999999999`)
/// while `57.49999999999999` gives 57. Negative `places` round to tens,
/// hundreds and so on. The one copy every 0.x port uses; pinned by
/// `fixtures/php/round.json`.
pub fn php_round(value: f64, places: i32) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    let exponent = power_of_ten(places.unsigned_abs());
    let scale_up = |number: f64| {
        if places > 0 {
            number * exponent
        } else {
            number / exponent
        }
    };
    let scale_down = |number: f64| {
        if places > 0 {
            number / exponent
        } else {
            number * exponent
        }
    };

    // The whole part of the scaled value, nudged up one when the next whole
    // number scales back to exactly `value` (0.285 * 100 is 28.499...).
    let mut whole = if value >= 0.0 {
        scale_up(value).floor()
    } else {
        scale_up(value).ceil()
    };
    let next = whole + 1.0_f64.copysign(value);
    if scale_down(next) == value {
        whole = next;
    }
    // Beyond a float's precision there is nothing left to round.
    if whole.abs() >= 1e16 {
        return value;
    }

    // Up when `value` reaches the halfway point, itself scaled back down.
    let halfway = scale_down(whole + 0.5_f64.copysign(value)).abs();
    if value.abs() >= halfway {
        whole += 1.0_f64.copysign(value);
    }
    scale_down(whole)
}

/// `10^power` as PHP's `php_intpow10` gives it: exact up to `10^22`.
fn power_of_ten(power: u32) -> f64 {
    if power > 22 {
        return 10f64.powi(power as i32);
    }
    let mut result = 1.0;
    for _ in 0..power {
        result *= 10.0;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_is_numeric_matches_php() {
        for yes in [
            "1", " 12", "12 ", "+3", "-3", "1.5", ".5", "5.", "1e3", "1E-2",
        ] {
            assert!(php_is_numeric(yes), "{yes:?}");
        }
        for no in [
            "", " ", "abc", "1a", "0x1A", ".", "e3", "1e", "--1", "1 2", "inf",
        ] {
            assert!(!php_is_numeric(no), "{no:?}");
        }
    }

    /// Every row of `fixtures/php/round.json`: `[value, places, round(value,
    /// places)]` as PHP 8.4 printed them (`serialize_precision` -1).
    #[test]
    fn php_round_matches_php_8_4() {
        let rows: Vec<(f64, i32, f64)> =
            serde_json::from_str(include_str!("../../../fixtures/php/round.json"))
                .expect("the round table is JSON");
        assert!(rows.len() > 600);
        for (value, places, expected) in rows {
            let got = php_round(value, places);
            assert_eq!(got, expected, "round({value:?}, {places})");
        }
    }

    #[test]
    fn php_int_casts_like_php() {
        assert_eq!(php_int("4"), Some(4));
        assert_eq!(php_int(" 4 "), Some(4));
        assert_eq!(php_int("4.9"), Some(4));
        assert_eq!(php_int("1e1"), Some(10));
        assert_eq!(php_int("12abc"), Some(12));
        assert_eq!(php_int("abc"), Some(0));
        assert_eq!(php_int(""), Some(0));
        assert_eq!(php_int("-3"), None);
        assert_eq!(php_int("99999999999"), None);
        // A leading number is read whole, fraction and exponent included.
        assert_eq!(php_int("1e3x"), Some(1000));
        assert_eq!(php_int("1.5e1x"), Some(15));
        assert_eq!(php_int(".5e1x"), Some(5));
        assert_eq!(php_int("1e+x"), Some(1));
        assert_eq!(php_int("4.x"), Some(4));
        assert_eq!(php_int("+4"), Some(4));
        assert_eq!(php_int("-0"), Some(0));
        assert_eq!(php_int("\t7"), Some(7));
        assert_eq!(php_int("\u{a0}7"), Some(0));
        assert_eq!(php_int("0x1A"), Some(0));
    }

    #[test]
    fn php_bool_matches_filter_var() {
        for value in ["true", "True", "TRUE", "tRuE", " true ", "1", "yes", "on"] {
            assert!(php_bool(value), "{value:?} should be true");
        }
        for value in ["false", "False", "0", "", "no", "off", "2"] {
            assert!(!php_bool(value), "{value:?} should be false");
        }
    }

    #[test]
    fn php_truthy_treats_empty_and_zero_as_false() {
        assert!(!php_truthy(""));
        assert!(!php_truthy("0"));
        assert!(php_truthy("W"));
        assert!(php_truthy("00"));
    }
}
