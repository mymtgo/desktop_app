//! Replace real player names with same-byte-length fakes. Names inside MetaMessage
//! byte arrays are length-framed, so a fake of a different length would corrupt the
//! payload; same length keeps every frame valid.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::{
    decode_meta_message, parse_entry, parse_game_management, parse_header, parse_login,
    split_complete,
};
use regex::Regex;

/// A name after `Username:` or `Username=`, spaces included. It runs to the first
/// known terminator: an account id `(123)`, `Deck Used`, a closing bracket, quote or
/// comma, or the end of the line. Capturing too much only blanks extra text, while
/// stopping at a space would leave the rest of a real name in the fixture.
static ANY_USERNAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)Username[:=][ \t]*(.+?)(?:\s+\(\d+\)|\s+Deck Used\b|[)",]|\s*$)"#)
        .expect("valid regex")
});
static DECK_USED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Username: (.+?) Deck Used in Game ID").expect("valid regex"));
static PLAYER_JSON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\{"Id":\d+,"Name":"([^"]+)""#).expect("valid regex"));
static PERSON_KEYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#""(?:UserName|Username|PlayerName|ScreenName|LoginName|OpponentName)"\s*:\s*"([^"]+)""#,
    )
    .expect("valid regex")
});

/// The name on the first `MtGO Login Success` line: the local player. Uses the same
/// parser as ingestion so names with spaces are kept whole.
pub fn login_name(bytes: &[u8]) -> Option<String> {
    split_complete(bytes).iter().find_map(|event| {
        let header = parse_header(&event.text)?;
        if header.category != "Login" || header.context != "MtGO Login Success" {
            return None;
        }
        parse_login(&event.text).map(|login| login.username)
    })
}

/// Every player name the file reveals: login lines, game state players, person-like
/// JSON keys, deck-used lines, and players named inside decoded MetaMessages.
pub fn detect_names(bytes: &[u8]) -> BTreeSet<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut names = BTreeSet::new();
    if let Some(local) = login_name(bytes) {
        names.insert(local);
    }
    for regex in [&*ANY_USERNAME, &*DECK_USED, &*PLAYER_JSON, &*PERSON_KEYS] {
        for caps in regex.captures_iter(&text) {
            names.insert(caps[1].trim().to_string());
        }
    }
    for event in split_complete(bytes) {
        let Some(meta) = parse_game_management(&event.text).meta_message else {
            continue;
        };
        let Some(message) = decode_meta_message(&meta) else {
            continue;
        };
        if let Some(player) = parse_entry(&message).player() {
            names.insert(player.to_string());
        }
    }
    names.retain(|name| name.len() >= 3);
    names
}

/// A fake of exactly `len` bytes: `Q`, the index in base 26 letters, then `x` padding.
pub fn fake_name(len: usize, index: usize) -> String {
    let mut name = String::from("Q");
    let mut n = index;
    loop {
        name.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            break;
        }
    }
    assert!(
        name.len() <= len,
        "name of {len} bytes is too short for fake index {index}"
    );
    while name.len() < len {
        name.push('x');
    }
    name
}

pub fn build_mapping(names: &BTreeSet<String>) -> BTreeMap<String, String> {
    names
        .iter()
        .enumerate()
        .map(|(index, real)| (real.clone(), fake_name(real.len(), index)))
        .collect()
}

pub fn anonymise(bytes: &[u8], mapping: &BTreeMap<String, String>) -> Vec<u8> {
    let mut pairs: Vec<(&String, &String)> = mapping.iter().collect();
    // Longest first, so a short name never clobbers part of a longer one.
    pairs.sort_by_key(|(real, _)| std::cmp::Reverse(real.len()));

    let mut out = bytes.to_vec();
    for (real, fake) in &pairs {
        out = replace_bytes(&out, real.as_bytes(), fake.as_bytes());
    }
    // Names inside MetaMessage arrays appear as comma separated decimals.
    for (real, fake) in &pairs {
        let from = format!(",{},", decimal_list(real));
        let to = format!(",{},", decimal_list(fake));
        out = replace_bytes(&out, from.as_bytes(), to.as_bytes());
    }
    out
}

/// How many game_management events decode to a phrase. Equal before and after
/// anonymisation proves no frame was corrupted.
pub fn decoded_entry_count(bytes: &[u8]) -> usize {
    split_complete(bytes)
        .iter()
        .filter_map(|event| parse_game_management(&event.text).meta_message)
        .filter(|meta| decode_meta_message(meta).is_some())
        .count()
}

fn decimal_list(text: &str) -> String {
    text.bytes()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn replace_bytes(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_array(message: &str) -> String {
        let mut bytes = vec![0u8; 24];
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
        bytes
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn sample() -> String {
        format!(
            "12:00:00 [INF] (Login|MtGO Login Success) Username: RealLocal (1)\n\
             12:00:01 [INF] (Twitch Info|Game Play Status Update for Game ID: 1, Match ID: 2, Event ID: 2) {{\"Players\":[{{\"Id\":0,\"Name\":\"RealLocal\"}},{{\"Id\":1,\"Name\":\"Real Opp\"}}]}}\n\
             12:00:02 [INF] (Game Management|Processing Registered Handler for GsMessageMessage in MatchJoinedEventUnderwayState) Message: {{\"MatchID\":2,\"GameID\":1,\"MetaMessage\":[{}]}}\n",
            meta_array("@PReal Opp wins the game.")
        )
    }

    #[test]
    fn fake_names_keep_the_byte_length() {
        assert_eq!(fake_name(9, 0).len(), 9);
        assert_eq!(fake_name(3, 675).len(), 3);
        assert_ne!(fake_name(5, 0), fake_name(5, 1));
    }

    #[test]
    fn detects_login_json_and_metamessage_names() {
        let names = detect_names(sample().as_bytes());
        assert!(names.contains("RealLocal"));
        assert!(names.contains("Real Opp"));
        assert_eq!(
            login_name(sample().as_bytes()).as_deref(),
            Some("RealLocal")
        );
    }

    #[test]
    fn login_name_keeps_spaces() {
        let log = "12:00:00 [INF] (Login|MtGO Login Success) Username: Real Local (1)\n";
        assert_eq!(login_name(log.as_bytes()).as_deref(), Some("Real Local"));
    }

    #[test]
    fn username_lines_keep_spaced_names_whole() {
        let log = "12:00:00 [INF] (UI|Login Executed) Username: Real Local\r\n\
                   12:00:01 [INF] (Twitch Info|Username: Real Opp Deck Used to Join Event ID:10397) []\n\
                   12:00:02 [INF] (UI|Something) {\"Username\":\"x\",\"Username=Real Third,\"}\n";
        let names = detect_names(log.as_bytes());
        assert!(names.contains("Real Local"), "{} names", names.len());
        assert!(names.contains("Real Opp"), "{} names", names.len());
        assert!(names.contains("Real Third"), "{} names", names.len());
        assert!(!names.contains("Real"));
    }

    #[test]
    fn a_london_mulligan_line_yields_only_the_player_name() {
        let log = format!(
            "12:00:00 [INF] (Game Management|Processing Registered Handler for GsMessageMessage in MatchJoinedEventUnderwayState) Message: {{\"MatchID\":2,\"GameID\":1,\"MetaMessage\":[{}]}}\n",
            meta_array(
                "@PReal Local puts a card on the bottom of their library and begins the game with six cards in hand."
            )
        );
        let names = detect_names(log.as_bytes());
        assert_eq!(names.len(), 1);
        assert!(names.contains("Real Local"));
    }

    #[test]
    fn anonymised_output_keeps_metamessages_decodable() {
        let input = sample();
        let mapping = build_mapping(&detect_names(input.as_bytes()));
        let output = anonymise(input.as_bytes(), &mapping);
        let text = String::from_utf8(output.clone()).expect("utf8");
        assert!(!text.contains("RealLocal"));
        assert!(!text.contains("Real Opp"));
        assert!(
            detect_names(&output)
                .iter()
                .all(|name| !mapping.contains_key(name))
        );
        assert_eq!(
            decoded_entry_count(&output),
            decoded_entry_count(input.as_bytes())
        );
    }
}
