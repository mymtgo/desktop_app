//! `game_management_json` events: the JSON header ids, the MetaMessage byte array,
//! and the key-value metadata block after `Receiver:`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

static MATCH_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)"MatchToken"\s*:\s*"([a-f0-9\-]{36})""#).expect("valid regex")
});
static MATCH_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""MatchID"\s*:\s*(\d+)"#).expect("valid regex"));
static GAME_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""GameID"\s*:\s*(\d+)"#).expect("valid regex"));
static META_MESSAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""MetaMessage"\s*:\s*\[([0-9,\s]*)\]"#).expect("valid regex"));
static KV_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^=]+?)\s*=\s*(.+)$").expect("valid regex"));

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameManagement {
    pub match_token: Option<String>,
    pub match_id: Option<u64>,
    pub game_id: Option<u64>,
    pub meta_message: Option<Vec<u8>>,
    pub kv: BTreeMap<String, String>,
}

pub fn parse_game_management(text: &str) -> GameManagement {
    GameManagement {
        match_token: MATCH_TOKEN.captures(text).map(|c| c[1].to_string()),
        match_id: MATCH_ID.captures(text).and_then(|c| c[1].parse().ok()),
        game_id: GAME_ID.captures(text).and_then(|c| c[1].parse().ok()),
        meta_message: META_MESSAGE
            .captures(text)
            .and_then(|c| parse_byte_list(&c[1])),
        kv: parse_kv_block(text),
    }
}

/// `"1, 2,255"` to bytes. Any value that is not 0..=255 makes the whole list invalid.
fn parse_byte_list(list: &str) -> Option<Vec<u8>> {
    list.split(',')
        .map(|value| value.trim().parse::<u8>().ok())
        .collect()
}

/// Port of 0.x `ExtractKeyValueBlock`: every `key = value` line after `Receiver:`.
pub fn parse_kv_block(text: &str) -> BTreeMap<String, String> {
    let Some(start) = text.find("Receiver:") else {
        return BTreeMap::new();
    };
    text[start..]
        .lines()
        .filter_map(|line| {
            let caps = KV_LINE.captures(line.trim())?;
            Some((caps[1].to_string(), caps[2].trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT: &str = "12:25:49 [INF] (Game Management|Processing Registered Handler for GsMessageMessage in LeagueMatchJoinedEventUnderwayState) Processor: LeagueMatchJoinedEventUnderwayState Message: {\"MatchToken\":\"f5e33a1f-c2e7-4678-b30d-309b63f17a40\",\"MatchID\":285746550,\"GameID\":945500844,\"MetaMessage\":[108,2,0,255]} Receiver: Event Token=f5e33a1f-c2e7-4678-b30d-309b63f17a40\nEvent Id:285746550\nCurrentStateProcessor=LeagueMatchJoinedEventUnderwayState\nDescription=Play up to 5 rounds with your Modern deck\nPlayFormatCd=CMODERN\nGameStructureCd= Modern\nLeague Token=d2050286-53fd-4072-804f-190d6a3c030a";

    #[test]
    fn reads_ids_bytes_and_key_values() {
        let gm = parse_game_management(EVENT);
        assert_eq!(
            gm.match_token.as_deref(),
            Some("f5e33a1f-c2e7-4678-b30d-309b63f17a40")
        );
        assert_eq!(gm.match_id, Some(285_746_550));
        assert_eq!(gm.game_id, Some(945_500_844));
        assert_eq!(gm.meta_message, Some(vec![108, 2, 0, 255]));
        assert_eq!(
            gm.kv.get("PlayFormatCd").map(String::as_str),
            Some("CMODERN")
        );
        assert_eq!(
            gm.kv.get("GameStructureCd").map(String::as_str),
            Some("Modern")
        );
        assert!(gm.kv.contains_key("League Token"));
        assert!(!gm.kv.contains_key("Event Id"));
    }

    #[test]
    fn malformed_byte_arrays_give_none_without_panicking() {
        for bad in ["[1,2,300]", "[1,x,3]", "[]", "[1,2,3"] {
            let text = format!("x Message: {{\"MatchID\":1,\"MetaMessage\":{bad}}}");
            assert_eq!(
                parse_game_management(&text).meta_message,
                None,
                "input {bad}"
            );
        }
    }

    #[test]
    fn no_receiver_means_no_key_values() {
        assert!(parse_kv_block("PlayFormatCd=CMODERN").is_empty());
    }
}
