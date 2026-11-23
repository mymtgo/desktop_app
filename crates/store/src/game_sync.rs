//! Per-game card rows (target schema section 6): `game_cards`,
//! `game_card_casts`, `game_mulligan_cards` and the hand sizes on `games`;
//! each game's state frames of an imported game (`game_timelines`); a
//! match's decoded `.dat` game log (`game_logs`).
//!
//! The game sync writes a log match's rows from its stored events, its
//! `.dat` game log (decoded once into `game_logs`) and the card table
//! (`crates/game-detail` `game_card_rows`). It writes only rows that changed,
//! so syncing after every event equals one sync at the end, and a second
//! sync writes nothing. It never touches a match that is not `log`: a manual
//! match's rows are the player's, an imported one's the import's.

use std::collections::{BTreeMap, HashMap};

use chrono_tz::Tz;
use decks::DeckCard;
use game_detail::{
    Frame, GameCardsInput, GameLogEntry, LogCard, MetaEvent, extract_game_results,
    extract_log_cards, game_card_rows, meta_message_entries, parse_frame_contents, parse_frames,
    parse_game_log, played_deck,
};
use mtgo_protocol::{EventKind, parse_players, prepare_at};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::cards::{CardRef, NameFix, ensure_id_stubs, ensure_stubs, replace_meta_names};
use crate::decks::{CardRow, card_infos};
pub use replay::FrameText;

use crate::platform::account_of_mtgo;
use crate::{Connection, StoreError};
pub use game_detail::{GAME_CARDS_VERSION, GameCardCast, GameCardRow, GameCards, Side};

/// The zstd level, as `compact` uses.
const ZSTD_LEVEL: i32 = 3;

/// The derivation name the backfill records (`derivations.name`).
pub const GAME_CARDS_DERIVATION: &str = "game_cards";

// ---- Card rows ---------------------------------------------------------------

/// A game's four hand sizes: local opening and starting, then the
/// opponent's.
type HandSizes = (Option<u32>, Option<u32>, Option<u32>, Option<u32>);

/// A game the sync reads: `(id, Game ID, on the play)`.
type SyncGame = (i64, Option<u64>, Option<bool>);

/// A game's stored rows: its hand sizes, card rows, casts and mulligans.
pub fn read_game_cards(conn: &Connection, game_id: i64) -> Result<GameCards, StoreError> {
    let mut cards = GameCards::default();
    let sizes: Option<HandSizes> = conn
        .prepare_cached(
            "SELECT local_opening_hand_size, local_starting_hand_size, opp_opening_hand_size,
                 opp_starting_hand_size FROM games WHERE id = ?1",
        )?
        .query_row([game_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    if let Some((opening, starting, opp_opening, opp_starting)) = sizes {
        cards.opening_hand_size = opening;
        cards.starting_hand_size = starting;
        cards.opp_opening_hand_size = opp_opening;
        cards.opp_starting_hand_size = opp_starting;
    }

    let mut stmt = conn.prepare_cached(
        "SELECT side, catalog_id, main_qty, side_qty, sided_in, sided_out, opening_hand, bottomed,
             starting_hand, first_draw, seen, pregame_revealed, pregame_played, `cast`, played,
             activated, discarded, cast_turn, hand_seen, graveyard_seen, exile_seen,
             battlefield_seen, has_zone_data, inferred
         FROM game_cards WHERE game_id = ?1 ORDER BY side, catalog_id",
    )?;
    let rows = stmt.query_map([game_id], |row| {
        let side: String = row.get(0)?;
        Ok(GameCardRow {
            side: Side::parse(&side).unwrap_or_default(),
            catalog_id: row.get(1)?,
            main_qty: row.get(2)?,
            side_qty: row.get(3)?,
            sided_in: row.get(4)?,
            sided_out: row.get(5)?,
            opening_hand: row.get(6)?,
            bottomed: row.get(7)?,
            starting_hand: row.get(8)?,
            first_draw: row.get(9)?,
            seen: row.get(10)?,
            pregame_revealed: row.get(11)?,
            pregame_played: row.get(12)?,
            cast: row.get(13)?,
            played: row.get(14)?,
            activated: row.get(15)?,
            discarded: row.get(16)?,
            cast_turn: row.get(17)?,
            hand_seen: row.get(18)?,
            graveyard_seen: row.get(19)?,
            exile_seen: row.get(20)?,
            battlefield_seen: row.get(21)?,
            has_zone_data: row.get::<_, i64>(22)? == 1,
            inferred: row.get::<_, i64>(23)? == 1,
        })
    })?;
    for row in rows {
        cards.cards.push(row?);
    }

    let mut stmt = conn.prepare_cached(
        "SELECT side, catalog_id, method, count FROM game_card_casts
         WHERE game_id = ?1 ORDER BY side, catalog_id, method",
    )?;
    let rows = stmt.query_map([game_id], |row| {
        let side: String = row.get(0)?;
        Ok(GameCardCast {
            side: Side::parse(&side).unwrap_or_default(),
            catalog_id: row.get(1)?,
            method: row.get(2)?,
            count: row.get(3)?,
        })
    })?;
    for row in rows {
        cards.casts.push(row?);
    }

    let mut stmt = conn.prepare_cached(
        "SELECT hand, catalog_id, quantity FROM game_mulligan_cards
         WHERE game_id = ?1 ORDER BY hand, catalog_id",
    )?;
    let rows = stmt.query_map([game_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    for row in rows {
        cards.mulligans.push(row?);
    }
    Ok(cards)
}

/// Each game's stored rows of a match, by game id.
pub fn match_game_cards(
    conn: &Connection,
    match_id: i64,
) -> Result<HashMap<i64, GameCards>, StoreError> {
    let mut ids = Vec::new();
    {
        let mut stmt = conn.prepare_cached("SELECT id FROM games WHERE match_id = ?1")?;
        let rows = stmt.query_map([match_id], |row| row.get::<_, i64>(0))?;
        for row in rows {
            ids.push(row?);
        }
    }
    let mut found = HashMap::new();
    for id in ids {
        found.insert(id, read_game_cards(conn, id)?);
    }
    Ok(found)
}

/// Write a game's rows, changing only what differs from the stored ones.
/// Returns whether anything was written.
pub fn write_game_cards(
    tx: &Transaction,
    game_id: i64,
    cards: &GameCards,
) -> Result<bool, StoreError> {
    let stored = read_game_cards(tx, game_id)?;
    if stored == *cards {
        return Ok(false);
    }
    // Every id the rows hold gets a `cards` row (a manual match or an
    // imported lay-over can name ids no source did).
    let mut ids: Vec<u32> = cards.cards.iter().map(|row| row.catalog_id).collect();
    ids.extend(cards.mulligans.iter().map(|(_, catalog_id, _)| *catalog_id));
    ids.sort_unstable();
    ids.dedup();
    ensure_id_stubs(tx, &ids)?;
    if (
        stored.opening_hand_size,
        stored.starting_hand_size,
        stored.opp_opening_hand_size,
        stored.opp_starting_hand_size,
    ) != (
        cards.opening_hand_size,
        cards.starting_hand_size,
        cards.opp_opening_hand_size,
        cards.opp_starting_hand_size,
    ) {
        tx.prepare_cached(
            "UPDATE games SET local_opening_hand_size = ?2, local_starting_hand_size = ?3,
                 opp_opening_hand_size = ?4, opp_starting_hand_size = ?5
             WHERE id = ?1",
        )?
        .execute(params![
            game_id,
            cards.opening_hand_size,
            cards.starting_hand_size,
            cards.opp_opening_hand_size,
            cards.opp_starting_hand_size
        ])?;
    }

    // Casts that went or changed first: they reference the card rows.
    for cast in &stored.casts {
        if !cards.casts.contains(cast) {
            tx.prepare_cached(
                "DELETE FROM game_card_casts
                 WHERE game_id = ?1 AND side = ?2 AND catalog_id = ?3 AND method = ?4",
            )?
            .execute(params![
                game_id,
                cast.side.as_str(),
                cast.catalog_id,
                cast.method
            ])?;
        }
    }
    for row in &stored.cards {
        if cards.row(row.side, row.catalog_id).is_none() {
            tx.prepare_cached(
                "DELETE FROM game_cards WHERE game_id = ?1 AND side = ?2 AND catalog_id = ?3",
            )?
            .execute(params![game_id, row.side.as_str(), row.catalog_id])?;
        }
    }
    for row in &cards.cards {
        if stored.row(row.side, row.catalog_id) == Some(row) {
            continue;
        }
        upsert_card_row(tx, game_id, row)?;
    }
    for cast in &cards.casts {
        if stored.casts.contains(cast) {
            continue;
        }
        tx.prepare_cached(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            game_id,
            cast.side.as_str(),
            cast.catalog_id,
            cast.method,
            cast.count
        ])?;
    }

    for mulligan in &stored.mulligans {
        if !cards.mulligans.contains(mulligan) {
            let (hand, catalog_id, _) = *mulligan;
            tx.prepare_cached(
                "DELETE FROM game_mulligan_cards WHERE game_id = ?1 AND hand = ?2 AND catalog_id = ?3",
            )?
            .execute(params![game_id, hand, catalog_id])?;
        }
    }
    for mulligan in &cards.mulligans {
        if stored.mulligans.contains(mulligan) {
            continue;
        }
        let (hand, catalog_id, quantity) = *mulligan;
        tx.prepare_cached(
            "INSERT INTO game_mulligan_cards (game_id, hand, catalog_id, quantity)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![game_id, hand, catalog_id, quantity])?;
    }
    Ok(true)
}

fn upsert_card_row(tx: &Transaction, game_id: i64, row: &GameCardRow) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty, sided_in,
             sided_out, opening_hand, bottomed, starting_hand, first_draw, seen, pregame_revealed,
             pregame_played, `cast`, played, activated, discarded, cast_turn, hand_seen,
             graveyard_seen, exile_seen, battlefield_seen, has_zone_data, inferred)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
             ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
         ON CONFLICT (game_id, side, catalog_id) DO UPDATE SET
             main_qty = excluded.main_qty, side_qty = excluded.side_qty,
             sided_in = excluded.sided_in, sided_out = excluded.sided_out,
             opening_hand = excluded.opening_hand, bottomed = excluded.bottomed,
             starting_hand = excluded.starting_hand, first_draw = excluded.first_draw,
             seen = excluded.seen, pregame_revealed = excluded.pregame_revealed,
             pregame_played = excluded.pregame_played, `cast` = excluded.`cast`,
             played = excluded.played, activated = excluded.activated,
             discarded = excluded.discarded, cast_turn = excluded.cast_turn,
             hand_seen = excluded.hand_seen, graveyard_seen = excluded.graveyard_seen,
             exile_seen = excluded.exile_seen, battlefield_seen = excluded.battlefield_seen,
             has_zone_data = excluded.has_zone_data, inferred = excluded.inferred",
    )?
    .execute(params![
        game_id,
        row.side.as_str(),
        row.catalog_id,
        row.main_qty,
        row.side_qty,
        row.sided_in,
        row.sided_out,
        row.opening_hand,
        row.bottomed,
        row.starting_hand,
        row.first_draw,
        row.seen,
        row.pregame_revealed,
        row.pregame_played,
        row.cast,
        row.played,
        row.activated,
        row.discarded,
        row.cast_turn,
        row.hand_seen,
        row.graveyard_seen,
        row.exile_seen,
        row.battlefield_seen,
        i64::from(row.has_zone_data),
        i64::from(row.inferred),
    ])?;
    Ok(())
}

// ---- Game logs and timelines -------------------------------------------------

/// One decoded game log entry as `game_logs` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredEntry {
    /// UTC unix ms.
    at_ms: i64,
    message: String,
}

/// Store a match's decoded game log, unless the same one is stored.
/// Returns whether it was written.
pub fn store_game_log(
    tx: &Transaction,
    match_id: i64,
    entries: &[GameLogEntry],
) -> Result<bool, StoreError> {
    if game_log(tx, match_id)?.as_deref() == Some(entries) {
        return Ok(false);
    }
    let stored: Vec<StoredEntry> = entries
        .iter()
        .map(|entry| StoredEntry {
            at_ms: entry.timestamp_ms,
            message: entry.message.clone(),
        })
        .collect();
    let raw = zstd::encode_all(serde_json::to_vec(&stored)?.as_slice(), ZSTD_LEVEL)?;
    tx.prepare_cached(
        "INSERT INTO game_logs (match_id, decoded_entries, entry_count, decoded_at)
         VALUES (?1, ?2, ?3, CAST(unixepoch('subsec') * 1000 AS INTEGER))
         ON CONFLICT (match_id) DO UPDATE SET decoded_entries = excluded.decoded_entries,
             entry_count = excluded.entry_count, decoded_at = excluded.decoded_at",
    )?
    .execute(params![match_id, raw, entries.len() as i64])?;
    Ok(true)
}

/// A match's stored game log, `None` without one. A stored log that cannot
/// be read is `Corrupt`.
pub fn game_log(conn: &Connection, match_id: i64) -> Result<Option<Vec<GameLogEntry>>, StoreError> {
    let raw: Option<Vec<u8>> = conn
        .prepare_cached("SELECT decoded_entries FROM game_logs WHERE match_id = ?1")?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let json = zstd::decode_all(raw.as_slice())?;
    let stored: Vec<StoredEntry> = serde_json::from_slice(&json)?;
    Ok(Some(
        stored
            .into_iter()
            .map(|entry| GameLogEntry {
                timestamp_ms: entry.at_ms,
                message: entry.message,
            })
            .collect(),
    ))
}

/// Whether the match `token` has a stored game log.
pub fn has_game_log(conn: &Connection, token: &str) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM game_logs l JOIN matches m ON m.id = l.match_id
                            WHERE m.token = ?1)",
        )?
        .query_row([token], |row| row.get(0))?)
}

/// Store an imported game's frames (JSON texts with their 0.x time texts),
/// unless the same ones are stored; no frames delete the row. Returns
/// whether anything changed.
pub fn store_timeline(
    tx: &Transaction,
    game_id: i64,
    frames: &[FrameText],
) -> Result<bool, StoreError> {
    let stored = timeline(tx, game_id)?;
    if frames.is_empty() {
        if stored.is_none() {
            return Ok(false);
        }
        tx.prepare_cached("DELETE FROM game_timelines WHERE game_id = ?1")?
            .execute([game_id])?;
        return Ok(true);
    }
    if stored.as_deref() == Some(frames) {
        return Ok(false);
    }
    let texts: Vec<&str> = frames.iter().map(|frame| frame.text.as_str()).collect();
    let times: Vec<&str> = frames.iter().map(|frame| frame.time.as_str()).collect();
    let raw = zstd::encode_all(serde_json::to_vec(&texts)?.as_slice(), ZSTD_LEVEL)?;
    let times = zstd::encode_all(serde_json::to_vec(&times)?.as_slice(), ZSTD_LEVEL)?;
    tx.prepare_cached(
        "INSERT INTO game_timelines (game_id, raw, times, frame_count) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (game_id) DO UPDATE SET raw = excluded.raw, times = excluded.times,
             frame_count = excluded.frame_count",
    )?
    .execute(params![game_id, raw, times, frames.len() as i64])?;
    Ok(true)
}

/// An imported game's stored frames, each with its time text, `None`
/// without any. A frame without a stored time (a `times` list shorter than
/// `raw`, which v1 never writes) has an empty one.
pub fn timeline(conn: &Connection, game_id: i64) -> Result<Option<Vec<FrameText>>, StoreError> {
    let found: Option<(Vec<u8>, Vec<u8>)> = conn
        .prepare_cached("SELECT raw, times FROM game_timelines WHERE game_id = ?1")?
        .query_row([game_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((raw, times)) = found else {
        return Ok(None);
    };
    let texts: Vec<String> = serde_json::from_slice(&zstd::decode_all(raw.as_slice())?)?;
    let mut times: Vec<String> = serde_json::from_slice(&zstd::decode_all(times.as_slice())?)?;
    times.resize(texts.len(), String::new());
    let mut frames = Vec::with_capacity(texts.len());
    for (text, time) in texts.into_iter().zip(times) {
        frames.push(FrameText { time, text });
    }
    Ok(Some(frames))
}

/// Delete everything a 0.x or history copy's games and log hold: a live
/// match taking its token over brings its own (live data wins).
pub(crate) fn clear_match_detail(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    for sql in [
        "DELETE FROM game_cards WHERE game_id IN (SELECT id FROM games WHERE match_id = ?1)",
        "DELETE FROM game_mulligan_cards WHERE game_id IN (SELECT id FROM games WHERE match_id = ?1)",
        "DELETE FROM game_timelines WHERE game_id IN (SELECT id FROM games WHERE match_id = ?1)",
        "UPDATE games SET local_opening_hand_size = NULL, local_starting_hand_size = NULL,
             opp_opening_hand_size = NULL, opp_starting_hand_size = NULL WHERE match_id = ?1",
        "DELETE FROM game_logs WHERE match_id = ?1",
    ] {
        tx.prepare_cached(sql)?.execute([match_id])?;
    }
    Ok(())
}

// ---- Building the rows -------------------------------------------------------

/// One game of a match as the rows are built from it.
#[derive(Debug, Clone, Default)]
pub struct GameSource {
    pub game_id: i64,
    pub on_play: Option<bool>,
    /// The state frames, in log order.
    pub frames: Vec<Frame>,
    /// The local deck as played (main and side rows); empty when unknown.
    pub local_deck: Vec<DeckCard>,
    /// The opponent's cards a 0.x import recorded.
    pub recorded_opponent_cards: Vec<(u32, u32)>,
}

/// Each game's rows of a match (spec section 6, "Writers"), in game order.
/// `reveal_entries` is the `.dat` game log (log-only reveals, as 0.x took
/// them from the file only); `counter_entries` the log the counters, the
/// pregame actions and the log's starting hand sizes come from (the `.dat`
/// log, else the MetaMessages). Games pair with the log's games by position.
///
/// First it writes a `cards` stub (`store::cards`) for every card the games
/// and logs name: the deck as played, every frame card (tokens), the
/// recorded opponent cards and every game log card with the log's name. The
/// names are read after, so a sync after each event and one at the end read
/// the same card table.
pub fn build_game_cards(
    tx: &Transaction,
    local: &str,
    opponent: Option<&str>,
    games: &[GameSource],
    reveal_entries: &[GameLogEntry],
    counter_entries: &[GameLogEntry],
) -> Result<Vec<GameCards>, StoreError> {
    let conn: &Connection = tx;
    let reveal_log = (!reveal_entries.is_empty()).then(|| extract_log_cards(reveal_entries));
    let counter_log = (!counter_entries.is_empty()).then(|| extract_log_cards(counter_entries));
    let results =
        (!counter_entries.is_empty()).then(|| extract_game_results(counter_entries, local));

    // One reference per id (frames repeat their cards), with the first name
    // a log gave it.
    let mut stubs: BTreeMap<u32, Option<String>> = BTreeMap::new();
    for game in games {
        for card in &game.local_deck {
            stubs.entry(card.catalog_id).or_default();
        }
        for frame in &game.frames {
            for card in &frame.cards {
                // 0.x drops empty ids (`filter()`).
                if card.catalog_id != 0 {
                    stubs.entry(card.catalog_id).or_default();
                }
            }
        }
        for (catalog_id, _) in &game.recorded_opponent_cards {
            stubs.entry(*catalog_id).or_default();
        }
    }
    for log in [&reveal_log, &counter_log].into_iter().flatten() {
        for player in &log.cards_by_player {
            for card in &player.cards {
                if card.mtgo_id == 0 {
                    continue;
                }
                let name = stubs.entry(card.mtgo_id).or_default();
                if name.is_none() && !card.name.is_empty() {
                    *name = Some(card.name.clone());
                }
            }
        }
    }
    let refs: Vec<CardRef> = stubs
        .into_iter()
        .map(|(catalog_id, name)| CardRef {
            catalog_id: i64::from(catalog_id),
            name,
        })
        .collect();
    ensure_stubs(tx, &refs)?;

    // Every card the rows name or merge by name.
    let mut ids: Vec<u32> = Vec::new();
    for game in games {
        ids.extend(game.local_deck.iter().map(|card| card.catalog_id));
        ids.extend(game.recorded_opponent_cards.iter().map(|(id, _)| *id));
        if let Some(last) = game.frames.last() {
            ids.extend(last.cards.iter().map(|card| card.catalog_id));
        }
    }
    if let Some(log) = &reveal_log {
        for player in &log.cards_by_player {
            ids.extend(player.cards.iter().map(|card| card.mtgo_id));
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let rows = card_infos(conn, &ids)?;
    let name = |id: u32| known_name(&rows, id);
    let name_of = |id: u32| name(id).unwrap_or_else(|| format!("Card {id}"));
    let oracle_of = |id: u32| {
        rows.get(&id)
            .and_then(|card| card.oracle_id.clone())
            .filter(|oracle| !oracle.is_empty())
    };

    let mut built = Vec::new();
    let mut game_one_deck: Vec<DeckCard> = Vec::new();
    for (index, game) in games.iter().enumerate() {
        let mut reveal_log_cards = Vec::new();
        if let (Some(log), Some(opponent)) = (&reveal_log, opponent) {
            for card in log.game_cards(index, opponent) {
                reveal_log_cards.push(LogCard {
                    catalog_id: card.mtgo_id,
                    name: name(card.mtgo_id).unwrap_or_else(|| card.name.clone()),
                });
            }
        }
        let empty_cards = Vec::new();
        let empty_pregame = Vec::new();
        let (local_cards, opponent_cards, local_pregame, opponent_pregame) = match &counter_log {
            Some(log) => {
                let game_log = log.games.get(index);
                let pregame_of = |player: &str| {
                    game_log
                        .and_then(|found| found.pregame.iter().find(|p| p.player == player))
                        .map(|found| found.actions.clone())
                        .unwrap_or_default()
                };
                (
                    log.game_cards(index, local).to_vec(),
                    opponent
                        .map(|name| log.game_cards(index, name).to_vec())
                        .unwrap_or_default(),
                    pregame_of(local),
                    opponent.map(pregame_of).unwrap_or_default(),
                )
            }
            None => (
                empty_cards.clone(),
                empty_cards,
                empty_pregame.clone(),
                empty_pregame,
            ),
        };
        let mut log_starting_hands = (None, None);
        if let Some(found) = results.as_ref().and_then(|found| found.games.get(index)) {
            for hand in &found.starting_hands {
                if hand.player == local {
                    log_starting_hands.0 = Some(hand.value);
                } else if log_starting_hands.1.is_none() {
                    log_starting_hands.1 = Some(hand.value);
                }
            }
        }
        let no_deck: Vec<DeckCard> = Vec::new();
        let input = GameCardsInput {
            frames: &game.frames,
            local_name: local,
            on_play: game.on_play,
            local_deck: &game.local_deck,
            game_one_deck: if index == 0 { &no_deck } else { &game_one_deck },
            recorded_opponent_cards: &game.recorded_opponent_cards,
            reveal_log_cards: &reveal_log_cards,
            has_log: counter_log.is_some(),
            local_log_cards: &local_cards,
            opponent_log_cards: &opponent_cards,
            local_pregame: &local_pregame,
            opponent_pregame: &opponent_pregame,
            log_starting_hands,
            name_of: &name_of,
            oracle_of: &oracle_of,
        };
        built.push(game_card_rows(&input));
        if index == 0 {
            game_one_deck = game.local_deck.clone();
        }
    }
    Ok(built)
}

fn known_name(rows: &HashMap<u32, CardRow>, id: u32) -> Option<String> {
    rows.get(&id)
        .and_then(|card| card.name.clone())
        .filter(|name| !name.is_empty())
}

// ---- The game sync -------------------------------------------------------------

/// The match a sync reads: where it came from and who played.
struct SyncHead {
    source: String,
    token: String,
    local: String,
    opponent: Option<String>,
    /// The player fixed the match (`corrected_at`): its stored opponent may
    /// be a name the log never gave.
    corrected: bool,
    /// The account is not an MTGO one (MTG Arena or Endstep): its rows are
    /// never read from MTGO log events.
    other_platform: bool,
}

fn sync_head(conn: &Connection, match_id: i64) -> Result<Option<SyncHead>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT m.source, m.token, a.username, o.username, m.corrected_at IS NOT NULL,
                 a.platform <> 'mtgo'
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| {
            Ok(SyncHead {
                source: row.get(0)?,
                token: row.get(1)?,
                local: row.get(2)?,
                opponent: row.get(3)?,
                corrected: row.get(4)?,
                other_platform: row.get(5)?,
            })
        })
        .optional()?)
}

/// What the rows of a log match are read from in its stored events.
#[derive(Debug, Default)]
struct MatchEvents {
    /// Each game's state frame texts, in log order, by Game ID.
    frames: HashMap<u64, Vec<String>>,
    /// Each game's first `deck_used` list, by Game ID (MTGO often logs the
    /// line twice; 0.x read only the first).
    deck_lists: HashMap<u64, Vec<DeckCard>>,
    /// The MetaMessage game log events.
    meta: Vec<MetaEvent>,
    /// The players the earliest game state that names any lists, as the
    /// projection reads them (`MatchFacts::first_players`).
    players: Option<Vec<String>>,
}

fn match_events(conn: &Connection, token: &str) -> Result<MatchEvents, StoreError> {
    let mut found = MatchEvents::default();
    for stored in crate::read::events_for_match(conn, token)? {
        // The kind and subjects do not depend on the local username.
        let Some(event) = prepare_at(
            stored.log_file_id as u64,
            stored.byte_offset,
            stored.logged_at_ms,
            &stored.raw,
            None,
        ) else {
            continue;
        };
        match event.kind {
            EventKind::GameStateUpdate => {
                if found.players.is_none() {
                    found.players = parse_players(&event.text).filter(|names| !names.is_empty());
                }
                if let Some(game_id) = event.subjects.game_id {
                    found.frames.entry(game_id).or_default().push(event.text);
                }
            }
            EventKind::DeckUsed => {
                if let Some((game_id, cards)) = decks::parse_deck_used(&event.text) {
                    found.deck_lists.entry(game_id).or_insert(cards);
                }
            }
            EventKind::GameManagementJson => found.meta.push(MetaEvent {
                at_ms: event.at_ms,
                text: event.text,
            }),
            _ => {}
        }
    }
    Ok(found)
}

/// A match's games as `(id, Game ID, on the play)`, in game order.
fn sync_games_of(conn: &Connection, match_id: i64) -> Result<Vec<SyncGame>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, mtgo_game_id, on_play FROM games WHERE match_id = ?1 ORDER BY game_number",
    )?;
    let rows = stmt.query_map([match_id], |row| {
        let mtgo_game_id: Option<i64> = row.get(1)?;
        Ok((
            row.get(0)?,
            mtgo_game_id.and_then(|id| u64::try_from(id).ok()),
            row.get(2)?,
        ))
    })?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row?);
    }
    Ok(games)
}

/// Sync one log match's games (spec section 6, "Writers"): read its events
/// (live or archived) and its stored game log, build each game's rows and
/// write the ones that changed. `dat` is the match's `.dat` file, decoded
/// into `game_logs` first when given; ticks are local wall-clock time in
/// `tz`. A match that is not `log` (manual, imported, history) is never
/// touched, nor is an MTG Arena match (its rows are `sync_arena_match`'s)
/// or an Endstep one (it has no MTGO log events).
/// Returns whether anything changed.
pub fn sync_match_games(
    tx: &Transaction,
    match_id: i64,
    tz: Tz,
    dat: Option<&[u8]>,
) -> Result<bool, StoreError> {
    let Some(head) = sync_head(tx, match_id)? else {
        return Ok(false);
    };
    if head.source != "log" || head.other_platform {
        return Ok(false);
    }
    let mut changed = false;
    if let Some(bytes) = dat
        && let Some(log) = parse_game_log(bytes, tz)
        && !log.entries.is_empty()
    {
        changed |= store_game_log(tx, match_id, &log.entries)?;
    }
    // A stored log that cannot be read counts as none: the MetaMessages stand in.
    let dat_entries = game_log(tx, match_id).ok().flatten().unwrap_or_default();
    let events = match_events(tx, &head.token)?;
    let counter_entries = if dat_entries.is_empty() {
        meta_message_entries(&events.meta)
    } else {
        dat_entries.clone()
    };

    // S1-5: the `.dat` name replaces the name this match's MetaMessages
    // gave a stub before the `.dat` file existed.
    if !dat_entries.is_empty() && !events.meta.is_empty() {
        let fixes = name_fixes(&meta_message_entries(&events.meta), &dat_entries);
        replace_meta_names(tx, &fixes)?;
    }

    let rows = sync_games_of(tx, match_id)?;
    let mut games = Vec::new();
    for (game_id, mtgo_game_id, on_play) in &rows {
        let texts: Vec<&str> = match mtgo_game_id.and_then(|id| events.frames.get(&id)) {
            Some(texts) => texts.iter().map(String::as_str).collect(),
            None => Vec::new(),
        };
        let frames = parse_frames(&texts);
        let deck_used: &[DeckCard] = match mtgo_game_id.and_then(|id| events.deck_lists.get(&id)) {
            Some(list) => list,
            None => &[],
        };
        let local_deck = played_deck(&frames, &head.local, deck_used);
        games.push(GameSource {
            game_id: *game_id,
            on_play: *on_play,
            frames,
            local_deck,
            recorded_opponent_cards: Vec::new(),
        });
    }
    // A corrected match reads its log by the log's own opponent (the
    // projection's: the frames' other player), so one whose opponent the
    // player renamed (edit a match) still finds the opponent's cards. The
    // stored name stands in for a log whose frames name nobody; for any
    // other match it is the projection's, the same name.
    let log_opponent = if head.corrected {
        events
            .players
            .as_deref()
            .and_then(|players| mtgo_match::games::other_player(players, &head.local))
    } else {
        None
    };
    let opponent = log_opponent.or(head.opponent);
    let built = build_game_cards(
        tx,
        &head.local,
        opponent.as_deref(),
        &games,
        &dat_entries,
        &counter_entries,
    )?;
    for (game, cards) in games.iter().zip(&built) {
        changed |= write_game_cards(tx, game.game_id, cards)?;
    }
    // The first sync of the complete match stamps it (decision C2), so
    // detection reads it only once its rows are written. Once: a second
    // sync writes nothing.
    stamp_games_synced(tx, match_id)?;
    Ok(changed)
}

/// Stamp a complete match's first game sync (`games_synced_at`, decision
/// C2), the mark detection waits for. Shared by the MTGO and Arena syncs.
/// Once: a stamped or unfinished match is left alone.
pub(crate) fn stamp_games_synced(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET games_synced_at = CAST(unixepoch('subsec') * 1000 AS INTEGER)
         WHERE id = ?1 AND state = 'complete' AND games_synced_at IS NULL",
    )?
    .execute([match_id])?;
    Ok(())
}

/// The cards a match's MetaMessages name differently from its `.dat` log
/// (`NameFix`), in catalog id order.
fn name_fixes(meta: &[GameLogEntry], dat: &[GameLogEntry]) -> Vec<NameFix> {
    let meta_names = log_names(meta);
    let dat_names = log_names(dat);
    let mut fixes = Vec::new();
    for (catalog_id, meta_name) in meta_names {
        if let Some(dat_name) = dat_names.get(&catalog_id)
            && *dat_name != meta_name
        {
            fixes.push((catalog_id, meta_name, dat_name.clone()));
        }
    }
    fixes
}

/// The first name a log gives each card (every player's).
fn log_names(entries: &[GameLogEntry]) -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    for player in &extract_log_cards(entries).cards_by_player {
        for card in &player.cards {
            if card.mtgo_id != 0 && !card.name.is_empty() {
                names
                    .entry(card.mtgo_id)
                    .or_insert_with(|| card.name.clone());
            }
        }
    }
    names
}

/// What one game sync request did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameSyncOutcome {
    /// Whether any row changed.
    pub changed: bool,
    /// The tokens of the matches whose rows changed, so a caller pushes
    /// only those again.
    pub changed_tokens: Vec<String>,
    /// The matches left as they were because their events could not be read
    /// (a corrupt archive). They are synced again on their next change or
    /// at the next `GAME_CARDS_VERSION`.
    pub skipped: Vec<String>,
}

/// Sync the log matches with these tokens, each with its `.dat` file when
/// one is given, in one transaction. A token with no match is skipped
/// silently; a match whose sync fails (its archive cannot be read) is rolled
/// back to its savepoint and reported in `skipped`, so one bad archive never
/// fails the other matches of the request.
pub fn sync_games(
    conn: &mut Connection,
    tokens: &[String],
    dat: &[(String, Vec<u8>)],
    tz: Tz,
) -> Result<GameSyncOutcome, StoreError> {
    let tx = conn.transaction()?;
    let mut outcome = GameSyncOutcome::default();
    for token in tokens {
        let match_id: Option<i64> = tx
            .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
            .query_row([token], |row| row.get(0))
            .optional()?;
        let Some(match_id) = match_id else {
            continue;
        };
        let file = dat
            .iter()
            .find(|(owner, _)| owner == token)
            .map(|(_, bytes)| bytes.as_slice());
        tx.execute_batch("SAVEPOINT game_sync_match")?;
        match sync_match_games(&tx, match_id, tz, file) {
            Ok(changed) => {
                outcome.changed |= changed;
                if changed {
                    outcome.changed_tokens.push(token.clone());
                }
            }
            // A database error (a busy writer, a full disk) is transient or
            // fatal, not a bad archive: fail the request, which rolls back.
            Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
            Err(_) => {
                tx.execute_batch("ROLLBACK TO game_sync_match")?;
                outcome.skipped.push(token.clone());
            }
        }
        tx.execute_batch("RELEASE game_sync_match")?;
    }
    tx.commit()?;
    Ok(outcome)
}

/// The tokens of every MTGO log match with games, oldest first (the
/// backfill). Arena matches write their games themselves.
pub fn log_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT token FROM matches
         WHERE source = 'log' AND EXISTS (SELECT 1 FROM games WHERE match_id = matches.id)
           AND {mtgo}
         ORDER BY id",
        mtgo = account_of_mtgo("account_id"),
    ))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// The version the stored game card rows were built with, `None` before
/// the first backfill.
pub fn game_cards_version(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT version FROM derivations WHERE name = ?1",
            [GAME_CARDS_DERIVATION],
            |row| row.get(0),
        )
        .optional()?)
}

/// Record that every log match's games were synced at `GAME_CARDS_VERSION`.
pub fn record_game_cards_version(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO derivations (name, version, rebuilt_at)
         VALUES (?1, ?2, CAST(unixepoch('subsec') * 1000 AS INTEGER))
         ON CONFLICT (name) DO UPDATE SET version = excluded.version,
             rebuilt_at = excluded.rebuilt_at",
        params![GAME_CARDS_DERIVATION, GAME_CARDS_VERSION],
    )?;
    Ok(())
}

/// Sync every log match's games, one transaction per match, and record the
/// version (the backfill in one call). Returns how many matches changed.
pub fn sync_all_games(conn: &mut Connection, tz: Tz) -> Result<u32, StoreError> {
    let mut changed = 0;
    for token in log_match_tokens(conn)? {
        if sync_games(conn, std::slice::from_ref(&token), &[], tz)?.changed {
            changed += 1;
        }
    }
    record_game_cards_version(conn)?;
    Ok(changed)
}

/// An imported game's frames as frames (`game_timelines` holds the JSON
/// without the log header).
pub fn timeline_frames(frames: &[FrameText]) -> Vec<Frame> {
    let jsons: Vec<&str> = frames.iter().map(|frame| frame.text.as_str()).collect();
    parse_frame_contents(&jsons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::write::{MatchWrite, NewEvent, WriteBatch, commit, insert_log_file};
    use fixture_tools::load_game_log;
    use mtgo_match::{GameResult, MatchView};
    use mtgo_protocol::Subjects;

    const UTC: Tz = chrono_tz::UTC;

    #[test]
    fn the_backfill_skips_arena_matches() {
        let (_dir, conn) = db("game-sync-tokens-arena");
        let me = account(&conn, "Qlocal");
        for token in ["q-mtgo", "q-arena"] {
            conn.execute(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', 'complete', 'win', 1, 1, 1, 1)",
                rusqlite::params![token, me],
            )
            .expect("match");
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason)
                 VALUES ((SELECT id FROM matches WHERE token = ?1), 1, 'win', 'win')",
                [token],
            )
            .expect("game");
        }
        move_match_to_arena(&conn, "q-arena");
        assert_eq!(log_match_tokens(&conn).expect("tokens"), ["q-mtgo"]);
    }

    #[test]
    fn the_sync_leaves_endstep_matches_alone() {
        let (_dir, mut conn) = db("game-sync-endstep");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform)
             VALUES ('endstep:q-user', 0, 'endstep')",
            [],
        )
        .expect("account");
        conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-endstep', (SELECT id FROM accounts WHERE username = 'endstep:q-user'),
                 'log', 'complete', 'win', 1, 1, 1, 1)",
            [],
        )
        .expect("match");
        let match_id = match_id(&conn, "q-endstep");
        // A correction rebuilds a log match's game rows through this sync:
        // an Endstep match has no MTGO log events to read them from.
        let tx = conn.transaction().expect("tx");
        assert!(!sync_match_games(&tx, match_id, UTC, None).expect("sync"));
        tx.commit().expect("commit");
        let stamped: Option<i64> = conn
            .query_row(
                "SELECT games_synced_at FROM matches WHERE id = ?1",
                [match_id],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(stamped, None, "never stamped by the MTGO sync");
    }

    /// A state frame of Game ID 7: hand counts, the opponent's library and
    /// `(instance id, catalog id, owner, zone)` cards.
    fn frame_raw(
        local_hand: i64,
        opponent_hand: i64,
        opponent_library: i64,
        cards: &[(i64, u32, i64, &str)],
    ) -> String {
        let cards: Vec<String> = cards
            .iter()
            .map(|(id, catalog_id, owner, zone)| {
                format!(
                    "{{\"Id\":{id},\"CatalogID\":{catalog_id},\"Zone\":\"{zone}\",\"ActualZone\":\"{zone}\",\"Owner\":{owner},\"Controller\":{owner}}}"
                )
            })
            .collect();
        format!(
            "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: 7, Match ID: 99, Event ID: 99) {{\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":53,\"HandCount\":{local_hand},\"Life\":20}},{{\"Id\":2,\"Name\":\"Qopponent\",\"LibraryCount\":{opponent_library},\"HandCount\":{opponent_hand},\"Life\":20}}],\"Cards\":[{}]}}",
            cards.join(",")
        )
    }

    fn hand_of(cards: &[(i64, u32)]) -> Vec<(i64, u32, i64, &'static str)> {
        cards
            .iter()
            .map(|&(id, catalog_id)| (id, catalog_id, 1, "Hand"))
            .collect()
    }

    /// A game with a mulligan (1xx sent back), one card bottomed (207), the
    /// turn 1 draw (300) and the opponent's land.
    fn mulligan_frames() -> Vec<String> {
        let first: Vec<(i64, u32)> = (1..=7).map(|i| (100 + i, 1000 + i as u32)).collect();
        let second: Vec<(i64, u32)> = (1..=7).map(|i| (200 + i, 2000 + i as u32)).collect();
        let kept: Vec<(i64, u32)> = second
            .iter()
            .copied()
            .filter(|(id, _)| *id != 207)
            .collect();
        let mut drawn = kept.clone();
        drawn.push((300, 2001));
        let mut played = hand_of(&drawn[1..]);
        played.push((201, 2001, 1, "Battlefield"));
        played.push((900, 9001, 2, "Battlefield"));
        vec![
            frame_raw(0, 0, 60, &[]),
            frame_raw(7, 7, 53, &hand_of(&first)),
            frame_raw(7, 7, 53, &hand_of(&second)),
            frame_raw(6, 7, 54, &hand_of(&kept)),
            frame_raw(7, 7, 54, &hand_of(&drawn)),
            frame_raw(6, 7, 54, &played),
        ]
    }

    fn frame_event(file: i64, index: usize, raw: &str) -> NewEvent {
        event_with(
            file,
            (index as u64 + 1) * 1_000,
            EventKind::GameStateUpdate,
            Subjects {
                match_id: Some(99),
                game_id: Some(7),
                ..Subjects::default()
            },
            raw,
        )
    }

    /// One game (Game ID 7), on the draw, won.
    fn one_game_view() -> MatchView {
        let mut view = view(TOKEN, "Qopponent", 1_000);
        let mut first = game(1, Some(7), GameResult::Win);
        first.on_play = Some(false);
        view.games = vec![first];
        view.wins = 1;
        view.losses = 0;
        view
    }

    fn store_match(conn: &mut Connection) -> i64 {
        let file = insert_log_file(conn, &new_log_file("Q/mtgo.log")).expect("file");
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(one_game_view()),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("match");
        file
    }

    fn sync(conn: &mut Connection, token: &str, dat: Option<&[u8]>) -> bool {
        let id = match_id(conn, token);
        let tx = conn.transaction().expect("tx");
        let changed = sync_match_games(&tx, id, UTC, dat).expect("sync");
        tx.commit().expect("commit");
        changed
    }

    fn rows(conn: &Connection) -> Vec<Vec<String>> {
        let mut all = dump(conn, "game_cards");
        all.extend(dump(conn, "game_card_casts"));
        all.extend(dump(conn, "game_mulligan_cards"));
        let sizes: String = conn
            .query_row(
                "SELECT group_concat(coalesce(local_opening_hand_size, -1) || ',' ||
                     coalesce(local_starting_hand_size, -1) || ',' ||
                     coalesce(opp_opening_hand_size, -1) || ',' ||
                     coalesce(opp_starting_hand_size, -1), ';') FROM games",
                [],
                |row| row.get(0),
            )
            .expect("sizes");
        all.push(vec![sizes]);
        all
    }

    #[test]
    fn game_sync_once_per_event_equals_one_sync_at_the_end() {
        let (_dir, mut live) = db("game-sync-live");
        let file = store_match(&mut live);
        for (index, raw) in mulligan_frames().iter().enumerate() {
            store_events(&mut live, vec![frame_event(file, index, raw)]);
            sync(&mut live, TOKEN, None);
        }

        let (_dir2, mut rebuilt) = db("game-sync-rebuilt");
        let file = store_match(&mut rebuilt);
        let events = mulligan_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut rebuilt, events);
        assert!(sync(&mut rebuilt, TOKEN, None));

        assert_eq!(rows(&live), rows(&rebuilt));
        let game = number(&rebuilt, "SELECT id FROM games");
        let cards = read_game_cards(&rebuilt, game).expect("cards");
        assert_eq!(cards.opening_hand_size, Some(7));
        assert_eq!(cards.starting_hand_size, Some(6));
        assert_eq!(cards.opp_starting_hand_size, Some(6));
        let bottomed = cards.row(Side::Local, 2007).expect("bottomed");
        assert_eq!(
            (
                bottomed.opening_hand,
                bottomed.bottomed,
                bottomed.starting_hand
            ),
            (1, 1, 0)
        );
        let drawn = cards.row(Side::Local, 2001).expect("drawn");
        assert_eq!((drawn.opening_hand, drawn.first_draw), (1, 1));
        assert_eq!(cards.mulligans.len(), 7);
        assert_eq!(cards.row(Side::Opponent, 9001).expect("seen").seen, 1);
    }

    /// `mulligan_frames` then a frame where 2002 (instance 202) went from the
    /// hand to the graveyard.
    fn discard_frames() -> Vec<String> {
        let mut frames = mulligan_frames();
        let mut cards: Vec<(i64, u32, i64, &str)> = (203..=206)
            .map(|id| (id, 2000 + (id - 200) as u32, 1, "Hand"))
            .collect();
        cards.push((300, 2001, 1, "Hand"));
        cards.push((201, 2001, 1, "Battlefield"));
        cards.push((202, 2002, 1, "Graveyard"));
        cards.push((900, 9001, 2, "Battlefield"));
        frames.push(frame_raw(5, 7, 54, &cards));
        frames
    }

    #[test]
    fn zone_counters_live_equal_rebuild() {
        let (_dir, mut live) = db("game-sync-zones-live");
        let file = store_match(&mut live);
        for (index, raw) in discard_frames().iter().enumerate() {
            store_events(&mut live, vec![frame_event(file, index, raw)]);
            sync(&mut live, TOKEN, None);
        }

        let (_dir2, mut rebuilt) = db("game-sync-zones-rebuilt");
        let file = store_match(&mut rebuilt);
        let events = discard_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut rebuilt, events);
        assert!(sync(&mut rebuilt, TOKEN, None));

        assert_eq!(rows(&live), rows(&rebuilt));
        let game = number(&rebuilt, "SELECT id FROM games");
        let cards = read_game_cards(&rebuilt, game).expect("cards");
        let discarded = cards.row(Side::Local, 2002).expect("discarded");
        assert_eq!(
            (
                discarded.hand_seen,
                discarded.graveyard_seen,
                discarded.discarded,
                discarded.has_zone_data
            ),
            (1, 1, 1, true)
        );
        let land = cards.row(Side::Local, 2001).expect("land");
        assert_eq!((land.hand_seen, land.battlefield_seen), (2, 1));
        let theirs = cards.row(Side::Opponent, 9001).expect("opponent");
        assert_eq!((theirs.battlefield_seen, theirs.has_zone_data), (0, false));
    }

    /// A game log where the opponent casts Damnation (catalog id 26279,
    /// logged doubled as 52558).
    fn damnation_log() -> Vec<GameLogEntry> {
        [
            "@P@PQlocal joined the game.",
            "@P@PQopponent joined the game.",
            "@PQopponent casts @[Damnation@:52558,101:@].",
            "@PQopponent wins the game.",
        ]
        .iter()
        .enumerate()
        .map(|(index, message)| GameLogEntry {
            timestamp_ms: 1_000 + index as i64,
            message: message.to_string(),
        })
        .collect()
    }

    fn store_log(conn: &mut Connection, entries: &[GameLogEntry]) {
        let id = match_id(conn, TOKEN);
        let tx = conn.transaction().expect("tx");
        store_game_log(&tx, id, entries).expect("log");
        tx.commit().expect("commit");
    }

    #[test]
    fn creates_card_stubs_for_opponent_cards_that_only_ever_appear_in_the_game_log() {
        // The opponent's Damnation never reaches a state frame, so nothing
        // else learns the id exists. The stub keeps the log's name for the
        // by-name pass.
        let (_dir, mut conn) = db("game-sync-log-stubs");
        store_match(&mut conn);
        store_log(&mut conn, &damnation_log());
        assert!(sync(&mut conn, TOKEN, None));
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM cards WHERE catalog_id = 26279",
                [],
                |row| row.get(0),
            )
            .expect("a stub for the log card");
        assert_eq!(name.as_deref(), Some("Damnation"));
    }

    /// The opponent's side of the Damnation game: the log card rows.
    fn opponent_log_rows(conn: &Connection) -> i64 {
        number(
            conn,
            "SELECT COUNT(*) FROM game_cards WHERE side = 'opponent' AND catalog_id = 26279",
        )
    }

    #[test]
    fn a_renamed_opponent_still_finds_the_opponents_cards_in_the_log() {
        // Edit a match: the player renames the opponent. The log still names
        // the player it saw (the frames' other player), so the game sync
        // reads the opponent's log cards by that name.
        let (_dir, mut conn) = db("game-sync-renamed-opponent");
        let file = store_match(&mut conn);
        store_log(&mut conn, &damnation_log());
        let events = mulligan_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut conn, events);
        assert!(sync(&mut conn, TOKEN, None));
        assert_eq!(opponent_log_rows(&conn), 1, "the log's opponent cards");

        conn.execute_batch(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qrenamed', 1);
             UPDATE matches SET corrected_at = 5,
                 opponent_id = (SELECT id FROM opponents WHERE username = 'Qrenamed');
             DELETE FROM game_cards;",
        )
        .expect("renamed");
        assert!(sync(&mut conn, TOKEN, None));
        assert_eq!(opponent_log_rows(&conn), 1, "rebuilt with the log's name");
    }

    #[test]
    fn stub_writes_live_equal_rebuild() {
        // The deck as played, every frame card (the opponent's 9001 token)
        // and every game log card get stubs, whether the match is synced
        // after each event or once at the end; a second sync adds nothing.
        let (_dir, mut live) = db("game-sync-stubs-live");
        let file = store_match(&mut live);
        store_log(&mut live, &damnation_log());
        for (index, raw) in mulligan_frames().iter().enumerate() {
            store_events(&mut live, vec![frame_event(file, index, raw)]);
            sync(&mut live, TOKEN, None);
        }

        let (_dir2, mut rebuilt) = db("game-sync-stubs-rebuilt");
        let file = store_match(&mut rebuilt);
        store_log(&mut rebuilt, &damnation_log());
        let events = mulligan_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut rebuilt, events);
        assert!(sync(&mut rebuilt, TOKEN, None));

        assert_eq!(dump(&live, "cards"), dump(&rebuilt, "cards"));
        assert_eq!(rows(&live), rows(&rebuilt));
        for catalog_id in [1001, 2007, 9001, 26279] {
            assert_eq!(
                number(
                    &rebuilt,
                    &format!("SELECT COUNT(*) FROM cards WHERE catalog_id = {catalog_id}")
                ),
                1,
                "a stub for {catalog_id}"
            );
        }

        crate::cards::take_stubs_added(&rebuilt).expect("reset");
        let changes_before = number(&rebuilt, "SELECT total_changes()");
        assert!(!sync(&mut rebuilt, TOKEN, None));
        assert_eq!(number(&rebuilt, "SELECT total_changes()"), changes_before);
        assert_eq!(crate::cards::take_stubs_added(&rebuilt).expect("count"), 0);
    }

    /// A chat MetaMessage of Game ID 7 (0.x's test envelope: u32 length,
    /// type 3, header bytes, u32 text length, then the text).
    fn meta_event(file: i64, offset: u64, text: &str) -> NewEvent {
        let len = text.len() as u8;
        let mut bytes = vec![len + 24, 0, 0, 0, 3, 17, 186, 129];
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&[len, 0, 0, 0]);
        bytes.extend_from_slice(text.as_bytes());
        let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
        event_with(
            file,
            offset,
            EventKind::GameManagementJson,
            Subjects {
                match_token: Some(TOKEN.to_string()),
                match_id: Some(99),
                game_id: Some(7),
                ..Subjects::default()
            },
            &format!(
                "12:00:00 [INF] (Game Management|Processing) Message: {{\"MatchToken\":\"{TOKEN}\",\"MatchID\":99,\"GameID\":7,\"MetaMessage\":[{}]}}",
                list.join(",")
            ),
        )
    }

    /// The `.dat` log naming 26279 (logged doubled as 52558) and 26281
    /// (52562) one way, the MetaMessages another.
    fn named_log(name: &str, other: &str) -> Vec<GameLogEntry> {
        [
            "@P@PQlocal joined the game.".to_string(),
            "@P@PQopponent joined the game.".to_string(),
            format!("@PQopponent casts @[{name}@:52558,101:@]."),
            format!("@PQopponent casts @[{other}@:52562,102:@]."),
        ]
        .iter()
        .enumerate()
        .map(|(index, message)| GameLogEntry {
            timestamp_ms: 1_500 + index as i64,
            message: message.clone(),
        })
        .collect()
    }

    fn stub_name(conn: &Connection, catalog_id: u32) -> Option<String> {
        conn.query_row(
            "SELECT name FROM cards WHERE catalog_id = ?1",
            [catalog_id],
            |row| row.get(0),
        )
        .expect("a stub")
    }

    #[test]
    fn the_dat_name_wins_over_the_metamessage_name_live_and_rebuilt() {
        // S1-5: live, the MetaMessages name a log-only card before the `.dat`
        // file exists; rebuilt, the `.dat` file is there from the start. Both
        // end with the `.dat` name. A known card (card data) keeps its name.
        let setup = |label: &str| {
            let (dir, mut conn) = db(label);
            let file = store_match(&mut conn);
            conn.execute(
                "INSERT INTO cards (catalog_id, oracle_id, name) VALUES (26281, 'q-oracle', 'Qknown')",
                [],
            )
            .expect("known card");
            let meta = named_log("Qmeta", "Qmeta other");
            let events = meta
                .iter()
                .enumerate()
                .map(|(index, entry)| meta_event(file, 10_000 + index as u64, &entry.message))
                .collect();
            store_events(&mut conn, events);
            (dir, conn)
        };
        let dat = named_log("Qdat", "Qdat other");

        let (_dir, mut live) = setup("game-sync-dat-name-live");
        sync(&mut live, TOKEN, None);
        assert_eq!(stub_name(&live, 26279).as_deref(), Some("Qmeta"));
        store_log(&mut live, &dat);
        sync(&mut live, TOKEN, None);

        let (_dir2, mut rebuilt) = setup("game-sync-dat-name-rebuilt");
        store_log(&mut rebuilt, &dat);
        sync(&mut rebuilt, TOKEN, None);

        assert_eq!(stub_name(&rebuilt, 26279).as_deref(), Some("Qdat"));
        assert_eq!(stub_name(&rebuilt, 26281).as_deref(), Some("Qknown"));
        assert_eq!(dump(&live, "cards"), dump(&rebuilt, "cards"));
        assert_eq!(rows(&live), rows(&rebuilt));
    }

    #[test]
    fn a_name_another_match_gave_is_not_replaced() {
        // Review M5: only this match's own MetaMessage name gives way to its
        // `.dat` name, so two matches never flip a stub's name.
        let (_dir, mut conn) = db("game-sync-dat-name-other");
        let file = store_match(&mut conn);
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (26279, 'Qother')",
            [],
        )
        .expect("a stub named by another match");
        let events = named_log("Qmeta", "Qmeta other")
            .iter()
            .enumerate()
            .map(|(index, entry)| meta_event(file, 10_000 + index as u64, &entry.message))
            .collect();
        store_events(&mut conn, events);
        store_log(&mut conn, &named_log("Qdat", "Qdat other"));
        sync(&mut conn, TOKEN, None);
        assert_eq!(stub_name(&conn, 26279).as_deref(), Some("Qother"));
        assert_eq!(stub_name(&conn, 26281).as_deref(), Some("Qdat other"));
    }

    #[test]
    fn a_database_error_fails_the_request_instead_of_skipping_the_match() {
        let (_dir, mut conn) = db("game-sync-sqlite-error");
        let file = store_match(&mut conn);
        let events = mulligan_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut conn, events);
        // A missing table is a database error, not an unreadable archive.
        conn.execute_batch("DROP TABLE game_cards")
            .expect("drop game_cards");

        let result = sync_games(&mut conn, &[TOKEN.to_string()], &[], UTC);
        assert!(
            matches!(result, Err(StoreError::Sqlite(_))),
            "a SQLite error is returned, not skipped: {result:?}"
        );
    }

    #[test]
    fn an_unreadable_archive_skips_that_match_and_the_rest_are_synced() {
        let (_dir, mut conn) = db("game-sync-corrupt");
        let file = store_match(&mut conn);
        let events = mulligan_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut conn, events);
        let mut other = view(OTHER, "Qopponent", 2_000);
        other.games = vec![game(1, Some(8), GameResult::Win)];
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(other),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("other");
        conn.execute(
            "INSERT INTO match_event_archives (match_token, raw, event_count, compacted_at)
             VALUES (?1, x'00', 1, 1)",
            [OTHER],
        )
        .expect("corrupt archive");

        let outcome = sync_games(&mut conn, &[OTHER.to_string(), TOKEN.to_string()], &[], UTC)
            .expect("the request does not fail");
        assert!(outcome.changed);
        assert_eq!(outcome.skipped, vec![OTHER.to_string()]);
        assert_eq!(outcome.changed_tokens, vec![TOKEN.to_string()]);
        // Nothing changed now: nothing to push again.
        let again = sync_games(&mut conn, &[TOKEN.to_string()], &[], UTC).expect("again");
        assert!(!again.changed);
        assert!(again.changed_tokens.is_empty());
        let id = match_id(&conn, TOKEN);
        assert!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM game_cards WHERE game_id IN
                     (SELECT id FROM games WHERE match_id = {id})"
                )
            ) > 0,
            "the readable match is synced"
        );
    }

    #[test]
    fn a_second_game_sync_writes_nothing() {
        let (_dir, mut conn) = db("game-sync-twice");
        let file = store_match(&mut conn);
        let events = mulligan_frames()
            .iter()
            .enumerate()
            .map(|(index, raw)| frame_event(file, index, raw))
            .collect();
        store_events(&mut conn, events);
        assert!(sync(&mut conn, TOKEN, None));
        let before = rows(&conn);
        let changes_before = number(&conn, "SELECT total_changes()");
        assert!(!sync(&mut conn, TOKEN, None));
        assert_eq!(number(&conn, "SELECT total_changes()"), changes_before);
        assert_eq!(rows(&conn), before);
    }

    /// Decision C2: the game sync stamps a complete log match once, so
    /// detection knows its rows are written (archetypes spec section 6.2).
    #[test]
    fn a_complete_match_is_stamped_synced_once() {
        let (_dir, mut conn) = db("game-sync-stamp");
        store_match(&mut conn);
        conn.execute("UPDATE matches SET state = 'in_progress'", [])
            .expect("in progress");
        sync(&mut conn, TOKEN, None);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NULL"
            ),
            1,
            "not while the match runs"
        );
        conn.execute("UPDATE matches SET state = 'complete'", [])
            .expect("complete");
        sync(&mut conn, TOKEN, None);
        let stamp = number(&conn, "SELECT games_synced_at FROM matches");
        assert!(stamp > 1_000_000_000_000, "a time: {stamp}");
        conn.execute("UPDATE matches SET games_synced_at = 5", [])
            .expect("older stamp");
        sync(&mut conn, TOKEN, None);
        assert_eq!(number(&conn, "SELECT games_synced_at FROM matches"), 5);
    }

    #[test]
    fn a_game_sync_never_touches_a_manual_game() {
        let (_dir, mut conn) = db("game-sync-manual");
        insert_manual(&conn, OTHER);
        let game = number(&conn, "SELECT id FROM games WHERE game_number = 1");
        let manual = GameCards {
            opening_hand_size: Some(7),
            starting_hand_size: Some(7),
            cards: vec![GameCardRow {
                catalog_id: 4001,
                opening_hand: 7,
                starting_hand: 7,
                ..GameCardRow::default()
            }],
            ..GameCards::default()
        };
        let tx = conn.transaction().expect("tx");
        write_game_cards(&tx, game, &manual).expect("write");
        tx.commit().expect("commit");
        let before = rows(&conn);
        assert!(!sync(&mut conn, OTHER, None));
        assert!(
            !sync_games(&mut conn, &[OTHER.to_string()], &[], UTC)
                .expect("sync")
                .changed
        );
        sync_all_games(&mut conn, UTC).expect("backfill");
        assert_eq!(rows(&conn), before);
        assert_eq!(read_game_cards(&conn, game).expect("cards"), manual);
    }

    #[test]
    fn every_written_game_row_and_mulligan_has_a_card_row() {
        // Manual matches and imported lay-overs write rows the sources never
        // named; each id still gets a stub in the same transaction.
        let (_dir, mut conn) = db("game-cards-stubs");
        insert_manual(&conn, OTHER);
        let game = number(&conn, "SELECT id FROM games WHERE game_number = 1");
        let cards = GameCards {
            cards: vec![GameCardRow {
                catalog_id: 4001,
                main_qty: 4,
                ..GameCardRow::default()
            }],
            mulligans: vec![(1, 4002, 1)],
            ..GameCards::default()
        };
        let tx = conn.transaction().expect("tx");
        write_game_cards(&tx, game, &cards).expect("write");
        tx.commit().expect("commit");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM cards WHERE catalog_id IN (4001, 4002)"
            ),
            2
        );
    }

    #[test]
    fn the_dat_file_is_stored_once_and_adds_log_counters() {
        let bytes = load_game_log("clean_2_0_win");
        let entries = parse_game_log(&bytes, UTC).expect("parses").entries;
        let players = game_detail::detect_players(&entries);
        let (_dir, mut conn) = db("game-sync-dat");
        let mut view = view(TOKEN, &players[1], 1_000);
        view.local_username = players[0].clone();
        view.games.truncate(2);
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("match");
        // No events and no file yet: nothing to write.
        assert!(!sync(&mut conn, TOKEN, None));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_cards"), 0);

        assert!(sync(&mut conn, TOKEN, Some(&bytes)));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_logs"), 1);
        assert_eq!(
            number(&conn, "SELECT entry_count FROM game_logs"),
            entries.len() as i64
        );
        let id = match_id(&conn, TOKEN);
        assert_eq!(game_log(&conn, id).expect("log"), Some(entries.clone()));
        assert!(number(&conn, "SELECT SUM(`cast`) FROM game_cards") > 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM game_cards WHERE played IS NULL"
            ),
            0,
            "a game log makes every counter known"
        );
        let cards = extract_log_cards(&entries);
        let first_game = number(&conn, "SELECT id FROM games WHERE game_number = 1");
        let stored = read_game_cards(&conn, first_game).expect("cards");
        for card in cards.game_cards(0, &players[0]) {
            let row = stored
                .row(Side::Local, card.mtgo_id)
                .expect("a row per log card");
            assert_eq!(row.cast, card.count(game_detail::Counter::Cast));
        }

        // The same file again, or none: the stored log stands, nothing changes.
        let decoded_at = number(&conn, "SELECT decoded_at FROM game_logs");
        assert!(!sync(&mut conn, TOKEN, Some(&bytes)));
        assert!(!sync(&mut conn, TOKEN, None));
        assert_eq!(
            number(&conn, "SELECT decoded_at FROM game_logs"),
            decoded_at
        );
    }

    #[test]
    fn starting_hand_is_opening_hand_minus_bottomed_in_the_schema() {
        let (_dir, conn) = db("game-cards-check");
        insert_manual(&conn, OTHER);
        let game = number(&conn, "SELECT id FROM games WHERE game_number = 1");
        let insert = |opening: i64, bottomed: i64, starting: i64| {
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id, opening_hand, bottomed,
                     starting_hand) VALUES (?1, 'local', ?2, ?3, ?4, ?5)",
                params![game, opening * 100 + starting, opening, bottomed, starting],
            )
        };
        assert!(insert(2, 1, 1).is_ok());
        assert!(
            insert(2, 1, 2).is_err(),
            "starting must be opening minus bottomed"
        );
        assert!(
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id) VALUES (?1, 'both', 1)",
                [game],
            )
            .is_err()
        );
    }

    fn frame(time: &str, text: &str) -> FrameText {
        FrameText {
            time: time.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn timelines_and_logs_round_trip_and_write_only_changes() {
        let (_dir, mut conn) = db("game-timelines");
        insert_manual(&conn, OTHER);
        let game = number(&conn, "SELECT id FROM games WHERE game_number = 1");
        let match_id = match_id(&conn, OTHER);
        let frames = vec![
            frame("10:00:01", "{\"Cards\":[]}"),
            frame("10:00:02", "{\"Cards\":[{}]}"),
        ];
        let entries = vec![GameLogEntry {
            timestamp_ms: 5,
            message: "Q".to_string(),
        }];
        let tx = conn.transaction().expect("tx");
        assert!(store_timeline(&tx, game, &frames).expect("timeline"));
        assert!(!store_timeline(&tx, game, &frames).expect("again"));
        assert!(store_game_log(&tx, match_id, &entries).expect("log"));
        assert!(!store_game_log(&tx, match_id, &entries).expect("again"));
        tx.commit().expect("commit");
        assert_eq!(timeline(&conn, game).expect("read"), Some(frames));
        assert_eq!(number(&conn, "SELECT frame_count FROM game_timelines"), 2);
        assert_eq!(game_log(&conn, match_id).expect("read"), Some(entries));
    }

    // Replays spec section 9: each stored frame keeps its 0.x time text in
    // `game_timelines.times`, one per frame in `raw`'s order; a change of a
    // time alone is a change.
    #[test]
    fn timeline_times_round_trip() {
        let (_dir, mut conn) = db("game-timeline-times");
        insert_manual(&conn, OTHER);
        let game = number(&conn, "SELECT id FROM games WHERE game_number = 1");
        let frames = vec![
            frame("03:04:06", "{\"Cards\":[]}"),
            frame("03:04:08.250", "{\"Cards\":[{}]}"),
            frame("03:04:09", "{\"Cards\":[]}"),
        ];
        let tx = conn.transaction().expect("tx");
        assert!(store_timeline(&tx, game, &frames).expect("timeline"));
        let mut moved = frames.clone();
        moved[1].time = "03:04:08".to_string();
        assert!(store_timeline(&tx, game, &moved).expect("a time changed"));
        assert!(!store_timeline(&tx, game, &moved).expect("again"));
        tx.commit().expect("commit");
        assert_eq!(timeline(&conn, game).expect("read"), Some(moved));
        let times: Vec<u8> = conn
            .query_row("SELECT times FROM game_timelines", [], |row| row.get(0))
            .expect("times");
        let times: Vec<String> =
            serde_json::from_slice(&zstd::decode_all(times.as_slice()).expect("zstd"))
                .expect("a JSON list");
        assert_eq!(times, vec!["03:04:06", "03:04:08", "03:04:09"]);

        let tx = conn.transaction().expect("tx");
        assert!(store_timeline(&tx, game, &[]).expect("none"));
        tx.commit().expect("commit");
        assert_eq!(timeline(&conn, game).expect("read"), None);
    }
}
