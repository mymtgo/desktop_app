//! Frames for the replay viewer: card and player decoration (0.x
//! `BuildReplayFrames`, shared with the snapshot builder) and the repeated
//! frame rule (replays spec section 4, ruling 4).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::repair::repair_active_player;
use crate::snapshot::{CardInfo, ImageMode, ReplayFrame};

/// 0.x `BuildReplayFrames::run`: a game's frames as the app's replay window
/// consumes them. Every player gets `IsLocal` (its `Name` equals the local
/// username, case-sensitive); every card gets `name`, `type` and `image`
/// from the catalog (MTGO's own `Name` for a card the catalog has no row for
/// and whose identity is known, `CatalogID > 0`), and MTGO's `Name` is
/// removed. Then `repair_active_player`; never the midnight repair (0.x
/// applies none in the app).
///
/// Images: `App` keeps the stored remote URL and adds `local_image` (the
/// stored file name) when the card has one; the window turns it into a
/// `card` scheme URL. `Portable` keeps only an `https://` remote URL.
pub fn decorate_frames(
    frames: Vec<ReplayFrame>,
    local_username: &str,
    cards: &BTreeMap<u32, CardInfo>,
    mode: ImageMode,
) -> Vec<ReplayFrame> {
    let mut frames = frames;
    for frame in &mut frames {
        decorate_content(&mut frame.content, Some(local_username), cards, mode);
    }
    repair_active_player(frames)
}

/// Decorates one frame's content in place (players and cards). Shared by
/// `decorate_frames` and the snapshot builder, which uses `Portable`.
pub(crate) fn decorate_content(
    content: &mut Value,
    local_username: Option<&str>,
    cards: &BTreeMap<u32, CardInfo>,
    mode: ImageMode,
) {
    if let Some(players) = content.get_mut("Players").and_then(Value::as_array_mut) {
        for player in players {
            let is_local = match (local_username, player.get("Name").and_then(Value::as_str)) {
                (Some(local), Some(name)) => local == name,
                _ => false,
            };
            if let Some(player) = player.as_object_mut() {
                player.insert("IsLocal".to_string(), Value::Bool(is_local));
            }
        }
    }

    if let Some(list) = content.get_mut("Cards").and_then(Value::as_array_mut) {
        for card in list {
            decorate_card(card, cards, mode);
        }
    }
}

fn decorate_card(card: &mut Value, cards: &BTreeMap<u32, CardInfo>, mode: ImageMode) {
    let known = catalog_id(card).and_then(|id| cards.get(&id));
    let mtgo = mtgo_name(card);
    let Some(card) = card.as_object_mut() else {
        return;
    };

    let image = match (known, mode) {
        (Some(known), ImageMode::App) => known.image.clone(),
        (Some(known), ImageMode::Portable) => https_only(known.image.as_deref()),
        (None, _) => None,
    };
    let name = known.and_then(|known| known.name.clone()).or(mtgo);
    let type_line = known.and_then(|known| known.type_line.clone());

    card.insert("image".to_string(), optional_string(image));
    card.insert("type".to_string(), optional_string(type_line));
    card.insert("name".to_string(), optional_string(name));
    card.remove("Name");

    if mode == ImageMode::App
        && let Some(local_image) = known.and_then(|known| known.local_image.clone())
    {
        card.insert("local_image".to_string(), Value::String(local_image));
    }

    // The other side of a double-faced card, for the preview. Set only when
    // known, so single-faced cards cost nothing.
    if let Some(other_image) = known.and_then(|known| https_only(known.other_image.as_deref())) {
        card.insert("other_image".to_string(), Value::String(other_image));
    }
}

/// A card's `CatalogID` as a catalog key.
pub(crate) fn catalog_id(card: &Value) -> Option<u32> {
    let id = card.get("CatalogID")?.as_u64()?;
    u32::try_from(id).ok()
}

/// 0.x and package `mtgoName`: the name MTGO itself gave a card, for ids the
/// catalog cannot resolve (most tokens). Only for a card whose identity is
/// known: a hidden card has catalog id 0 and must stay nameless.
fn mtgo_name(card: &Value) -> Option<String> {
    let name = card.get("Name").and_then(Value::as_str)?;
    if php_int(card.get("CatalogID")) > 0 && !name.is_empty() {
        Some(name.to_string())
    } else {
        None
    }
}

/// PHP's `(int) ($value ?? 0)` for the JSON values a frame can hold.
fn php_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64))
            .unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(0),
        Some(Value::Bool(true)) => 1,
        _ => 0,
    }
}

/// The URL when it is a remote `https://` one, else `None`.
pub(crate) fn https_only(url: Option<&str>) -> Option<String> {
    match url {
        Some(url) if url.starts_with("https://") => Some(url.to_string()),
        _ => None,
    }
}

fn optional_string(value: Option<String>) -> Value {
    match value {
        Some(text) => Value::String(text),
        None => Value::Null,
    }
}

/// A stored frame before decoding: its time text (`HH:MM:SS`) and its JSON
/// text exactly as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameText {
    pub time: String,
    pub text: String,
}

/// One game's frames with consecutive repeats dropped (replays spec ruling
/// 4): a frame whose JSON text equals the frame kept just before it is
/// dropped, so a run keeps its first frame and that frame's time. The game's
/// last frame is always kept, even when it repeats, so the timeline keeps
/// its length and every kept frame its scrubber position.
pub fn drop_repeats(frames: Vec<FrameText>) -> Vec<FrameText> {
    let count = frames.len();
    let mut kept: Vec<FrameText> = Vec::with_capacity(count);

    for (index, frame) in frames.into_iter().enumerate() {
        let is_last = index + 1 == count;
        let repeats = kept.last().is_some_and(|before| before.text == frame.text);
        if !repeats || is_last {
            kept.push(frame);
        }
    }

    kept
}

/// A 0.x frame time as v1 stores it (replays spec section 10, ruling 5).
/// `HH:MM:SS` and `HH:MM:SS.mmm` (a time of day, as MTGO's log header and
/// 0.x's dense recorder wrote them) are kept as they are. An older 0.x row's
/// `Y-m-d H:i:s` keeps its time part (with its fraction when it has one):
/// the viewer reads a time of day and pairs log lines with frames by their
/// first eight characters. Any other text (an ISO 8601 instant of a pulled
/// bundle among them) is kept as written, trimmed.
pub fn normalise_time(text: &str) -> String {
    let trimmed = text.trim();
    if is_time_of_day(trimmed) {
        return trimmed.to_string();
    }
    if let Some(time) = date_time_part(trimmed) {
        return time;
    }
    trimmed.to_string()
}

/// Whether `text` is `HH:MM:SS` with an optional `.` and digits.
pub fn is_time_of_day(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 8 {
        return false;
    }
    let clock = &bytes[..8];
    let clock_ok = [0, 1, 3, 4, 6, 7]
        .iter()
        .all(|&at| clock[at].is_ascii_digit())
        && clock[2] == b':'
        && clock[5] == b':';
    if !clock_ok {
        return false;
    }
    let rest = &bytes[8..];
    if rest.is_empty() {
        return true;
    }
    rest.len() > 1 && rest[0] == b'.' && rest[1..].iter().all(u8::is_ascii_digit)
}

/// The time part of `YYYY-MM-DD HH:MM:SS[.f]`; `None` for any other shape.
fn date_time_part(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || !text.is_char_boundary(11) {
        return None;
    }
    let date_ok = [0, 1, 2, 3, 5, 6, 8, 9]
        .iter()
        .all(|&at| bytes[at].is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' ';
    let time = &text[11..];
    (date_ok && is_time_of_day(time)).then(|| time.to_string())
}
