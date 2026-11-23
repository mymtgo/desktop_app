//! A match's replay inputs from its stored rows (replays spec section 4):
//! each game's frames, game log and sideboard, the match's head (format,
//! start, local player, login id, share link) and every opponent name its
//! frames hold, for the replay window (`replay_view`) and the share
//! (`build_snapshot`, then `prepare_shared_snapshot`).
//!
//! Frames by source:
//! - `log` (live or rebuilt): the match's `game_state_update` events, live
//!   rows plus the compaction archive, in `(log_file_id, byte_offset)`
//!   order, split per game by the header's Game ID; each frame's time is the
//!   header's `HH:MM:SS` (`game_detail::frame_time`).
//! - `imported`: the game's `game_timelines` row with its times.
//! - `manual`, `history`: none (no Replay button).
//!
//! Frame content is the whole JSON value, every key kept. A frame whose JSON
//! cannot be read is skipped (0.x `ExtractJson`); then consecutive identical
//! frames are dropped through `replay::drop_repeats`, keeping the first
//! one's time and the game's last frame (ruling 4).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono_tz::Tz;
use game_detail::{
    GameLogEntry, MetaEvent, frame_content_value, frame_text, frame_time, game_log_entries,
    meta_message_entries,
};
use mtgo_protocol::{EventKind, prepare_at};
use replay::{CardInfo, FrameText, GameInput, LogEntry, ReplayFrame, SnapshotInput, drop_repeats};
use rusqlite::OptionalExtension;
use serde_json::Value;

use crate::match_detail::{GameRow, match_games};
use crate::names::game_won;
use crate::{Connection, StoreError};

/// A match's shared replay link on mymtgo.com.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub uuid: String,
    pub url: String,
    pub shared_at_ms: i64,
}

/// One game of the match as the replay reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayGameInput {
    /// The `games.id`.
    pub id: i64,
    /// The game's number in its match.
    pub number: u32,
    /// `win` true, `loss` false, anything else unknown.
    pub won: Option<bool>,
    /// The frames, undecorated (`replay::decorate_frames` or the snapshot
    /// builder decorate them).
    pub frames: Vec<ReplayFrame>,
    /// The game's log lines, `HH:MM:SS` local time (0.x `GetGameLogEntries`).
    pub log: Vec<LogEntry>,
    /// Your sideboard as the game began: `(catalog id, quantity)`.
    pub sideboard: Vec<(u32, u32)>,
}

/// Everything a replay of one match is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayInputs {
    pub token: String,
    /// `log`, `imported`, `manual` or `history`.
    pub source: String,
    /// MTGO's format code (`CMODERN`).
    pub format: Option<String>,
    /// The first game's start, else the match's (the snapshot's
    /// `played_at`, as 0.x `BuildReplaySnapshot`).
    pub started_at_ms: i64,
    /// The match account's username: the local seat in the frames.
    pub local_username: String,
    /// The match account's MTGO login id, when known.
    pub login_id: Option<i64>,
    /// The match's opponent and every other player name in its frames,
    /// each once ignoring ASCII case: the names a share redacts.
    pub opponent_names: Vec<String>,
    /// The match deck's archetype name (`decks.archetype_id`, 0.x
    /// `BuildReplaySnapshot`), `None` when the match has no deck or the deck
    /// no archetype.
    pub local_archetype: Option<String>,
    /// The opponent seat's `match_archetypes` name, `None` when unknown.
    pub opponent_archetype: Option<String>,
    pub share: Option<Share>,
    /// Every game by game number, frameless games included.
    pub games: Vec<ReplayGameInput>,
}

impl ReplayInputs {
    /// The snapshot builder's input (0.x `BuildReplaySnapshot`): every game,
    /// the log only for games with frames.
    pub fn snapshot_input(&self) -> SnapshotInput {
        let mut games = Vec::new();
        for game in &self.games {
            let log = if game.frames.is_empty() {
                Vec::new()
            } else {
                game.log.clone()
            };
            games.push(GameInput {
                won: game.won,
                local_username: self.local_username.clone(),
                frames: game.frames.clone(),
                log,
                sideboard: game.sideboard.clone(),
            });
        }
        SnapshotInput {
            format: self.format.clone(),
            played_at_ms: self.started_at_ms,
            local_archetype: self.local_archetype.clone(),
            opponent_archetype: self.opponent_archetype.clone(),
            games,
        }
    }
}

/// 0.x `ShowController::previousSideboard`: the nearest earlier game of
/// `games` (in play order) with a sideboard, as `(its number, its
/// sideboard)`. `None` for the first game, an unknown game, or when no
/// earlier game has one.
pub fn previous_sideboard(
    games: &[ReplayGameInput],
    game_id: i64,
) -> Option<(u32, Vec<(u32, u32)>)> {
    let at = games.iter().position(|game| game.id == game_id)?;
    for earlier in games[..at].iter().rev() {
        if !earlier.sideboard.is_empty() {
            return Some((earlier.number, earlier.sideboard.clone()));
        }
    }
    None
}

/// The match columns the replay reads.
struct Head {
    token: String,
    source: String,
    format: Option<String>,
    started_at_ms: i64,
    local_username: String,
    login_id: Option<i64>,
    opponent: Option<String>,
    share: Option<Share>,
}

fn read_head(conn: &Connection, match_id: i64) -> Result<Option<Head>, StoreError> {
    let head = conn
        .prepare_cached(
            "SELECT m.token, m.source, m.format, m.started_at, a.username, a.login_id,
                 o.username, m.replay_share_uuid, m.replay_share_url, m.replay_shared_at
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| {
            let uuid: Option<String> = row.get(7)?;
            let url: Option<String> = row.get(8)?;
            let shared_at_ms: Option<i64> = row.get(9)?;
            let share = match (uuid, url, shared_at_ms) {
                (Some(uuid), Some(url), Some(shared_at_ms)) => Some(Share {
                    uuid,
                    url,
                    shared_at_ms,
                }),
                _ => None,
            };
            Ok(Head {
                token: row.get(0)?,
                source: row.get(1)?,
                format: row.get(2)?,
                started_at_ms: row.get(3)?,
                local_username: row.get(4)?,
                login_id: row.get(5)?,
                opponent: row.get(6)?,
                share,
            })
        })
        .optional()?;
    Ok(head)
}

/// The archetype name of the deck match `match_id` was played with (0.x
/// `$match->deck?->archetype?->name`), through its deck version.
fn deck_archetype_name(conn: &Connection, match_id: i64) -> Result<Option<String>, StoreError> {
    let name: Option<Option<String>> = conn
        .prepare_cached(
            "SELECT a.name FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id
             JOIN decks d ON d.id = v.deck_id
             LEFT JOIN archetypes a ON a.id = d.archetype_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    Ok(name.flatten())
}

/// The replay inputs of match row `match_id`; `None` when there is none.
/// Log times are local to `tz`. `dat` is the entries of the match's
/// `Match_GameLog` `.dat` file when the caller found one (the store does no
/// file IO; the app passes the match page's `match_dat_entries`), empty
/// otherwise.
pub fn replay_inputs(
    conn: &Connection,
    match_id: i64,
    tz: Tz,
    dat: &[GameLogEntry],
) -> Result<Option<ReplayInputs>, StoreError> {
    let Some(head) = read_head(conn, match_id)? else {
        return Ok(None);
    };
    let rows = match_games(conn, match_id)?;
    let mut live = if head.source == "log" {
        live_events(conn, &head.token)?
    } else {
        LiveEvents::default()
    };

    // The match's game log (spec section 4, as the match page's
    // `page_log`): its stored `game_logs` row (the tracker's decoded `.dat`
    // file, or 0.x's), else the `.dat` entries passed in, else the
    // MetaMessages.
    let stored = match crate::game_sync::game_log(conn, match_id) {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!(%error, "a stored game log could not be read");
            None
        }
    };
    let stored_log = stored.unwrap_or_else(|| dat.to_vec());
    let meta_log = if stored_log.is_empty() {
        meta_message_entries(&live.meta)
    } else {
        Vec::new()
    };

    let mut games = Vec::new();
    for row in &rows {
        let texts = game_frame_texts(conn, &head.source, row, &mut live.frames);
        let frames = replay_frames(texts);
        let mut sideboard = local_sideboard(conn, row.id)?;
        if sideboard.is_empty() {
            sideboard = frames_sideboard(&frames, &head.local_username);
        }
        games.push(ReplayGameInput {
            id: row.id,
            number: row.game_number,
            won: game_won(&row.result),
            log: game_log(&stored_log, &meta_log, row, tz),
            frames,
            sideboard,
        });
    }

    let started_at_ms = rows
        .first()
        .and_then(|row| row.started_at_ms)
        .unwrap_or(head.started_at_ms);
    let opponent_names = opponent_names(head.opponent.as_deref(), &head.local_username, &games);

    Ok(Some(ReplayInputs {
        token: head.token,
        source: head.source,
        format: head.format,
        started_at_ms,
        local_username: head.local_username,
        login_id: head.login_id,
        opponent_names,
        local_archetype: deck_archetype_name(conn, match_id)?,
        opponent_archetype: crate::match_list::opponent_archetype(conn, match_id)?
            .map(|seat| seat.name),
        share: head.share,
        games,
    }))
}

/// The match of one game as the share dialog reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameShare {
    pub match_id: i64,
    pub token: String,
    /// The game's number in its match (the link's `?game=N`).
    pub game_number: u32,
    /// The match's shared link, when all three columns are set.
    pub share: Option<Share>,
}

/// The match, game number and shared link of game row `game_id`; `None`
/// when there is no such game. A cheap read: no frames, no log.
pub fn game_share(conn: &Connection, game_id: i64) -> Result<Option<GameShare>, StoreError> {
    let found = conn
        .prepare_cached(
            "SELECT m.id, m.token, g.game_number, m.replay_share_uuid, m.replay_share_url,
                 m.replay_shared_at
             FROM games g JOIN matches m ON m.id = g.match_id
             WHERE g.id = ?1",
        )?
        .query_row([game_id], |row| {
            let uuid: Option<String> = row.get(3)?;
            let url: Option<String> = row.get(4)?;
            let shared_at_ms: Option<i64> = row.get(5)?;
            let share = match (uuid, url, shared_at_ms) {
                (Some(uuid), Some(url), Some(shared_at_ms)) => Some(Share {
                    uuid,
                    url,
                    shared_at_ms,
                }),
                _ => None,
            };
            Ok(GameShare {
                match_id: row.get(0)?,
                token: row.get(1)?,
                game_number: row.get(2)?,
                share,
            })
        })
        .optional()?;
    Ok(found)
}

/// Whether game row `game_id` has a frame to replay (the Replay button):
/// never for a `manual` or `history` match. Decodes the whole match: for
/// every game of a match, read `games_with_frames` once instead.
#[cfg(test)]
fn game_has_frames(conn: &Connection, game_id: i64) -> Result<bool, StoreError> {
    let match_id: Option<i64> = conn
        .prepare_cached("SELECT match_id FROM games WHERE id = ?1")?
        .query_row([game_id], |row| row.get(0))
        .optional()?;
    let Some(match_id) = match_id else {
        return Ok(false);
    };
    Ok(games_with_frames(conn, match_id)?.contains(&game_id))
}

/// The game rows of match row `match_id` that have a frame to replay (the
/// Replay button on every game card), read in one pass: a `log` match's
/// events are decoded once for all its games. Empty for a `manual` or
/// `history` match and for an unknown one.
pub fn games_with_frames(conn: &Connection, match_id: i64) -> Result<BTreeSet<i64>, StoreError> {
    let mut found = BTreeSet::new();
    let head: Option<(String, String)> = conn
        .prepare_cached("SELECT source, token FROM matches WHERE id = ?1")?
        .query_row([match_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((source, token)) = head else {
        return Ok(found);
    };
    if source != "log" && source != "imported" {
        return Ok(found);
    }
    let mut live = if source == "log" {
        live_events(conn, &token)?.frames
    } else {
        HashMap::new()
    };
    for row in match_games(conn, match_id)? {
        let texts = game_frame_texts(conn, &source, &row, &mut live);
        if texts.iter().any(|frame| reads(&frame.text)) {
            found.insert(row.id);
        }
    }
    Ok(found)
}

/// The card rows (Plan S `cards`) of these catalog ids as the replay
/// decorates cards. An id without a row, or a stub without a name, is left
/// out. `other_image` is never set (0.x desktop never set it).
pub fn card_infos(
    conn: &Connection,
    catalog_ids: &[u32],
) -> Result<BTreeMap<u32, CardInfo>, StoreError> {
    let mut found = BTreeMap::new();
    for (catalog_id, card) in crate::decks::card_infos(conn, catalog_ids)? {
        found.insert(
            catalog_id,
            CardInfo {
                name: card.name,
                type_line: card.card_type,
                image: card.image,
                local_image: card.local_image,
                other_image: None,
            },
        );
    }
    Ok(found)
}

// ---- Frames ------------------------------------------------------------------

/// What the replay reads from a log match's events.
#[derive(Debug, Default)]
struct LiveEvents {
    /// Each game's frames by Game ID, in `(file, offset)` order.
    frames: HashMap<u64, Vec<FrameText>>,
    /// The MetaMessage game log events.
    meta: Vec<MetaEvent>,
}

/// A log match's frames and MetaMessages, live rows plus its archive.
fn live_events(conn: &Connection, token: &str) -> Result<LiveEvents, StoreError> {
    let mut found = LiveEvents::default();
    for stored in crate::read::events_for_match(conn, token)? {
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
                // A classified event always starts with its header, so every
                // frame here has its time; one without is skipped.
                let (Some(game_id), Some(text), Some(time)) = (
                    event.subjects.game_id,
                    frame_text(&event.text),
                    frame_time(&event.text),
                ) else {
                    continue;
                };
                found.frames.entry(game_id).or_default().push(FrameText {
                    time,
                    text: text.to_string(),
                });
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

/// One game's stored frame texts: its events' for a `log` match (taken out
/// of `live`), its timeline's for an `imported` one, none otherwise. A
/// timeline that cannot be read counts as none, as on the match page.
fn game_frame_texts(
    conn: &Connection,
    source: &str,
    row: &GameRow,
    live: &mut HashMap<u64, Vec<FrameText>>,
) -> Vec<FrameText> {
    match source {
        "log" => row
            .mtgo_game_id
            .and_then(|id| live.remove(&id))
            .unwrap_or_default(),
        "imported" => match crate::game_sync::timeline(conn, row.id) {
            Ok(frames) => frames.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "an imported game's frames could not be read");
                Vec::new()
            }
        },
        _ => Vec::new(),
    }
}

/// A game's replay frames from its stored texts: unreadable ones skipped,
/// then repeats dropped (`drop_repeats`: the first time and the game's last
/// frame kept), then each read whole.
fn replay_frames(texts: Vec<FrameText>) -> Vec<ReplayFrame> {
    let mut readable: Vec<FrameText> = Vec::with_capacity(texts.len());
    for frame in texts {
        // A repeat of a readable frame reads too: no need to check it again.
        let repeat = readable
            .last()
            .is_some_and(|before| before.text == frame.text);
        if repeat || reads(&frame.text) {
            readable.push(frame);
        }
    }
    let mut frames = Vec::new();
    for frame in drop_repeats(readable) {
        if let Some(content) = frame_content_value(&frame.text) {
            frames.push(ReplayFrame {
                timestamp: frame.time,
                content,
            });
        }
    }
    frames
}

/// Whether a frame's JSON text reads (its first JSON value), without
/// building it.
fn reads(text: &str) -> bool {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<serde::de::IgnoredAny>();
    matches!(stream.next(), Some(Ok(_)))
}

// ---- Games -------------------------------------------------------------------

/// One game's log lines (0.x `GetGameLogEntries`), `HH:MM:SS` in `tz`.
fn game_log(
    stored: &[GameLogEntry],
    meta: &[GameLogEntry],
    row: &GameRow,
    tz: Tz,
) -> Vec<LogEntry> {
    let mut lines = Vec::new();
    for entry in game_log_entries(stored, meta, row.started_at_ms, row.ended_at_ms) {
        lines.push(LogEntry {
            timestamp: local_time(entry.timestamp_ms, tz),
            message: entry.message,
        });
    }
    lines
}

fn local_time(ms: i64, tz: Tz) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(at) => at.with_timezone(&tz).format("%H:%M:%S").to_string(),
        None => String::new(),
    }
}

/// Your sideboard as the game began from its recorded deck (0.x
/// `GetLocalSideboard`): the local rows with sideboard copies.
fn local_sideboard(conn: &Connection, game_id: i64) -> Result<Vec<(u32, u32)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, side_qty FROM game_cards
         WHERE game_id = ?1 AND side = 'local' AND side_qty > 0 ORDER BY catalog_id",
    )?;
    let rows = stmt.query_map([game_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut sideboard = Vec::new();
    for row in rows {
        sideboard.push(row?);
    }
    Ok(sideboard)
}

/// 0.x `ShowController::sideboardFromFrames`: the local player's cards in
/// `Sideboard` (`Zone` or `ActualZone`) in the first frame holding any,
/// counted per catalog id in first-seen order. The local seat is the player
/// named `local_username`.
fn frames_sideboard(frames: &[ReplayFrame], local_username: &str) -> Vec<(u32, u32)> {
    for frame in frames {
        let local_seat = frame
            .content
            .get("Players")
            .and_then(Value::as_array)
            .and_then(|players| {
                players.iter().find(|player| {
                    player.get("Name").and_then(Value::as_str) == Some(local_username)
                })
            })
            .and_then(|player| player.get("Id"))
            .filter(|seat| !seat.is_null());
        let Some(local_seat) = local_seat else {
            continue;
        };
        let Some(cards) = frame.content.get("Cards").and_then(Value::as_array) else {
            continue;
        };
        let mut counts: Vec<(u32, u32)> = Vec::new();
        for card in cards {
            let in_sideboard = card.get("Zone").and_then(Value::as_str) == Some("Sideboard")
                || card.get("ActualZone").and_then(Value::as_str) == Some("Sideboard");
            if card.get("Owner") != Some(local_seat) || !in_sideboard {
                continue;
            }
            let Some(catalog_id) = card
                .get("CatalogID")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
            else {
                continue;
            };
            match counts.iter_mut().find(|(id, _)| *id == catalog_id) {
                Some((_, quantity)) => *quantity += 1,
                None => counts.push((catalog_id, 1)),
            }
        }
        if !counts.is_empty() {
            return counts;
        }
    }
    Vec::new()
}

/// The names a share redacts: the match's opponent, then every other
/// player name in the games' frames in first-seen order, each once ignoring
/// ASCII case (as the redaction matches). Blank names and the local
/// player's (in any case) are left out.
fn opponent_names(
    opponent: Option<&str>,
    local_username: &str,
    games: &[ReplayGameInput],
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if name.trim().is_empty() || name.eq_ignore_ascii_case(local_username) {
            return;
        }
        if !names.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            names.push(name.to_string());
        }
    };
    if let Some(opponent) = opponent {
        add(opponent);
    }
    for game in games {
        for frame in &game.frames {
            let Some(players) = frame.content.get("Players").and_then(Value::as_array) else {
                continue;
            };
            for player in players {
                if let Some(name) = player.get("Name").and_then(Value::as_str) {
                    add(name);
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_sync::store_timeline;
    use crate::test_support::{TOKEN, db, insert_manual, match_id, new_log_file, view};
    use crate::write::{MatchWrite, NewEvent, WriteBatch, commit, insert_log_file};
    use fixture_tools::{load_fixture, prepared_events};
    use mtgo_match::Projector;
    use mtgo_protocol::{EventKind, Subjects};
    use replay::{SideboardEntry, build_snapshot, prepare_shared_snapshot, validate_snapshot};
    use rusqlite::params;
    use serde_json::{Value, json};

    const UTC: Tz = chrono_tz::UTC;

    fn inputs(conn: &Connection, match_id: i64) -> ReplayInputs {
        replay_inputs(conn, match_id, UTC, &[])
            .expect("read")
            .expect("a match")
    }

    fn game_ids(conn: &Connection, match_id: i64) -> Vec<(i64, Option<u64>)> {
        let mut stmt = conn
            .prepare("SELECT id, mtgo_game_id FROM games WHERE match_id = ?1 ORDER BY game_number")
            .expect("prepare");
        let rows = stmt
            .query_map([match_id], |row| {
                let mtgo_id: Option<i64> = row.get(1)?;
                Ok((row.get(0)?, mtgo_id.map(|id| id as u64)))
            })
            .expect("games");
        rows.map(|row| row.expect("row")).collect()
    }

    /// A frame line as MTGO logs it, Match ID 99.
    fn frame_line(time: &str, game_id: u64, json: &str) -> String {
        format!(
            "{time} [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: 99, Event ID: 99) {json}"
        )
    }

    /// A board between Qplayer (seat 1) and `foe` (seat 2).
    fn board(foe: &str, hand: u32, life: u32, cards: &str) -> String {
        format!(
            r#"{{"Players":[{{"Id":1,"Name":"Qplayer","Life":{life},"HandCount":{hand},"LibraryCount":53}},{{"Id":2,"Name":"{foe}","Life":20,"HandCount":7,"LibraryCount":53}}],"Cards":[{cards}]}}"#
        )
    }

    /// A complete log match of Qplayer against Qfoe (Match ID 99, Game IDs
    /// 7, 8, 9) whose events are these frames `(Game ID, time, JSON)`, in
    /// this order.
    fn live_match(conn: &mut Connection, frames: &[(u64, &str, String)]) -> i64 {
        let file = insert_log_file(conn, &new_log_file("/q/mtgo.log")).expect("file");
        let mut played = view(TOKEN, "Qfoe", 1_000);
        played.local_username = "Qplayer".to_string();
        let mut events = Vec::new();
        for (index, (game_id, time, json)) in frames.iter().enumerate() {
            events.push(NewEvent {
                log_file_id: file,
                byte_offset: (index as u64 + 1) * 100,
                kind: Some(EventKind::GameStateUpdate),
                subjects: Subjects {
                    match_id: Some(99),
                    game_id: Some(*game_id),
                    ..Subjects::default()
                },
                logged_at_ms: 1_000 + index as i64,
                raw: frame_line(time, *game_id, json),
                quarantined: false,
            });
        }
        commit(
            conn,
            &WriteBatch {
                events,
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(played),
                    revision: 1,
                }],
                now_ms: 1_000,
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        match_id(conn, TOKEN)
    }

    /// A complete match of Qplayer (login id 4242) against Qfoe from
    /// `source`, format `CModern`, started at 1_000, no games.
    fn stored_match(conn: &Connection, token: &str, source: &str) -> i64 {
        conn.execute_batch(
            "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qplayer', 4242, 0)
                 ON CONFLICT (username) DO NOTHING;
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0)
                 ON CONFLICT (username) DO NOTHING;",
        )
        .expect("registries");
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, ended_at, last_activity_at, revision,
                 derived_version)
             VALUES (?1, (SELECT id FROM accounts WHERE username = 'Qplayer'),
                 (SELECT id FROM opponents WHERE username = 'Qfoe'), ?2, 'CModern',
                 'Constructed', 'complete', 'win', 1000, 90000, 90000, 1, 0)",
            params![token, source],
        )
        .expect("a match");
        match_id(conn, token)
    }

    fn add_game(
        conn: &Connection,
        match_id: i64,
        number: u32,
        result: &str,
        started_at: Option<i64>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, started_at)
             VALUES (?1, ?2, ?3, 'unknown', ?4)",
            params![match_id, number, result, started_at],
        )
        .expect("a game");
        conn.last_insert_rowid()
    }

    fn timeline(conn: &Connection, game_id: i64, frames: &[(&str, String)]) {
        let texts: Vec<FrameText> = frames
            .iter()
            .map(|(time, text)| FrameText {
                time: time.to_string(),
                text: text.clone(),
            })
            .collect();
        let tx = conn.unchecked_transaction().expect("tx");
        store_timeline(&tx, game_id, &texts).expect("timeline");
        tx.commit().expect("commit");
    }

    fn card(conn: &Connection, catalog_id: u32, name: &str, type_line: &str, image: Option<&str>) {
        conn.execute(
            "INSERT INTO cards (catalog_id, name, type, image) VALUES (?1, ?2, ?3, ?4)",
            params![catalog_id, name, type_line, image],
        )
        .expect("card");
    }

    fn local_card(conn: &Connection, game_id: i64, catalog_id: u32, main: u32, side: u32) {
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty)
             VALUES (?1, 'local', ?2, ?3, ?4)",
            params![game_id, catalog_id, main, side],
        )
        .expect("game card");
    }

    fn times(frames: &[ReplayFrame]) -> Vec<&str> {
        frames
            .iter()
            .map(|frame| frame.timestamp.as_str())
            .collect()
    }

    // ---- The fixture log ----------------------------------------------------

    /// The draft_hobbit fixture stored as the tracker stores it (events and
    /// projected matches); the log matches' row ids.
    fn hobbit(conn: &mut Connection) -> Vec<i64> {
        let fixture = load_fixture("draft_hobbit");
        let mut file = new_log_file("Q/mtgo.log");
        file.inherited_username = Some(fixture.meta.local_username.clone());
        let file_id = insert_log_file(conn, &file).expect("file");
        let mut projector = Projector::new(vec![fixture.meta.local_username.clone()]);
        let mut events = Vec::new();
        for event in prepared_events(&fixture) {
            projector.apply(&event);
            events.push(NewEvent {
                log_file_id: file_id,
                byte_offset: event.offset,
                kind: Some(event.kind),
                subjects: event.subjects.clone(),
                logged_at_ms: event.at_ms,
                raw: event.text.clone(),
                quarantined: false,
            });
        }
        projector.finalize(fixture.meta.now_ms());
        let mut matches = Vec::new();
        for (index, view) in projector.views().into_iter().enumerate() {
            matches.push(MatchWrite::Upsert {
                view: Box::new(view.clone()),
                revision: index as u64 + 1,
            });
        }
        commit(
            conn,
            &WriteBatch {
                events,
                matches,
                now_ms: fixture.meta.now_ms(),
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        let mut stmt = conn
            .prepare("SELECT id FROM matches WHERE source = 'log' ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("ids")
            .map(|row| row.expect("row"))
            .collect()
    }

    /// The fixture's frames of one Game ID as the replay must give them,
    /// worked out here without the store: in log order, unreadable ones
    /// skipped, a repeat of the frame before dropped unless it is the last.
    fn expected_frames(game_id: u64) -> Vec<ReplayFrame> {
        let fixture = load_fixture("draft_hobbit");
        let mut readable: Vec<(String, String)> = Vec::new();
        for event in prepared_events(&fixture) {
            if event.kind != EventKind::GameStateUpdate || event.subjects.game_id != Some(game_id) {
                continue;
            }
            if game_detail::frame_value(&event.text).is_none() {
                continue;
            }
            let time = game_detail::frame_time(&event.text).expect("a header time");
            let text = game_detail::frame_text(&event.text)
                .expect("json")
                .to_string();
            readable.push((time, text));
        }
        let mut frames = Vec::new();
        let mut last_text: Option<String> = None;
        let count = readable.len();
        for (index, (time, text)) in readable.into_iter().enumerate() {
            if last_text.as_deref() == Some(text.as_str()) && index + 1 < count {
                continue;
            }
            frames.push(ReplayFrame {
                timestamp: time,
                content: serde_json::from_str::<Value>(&text).expect("json"),
            });
            last_text = Some(text);
        }
        frames
    }

    #[test]
    fn a_live_fixture_match_gives_its_frames_in_log_order() {
        let (_dir, mut conn) = db("replay-live-fixture");
        let local = load_fixture("draft_hobbit").meta.local_username;
        let matches = hobbit(&mut conn);
        assert!(!matches.is_empty());
        let mut with_frames = 0;
        for match_id in matches {
            let read = inputs(&conn, match_id);
            assert_eq!(read.source, "log");
            assert_eq!(read.local_username, local);
            let ids = game_ids(&conn, match_id);
            assert_eq!(read.games.len(), ids.len());
            for (game, (id, mtgo_id)) in read.games.iter().zip(ids) {
                assert_eq!(game.id, id);
                let expected = mtgo_id.map(expected_frames).unwrap_or_default();
                assert_eq!(game.frames, expected, "game {id}");
                assert_eq!(
                    game_has_frames(&conn, id).expect("has"),
                    !expected.is_empty()
                );
                if !game.frames.is_empty() {
                    with_frames += 1;
                    for frame in &game.frames {
                        let names: Vec<&str> = frame.content["Players"]
                            .as_array()
                            .expect("players")
                            .iter()
                            .filter_map(|player| player["Name"].as_str())
                            .collect();
                        assert!(names.contains(&local.as_str()), "{names:?}");
                    }
                    for pair in game.frames.windows(2) {
                        assert!(pair[0].timestamp <= pair[1].timestamp, "log order");
                    }
                }
            }
        }
        assert!(with_frames >= 2, "{with_frames} games with frames");
    }

    #[test]
    fn an_archived_match_gives_the_same_frames() {
        let (_dir, mut conn) = db("replay-archived");
        let matches = hobbit(&mut conn);
        let before: Vec<ReplayInputs> = matches.iter().map(|id| inputs(&conn, *id)).collect();
        let mut moved = 0;
        for read in &before {
            moved +=
                crate::compact::compact_match(&conn, &read.token, i64::MAX / 2).expect("compact");
        }
        assert!(moved > 0, "events archived");
        let frames_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM log_events WHERE kind = 'game_state_update'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(frames_left, 0, "every frame is in an archive");
        let after: Vec<ReplayInputs> = matches.iter().map(|id| inputs(&conn, *id)).collect();
        assert_eq!(after, before);
    }

    #[test]
    fn an_imported_match_gives_its_frames_with_times() {
        let (_dir, conn) = db("replay-imported");
        let match_id = stored_match(&conn, "q-imported", "imported");
        let first = add_game(&conn, match_id, 1, "win", Some(5_000));
        let second = add_game(&conn, match_id, 2, "loss", Some(9_000));
        timeline(
            &conn,
            first,
            &[
                ("18:00:00", board("Qfoe", 7, 20, "")),
                ("18:00:01.500", board("Qfoe", 6, 20, "")),
            ],
        );
        let read = inputs(&conn, match_id);
        assert_eq!(read.source, "imported");
        assert_eq!(read.token, "q-imported");
        assert_eq!(read.format.as_deref(), Some("CModern"));
        assert_eq!(read.local_username, "Qplayer");
        assert_eq!(read.login_id, Some(4242));
        assert_eq!(read.started_at_ms, 5_000, "the first game's start");
        assert_eq!(read.share, None);
        assert_eq!(
            (
                read.local_archetype.clone(),
                read.opponent_archetype.clone()
            ),
            (None, None)
        );
        assert_eq!(read.opponent_names, vec!["Qfoe".to_string()]);
        assert_eq!(
            times(&read.games[0].frames),
            vec!["18:00:00", "18:00:01.500"]
        );
        assert_eq!(
            read.games[0].frames[1].content["Players"][0]["HandCount"],
            6
        );
        assert!(read.games[1].frames.is_empty());
        assert!(game_has_frames(&conn, first).expect("has"));
        assert!(!game_has_frames(&conn, second).expect("has"));

        conn.execute(
            "UPDATE matches SET replay_share_uuid = 'Q-uuid', replay_share_url = 'https://mymtgo.test/r/Q',
                 replay_shared_at = 77 WHERE id = ?1",
            [match_id],
        )
        .expect("share");
        assert_eq!(
            inputs(&conn, match_id).share,
            Some(Share {
                uuid: "Q-uuid".to_string(),
                url: "https://mymtgo.test/r/Q".to_string(),
                shared_at_ms: 77,
            })
        );
        assert_eq!(replay_inputs(&conn, 9_999, UTC, &[]).expect("read"), None);
    }

    /// A manual archetype named `name` in Modern.
    fn archetype(conn: &Connection, uuid: &str, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?2, 'modern', 'manual', 1, 1)",
            params![uuid, name],
        )
        .expect("an archetype");
        conn.last_insert_rowid()
    }

    // 0.x `BuildReplaySnapshot`: `local_archetype` is the match deck's
    // archetype name, `opponent_archetype` the opponent seat's; each `None`
    // when unknown.
    #[test]
    fn names_the_deck_archetype_and_the_opponent_seat_archetype() {
        let (_dir, conn) = db("replay-archetypes");
        let match_id = stored_match(&conn, "q-archetypes", "imported");
        let names = |conn: &Connection| {
            let read = inputs(conn, match_id);
            let input = read.snapshot_input();
            assert_eq!(
                (
                    input.local_archetype.clone(),
                    input.opponent_archetype.clone()
                ),
                (
                    read.local_archetype.clone(),
                    read.opponent_archetype.clone()
                )
            );
            (read.local_archetype, read.opponent_archetype)
        };
        assert_eq!(names(&conn), (None, None), "no deck, no seats");

        let boros = archetype(&conn, "q-boros", "Qboros Energy");
        let phoenix = archetype(&conn, "q-phoenix", "Qizzet Phoenix");
        let rogue = archetype(&conn, "q-rogue", "Qrogue");
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (7, 'Q-deck-7', 'Qdeck', 'modern', 1, 1, 'file')",
            [],
        )
        .expect("a deck");
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (70, 7, 'Q-sig-7', 1)",
            [],
        )
        .expect("a deck version");
        conn.execute(
            "UPDATE matches SET deck_version_id = 70 WHERE id = ?1",
            [match_id],
        )
        .expect("the deck link");
        assert_eq!(names(&conn), (None, None), "a deck without an archetype");

        // The player seat's own row is not the local name: the deck's is.
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES (?1, 0, ?2, 1.0)",
            params![match_id, rogue],
        )
        .expect("the player seat");
        assert_eq!(names(&conn), (None, None), "only the player seat");

        conn.execute("UPDATE decks SET archetype_id = ?1 WHERE id = 7", [boros])
            .expect("the deck archetype");
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES (?1, 1, ?2, 0.5)",
            params![match_id, phoenix],
        )
        .expect("the opponent seat");
        assert_eq!(
            names(&conn),
            (
                Some("Qboros Energy".to_string()),
                Some("Qizzet Phoenix".to_string())
            )
        );
    }

    #[test]
    fn manual_and_history_matches_have_no_frames() {
        let (_dir, conn) = db("replay-manual-history");
        insert_manual(&conn, "q-manual");
        let manual = match_id(&conn, "q-manual");
        let history = stored_match(&conn, "q-history", "history");
        let history_game = add_game(&conn, history, 1, "win", Some(1_000));
        // Even a stored timeline is not a history match's replay.
        timeline(
            &conn,
            history_game,
            &[("18:00:00", board("Qfoe", 7, 20, ""))],
        );
        for match_id in [manual, history] {
            let read = inputs(&conn, match_id);
            assert!(!read.games.is_empty());
            for game in &read.games {
                assert!(game.frames.is_empty());
                assert!(!game_has_frames(&conn, game.id).expect("has"));
            }
        }
        assert!(!game_has_frames(&conn, 9_999).expect("unknown game"));
        for match_id in [manual, history, 9_999] {
            assert!(games_with_frames(&conn, match_id).expect("read").is_empty());
        }
    }

    #[test]
    fn games_with_frames_agrees_with_each_game_in_one_read() {
        let (_dir, mut conn) = db("replay-games-with-frames");
        let mut found_any = false;
        for match_id in hobbit(&mut conn) {
            // Independently: the games whose replay has frames.
            let with = games_with_frames(&conn, match_id).expect("read");
            for game in inputs(&conn, match_id).games {
                let has = !game.frames.is_empty();
                assert_eq!(with.contains(&game.id), has, "game {}", game.id);
                found_any |= has;
            }
        }
        assert!(found_any, "the fixture has games with frames");

        // Imported: a readable timeline, an unreadable one, none.
        let imported = stored_match(&conn, "q-imported-frames", "imported");
        let readable = add_game(&conn, imported, 1, "win", Some(1_000));
        let unreadable = add_game(&conn, imported, 2, "loss", Some(2_000));
        let frameless = add_game(&conn, imported, 3, "win", Some(3_000));
        timeline(&conn, readable, &[("18:00:00", board("Qfoe", 7, 20, ""))]);
        timeline(&conn, unreadable, &[("18:00:00", "{not json".to_string())]);
        let with = games_with_frames(&conn, imported).expect("read");
        assert_eq!(with, BTreeSet::from([readable]));
        assert!(!with.contains(&unreadable));
        assert!(!with.contains(&frameless));
    }

    // ---- Repeated frames (ruling 4) -----------------------------------------

    /// One game's frames with same-second repeats, later-second repeats and
    /// a trailing repeat.
    fn repeat_frames() -> Vec<(&'static str, String)> {
        let a = board("Qfoe", 7, 20, "");
        let b = board(
            "Qfoe",
            6,
            20,
            r#"{"Id":10,"CatalogID":1234,"Zone":"Battlefield","Owner":1}"#,
        );
        let c = board(
            "Qfoe",
            6,
            18,
            r#"{"Id":10,"CatalogID":1234,"Zone":"Battlefield","Owner":1}"#,
        );
        let d = board(
            "Qfoe",
            5,
            18,
            r#"{"Id":10,"CatalogID":1234,"Zone":"Graveyard","Owner":1}"#,
        );
        let e = board(
            "Qfoe",
            5,
            15,
            r#"{"Id":10,"CatalogID":1234,"Zone":"Graveyard","Owner":1}"#,
        );
        vec![
            ("10:00:00", a.clone()),
            ("10:00:00", a),
            ("10:00:01", b.clone()),
            ("10:00:03", b),
            ("10:00:05", c),
            ("10:00:05", d.clone()),
            ("10:00:05", d),
            ("10:00:08", e.clone()),
            ("10:00:09", e.clone()),
            ("10:00:12", e),
        ]
    }

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../ui/src/replay/__fixtures__")
            .join(name)
    }

    /// The checked-in fixture `name` equals `frames`; with
    /// `REPLAY_FIXTURES_UPDATE=1` it is written instead.
    fn check_fixture(name: &str, frames: &[ReplayFrame]) {
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(frames).expect("fixture json")
        );
        let path = fixture_path(name);
        if std::env::var("REPLAY_FIXTURES_UPDATE").is_ok_and(|value| value == "1") {
            std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
            std::fs::write(&path, &text).expect("write fixture");
        }
        let stored = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            stored, text,
            "{name} differs from the store; rerun with REPLAY_FIXTURES_UPDATE=1 and review"
        );
    }

    #[test]
    fn repeated_frames_are_dropped_keeping_the_first_time_and_the_last_frame() {
        let frames = repeat_frames();
        let all: Vec<ReplayFrame> = frames
            .iter()
            .map(|(time, text)| ReplayFrame {
                timestamp: time.to_string(),
                content: serde_json::from_str(text).expect("json"),
            })
            .collect();

        // Live: the match's events.
        let (_dir, mut conn) = db("replay-repeats-live");
        let live: Vec<(u64, &str, String)> = frames
            .iter()
            .map(|(time, text)| (7, *time, text.clone()))
            .collect();
        let live_id = live_match(&mut conn, &live);
        let live_frames = inputs(&conn, live_id).games[0].frames.clone();

        // Imported: the game's timeline.
        let imported_id = stored_match(&conn, "q-imported", "imported");
        let game = add_game(&conn, imported_id, 1, "win", Some(1_000));
        timeline(&conn, game, &frames);
        let imported_frames = inputs(&conn, imported_id).games[0].frames.clone();

        assert_eq!(
            times(&live_frames),
            vec![
                "10:00:00", "10:00:01", "10:00:05", "10:00:05", "10:00:08", "10:00:12"
            ]
        );
        assert_eq!(imported_frames, live_frames, "live and imported alike");
        assert_eq!(live_frames.last(), all.last(), "the last frame is kept");
        check_fixture("repeats-all.json", &all);
        check_fixture("repeats-dropped.json", &live_frames);
    }

    // ---- Redaction names ----------------------------------------------------

    #[test]
    fn every_frame_name_of_the_match_is_redacted() {
        let (_dir, mut conn) = db("replay-names");
        let match_id = live_match(
            &mut conn,
            &[
                (7, "10:00:00", board("Qfoe", 7, 20, "")),
                (8, "10:10:00", board("Qghost", 7, 20, "")),
                (9, "10:20:00", board("QFOE", 7, 20, "")),
            ],
        );
        let read = inputs(&conn, match_id);
        assert_eq!(
            read.opponent_names,
            vec!["Qfoe".to_string(), "Qghost".to_string()]
        );
        let snapshot = build_snapshot(read.snapshot_input(), &|_ids: &[u32]| BTreeMap::new());
        let prepared = prepare_shared_snapshot(snapshot, &read.opponent_names).expect("prepared");
        let text = String::from_utf8(prepared.into_json())
            .expect("utf8")
            .to_ascii_lowercase();
        assert!(!text.contains("qghost"), "{text}");
        assert!(!text.contains("qfoe"), "{text}");
        assert!(text.contains("qplayer"));
    }

    // ---- 0.x ShowControllerTest ---------------------------------------------

    #[test]
    fn lists_the_match_games_in_play_order_so_the_replay_can_offer_the_next_one() {
        let (_dir, conn) = db("replay-match-games");
        let match_id = stored_match(&conn, "q-games", "imported");
        let third = add_game(&conn, match_id, 3, "win", Some(3_000));
        let first = add_game(&conn, match_id, 1, "win", Some(1_000));
        let second = add_game(&conn, match_id, 2, "loss", Some(2_000));
        let other = stored_match(&conn, "q-other", "imported");
        add_game(&conn, other, 1, "win", Some(1_500));

        let read = inputs(&conn, match_id);
        let games: Vec<(i64, u32, Option<bool>)> = read
            .games
            .iter()
            .map(|game| (game.id, game.number, game.won))
            .collect();
        assert_eq!(
            games,
            vec![
                (first, 1, Some(true)),
                (second, 2, Some(false)),
                (third, 3, Some(true))
            ]
        );
        assert_eq!(
            (read.local_archetype, read.opponent_archetype),
            (None, None)
        );
    }

    /// 0.x `sideboardedMatch`: two games of Qplayer (seat 1) against Qfoe
    /// (seat 2); game 1 begins with Duress and Abrade in your sideboard,
    /// game 2 with two Duress and a Thoughtseize. The recorded deck
    /// (`game_cards`) holds the sideboard when `in_deck`; the second frame
    /// holds it when `in_frames`.
    fn sideboarded_match(conn: &Connection, in_frames: bool, in_deck: bool) -> Vec<i64> {
        let match_id = stored_match(conn, "q-sideboarded", "imported");
        for (catalog_id, name) in [
            (101, "Duress"),
            (102, "Abrade"),
            (103, "Thoughtseize"),
            (104, "Ragavan"),
        ] {
            card(conn, catalog_id, name, "Sorcery", None);
        }
        let mut games = Vec::new();
        for (index, sideboard) in [vec![(101, 1), (102, 1)], vec![(101, 2), (103, 1)]]
            .into_iter()
            .enumerate()
        {
            let game = add_game(
                conn,
                match_id,
                index as u32 + 1,
                "win",
                Some(1_000 + index as i64),
            );
            let mut cards =
                vec![r#"{"Id":50,"CatalogID":900,"Zone":"Sideboard","Owner":2}"#.to_string()];
            if in_deck {
                local_card(conn, game, 104, 4, 0);
            }
            for (catalog_id, quantity) in sideboard {
                if in_deck {
                    local_card(conn, game, catalog_id, 0, quantity);
                }
                for _ in 0..quantity {
                    let id = cards.len() + index * 10;
                    cards.push(format!(
                        r#"{{"Id":{id},"CatalogID":{catalog_id},"Zone":"Sideboard","Owner":1}}"#
                    ));
                }
            }
            let shown = if in_frames {
                cards.join(",")
            } else {
                String::new()
            };
            timeline(
                conn,
                game,
                &[
                    ("18:00:00", board("Qfoe", 7, 20, "")),
                    ("18:00:01", board("Qfoe", 7, 20, &shown)),
                ],
            );
            games.push(game);
        }
        games
    }

    fn sideboard_names(conn: &Connection, sideboard: &[(u32, u32)]) -> Vec<String> {
        let ids: Vec<u32> = sideboard.iter().map(|(id, _)| *id).collect();
        let cards = card_infos(conn, &ids).expect("cards");
        let mut names: Vec<String> = sideboard
            .iter()
            .map(|(id, quantity)| {
                let name = cards
                    .get(id)
                    .and_then(|card| card.name.clone())
                    .unwrap_or_default();
                format!("{quantity} {name}")
            })
            .collect();
        names.sort();
        names
    }

    fn assert_sideboards(in_frames: bool, in_deck: bool) {
        let (_dir, conn) = db(&format!("replay-sideboard-{in_frames}-{in_deck}"));
        let games = sideboarded_match(&conn, in_frames, in_deck);
        let match_id = match_id(&conn, "q-sideboarded");
        let read = inputs(&conn, match_id);
        assert_eq!(
            sideboard_names(&conn, &read.games[1].sideboard),
            vec!["1 Thoughtseize", "2 Duress"]
        );
        let (number, previous) = previous_sideboard(&read.games, games[1]).expect("previous");
        assert_eq!(number, 1);
        assert_eq!(
            sideboard_names(&conn, &previous),
            vec!["1 Abrade", "1 Duress"]
        );
    }

    #[test]
    fn gives_a_game_your_recorded_sideboard_even_when_its_frames_hold_none() {
        assert_sideboards(false, true);
    }

    #[test]
    fn falls_back_to_the_frames_when_no_deck_was_recorded() {
        assert_sideboards(true, false);
    }

    #[test]
    fn gives_the_first_game_no_previous_sideboard() {
        let (_dir, conn) = db("replay-sideboard-first");
        let games = sideboarded_match(&conn, false, true);
        let read = inputs(&conn, match_id(&conn, "q-sideboarded"));
        assert_eq!(previous_sideboard(&read.games, games[0]), None);
        assert_eq!(previous_sideboard(&read.games, 9_999), None);
    }

    // ---- 0.x BuildReplaySnapshotTest ----------------------------------------

    fn snapshot_of(conn: &Connection, match_id: i64) -> replay::Snapshot {
        let read = inputs(conn, match_id);
        build_snapshot(read.snapshot_input(), &|ids: &[u32]| {
            card_infos(conn, ids).expect("cards")
        })
    }

    /// 0.x `snapshotMatch`: a three-game modern match, game 1 lost, game 2
    /// won, game 3 without frames; game 1 has a recorded deck.
    fn snapshot_match(conn: &Connection) -> i64 {
        let match_id = stored_match(conn, "a1b2c3d4-0000-4000-8000-000000000001", "imported");
        let games = [
            add_game(conn, match_id, 1, "loss", Some(1_000)),
            add_game(conn, match_id, 2, "win", Some(2_000)),
            add_game(conn, match_id, 3, "unknown", Some(3_000)),
        ];
        for game in &games[..2] {
            timeline(conn, *game, &[("18:00:01", board("Qfoe", 7, 20, ""))]);
        }
        local_card(conn, games[0], 1234, 4, 0);
        local_card(conn, games[0], 5678, 0, 2);
        match_id
    }

    #[test]
    fn builds_a_match_snapshot_the_package_accepts() {
        let (_dir, conn) = db("replay-snapshot-3");
        let match_id = snapshot_match(&conn);
        let snapshot = snapshot_of(&conn, match_id);
        let value = serde_json::to_value(&snapshot).expect("json");
        assert_eq!(validate_snapshot(&value), Ok(()));
        assert_eq!(snapshot.version, 2);
        // Game 3 has no frames, so two games travel, keeping their numbers.
        let numbers: Vec<u32> = snapshot.games.iter().map(|game| game.game_number).collect();
        assert_eq!(numbers, vec![1, 2]);
        let won: Vec<Option<bool>> = snapshot.games.iter().map(|game| game.won).collect();
        assert_eq!(won, vec![Some(false), Some(true)]);
        assert_eq!(snapshot.meta.games_in_match, 3);
        assert_eq!(snapshot.meta.format.as_deref(), Some("modern"));
        assert_eq!(
            snapshot.games[0].sideboard,
            Some(vec![SideboardEntry {
                catalog_id: 5678,
                quantity: 2,
                name: None,
                type_line: None,
                image: None,
            }])
        );
        assert_eq!(snapshot.games[1].sideboard, None);
        assert!(
            !value["games"][1]
                .as_object()
                .expect("game")
                .contains_key("sideboard")
        );
    }

    #[test]
    fn builds_the_snapshot_the_desktop_has_always_shared() {
        let (_dir, conn) = db("replay-snapshot-shape");
        card(
            &conn,
            1234,
            "Ragavan, Nimble Pilferer",
            "Legendary Creature",
            Some("https://cards.scryfall.io/a.jpg"),
        );
        card(&conn, 5678, "Island", "Basic Land", None);
        let match_id = stored_match(&conn, "q-replay", "imported");
        let first = add_game(&conn, match_id, 1, "win", Some(1_000));
        add_game(&conn, match_id, 2, "loss", Some(2_000));
        let cards = r#"{"Id":10,"CatalogID":1234,"Zone":"Battlefield","Owner":2},{"Id":11,"CatalogID":5678,"Zone":"Battlefield","Owner":1}"#;
        let content = format!(
            r#"{{"Players":[{{"Id":1,"Name":"Qplayer"}},{{"Id":2,"Name":"Qfoe"}}],"Cards":[{cards}]}}"#
        );
        timeline(&conn, first, &[("18:00:01", content)]);

        let snapshot = serde_json::to_value(snapshot_of(&conn, match_id)).expect("json");
        assert_eq!(snapshot["version"], 2);
        assert_eq!(snapshot["meta"]["format"], "modern");
        assert_eq!(snapshot["meta"]["games_in_match"], 2);
        assert_eq!(snapshot["games"].as_array().expect("games").len(), 1);
        let game = &snapshot["games"][0];
        assert_eq!(game["game_number"], 1);
        assert_eq!(game["won"], true);
        let frame = &game["frames"][0];
        assert_eq!(frame["timestamp"], "18:00:01");
        assert_eq!(frame["content"]["Players"][0]["IsLocal"], true);
        assert_eq!(frame["content"]["Players"][1]["IsLocal"], false);
        assert_eq!(
            frame["content"]["Cards"][0],
            json!({
                "Id": 10, "CatalogID": 1234, "Zone": "Battlefield", "Owner": 2,
                "image": "https://cards.scryfall.io/a.jpg", "type": "Legendary Creature",
                "name": "Ragavan, Nimble Pilferer",
            })
        );
        assert_eq!(frame["content"]["Cards"][1]["image"], Value::Null);
    }

    // ---- Game logs and cards ------------------------------------------------

    #[test]
    fn the_game_log_is_the_stored_one_in_local_time_within_the_game() {
        let (_dir, conn) = db("replay-game-log");
        let match_id = stored_match(&conn, "q-log", "imported");
        let game = add_game(&conn, match_id, 1, "win", Some(10_000));
        conn.execute("UPDATE games SET ended_at = 60000 WHERE id = ?1", [game])
            .expect("end");
        timeline(&conn, game, &[("00:00:10", board("Qfoe", 7, 20, ""))]);
        let tx = conn.unchecked_transaction().expect("tx");
        crate::game_sync::store_game_log(
            &tx,
            match_id,
            &[
                game_detail::GameLogEntry {
                    timestamp_ms: 1_000,
                    message: "@PQplayer early.".to_string(),
                },
                game_detail::GameLogEntry {
                    timestamp_ms: 20_000,
                    message: "@PQplayer casts @[Duress@:101,1:@].".to_string(),
                },
                game_detail::GameLogEntry {
                    timestamp_ms: 70_000,
                    message: "@PQfoe late.".to_string(),
                },
            ],
        )
        .expect("log");
        tx.commit().expect("commit");
        let london: Tz = "Europe/London".parse().expect("tz");
        let read = replay_inputs(&conn, match_id, london, &[])
            .expect("read")
            .expect("match");
        assert_eq!(
            read.games[0].log,
            vec![LogEntry {
                timestamp: "01:00:20".to_string(),
                message: "Qplayer casts Duress.".to_string(),
            }]
        );
    }

    /// A chat MetaMessage (0.x's test envelope: u32 length, type 3, header
    /// bytes, u32 text length, then the text) as a `game_management_json`
    /// line's byte list.
    fn chat_line(text: &str) -> String {
        let len = text.len() as u8;
        let mut bytes = vec![len + 24, 0, 0, 0, 3, 17, 186, 129];
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&[len, 0, 0, 0]);
        bytes.extend_from_slice(text.as_bytes());
        let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
        format!(
            "10:00:00 [INF] (Game Management|Processing) Message: {{\"MatchToken\":\"{TOKEN}\",\"MatchID\":99,\"GameID\":7,\"MetaMessage\":[{}]}}",
            list.join(",")
        )
    }

    #[test]
    fn the_game_log_is_the_stored_one_else_the_dat_file_else_the_metamessages() {
        let (_dir, mut conn) = db("replay-game-log-order");
        // Game 7 runs from 1_000 to 2_000 (`test_support::game`).
        let match_id = live_match(&mut conn, &[(7, "00:00:01", board("Qfoe", 7, 20, ""))]);
        commit(
            &mut conn,
            &WriteBatch {
                events: vec![NewEvent {
                    log_file_id: 1,
                    byte_offset: 50_000,
                    kind: Some(EventKind::GameManagementJson),
                    subjects: Subjects {
                        match_token: Some(TOKEN.to_string()),
                        match_id: Some(99),
                        game_id: Some(7),
                        ..Subjects::default()
                    },
                    logged_at_ms: 1_500,
                    raw: chat_line("@PQplayer rolled a 2."),
                    quarantined: false,
                }],
                now_ms: 1_500,
                ..WriteBatch::default()
            },
        )
        .expect("meta event");
        let messages = |dat: &[game_detail::GameLogEntry]| -> Vec<String> {
            replay_inputs(&conn, match_id, UTC, dat)
                .expect("read")
                .expect("a match")
                .games[0]
                .log
                .iter()
                .map(|entry| entry.message.clone())
                .collect()
        };
        let dat = vec![game_detail::GameLogEntry {
            timestamp_ms: 1_800,
            message: "@PQplayer casts @[Duress@:101,1:@].".to_string(),
        }];

        // No stored log and no `.dat` file: the MetaMessages.
        assert_eq!(messages(&[]), vec!["Qplayer rolled a 2."]);
        // A `.dat` file: its lines, not the MetaMessages.
        assert_eq!(messages(&dat), vec!["Qplayer casts Duress."]);
        // A stored log wins over both.
        let tx = conn.unchecked_transaction().expect("tx");
        crate::game_sync::store_game_log(
            &tx,
            match_id,
            &[game_detail::GameLogEntry {
                timestamp_ms: 1_200,
                message: "@PQfoe wins the game.".to_string(),
            }],
        )
        .expect("stored log");
        tx.commit().expect("commit");
        assert_eq!(messages(&dat), vec!["Qfoe wins the game."]);
    }

    #[test]
    fn card_infos_are_the_card_rows() {
        let (_dir, conn) = db("replay-card-infos");
        card(
            &conn,
            1234,
            "Ragavan, Nimble Pilferer",
            "Legendary Creature",
            Some("https://cards.scryfall.io/a.jpg"),
        );
        conn.execute(
            "UPDATE cards SET local_image = '1234.jpg' WHERE catalog_id = 1234",
            [],
        )
        .expect("local");
        conn.execute("INSERT INTO cards (catalog_id) VALUES (77)", [])
            .expect("a stub");
        let found = card_infos(&conn, &[1234, 77, 5]).expect("cards");
        assert_eq!(
            found,
            BTreeMap::from([(
                1234,
                CardInfo {
                    name: Some("Ragavan, Nimble Pilferer".to_string()),
                    type_line: Some("Legendary Creature".to_string()),
                    image: Some("https://cards.scryfall.io/a.jpg".to_string()),
                    local_image: Some("1234.jpg".to_string()),
                    other_image: None,
                }
            )])
        );
    }

    // ---- Share state (replays spec section 8) -------------------------------

    #[test]
    fn a_game_share_is_its_match_link_and_its_number() {
        let (_dir, conn) = db("replay-game-share");
        let match_id = stored_match(&conn, "q-share", "imported");
        let first = add_game(&conn, match_id, 1, "win", Some(1_000));
        let second = add_game(&conn, match_id, 2, "loss", Some(2_000));
        let found = game_share(&conn, second).expect("read").expect("a game");
        assert_eq!(
            found,
            GameShare {
                match_id,
                token: "q-share".to_string(),
                game_number: 2,
                share: None,
            }
        );
        conn.execute(
            "UPDATE matches SET replay_share_uuid = 'q-uuid',
                 replay_share_url = 'https://mymtgo.test/players/Qplayer/replays/q-uuid',
                 replay_shared_at = 5000 WHERE id = ?1",
            [match_id],
        )
        .expect("share");
        let found = game_share(&conn, first).expect("read").expect("a game");
        assert_eq!(found.game_number, 1);
        assert_eq!(
            found.share,
            Some(Share {
                uuid: "q-uuid".to_string(),
                url: "https://mymtgo.test/players/Qplayer/replays/q-uuid".to_string(),
                shared_at_ms: 5_000,
            })
        );
        assert_eq!(game_share(&conn, 999_999).expect("read"), None);
    }
}
