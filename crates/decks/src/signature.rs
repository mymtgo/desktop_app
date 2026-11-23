//! Deck version signatures (0.x `GenerateDeckSignature`, the
//! `DeckVersion::getCardsAttribute` decoder and
//! `FindDeckVersionByOracleIdentity`'s printing-independent key).
//!
//! A signature is base64 of `"{catalog id}:{quantity}:{true|false}"` segments
//! joined with `|`, sorted by catalog id then by the sideboard text. It must
//! stay byte-identical to 0.x so imported versions keep matching.

use std::collections::BTreeMap;

use crate::{DeckCard, php_bool, php_int, php_is_numeric};

/// The signature of a card list. Rows are sorted by catalog id, then main
/// before side (`false` sorts before `true`); equal rows keep their order and
/// are never merged. An empty list gives an empty string.
pub fn signature(cards: &[DeckCard]) -> String {
    let mut sorted = cards.to_vec();
    // `sort_by_key` is stable, like PHP's sort, so equal rows keep their order.
    sorted.sort_by_key(|card| (card.catalog_id, card.sideboard));

    let segments: Vec<String> = sorted
        .iter()
        .map(|card| format!("{}:{}:{}", card.catalog_id, card.quantity, card.sideboard))
        .collect();

    base64_encode(segments.join("|").as_bytes())
}

/// The card rows of a stored signature. Anything 0.x could not decode gives an
/// empty list: invalid base64, and legacy signatures whose first segment is an
/// oracle id rather than a catalog id. Segments with fewer than three parts are
/// skipped, as are later segments whose numbers PHP would read as negative or beyond
/// a `u32` (other text reads as 0, as `(int)` does).
pub fn decode_signature(signature: &str) -> Vec<DeckCard> {
    let Some(bytes) = base64_decode(signature) else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new();
    };

    // Segments with at least three `:` parts, as in DeckVersion::getCardsAttribute.
    let segments: Vec<Vec<&str>> = text
        .split('|')
        .map(|segment| segment.split(':').collect::<Vec<&str>>())
        .filter(|parts| parts.len() >= 3)
        .collect();

    // Legacy signatures start with an oracle id; nothing here can read them.
    let Some(first) = segments.first() else {
        return Vec::new();
    };
    if !php_is_numeric(first[0]) {
        return Vec::new();
    }

    let mut cards = Vec::new();
    for parts in segments {
        // PHP casts with `(int)`: text that is not a number reads as 0.
        if let (Some(catalog_id), Some(quantity)) = (php_int(parts[0]), php_int(parts[1])) {
            cards.push(DeckCard {
                catalog_id,
                quantity,
                sideboard: php_bool(parts[2]),
            });
        }
    }
    cards
}

/// Printing-independent key of a card list: `"{oracle id}:{true|false}"` per
/// card with the quantities of every printing summed, sorted, then
/// `"{key}:{quantity}"` joined with `|`. `None` for an empty list or when any
/// card has no oracle id, since a partial key could match the wrong list.
pub fn oracle_key(cards: &[DeckCard], oracle_of: &dyn Fn(u32) -> Option<String>) -> Option<String> {
    if cards.is_empty() {
        return None;
    }

    // A BTreeMap keeps its keys sorted byte by byte, like PHP's ksort here.
    let mut quantities: BTreeMap<String, u64> = BTreeMap::new();
    for card in cards {
        let oracle_id = oracle_of(card.catalog_id)?;
        let key = format!("{}:{}", oracle_id, card.sideboard);
        *quantities.entry(key).or_insert(0) += u64::from(card.quantity);
    }

    let parts: Vec<String> = quantities
        .iter()
        .map(|(key, quantity)| format!("{key}:{quantity}"))
        .collect();
    Some(parts.join("|"))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (RFC 4648 alphabet) with `=` padding, as PHP's
/// `base64_encode`.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        out.push(ALPHABET[usize::from(b0 >> 2)] as char);
        out.push(ALPHABET[usize::from(((b0 & 0x03) << 4) | (b1 >> 4))] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[usize::from(((b1 & 0x0f) << 2) | (b2 >> 6))] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[usize::from(b2 & 0x3f)] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// The 6-bit value of a base64 alphabet character.
fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// PHP's `base64_decode($text, true)`: spaces, tabs, carriage returns and line
/// feeds are skipped; any other character outside the alphabet fails; padding
/// is optional but, when present, must complete the last group; data after
/// padding fails; a lone final character fails.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut values: Vec<u8> = Vec::new();
    let mut padding = 0;

    for byte in text.bytes() {
        if byte == b'=' {
            padding += 1;
            continue;
        }
        if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            continue;
        }
        let value = base64_value(byte)?;
        if padding > 0 {
            return None;
        }
        values.push(value);
    }

    if values.len() % 4 == 1 {
        return None;
    }
    if padding > 0 && (padding > 2 || !(values.len() + padding).is_multiple_of(4)) {
        return None;
    }

    // Every 4 values carry 3 bytes; a trailing 2 or 3 values carry 1 or 2.
    let mut out = Vec::new();
    for group in values.chunks(4) {
        let v0 = group[0];
        let v1 = group[1];
        out.push((v0 << 2) | (v1 >> 4));
        if let Some(&v2) = group.get(2) {
            out.push((v1 << 4) | (v2 >> 2));
            if let Some(&v3) = group.get(3) {
                out.push((v2 << 6) | v3);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    // Expected strings below were produced by running 0.x's
    // GenerateDeckSignature logic under PHP.

    #[test]
    fn signature_matches_0x_for_two_cards() {
        let cards = [card(1001, 4, false), card(1002, 4, false)];
        assert_eq!(signature(&cards), "MTAwMTo0OmZhbHNlfDEwMDI6NDpmYWxzZQ==");
    }

    #[test]
    fn signature_is_stable_for_the_same_cards() {
        let cards = [card(1001, 4, false), card(1002, 4, false)];
        assert_eq!(signature(&cards), signature(&cards));
    }

    #[test]
    fn signature_differs_for_different_quantities() {
        assert_eq!(signature(&[card(1001, 4, false)]), "MTAwMTo0OmZhbHNl");
        assert_eq!(signature(&[card(1001, 3, false)]), "MTAwMTozOmZhbHNl");
    }

    #[test]
    fn signature_differs_when_sideboard_differs() {
        assert_eq!(signature(&[card(1001, 4, true)]), "MTAwMTo0OnRydWU=");
        assert_ne!(
            signature(&[card(1001, 4, true)]),
            signature(&[card(1001, 4, false)])
        );
    }

    #[test]
    fn signature_of_an_empty_list_is_empty() {
        assert_eq!(signature(&[]), "");
    }

    #[test]
    fn signature_ignores_card_order() {
        let order_a = [
            card(1001, 4, false),
            card(1002, 4, false),
            card(1003, 2, true),
        ];
        let order_b = [
            card(1003, 2, true),
            card(1001, 4, false),
            card(1002, 4, false),
        ];
        let expected = "MTAwMTo0OmZhbHNlfDEwMDI6NDpmYWxzZXwxMDAzOjI6dHJ1ZQ==";
        assert_eq!(signature(&order_a), expected);
        assert_eq!(signature(&order_b), expected);
    }

    #[test]
    fn signature_sorts_ids_numerically_and_main_before_side() {
        let cards = [card(1001, 2, true), card(1001, 4, false)];
        assert_eq!(signature(&cards), "MTAwMTo0OmZhbHNlfDEwMDE6Mjp0cnVl");

        let numeric = [card(10, 1, false), card(9, 1, false), card(100, 1, false)];
        let decoded = base64_decode(&signature(&numeric)).unwrap();
        assert_eq!(decoded, b"9:1:false|10:1:false|100:1:false");
    }

    #[test]
    fn signature_keeps_duplicate_rows_in_input_order() {
        let cards = [
            card(1001, 4, false),
            card(1001, 2, true),
            card(1001, 1, false),
            card(999, 1, true),
        ];
        assert_eq!(
            signature(&cards),
            "OTk5OjE6dHJ1ZXwxMDAxOjQ6ZmFsc2V8MTAwMToxOmZhbHNlfDEwMDE6Mjp0cnVl"
        );
    }

    #[test]
    fn decode_reads_catalog_signatures() {
        let encoded = base64_encode(b"1001:4:false|1002:3:true");
        assert_eq!(
            decode_signature(&encoded),
            vec![card(1001, 4, false), card(1002, 3, true)]
        );
    }

    #[test]
    fn decode_round_trips_a_signature() {
        let cards = vec![
            card(1001, 4, false),
            card(1002, 4, false),
            card(1003, 2, true),
        ];
        assert_eq!(decode_signature(&signature(&cards)), cards);
    }

    #[test]
    fn decode_keeps_a_card_with_no_card_row() {
        let encoded = base64_encode(b"9999:1:false");
        assert_eq!(decode_signature(&encoded), vec![card(9999, 1, false)]);
    }

    #[test]
    fn decode_reads_numeric_sideboard_flags() {
        // GenerateDeckDekFileTest builds signatures with 0 and 1 flags.
        let encoded = base64_encode(b"100:4:0|200:20:0|100:2:1");
        assert_eq!(
            decode_signature(&encoded),
            vec![
                card(100, 4, false),
                card(200, 20, false),
                card(100, 2, true)
            ]
        );
    }

    #[test]
    fn decode_reads_numbers_like_php_int_casts() {
        // PHP: is_numeric(" 1001") is true, (int)"4.9" is 4, (int)"x" is 0.
        let encoded = base64_encode(b" 1001:4.9:false|1002:x:true|1003:2abc:false|-5:1:false");
        assert_eq!(
            decode_signature(&encoded),
            vec![
                card(1001, 4, false),
                card(1002, 0, true),
                card(1003, 2, false)
            ]
        );
    }

    #[test]
    fn decode_of_an_empty_signature_is_empty() {
        assert_eq!(decode_signature(""), vec![]);
    }

    #[test]
    fn decode_of_invalid_base64_is_empty() {
        assert_eq!(decode_signature("!!!not-base64!!!"), vec![]);
    }

    #[test]
    fn decode_of_a_legacy_oracle_signature_is_empty() {
        let encoded = base64_encode(b"oracle-1001:4:false|oracle-1002:3:true");
        assert_eq!(decode_signature(&encoded), vec![]);
    }

    #[test]
    fn decode_skips_segments_missing_parts() {
        let encoded = base64_encode(b"1001:4:false|malformed||1002:2:true");
        assert_eq!(
            decode_signature(&encoded),
            vec![card(1001, 4, false), card(1002, 2, true)]
        );
    }

    #[test]
    fn base64_encode_pads_like_php() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b"abcd"), "YWJjZA==");
        assert_eq!(base64_encode(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn base64_decode_follows_php_strict_mode() {
        // Each case checked against PHP's base64_decode($text, true).
        let ok: [(&str, &[u8]); 10] = [
            ("", b""),
            ("YQ", b"a"),
            ("YQ==", b"a"),
            ("YQ\n=\t=", b"a"),
            ("YQ==\n", b"a"),
            ("YR==", b"a"),
            ("YWJj", b"abc"),
            ("YW Jj", b"abc"),
            ("YW\rJj", b"abc"),
            ("YWJjZA", b"abcd"),
        ];
        for (text, expected) in ok {
            assert_eq!(base64_decode(text).as_deref(), Some(expected), "{text:?}");
        }
        let failing = [
            "Y",
            "A",
            "YQ=",
            "YQ==YQ==",
            "YWJjZA=",
            "YWJjZA===",
            "=",
            "==",
            "====",
            "YWJj=",
            "YW=j",
            "YW\x0bJj",
            "!!!not-base64!!!",
        ];
        for text in failing {
            assert_eq!(base64_decode(text), None, "{text:?}");
        }
        assert_eq!(base64_decode("+/8=").as_deref(), Some(&[0xfb, 0xff][..]));
    }

    fn oracle_table(catalog_id: u32) -> Option<String> {
        match catalog_id {
            1001 | 9001 => Some("oracle-basilisk-gate".to_string()),
            1002 => Some("oracle-dust-to-dust".to_string()),
            1003 => Some("oracle-1003".to_string()),
            _ => None,
        }
    }

    #[test]
    fn oracle_key_ignores_the_printing() {
        // DetermineMatchDeckTest: the log reports another printing of the
        // maindeck card, so the key still matches.
        let deck = [card(1001, 4, false), card(1002, 2, true)];
        let log = [card(9001, 4, false), card(1002, 2, true)];
        let key = oracle_key(&deck, &oracle_table);
        assert_eq!(
            key.as_deref(),
            Some("oracle-basilisk-gate:false:4|oracle-dust-to-dust:true:2")
        );
        assert_eq!(oracle_key(&log, &oracle_table), key);
    }

    #[test]
    fn oracle_key_sums_two_printings_of_one_card() {
        let cards = [card(1001, 2, false), card(9001, 2, false)];
        assert_eq!(
            oracle_key(&cards, &oracle_table).as_deref(),
            Some("oracle-basilisk-gate:false:4")
        );
    }

    #[test]
    fn oracle_key_differs_when_the_lists_differ() {
        let deck = [card(1001, 4, false), card(1002, 2, false)];
        let log = [card(9001, 3, false), card(1002, 2, false)];
        assert_ne!(
            oracle_key(&deck, &oracle_table),
            oracle_key(&log, &oracle_table)
        );
    }

    #[test]
    fn oracle_key_keeps_main_and_side_apart() {
        let cards = [card(1001, 4, false), card(9001, 1, true)];
        assert_eq!(
            oracle_key(&cards, &oracle_table).as_deref(),
            Some("oracle-basilisk-gate:false:4|oracle-basilisk-gate:true:1")
        );
    }

    #[test]
    fn oracle_key_needs_every_oracle_id() {
        let cards = [card(1001, 4, false), card(5555, 1, false)];
        assert_eq!(oracle_key(&cards, &oracle_table), None);
    }

    #[test]
    fn oracle_key_of_an_empty_list_is_none() {
        assert_eq!(oracle_key(&[], &oracle_table), None);
    }
}
