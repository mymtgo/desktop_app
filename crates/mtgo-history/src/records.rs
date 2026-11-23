//! History records: the root `List<HistoricalItem>`'s `HistoricalMatch`
//! items, mapped as 0.x `extractMatches` / `cleanMatch` / `extractList` did.

use std::collections::HashMap;

use chrono::DateTime;
use chrono_tz::Tz;

use crate::HistoryError;
use crate::binary::{Object, Stream, Value, read_stream};

/// .NET ticks between 0001-01-01 and 1970-01-01.
const DOTNET_EPOCH_OFFSET: i64 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;
/// The top two bits of a .NET `DateTime` hold its kind, not ticks.
const TICKS_MASK: u64 = 0x3FFF_FFFF_FFFF_FFFF;
/// 0.x's accepted range, in seconds from 1970: years 1 to 9999.
const MIN_SECONDS: i64 = -62_135_596_800;
const MAX_SECONDS: i64 = 253_402_300_800;

/// One past match from MTGO's game history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRecord {
    /// MTGO's match id (`Id`).
    pub id: u64,
    /// `StartTime` as UTC unix milliseconds, truncated to the second. MTGO
    /// writes local wall-clock time; `None` when out of range.
    pub start_time_ms: Option<i64>,
    pub opponents: Vec<String>,
    pub match_winners: Vec<String>,
    pub match_losers: Vec<String>,
    pub game_wins: u32,
    pub game_losses: u32,
    pub game_wins_to_win: Option<u32>,
    pub game_ids: Vec<u64>,
    pub round: u32,
    pub description: String,
    /// `GameStructure.GameStructureCd`, such as `CMODERN`.
    pub format: Option<String>,
}

/// Every match record in one history file, in list order. Items without an
/// id are dropped. `tz` is the timezone MTGO wrote the start times in.
pub fn parse_history(bytes: &[u8], tz: Tz) -> Result<Vec<HistoryRecord>, HistoryError> {
    let stream = read_stream(bytes)?;
    let Value::Object(root) = stream.root() else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for item in list_items(&stream, root) {
        if let Value::Object(item) = stream.resolve(item)
            && let Some(record) = clean_match(&stream, item, tz)
        {
            records.push(record);
        }
    }
    Ok(records)
}

/// Records from several history files merged by id: a later file's record
/// replaces an earlier one in place, so the order is first appearance.
pub fn merge_records(files: Vec<Vec<HistoryRecord>>) -> Vec<HistoryRecord> {
    let mut merged: Vec<HistoryRecord> = Vec::new();
    let mut index_by_id: HashMap<u64, usize> = HashMap::new();
    for record in files.into_iter().flatten() {
        match index_by_id.get(&record.id) {
            Some(&index) => merged[index] = record,
            None => {
                index_by_id.insert(record.id, merged.len());
                merged.push(record);
            }
        }
    }
    merged
}

/// A .NET `List`'s live items: `_items` cut to `_size` (all of `_items`
/// when `_size` is not a number). Nothing when `_items` is not an array.
fn list_items<'s>(stream: &'s Stream, list: &'s Object) -> Vec<&'s Value> {
    let Some(Value::Array(items)) = list.member("_items").map(|items| stream.resolve(items)) else {
        return Vec::new();
    };
    let size = match list.member("_size").map(|size| stream.resolve(size)) {
        Some(Value::Int(size)) => usize::try_from(*size).unwrap_or(0).min(items.len()),
        _ => items.len(),
    };
    items[..size].iter().collect()
}

/// 0.x `extractList`: a `List`'s live items, a bare array's items, or the
/// value itself; nulls dropped, every value resolved.
fn extract_list<'s>(stream: &'s Stream, value: Option<&'s Value>) -> Vec<&'s Value> {
    let Some(value) = value else {
        return Vec::new();
    };
    let items: Vec<&Value> = match stream.resolve(value) {
        Value::Null => Vec::new(),
        Value::Object(list) if list.member("_items").is_some() => list_items(stream, list),
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    items
        .into_iter()
        .map(|item| stream.resolve(item))
        .filter(|item| **item != Value::Null)
        .collect()
}

fn strings(stream: &Stream, value: Option<&Value>) -> Vec<String> {
    extract_list(stream, value)
        .into_iter()
        .filter_map(|item| match item {
            Value::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn ids(stream: &Stream, value: Option<&Value>) -> Vec<u64> {
    extract_list(stream, value)
        .into_iter()
        .filter_map(as_u64)
        .collect()
}

fn as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Int(number) => u64::try_from(*number).ok(),
        Value::UInt(number) => Some(*number),
        _ => None,
    }
}

fn as_u32(value: Option<&Value>) -> Option<u32> {
    match value {
        Some(Value::Int(number)) => u32::try_from(*number).ok(),
        _ => None,
    }
}

/// 0.x `cleanMatch`. `None` when the item has no usable id.
fn clean_match(stream: &Stream, item: &Object, tz: Tz) -> Option<HistoryRecord> {
    let member = |name: &str| item.member(name).map(|value| stream.resolve(value));

    let id = member("Id").and_then(as_u64)?;
    let start_time_ms = match member("StartTime") {
        Some(Value::DateTime(raw)) => start_time_ms(*raw, tz),
        _ => None,
    };
    let description = match member("Description") {
        Some(Value::Text(text)) => text.clone(),
        _ => String::new(),
    };

    Some(HistoryRecord {
        id,
        start_time_ms,
        opponents: strings(stream, item.member("Opponents")),
        match_winners: strings(stream, item.member("MatchWinners")),
        match_losers: strings(stream, item.member("MatchLosers")),
        game_wins: as_u32(member("GameWins")).unwrap_or(0),
        game_losses: as_u32(member("GameLosses")).unwrap_or(0),
        game_wins_to_win: as_u32(member("GameWinsToWinMatch")),
        game_ids: ids(stream, item.member("GameIds")),
        round: as_u32(member("Round")).unwrap_or(0),
        description,
        format: format(stream, member("GameStructure")),
    })
}

/// `GameStructure.GameStructureCd`, or its `value__` when MTGO wrote it as
/// an enum object.
fn format(stream: &Stream, structure: Option<&Value>) -> Option<String> {
    let Some(Value::Object(structure)) = structure else {
        return None;
    };
    let mut code = stream.resolve(structure.member("GameStructureCd")?);
    if let Value::Object(wrapper) = code
        && let Some(inner) = wrapper.member("value__")
    {
        code = stream.resolve(inner);
    }
    match code {
        Value::Text(text) => Some(text.clone()),
        Value::Int(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A .NET `DateTime`'s raw bits to UTC unix milliseconds: kind bits masked
/// off, sub-seconds truncated, then read as wall-clock time in `tz` (0.x
/// `readDateTime`). A wall-clock time inside a DST gap is moved on by the
/// real length of the gap; an ambiguous one takes the earlier instant.
fn start_time_ms(raw: u64, tz: Tz) -> Option<i64> {
    let ticks = (raw & TICKS_MASK) as i64;
    let unix_ticks = ticks - DOTNET_EPOCH_OFFSET;
    let in_range = MIN_SECONDS * TICKS_PER_SECOND..=MAX_SECONDS * TICKS_PER_SECOND;
    if !in_range.contains(&unix_ticks) {
        return None;
    }
    let seconds = unix_ticks / TICKS_PER_SECOND;
    let wall_clock = DateTime::from_timestamp(seconds, 0)?.naive_utc();
    mtgo_protocol::local_to_utc_ms(wall_clock, tz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::build::{Bytes, DATETIME, INT32, Member};
    use chrono::{NaiveDate, NaiveDateTime};

    const LONDON: Tz = chrono_tz::Europe::London;

    /// .NET ticks of a wall-clock time, with `Local` kind bits set as MTGO
    /// writes them.
    fn ticks(wall_clock: &str) -> u64 {
        let at = NaiveDateTime::parse_from_str(wall_clock, "%Y-%m-%d %H:%M:%S%.f").expect("time");
        let micros = at.and_utc().timestamp_micros();
        let ticks = 621_355_968_000_000_000 + micros * 10;
        (ticks as u64) | 0x8000_0000_0000_0000
    }

    fn utc_ms(wall_clock: &str) -> i64 {
        NaiveDateTime::parse_from_str(wall_clock, "%Y-%m-%d %H:%M:%S")
            .expect("time")
            .and_utc()
            .timestamp_millis()
    }

    /// One hand-made `HistoricalMatch`.
    #[derive(Clone)]
    struct Match {
        id: Option<i32>,
        ticks: u64,
        opponents: Vec<Option<&'static str>>,
        winners: Vec<Option<&'static str>>,
        losers: Vec<Option<&'static str>>,
        wins: i32,
        losses: i32,
        to_win: i32,
        round: i32,
        game_ids: Vec<i32>,
        description: &'static str,
        format: &'static str,
        format_as_enum: bool,
    }

    fn a_match() -> Match {
        Match {
            id: Some(278_248_231),
            ticks: ticks("2025-05-06 21:04:56.789"),
            opponents: vec![Some("Qopponent")],
            winners: vec![Some("Qopponent")],
            losers: vec![Some("Qlocalxx")],
            wins: 0,
            losses: 2,
            to_win: 2,
            round: 0,
            game_ids: vec![915_866_742, 915_867_768],
            description: "",
            format: "CMODERN",
            format_as_enum: false,
        }
    }

    /// Writes a history stream shaped like MTGO's: a `List` root whose
    /// `_items` is a class array of references, the items after it.
    struct Writer {
        bytes: Bytes,
        next_id: i32,
        match_class: Option<i32>,
        string_list_class: Option<i32>,
        int_list_class: Option<i32>,
        structure_class: Option<i32>,
    }

    impl Writer {
        fn id(&mut self) -> i32 {
            self.next_id += 1;
            self.next_id
        }

        /// A `List<String>` value: its elements, `_size` and `_version`.
        fn string_list(&mut self, items: &[Option<&str>], size: i32) {
            let id = self.id();
            match self.string_list_class {
                Some(class) => {
                    self.bytes.class_with_id(id, class);
                }
                None => {
                    self.string_list_class = Some(id);
                    self.bytes.class_def(
                        4,
                        id,
                        "System.Collections.Generic.List`1[[System.String]]",
                        &[
                            ("_items", Member::StringArray),
                            ("_size", Member::Primitive(INT32)),
                            ("_version", Member::Primitive(INT32)),
                        ],
                        0,
                    );
                }
            }
            let array = self.id();
            self.bytes.string_array(array, items.len() as i32);
            for item in items {
                match item {
                    Some(text) => {
                        let string = self.id();
                        self.bytes.object_string(string, text);
                    }
                    None => {
                        self.bytes.null();
                    }
                }
            }
            self.bytes.i32(size).i32(0);
        }

        fn int_list(&mut self, items: &[i32]) {
            let id = self.id();
            match self.int_list_class {
                Some(class) => {
                    self.bytes.class_with_id(id, class);
                }
                None => {
                    self.int_list_class = Some(id);
                    self.bytes.class_def(
                        4,
                        id,
                        "System.Collections.Generic.List`1[[System.Int32]]",
                        &[
                            ("_items", Member::PrimitiveArray(INT32)),
                            ("_size", Member::Primitive(INT32)),
                            ("_version", Member::Primitive(INT32)),
                        ],
                        0,
                    );
                }
            }
            let array = self.id();
            // Capacity beyond the size, as .NET lists grow.
            let mut padded = items.to_vec();
            padded.extend([0, 0]);
            self.bytes.int32_array(array, &padded);
            self.bytes.i32(items.len() as i32).i32(0);
        }

        fn structure(&mut self, format: &str, as_enum: bool) {
            let id = self.id();
            match self.structure_class {
                Some(class) => {
                    self.bytes.class_with_id(id, class);
                }
                None => {
                    self.structure_class = Some(id);
                    self.bytes.class_def(
                        5,
                        id,
                        "GameStructure",
                        &[("GameStructureCd", Member::Object)],
                        2,
                    );
                }
            }
            if as_enum {
                let code = self.id();
                self.bytes
                    .class_def(4, code, "Code", &[("value__", Member::String)], 0);
                let text = self.id();
                self.bytes.object_string(text, format);
            } else {
                let text = self.id();
                self.bytes.object_string(text, format);
            }
        }

        fn item(&mut self, id: i32, item: &Match) {
            match self.match_class {
                Some(class) => {
                    self.bytes.class_with_id(id, class);
                }
                None => {
                    self.match_class = Some(id);
                    self.bytes.class_def(
                        5,
                        id,
                        "WotC.MtGO.Client.Model.Settings.History.HistoricalMatch",
                        &[
                            ("Id", Member::Object),
                            ("StartTime", Member::Primitive(DATETIME)),
                            ("Opponents", Member::SystemClass("List`1[String]")),
                            ("MatchWinners", Member::SystemClass("List`1[String]")),
                            ("MatchLosers", Member::SystemClass("List`1[String]")),
                            ("GameWins", Member::Primitive(INT32)),
                            ("GameLosses", Member::Primitive(INT32)),
                            ("GameWinsToWinMatch", Member::Primitive(INT32)),
                            ("Round", Member::Primitive(INT32)),
                            ("GameIds", Member::SystemClass("List`1[Int32]")),
                            ("Description", Member::String),
                            ("GameStructure", Member::Class("GameStructure", 2)),
                        ],
                        2,
                    );
                }
            }
            match item.id {
                Some(value) => {
                    self.bytes.u8(8).u8(INT32).i32(value);
                }
                None => {
                    self.bytes.null();
                }
            }
            self.bytes.u64(item.ticks);
            let size = item.opponents.len() as i32;
            self.string_list(&item.opponents, size);
            let size = item.winners.len() as i32;
            self.string_list(&item.winners, size);
            let size = item.losers.len() as i32;
            self.string_list(&item.losers, size);
            self.bytes
                .i32(item.wins)
                .i32(item.losses)
                .i32(item.to_win)
                .i32(item.round);
            self.int_list(&item.game_ids);
            let description = self.id();
            self.bytes.object_string(description, item.description);
            self.structure(item.format, item.format_as_enum);
        }
    }

    /// A history stream holding `matches`, with list `_size` `size` and
    /// `_items` capacity `capacity`.
    fn history(matches: &[Match], size: i32, capacity: i32) -> Vec<u8> {
        let mut writer = Writer {
            bytes: Bytes::header(1),
            next_id: 10_000,
            match_class: None,
            string_list_class: None,
            int_list_class: None,
            structure_class: None,
        };
        writer.bytes.library(2, "WotC.MtGO.Client.Model.Settings");
        writer
            .bytes
            .class_def(
                4,
                1,
                "System.Collections.Generic.List`1[[HistoricalItem]]",
                &[
                    ("_items", Member::Class("HistoricalItem[]", 2)),
                    ("_size", Member::Primitive(INT32)),
                    ("_version", Member::Primitive(INT32)),
                ],
                0,
            )
            .reference(3)
            .i32(size)
            .i32(size);
        writer.bytes.class_array(3, capacity, "HistoricalItem", 2);
        let item_ids: Vec<i32> = (0..matches.len() as i32).map(|n| 100 + n).collect();
        for id in &item_ids {
            writer.bytes.reference(*id);
        }
        let empty = capacity - matches.len() as i32;
        if empty > 0 {
            writer.bytes.nulls(empty);
        }
        for (id, item) in item_ids.iter().zip(matches) {
            writer.item(*id, item);
        }
        writer.bytes.end()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn maps_a_match_as_0x_cleans_it() {
        let bytes = history(&[a_match()], 1, 4);
        let records = parse_history(&bytes, LONDON).expect("parses");
        assert_eq!(
            records,
            vec![HistoryRecord {
                id: 278_248_231,
                // 21:04:56 BST is 20:04:56 UTC; the fraction is dropped.
                start_time_ms: Some(utc_ms("2025-05-06 20:04:56")),
                opponents: strings(&["Qopponent"]),
                match_winners: strings(&["Qopponent"]),
                match_losers: strings(&["Qlocalxx"]),
                game_wins: 0,
                game_losses: 2,
                game_wins_to_win: Some(2),
                game_ids: vec![915_866_742, 915_867_768],
                round: 0,
                description: String::new(),
                format: Some("CMODERN".to_string()),
            }]
        );
    }

    #[test]
    fn start_times_are_local_wall_clock_in_the_given_zone() {
        let bytes = history(&[a_match()], 1, 1);
        let utc = parse_history(&bytes, chrono_tz::UTC).expect("parses");
        assert_eq!(utc[0].start_time_ms, Some(utc_ms("2025-05-06 21:04:56")));
        let tokyo = parse_history(&bytes, chrono_tz::Asia::Tokyo).expect("parses");
        assert_eq!(tokyo[0].start_time_ms, Some(utc_ms("2025-05-06 12:04:56")));
    }

    #[test]
    fn dst_gaps_move_on_and_overlaps_take_the_earlier_instant() {
        let mut gap = a_match();
        gap.ticks = ticks("2025-03-30 01:30:00");
        let mut overlap = a_match();
        overlap.id = Some(2);
        overlap.ticks = ticks("2025-10-26 01:30:00");
        let records = parse_history(&history(&[gap, overlap], 2, 2), LONDON).expect("parses");
        assert_eq!(
            records[0].start_time_ms,
            Some(utc_ms("2025-03-30 01:30:00"))
        );
        assert_eq!(
            records[1].start_time_ms,
            Some(utc_ms("2025-10-26 00:30:00"))
        );
    }

    #[test]
    fn a_dst_gap_of_half_an_hour_moves_by_half_an_hour() {
        // Lord Howe jumps 02:00 to 02:30 on 2026-10-04 (+10:30 to +11:00).
        let mut gap = a_match();
        gap.ticks = ticks("2026-10-04 02:15:00");
        let records =
            parse_history(&history(&[gap], 1, 1), chrono_tz::Australia::Lord_Howe).expect("parses");
        // 02:45 at +11:00 is 15:45 UTC the day before.
        assert_eq!(
            records[0].start_time_ms,
            Some(utc_ms("2026-10-03 15:45:00"))
        );
    }

    #[test]
    fn out_of_range_start_times_are_none() {
        let mut item = a_match();
        item.ticks = 0x3FFF_FFFF_FFFF_FFFF;
        let records = parse_history(&history(&[item], 1, 1), LONDON).expect("parses");
        assert_eq!(records[0].start_time_ms, None);

        let mut item = a_match();
        item.ticks = 0;
        let records = parse_history(&history(&[item], 1, 1), chrono_tz::UTC).expect("parses");
        let year_one = NaiveDate::from_ymd_opt(1, 1, 1)
            .expect("date")
            .and_hms_opt(0, 0, 0)
            .expect("time")
            .and_utc()
            .timestamp_millis();
        assert_eq!(records[0].start_time_ms, Some(year_one));
    }

    #[test]
    fn items_past_the_list_size_and_null_items_are_skipped() {
        let mut second = a_match();
        second.id = Some(2);
        let mut third = a_match();
        third.id = Some(3);
        let bytes = history(&[a_match(), second, third], 2, 8);
        let ids: Vec<u64> = parse_history(&bytes, LONDON)
            .expect("parses")
            .iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(ids, vec![278_248_231, 2]);

        // A size past the capacity reads every item and the nulls after.
        let bytes = history(&[a_match()], 50, 4);
        assert_eq!(parse_history(&bytes, LONDON).expect("parses").len(), 1);
    }

    #[test]
    fn items_without_an_id_are_dropped() {
        let mut no_id = a_match();
        no_id.id = None;
        let mut negative = a_match();
        negative.id = Some(-1);
        let mut kept = a_match();
        kept.id = Some(5);
        let bytes = history(&[no_id, negative, kept], 3, 3);
        let ids: Vec<u64> = parse_history(&bytes, LONDON)
            .expect("parses")
            .iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(ids, vec![5]);
    }

    #[test]
    fn name_lists_drop_nulls() {
        let mut item = a_match();
        item.opponents = vec![None, Some("Qa"), None, Some("Qb")];
        item.winners = vec![];
        let records = parse_history(&history(&[item], 1, 1), LONDON).expect("parses");
        assert_eq!(records[0].opponents, strings(&["Qa", "Qb"]));
        assert!(records[0].match_winners.is_empty());
    }

    #[test]
    fn a_format_code_inside_an_enum_object_is_read() {
        let mut item = a_match();
        item.format = "CSTANDARD";
        item.format_as_enum = true;
        let records = parse_history(&history(&[item], 1, 1), LONDON).expect("parses");
        assert_eq!(records[0].format.as_deref(), Some("CSTANDARD"));
    }

    #[test]
    fn a_list_whose_items_are_itself_does_not_loop() {
        let mut bytes = Bytes::header(1);
        bytes
            .class_def(
                4,
                1,
                "List",
                &[
                    ("_items", Member::Object),
                    ("_size", Member::Primitive(INT32)),
                ],
                0,
            )
            .reference(1)
            .i32(5);
        assert_eq!(parse_history(&bytes.end(), LONDON), Ok(vec![]));

        // An item whose name list is the root list.
        let mut bytes = Bytes::header(1);
        bytes
            .class_def(
                4,
                1,
                "List",
                &[
                    ("_items", Member::ObjectArray),
                    ("_size", Member::Primitive(INT32)),
                ],
                0,
            )
            .object_array(2, 1)
            .class_def(
                4,
                3,
                "HistoricalMatch",
                &[
                    ("Id", Member::Primitive(INT32)),
                    ("Opponents", Member::Object),
                ],
                0,
            )
            .i32(9)
            .reference(1);
        bytes.i32(1);
        let records = parse_history(&bytes.end(), LONDON).expect("parses");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 9);
        assert!(records[0].opponents.is_empty());
    }

    #[test]
    fn a_root_that_is_not_a_list_has_no_records() {
        let bytes = Bytes::header(1).object_string(1, "x").end();
        assert_eq!(parse_history(&bytes, LONDON), Ok(vec![]));
    }

    #[test]
    fn corrupt_streams_are_errors() {
        let bytes = history(&[a_match()], 1, 2);
        for cut in 0..bytes.len() {
            assert!(
                parse_history(&bytes[..cut], LONDON).is_err(),
                "cut at {cut}"
            );
        }
        let mut bad = bytes.clone();
        bad[0] = 0x30;
        assert_eq!(parse_history(&bad, LONDON), Err(HistoryError::BadHeader));
    }

    fn record(id: u64, description: &str) -> HistoryRecord {
        HistoryRecord {
            id,
            start_time_ms: None,
            opponents: vec![],
            match_winners: vec![],
            match_losers: vec![],
            game_wins: 0,
            game_losses: 0,
            game_wins_to_win: None,
            game_ids: vec![],
            round: 0,
            description: description.to_string(),
            format: None,
        }
    }

    #[test]
    fn merge_keeps_first_order_and_the_last_file_wins() {
        let merged = merge_records(vec![
            vec![record(1, "old"), record(2, "old")],
            vec![record(3, "new"), record(1, "new")],
            vec![],
        ]);
        assert_eq!(
            merged,
            vec![record(1, "new"), record(2, "old"), record(3, "new")]
        );
    }
}
