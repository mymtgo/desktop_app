//! Game state frames: the JSON MTGO logs after
//! `Game Play Status Update for Game ID: N, Match ID: M, Event ID: E`.
//!
//! `{"Players":[{"Id","Name","LibraryCount","HandCount","Life"}],"Cards":[{"Id","CatalogID","Zone","ActualZone","Owner","Controller"}]}`
//!
//! Only the local player's Hand, Library and Sideboard are visible; the
//! opponent shows public zones only. About 60% of frames repeat the previous
//! one. The JSON can be malformed or cut short, so reads are tolerant: a
//! missing or `null` field takes its default, an oddly typed field is
//! coerced as 0.x's PHP casts did (#155), a list entry that is not an object
//! is skipped, text after the JSON is ignored, and a frame that cannot be
//! read at all is skipped (`None`).

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// One game state snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Frame {
    #[serde(rename = "Players", deserialize_with = "list")]
    pub players: Vec<FramePlayer>,
    #[serde(rename = "Cards", deserialize_with = "list")]
    pub cards: Vec<FrameCard>,
}

/// A player in a frame. `id` is the in-game seat id that cards' `owner` and
/// `controller` refer to (0 or 1, and it differs between matches).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FramePlayer {
    #[serde(rename = "Id", deserialize_with = "int")]
    pub id: i64,
    #[serde(rename = "Name", deserialize_with = "text")]
    pub name: String,
    #[serde(rename = "LibraryCount", deserialize_with = "int")]
    pub library_count: i64,
    #[serde(rename = "HandCount", deserialize_with = "int")]
    pub hand_count: i64,
    #[serde(rename = "Life", deserialize_with = "int")]
    pub life: i64,
}

/// A card in a frame. `id` is the in-game instance id; `catalog_id` is the
/// MTGO printing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FrameCard {
    #[serde(rename = "Id", deserialize_with = "int")]
    pub id: i64,
    #[serde(rename = "CatalogID", deserialize_with = "catalog_id")]
    pub catalog_id: u32,
    #[serde(rename = "Zone", deserialize_with = "text")]
    pub zone: String,
    #[serde(rename = "ActualZone", deserialize_with = "text")]
    pub actual_zone: String,
    #[serde(rename = "Owner", deserialize_with = "int")]
    pub owner: i64,
    #[serde(rename = "Controller", deserialize_with = "int")]
    pub controller: i64,
}

impl Frame {
    /// The player with this seat id, if the frame has one.
    pub fn player(&self, id: i64) -> Option<&FramePlayer> {
        self.players.iter().find(|player| player.id == id)
    }
}

/// The frame in a stored `game_state_update` event's text. `None` when the
/// text has no `Game Play Status Update for Game ID:` or its JSON cannot be read.
pub fn parse_frame(raw: &str) -> Option<Frame> {
    parse_frame_content(frame_json(raw)?)
}

/// A frame from its bare JSON text, without the log header: how the 0.x
/// import stores frames (0.x `game_timelines.content`). `None` when the JSON
/// cannot be read.
pub fn parse_frame_content(json: &str) -> Option<Frame> {
    // Read one JSON value and ignore whatever follows it (continuation lines).
    // A value that is not an object is no frame.
    let mut stream = serde_json::Deserializer::from_str(json).into_iter::<Entry<Frame>>();
    stream.next()?.ok()?.0
}

/// The readable frames of one game's bare JSON texts, in the order given.
/// A text that is the same as the previous one is skipped without parsing.
pub fn parse_frame_contents(jsons: &[&str]) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut previous: Option<&str> = None;
    for json in jsons {
        if previous == Some(*json) {
            continue;
        }
        previous = Some(*json);
        if let Some(frame) = parse_frame_content(json) {
            frames.push(frame);
        }
    }
    frames
}

/// The readable frames of one game's events, in the order given (the caller
/// orders them by `(file_id, byte_offset)`). A frame whose JSON text is the
/// same as the previous event's is skipped without parsing: repeats change
/// nothing in this crate's results.
pub fn parse_frames(raws: &[&str]) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut previous_line: Option<&str> = None;
    for raw in raws {
        let Some(json) = frame_json(raw) else {
            continue;
        };
        // The JSON sits on the header line; later lines are not part of it.
        let line = json.split('\n').next().unwrap_or(json);
        if previous_line == Some(line) {
            continue;
        }
        previous_line = Some(line);
        if let Some(frame) = parse_frame(raw) {
            frames.push(frame);
        }
    }
    frames
}

/// The frame's JSON text in a stored `game_state_update` event: from the
/// first `{` after the frame marker to the end of the text, trailing
/// whitespace trimmed. Two frames are repeats when these texts are equal.
pub fn frame_text(raw: &str) -> Option<&str> {
    Some(frame_json(raw)?.trim_end())
}

/// The frame in a stored event's text as a whole JSON value, every key kept
/// (the replay viewer reads keys `Frame` drops). The same tolerant read as
/// `parse_frame`: the first JSON value after the marker, whatever follows it
/// ignored. `None` without the marker or when the JSON cannot be read.
pub fn frame_value(raw: &str) -> Option<serde_json::Value> {
    frame_content_value(frame_json(raw)?)
}

/// A bare frame JSON text (0.x `game_timelines.content`, or `frame_text`)
/// as a whole JSON value: the first JSON value, whatever follows ignored.
pub fn frame_content_value(json: &str) -> Option<serde_json::Value> {
    let mut stream = serde_json::Deserializer::from_str(json).into_iter::<serde_json::Value>();
    stream.next()?.ok()
}

/// The time of day of a stored event's header, `HH:MM:SS` (0.x stored it as
/// the frame's `log_events.timestamp`). `None` when the text does not start
/// with a log header.
pub fn frame_time(raw: &str) -> Option<String> {
    let time = mtgo_protocol::header_time(raw)?;
    Some(time.format("%H:%M:%S").to_string())
}

/// The text from the first `{` after the frame marker, if any.
fn frame_json(raw: &str) -> Option<&str> {
    const MARKER: &str = "Game Play Status Update for Game ID:";
    let at = raw.find(MARKER)?;
    let open = raw[at..].find('{')?;
    Some(&raw[at + open..])
}

/// A number field, coerced as 0.x's PHP `(int)` cast did: a fraction is
/// cut off, a string gives its leading number (0 when it has none), `true`
/// is 1. `null`, a list or an object is 0.
fn int<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(IntVisitor)
}

/// A printing id: as `int`, and 0 when it is not a valid `u32` (negative or
/// too large), as for any unknown printing.
fn catalog_id<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(u32::try_from(int(deserializer)?).unwrap_or(0))
}

/// A text field, coerced as PHP's `(string)` cast did: a number gives its
/// digits, `true` is "1". `null`, `false`, a list or an object is empty.
fn text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(TextVisitor)
}

/// A list of objects: an entry that is not an object is skipped, and a
/// value that is not a list is an empty list.
fn list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_any(ListVisitor(PhantomData))
}

/// PHP's `(int)` of a string: leading whitespace, then the longest number
/// at the start ("7 cards" is 7, "1.9" is 1, "1e3" is 1000), else 0.
fn php_int(text: &str) -> i64 {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == digits_start {
        return 0;
    }
    let whole = end;
    // A fraction or an exponent makes it a float, cut toward zero.
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut exponent = end + 1;
        if exponent < bytes.len() && (bytes[exponent] == b'+' || bytes[exponent] == b'-') {
            exponent += 1;
        }
        let exponent_digits = exponent;
        while exponent < bytes.len() && bytes[exponent].is_ascii_digit() {
            exponent += 1;
        }
        if exponent > exponent_digits {
            end = exponent;
        }
    }
    // A whole number too large for `i64` saturates, as the float path does.
    if end == whole
        && let Ok(value) = text[..end].parse::<i64>()
    {
        return value;
    }
    text[..end].parse::<f64>().map(float_to_int).unwrap_or(0)
}

/// A float cut toward zero; out of range saturates (Rust's `as`).
fn float_to_int(value: f64) -> i64 {
    value as i64
}

/// Reads any JSON value as `int` does.
struct IntVisitor;

impl<'de> Visitor<'de> for IntVisitor {
    type Value = i64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<i64, E> {
        Ok(i64::from(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<i64, E> {
        Ok(value)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<i64, E> {
        Ok(i64::try_from(value).unwrap_or(i64::MAX))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<i64, E> {
        Ok(float_to_int(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<i64, E> {
        Ok(php_int(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<i64, E> {
        Ok(0)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<i64, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(0)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<i64, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(0)
    }
}

/// Reads any JSON value as `text` does.
struct TextVisitor;

impl<'de> Visitor<'de> for TextVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<String, E> {
        Ok(if value {
            "1".to_string()
        } else {
            String::new()
        })
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<String, E> {
        Ok(value.to_string())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<String, E> {
        Ok(value.to_string())
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<String, E> {
        Ok(value.to_string())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        Ok(value.to_string())
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<String, E> {
        Ok(value)
    }

    fn visit_unit<E: de::Error>(self) -> Result<String, E> {
        Ok(String::new())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<String, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(String::new())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<String, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(String::new())
    }
}

/// One value that is read as `T` only when it is an object (`None` for any
/// other value), so a list keeps its other entries.
struct Entry<T>(Option<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Entry<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(EntryVisitor(PhantomData))
    }
}

struct EntryVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for EntryVisitor<T> {
    type Value = Entry<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Entry<T>, A::Error> {
        let value = T::deserialize(de::value::MapAccessDeserializer::new(map))?;
        Ok(Entry(Some(value)))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Entry<T>, E> {
        Ok(Entry(None))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Entry<T>, E> {
        Ok(Entry(None))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Entry<T>, E> {
        Ok(Entry(None))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Entry<T>, E> {
        Ok(Entry(None))
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Entry<T>, E> {
        Ok(Entry(None))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Entry<T>, E> {
        Ok(Entry(None))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Entry<T>, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Entry(None))
    }
}

/// Reads any JSON value as `list` does.
struct ListVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for ListVisitor<T> {
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut items = Vec::new();
        while let Some(entry) = seq.next_element::<Entry<T>>()? {
            if let Some(item) = entry.0 {
                items.push(item);
            }
        }
        Ok(items)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_unit<E: de::Error>(self) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "12:26:09 [INF] (Twitch Info|Game Play Status Update for Game ID: 945500844, Match ID: 285746550, Event ID: 285746550) ";

    fn line(json: &str) -> String {
        format!("{HEADER}{json}")
    }

    #[test]
    fn reads_players_and_cards() {
        let raw = line(
            r#"{"Players":[{"Id":0,"Name":"Qrdxxxxxxx","LibraryCount":53,"HandCount":7,"Life":20},{"Id":1,"Name":"Quxxxxx","LibraryCount":53,"HandCount":7,"Life":19}],"Cards":[{"Id":416,"CatalogID":125847,"Zone":"Hand","ActualZone":"Hand","Owner":0,"Controller":0}]}"#,
        );
        let frame = parse_frame(&raw).expect("frame");

        assert_eq!(
            frame.players,
            vec![
                FramePlayer {
                    id: 0,
                    name: "Qrdxxxxxxx".to_string(),
                    library_count: 53,
                    hand_count: 7,
                    life: 20,
                },
                FramePlayer {
                    id: 1,
                    name: "Quxxxxx".to_string(),
                    library_count: 53,
                    hand_count: 7,
                    life: 19,
                },
            ]
        );
        assert_eq!(
            frame.cards,
            vec![FrameCard {
                id: 416,
                catalog_id: 125847,
                zone: "Hand".to_string(),
                actual_zone: "Hand".to_string(),
                owner: 0,
                controller: 0,
            }]
        );
        assert_eq!(frame.player(1).map(|p| p.life), Some(19));
        assert_eq!(frame.player(2), None);
    }

    #[test]
    fn ignores_text_after_the_json() {
        let raw = format!(
            "{}\n  Receiver: something {{\"not\":\"json\"\n",
            line(r#"{"Players":[{"Id":0,"Name":"Qa"}],"Cards":[]}"#)
        );
        let frame = parse_frame(&raw).expect("frame");
        assert_eq!(frame.players.len(), 1);
        assert_eq!(frame.players[0].name, "Qa");
    }

    #[test]
    fn missing_and_null_fields_take_defaults() {
        let raw = line(
            r#"{"Players":[{"Id":1,"Name":null,"HandCount":null}],"Cards":[{"Id":5,"CatalogID":77,"Owner":1}],"Extra":[1,2]}"#,
        );
        let frame = parse_frame(&raw).expect("frame");
        assert_eq!(frame.players[0].name, "");
        assert_eq!(frame.players[0].hand_count, 0);
        assert_eq!(frame.players[0].library_count, 0);
        assert_eq!(frame.cards[0].zone, "");
        assert_eq!(frame.cards[0].catalog_id, 77);

        let empty = parse_frame(&line(r#"{"Players":null}"#)).expect("frame");
        assert_eq!(empty, Frame::default());
    }

    #[test]
    fn key_order_does_not_matter() {
        let raw = line(
            r#"{"Cards":[{"Id":5,"CatalogID":77,"Zone":"Hand","Owner":1}],"Players":[{"Id":1}]}"#,
        );
        let frame = parse_frame(&raw).expect("frame");
        assert_eq!(frame.cards.len(), 1);
        assert_eq!(frame.players.len(), 1);
    }

    #[test]
    fn unreadable_text_gives_none() {
        // Cut short mid-write.
        assert_eq!(parse_frame(&line(r#"{"Players":[{"Id":0,"#)), None);
        // Not a frame line.
        assert_eq!(
            parse_frame(r#"(Twitch Info|Deck Used in Game ID: 1) {"Players":[]}"#),
            None
        );
        // A frame line with no JSON.
        assert_eq!(parse_frame(&line("")), None);
        // Not an object.
        assert_eq!(parse_frame(&line("[1, 2]")), None);
    }

    /// An oddly typed field takes the value 0.x's PHP casts gave it, and the
    /// rest of the frame is still read (#155): `(int)` on a number, a numeric
    /// string's leading digits, a bool; `(string)` on a number.
    #[test]
    fn oddly_typed_fields_are_coerced_as_0x_cast_them() {
        let raw = line(
            r#"{"Players":[{"Id":"1","Name":42,"LibraryCount":53.9,"HandCount":"7 cards","Life":true},{"Id":"zero","Name":false,"Life":[20]}],"Cards":[{"Id":" 416","CatalogID":"125847","Zone":"Hand","Owner":1.0,"Controller":{}},{"Id":5,"CatalogID":-3},{"Id":6,"CatalogID":99999999999}]}"#,
        );
        let frame = parse_frame(&raw).expect("frame");
        assert_eq!(
            frame.players,
            vec![
                FramePlayer {
                    id: 1,
                    name: "42".to_string(),
                    library_count: 53,
                    hand_count: 7,
                    life: 1,
                },
                FramePlayer::default(),
            ]
        );
        assert_eq!(
            frame.cards[0],
            FrameCard {
                id: 416,
                catalog_id: 125847,
                zone: "Hand".to_string(),
                actual_zone: String::new(),
                owner: 1,
                controller: 0,
            }
        );
        assert_eq!(frame.cards[1].catalog_id, 0, "no negative printing");
        assert_eq!(frame.cards[2].catalog_id, 0, "no printing that large");
    }

    #[test]
    fn php_int_reads_a_leading_number() {
        let cases = [
            ("12", 12),
            (" 12", 12),
            ("-5x", -5),
            ("+7", 7),
            ("1.9", 1),
            ("-1.9", -1),
            ("1e3", 1000),
            ("2e", 2),
            ("3.5e1z", 35),
            ("", 0),
            ("+", 0),
            ("x1", 0),
            ("99999999999999999999", i64::MAX),
        ];
        for (text, expected) in cases {
            assert_eq!(php_int(text), expected, "{text:?}");
        }
    }

    /// A list that is not a list is empty, and an entry that is not an
    /// object is skipped: the other entries are still read.
    #[test]
    fn odd_lists_and_entries_are_skipped() {
        let raw = line(r#"{"Players":"none","Cards":[7,{"Id":5,"CatalogID":77},null,"x"]}"#);
        let frame = parse_frame(&raw).expect("frame");
        assert!(frame.players.is_empty());
        assert_eq!(frame.cards.len(), 1);
        assert_eq!(frame.cards[0].catalog_id, 77);
    }

    #[test]
    fn parse_frames_skips_repeats_and_unreadable_frames() {
        let first = line(r#"{"Players":[{"Id":0,"HandCount":0}],"Cards":[]}"#);
        // The same JSON at a later time of day is a repeat.
        let repeat = first.replace("12:26:09", "12:26:10");
        let broken = line(r#"{"Players":[{"#);
        let second = line(r#"{"Players":[{"Id":0,"HandCount":7}],"Cards":[]}"#);
        let back = first.clone();

        let frames = parse_frames(&[&first, &repeat, &broken, &second, &second, &back]);

        let hands: Vec<i64> = frames.iter().map(|f| f.players[0].hand_count).collect();
        assert_eq!(hands, vec![0, 7, 0]);
    }

    #[test]
    fn parse_frames_of_nothing_is_empty() {
        assert!(parse_frames(&[]).is_empty());
    }

    #[test]
    fn a_frame_without_its_log_header_is_read_from_its_json() {
        let json = r#"{"Players":[{"Id":0,"Name":"Qa","HandCount":7}],"Cards":[{"Id":5,"CatalogID":77,"Zone":"Hand","Owner":0}]}"#;
        let frame = parse_frame_content(json).expect("frame");
        assert_eq!(frame.players[0].name, "Qa");
        assert_eq!(frame.cards[0].catalog_id, 77);
        assert_eq!(
            Some(frame),
            parse_frame(&line(json)),
            "the same as with a header"
        );
        // Tolerant as `parse_frame` is: text after the JSON is ignored.
        assert!(parse_frame_content(&format!("{json}\n trailing")).is_some());
        assert_eq!(parse_frame_content(r#"{"Players":[{"Id":0,"#), None);
        assert_eq!(parse_frame_content(""), None);
        assert_eq!(
            parse_frame_content(r#"{"Players":[{"Id":"zero"}]}"#),
            Some(Frame {
                players: vec![FramePlayer::default()],
                cards: Vec::new(),
            }),
            "an oddly typed field is coerced, as 0.x did"
        );
    }

    #[test]
    fn parse_frame_contents_skips_repeats_and_unreadable_frames() {
        let first = r#"{"Players":[{"Id":0,"HandCount":0}],"Cards":[]}"#;
        let broken = r#"{"Players":[{"#;
        let second = r#"{"Players":[{"Id":0,"HandCount":7}],"Cards":[]}"#;
        let frames = parse_frame_contents(&[first, first, broken, second, second, first]);
        let hands: Vec<i64> = frames.iter().map(|f| f.players[0].hand_count).collect();
        assert_eq!(hands, vec![0, 7, 0]);
        assert!(parse_frame_contents(&[]).is_empty());
    }

    #[test]
    fn frame_value_keeps_every_key_and_reads_tolerantly() {
        let raw = format!(
            "{}\n  Receiver: something {{\"not\":\"json\"\n",
            line(
                r#"{"Turn":3,"Players":[{"Id":0,"Name":"Qa","TimeLeft":1200}],"Cards":[{"Id":5,"CatalogID":77,"Tapped":true}]}"#
            )
        );
        assert_eq!(
            frame_value(&raw),
            Some(serde_json::json!({
                "Turn": 3,
                "Players": [{"Id": 0, "Name": "Qa", "TimeLeft": 1200}],
                "Cards": [{"Id": 5, "CatalogID": 77, "Tapped": true}],
            }))
        );
        // Cut short, not a frame line, no JSON.
        assert_eq!(frame_value(&line(r#"{"Players":[{"Id":0,"#)), None);
        assert_eq!(
            frame_value(r#"(Twitch Info|Deck Used in Game ID: 1) {"Players":[]}"#),
            None
        );
        assert_eq!(frame_value(&line("")), None);
        // A bare JSON text reads the same.
        assert_eq!(
            frame_content_value(r#"{"Players":[]} trailing"#),
            Some(serde_json::json!({"Players": []}))
        );
        assert_eq!(frame_content_value("{\"Players\":"), None);
    }

    #[test]
    fn frame_text_is_the_json_after_the_marker() {
        let json = r#"{"Players":[],"Cards":[]}"#;
        assert_eq!(frame_text(&format!("{}\r\n", line(json))), Some(json));
        // The same frame at a later second has the same text.
        let later = line(json).replace("12:26:09", "12:26:10");
        assert_eq!(frame_text(&later), frame_text(&line(json)));
        assert_eq!(frame_text("12:26:09 [INF] (UI|nothing)"), None);
    }

    #[test]
    fn frame_time_is_the_header_time_of_day() {
        assert_eq!(frame_time(&line("{}")), Some("12:26:09".to_string()));
        assert_eq!(
            frame_time(
                "00:00:01 [INF] (Twitch Info|Game Play Status Update for Game ID: 1, Match ID: 2, Event ID: 2) {}"
            ),
            Some("00:00:01".to_string())
        );
        assert_eq!(
            frame_time("Game Play Status Update for Game ID: 1 {}"),
            None
        );
        assert_eq!(frame_time("25:00:00 [INF] (Twitch Info|x) {}"), None);
    }
}
