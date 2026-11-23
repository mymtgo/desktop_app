//! Reading one 0.x match's per-game detail (Plan O, issue #177): each
//! game's seats' `deck_json` (the local seat's deck list, the opponent
//! seat's cards seen), the local seat's `opening_hand_json`, each seat's
//! `starting_hand_size`, its `card_game_stats` rows (`card_stats_read`), its
//! `game_timelines` frames and the match's decoded `game_logs` entries.
//!
//! 0.x keeps about 185 MB of frames, so the import reads and stores one
//! match at a time (`DetailReader::read`), never all of them at once, and
//! drops each frame that repeats the one before it (about 60% of them) by
//! the one shared rule, `replay::drop_repeats` (a game's last frame is
//! always kept). Each kept frame keeps its 0.x time text, normalised by
//! `replay::normalise_time` (replays spec section 10).
//!
//! Values are read loosely, as in `read`: broken JSON in any column skips
//! that piece (a deck list, a frame, a log line, or the whole log), never
//! the match or the import.

use std::path::Path;

use chrono::DateTime;
use replay::{FrameText, drop_repeats, normalise_time};
use rusqlite::{Connection, Transaction};
use serde_json::Value;
use store::imported_detail::{
    DetailCard, DetailLogEntry, ImportedDetail, ImportedGameDetail, ImportedHand,
};

use crate::card_stats_read::{CardStatsColumns, card_stats};
use crate::manual_read::{deck_rows, parse, read_hand};
use crate::read::{ImportError, numbered_unique_games, read_error};
use crate::sql::{
    as_bool, as_integer, as_text, column_list, has_columns, parse_id, parse_utc_ms, value,
};

/// `games` columns selected, in the order `match_games` reads them.
const GAME_COLUMNS: [&str; 2] = ["id", "mtgo_id"];
/// `games` columns that pick and order a match's games.
const GAME_FILTER: [&str; 2] = ["match_id", "started_at"];
/// `game_player` columns selected, in the order `seat_decks` reads them.
const SEAT_COLUMNS: [&str; 2] = ["is_local", "deck_json"];
/// `game_player` hand columns selected after them (NULL when 0.x lacks them).
const SEAT_HAND_COLUMNS: [&str; 2] = ["opening_hand_json", "starting_hand_size"];
/// `game_player` columns that join to the player and pick the game.
const SEAT_FILTER: [&str; 2] = ["player_id", "game_id"];
/// `players` columns: the name selected and the key joined on.
const PLAYER_COLUMNS: [&str; 2] = ["username", "id"];
/// `game_timelines` columns selected: the frame and its time text.
const FRAME_COLUMNS: [&str; 2] = ["content", "timestamp"];
/// `game_timelines` columns that pick and order a game's frames.
const FRAME_FILTER: [&str; 3] = ["game_id", "timestamp", "id"];
/// `game_logs` columns: the entries selected and the match they belong to.
const LOG_COLUMNS: [&str; 2] = ["decoded_entries", "match_token"];

/// Every `(table, columns)` this reader selects or filters on. The SQL is
/// built from the same lists, so the coverage ledger cannot drift from it.
pub const READ_COLUMNS: [(&str, &[&str]); 9] = [
    ("games", &GAME_COLUMNS),
    ("games", &GAME_FILTER),
    ("game_player", &SEAT_COLUMNS),
    ("game_player", &SEAT_HAND_COLUMNS),
    ("game_player", &SEAT_FILTER),
    ("players", &PLAYER_COLUMNS),
    ("game_timelines", &FRAME_COLUMNS),
    ("game_timelines", &FRAME_FILTER),
    ("game_logs", &LOG_COLUMNS),
];

/// Reads 0.x matches' detail one match at a time from a read-only 0.x
/// database. Tables and columns an older 0.x lacks read as empty.
pub struct DetailReader {
    conn: Connection,
    /// `game_player.deck_json` exists.
    has_deck_json: bool,
    /// Which `game_player` hand columns exist.
    has_hand_json: bool,
    has_starting_size: bool,
    /// Which `card_game_stats` columns exist.
    card_stats: CardStatsColumns,
    /// `game_timelines` exists with the columns read.
    has_timelines: bool,
    /// `game_logs` exists with the columns read.
    has_logs: bool,
}

impl DetailReader {
    /// Open the 0.x database at `path` read-only.
    pub fn open(path: &Path) -> Result<DetailReader, ImportError> {
        let conn = crate::read::open(path)?;
        let has_deck_json = has_columns(&conn, "game_player", &["deck_json"])?;
        let has_timelines = has_columns(
            &conn,
            "game_timelines",
            &["id", "game_id", "timestamp", "content"],
        )?;
        let has_logs = has_columns(&conn, "game_logs", &["match_token", "decoded_entries"])?;
        let has_hand_json = has_columns(&conn, "game_player", &["opening_hand_json"])?;
        let has_starting_size = has_columns(&conn, "game_player", &["starting_hand_size"])?;
        let card_stats = CardStatsColumns::read(&conn)?;
        Ok(DetailReader {
            conn,
            has_deck_json,
            has_hand_json,
            has_starting_size,
            card_stats,
            has_timelines,
            has_logs,
        })
    }

    /// The detail of 0.x match `match_id` (its `matches.id`, from
    /// `ReadResult::match_ids`) with this token, read in one transaction so
    /// 0.x can keep writing meanwhile. Games are numbered and a repeated
    /// Game ID dropped as `read_database` does. `local_username` is the
    /// match's local player (`ImportedMatch::local_username`): its seat is
    /// the local one when 0.x marked none. Empty when 0.x has nothing.
    pub fn read(
        &mut self,
        match_id: i64,
        token: &str,
        local_username: &str,
    ) -> Result<ImportedDetail, ImportError> {
        let has_deck_json = self.has_deck_json;
        let has_timelines = self.has_timelines;
        let has_logs = self.has_logs;
        // The reader lives for the whole import: the time limit is per match.
        crate::read::set_time_limit(&self.conn, crate::read::READ_TIME_LIMIT)
            .map_err(read_error)?;
        let tx = self.conn.transaction().map_err(read_error)?;
        let mut games = Vec::new();
        let (numbered, _) = numbered_unique_games(match_games(&tx, match_id)?);
        for kept in numbered {
            let game_id = kept.game;
            let seats = seat_details(
                &tx,
                game_id,
                local_username,
                has_deck_json,
                self.has_hand_json,
                self.has_starting_size,
            )?;
            let stats = card_stats(&tx, &self.card_stats, game_id)?;
            let frames = if has_timelines {
                frames(&tx, game_id)?
            } else {
                Vec::new()
            };
            games.push(ImportedGameDetail {
                mtgo_game_id: kept.mtgo_game_id.and_then(|id| u64::try_from(id).ok()),
                game_number: kept.number,
                local_deck: seats.local_deck,
                opponent_cards: seats.opponent_cards,
                frames,
                local_hand: seats.local_hand,
                local_starting_hand_size: seats.local_starting_hand_size,
                opp_starting_hand_size: seats.opp_starting_hand_size,
                card_stats: stats,
            });
        }
        let log = if has_logs {
            log(&tx, token)?
        } else {
            Vec::new()
        };
        // Read only: dropping the transaction ends it.
        drop(tx);
        Ok(ImportedDetail { games, log })
    }
}

/// The match's games as `(Game ID, id)`, in game order as `read_database`
/// orders them (start time, then id), ready for `numbered_unique_games`.
pub(crate) fn match_games(
    tx: &Transaction,
    match_id: i64,
) -> Result<Vec<(Option<i64>, i64)>, ImportError> {
    let sql = format!(
        "SELECT {} FROM games WHERE \"{}\" = ?1 ORDER BY \"{}\", id",
        column_list("", &GAME_COLUMNS),
        GAME_FILTER[0],
        GAME_FILTER[1]
    );
    let mut stmt = tx.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([match_id], |row| {
            let game_id = row.get::<_, i64>(0)?;
            let mtgo_id = as_text(value(row, 1)?).as_deref().and_then(parse_id);
            Ok((mtgo_id, game_id))
        })
        .map_err(read_error)?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(read_error)?);
    }
    Ok(games)
}

/// What one game's seats hold.
#[derive(Debug, Default)]
struct SeatDetail {
    local_deck: Vec<DetailCard>,
    opponent_cards: Vec<DetailCard>,
    local_hand: Option<ImportedHand>,
    local_starting_hand_size: Option<u32>,
    opp_starting_hand_size: Option<u32>,
}

/// One seat's columns.
struct SeatRow {
    is_local: bool,
    deck: Option<String>,
    hand: Option<String>,
    starting_size: Option<i64>,
}

/// The local seat's deck list, recorded hand and starting hand size, and
/// the opponent seat's cards seen and starting hand size. Of the seats with
/// a named player, the local one is the first marked local, else the first
/// with `local_username` (issue #190: a fallback account can sit on a seat
/// 0.x did not mark); the opponent is the first other seat whose name is
/// not `local_username`. Columns this 0.x lacks read as nothing.
fn seat_details(
    tx: &Transaction,
    game_id: i64,
    local_username: &str,
    has_deck_json: bool,
    has_hand_json: bool,
    has_starting_size: bool,
) -> Result<SeatDetail, ImportError> {
    let column = |present: bool, name: &str| {
        if present {
            format!("gp.\"{name}\"")
        } else {
            "NULL".to_string()
        }
    };
    let sql = format!(
        "SELECT gp.\"{}\", {}, {}, {}, p.\"{}\"
         FROM game_player gp JOIN players p ON p.\"{}\" = gp.\"{}\"
         WHERE gp.\"{}\" = ?1 ORDER BY gp.rowid",
        SEAT_COLUMNS[0],
        column(has_deck_json, SEAT_COLUMNS[1]),
        column(has_hand_json, SEAT_HAND_COLUMNS[0]),
        column(has_starting_size, SEAT_HAND_COLUMNS[1]),
        PLAYER_COLUMNS[0],
        PLAYER_COLUMNS[1],
        SEAT_FILTER[0],
        SEAT_FILTER[1]
    );
    let mut stmt = tx.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([game_id], |row| {
            let seat = SeatRow {
                is_local: as_bool(value(row, 0)?).unwrap_or(false),
                deck: as_text(value(row, 1)?),
                hand: as_text(value(row, 2)?),
                starting_size: as_integer(value(row, 3)?),
            };
            let username = as_text(value(row, 4)?).unwrap_or_default();
            Ok((seat, username))
        })
        .map_err(read_error)?;
    let mut named: Vec<(SeatRow, String)> = Vec::new();
    for row in rows {
        let (seat, username) = row.map_err(read_error)?;
        if !username.trim().is_empty() {
            named.push((seat, username));
        }
    }
    let local_index = named
        .iter()
        .position(|(seat, _)| seat.is_local)
        .or_else(|| named.iter().position(|(_, name)| name == local_username));
    let mut local: Option<SeatRow> = None;
    let mut opponent: Option<SeatRow> = None;
    for (index, (seat, username)) in named.into_iter().enumerate() {
        if Some(index) == local_index {
            local = Some(seat);
        } else if opponent.is_none() && !seat.is_local && username != local_username {
            opponent = Some(seat);
        }
    }
    let size = |seat: &SeatRow| seat.starting_size.and_then(|size| u32::try_from(size).ok());
    let mut detail = SeatDetail::default();
    if let Some(seat) = &local {
        detail.local_deck = seat.deck.as_deref().map(cards).unwrap_or_default();
        detail.local_hand = seat
            .hand
            .as_deref()
            .and_then(read_hand)
            .map(|hand| ImportedHand {
                starting: hand.kept,
                bottomed: hand.bottomed,
                mulligans: hand.mulligans,
            });
        detail.local_starting_hand_size = size(seat);
    }
    if let Some(seat) = &opponent {
        detail.opponent_cards = seat.deck.as_deref().map(cards).unwrap_or_default();
        detail.opp_starting_hand_size = size(seat);
    }
    Ok(detail)
}

/// The readable rows of a `deck_json`, in stored order. A row without a
/// quantity is one copy, as 0.x showed it; rows without a copy are left out.
pub(crate) fn cards(text: &str) -> Vec<DetailCard> {
    let mut found = Vec::new();
    for (mtgo_id, quantity, sideboard) in deck_rows(text, 1) {
        if let Ok(quantity) = u32::try_from(quantity)
            && quantity > 0
        {
            found.push(DetailCard {
                mtgo_id,
                quantity,
                sideboard,
            });
        }
    }
    found
}

/// The game's readable frames by time, then id, each with its time text
/// (normalised), repeats dropped by `replay::drop_repeats`. One game's rows
/// at a time.
fn frames(tx: &Transaction, game_id: i64) -> Result<Vec<FrameText>, ImportError> {
    let sql = format!(
        "SELECT {} FROM game_timelines WHERE \"{}\" = ?1 ORDER BY \"{}\", \"{}\"",
        column_list("", &FRAME_COLUMNS),
        FRAME_FILTER[0],
        FRAME_FILTER[1],
        FRAME_FILTER[2]
    );
    let mut stmt = tx.prepare(&sql).map_err(read_error)?;
    let mut rows = stmt.query([game_id]).map_err(read_error)?;
    let mut frames: Vec<FrameText> = Vec::new();
    while let Some(row) = rows.next().map_err(read_error)? {
        let Some(content) = as_text(value(row, 0).map_err(read_error)?) else {
            continue;
        };
        let Some(text) = frame_text(content) else {
            continue;
        };
        let time = as_text(value(row, 1).map_err(read_error)?).unwrap_or_default();
        frames.push(FrameText {
            time: normalise_time(&time),
            text,
        });
    }
    Ok(drop_repeats(frames))
}

/// A frame's JSON text when it is a JSON object; a JSON string holding one
/// (text written twice) is read through. Anything else is not a frame.
fn frame_text(content: String) -> Option<String> {
    match serde_json::from_str::<Value>(&content).ok()? {
        Value::Object(_) => Some(content),
        Value::String(inner) => {
            let is_object = serde_json::from_str::<Value>(&inner)
                .ok()
                .is_some_and(|value| value.is_object());
            is_object.then_some(inner)
        }
        _ => None,
    }
}

/// The match's decoded game log: from the longest `game_logs` row with a
/// readable line (0.x kept the longest of duplicates), each line with a
/// readable time and message, in stored order.
fn log(tx: &Transaction, token: &str) -> Result<Vec<DetailLogEntry>, ImportError> {
    let [entries, match_token] = LOG_COLUMNS;
    let sql = format!(
        "SELECT \"{entries}\" FROM game_logs
         WHERE \"{match_token}\" = ?1 AND \"{entries}\" IS NOT NULL
         ORDER BY LENGTH(\"{entries}\") DESC, rowid"
    );
    let mut stmt = tx.prepare(&sql).map_err(read_error)?;
    let mut rows = stmt.query([token]).map_err(read_error)?;
    while let Some(row) = rows.next().map_err(read_error)? {
        let Some(text) = as_text(value(row, 0).map_err(read_error)?) else {
            continue;
        };
        // A row with no readable line is no log (issue #190): try the next.
        if let Some(entries) = log_entries(&text)
            && !entries.is_empty()
        {
            return Ok(entries);
        }
    }
    Ok(Vec::new())
}

/// The lines of one `decoded_entries`, or `None` when it is not a JSON list.
fn log_entries(text: &str) -> Option<Vec<DetailLogEntry>> {
    let parsed = parse(text)?;
    let mut entries = Vec::new();
    for entry in parsed.as_array()? {
        let at_ms = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(log_time_ms);
        let message = entry.get("message").and_then(Value::as_str);
        if let (Some(at_ms), Some(message)) = (at_ms, message) {
            entries.push(DetailLogEntry {
                at_ms,
                message: message.to_string(),
            });
        }
    }
    Some(entries)
}

/// An ISO 8601 time with an offset (`2026-01-01T16:36:49+00:00`) as UTC
/// unix ms; one without an offset counts as UTC.
fn log_time_ms(text: &str) -> Option<i64> {
    match DateTime::parse_from_rfc3339(text.trim()) {
        Ok(time) => Some(time.timestamp_millis()),
        Err(_) => parse_utc_ms(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_times_with_an_offset_become_utc() {
        assert_eq!(log_time_ms("1970-01-01T01:00:01+01:00"), Some(1_000));
        assert_eq!(log_time_ms("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(log_time_ms("1970-01-01 00:00:02"), Some(2_000));
        assert_eq!(log_time_ms("yesterday"), None);
    }

    #[test]
    fn only_json_objects_are_frames() {
        assert_eq!(frame_text("{}".to_string()), Some("{}".to_string()));
        assert_eq!(
            frame_text(r#""{\"Cards\":[]}""#.to_string()),
            Some(r#"{"Cards":[]}"#.to_string())
        );
        assert_eq!(frame_text("[]".to_string()), None);
        assert_eq!(frame_text(r#""[]""#.to_string()), None);
        assert_eq!(frame_text("{".to_string()), None);
    }
}
