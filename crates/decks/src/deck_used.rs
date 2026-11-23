//! The cards of a `deck_used` log event (0.x `ResolveMatchDeckFromLog` with
//! `ExtractJson`).
//!
//! MTGO logs the deck at the start of each game:
//! `(Twitch Info|Username: Q... Deck Used in Game ID: 945500844) [{"CatalogId":82696,"Quantity":4,"Annotation":"NotSet","InSideboard":false},...]`
//!
//! It also logs the match's registered deck (`FlsMatchDeckGetRespMessage`),
//! read by `parse_registered_deck`, and at a login during a tournament the
//! tournament's (`FlsTournamentDeckRespMessage`), read by
//! `parse_tournament_deck`.

use serde_json::Value;

use crate::{DeckCard, php_truthy};

/// The game id and cards of a stored `deck_used` event's raw text. `None` when
/// the text has no `Deck Used in Game ID: N`, no JSON, or a card list 0.x
/// could not use (not an array of cards, or empty: 0.x then leaves the match's
/// deck link alone). The first balanced JSON value in the text is the deck,
/// as in 0.x.
pub fn parse_deck_used(raw: &str) -> Option<(u64, Vec<DeckCard>)> {
    let game_id = game_id(raw)?;

    let Value::Array(items) = first_json(raw)? else {
        return None;
    };
    if items.is_empty() {
        return None;
    }

    let mut cards = Vec::new();
    for item in &items {
        cards.push(DeckCard {
            catalog_id: json_u32(item.get("CatalogId")?)?,
            quantity: crate::clamp_quantity(json_u32(item.get("Quantity")?)?),
            sideboard: json_truthy(item.get("InSideboard")?),
        });
    }
    Some((game_id, cards))
}

/// The match token and cards of a registered deck event (`MatchDeckRegistered`):
///
/// `(BaseClient|Inbound: FlsMatchDeckGetRespMessage) {"MatchToken":"...","MatchID":1,"Cards":[{"CatalogID":82696,"Annotation":0,"PermissionTypeCode":0,"Quantity":4,"InSideboard":false},...]}`
///
/// MTGO logs it just before sideboarding after game 1, and its list then
/// equals game 1's `deck_used` list. The key is `CatalogID` here. `None` when
/// the text is not this message, has no `MatchToken`, or has no usable cards
/// (the same rules as `parse_deck_used`). 0.x never read this message for
/// decks; v1 uses it only when game 1 has no `deck_used`.
pub fn parse_registered_deck(raw: &str) -> Option<(String, Vec<DeckCard>)> {
    const MARKER: &str = "FlsMatchDeckGetRespMessage";
    let at = raw.find(MARKER)?;
    let Value::Object(message) = first_json(&raw[at..])? else {
        return None;
    };
    let Some(Value::String(token)) = message.get("MatchToken") else {
        return None;
    };
    Some((token.clone(), registered_cards(message.get("Cards")?)?))
}

/// The tournament id and cards of a tournament's registered deck
/// (`MatchDeckRegistered` with no match token), logged when MTGO logs in
/// while the tournament runs:
///
/// `(Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in TournamentJoinedRoundInProgressState) ... Message: {"TournamentToken":"...","TournamentID":12857433,"DeckCards":[{"CatalogID":144586,...,"Quantity":4,"InSideboard":false},...],"DeckName":"...","DeckBoxID":1,"ReturnCode":0}`
///
/// The id is the match's `tournament_event_id`. MTGO locks a tournament's
/// list for the whole event, so it is the list every round's game 1 is played
/// with. `None` on the same rules as `parse_registered_deck`; a finished
/// tournament's answer has an empty list (`ReturnCode` 2).
pub fn parse_tournament_deck(raw: &str) -> Option<(u64, Vec<DeckCard>)> {
    const MARKER: &str = "FlsTournamentDeckRespMessage";
    let at = raw.find(MARKER)?;
    let Value::Object(message) = first_json(&raw[at..])? else {
        return None;
    };
    let Some(Value::Number(id)) = message.get("TournamentID") else {
        return None;
    };
    Some((id.as_u64()?, registered_cards(message.get("DeckCards")?)?))
}

/// The cards of a registered list (key `CatalogID`); `None` when it is not a
/// list, is empty or has a row 0.x could not use.
fn registered_cards(list: &Value) -> Option<Vec<DeckCard>> {
    let Value::Array(items) = list else {
        return None;
    };
    if items.is_empty() {
        return None;
    }
    let mut cards = Vec::new();
    for item in items {
        cards.push(DeckCard {
            catalog_id: json_u32(item.get("CatalogID")?)?,
            quantity: crate::clamp_quantity(json_u32(item.get("Quantity")?)?),
            sideboard: json_truthy(item.get("InSideboard")?),
        });
    }
    Some(cards)
}

/// The number after `Deck Used in Game ID:` (spaces allowed before it).
fn game_id(raw: &str) -> Option<u64> {
    const MARKER: &str = "Deck Used in Game ID:";
    let at = raw.find(MARKER)?;
    let after = raw[at + MARKER.len()..].trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok()
}

/// The first JSON object or array in the text (0.x `ExtractJson`): from each
/// `{` or `[` in turn, take the balanced text up to its matching close and keep
/// it when it parses. Also the tournament observations' payload (07).
///
/// The matching closes of every start are found in one pass per bracket kind
/// (`closing_positions`), so text full of unclosed brackets stays linear.
pub fn first_json(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let curly = closing_positions(bytes, b'{', b'}').0;
    let square = closing_positions(bytes, b'[', b']').0;
    for start in 0..bytes.len() {
        let end = match bytes[start] {
            b'{' => curly[start],
            b'[' => square[start],
            _ => None,
        };
        let Some(end) = end else {
            continue;
        };
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes[start..=end]) {
            return Some(value);
        }
    }
    None
}

/// For every `open` byte, the index of the bracket closing it (`None` when
/// the text ends first), skipping brackets inside JSON strings. Only the
/// opening kind counts towards depth, as in 0.x. A start is judged as if
/// scanning began at it with no string open: one scan serves every start it
/// reaches outside a string, and a start inside a string of that scan gets a
/// scan of its own.
///
/// Crafted quoting can make every start need its own scan, so the bytes
/// scanned are capped at `SCAN_BUDGET_PER_BYTE` times the text length (plus a
/// floor): past it the starts not yet answered get no end. Real lists scan
/// each byte about once. Also returns the bytes scanned.
fn closing_positions(bytes: &[u8], open: u8, close: u8) -> (Vec<Option<usize>>, usize) {
    let budget = SCAN_BUDGET_PER_BYTE * bytes.len() + SCAN_BUDGET_FLOOR;
    let mut scanned = 0usize;
    let mut ends = vec![None; bytes.len()];
    // Starts a scan has reached outside a string: their answer is final.
    let mut reached = vec![false; bytes.len()];
    for first in 0..bytes.len() {
        if bytes[first] != open || reached[first] {
            continue;
        }
        // Each open bracket, and whether this scan answers for it (an earlier
        // scan may have answered already).
        let mut open_starts: Vec<(usize, bool)> = Vec::new();
        let mut in_string = false;
        let mut escaped = false;
        for (index, &byte) in bytes.iter().enumerate().skip(first) {
            scanned += 1;
            if scanned > budget {
                return (ends, scanned);
            }
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
            } else if byte == b'"' {
                in_string = true;
            } else if byte == open {
                open_starts.push((index, !reached[index]));
                reached[index] = true;
            } else if byte == close
                && let Some((start, owned)) = open_starts.pop()
                && owned
            {
                ends[start] = Some(index);
            }
        }
    }
    (ends, scanned)
}

/// How many times each byte may be scanned, on average, before the scan gives up.
const SCAN_BUDGET_PER_BYTE: usize = 16;
/// Extra scan allowance so short texts always finish.
const SCAN_BUDGET_FLOOR: usize = 4096;

/// A whole number that fits a catalog id or quantity (a JSON number or a
/// numeric string).
fn json_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => u32::try_from(number.as_u64()?).ok(),
        Value::String(text) => text.trim().parse::<u32>().ok(),
        _ => None,
    }
}

/// PHP truthiness of a decoded JSON value (0.x `$card['InSideboard'] ? ...`).
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => php_truthy(text),
        Value::Array(items) => !items.is_empty(),
        Value::Object(_) => true,
    }
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

    const REAL_SHAPE: &str = "12:25:49 [INF] (Twitch Info|Username: Qrdxxxxxxx Deck Used in Game ID: 945500844) \
        [{\"CatalogId\":82696,\"Quantity\":4,\"Annotation\":\"NotSet\",\"InSideboard\":false},\
        {\"CatalogId\":139635,\"Quantity\":3,\"Annotation\":\"NotSet\",\"InSideboard\":false},\
        {\"CatalogId\":126021,\"Quantity\":2,\"Annotation\":\"NotSet\",\"InSideboard\":true}]";

    #[test]
    fn reads_the_game_id_and_cards() {
        assert_eq!(
            parse_deck_used(REAL_SHAPE),
            Some((
                945_500_844,
                vec![
                    card(82696, 4, false),
                    card(139635, 3, false),
                    card(126021, 2, true)
                ]
            ))
        );
    }

    #[test]
    fn a_reordered_log_list_gives_the_deck_file_signature() {
        // DetermineMatchDeckTest: log order differs from the XML order.
        let raw = "Deck Used in Game ID: 7) [{\"CatalogId\":1003,\"Quantity\":2,\"InSideboard\":true},\
            {\"CatalogId\":1001,\"Quantity\":4,\"InSideboard\":false},\
            {\"CatalogId\":1002,\"Quantity\":4,\"InSideboard\":false}]";
        let (_, cards) = parse_deck_used(raw).unwrap();
        assert_eq!(
            crate::signature(&cards),
            "MTAwMTo0OmZhbHNlfDEwMDI6NDpmYWxzZXwxMDAzOjI6dHJ1ZQ=="
        );
    }

    #[test]
    fn needs_the_game_id() {
        let raw = "12:00:00 [INF] (Deck|Used) [{\"CatalogId\":1001,\"Quantity\":4,\"InSideboard\":false}]";
        assert_eq!(parse_deck_used(raw), None);
    }

    #[test]
    fn needs_a_card_list() {
        assert_eq!(parse_deck_used("Deck Used in Game ID: 7)"), None);
        assert_eq!(parse_deck_used("Deck Used in Game ID: 7) []"), None);
        assert_eq!(
            parse_deck_used("Deck Used in Game ID: 7) [{\"CatalogId\":1"),
            None
        );
        assert_eq!(
            parse_deck_used("Deck Used in Game ID: 7) {\"CatalogId\":1}"),
            None
        );
        assert_eq!(
            parse_deck_used("Deck Used in Game ID: 7) [{\"Quantity\":4,\"InSideboard\":false}]"),
            None
        );
        assert_eq!(
            parse_deck_used("Deck Used in Game ID: 7) [{\"CatalogId\":1,\"Quantity\":4}]"),
            None
        );
        assert_eq!(
            parse_deck_used(
                "Deck Used in Game ID: 7) [{\"CatalogId\":-1,\"Quantity\":4,\"InSideboard\":false}]"
            ),
            None
        );
    }

    #[test]
    fn skips_bracketed_text_that_is_not_json() {
        // `[INF]` and `[x]` are tried first and fail, as in 0.x ExtractJson.
        let raw = "[INF] [x] Deck Used in Game ID:12 \
            [{\"CatalogId\":5,\"Quantity\":1,\"InSideboard\":false}] [{\"CatalogId\":6}]";
        assert_eq!(parse_deck_used(raw), Some((12, vec![card(5, 1, false)])));
    }

    #[test]
    fn brackets_inside_strings_do_not_end_the_list() {
        let raw = "Deck Used in Game ID: 3) \
            [{\"CatalogId\":5,\"Quantity\":1,\"Annotation\":\"a]b\\\"[\",\"InSideboard\":false}]";
        assert_eq!(parse_deck_used(raw), Some((3, vec![card(5, 1, false)])));
    }

    /// The old per-start scan, kept to check `first_json` finds the same text.
    fn first_json_slow(text: &str) -> Option<Value> {
        let bytes = text.as_bytes();
        for start in 0..bytes.len() {
            if bytes[start] != b'{' && bytes[start] != b'[' {
                continue;
            }
            let open = bytes[start];
            let close = if open == b'{' { b'}' } else { b']' };
            let (mut depth, mut in_string, mut escaped) = (0, false, false);
            let mut end = None;
            for (index, &byte) in bytes.iter().enumerate().skip(start) {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                    continue;
                }
                if byte == b'"' {
                    in_string = true;
                } else if byte == open {
                    depth += 1;
                } else if byte == close {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
            }
            if let Some(end) = end
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes[start..=end])
            {
                return Some(value);
            }
        }
        None
    }

    #[test]
    fn first_json_agrees_with_the_per_start_scan() {
        // A cheap deterministic generator over a hostile alphabet.
        let alphabet: &[u8] = b"{}[]\"\\ a1:,";
        let mut seed: u64 = 12345;
        for _ in 0..4000 {
            let mut text = String::new();
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = (seed >> 33) % 24;
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                text.push(alphabet[((seed >> 33) as usize) % alphabet.len()] as char);
            }
            assert_eq!(first_json(&text), first_json_slow(&text), "{text:?}");
        }
    }

    #[test]
    fn crafted_quoting_scans_a_bounded_number_of_bytes() {
        // A quote, then `{\"` over and over: every start sits inside a string
        // of the scans before it, so each used to scan to the end.
        let text = format!("\"{}", "{\\\"".repeat(40_000));
        let (_, scanned) = closing_positions(text.as_bytes(), b'{', b'}');
        let budget = SCAN_BUDGET_PER_BYTE * text.len() + SCAN_BUDGET_FLOOR;
        assert!(scanned <= budget + 1, "scanned {scanned} of {}", text.len());
        // Whole-text first_json also returns without a quadratic scan.
        assert_eq!(first_json(&text), None);
    }

    #[test]
    fn first_json_stays_linear_on_unclosed_brackets() {
        // Quadratic scanning would take minutes here.
        let text = format!("{}{}", "{[".repeat(200_000), "{\"a\":1}");
        let started = std::time::Instant::now();
        assert_eq!(first_json(&text), Some(serde_json::json!({"a": 1})));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    const REGISTERED: &str = "12:27:01 [INF] (BaseClient|Inbound: FlsMatchDeckGetRespMessage) \
        {\"MatchToken\":\"f5e33a1f-c2e7-4678-b30d-309b63f17a40\",\"MatchID\":285746550,\"Cards\":[\
        {\"CatalogID\":82696,\"Annotation\":0,\"PermissionTypeCode\":0,\"Quantity\":4,\"InSideboard\":false},\
        {\"CatalogID\":126021,\"Annotation\":0,\"PermissionTypeCode\":0,\"Quantity\":2,\"InSideboard\":true}]}";

    #[test]
    fn reads_the_registered_deck_and_its_match_token() {
        assert_eq!(
            parse_registered_deck(REGISTERED),
            Some((
                "f5e33a1f-c2e7-4678-b30d-309b63f17a40".to_string(),
                vec![card(82696, 4, false), card(126021, 2, true)]
            ))
        );
    }

    #[test]
    fn quantities_are_clamped_to_the_card_quantity_cap() {
        let raw = "Deck Used in Game ID: 7) [{\"CatalogId\":5,\"Quantity\":4000000000,\"InSideboard\":false}]";
        let (_, cards) = parse_deck_used(raw).expect("parses");
        assert_eq!(cards, vec![card(5, crate::MAX_CARD_QUANTITY, false)]);
        let registered = REGISTERED.replacen("\"Quantity\":4", "\"Quantity\":5000", 1);
        let (_, cards) = parse_registered_deck(&registered).expect("parses");
        assert_eq!(cards[0].quantity, crate::MAX_CARD_QUANTITY);
    }

    #[test]
    fn a_registered_deck_needs_the_message_a_token_and_cards() {
        let no_marker = REGISTERED.replace("FlsMatchDeckGetRespMessage", "FlsOtherMessage");
        assert_eq!(parse_registered_deck(&no_marker), None);
        let no_token = REGISTERED.replace("MatchToken", "Token");
        assert_eq!(parse_registered_deck(&no_token), None);
        let empty =
            "(BaseClient|Inbound: FlsMatchDeckGetRespMessage) {\"MatchToken\":\"t\",\"Cards\":[]}";
        assert_eq!(parse_registered_deck(empty), None);
        // The key is `CatalogID` here, not `CatalogId` as in `deck_used`.
        let wrong_key = REGISTERED.replace("CatalogID", "CatalogId");
        assert_eq!(parse_registered_deck(&wrong_key), None);
        assert_eq!(parse_registered_deck(REAL_SHAPE), None);
    }

    const TOURNAMENT: &str = "17:02:17 [INF] (Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in TournamentJoinedRoundInProgressState) \
        Processor: TournamentJoinedRoundInProgressState Message: \
        {\"TournamentToken\":\"c8eed54f-8182-44d5-adb0-dc8cb4ac95f7\",\"TournamentID\":12857433,\"DeckCards\":[\
        {\"CatalogID\":144586,\"Annotation\":0,\"PermissionTypeCode\":0,\"Quantity\":4,\"InSideboard\":false},\
        {\"CatalogID\":126021,\"Annotation\":0,\"PermissionTypeCode\":0,\"Quantity\":2,\"InSideboard\":true}],\
        \"DeckName\":\"Qdeck\",\"DeckBoxID\":1,\"ReturnCode\":0}\n\
        } Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament";

    #[test]
    fn reads_the_tournament_deck_and_its_tournament_id() {
        assert_eq!(
            parse_tournament_deck(TOURNAMENT),
            Some((
                12_857_433,
                vec![card(144586, 4, false), card(126021, 2, true)]
            ))
        );
    }

    #[test]
    fn a_tournament_deck_needs_the_message_an_id_and_cards() {
        // A finished tournament's answer: no list, `ReturnCode` 2.
        let finished = "(Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in X) \
            Message: {\"TournamentToken\":\"t\",\"TournamentID\":12857397,\"DeckCards\":[],\
            \"DeckName\":\"\",\"DeckBoxID\":0,\"ReturnCode\":2}";
        assert_eq!(parse_tournament_deck(finished), None);
        let no_marker = TOURNAMENT.replace("FlsTournamentDeckRespMessage", "FlsOtherMessage");
        assert_eq!(parse_tournament_deck(&no_marker), None);
        let no_id = TOURNAMENT.replace("\"TournamentID\":12857433,", "");
        assert_eq!(parse_tournament_deck(&no_id), None);
        let wrong_key = TOURNAMENT.replace("CatalogID", "CatalogId");
        assert_eq!(parse_tournament_deck(&wrong_key), None);
        assert_eq!(parse_tournament_deck(REGISTERED), None);
        assert_eq!(parse_registered_deck(TOURNAMENT), None);
    }

    #[test]
    fn in_sideboard_uses_php_truthiness() {
        let raw = "Deck Used in Game ID: 3) [\
            {\"CatalogId\":1,\"Quantity\":1,\"InSideboard\":1},\
            {\"CatalogId\":2,\"Quantity\":1,\"InSideboard\":0},\
            {\"CatalogId\":3,\"Quantity\":1,\"InSideboard\":\"0\"},\
            {\"CatalogId\":4,\"Quantity\":1,\"InSideboard\":\"false\"},\
            {\"CatalogId\":5,\"Quantity\":1,\"InSideboard\":null}]";
        let (_, cards) = parse_deck_used(raw).unwrap();
        let flags: Vec<bool> = cards.iter().map(|c| c.sideboard).collect();
        assert_eq!(flags, vec![true, false, false, true, false]);
    }
}
