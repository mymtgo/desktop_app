//! MTGO `mtgo_game_history` files for fixtures: replace every string that
//! could name a person with a same-byte-length fake, in place. Strings are
//! length-prefixed, so a fake of another length would corrupt the stream.
//!
//! What is kept: the empty string, and the values of `GameStructure`
//! members (format code, format description, land set code), unless the
//! same text is also a player name. Everything else is replaced: opponents,
//! match winners and losers, the match `Description` (a room title another
//! player may have typed) and any string the reader cannot place.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use mtgo_history::binary::{Stream, Value, read_stream};

/// Text kept as it is: values of `GameStructure` members in the history's
/// match items.
pub fn kept_strings(stream: &Stream) -> BTreeSet<String> {
    let mut kept = BTreeSet::new();
    for item in match_items(stream) {
        let Some(Value::Object(structure)) = item
            .member("GameStructure")
            .map(|value| stream.resolve(value))
        else {
            continue;
        };
        for (_, value) in &structure.members {
            if let Value::Text(text) = stream.resolve(value) {
                kept.insert(text.clone());
            }
        }
    }
    kept
}

/// Every string a player-name list (`Opponents`, `MatchWinners`,
/// `MatchLosers`) holds.
pub fn player_names(stream: &Stream) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for item in match_items(stream) {
        for member in ["Opponents", "MatchWinners", "MatchLosers"] {
            let Some(Value::Object(list)) = item.member(member).map(|value| stream.resolve(value))
            else {
                continue;
            };
            let Some(Value::Array(entries)) =
                list.member("_items").map(|value| stream.resolve(value))
            else {
                continue;
            };
            for entry in entries {
                if let Value::Text(text) = stream.resolve(entry) {
                    names.insert(text.clone());
                }
            }
        }
    }
    names
}

/// The root list's items that are objects, whatever `_size` says.
fn match_items(stream: &Stream) -> Vec<&mtgo_history::binary::Object> {
    let Value::Object(root) = stream.root() else {
        return Vec::new();
    };
    let Some(Value::Array(items)) = root.member("_items").map(|value| stream.resolve(value)) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match stream.resolve(item) {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .collect()
}

/// Every string payload of the file, as text. Fails when a string is not
/// valid UTF-8 (its text would not keep its byte length).
fn all_strings(bytes: &[u8], stream: &Stream) -> Result<Vec<String>> {
    let mut texts = Vec::new();
    for span in &stream.strings {
        match std::str::from_utf8(&bytes[span.start..span.end]) {
            Ok(text) => texts.push(text.to_string()),
            Err(_) => bail!("a string at byte {} is not UTF-8", span.start),
        }
    }
    Ok(texts)
}

/// Real text to fake text for every string to replace. The same text always
/// gets the same fake; fakes keep the byte length, start with `Q` where the
/// length allows, and never equal (in any case) a string of the file or
/// another fake.
pub fn history_mapping(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let stream = read_stream(bytes)?;
    let names = player_names(&stream);
    let kept: BTreeSet<String> = kept_strings(&stream)
        .into_iter()
        .filter(|text| !names.contains(text))
        .collect();
    let texts = all_strings(bytes, &stream)?;
    let mut avoid: BTreeSet<String> = texts.iter().map(|text| text.to_lowercase()).collect();

    let to_replace: BTreeSet<&String> = texts
        .iter()
        .filter(|text| !text.is_empty() && !kept.contains(*text))
        .collect();
    let mut made_per_length: BTreeMap<usize, usize> = BTreeMap::new();
    let mut mapping = BTreeMap::new();
    for real in to_replace {
        let made = made_per_length.entry(real.len()).or_insert(0);
        let fake = fake_text(real.len(), *made, &avoid);
        *made += 1;
        avoid.insert(fake.to_lowercase());
        mapping.insert(real.clone(), fake);
    }
    Ok(mapping)
}

/// A fake of exactly `len` bytes, the first from attempt `index` on that is
/// not in `avoid` (lowercase); after 10,000 tries the last one is used.
fn fake_text(len: usize, index: usize, avoid: &BTreeSet<String>) -> String {
    let mut fake = String::new();
    for attempt in index..index + 10_000 {
        fake = candidate(len, attempt);
        if !avoid.contains(&fake.to_lowercase()) {
            break;
        }
    }
    fake
}

/// Attempt `attempt` at a fake of `len` bytes: `Q`, the attempt in base 26
/// letters, then `x` padding. Text too short for that (one or two bytes)
/// gets the attempt as `len` capital letters instead.
fn candidate(len: usize, attempt: usize) -> String {
    let mut fake = String::from("Q");
    let mut n = attempt;
    loop {
        fake.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            break;
        }
    }
    if fake.len() <= len {
        while fake.len() < len {
            fake.push('x');
        }
        return fake;
    }
    let mut letters = vec![b'A'; len];
    let mut n = attempt;
    for slot in letters.iter_mut().rev() {
        *slot = b'A' + (n % 26) as u8;
        n /= 26;
    }
    String::from_utf8(letters).expect("ASCII letters")
}

/// The file with every mapped string replaced in place.
pub fn anonymise_history(bytes: &[u8], mapping: &BTreeMap<String, String>) -> Result<Vec<u8>> {
    let stream = read_stream(bytes)?;
    let texts = all_strings(bytes, &stream)?;
    let mut out = bytes.to_vec();
    for (span, text) in stream.strings.iter().zip(texts) {
        if let Some(fake) = mapping.get(&text) {
            if fake.len() != span.end - span.start {
                bail!("a fake does not keep its string's byte length");
            }
            out[span.start..span.end].copy_from_slice(fake.as_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures_dir;
    use mtgo_history::parse_history;

    fn fixture() -> Vec<u8> {
        std::fs::read(fixtures_dir().join("history/mtgo_game_history")).expect("history fixture")
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn keeps_only_game_structure_values() {
        let stream = read_stream(&fixture()).expect("reads");
        assert_eq!(
            kept_strings(&stream),
            set(&[
                " Modern", " Pauper", " Pioneer", "CMODERN", "CPAUPER", "CPIONEER", "M15"
            ])
        );
    }

    #[test]
    fn maps_every_other_string_to_a_same_length_q_fake() {
        let bytes = fixture();
        let stream = read_stream(&bytes).expect("reads");
        let mapping = history_mapping(&bytes).expect("maps");
        let names = player_names(&stream);
        assert_eq!(names.len(), 548);
        for name in &names {
            let fake = mapping.get(name).expect("every name is mapped");
            assert_eq!(fake.len(), name.len());
            assert!(fake.starts_with('Q'));
        }
        let fakes: BTreeSet<&String> = mapping.values().collect();
        assert_eq!(fakes.len(), mapping.len(), "fakes are distinct");
        for kept in kept_strings(&stream) {
            assert!(!mapping.contains_key(&kept));
        }
        // Descriptions are replaced too.
        let records = parse_history(&bytes, chrono_tz::UTC).expect("parses");
        let described = records
            .iter()
            .find(|record| !record.description.is_empty())
            .expect("a described match");
        assert!(mapping.contains_key(&described.description));
    }

    #[test]
    fn anonymising_keeps_every_field_but_the_text() {
        let bytes = fixture();
        let mapping = history_mapping(&bytes).expect("maps");
        let out = anonymise_history(&bytes, &mapping).expect("anonymises");
        assert_eq!(out.len(), bytes.len());
        let before = parse_history(&bytes, chrono_tz::UTC).expect("parses");
        let after = parse_history(&out, chrono_tz::UTC).expect("parses");
        assert_eq!(after.len(), before.len());
        let fake = |name: &String| mapping.get(name).cloned().unwrap_or(name.clone());
        for (old, new) in before.iter().zip(&after) {
            assert_eq!(new.id, old.id);
            assert_eq!(new.start_time_ms, old.start_time_ms);
            assert_eq!(new.game_ids, old.game_ids);
            assert_eq!(
                (new.game_wins, new.game_losses),
                (old.game_wins, old.game_losses)
            );
            assert_eq!(new.format, old.format);
            assert_eq!(
                new.opponents,
                old.opponents.iter().map(fake).collect::<Vec<_>>()
            );
            assert_eq!(
                new.match_winners,
                old.match_winners.iter().map(fake).collect::<Vec<_>>()
            );
            assert_eq!(new.description, fake(&old.description));
        }
        let stream = read_stream(&out).expect("reads");
        for span in &stream.strings {
            let text = std::str::from_utf8(&out[span.start..span.end]).expect("UTF-8");
            assert!(!mapping.contains_key(text), "a mapped string survived");
        }
    }

    #[test]
    fn fakes_fit_short_strings() {
        assert_eq!(fake_text(1, 0, &BTreeSet::new()), "A");
        assert_eq!(fake_text(1, 0, &set(&["a"])), "B");
        assert_eq!(fake_text(2, 3, &BTreeSet::new()), "Qd");
        assert_eq!(fake_text(2, 26, &BTreeSet::new()), "BA");
        assert_eq!(fake_text(5, 0, &BTreeSet::new()), "Qaxxx");
        // A fake equal (in any case) to a string of the file is skipped.
        assert_eq!(fake_text(5, 0, &set(&["qaxxx"])), "Qbxxx");
    }
}
