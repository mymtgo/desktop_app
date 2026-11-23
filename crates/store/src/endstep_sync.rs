//! Endstep sync bookkeeping (endstep plan 2): which capture units are folded
//! at the current sync version, the refs each unit carries (so a unit
//! arriving late finds the matches it belongs to), and each ranked match's
//! rating change.
//!
//! A ref is `game:<forge game id>`, `match:<match id>`, `db:<db id>`,
//! `lobby:<lobby id>`, `deck:<deck id>` (a lobby's seat deck) or
//! `deckbody:<deck id>` (a deck body) (`endstep_protocol::unit_refs`). A
//! match's units are linked in a short chain: its match body unit carries
//! its game, match, db and lobby refs; its lobby unit carries the lobby and
//! the seats' `deck:` refs; a deck body unit carries `deckbody:`; the ranked
//! result unit carries the db id. `match:` links nothing: it finds a stored
//! match's game again (`unfinished_matches`, `game_log_lines`).

use std::collections::BTreeMap;

use endstep_match::{EndstepGameCards, EndstepLogLine, game_log};
use endstep_protocol::{EndstepCardKey, FrameKind, RankedResult, Unit};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::endstep_captures::decoded_unit;
use crate::endstep_cards::endstep_card_ids;
use crate::game_sync::{GameCardRow, GameCards, stamp_games_synced, write_game_cards};

/// The Endstep sync's version. Raising it refolds every Endstep match:
/// every unit reads as unsynced again (live equals rebuild), and each
/// match pulled from another device is derived again from its source.
/// 2: the matches' game rows (`game_cards`).
/// 3: hands, mulligan hands, a deck per game, reveals.
/// 4: a refetched match body is not activity, a decided game keeps its
/// outcome, repeats are kind, `seq` and `sequenceNumber`, a score past the
/// game limit is counted (#676); a card id counts once under its first
/// name, an activation is a sighting (#691).
/// 5: mulligan counts from the hands and `HAND_SETTLED` too, the starting
/// hand size from `HAND_SETTLED`'s `amount`, discards from `DISCARD_LOG` and
/// without a state, an echo reveal of a card from a hand sent back (#707).
pub const ENDSTEP_SYNC_VERSION: i64 = 5;

/// A ranked Endstep match's rating change, stored against the match whose
/// token (`matches.token`, the Endstep match id) is `match_token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepRanked {
    pub match_token: String,
    pub status: Option<String>,
    pub format_id: Option<String>,
    pub rating_before: Option<i64>,
    pub rating_after: Option<i64>,
    pub score_before: Option<i64>,
    pub score_after: Option<i64>,
    pub delta: Option<i64>,
    pub rd_before: Option<i64>,
    pub rd_after: Option<i64>,
    pub rank: Option<String>,
}

impl EndstepRanked {
    /// The ranked result `result` of the match under `match_token`.
    pub fn from_result(match_token: &str, result: &RankedResult) -> EndstepRanked {
        EndstepRanked {
            match_token: match_token.to_string(),
            status: result.status.clone(),
            format_id: result.format_id.clone(),
            rating_before: result.rating_before,
            rating_after: result.rating_after,
            score_before: result.score_before,
            score_after: result.score_after,
            delta: result.delta,
            rd_before: result.rd_before,
            rd_after: result.rd_after,
            rank: result.rank.clone(),
        }
    }
}

/// Up to `limit` units not yet synced at `ENDSTEP_SYNC_VERSION`, in fold
/// order `(started_at, counter)`.
pub fn unsynced_units(conn: &Connection, limit: u32) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT unit_id FROM endstep_captures WHERE synced_version < ?1
         ORDER BY started_at, counter, unit_id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![ENDSTEP_SYNC_VERSION, limit], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// Every unit not yet synced at `ENDSTEP_SYNC_VERSION`, with its file hash,
/// in fold order `(started_at, counter)`.
pub fn unsynced_unit_hashes(conn: &Connection) -> Result<Vec<(String, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT unit_id, file_hash FROM endstep_captures WHERE synced_version < ?1
         ORDER BY started_at, counter, unit_id",
    )?;
    let rows = stmt.query_map([ENDSTEP_SYNC_VERSION], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<(String, String)>, _>>()?)
}

/// The Forge game ids a set of refs reaches, sorted: each `game:` ref
/// itself; each `db:` or `lobby:` ref through every unit that has it and a
/// `game:` ref; each `deckbody:<id>` ref through every unit with `deck:<id>`
/// (a lobby naming the deck), to that unit's own games and the games its
/// `db:` and `lobby:` refs reach. Each ref is followed on its own, so the
/// answer is the same however the refs are grouped. The other way round
/// from `units_of_game`: a unit reaches the games whose units it is among.
pub fn games_of_refs(conn: &Connection, refs: &[String]) -> Result<Vec<String>, StoreError> {
    let mut games: Vec<String> = Vec::new();
    let mut links: Vec<String> = Vec::new();
    for r in refs {
        if let Some(game) = r.strip_prefix("game:") {
            games.push(game.to_string());
        } else if r.starts_with("db:") || r.starts_with("lobby:") {
            links.push(r.clone());
        } else if let Some(deck_id) = r.strip_prefix("deckbody:") {
            for lobby_ref in refs_of_units_naming_deck(conn, deck_id)? {
                if let Some(game) = lobby_ref.strip_prefix("game:") {
                    games.push(game.to_string());
                } else {
                    links.push(lobby_ref);
                }
            }
        }
    }
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT substr(g.ref, 6) FROM endstep_unit_refs l
         JOIN endstep_unit_refs g ON g.unit_id = l.unit_id AND g.ref LIKE 'game:%'
         WHERE l.ref = ?1",
    )?;
    for link in &links {
        let rows = stmt.query_map([link], |row| row.get::<_, String>(0))?;
        for game in rows {
            games.push(game?);
        }
    }
    games.sort();
    games.dedup();
    Ok(games)
}

/// The Forge games `games` in the order they were played: by their first
/// unit `(started_at, counter)`, then by id (a game with no unit last).
/// Folding a group's games in this order keeps a deck's versions in play
/// order too.
pub fn games_in_start_order(
    conn: &Connection,
    games: Vec<String>,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT MIN(c.started_at), MIN(c.counter) FROM endstep_unit_refs r
         JOIN endstep_captures c ON c.unit_id = r.unit_id
         WHERE r.ref = ?1",
    )?;
    let mut keyed: Vec<(i64, i64, String)> = Vec::new();
    for game in games {
        let (started_at, counter): (Option<i64>, Option<i64>) = stmt
            .query_row([format!("game:{game}")], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        keyed.push((
            started_at.unwrap_or(i64::MAX),
            counter.unwrap_or(i64::MAX),
            game,
        ));
    }
    keyed.sort();
    Ok(keyed.into_iter().map(|(_, _, game)| game).collect())
}

/// The `game:`, `db:` and `lobby:` refs of the units with `deck:<deck_id>`.
fn refs_of_units_naming_deck(conn: &Connection, deck_id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT l.ref FROM endstep_unit_refs d
         JOIN endstep_unit_refs l ON l.unit_id = d.unit_id
             AND (l.ref LIKE 'game:%' OR l.ref LIKE 'db:%' OR l.ref LIKE 'lobby:%')
         WHERE d.ref = ?1",
    )?;
    let rows = stmt.query_map([format!("deck:{deck_id}")], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// The units of the match with Forge game `forge_game_id`, in fold order
/// `(started_at, counter)`: the units with `game:<id>`, the units sharing a
/// `db:` or `lobby:` ref with them, and, for each `deck:<deck id>` those
/// units carry, the units with `deckbody:<deck id>`. Another match naming
/// the same deck shares only the deck body units, never its own.
pub fn units_of_game(conn: &Connection, forge_game_id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "WITH game_units AS (
             SELECT unit_id FROM endstep_unit_refs WHERE ref = ?1
         ),
         links AS (
             SELECT DISTINCT r.ref FROM endstep_unit_refs r
             JOIN game_units g ON g.unit_id = r.unit_id
             WHERE r.ref LIKE 'db:%' OR r.ref LIKE 'lobby:%'
         ),
         linked_units AS (
             SELECT r.unit_id FROM endstep_unit_refs r JOIN links l ON l.ref = r.ref
         ),
         deck_bodies AS (
             SELECT DISTINCT 'deckbody:' || substr(r.ref, 6) AS ref FROM endstep_unit_refs r
             WHERE r.unit_id IN (SELECT unit_id FROM game_units UNION
                                 SELECT unit_id FROM linked_units)
               AND r.ref LIKE 'deck:%'
         ),
         deck_units AS (
             SELECT r.unit_id FROM endstep_unit_refs r JOIN deck_bodies d ON d.ref = r.ref
         ),
         units AS (
             SELECT unit_id FROM game_units
             UNION SELECT unit_id FROM linked_units
             UNION SELECT unit_id FROM deck_units
         )
         SELECT c.unit_id FROM endstep_captures c JOIN units u ON u.unit_id = c.unit_id
         ORDER BY c.started_at, c.counter, c.unit_id",
    )?;
    let rows = stmt.query_map([format!("game:{forge_game_id}")], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// The refs stored for the unit `unit_id` (none for a unit never synced),
/// sorted.
pub fn refs_of_unit(conn: &Connection, unit_id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT ref FROM endstep_unit_refs WHERE unit_id = ?1 ORDER BY ref")?;
    let rows = stmt.query_map([unit_id], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// The Forge game of the Endstep match `token` (its match id): the game of
/// the last match body with that id, in fold order `(started_at, counter)`.
/// `None` when no stored unit has that match body.
pub(crate) fn forge_game_of_match(
    conn: &Connection,
    token: &str,
) -> Result<Option<String>, StoreError> {
    Ok(match_body_units(conn, token)?.0)
}

/// The Forge game of the Endstep match `token` (as `forge_game_of_match`)
/// and the decoded units that hold a match body with that id, by unit id,
/// so a caller that needs them again does not decode them twice.
fn match_body_units(
    conn: &Connection,
    token: &str,
) -> Result<(Option<String>, BTreeMap<String, Unit>), StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.unit_id FROM endstep_unit_refs r
         JOIN endstep_captures c ON c.unit_id = r.unit_id
         WHERE r.ref = ?1
         ORDER BY c.started_at, c.counter, c.unit_id",
    )?;
    let rows = stmt.query_map([format!("match:{token}")], |row| row.get(0))?;
    let unit_ids = rows.collect::<Result<Vec<String>, _>>()?;
    let mut game = None;
    let mut decoded = BTreeMap::new();
    for unit_id in unit_ids {
        let Some(unit) = decoded_unit(conn, &unit_id)? else {
            continue;
        };
        for frame in &unit.frames {
            if let FrameKind::MatchBody(body) = &frame.kind
                && body.id == token
            {
                game = Some(body.forge_game_id.clone());
            }
        }
        decoded.insert(unit_id, unit);
    }
    Ok((game, decoded))
}

/// The Forge game of the Endstep match `token` and its stored units,
/// decoded, in fold order (`units_of_game`); a unit that does not decode is
/// left out. `None` when no stored unit has the match's body. Walks the
/// whole match: call it once per request, never in a loop over games. Each
/// unit is decoded once: the units read to find the game are reused.
pub(crate) fn match_units(
    conn: &Connection,
    token: &str,
) -> Result<Option<(String, Vec<Unit>)>, StoreError> {
    if let Some(synced) = synced_units(conn, token)? {
        return Ok(Some(synced));
    }
    let (Some(forge_game_id), mut decoded) = match_body_units(conn, token)? else {
        return Ok(None);
    };
    let mut units = Vec::new();
    for unit_id in units_of_game(conn, &forge_game_id)? {
        if let Some(unit) = decoded.remove(&unit_id) {
            units.push(unit);
        } else if let Some(unit) = decoded_unit(conn, &unit_id)? {
            units.push(unit);
        }
    }
    Ok(Some((forge_game_id, units)))
}

/// The Forge game and units of an Endstep match pulled from another
/// device (an `imported` row with a `synced_sources` row), as
/// `source_units` reads them. `None` for any other match.
fn synced_units(conn: &Connection, token: &str) -> Result<Option<(String, Vec<Unit>)>, StoreError> {
    let match_id: Option<i64> = conn
        .prepare_cached(
            "SELECT m.id FROM matches m JOIN synced_sources s ON s.match_id = m.id
             WHERE m.token = ?1 AND m.source = 'imported' AND s.platform = 'endstep'",
        )?
        .query_row([token], |row| row.get(0))
        .optional()?;
    let Some(match_id) = match_id else {
        return Ok(None);
    };
    let Some(sync_bundles::MatchSources::Endstep(sources)) =
        crate::synced_sources::read(conn, match_id)?
    else {
        return Ok(None);
    };
    Ok(Some(source_units(&sources)))
}

/// A pulled Endstep source's Forge game and units, decoded, in the order
/// they came; a unit that does not decode is left out, as the local read
/// leaves one out.
fn source_units(sources: &sync_bundles::EndstepSources) -> (String, Vec<Unit>) {
    let mut units = Vec::new();
    for unit in &sources.units {
        match endstep_protocol::decode_unit(&unit.lines.join("\n")) {
            Ok(decoded) => units.push(decoded),
            Err(error) => {
                tracing::warn!(unit_id = unit.unit_id, %error, "a pulled Endstep unit did not decode; skipped");
            }
        }
    }
    (sources.forge_game_id.clone(), units)
}

/// The game log of game `game_number` of the Endstep match `token`, read
/// from its stored units (`endstep_match::game_log`). Empty when the match
/// has no stored units or no such game. Walks the whole match: call it once
/// per request, never in a loop over games.
pub fn game_log_lines(
    conn: &Connection,
    token: &str,
    game_number: u32,
) -> Result<Vec<EndstepLogLine>, StoreError> {
    let Some((forge_game_id, units)) = match_units(conn, token)? else {
        return Ok(Vec::new());
    };
    Ok(game_log(&forge_game_id, &units, game_number))
}

/// The derivation name of the Endstep sync (`derivations.name`).
const SYNC_DERIVATION: &str = "endstep_sync";

/// The Endstep sync version stored with the last full resync, `None`
/// before the first one.
pub fn sync_version(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT version FROM derivations WHERE name = ?1")?
        .query_row([SYNC_DERIVATION], |row| row.get(0))
        .optional()?)
}

/// Every Endstep match still in progress, hidden ones too, as the Forge
/// games its match body units name (by its `match:` ref) with the match's
/// last activity: the tracker folds them again once they are quiet, so
/// they turn abandoned without new captures. Sorted. A session unit can
/// hold several match bodies, so this may also name other games of that
/// unit: they are folded once more, which changes nothing (harmless).
pub fn unfinished_matches(conn: &Connection) -> Result<Vec<(String, i64)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT substr(g.ref, 6), m.last_activity_at FROM matches m
         JOIN accounts a ON a.id = m.account_id AND a.platform = 'endstep'
         JOIN endstep_unit_refs r ON r.ref = 'match:' || m.token
         JOIN endstep_unit_refs g ON g.unit_id = r.unit_id AND g.ref LIKE 'game:%'
         WHERE m.source = 'log' AND m.state = 'in_progress'
         ORDER BY 1",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<(String, i64)>, _>>()?)
}

/// The ranked result stored for the match `match_id`, if any.
pub fn endstep_rating(
    conn: &Connection,
    match_id: i64,
) -> Result<Option<EndstepRanked>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT m.token, r.status, r.format_id, r.rating_before, r.rating_after,
                 r.score_before, r.score_after, r.delta, r.rd_before, r.rd_after, r.rank
             FROM endstep_ranked_results r JOIN matches m ON m.id = r.match_id
             WHERE r.match_id = ?1",
        )?
        .query_row([match_id], |row| {
            Ok(EndstepRanked {
                match_token: row.get(0)?,
                status: row.get(1)?,
                format_id: row.get(2)?,
                rating_before: row.get(3)?,
                rating_after: row.get(4)?,
                score_before: row.get(5)?,
                score_after: row.get(6)?,
                delta: row.get(7)?,
                rd_before: row.get(8)?,
                rd_after: row.get(9)?,
                rank: row.get(10)?,
            })
        })
        .optional()?)
}

/// Stores a ranked result against its match, replacing any it had. A token
/// with no match is skipped with a warning.
pub(crate) fn upsert_ranked(tx: &Transaction, ranked: &EndstepRanked) -> Result<(), StoreError> {
    let Some(match_id) = crate::write::match_id_of(tx, &ranked.match_token)? else {
        tracing::warn!("Endstep ranked result skipped: no match has its token");
        return Ok(());
    };
    tx.prepare_cached(
        "INSERT INTO endstep_ranked_results (match_id, status, format_id, rating_before,
             rating_after, score_before, score_after, delta, rd_before, rd_after, rank)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (match_id) DO UPDATE SET status = excluded.status,
             format_id = excluded.format_id, rating_before = excluded.rating_before,
             rating_after = excluded.rating_after, score_before = excluded.score_before,
             score_after = excluded.score_after, delta = excluded.delta,
             rd_before = excluded.rd_before, rd_after = excluded.rd_after, rank = excluded.rank",
    )?
    .execute(params![
        match_id,
        ranked.status,
        ranked.format_id,
        ranked.rating_before,
        ranked.rating_after,
        ranked.score_before,
        ranked.score_after,
        ranked.delta,
        ranked.rd_before,
        ranked.rd_after,
        ranked.rank
    ])?;
    Ok(())
}

/// Writes the game rows of the Endstep match `token`, each card at its
/// Endstep catalog id, then stamps the complete match `games_synced_at`
/// (the mark archetype detection waits for). Rows that changed under an
/// already detected match ask detection again. Returns whether any rows
/// changed.
///
/// Rows, hand sizes and mulligan hands are written; casts stay empty. A
/// token with no stored Endstep log match (a dropped bot match), or a match
/// the player corrected, is skipped; so is a game number with no `games`
/// row.
pub(crate) fn write_endstep_games(
    tx: &Transaction,
    token: &str,
    games: &[EndstepGameCards],
    now_ms: i64,
) -> Result<bool, StoreError> {
    let Some(match_id) = syncable_match_id(tx, token)? else {
        return Ok(false);
    };
    write_games(tx, match_id, games, now_ms)
}

/// A pulled Endstep match's game rows, folded from `sources`, the source it
/// came with (cross-device sync spec section 7), and written as `write_endstep_games`
/// writes a recorded one's. Its account and opponent take the names the
/// fold gives them when they have none (a bundle carries no Endstep
/// display names). Only an Endstep account's `imported` row; a match the
/// fold drops (no match body, a bot, a spectator) gets no rows. Returns
/// whether any rows changed.
pub fn write_imported_games(
    tx: &Transaction,
    match_id: i64,
    sources: &sync_bundles::EndstepSources,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let pulled: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches m
                 JOIN accounts a ON a.id = m.account_id AND a.platform = 'endstep'
                 WHERE m.id = ?1 AND m.source = 'imported')",
        )?
        .query_row([match_id], |row| row.get(0))?;
    if !pulled {
        return Ok(false);
    }
    let (forge_game_id, units) = source_units(sources);
    let found = match endstep_match::fold_match(&forge_game_id, &units, now_ms) {
        endstep_match::Folded::Match(found) => found,
        endstep_match::Folded::Dropped(reason) => {
            tracing::warn!(?reason, "a pulled Endstep match did not fold; no game rows");
            return Ok(false);
        }
    };
    name_seats(tx, match_id, &found)?;
    write_games(tx, match_id, &found.game_cards, now_ms)
}

/// The fold's names for the account and opponent of the match `match_id`,
/// where they have none.
fn name_seats(
    tx: &Transaction,
    match_id: i64,
    found: &endstep_match::EndstepMatch,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE accounts SET display_name = ?2
         WHERE id = (SELECT account_id FROM matches WHERE id = ?1) AND display_name IS NULL",
    )?
    .execute(params![match_id, found.account.username])?;
    if let Some(opponent) = &found.opponent {
        tx.prepare_cached(
            "UPDATE opponents SET display_name = ?2
             WHERE id = (SELECT opponent_id FROM matches WHERE id = ?1)
               AND display_name IS NULL",
        )?
        .execute(params![match_id, opponent.username])?;
    }
    Ok(())
}

/// Writes each game's rows of the match `match_id`, then stamps it
/// `games_synced_at` and asks detection again when rows changed.
fn write_games(
    tx: &Transaction,
    match_id: i64,
    games: &[EndstepGameCards],
    now_ms: i64,
) -> Result<bool, StoreError> {
    let mut changed = false;
    for game in games {
        let Some(game_id) = game_id_of(tx, match_id, game.game_number)? else {
            continue;
        };
        let cards = game_cards_of(tx, game, now_ms)?;
        changed |= write_game_cards(tx, game_id, &cards)?;
    }
    stamp_games_synced(tx, match_id)?;
    if changed {
        tx.prepare_cached(
            "UPDATE matches SET archetypes_detected_at = NULL
             WHERE id = ?1 AND archetypes_detected_at IS NOT NULL",
        )?
        .execute([match_id])?;
    }
    Ok(changed)
}

/// The id of the Endstep log match `token`, unless the player corrected it
/// (its games are their fix).
fn syncable_match_id(tx: &Transaction, token: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT m.id FROM matches m
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'endstep'
             WHERE m.token = ?1 AND m.source = 'log' AND m.corrected_at IS NULL",
        )?
        .query_row([token], |row| row.get(0))
        .optional()?)
}

/// The id of game `game_number` of the match `match_id`, if stored.
fn game_id_of(
    tx: &Transaction,
    match_id: i64,
    game_number: u32,
) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM games WHERE match_id = ?1 AND game_number = ?2")?
        .query_row(params![match_id, game_number], |row| row.get(0))
        .optional()?)
}

/// One game's rows, hand sizes and mulligan hands with their catalog ids.
/// Rows sort by side then catalog id; two keys that resolve to one catalog
/// id (their set codes differ only by case) are summed into one row, so are
/// the quantities of one card in one mulligan hand. The keys of the rows and of
/// the mulligan hands go through `endstep_card_ids` in one call.
fn game_cards_of(
    tx: &Transaction,
    game: &EndstepGameCards,
    now_ms: i64,
) -> Result<GameCards, StoreError> {
    let mut keys: Vec<EndstepCardKey> = game.rows.iter().map(|(key, _)| key.clone()).collect();
    keys.extend(game.mulligans.iter().map(|(_, key, _)| key.clone()));
    let ids = endstep_card_ids(tx, &keys, now_ms)?;
    let (row_ids, mulligan_ids) = ids.split_at(game.rows.len());

    let mut cards: Vec<GameCardRow> = Vec::new();
    for ((_, row), catalog_id) in game.rows.iter().zip(row_ids) {
        let mut row = row.clone();
        row.catalog_id = *catalog_id;
        match cards
            .iter_mut()
            .find(|have| have.side == row.side && have.catalog_id == row.catalog_id)
        {
            Some(have) => merge_card_rows(have, &row),
            None => cards.push(row),
        }
    }
    cards.sort_by_key(|row| (row.side, row.catalog_id));

    let mut mulligans: Vec<(u32, u32, u32)> = Vec::new();
    for ((hand, _, quantity), catalog_id) in game.mulligans.iter().zip(mulligan_ids) {
        match mulligans
            .iter_mut()
            .find(|(h, id, _)| h == hand && id == catalog_id)
        {
            Some(have) => have.2 = have.2.saturating_add(*quantity),
            None => mulligans.push((*hand, *catalog_id, *quantity)),
        }
    }
    mulligans.sort_unstable();

    Ok(GameCards {
        opening_hand_size: game.opening_hand_size,
        starting_hand_size: game.starting_hand_size,
        opp_opening_hand_size: game.opp_opening_hand_size,
        opp_starting_hand_size: game.opp_starting_hand_size,
        cards,
        mulligans,
        ..GameCards::default()
    })
}

/// Adds `other`'s counters to `row` (two keys of one card). The starting
/// hand is recomputed so `starting_hand = opening_hand - bottomed` holds.
fn merge_card_rows(row: &mut GameCardRow, other: &GameCardRow) {
    row.main_qty = row.main_qty.saturating_add(other.main_qty);
    row.side_qty = row.side_qty.saturating_add(other.side_qty);
    row.sided_in = row.sided_in.saturating_add(other.sided_in);
    row.sided_out = row.sided_out.saturating_add(other.sided_out);
    row.opening_hand = row.opening_hand.saturating_add(other.opening_hand);
    row.bottomed = row.bottomed.saturating_add(other.bottomed);
    row.starting_hand = row.opening_hand.saturating_sub(row.bottomed);
    row.first_draw = row.first_draw.saturating_add(other.first_draw);
    row.seen = row.seen.saturating_add(other.seen);
    row.pregame_revealed = row.pregame_revealed.saturating_add(other.pregame_revealed);
    row.pregame_played = row.pregame_played.saturating_add(other.pregame_played);
    row.cast = row.cast.saturating_add(other.cast);
    row.played = sum_options(row.played, other.played);
    row.activated = sum_options(row.activated, other.activated);
    row.cast_turn = match (row.cast_turn, other.cast_turn) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    row.discarded = row.discarded.saturating_add(other.discarded);
    row.hand_seen = row.hand_seen.saturating_add(other.hand_seen);
    row.graveyard_seen = row.graveyard_seen.saturating_add(other.graveyard_seen);
    row.exile_seen = row.exile_seen.saturating_add(other.exile_seen);
    row.battlefield_seen = row.battlefield_seen.saturating_add(other.battlefield_seen);
    row.has_zone_data |= other.has_zone_data;
    row.inferred |= other.inferred;
}

fn sum_options(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    }
}

/// Replaces the stored refs of the unit `unit_id` with `refs`. A unit that
/// is not stored keeps none (its refs would name no capture).
pub(crate) fn replace_refs(
    tx: &Transaction,
    unit_id: &str,
    refs: &[String],
) -> Result<(), StoreError> {
    tx.prepare_cached("DELETE FROM endstep_unit_refs WHERE unit_id = ?1")?
        .execute([unit_id])?;
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO endstep_unit_refs (unit_id, ref)
         SELECT unit_id, ?2 FROM endstep_captures WHERE unit_id = ?1",
    )?;
    for r in refs {
        insert.execute(params![unit_id, r])?;
    }
    Ok(())
}

/// Every unit unsynced (`synced_version` 0) and the current version
/// stored, so every Endstep match is folded again (live equals rebuild).
/// The refs stay: each unit replaces its own when it is synced again.
/// Matches pulled from another device have no units here: they are
/// derived again now, from the source each came with
/// (`rederive_pulled_matches`).
pub(crate) fn resync_all(tx: &Transaction, now_ms: i64) -> Result<(), StoreError> {
    rederive_pulled_matches(tx, now_ms)?;
    tx.execute("UPDATE endstep_captures SET synced_version = 0", [])?;
    tx.prepare_cached(
        "INSERT INTO derivations (name, version, rebuilt_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (name) DO UPDATE SET version = excluded.version,
             rebuilt_at = excluded.rebuilt_at",
    )?
    .execute(params![SYNC_DERIVATION, ENDSTEP_SYNC_VERSION, now_ms])?;
    Ok(())
}

/// Every Endstep match pulled from another device (an `imported` row with
/// an Endstep `synced_sources` row) derived again from the source it came
/// with, by the writer the pull used (`write_imported_games`), so a raise
/// of `ENDSTEP_SYNC_VERSION` reaches them too (cross-device sync spec
/// section 7, "Derivation version raises"). Local rows only: the sync row
/// and the pushed bundle are not touched. Each match runs in a savepoint: a
/// database error fails the resync, any other (a source that does not read
/// back, a card row past the Endstep range) rolls that match back and is
/// logged, so the resync, its version and the local units go on.
fn rederive_pulled_matches(tx: &Transaction, now_ms: i64) -> Result<(), StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT m.id FROM matches m
         JOIN synced_sources s ON s.match_id = m.id AND s.platform = 'endstep'
         WHERE m.source = 'imported'
         ORDER BY m.id",
    )?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    for match_id in ids {
        tx.execute_batch("SAVEPOINT endstep_pulled_rederive")?;
        match rederive_pulled_match(tx, match_id, now_ms) {
            Ok(()) => tx.execute_batch("RELEASE endstep_pulled_rederive")?,
            Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
            Err(error) => {
                tx.execute_batch(
                    "ROLLBACK TO endstep_pulled_rederive; RELEASE endstep_pulled_rederive",
                )?;
                tracing::warn!(match_id, %error, "a pulled Endstep match could not be derived again; skipped");
            }
        }
    }
    Ok(())
}

/// One pulled Endstep match derived again from its stored source.
fn rederive_pulled_match(tx: &Transaction, match_id: i64, now_ms: i64) -> Result<(), StoreError> {
    if let Some(sync_bundles::MatchSources::Endstep(sources)) =
        crate::synced_sources::read(tx, match_id)?
    {
        write_imported_games(tx, match_id, &sources, now_ms)?;
    }
    Ok(())
}

/// Marks the unit `unit_id` synced at `ENDSTEP_SYNC_VERSION` when its file
/// hash is still `file_hash`: a unit replaced after it was taken (its new
/// text not folded yet) stays unsynced.
pub(crate) fn mark_synced(
    tx: &Transaction,
    unit_id: &str,
    file_hash: &str,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE endstep_captures SET synced_version = ?2 WHERE unit_id = ?1 AND file_hash = ?3",
    )?
    .execute(params![unit_id, ENDSTEP_SYNC_VERSION, file_hash])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endstep_captures::{NewCapture, capture_hash, store_captures};
    use crate::test_support::{db, view};
    use crate::write::{MatchWrite, WriteBatch, commit};
    use endstep_capture::{CaptureHeader, Classification, UnitKind};
    use game_detail::{GameCardRow, Side};
    use mtgo_match::{MatchState, Outcome};

    const A: &str = "00000000-0000-4000-8000-00000000000a";
    const B: &str = "00000000-0000-4000-8000-00000000000b";
    const C: &str = "00000000-0000-4000-8000-00000000000c";
    const D: &str = "00000000-0000-4000-8000-00000000000d";
    const E: &str = "00000000-0000-4000-8000-00000000000e";
    const TOKEN: &str = "q-endstep-match-1";

    fn capture(unit_id: &str, counter: i64, hash: &str) -> NewCapture {
        NewCapture {
            header: CaptureHeader {
                format: 1,
                unit_id: unit_id.to_string(),
                kind: UnitKind::Session,
                started_at: 1_000,
                counter,
                extension: "0.1.0".to_string(),
                browser: "chrome".to_string(),
                developer: false,
                classification: Classification::Human,
            },
            frame_count: 1,
            text: format!("{{\"unit\":\"{unit_id}\"}}\n"),
            file_hash: hash.to_string(),
        }
    }

    fn store(conn: &mut Connection, captures: &[NewCapture]) {
        let tx = conn.transaction().expect("tx");
        store_captures(&tx, captures, 99).expect("store");
        tx.commit().expect("commit");
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|text| text.to_string()).collect()
    }

    /// Stores each `(unit, refs)` as a capture, in order, and its refs.
    fn units_with_refs(conn: &mut Connection, units: &[(&str, &[&str])]) {
        let captures: Vec<NewCapture> = units
            .iter()
            .enumerate()
            .map(|(index, (unit, _))| capture(unit, index as i64 + 1, "h1"))
            .collect();
        store(conn, &captures);
        commit(
            conn,
            &WriteBatch {
                endstep_refs: units
                    .iter()
                    .map(|(unit, refs)| (unit.to_string(), strings(refs)))
                    .collect(),
                ..WriteBatch::default()
            },
        )
        .expect("refs");
    }

    /// Marks `units` synced as folded from the text with hash `h1`.
    fn mark(conn: &mut Connection, units: &[&str]) {
        commit(
            conn,
            &WriteBatch {
                endstep_synced: units
                    .iter()
                    .map(|unit| (unit.to_string(), "h1".to_string()))
                    .collect(),
                ..WriteBatch::default()
            },
        )
        .expect("mark");
    }

    #[test]
    fn stored_units_are_unsynced_until_marked() {
        let (_dir, mut conn) = db("endstep-sync-unsynced");
        store(&mut conn, &[capture(B, 2, "h1"), capture(A, 1, "h1")]);
        assert_eq!(unsynced_units(&conn, 10).unwrap(), strings(&[A, B]));
        assert_eq!(unsynced_units(&conn, 1).unwrap(), strings(&[A]));
        mark(&mut conn, &[A]);
        assert_eq!(unsynced_units(&conn, 10).unwrap(), strings(&[B]));
        mark(&mut conn, &[B]);
        assert!(unsynced_units(&conn, 10).unwrap().is_empty());
    }

    #[test]
    fn unsynced_units_come_with_their_hashes() {
        let (_dir, mut conn) = db("endstep-sync-unsynced-hashes");
        store(&mut conn, &[capture(B, 2, "h1"), capture(A, 1, "h2")]);
        let pair = |unit: &str, hash: &str| (unit.to_string(), hash.to_string());
        assert_eq!(
            unsynced_unit_hashes(&conn).unwrap(),
            vec![pair(A, "h2"), pair(B, "h1")]
        );
        mark(&mut conn, &[B]);
        assert_eq!(unsynced_unit_hashes(&conn).unwrap(), vec![pair(A, "h2")]);
    }

    #[test]
    fn a_mark_of_older_text_leaves_the_unit_unsynced() {
        let (_dir, mut conn) = db("endstep-sync-stale-mark");
        store(&mut conn, &[capture(A, 1, "h2")]);
        mark(&mut conn, &[A]);
        assert_eq!(
            unsynced_units(&conn, 10).unwrap(),
            strings(&[A]),
            "folded as h1, stored as h2: its new text is not folded yet"
        );
        assert_eq!(capture_hash(&conn, A).unwrap().as_deref(), Some("h2"));
        assert_eq!(capture_hash(&conn, B).unwrap(), None);
    }

    #[test]
    fn a_replaced_unit_is_unsynced_again() {
        let (_dir, mut conn) = db("endstep-sync-replaced");
        units_with_refs(&mut conn, &[(A, &["game:g"])]);
        mark(&mut conn, &[A]);
        store(&mut conn, &[capture(A, 1, "h1")]);
        assert!(
            unsynced_units(&conn, 10).unwrap().is_empty(),
            "the same file again changes nothing"
        );
        store(&mut conn, &[capture(A, 1, "h2")]);
        assert_eq!(unsynced_units(&conn, 10).unwrap(), strings(&[A]));
        assert_eq!(
            units_of_game(&conn, "g").unwrap(),
            strings(&[A]),
            "its refs stay until it is synced again"
        );
    }

    #[test]
    fn refs_reach_games_through_db_ids() {
        let (_dir, mut conn) = db("endstep-sync-db-refs");
        units_with_refs(&mut conn, &[(A, &["game:g", "db:d"]), (B, &["db:d"])]);
        assert_eq!(units_of_game(&conn, "g").unwrap(), strings(&[A, B]));
        assert_eq!(
            games_of_refs(&conn, &strings(&["db:d"])).unwrap(),
            strings(&["g"])
        );
        assert_eq!(
            games_of_refs(&conn, &strings(&["game:h"])).unwrap(),
            strings(&["h"]),
            "a game ref is its game, stored or not"
        );
        assert!(units_of_game(&conn, "h").unwrap().is_empty());
    }

    #[test]
    fn a_match_reaches_its_lobby_deck_and_ranked_units() {
        let (_dir, mut conn) = db("endstep-sync-chain");
        // A match body unit, its lobby unit, a deck body unit, its ranked
        // result unit; then another match with the same deck: its match body
        // unit (E), lobby unit (F) and ranked result unit (H).
        let f = "00000000-0000-4000-8000-00000000000f";
        let h = "00000000-0000-4000-8000-000000000011";
        units_with_refs(
            &mut conn,
            &[
                (A, &["db:d", "game:g", "lobby:l"]),
                (B, &["deck:k", "lobby:l"]),
                (C, &["deckbody:k"]),
                (D, &["db:d"]),
                (E, &["db:d2", "game:g2", "lobby:l2"]),
                (f, &["deck:k", "lobby:l2"]),
                (h, &["db:d2"]),
            ],
        );
        assert_eq!(
            units_of_game(&conn, "g").unwrap(),
            strings(&[A, B, C, D]),
            "no unit of the other match"
        );
        assert_eq!(units_of_game(&conn, "g2").unwrap(), strings(&[C, E, f, h]));
        assert_eq!(
            games_of_refs(&conn, &strings(&["deck:k", "lobby:l"])).unwrap(),
            strings(&["g"]),
            "a lobby unit reaches its match, not every match with its decks"
        );
        assert_eq!(
            games_of_refs(&conn, &strings(&["deckbody:k"])).unwrap(),
            strings(&["g", "g2"]),
            "a deck body reaches every match whose lobby names its deck"
        );
        assert_eq!(
            games_of_refs(&conn, &strings(&["db:d"])).unwrap(),
            strings(&["g"])
        );
        assert_eq!(
            games_of_refs(&conn, &strings(&["db:d2"])).unwrap(),
            strings(&["g2"])
        );
    }

    #[test]
    fn games_of_refs_is_the_same_however_refs_are_grouped() {
        let (_dir, mut conn) = db("endstep-sync-grouping");
        let f = "00000000-0000-4000-8000-00000000000f";
        let units: [(&str, &[&str]); 6] = [
            (A, &["db:d", "game:g", "lobby:l"]),
            (B, &["deck:k", "lobby:l"]),
            (C, &["deckbody:k"]),
            (D, &["db:d"]),
            (E, &["game:g2", "lobby:l2"]),
            (f, &["deck:j", "lobby:l2"]),
        ];
        units_with_refs(&mut conn, &units);
        for (first, second) in [(1, 2), (2, 4), (2, 5), (3, 5), (0, 2)] {
            let mut merged = games_of_refs(&conn, &strings(units[first].1)).unwrap();
            merged.extend(games_of_refs(&conn, &strings(units[second].1)).unwrap());
            merged.sort();
            merged.dedup();
            let mut union = strings(units[first].1);
            union.extend(strings(units[second].1));
            assert_eq!(
                games_of_refs(&conn, &union).unwrap(),
                merged,
                "units {first} and {second}"
            );
        }
        assert_eq!(
            games_of_refs(&conn, &strings(&["deckbody:k", "lobby:l2"])).unwrap(),
            strings(&["g", "g2"])
        );
    }

    #[test]
    fn refs_are_replaced_and_dropped_with_their_unit() {
        let (_dir, mut conn) = db("endstep-sync-refs-replaced");
        units_with_refs(&mut conn, &[(A, &["game:g", "db:d"])]);
        commit(
            &mut conn,
            &WriteBatch {
                endstep_refs: vec![
                    (A.to_string(), strings(&["game:h"])),
                    (B.to_string(), strings(&["game:g"])),
                ],
                ..WriteBatch::default()
            },
        )
        .expect("refs");
        assert!(units_of_game(&conn, "g").unwrap().is_empty(), "no unit B");
        assert_eq!(units_of_game(&conn, "h").unwrap(), strings(&[A]));
        conn.execute("DELETE FROM endstep_captures WHERE unit_id = ?1", [A])
            .expect("delete");
        assert!(units_of_game(&conn, "h").unwrap().is_empty());
    }

    #[test]
    fn a_unit_s_stored_refs_are_read_back() {
        let (_dir, mut conn) = db("endstep-sync-refs-of-unit");
        units_with_refs(&mut conn, &[(A, &["game:g", "db:d"]), (B, &["lobby:l"])]);
        assert_eq!(
            refs_of_unit(&conn, A).unwrap(),
            strings(&["db:d", "game:g"])
        );
        assert!(refs_of_unit(&conn, C).unwrap().is_empty());
    }

    #[test]
    fn a_resync_unsyncs_every_unit_and_records_the_version() {
        let (_dir, mut conn) = db("endstep-sync-resync");
        assert_eq!(sync_version(&conn).unwrap(), None);
        units_with_refs(&mut conn, &[(A, &["game:g"]), (B, &["game:h"])]);
        mark(&mut conn, &[A, B]);
        commit(
            &mut conn,
            &WriteBatch {
                endstep_resync: true,
                // A mark in the same batch is kept: the resync goes first.
                endstep_synced: vec![(B.to_string(), "h1".to_string())],
                now_ms: 7,
                ..WriteBatch::default()
            },
        )
        .expect("resync");
        assert_eq!(sync_version(&conn).unwrap(), Some(ENDSTEP_SYNC_VERSION));
        assert_eq!(unsynced_units(&conn, 10).unwrap(), strings(&[A]));
        assert_eq!(
            units_of_game(&conn, "g").unwrap(),
            strings(&[A]),
            "the refs stay"
        );
    }

    #[test]
    fn unfinished_matches_name_their_games() {
        let (_dir, mut conn) = db("endstep-sync-unfinished");
        let mut live = view("q-live", "endstep:u2", 1_000);
        live.local_username = "endstep:u1".to_string();
        live.state = MatchState::InProgress;
        live.resolution = None;
        live.outcome = Outcome::Unknown;
        live.ended_at_ms = None;
        let mut done = live.clone();
        done.token = "q-done".to_string();
        done.state = MatchState::Complete;
        let mut mtgo = view("q-mtgo", "Qrival", 1_000);
        mtgo.state = MatchState::InProgress;
        commit(
            &mut conn,
            &WriteBatch {
                platform_accounts: vec![crate::arena::PlatformAccount {
                    platform: crate::Platform::Endstep,
                    username: "endstep:u1".to_string(),
                    user_id: None,
                    display_name: Some("Player".to_string()),
                    seen_at_ms: 1,
                }],
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(live),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(done),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(mtgo),
                        revision: 1,
                    },
                ],
                now_ms: 5,
                ..WriteBatch::default()
            },
        )
        .expect("matches");
        units_with_refs(
            &mut conn,
            &[
                (A, &["game:g", "match:q-live"]),
                (B, &["game:h", "match:q-done"]),
                (C, &["game:g2", "match:q-mtgo"]),
            ],
        );
        assert_eq!(
            unfinished_matches(&conn).unwrap(),
            vec![("g".to_string(), 1_000 + 4_000)]
        );
    }

    fn ranked(token: &str, delta: i64) -> EndstepRanked {
        EndstepRanked::from_result(
            token,
            &RankedResult {
                db_id: "d".to_string(),
                status: Some("ok".to_string()),
                format_id: Some("q-format".to_string()),
                rating_before: Some(1500),
                rating_after: Some(1500 + delta),
                score_before: Some(10),
                score_after: Some(11),
                delta: Some(delta),
                rd_before: Some(80),
                rd_after: Some(78),
                rank: None,
            },
        )
    }

    #[test]
    fn ranked_results_follow_their_match() {
        let (_dir, mut conn) = db("endstep-sync-ranked");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qrival", 1_000)),
                    revision: 1,
                }],
                endstep_ranked: vec![ranked(TOKEN, 12), ranked("q-no-match", 5)],
                now_ms: 5,
                ..WriteBatch::default()
            },
        )
        .expect("match and ranked result");
        let match_id = crate::test_support::match_id(&conn, TOKEN);
        assert_eq!(
            endstep_rating(&conn, match_id).unwrap(),
            Some(ranked(TOKEN, 12))
        );
        let stored: i64 = conn
            .query_row("SELECT COUNT(*) FROM endstep_ranked_results", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, 1, "a result with no match is skipped");
        commit(
            &mut conn,
            &WriteBatch {
                endstep_ranked: vec![ranked(TOKEN, -7)],
                ..WriteBatch::default()
            },
        )
        .expect("again");
        assert_eq!(
            endstep_rating(&conn, match_id).unwrap(),
            Some(ranked(TOKEN, -7)),
            "a later result replaces it"
        );
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Remove {
                    token: TOKEN.to_string(),
                }],
                ..WriteBatch::default()
            },
        )
        .expect("remove");
        assert_eq!(endstep_rating(&conn, match_id).unwrap(), None);
        let stored: i64 = conn
            .query_row("SELECT COUNT(*) FROM endstep_ranked_results", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, 0, "removed with its match");
    }

    /// The Endstep match `TOKEN` (three games), its account stored first.
    fn endstep_match(conn: &mut Connection) -> i64 {
        let mut played = view(TOKEN, "endstep:u2", 1_000);
        played.local_username = "endstep:u1".to_string();
        commit(
            conn,
            &WriteBatch {
                platform_accounts: vec![crate::arena::PlatformAccount {
                    platform: crate::Platform::Endstep,
                    username: "endstep:u1".to_string(),
                    user_id: None,
                    display_name: Some("Player".to_string()),
                    seen_at_ms: 1,
                }],
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(played),
                    revision: 1,
                }],
                now_ms: 5,
                ..WriteBatch::default()
            },
        )
        .expect("match");
        crate::test_support::match_id(conn, TOKEN)
    }

    fn key(name: &str) -> EndstepCardKey {
        EndstepCardKey {
            name: name.to_string(),
            set_code: String::new(),
            collector_number: String::new(),
        }
    }

    /// Game 1 with `copies` of a local card and one opponent card; games 2
    /// and 3 known only by number.
    fn games(copies: u32) -> Vec<EndstepGameCards> {
        let local = GameCardRow {
            side: Side::Local,
            main_qty: copies,
            played: Some(0),
            activated: Some(0),
            ..GameCardRow::default()
        };
        let opponent = GameCardRow {
            side: Side::Opponent,
            seen: 1,
            played: Some(1),
            activated: Some(0),
            ..GameCardRow::default()
        };
        vec![
            EndstepGameCards {
                game_number: 1,
                rows: vec![(key("Qcard"), local), (key("Qother"), opponent)],
                opening_hand_size: None,
                starting_hand_size: None,
                opp_opening_hand_size: None,
                opp_starting_hand_size: None,
                mulligans: Vec::new(),
            },
            EndstepGameCards {
                game_number: 2,
                rows: Vec::new(),
                opening_hand_size: None,
                starting_hand_size: None,
                opp_opening_hand_size: None,
                opp_starting_hand_size: None,
                mulligans: Vec::new(),
            },
            EndstepGameCards {
                game_number: 3,
                rows: Vec::new(),
                opening_hand_size: None,
                starting_hand_size: None,
                opp_opening_hand_size: None,
                opp_starting_hand_size: None,
                mulligans: Vec::new(),
            },
        ]
    }

    fn write_games(conn: &mut Connection, token: &str, games: Vec<EndstepGameCards>) {
        commit(
            conn,
            &WriteBatch {
                endstep_game_cards: vec![(token.to_string(), games)],
                now_ms: 9,
                ..WriteBatch::default()
            },
        )
        .expect("game rows");
    }

    fn number(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect(sql)
    }

    #[test]
    fn a_match_without_captures_has_no_game_log() {
        let (_dir, mut conn) = db("endstep-sync-log-no-captures");
        endstep_match(&mut conn);
        for game_number in 0..=3 {
            assert!(
                game_log_lines(&conn, TOKEN, game_number)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            game_log_lines(&conn, "q-no-such-match", 1)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn game_rows_are_written_by_name() {
        let (_dir, mut conn) = db("endstep-sync-game-rows");
        let match_id = endstep_match(&mut conn);
        write_games(&mut conn, TOKEN, games(4));
        let rows: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT g.game_number || '|' || gc.side || '|' || c.name || '|'
                         || gc.main_qty || '|' || gc.seen || '|' || gc.played
                     FROM game_cards gc JOIN games g ON g.id = gc.game_id
                     JOIN cards c ON c.catalog_id = gc.catalog_id
                     ORDER BY g.game_number, gc.side",
                )
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(rows, vec!["1|local|Qcard|4|0|0", "1|opponent|Qother|0|1|1"]);
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM matches WHERE id = {match_id} AND games_synced_at IS NOT NULL"
                )
            ),
            1,
            "the complete match is stamped for detection"
        );
    }

    #[test]
    fn changed_rows_requeue_detection() {
        let (_dir, mut conn) = db("endstep-sync-requeue");
        endstep_match(&mut conn);
        write_games(&mut conn, TOKEN, games(4));
        conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
            .unwrap();
        write_games(&mut conn, TOKEN, games(4));
        assert_eq!(
            number(&conn, "SELECT archetypes_detected_at FROM matches"),
            5,
            "the same rows leave detection alone"
        );
        write_games(&mut conn, TOKEN, games(3));
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NULL"
            ),
            1,
            "changed rows ask detection again"
        );
    }

    #[test]
    fn game_rows_of_other_matches_are_skipped() {
        let (_dir, mut conn) = db("endstep-sync-game-rows-skipped");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view("q-mtgo", "Qrival", 1_000)),
                    revision: 1,
                }],
                now_ms: 5,
                ..WriteBatch::default()
            },
        )
        .expect("an MTGO match");
        write_games(&mut conn, "q-no-match", games(4));
        write_games(&mut conn, "q-mtgo", games(4));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_cards"), 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NOT NULL"
            ),
            0,
            "an MTGO match is not the Endstep sync's to stamp"
        );
    }

    fn hand_row(side: Side, opening: u32, bottomed: u32) -> GameCardRow {
        GameCardRow {
            side,
            opening_hand: opening,
            bottomed,
            starting_hand: opening - bottomed,
            has_zone_data: true,
            ..GameCardRow::default()
        }
    }

    fn keyed(game_number: u32) -> EndstepGameCards {
        EndstepGameCards {
            game_number,
            rows: Vec::new(),
            opening_hand_size: None,
            starting_hand_size: None,
            opp_opening_hand_size: None,
            opp_starting_hand_size: None,
            mulligans: Vec::new(),
        }
    }

    #[test]
    fn hands_and_mulligan_hands_are_written() {
        let (_dir, mut conn) = db("endstep-sync-hands");
        endstep_match(&mut conn);
        let mut game = keyed(3);
        game.opening_hand_size = Some(7);
        game.starting_hand_size = Some(6);
        game.opp_opening_hand_size = Some(7);
        game.opp_starting_hand_size = Some(7);
        game.rows = vec![
            (key("Qkarn"), hand_row(Side::Local, 1, 1)),
            (key("Qugin"), hand_row(Side::Local, 1, 0)),
        ];
        game.mulligans = vec![
            (1, key("Qugin"), 1),
            (1, key("Qforest"), 2),
            (1, key("Qkarn"), 1),
            (2, key("Qforest"), 1),
        ];
        write_games(&mut conn, TOKEN, vec![game]);
        let sizes = |column: &str| {
            number(
                &conn,
                &format!("SELECT {column} FROM games WHERE game_number = 3"),
            )
        };
        assert_eq!(sizes("local_opening_hand_size"), 7);
        assert_eq!(sizes("local_starting_hand_size"), 6);
        assert_eq!(sizes("opp_opening_hand_size"), 7);
        assert_eq!(sizes("opp_starting_hand_size"), 7);
        assert_eq!(
            number(
                &conn,
                "SELECT SUM(quantity) FROM game_mulligan_cards WHERE hand = 1"
            ),
            4
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM game_mulligan_cards WHERE hand = 2"
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM game_cards WHERE bottomed = 1 AND starting_hand = 0"
            ),
            1
        );
        write_games(&mut conn, TOKEN, vec![keyed(3)]);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM game_mulligan_cards"),
            0,
            "a refold without the hands removes them"
        );
    }

    #[test]
    fn per_game_decks_are_written() {
        let (_dir, mut conn) = db("endstep-sync-per-game-decks");
        endstep_match(&mut conn);
        let mut game = keyed(2);
        game.rows = vec![(
            key("Qboulder"),
            GameCardRow {
                side: Side::Local,
                main_qty: 2,
                side_qty: 0,
                sided_out: 2,
                ..GameCardRow::default()
            },
        )];
        write_games(&mut conn, TOKEN, vec![game]);
        assert_eq!(
            number(
                &conn,
                "SELECT gc.sided_out FROM game_cards gc
                 JOIN cards c ON c.catalog_id = gc.catalog_id WHERE c.name = 'Qboulder'"
            ),
            2
        );
    }

    #[test]
    fn two_keys_of_one_card_are_summed() {
        let (_dir, mut conn) = db("endstep-sync-merged-keys");
        endstep_match(&mut conn);
        let printed = EndstepCardKey {
            name: "Qtwin".to_string(),
            set_code: "QQQ".to_string(),
            collector_number: "7".to_string(),
        };
        let lower = EndstepCardKey {
            set_code: "qqq".to_string(),
            ..printed.clone()
        };
        let mut game = keyed(1);
        game.rows = vec![
            (printed.clone(), hand_row(Side::Local, 1, 1)),
            (lower.clone(), hand_row(Side::Local, 2, 0)),
        ];
        game.mulligans = vec![(1, printed, 1), (1, lower, 2)];
        write_games(&mut conn, TOKEN, vec![game]);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM game_cards"),
            1,
            "one row for one catalog id"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT opening_hand * 100 + bottomed * 10 + starting_hand FROM game_cards"
            ),
            312,
            "counters are summed and starting = opening - bottomed"
        );
        assert_eq!(number(&conn, "SELECT has_zone_data FROM game_cards"), 1);
        assert_eq!(
            number(&conn, "SELECT SUM(quantity) FROM game_mulligan_cards"),
            3
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_mulligan_cards"), 1);
    }
}
