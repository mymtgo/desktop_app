//! MTGO game log files (`*Match_GameLog*.dat`) for fixtures: read the raw
//! records, replace player names with same-byte-length fakes, and write the
//! file back. The format (0.x `ParseGameLogBinary`): a header (u8 version,
//! u8 flag, u8 length + match UUID, u8 type, u8 flag, u8 length + game UUID),
//! then records of i64 LE ticks, a u8 flag, a 7-bit varint length and the
//! message bytes.
//!
//! The reader here keeps every byte (flags, header, a cut-short tail) so the
//! written file differs from the input only in the names. `game-detail`
//! owns the parser the app uses.

use std::collections::{BTreeMap, BTreeSet};

use mtgo_protocol::parse_entry;

/// One record, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatRecord {
    pub ticks: u64,
    pub flag: u8,
    pub message: Vec<u8>,
}

/// A whole `.dat` file, split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatFile {
    pub header: Vec<u8>,
    pub records: Vec<DatRecord>,
    /// Bytes after the last whole record (a record MTGO was still writing).
    pub tail: Vec<u8>,
}

/// Split a `.dat` file. `None` when it has no complete header.
pub fn read_dat(raw: &[u8]) -> Option<DatFile> {
    let match_len = *raw.get(2)? as usize;
    let game_len_at = 3 + match_len + 2;
    let game_len = *raw.get(game_len_at)? as usize;
    let header_end = game_len_at + 1 + game_len;
    let header = raw.get(..header_end)?.to_vec();

    let mut records = Vec::new();
    let mut pos = header_end;
    while let Some((record, next)) = read_record(raw, pos) {
        records.push(record);
        pos = next;
    }
    Some(DatFile {
        header,
        records,
        tail: raw[pos..].to_vec(),
    })
}

fn read_record(raw: &[u8], pos: usize) -> Option<(DatRecord, usize)> {
    let ticks = u64::from_le_bytes(raw.get(pos..pos + 8)?.try_into().ok()?);
    let flag = *raw.get(pos + 8)?;
    let mut at = pos + 9;
    let mut len: usize = 0;
    let mut shift = 0;
    loop {
        let byte = *raw.get(at)?;
        at += 1;
        len |= usize::from(byte & 0x7F).checked_shl(shift)?;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    let message = raw.get(at..at.checked_add(len)?)?.to_vec();
    Some((
        DatRecord {
            ticks,
            flag,
            message,
        },
        at + len,
    ))
}

/// The file's bytes, with each record's length re-encoded.
pub fn write_dat(file: &DatFile) -> Vec<u8> {
    let mut out = file.header.clone();
    for record in &file.records {
        out.extend_from_slice(&record.ticks.to_le_bytes());
        out.push(record.flag);
        let mut len = record.message.len();
        loop {
            let byte = (len & 0x7F) as u8;
            len >>= 7;
            if len == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.extend_from_slice(&record.message);
    }
    out.extend_from_slice(&file.tail);
    out
}

/// Every player the records name in a result phrase (rolls, joins, wins,
/// concedes, ...), as the `mtgo.log` anonymiser finds them.
pub fn dat_names(file: &DatFile) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for record in &file.records {
        let message = String::from_utf8_lossy(&record.message);
        if let Some(player) = parse_entry(&message).player() {
            names.insert(player.to_string());
        }
    }
    names.retain(|name| name.len() >= MIN_NAME_LEN);
    names
}

/// Names under this many bytes are not replaced (they would match inside
/// ordinary words); `dat_short_names` reports them so the tool can refuse.
const MIN_NAME_LEN: usize = 3;

/// Players named in a result phrase whose names are too short to replace
/// safely. The anonymiser stops when there are any.
pub fn dat_short_names(file: &DatFile) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for record in &file.records {
        let message = String::from_utf8_lossy(&record.message);
        if let Some(player) = parse_entry(&message).player()
            && player.len() < MIN_NAME_LEN
        {
            names.insert(player.to_string());
        }
    }
    names
}

/// Whether `name` is still in `bytes`, ignoring ASCII case (the survival
/// check after anonymising must not pass a name that differs only in case).
pub fn contains_name(bytes: &[u8], name: &str) -> bool {
    let name = name.as_bytes();
    !name.is_empty()
        && bytes
            .windows(name.len())
            .any(|window| window.eq_ignore_ascii_case(name))
}

/// The file with every mapped name replaced, longest name first, in every
/// record and in the tail. Fakes must have the real names' byte lengths.
pub fn anonymise_dat(file: &DatFile, mapping: &BTreeMap<String, String>) -> DatFile {
    let mut pairs: Vec<(&String, &String)> = mapping.iter().collect();
    pairs.sort_by_key(|(real, _)| std::cmp::Reverse(real.len()));
    let replace_all = |bytes: &[u8]| {
        let mut out = bytes.to_vec();
        for (real, fake) in &pairs {
            assert_eq!(real.len(), fake.len(), "fakes keep the byte length");
            out = replace_bytes(&out, real.as_bytes(), fake.as_bytes());
        }
        out
    };
    DatFile {
        header: file.header.clone(),
        records: file
            .records
            .iter()
            .map(|record| DatRecord {
                ticks: record.ticks,
                flag: record.flag,
                message: replace_all(&record.message),
            })
            .collect(),
        tail: replace_all(&file.tail),
    }
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
    use crate::anonymise::build_mapping;

    const UUID: &[u8] = b"11111111-2222-4333-8444-555555555555";

    fn record(ticks: u64, message: &str) -> DatRecord {
        DatRecord {
            ticks,
            flag: 7,
            message: message.as_bytes().to_vec(),
        }
    }

    fn sample() -> DatFile {
        let mut header = vec![1, 0, UUID.len() as u8];
        header.extend_from_slice(UUID);
        header.extend_from_slice(&[4, 0, UUID.len() as u8]);
        header.extend_from_slice(UUID);
        let long = format!(
            "@PReal Local casts @[{}@:1000,1:@] targeting Real Opp.",
            "x".repeat(200)
        );
        DatFile {
            header,
            records: vec![
                record(638_000_000_000_000_000, "@PReal Local rolled a 4."),
                record(638_000_000_010_000_000, "@P@PReal Opp joined the game."),
                record(638_000_000_020_000_000, &long),
                record(638_000_000_030_000_000, "@PTurn 1: Real Local"),
                record(638_000_000_040_000_000, "@PReal Opp wins the game."),
            ],
            tail: b"\x01\x02Real Opp".to_vec(),
        }
    }

    #[test]
    fn writes_back_the_bytes_it_read() {
        let raw = write_dat(&sample());
        let read = read_dat(&raw).expect("dat");
        assert_eq!(read, sample());
        assert_eq!(write_dat(&read), raw);
    }

    #[test]
    fn a_file_without_a_whole_header_reads_as_none() {
        assert_eq!(read_dat(&[1, 0, 36, b'a']), None);
        assert_eq!(read_dat(&[]), None);
    }

    #[test]
    fn finds_the_players_named_in_result_phrases() {
        let names = dat_names(&sample());
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["Real Local".to_string(), "Real Opp".to_string()]
        );
    }

    #[test]
    fn anonymised_file_keeps_every_record_and_loses_every_name() {
        let original = write_dat(&sample());
        let mapping = build_mapping(&dat_names(&sample()));
        let anonymised = write_dat(&anonymise_dat(&sample(), &mapping));

        assert_eq!(anonymised.len(), original.len());
        let read = read_dat(&anonymised).expect("dat");
        assert_eq!(read.records.len(), sample().records.len());
        for (before, after) in sample().records.iter().zip(&read.records) {
            assert_eq!(before.ticks, after.ticks);
            assert_eq!(before.flag, after.flag);
            assert_eq!(before.message.len(), after.message.len());
        }
        for real in mapping.keys() {
            assert!(
                !anonymised
                    .windows(real.len())
                    .any(|window| window == real.as_bytes()),
                "a name survived"
            );
        }
        let text = String::from_utf8_lossy(&anonymised);
        let local = &mapping["Real Local"];
        let opponent = &mapping["Real Opp"];
        assert!(text.contains(&format!("@P{local} rolled a 4.")));
        assert!(text.contains(&format!("@PTurn 1: {local}")));
        assert!(text.contains(&format!("targeting {opponent}.")));
    }

    #[test]
    fn short_names_are_reported_not_dropped_silently() {
        let mut file = sample();
        file.records.push(record(5, "@PAb rolled a 3."));
        assert_eq!(
            dat_short_names(&file).into_iter().collect::<Vec<_>>(),
            vec!["Ab".to_string()]
        );
        assert!(!dat_names(&file).contains("Ab"));
    }

    #[test]
    fn the_survival_check_ignores_case() {
        assert!(contains_name(b"hello REAL local there", "Real Local"));
        assert!(!contains_name(b"hello", "Real Local"));
        assert!(!contains_name(b"hello", ""));
    }
}
