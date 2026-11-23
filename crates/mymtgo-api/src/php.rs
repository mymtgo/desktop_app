//! Encodings that must match 0.x's PHP byte for byte: `urlencode` and
//! `http_build_query` (RFC 1738: a space is `+`) and Carbon's
//! `toIso8601String()` in UTC (`YYYY-MM-DDTHH:MM:SS+00:00`).

/// PHP's `urlencode`: ASCII letters, digits and `-_.` stay, a space becomes
/// `+`, every other byte becomes `%XX` (uppercase hex).
pub fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// PHP's `urldecode`: `+` becomes a space and `%XX` its byte; a `%` not
/// followed by two hex digits stays as it is. Bytes that are not UTF-8 are
/// replaced.
pub fn urldecode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'+' {
            out.push(b' ');
            index += 1;
            continue;
        }
        if byte == b'%'
            && let Some(decoded) = hex_pair(bytes.get(index + 1), bytes.get(index + 2))
        {
            out.push(decoded);
            index += 3;
            continue;
        }
        out.push(byte);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte two hex digits spell, if both are hex digits.
fn hex_pair(high: Option<&u8>, low: Option<&u8>) -> Option<u8> {
    let digit = |byte: Option<&u8>| (*byte? as char).to_digit(16);
    Some((digit(high)? * 16 + digit(low)?) as u8)
}

/// PHP's `http_build_query` for a flat list of pairs (the default
/// `PHP_QUERY_RFC1738`): `key=value` joined with `&`, both sides encoded
/// with `urlencode`.
pub fn http_build_query(pairs: &[(String, String)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
        .collect();
    parts.join("&")
}

/// Carbon's `toIso8601String()` for a UTC unix millisecond time:
/// `YYYY-MM-DDTHH:MM:SS+00:00`, milliseconds dropped. One implementation,
/// `replay::iso8601_utc` (the snapshot's `played_at` uses it too), pinned by
/// `fixtures/php/iso8601_utc.json`.
pub fn iso8601_utc(ms: i64) -> String {
    replay::iso8601_utc(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_matches_php() {
        // php -r 'echo urlencode("a b+c/d?e=f&g~h*i.j-k_l");'
        assert_eq!(
            urlencode("a b+c/d?e=f&g~h*i.j-k_l"),
            "a+b%2Bc%2Fd%3Fe%3Df%26g%7Eh%2Ai.j-k_l"
        );
        // php -r 'echo urlencode("é");'
        assert_eq!(urlencode("é"), "%C3%A9");
        assert_eq!(urlencode(""), "");
    }

    #[test]
    fn urldecode_matches_php() {
        // php -r 'echo urldecode("a+b%2Bc%2fd%zz%4");'
        assert_eq!(urldecode("a+b%2Bc%2fd%zz%4"), "a b+c/d%zz%4");
        assert_eq!(urldecode("%C3%A9"), "é");
        assert_eq!(
            urldecode(&urlencode("Qplayer one/é?&=")),
            "Qplayer one/é?&="
        );
        assert_eq!(urldecode(""), "");
    }

    #[test]
    fn http_build_query_matches_php() {
        let pairs = vec![
            ("name".to_string(), "Qplayer one".to_string()),
            ("ids[]".to_string(), "1".to_string()),
            ("period".to_string(), "week".to_string()),
        ];
        assert_eq!(
            http_build_query(&pairs),
            "name=Qplayer+one&ids%5B%5D=1&period=week"
        );
        assert_eq!(http_build_query(&[]), "");
    }

    #[test]
    fn iso8601_matches_carbon_in_utc() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso8601_utc(1_759_925_045_999), "2025-10-08T12:04:05+00:00");
        assert_eq!(iso8601_utc(-1), "1969-12-31T23:59:59+00:00");
    }

    /// The shared table, through this crate's name for it.
    #[test]
    fn iso8601_matches_the_shared_table() {
        let cases: Vec<(i64, String)> =
            serde_json::from_str(include_str!("../../../fixtures/php/iso8601_utc.json"))
                .expect("the shared iso8601_utc table is JSON");

        assert!(!cases.is_empty());
        for (ms, expected) in cases {
            assert_eq!(iso8601_utc(ms), expected, "{ms}");
        }
    }
}
