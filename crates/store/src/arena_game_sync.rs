//! The Arena game sync: one Arena match's stored game messages, folded and
//! summarised per game by `arena-game`, written as the rows MTGO's game sync
//! writes (`games` columns, `game_cards`, `game_mulligan_cards`), once per
//! match and stamped in `arena_game_syncs`. Match detail, Card stats, Game
//! stats and Sideboard guides read these rows as they read MTGO's.

use std::collections::BTreeMap;

use arena_game::{
    Commanders, GameColumns, GameInput, LogLine, Source, fold_game, local_main_deck, log_lines,
    summarise_game,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::arena_games::{
    GameMessageSource, StoredGameMessage, archive_game_messages, game_messages,
};
use crate::game_sync::{GameCards, stamp_games_synced, write_game_cards};
use crate::match_commanders::{
    MatchCommanders, ask_detection_again, match_deck_info, replace_commanders,
};

/// Raise to derive every stamped Arena match again.
pub const ARENA_GAME_SYNC_VERSION: i64 = 2;

/// Derives and writes one Arena match's game rows; stamps arena_game_syncs.
/// Skips manual matches, corrected log matches and imported ones, except a
/// match pulled from another device: it is derived again from the source
/// it came with (`derive_imported`), as a raise of
/// `ARENA_GAME_SYNC_VERSION` asks. Returns true when rows changed.
///
/// Every game with a `games` row and messages that show it starting gets
/// its rows; messages of a game number with no row are ignored. Game 1's
/// local main deck is the sideboarding baseline of the later games.
/// A complete match is stamped `games_synced_at` after either stamp, as
/// MTGO's sync does, so archetype detection takes it. Rows that changed
/// under an already detected match ask detection again.
pub fn sync_arena_match(tx: &Transaction, match_id: i64, now_ms: i64) -> Result<bool, StoreError> {
    let Some(token) = syncable_token(tx, match_id, Entered::Derived)? else {
        return match crate::synced_sources::read(tx, match_id) {
            Ok(Some(sync_bundles::MatchSources::Arena(sources))) => {
                derive_imported(tx, match_id, &sources, now_ms)
            }
            Ok(_) => Ok(false),
            Err(StoreError::Sqlite(error)) => Err(StoreError::Sqlite(error)),
            // A pulled source that does not read back gives no messages:
            // stamped so, at this version, it waits for the next raise
            // instead of failing every start.
            Err(error) => {
                tracing::warn!(match_id, %error, "a pulled Arena match's source could not be read; stamped without messages");
                stamp(tx, match_id, "no_messages", now_ms)?;
                Ok(false)
            }
        };
    };
    let messages = game_messages(tx, &token)?;
    derive(tx, match_id, messages, now_ms, Entered::Derived)
}

/// A corrected Arena log match's game rows built again from its stored
/// messages, for Edit match (a game added or removed: #573, #582). As
/// `sync_arena_match`, so it stamps `games_synced_at` and detection takes
/// the match, but the play side and turns the player entered are kept. The
/// tracker's sync never takes a corrected match, so later messages cannot
/// undo the edit. Any other match (MTGO, manual, imported) is left alone.
pub fn sync_corrected_arena_match(
    tx: &Transaction,
    match_id: i64,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let Some(token) = syncable_token(tx, match_id, Entered::Kept)? else {
        return Ok(false);
    };
    let messages = game_messages(tx, &token)?;
    derive(tx, match_id, messages, now_ms, Entered::Kept)
}

/// A pulled Arena match's game rows derived from `sources`, the source it
/// came with (cross-device sync spec section 7), as `sync_arena_match`
/// derives a recorded one's: the same fold and writers, stamped the same
/// way. The play side and turns are the bundle's (`Entered::Kept`), so a
/// correction made on the device that played it stays. Only an Arena
/// account's `imported` row: the tracker's syncs never take one. Returns
/// true when rows changed.
pub fn derive_imported(
    tx: &Transaction,
    match_id: i64,
    sources: &sync_bundles::ArenaSources,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let pulled: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches m
                 JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
                 WHERE m.id = ?1 AND m.source = 'imported')",
        )?
        .query_row([match_id], |row| row.get(0))?;
    if !pulled {
        return Ok(false);
    }
    let messages = crate::arena_games::source_messages(sources)?;
    derive(tx, match_id, messages, now_ms, Entered::Kept)
}

/// Whose play side and turn counts a sync writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entered {
    /// The messages' (the tracker's sync).
    Derived,
    /// The player's, from Edit match (the correction's sync).
    Kept,
}

fn derive(
    tx: &Transaction,
    match_id: i64,
    messages: Vec<StoredGameMessage>,
    now_ms: i64,
    entered: Entered,
) -> Result<bool, StoreError> {
    if messages.is_empty() {
        let changed = sync_commanders(tx, match_id, None)?;
        if changed {
            ask_detection_again(tx, match_id)?;
        }
        stamp(tx, match_id, "no_messages", now_ms)?;
        stamp_games_synced(tx, match_id)?;
        return Ok(changed);
    }
    let per_game = group_by_game(messages);
    let mut changed = false;
    let mut first_game_deck: Option<BTreeMap<u32, u32>> = None;
    let mut game_one_commanders: Option<Commanders> = None;
    for (game_id, game_number) in match_games(tx, match_id)? {
        // A game whose messages no longer show it starting (a re-derive
        // under newer rules) keeps no rows from an earlier derive.
        let Some(inputs) = per_game.get(&game_number) else {
            changed |= write_game_cards(tx, game_id, &GameCards::default())?;
            continue;
        };
        let fold = fold_game(inputs);
        if !fold.started {
            changed |= write_game_cards(tx, game_id, &GameCards::default())?;
            continue;
        }
        let baseline = if game_number == 1 {
            None
        } else {
            first_game_deck.as_ref()
        };
        let summary = summarise_game(&fold, inputs, baseline);
        if game_number == 1 {
            first_game_deck = Some(local_main_deck(&summary));
            game_one_commanders = Some(summary.commanders.clone());
        }
        let mut columns = summary.columns.clone();
        if entered == Entered::Kept {
            columns.on_play = None;
            columns.turn_count = None;
        }
        changed |= write_columns(tx, game_id, &columns)?;
        changed |= write_game_cards(tx, game_id, &summary.cards)?;
    }
    changed |= sync_commanders(tx, match_id, game_one_commanders.as_ref())?;
    stamp(tx, match_id, "derived", now_ms)?;
    stamp_games_synced(tx, match_id)?;
    if changed {
        ask_detection_again(tx, match_id)?;
    }
    Ok(changed)
}

/// Write the match's commanders: game 1's command zone for both seats, the
/// local seat from the deck's commander when the zone gave none. A match
/// not in a commander format keeps none. Returns whether they changed.
fn sync_commanders(
    tx: &Transaction,
    match_id: i64,
    game_one: Option<&Commanders>,
) -> Result<bool, StoreError> {
    let info = match_deck_info(tx, match_id)?;
    let mut wanted = MatchCommanders::default();
    if info.commander_format {
        if let Some(zone) = game_one {
            wanted.player = zone.local.iter().map(|id| i64::from(*id)).collect();
            wanted.opponent = zone.opponent.iter().map(|id| i64::from(*id)).collect();
        }
        // The table holds two positions: compare what it can store.
        wanted.opponent.truncate(2);
        if wanted.player.is_empty() {
            wanted.player = info.deck_commanders;
        }
        wanted.player.truncate(2);
    }
    replace_commanders(tx, match_id, &wanted)
}

/// What the match-end sync (`sync_arena_games`) did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArenaSyncOutcome {
    /// The matches whose game rows changed (the tracker pushes them again).
    pub changed_tokens: Vec<String>,
    /// The matches whose messages could not be read (logged, rolled back,
    /// not stamped: the next start tries them again, once).
    pub skipped: Vec<String>,
}

/// The match-end sync, in its own transaction after the batch that wrote
/// the match result (so the result reaches the pages first): each match's
/// live messages archived, then its game rows derived. A match that cannot
/// be read is skipped, the others still sync.
pub fn sync_arena_games(
    conn: &mut Connection,
    tokens: &[String],
    now_ms: i64,
) -> Result<ArenaSyncOutcome, StoreError> {
    let tx = conn.transaction()?;
    let mut outcome = ArenaSyncOutcome::default();
    for token in tokens {
        let Some(moved) = archive_guarded(&tx, token, now_ms)? else {
            outcome.skipped.push(token.clone());
            continue;
        };
        // A start rebuilds a quietly settled match in progress and the
        // first tick settles it again: with no new messages and a current
        // stamp, its rows are what a derive would write.
        if !moved && derived_and_current(&tx, token)? {
            continue;
        }
        match sync_guarded(&tx, token, now_ms)? {
            None => outcome.skipped.push(token.clone()),
            Some(true) => outcome.changed_tokens.push(token.clone()),
            Some(false) => {}
        }
    }
    // A limited match's game 1 cards say which entry of its event it is
    // when no course covers it, so its account's runs are folded again when
    // those cards can move it.
    for account_id in accounts_to_fold(&tx, &outcome.changed_tokens)? {
        crate::arena_run_sync::sync_arena_runs_guarded(&tx, account_id, now_ms)?;
    }
    tx.commit()?;
    Ok(outcome)
}

/// The accounts whose runs the new game rows of `tokens` can move, each
/// once.
fn accounts_to_fold(tx: &Transaction, tokens: &[String]) -> Result<Vec<i64>, StoreError> {
    let mut accounts: Vec<i64> = Vec::new();
    for token in tokens {
        let account = crate::arena_run_sync::account_to_fold_at_match_end(tx, token)?;
        if let Some(account_id) = account
            && !accounts.contains(&account_id)
        {
            accounts.push(account_id);
        }
    }
    Ok(accounts)
}

/// Archive a match's live messages in a savepoint: whether any were moved,
/// or `None` when its messages could not be read (rolled back and logged).
pub(crate) fn archive_guarded(
    tx: &Transaction,
    token: &str,
    now_ms: i64,
) -> Result<Option<bool>, StoreError> {
    guarded(tx, token, || archive_game_messages(tx, token, now_ms))
}

/// Whether the match's game rows are derived at the current version and
/// need nothing more: not a complete match still waiting for its
/// `games_synced_at` stamp.
fn derived_and_current(tx: &Transaction, token: &str) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches m
                 JOIN arena_game_syncs s ON s.match_id = m.id
                 WHERE m.token = ?1 AND s.status = 'derived' AND s.derived_version = ?2
                   AND NOT (m.state = 'complete' AND m.games_synced_at IS NULL))",
        )?
        .query_row(params![token, ARENA_GAME_SYNC_VERSION], |row| row.get(0))?)
}

/// Sync a match by token in a savepoint: whether its rows changed (`false`
/// for a token with no match row), or `None` when its messages could not
/// be read (rolled back and logged, not stamped).
pub(crate) fn sync_guarded(
    tx: &Transaction,
    token: &str,
    now_ms: i64,
) -> Result<Option<bool>, StoreError> {
    let Some(match_id) = crate::write::match_id_of(tx, token)? else {
        return Ok(Some(false));
    };
    guarded(tx, token, || sync_arena_match(tx, match_id, now_ms))
}

/// Run `work` in a savepoint. A database error (busy, full disk) is not a
/// bad archive: it fails the caller, as MTGO's `sync_games` does. Any other
/// error (an archive or message that does not read back) is rolled back to
/// the savepoint, logged, and answered `None`.
fn guarded<T>(
    tx: &Transaction,
    token: &str,
    work: impl FnOnce() -> Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    tx.execute_batch("SAVEPOINT arena_game_sync")?;
    match work() {
        Ok(value) => {
            tx.execute_batch("RELEASE arena_game_sync")?;
            Ok(Some(value))
        }
        Err(StoreError::Sqlite(error)) => Err(StoreError::Sqlite(error)),
        Err(error) => {
            tx.execute_batch("ROLLBACK TO arena_game_sync; RELEASE arena_game_sync")?;
            tracing::warn!(%token, %error, "an Arena match's game messages could not be read; skipped");
            Ok(None)
        }
    }
}

/// Arena matches with no stamp, or a stamp below ARENA_GAME_SYNC_VERSION, or
/// 'no_messages' that now have messages, or 'derived' with live (unarchived)
/// messages that came after the derive, or complete with no
/// `games_synced_at` (synced while ended or abandoned, then settled quietly
/// while the app was not running, or by a release that ran no match-end
/// sync for a quiet settlement: detection and the match report wait for
/// that stamp). The tracker archives each in the
/// batch that syncs it, so a match with late messages is due once.
///
/// Only matches the sync writes: from the log, not corrected, and no longer
/// in progress (a match still being played is synced when it ends). A
/// match pulled from another device (an `imported` row with an Arena
/// `synced_sources` row) is due when unstamped or stamped below the
/// version: it is derived again from its source (cross-device sync spec
/// section 7, "Derivation version raises"), which writes local rows only.
pub fn arena_matches_to_sync(conn: &Connection) -> Result<Vec<i64>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT m.id FROM matches m
         JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
         LEFT JOIN arena_game_syncs s ON s.match_id = m.id
         WHERE (m.source = 'log' AND m.corrected_at IS NULL AND m.state <> 'in_progress'
           AND (s.match_id IS NULL
                OR s.derived_version < ?1
                OR (m.state = 'complete' AND m.games_synced_at IS NULL
                    AND m.detached_at IS NULL)
                OR EXISTS (SELECT 1 FROM arena_game_messages l WHERE l.match_token = m.token)
                OR (s.status = 'no_messages'
                    AND EXISTS (SELECT 1 FROM arena_game_archives r
                                WHERE r.match_token = m.token))))
            OR (m.source = 'imported'
                AND EXISTS (SELECT 1 FROM synced_sources x
                            WHERE x.match_id = m.id AND x.platform = 'arena')
                AND (s.match_id IS NULL OR s.derived_version < ?1))
         ORDER BY m.id",
    )?;
    let rows = statement.query_map([ARENA_GAME_SYNC_VERSION], |row| row.get::<_, i64>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// The match's token when the sync may write it: an Arena account's match
/// from the log, and for the tracker's sync not corrected.
fn syncable_token(
    tx: &Transaction,
    match_id: i64,
    entered: Entered,
) -> Result<Option<String>, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT m.token FROM matches m
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
             WHERE m.id = ?1 AND m.source = 'log' AND (?2 OR m.corrected_at IS NULL)",
        )?
        .query_row(params![match_id, entered == Entered::Kept], |row| {
            row.get(0)
        })
        .optional()?)
}

/// The match's games, `(id, game_number)`, game 1 first.
fn match_games(tx: &Transaction, match_id: i64) -> Result<Vec<(i64, u32)>, StoreError> {
    let mut statement = tx.prepare_cached(
        "SELECT id, game_number FROM games WHERE match_id = ?1 ORDER BY game_number",
    )?;
    let rows = statement.query_map([match_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row?);
    }
    Ok(games)
}

/// One game's log lines (the match detail game log panel), folded from the
/// match's stored messages of that game number. Empty when the game has no
/// messages or they never show it starting.
pub fn game_log_lines(
    conn: &Connection,
    match_token: &str,
    game_number: u32,
) -> Result<Vec<LogLine>, StoreError> {
    let mut per_game = group_by_game(game_messages(conn, match_token)?);
    let Some(inputs) = per_game.remove(&game_number) else {
        return Ok(Vec::new());
    };
    Ok(log_lines(&fold_game(&inputs)))
}

/// The messages of each game number, in log order. A message with no game
/// number belongs to no game.
pub(crate) fn group_by_game(messages: Vec<StoredGameMessage>) -> BTreeMap<u32, Vec<GameInput>> {
    let mut per_game: BTreeMap<u32, Vec<GameInput>> = BTreeMap::new();
    for message in messages {
        let Some(game_number) = message.game_number else {
            continue;
        };
        let source = match message.source {
            GameMessageSource::Gre => Source::Gre,
            GameMessageSource::Client => Source::Client,
        };
        per_game.entry(game_number).or_default().push(GameInput {
            source,
            logged_at: message.logged_at,
            json: message.json,
        });
    }
    per_game
}

/// Write the game's columns the messages told (a `None` keeps the stored
/// value). Returns whether a value changed.
fn write_columns(
    tx: &Transaction,
    game_id: i64,
    columns: &GameColumns,
) -> Result<bool, StoreError> {
    let updated = tx
        .prepare_cached(
            "UPDATE games SET on_play = COALESCE(?2, on_play),
                 local_mulligans = COALESCE(?3, local_mulligans),
                 opp_mulligans = COALESCE(?4, opp_mulligans),
                 turn_count = COALESCE(?5, turn_count),
                 started_at = COALESCE(?6, started_at),
                 ended_at = COALESCE(?7, ended_at)
             WHERE id = ?1
               AND (on_play IS NOT COALESCE(?2, on_play)
                    OR local_mulligans IS NOT COALESCE(?3, local_mulligans)
                    OR opp_mulligans IS NOT COALESCE(?4, opp_mulligans)
                    OR turn_count IS NOT COALESCE(?5, turn_count)
                    OR started_at IS NOT COALESCE(?6, started_at)
                    OR ended_at IS NOT COALESCE(?7, ended_at))",
        )?
        .execute(params![
            game_id,
            columns.on_play,
            columns.local_mulligans,
            columns.opp_mulligans,
            columns.turn_count,
            columns.started_at,
            columns.ended_at,
        ])?;
    Ok(updated > 0)
}

fn stamp(tx: &Transaction, match_id: i64, status: &str, now_ms: i64) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO arena_game_syncs (match_id, status, derived_version, synced_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (match_id) DO UPDATE SET status = excluded.status,
             derived_version = excluded.derived_version, synced_at = excluded.synced_at",
    )?
    .execute(params![match_id, status, ARENA_GAME_SYNC_VERSION, now_ms])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use mtgo_match::{GameResult, MatchView};
    use rusqlite::params;

    use super::*;
    use crate::arena_games::{GameMessageSource, NewGameMessage};
    use crate::test_support::*;
    use crate::write::{MatchWrite, WriteBatch, commit};

    const ARENA: &str = "q-arena-match";
    /// Arena grpIds of the local deck (catalog ids are offset).
    const DECK: [u32; 3] = [70_001, 70_001, 70_002];

    /// A complete Arena Bo3 view as the Arena projector sends it: no game
    /// ids, no opponent mulligans and no turn counts.
    fn arena_view(token: &str) -> MatchView {
        let mut view = view(token, "arena:Qfoe-id", 1_000);
        view.mtgo_match_id = None;
        view.local_username = "Qarena#12345".to_string();
        view.game_structure = Some("bo3".to_string());
        view.games = vec![
            game(1, None, GameResult::Win),
            game(2, None, GameResult::Win),
        ];
        for game in &mut view.games {
            game.opp_mulligans = None;
            game.turn_count = None;
            game.local_die_roll = None;
            game.opp_die_roll = None;
        }
        view
    }

    fn gre(json: &str) -> (GameMessageSource, String) {
        (GameMessageSource::Gre, json.to_string())
    }

    /// One game's messages: the deck (game 1's connect response, a later
    /// game's submitted deck) and a full state at turn 5 where the local
    /// seat 1 holds a deck card and the opponent took one mulligan.
    fn game_messages(game_number: u32) -> Vec<(GameMessageSource, String)> {
        let deck = format!("{{\"deckCards\":{DECK:?},\"sideboardCards\":[70003]}}");
        let deck_message = if game_number == 1 {
            gre(&format!(
                "{{\"type\":\"GREMessageType_ConnectResp\",\"systemSeatIds\":[1],\
                 \"connectResp\":{{\"deckMessage\":{deck}}}}}"
            ))
        } else {
            (
                GameMessageSource::Client,
                format!("{{\"submitDeckResp\":{{\"deck\":{deck}}}}}"),
            )
        };
        let state = gre(
            "{\"type\":\"GREMessageType_GameStateMessage\",\"systemSeatIds\":[1],\
             \"gameStateMessage\":{\"type\":\"GameStateType_Full\",\"gameStateId\":1,\
             \"players\":[{\"systemSeatNumber\":1,\"lifeTotal\":20,\"mulliganCount\":0},\
             {\"systemSeatNumber\":2,\"lifeTotal\":20,\"mulliganCount\":1}],\
             \"turnInfo\":{\"turnNumber\":5,\"activePlayer\":1},\
             \"zones\":[{\"zoneId\":31,\"type\":\"ZoneType_Hand\",\"ownerSeatId\":1,\
             \"objectInstanceIds\":[10]}],\
             \"gameObjects\":[{\"instanceId\":10,\"grpId\":70001,\
             \"type\":\"GameObjectType_Card\",\"zoneId\":31,\"visibility\":\"Visibility_Private\",\
             \"ownerSeatId\":1,\"controllerSeatId\":1}]}}",
        );
        vec![deck_message, state]
    }

    /// Store `games`' messages for `token` (game numbers as given).
    fn store_messages(conn: &mut Connection, token: &str, games: &[u32]) {
        let file_id = arena_log_file(conn);
        let mut messages = Vec::new();
        for game_number in games {
            for (index, (source, json)) in game_messages(*game_number).into_iter().enumerate() {
                messages.push(NewGameMessage {
                    file_id,
                    byte_offset: u64::from(*game_number) * 1_000 + index as u64,
                    seq: 0,
                    match_token: token.to_string(),
                    game_number: Some(*game_number),
                    source,
                    logged_at: i64::from(*game_number) * 10_000 + index as i64,
                    json,
                });
            }
        }
        commit(
            conn,
            &WriteBatch {
                arena_game_messages: messages,
                ..WriteBatch::default()
            },
        )
        .expect("messages");
    }

    fn upsert(conn: &mut Connection, view: MatchView, revision: u64) {
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view),
                    revision,
                }],
                now_ms: 2_000,
                ..WriteBatch::default()
            },
        )
        .expect("upsert");
    }

    /// An Arena match `token` with its two games' messages.
    fn arena_match(conn: &mut Connection, token: &str) -> i64 {
        arena_account(conn);
        upsert(conn, arena_view(token), 1);
        store_messages(conn, token, &[1, 2]);
        match_id(conn, token)
    }

    fn sync(conn: &mut Connection, id: i64) -> bool {
        let tx = conn.transaction().expect("tx");
        let changed = sync_arena_match(&tx, id, 5_000).expect("sync");
        tx.commit().expect("commit");
        changed
    }

    /// The batch the tracker sends for a match it syncs: its live messages
    /// archived, then the sync.
    fn archive_and_sync(conn: &mut Connection, token: &str) {
        commit(
            conn,
            &WriteBatch {
                arena_archive_tokens: vec![token.to_string()],
                arena_sync_tokens: vec![token.to_string()],
                now_ms: 5_000,
                ..WriteBatch::default()
            },
        )
        .expect("sync batch");
    }

    fn stamp(conn: &Connection, id: i64) -> Option<(String, i64, i64)> {
        conn.query_row(
            "SELECT status, derived_version, synced_at FROM arena_game_syncs WHERE match_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
    }

    /// Each game's `(game_number, turn_count, opp_mulligans)`.
    fn game_columns(conn: &Connection, id: i64) -> Vec<(i64, Option<i64>, Option<i64>)> {
        let mut stmt = conn
            .prepare(
                "SELECT game_number, turn_count, opp_mulligans FROM games
                 WHERE match_id = ?1 ORDER BY game_number",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    fn zone_rows(conn: &Connection, id: i64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM game_cards c JOIN games g ON g.id = c.game_id
             WHERE g.match_id = ?1 AND c.side = 'local' AND c.has_zone_data = 1",
            [id],
            |row| row.get(0),
        )
        .expect("count")
    }

    /// Every game card and mulligan card row of the match, as text.
    fn game_rows(conn: &Connection, id: i64) -> Vec<String> {
        let mut rows = Vec::new();
        for sql in [
            "SELECT c.game_id || ':' || c.side || ':' || c.catalog_id || ':' || c.main_qty
                 || ':' || c.opening_hand || ':' || c.seen
             FROM game_cards c JOIN games g ON g.id = c.game_id
             WHERE g.match_id = ?1 ORDER BY c.game_id, c.side, c.catalog_id",
            "SELECT 'mulligan ' || c.game_id || ':' || c.hand || ':' || c.catalog_id
             FROM game_mulligan_cards c JOIN games g ON g.id = c.game_id
             WHERE g.match_id = ?1 ORDER BY 1",
        ] {
            let mut stmt = conn.prepare(sql).expect("prepare");
            let found = stmt
                .query_map([id], |row| row.get::<_, String>(0))
                .expect("query");
            for row in found {
                rows.push(row.expect("row"));
            }
        }
        rows
    }

    /// An Arena deck holding the synced match `id`, from a version of `DECK`.
    fn arena_deck_with_match(conn: &mut Connection, label: &str) -> i64 {
        let id = arena_match(conn, ARENA);
        sync(conn, id);
        conn.execute(
            "UPDATE games SET on_play = (game_number = 1) WHERE match_id = ?1",
            [id],
        )
        .expect("play side");
        let deck_id: i64 = conn
            .query_row(
                "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (?1, 'Qarena deck', 'CPIONEER', 1, 1, 'file') RETURNING id",
                [label],
                |row| row.get(0),
            )
            .expect("deck");
        let version_id: i64 = conn
            .query_row(
                "INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES (?1, 'q-sig', 1) RETURNING id",
                [deck_id],
                |row| row.get(0),
            )
            .expect("version");
        conn.execute(
            "UPDATE matches SET deck_version_id = ?2 WHERE id = ?1",
            [id, version_id],
        )
        .expect("link");
        crate::test_support::move_deck_to_arena(conn, deck_id);
        deck_id
    }

    #[test]
    fn card_stats_on_an_arena_deck() {
        let (_dir, mut conn) = db("arena-card-stats");
        let deck_id = arena_deck_with_match(&mut conn, "q-arena-card-stats");
        let stats = crate::card_stats::card_stats(
            &conn,
            deck_id,
            &crate::card_stats::CardStatsFilter::all(),
        )
        .expect("card stats");
        let card = cards::ARENA_ID_OFFSET + 70_001;
        let row = stats
            .iter()
            .find(|stat| stat.catalog_id == card)
            .expect("the deck card has a row under its Arena catalog id");
        assert!(row.total_games > 0, "{row:?}");
        assert!(
            stats
                .iter()
                .all(|stat| stat.catalog_id >= cards::ARENA_ID_OFFSET)
        );
    }

    #[test]
    fn game_stats_on_an_arena_deck() {
        let (_dir, mut conn) = db("arena-game-stats");
        let deck_id = arena_deck_with_match(&mut conn, "q-arena-game-stats");
        let rows =
            crate::game_stats::game_stats(&conn, deck_id, decks::stats::Range::all_time(), None)
                .expect("game stats");
        let all: Vec<_> = rows
            .iter()
            .filter(|row| row.group == crate::game_stats::GameGroup::AllGames)
            .collect();
        assert!(all.iter().any(|row| row.wins + row.losses > 0), "{all:?}");
        let on_play = all
            .iter()
            .find(|row| row.split == crate::game_stats::PlaySplit::Play)
            .expect("play row");
        assert_eq!(on_play.wins, 1, "game 1 was on the play");
        assert_eq!(
            on_play.opponent_mulligans,
            Some(1.0),
            "from the synced games"
        );
    }

    #[test]
    fn mtgo_game_sync_leaves_an_arena_match_alone() {
        let (_dir, mut conn) = db("arena-mtgo-sync-skips");
        let id = arena_match(&mut conn, ARENA);
        sync(&mut conn, id);
        conn.execute("UPDATE matches SET state = 'complete' WHERE id = ?1", [id])
            .expect("complete");
        let before = game_rows(&conn, id);
        assert!(!before.is_empty());
        conn.execute("UPDATE matches SET games_synced_at = NULL", [])
            .expect("unstamped");
        let tx = conn.transaction().expect("tx");
        let changed =
            crate::game_sync::sync_match_games(&tx, id, chrono_tz::UTC, None).expect("sync");
        tx.commit().expect("commit");
        assert!(!changed, "nothing written");
        assert_eq!(game_rows(&conn, id), before);
        let synced_at: Option<i64> = conn
            .query_row(
                "SELECT games_synced_at FROM matches WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("games_synced_at");
        assert_eq!(synced_at, None);
    }

    #[test]
    fn card_resyncs_never_name_an_arena_match() {
        let (_dir, mut conn) = db("arena-card-resyncs");
        let id = arena_match(&mut conn, ARENA);
        sync(&mut conn, id);
        conn.execute("UPDATE matches SET state = 'complete' WHERE id = ?1", [id])
            .expect("complete");
        let card = cards::ARENA_ID_OFFSET + 70_001;
        assert!(
            crate::cards::log_matches_using(&conn, &[card])
                .expect("users")
                .is_empty()
        );
        // The match was played with an Arena deck.
        let deck_id: i64 = conn
            .query_row(
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES ('q-arena-deck', ?1, 'Qdeck', 'CPIONEER', 0, 0, 'file') RETURNING id",
                [arena_account(&conn)],
                |row| row.get(0),
            )
            .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, 'SIG', 0)",
            [deck_id],
        )
        .expect("version");
        conn.execute(
            "UPDATE matches SET deck_version_id = last_insert_rowid() WHERE id = ?1",
            [id],
        )
        .expect("deck version");
        let tx = conn.transaction().expect("tx");
        let done = crate::card_stats::regenerate(&tx, deck_id).expect("regenerate");
        tx.commit().expect("commit");
        assert!(done.log_tokens.is_empty());
    }

    #[test]
    fn sync_writes_each_game_and_stamps_once() {
        let (_dir, mut conn) = db("arena-sync-writes");
        let id = arena_match(&mut conn, ARENA);
        assert!(sync(&mut conn, id), "rows were written");
        assert_eq!(
            game_columns(&conn, id),
            vec![(1, Some(5), Some(1)), (2, Some(5), Some(1))]
        );
        // Two deck cards and the sideboard card in each game.
        assert_eq!(zone_rows(&conn, id), 6);
        let main: i64 = conn
            .query_row(
                "SELECT c.main_qty FROM game_cards c JOIN games g ON g.id = c.game_id
                 WHERE g.match_id = ?1 AND g.game_number = 1 AND c.catalog_id = ?2",
                params![id, cards::ARENA_ID_OFFSET + 70_001],
                |row| row.get(0),
            )
            .expect("main qty");
        assert_eq!(main, 2);
        assert_eq!(stamp(&conn, id), Some(("derived".to_string(), 2, 5_000)));
        assert!(!sync(&mut conn, id), "nothing new the second time");
    }

    #[test]
    fn a_batch_syncs_after_its_upserts_and_archiving() {
        let (_dir, mut conn) = db("arena-sync-batch");
        arena_account(&conn);
        store_messages(&mut conn, ARENA, &[1, 2]);
        let batch = WriteBatch {
            matches: vec![MatchWrite::Upsert {
                view: Box::new(arena_view(ARENA)),
                revision: 1,
            }],
            arena_archive_tokens: vec![ARENA.to_string()],
            arena_sync_tokens: vec![ARENA.to_string(), "q-unknown".to_string()],
            now_ms: 7_000,
            ..WriteBatch::default()
        };
        assert!(
            !WriteBatch {
                arena_sync_tokens: vec![ARENA.to_string()],
                ..WriteBatch::default()
            }
            .is_empty()
        );
        commit(&mut conn, &batch).expect("commit");
        let id = match_id(&conn, ARENA);
        assert_eq!(zone_rows(&conn, id), 6);
        assert_eq!(stamp(&conn, id), Some(("derived".to_string(), 2, 7_000)));
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM arena_game_messages"),
            0,
            "archived first"
        );
    }

    /// A match's archive that no longer reads back.
    fn corrupt_archive(conn: &Connection, token: &str) {
        conn.execute(
            "INSERT INTO arena_game_archives (match_token, messages, message_count, archived_at)
             VALUES (?1, CAST('not zstd' AS BLOB), 1, 1)",
            [token],
        )
        .expect("corrupt archive");
    }

    /// A match with live rows and a corrupt archive, and a good one.
    fn bad_and_good(conn: &mut Connection) -> (i64, i64) {
        let bad = arena_match(conn, "q-arena-bad");
        // The good match's messages take the same offsets: move these.
        conn.execute(
            "UPDATE arena_game_messages SET byte_offset = byte_offset + 100000",
            [],
        )
        .expect("move");
        corrupt_archive(conn, "q-arena-bad");
        (bad, arena_match(conn, ARENA))
    }

    #[test]
    fn a_corrupt_archive_skips_only_its_match() {
        let (_dir, mut conn) = db("arena-sync-corrupt");
        let (bad, good) = bad_and_good(&mut conn);
        let tokens = vec!["q-arena-bad".to_string(), ARENA.to_string()];
        commit(
            &mut conn,
            &WriteBatch {
                arena_archive_tokens: tokens.clone(),
                arena_sync_tokens: tokens,
                now_ms: 7_000,
                ..WriteBatch::default()
            },
        )
        .expect("the batch still commits");
        assert_eq!(stamp(&conn, good), Some(("derived".to_string(), 2, 7_000)));
        assert_eq!(zone_rows(&conn, good), 6);
        // Not stamped: the next start tries again (once per start).
        assert_eq!(stamp(&conn, bad), None);
        assert_eq!(zone_rows(&conn, bad), 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM arena_game_messages WHERE match_token = 'q-arena-bad'"
            ),
            4,
            "its live rows are kept"
        );
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![bad]);
    }

    const SEALED: &str = "ArenaDirect_EOE_Play_Sealed_20261106";

    /// A limited Arena match `ARENA` with its game messages, played while a
    /// course stating `pool` was open, and the runs synced before its game
    /// rows exist (as the batch that wrote its result does). The run name is
    /// then made stale, so a later fold shows by rewriting it.
    fn limited_match_in_a_course(
        label: &str,
        pool: Vec<u32>,
    ) -> (fixture_tools::TestDir, Connection) {
        let (dir, mut conn) = db(label);
        let id = arena_match(&mut conn, ARENA);
        let file_id = arena_log_file(&conn);
        conn.execute(
            "UPDATE matches SET match_type = 'limited', format = 'S6EOE' WHERE id = ?1",
            [id],
        )
        .expect("limited");
        conn.execute(
            "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                 match_token, logged_at, raw)
             VALUES (?1, 500, 0, 'room_state', ?2, 1, x'')",
            params![file_id, ARENA],
        )
        .expect("event");
        commit(
            &mut conn,
            &WriteBatch {
                arena_course_observations: vec![crate::arena_courses::CourseObservation {
                    file_id,
                    byte_offset: 100,
                    observed_at_ms: 100,
                    account_username: "Qarena#12345".to_string(),
                    course_id: "q-course".to_string(),
                    event_name: SEALED.to_string(),
                    module: "CreateMatch".to_string(),
                    wins: 0,
                    losses: 0,
                    deck_id: "q-deck".to_string(),
                    deck_name: "Sealed Deck".to_string(),
                    main: Vec::new(),
                    side: Vec::new(),
                    pool,
                }],
                arena_match_events: vec![(ARENA.to_string(), SEALED.to_string())],
                ..WriteBatch::default()
            },
        )
        .expect("course");
        let account_id = arena_account(&conn);
        let tx = conn.transaction().expect("tx");
        crate::arena_run_sync::sync_arena_runs(&tx, account_id, 6_000).expect("runs");
        tx.commit().expect("commit");
        assert_eq!(
            run_of_match(&conn),
            (
                "arena:q-course".to_string(),
                "Arena Direct · EOE Sealed".to_string()
            ),
            "no cards yet"
        );
        conn.execute("UPDATE leagues SET name = 'Qstale'", [])
            .expect("stale");
        (dir, conn)
    }

    /// The run key and name of `ARENA`'s run.
    fn run_of_match(conn: &Connection) -> (String, String) {
        conn.query_row(
            "SELECT l.run_key, l.name FROM matches m JOIN leagues l ON l.id = m.league_id
             WHERE m.token = ?1",
            [ARENA],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("run")
    }

    #[test]
    fn the_match_end_sync_folds_a_limited_matchs_runs_again() {
        // The course states another pool than the match's deck.
        let other_pool: Vec<u32> = (100_200_000..100_200_040).collect();
        let (_dir, mut conn) = limited_match_in_a_course("arena-sync-end-runs", other_pool);

        sync_arena_games(&mut conn, &[ARENA.to_string()], 7_000).expect("sync");

        assert_eq!(
            run_of_match(&conn).0,
            format!("arena:{SEALED}:{ARENA}"),
            "game 1's deck is another pool than the course's"
        );
    }

    #[test]
    fn the_match_end_sync_leaves_a_match_its_course_fits() {
        // The course's pool is the match's deck and sideboard.
        let pool: Vec<u32> = [70_001, 70_002, 70_003]
            .iter()
            .map(|grp| cards::ARENA_ID_OFFSET + grp)
            .collect();
        let (_dir, mut conn) = limited_match_in_a_course("arena-sync-end-fits", pool);

        sync_arena_games(&mut conn, &[ARENA.to_string()], 7_000).expect("sync");

        assert_eq!(
            run_of_match(&conn),
            ("arena:q-course".to_string(), "Qstale".to_string()),
            "the course already places the match: no fold"
        );
    }

    #[test]
    fn the_match_end_sync_skips_a_corrupt_archive() {
        let (_dir, mut conn) = db("arena-sync-end-corrupt");
        let (bad, good) = bad_and_good(&mut conn);
        let outcome = sync_arena_games(
            &mut conn,
            &["q-arena-bad".to_string(), ARENA.to_string()],
            7_000,
        )
        .expect("sync");
        assert_eq!(outcome.changed_tokens, vec![ARENA.to_string()]);
        assert_eq!(outcome.skipped, vec!["q-arena-bad".to_string()]);
        assert_eq!(stamp(&conn, good), Some(("derived".to_string(), 2, 7_000)));
        assert_eq!(stamp(&conn, bad), None);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM arena_game_messages"),
            4,
            "the good match is archived, the bad one's rows are kept"
        );
    }

    /// Game 1's messages with a command zone per seat: grpId 80001 for the
    /// local seat 1 and 80002 for the opponent.
    fn store_command_zone_messages(conn: &mut Connection, token: &str) {
        store_command_zone_messages_with(conn, token, false);
    }

    /// `three_opponent_cards`: the opponent's command zone holds three cards.
    fn store_command_zone_messages_with(
        conn: &mut Connection,
        token: &str,
        three_opponent_cards: bool,
    ) {
        let file_id = arena_log_file(conn);
        let mut json = game_messages(1);
        json[1] = gre(
            "{\"type\":\"GREMessageType_GameStateMessage\",\"systemSeatIds\":[1],\
             \"gameStateMessage\":{\"type\":\"GameStateType_Full\",\"gameStateId\":1,\
             \"players\":[{\"systemSeatNumber\":1,\"lifeTotal\":20,\"mulliganCount\":0},\
             {\"systemSeatNumber\":2,\"lifeTotal\":20,\"mulliganCount\":1}],\
             \"turnInfo\":{\"turnNumber\":5,\"activePlayer\":1},\
             \"zones\":[{\"zoneId\":31,\"type\":\"ZoneType_Hand\",\"ownerSeatId\":1,\
             \"objectInstanceIds\":[10]},\
             {\"zoneId\":32,\"type\":\"ZoneType_Command\",\"ownerSeatId\":1,\
             \"objectInstanceIds\":[11]},\
             {\"zoneId\":33,\"type\":\"ZoneType_Command\",\"ownerSeatId\":2,\
             \"objectInstanceIds\":[12]}],\
             \"gameObjects\":[{\"instanceId\":10,\"grpId\":70001,\
             \"type\":\"GameObjectType_Card\",\"zoneId\":31,\"visibility\":\"Visibility_Private\",\
             \"ownerSeatId\":1,\"controllerSeatId\":1},\
             {\"instanceId\":11,\"grpId\":80001,\"type\":\"GameObjectType_Card\",\
             \"zoneId\":32,\"visibility\":\"Visibility_Public\",\"ownerSeatId\":1,\
             \"controllerSeatId\":1},\
             {\"instanceId\":12,\"grpId\":80002,\"type\":\"GameObjectType_Card\",\
             \"zoneId\":33,\"visibility\":\"Visibility_Public\",\"ownerSeatId\":2,\
             \"controllerSeatId\":2}]}}",
        );
        if three_opponent_cards {
            let extra = r#"{"instanceId":13,"grpId":80003,"type":"GameObjectType_Card","zoneId":33,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2},{"instanceId":14,"grpId":80004,"type":"GameObjectType_Card","zoneId":33,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2},"#;
            json[1].1 = json[1]
                .1
                .replace(
                    "\"objectInstanceIds\":[12]",
                    "\"objectInstanceIds\":[12,13,14]",
                )
                .replace("\"gameObjects\":[", &format!("\"gameObjects\":[{extra}"));
        }
        let messages = json
            .into_iter()
            .enumerate()
            .map(|(index, (source, json))| NewGameMessage {
                file_id,
                byte_offset: 1_000 + index as u64,
                seq: 0,
                match_token: token.to_string(),
                game_number: Some(1),
                source,
                logged_at: 10_000 + index as i64,
                json,
            })
            .collect();
        commit(
            conn,
            &WriteBatch {
                arena_game_messages: messages,
                ..WriteBatch::default()
            },
        )
        .expect("messages");
    }

    fn brawl_view(token: &str, format: &str) -> MatchView {
        let mut view = arena_view(token);
        view.format = Some(format.to_string());
        view.game_structure = Some("bo1".to_string());
        view.games.truncate(1);
        view
    }

    fn offset(grp_id: i64) -> i64 {
        cards::ARENA_ID_OFFSET as i64 + grp_id
    }

    #[test]
    fn sync_writes_match_commanders_from_game_one() {
        let (_dir, mut conn) = db("arena-sync-commanders");
        arena_account(&conn);
        upsert(&mut conn, brawl_view(ARENA, "ARENA_HISTORIC_BRAWL"), 1);
        store_command_zone_messages(&mut conn, ARENA);
        let id = match_id(&conn, ARENA);
        conn.execute(
            "UPDATE matches SET archetypes_detected_at = 9 WHERE id = ?1",
            [id],
        )
        .expect("detected");
        assert!(sync(&mut conn, id));
        let commanders = crate::match_commanders::commanders_of(&conn, id).expect("read");
        assert_eq!(commanders.player, vec![offset(80_001)]);
        assert_eq!(commanders.opponent, vec![offset(80_002)]);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NULL"
            ),
            1,
            "new commanders ask detection again"
        );

        assert!(!sync(&mut conn, id), "a re-sync changes nothing");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM match_commanders"),
            2,
            "no duplicates"
        );
        assert_eq!(
            crate::match_commanders::commanders_of(&conn, id).expect("read"),
            commanders
        );
    }

    #[test]
    fn a_seat_with_three_command_zone_cards_syncs_once() {
        let (_dir, mut conn) = db("arena-sync-three-commanders");
        arena_account(&conn);
        upsert(&mut conn, brawl_view(ARENA, "ARENA_HISTORIC_BRAWL"), 1);
        store_command_zone_messages_with(&mut conn, ARENA, true);
        let id = match_id(&conn, ARENA);
        sync(&mut conn, id);
        let commanders = crate::match_commanders::commanders_of(&conn, id).expect("read");
        assert_eq!(commanders.opponent.len(), 2);
        assert!(!sync(&mut conn, id), "a second sync changes nothing");
    }

    #[test]
    fn a_match_outside_a_commander_format_gets_no_commanders() {
        let (_dir, mut conn) = db("arena-sync-no-commanders");
        arena_account(&conn);
        upsert(&mut conn, brawl_view(ARENA, "ARENA_HISTORIC"), 1);
        store_command_zone_messages(&mut conn, ARENA);
        let id = match_id(&conn, ARENA);
        sync(&mut conn, id);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_commanders"), 0);
    }

    #[test]
    fn player_commanders_fall_back_to_the_deck() {
        let (_dir, mut conn) = db("arena-sync-commander-fallback");
        arena_account(&conn);
        upsert(&mut conn, brawl_view(ARENA, "ARENA_HISTORIC_BRAWL"), 1);
        let id = match_id(&conn, ARENA);
        let signature = decks::signature(&[
            decks::DeckCard {
                catalog_id: offset(70_001) as u32,
                quantity: 1,
                sideboard: false,
            },
            decks::DeckCard {
                catalog_id: offset(83_902) as u32,
                quantity: 1,
                sideboard: true,
            },
        ]);
        let deck_id: i64 = conn
            .query_row(
                "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
                 VALUES ('q-brawl', 'Qbrawl deck', 'CPIONEER', 1, 1, 'file') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, 1)",
            params![deck_id, signature],
        )
        .expect("version");
        conn.execute(
            "UPDATE matches SET deck_version_id = (SELECT id FROM deck_versions) WHERE id = ?1",
            [id],
        )
        .expect("link");
        sync(&mut conn, id);
        let commanders = crate::match_commanders::commanders_of(&conn, id).expect("read");
        assert_eq!(commanders.player, vec![offset(83_902)]);
        assert!(commanders.opponent.is_empty());
        // The commander gets a `cards` stub, so the cards worker resolves
        // it and detection can run.
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM cards WHERE catalog_id = {}",
                    offset(83_902)
                )
            ),
            1
        );
    }

    #[test]
    fn no_messages_stamps_no_messages_until_messages_arrive() {
        let (_dir, mut conn) = db("arena-sync-no-messages");
        arena_account(&conn);
        upsert(&mut conn, arena_view(ARENA), 1);
        let id = match_id(&conn, ARENA);
        assert!(!sync(&mut conn, id));
        assert_eq!(
            stamp(&conn, id),
            Some(("no_messages".to_string(), 2, 5_000))
        );
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
        store_messages(&mut conn, ARENA, &[1]);
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![id]);
        assert!(sync(&mut conn, id));
        assert_eq!(stamp(&conn, id).expect("stamp").0, "derived");
    }

    #[test]
    fn messages_of_a_game_without_a_row_are_ignored() {
        let (_dir, mut conn) = db("arena-sync-no-row");
        let id = arena_match(&mut conn, ARENA);
        store_messages(&mut conn, ARENA, &[3]);
        assert!(sync(&mut conn, id));
        assert_eq!(game_columns(&conn, id).len(), 2);
    }

    #[test]
    fn projector_upsert_keeps_game_data_columns() {
        let (_dir, mut conn) = db("arena-sync-keeps");
        let id = arena_match(&mut conn, ARENA);
        sync(&mut conn, id);
        let before = game_columns(&conn, id);
        upsert(&mut conn, arena_view(ARENA), 2);
        assert_eq!(game_columns(&conn, id), before);
        assert_eq!(match_id(&conn, ARENA), id);
    }

    #[test]
    fn mtgo_upsert_unchanged() {
        let (_dir, mut conn) = db("arena-sync-mtgo");
        upsert(&mut conn, view(TOKEN, "Qopponent", 1_000), 1);
        let id = match_id(&conn, TOKEN);
        conn.execute(
            "INSERT INTO cards (catalog_id) VALUES (500) ON CONFLICT DO NOTHING",
            [],
        )
        .expect("card");
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, has_zone_data)
             SELECT id, 'local', 500, 1 FROM games WHERE match_id = ?1",
            [id],
        )
        .expect("zone rows");
        let mut changed = view(TOKEN, "Qopponent", 1_000);
        for game in &mut changed.games {
            game.turn_count = Some(4);
            game.opp_mulligans = Some(2);
        }
        upsert(&mut conn, changed, 2);
        assert_eq!(
            game_columns(&conn, id),
            vec![
                (1, Some(4), Some(2)),
                (2, Some(4), Some(2)),
                (3, Some(4), Some(2))
            ]
        );
    }

    #[test]
    fn manual_and_corrected_matches_are_skipped() {
        let (_dir, mut conn) = db("arena-sync-skips");
        insert_manual(&conn, "q-manual");
        move_match_to_arena(&conn, "q-manual");
        store_messages(&mut conn, "q-manual", &[1]);
        let manual = match_id(&conn, "q-manual");
        let corrected = arena_match(&mut conn, ARENA);
        conn.execute(
            "UPDATE matches SET corrected_at = 1 WHERE id = ?1",
            [corrected],
        )
        .expect("correct");
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
        for id in [manual, corrected] {
            assert!(!sync(&mut conn, id));
            assert_eq!(stamp(&conn, id), None);
            assert_eq!(zone_rows(&conn, id), 0);
        }
    }

    /// Edit match adds game 2 to an Arena match that ended with one game row
    /// (#573, #582): the correction's sync gives game 2 its rows from the
    /// stored messages, keeps the play side and turns the player entered,
    /// and stamps `games_synced_at` so detection takes the match.
    #[test]
    fn a_corrected_match_syncs_its_games_and_keeps_the_entered_columns() {
        let (_dir, mut conn) = db("arena-sync-corrected");
        arena_account(&conn);
        let mut one_game = arena_view(ARENA);
        one_game.games.truncate(1);
        upsert(&mut conn, one_game, 1);
        store_messages(&mut conn, ARENA, &[1, 2]);
        let id = match_id(&conn, ARENA);
        conn.execute("UPDATE matches SET corrected_at = 1 WHERE id = ?1;", [id])
            .expect("correct");
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                 local_mulligans, opp_mulligans, turn_count, started_at, ended_at)
             VALUES (?1, 2, 'win', 'unknown', 0, 0, 0, 9, 1, 2)",
            [id],
        )
        .expect("game 2");
        assert!(!sync(&mut conn, id), "the tracker's sync never takes it");

        let tx = conn.transaction().expect("tx");
        assert!(sync_corrected_arena_match(&tx, id, 5_000).expect("sync"));
        tx.commit().expect("commit");

        let entered: (bool, i64, i64) = conn
            .query_row(
                "SELECT on_play, turn_count, opp_mulligans FROM games
                 WHERE match_id = ?1 AND game_number = 2",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("game 2");
        assert_eq!(entered, (false, 9, 1), "entered kept, mulligans derived");
        let game_two_zone_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM game_cards c JOIN games g ON g.id = c.game_id
                 WHERE g.match_id = ?1 AND g.game_number = 2 AND c.has_zone_data = 1",
                [id],
                |row| row.get(0),
            )
            .expect("count");
        assert!(game_two_zone_rows > 0);
        let synced: Option<i64> = conn
            .query_row(
                "SELECT games_synced_at FROM matches WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("synced");
        assert!(synced.is_some());
    }

    #[test]
    fn the_corrected_sync_leaves_mtgo_and_manual_matches_alone() {
        let (_dir, mut conn) = db("arena-sync-corrected-skips");
        insert_manual(&conn, "q-manual");
        move_match_to_arena(&conn, "q-manual");
        store_messages(&mut conn, "q-manual", &[1]);
        upsert(&mut conn, view(TOKEN, "Qopponent", 1_000), 1);
        for token in ["q-manual", TOKEN] {
            let id = match_id(&conn, token);
            let tx = conn.transaction().expect("tx");
            assert!(!sync_corrected_arena_match(&tx, id, 5_000).expect("sync"));
            tx.commit().expect("commit");
            assert_eq!(stamp(&conn, id), None);
        }
    }

    #[test]
    fn stamp_skips_second_sync() {
        let (_dir, mut conn) = db("arena-sync-stamp");
        let id = arena_match(&mut conn, ARENA);
        // An MTGO match is never an Arena sync's.
        upsert(&mut conn, view(TOKEN, "Qopponent", 1_000), 1);
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![id]);
        archive_and_sync(&mut conn, ARENA);
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
    }

    #[test]
    fn version_bump_resyncs() {
        let (_dir, mut conn) = db("arena-sync-version");
        let id = arena_match(&mut conn, ARENA);
        archive_and_sync(&mut conn, ARENA);
        conn.execute(
            "UPDATE arena_game_syncs SET derived_version = ?2 WHERE match_id = ?1",
            params![id, ARENA_GAME_SYNC_VERSION - 1],
        )
        .expect("old stamp");
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![id]);
        let games = dump(&conn, "games");
        let cards = dump(&conn, "game_cards");
        let mulligans = dump(&conn, "game_mulligan_cards");
        assert!(!cards.is_empty());
        archive_and_sync(&mut conn, ARENA);
        assert_eq!(dump(&conn, "games"), games);
        assert_eq!(dump(&conn, "game_cards"), cards);
        assert_eq!(dump(&conn, "game_mulligan_cards"), mulligans);
        assert_eq!(
            stamp(&conn, id),
            Some(("derived".to_string(), ARENA_GAME_SYNC_VERSION, 5_000))
        );
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
    }

    /// Each game's local zone rows, game 1 first.
    fn zone_rows_per_game(conn: &Connection, id: i64) -> Vec<(i64, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT g.game_number, (SELECT COUNT(*) FROM game_cards c
                     WHERE c.game_id = g.id AND c.has_zone_data = 1)
                 FROM games g WHERE g.match_id = ?1 ORDER BY g.game_number",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    #[test]
    fn a_game_whose_fold_no_longer_starts_is_cleared() {
        let (_dir, mut conn) = db("arena-sync-clears");
        let id = arena_match(&mut conn, ARENA);
        sync(&mut conn, id);
        assert_eq!(zone_rows_per_game(&conn, id), vec![(1, 3), (2, 3)]);
        // Game 2's messages no longer show a game starting (only its deck),
        // and the sync runs again (a version bump).
        conn.execute_batch(
            "DELETE FROM arena_game_messages WHERE game_number = 2;
             UPDATE arena_game_syncs SET derived_version = 0;",
        )
        .expect("game 2 without a start");
        let file_id = arena_log_file(&conn);
        let (source, json) = game_messages(2).remove(0);
        commit(
            &mut conn,
            &WriteBatch {
                arena_game_messages: vec![NewGameMessage {
                    file_id,
                    byte_offset: 2_000,
                    seq: 0,
                    match_token: ARENA.to_string(),
                    game_number: Some(2),
                    source,
                    logged_at: 20_000,
                    json,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("deck only");
        assert!(sync(&mut conn, id), "game 2's rows went");
        assert_eq!(zone_rows_per_game(&conn, id), vec![(1, 3), (2, 0)]);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM game_mulligan_cards m JOIN games g ON g.id = m.game_id
                 WHERE g.game_number = 2"
            ),
            0
        );
    }

    #[test]
    fn a_derived_match_with_live_rows_is_due_until_archived() {
        let (_dir, mut conn) = db("arena-sync-late-rows");
        let id = arena_match(&mut conn, ARENA);
        commit(
            &mut conn,
            &WriteBatch {
                arena_archive_tokens: vec![ARENA.to_string()],
                arena_sync_tokens: vec![ARENA.to_string()],
                now_ms: 6_000,
                ..WriteBatch::default()
            },
        )
        .expect("decided batch");
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
        // A late message after the match was archived and derived.
        store_messages(&mut conn, ARENA, &[3]);
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![id]);
        // The sync batch archives it too, so the match is not due again.
        commit(
            &mut conn,
            &WriteBatch {
                arena_archive_tokens: vec![ARENA.to_string()],
                arena_sync_tokens: vec![ARENA.to_string()],
                now_ms: 7_000,
                ..WriteBatch::default()
            },
        )
        .expect("sync batch");
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM arena_game_messages"), 0);
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
        assert_eq!(stamp(&conn, id), Some(("derived".to_string(), 2, 7_000)));
    }

    #[test]
    fn only_log_matches_are_synced() {
        let (_dir, mut conn) = db("arena-sync-log-only");
        let id = arena_match(&mut conn, ARENA);
        for source in ["imported", "history"] {
            conn.execute(
                "UPDATE matches SET source = ?2 WHERE id = ?1",
                params![id, source],
            )
            .expect("source");
            assert!(
                arena_matches_to_sync(&conn).expect("due").is_empty(),
                "{source}"
            );
            assert!(!sync(&mut conn, id), "{source}");
            assert_eq!(stamp(&conn, id), None, "{source}");
        }
        conn.execute("UPDATE matches SET source = 'log' WHERE id = ?1", [id])
            .expect("log");
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![id]);
    }

    // A match synced while it was ended (no result yet) and settled later
    // by quiet completion gets no match-end sync: it is due again until the
    // sync stamps `games_synced_at`, so detection and its report take it.
    #[test]
    fn a_match_synced_before_it_completed_is_due_again() {
        let (_dir, mut conn) = db("arena-sync-completed-later");
        arena_account(&conn);
        let mut ended = arena_view(ARENA);
        ended.state = mtgo_match::MatchState::Ended;
        upsert(&mut conn, ended, 1);
        store_messages(&mut conn, ARENA, &[1, 2]);
        let id = match_id(&conn, ARENA);
        archive_and_sync(&mut conn, ARENA);
        assert!(stamp(&conn, id).is_some());
        assert_eq!(games_synced_at(&conn, id), None);
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
        upsert(&mut conn, arena_view(ARENA), 2);
        assert_eq!(arena_matches_to_sync(&conn).expect("due"), vec![id]);
        archive_and_sync(&mut conn, ARENA);
        assert!(games_synced_at(&conn, id).is_some());
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
    }

    #[test]
    fn in_progress_matches_wait() {
        let (_dir, mut conn) = db("arena-sync-in-progress");
        arena_account(&conn);
        let mut open = arena_view(ARENA);
        open.state = mtgo_match::MatchState::InProgress;
        upsert(&mut conn, open, 1);
        store_messages(&mut conn, ARENA, &[1]);
        assert!(arena_matches_to_sync(&conn).expect("due").is_empty());
    }

    fn games_synced_at(conn: &Connection, id: i64) -> Option<i64> {
        conn.query_row(
            "SELECT games_synced_at FROM matches WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .expect("games_synced_at")
    }

    #[test]
    fn a_derived_match_is_stamped_for_detection() {
        let (_dir, mut conn) = db("arena-sync-detection");
        let id = arena_match(&mut conn, ARENA);
        assert!(
            crate::archetypes::due_matches(&conn, 10)
                .expect("due")
                .is_empty()
        );
        sync(&mut conn, id);
        assert!(games_synced_at(&conn, id).is_some());
        assert_eq!(
            crate::archetypes::due_matches(&conn, 10).expect("due"),
            [id]
        );
        assert_eq!(
            crate::archetypes::due_log_matches(&conn, 10).expect("due"),
            [id]
        );
    }

    #[test]
    fn a_match_without_messages_is_stamped_too() {
        let (_dir, mut conn) = db("arena-sync-detection-empty");
        arena_account(&conn);
        upsert(&mut conn, arena_view(ARENA), 1);
        let id = match_id(&conn, ARENA);
        assert!(!sync(&mut conn, id));
        assert_eq!(stamp(&conn, id).map(|s| s.0), Some("no_messages".into()));
        assert!(games_synced_at(&conn, id).is_some());
    }

    #[test]
    fn a_changed_derivation_asks_detection_again() {
        let (_dir, mut conn) = db("arena-sync-redetect");
        let id = arena_match(&mut conn, ARENA);
        sync(&mut conn, id);
        conn.execute("UPDATE matches SET archetypes_detected_at = 9", [])
            .expect("detected");
        let detected = |conn: &Connection| -> Option<i64> {
            conn.query_row("SELECT archetypes_detected_at FROM matches", [], |row| {
                row.get(0)
            })
            .expect("detected_at")
        };
        assert!(!sync(&mut conn, id), "nothing changed");
        assert_eq!(detected(&conn), Some(9));
        conn.execute("DELETE FROM game_cards", []).expect("wipe");
        assert!(sync(&mut conn, id), "rows were written again");
        assert_eq!(detected(&conn), None);
    }

    // ---- Pulled from another device (cross-device sync spec section 7) ----

    const BO3: &str = "q-arena-bo3";

    /// The account and opponent seats of the match `token`:
    /// `(username, platform, arena_user_id, display_name)` each.
    type Seat = (String, String, Option<String>, Option<String>);

    fn seats(conn: &Connection, token: &str) -> (Seat, Seat) {
        conn.query_row(
            "SELECT a.username, a.platform, a.arena_user_id, a.display_name,
                 o.username, o.platform, NULL, o.display_name
             FROM matches m JOIN accounts a ON a.id = m.account_id
             JOIN opponents o ON o.id = m.opponent_id WHERE m.token = ?1",
            [token],
            |row| {
                Ok((
                    (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?),
                    (row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?),
                ))
            },
        )
        .expect("seats")
    }

    fn seat(username: &str, platform: &str, user_id: Option<&str>, shown: &str) -> Seat {
        (
            username.to_string(),
            platform.to_string(),
            user_id.map(str::to_string),
            Some(shown.to_string()),
        )
    }

    /// Device A's Bo3 Arena match `BO3`, synced, its account shown as
    /// Qshown and its opponent as Qfoe.
    fn recorded_bo3(conn: &mut Connection) {
        arena_match(conn, BO3);
        archive_and_sync(conn, BO3);
        conn.execute(
            "UPDATE accounts SET display_name = 'Qshown' WHERE platform = 'arena'",
            [],
        )
        .expect("display name");
        name_arena_opponent(conn, "arena:Qfoe-id", "Qfoe");
    }

    /// Device A's Brawl match `ARENA` with both seats' commanders, synced.
    fn recorded_brawl(conn: &mut Connection) {
        arena_account(conn);
        upsert(conn, brawl_view(ARENA, "ARENA_HISTORIC_BRAWL"), 1);
        store_command_zone_messages(conn, ARENA);
        archive_and_sync(conn, ARENA);
    }

    fn source_of(conn: &Connection, token: &str) -> String {
        conn.query_row(
            "SELECT source FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("source")
    }

    #[test]
    fn pulled_arena_match_lands_under_arena_account() {
        let (_a, mut here) = db("arena-pull-account-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-account-there");
        pull_into(&mut there, pulled_from(&here, BO3), 9_000);
        assert_eq!(source_of(&there, BO3), "imported");
        assert_eq!(
            seats(&there, BO3),
            (
                seat("Qarena#12345", "arena", Some("Qarena-id"), "Qshown"),
                seat("arena:Qfoe-id", "arena", None, "Qfoe"),
            )
        );
    }

    /// The deck pages filter Arena matches by kind (`game_structure`), so a
    /// pulled Bo1 match keeps `bo1` and counts on its deck's Bo1 page.
    #[test]
    fn a_pulled_arena_bo1_match_keeps_its_kind_for_the_deck_pages() {
        let (_a, mut here) = db("arena-pull-kind-here");
        recorded_brawl(&mut here);
        let (_b, mut there) = db("arena-pull-kind-there");
        pull_into(&mut there, pulled_from(&here, ARENA), 9_000);
        let kind: Option<String> = there
            .query_row(
                "SELECT game_structure FROM matches WHERE token = ?1",
                [ARENA],
                |row| row.get(0),
            )
            .expect("kind");
        assert_eq!(kind.as_deref(), Some("bo1"));
        let account: i64 = there
            .query_row(
                "SELECT account_id FROM matches WHERE token = ?1",
                [ARENA],
                |row| row.get(0),
            )
            .expect("account");
        let deck_id: i64 = there
            .query_row(
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at,
                     source)
                 VALUES ('qbrawl-deck', ?1, 'Qbrawl', 'ARENA_HISTORIC_BRAWL', 0, 0, 'file')
                 RETURNING id",
                [account],
                |row| row.get(0),
            )
            .expect("deck");
        there
            .execute(
                "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, 'QSIG', 0)",
                [deck_id],
            )
            .expect("version");
        there
            .execute(
                "UPDATE matches SET deck_version_id = last_insert_rowid() WHERE token = ?1",
                [ARENA],
            )
            .expect("link");
        let kinds = crate::deck_pages::deck_kinds(&there, deck_id).expect("kinds");
        assert_eq!((kinds.bo1, kinds.bo3), (1, 0));
        assert_eq!(kinds.latest, Some(crate::platform::BestOf::One));
    }

    #[test]
    fn a_pulled_arena_match_finds_its_renamed_account() {
        let (_a, mut here) = db("arena-pull-renamed-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-renamed-there");
        there
            .execute(
                "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Qrenamed#99999', 0, 'arena', 'Qarena-id')",
                [],
            )
            .expect("renamed account");
        pull_into(&mut there, pulled_from(&here, BO3), 9_000);
        assert_eq!(
            seats(&there, BO3).0.0,
            "Qrenamed#99999",
            "matched by user id, never renamed back"
        );
        assert_eq!(number(&there, "SELECT COUNT(*) FROM accounts"), 1);
    }

    #[test]
    fn a_pulled_arena_match_with_no_player_takes_the_current_arena_account() {
        let (_a, mut here) = db("arena-pull-no-player-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-no-player-there");
        arena_account(&there);
        there
            .execute(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qmtgo', 0)",
                [],
            )
            .expect("an MTGO account");
        let mut pulled = pulled_from(&here, BO3);
        pulled.imported.local_username = String::new();
        pull_into(&mut there, pulled, 9_000);
        assert_eq!(seats(&there, BO3).0.1, "arena");
    }

    #[test]
    fn pulled_arena_match_has_game_rows_and_commanders() {
        let (_a, mut here) = db("arena-pull-rows-here");
        recorded_brawl(&mut here);
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-rows-there");
        for token in [ARENA, BO3] {
            let local = pulled_rows(&here, token);
            assert!(local.iter().any(|row| row.starts_with("card ")));
            pull_into(&mut there, pulled_from(&here, token), 9_000);
            assert_eq!(pulled_rows(&there, token), local, "{token}");
        }
        assert!(
            pulled_rows(&there, ARENA)
                .iter()
                .any(|row| row.starts_with("commander "))
        );
        assert_eq!(number(&there, "SELECT COUNT(*) FROM synced_sources"), 2);
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NOT NULL"
            ),
            2
        );
        // Pulled sources never enter the local fact tables.
        for table in [
            "arena_game_messages",
            "arena_game_archives",
            "arena_log_events",
        ] {
            assert_eq!(number(&there, &format!("SELECT COUNT(*) FROM {table}")), 0);
        }
        assert!(arena_matches_to_sync(&there).expect("due").is_empty());
        // The messages read back through the one seam the readers use.
        let messages = crate::arena_games::game_messages(&there, BO3).expect("messages");
        assert_eq!(messages.len(), 4);
        assert_eq!(
            group_by_game(messages),
            group_by_game(crate::arena_games::game_messages(&here, BO3).expect("here"))
        );
    }

    #[test]
    fn pulled_arena_match_without_sources_is_refused() {
        let (_a, mut here) = db("arena-pull-no-sources-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-no-sources-there");
        let mut pulled = pulled_from(&here, BO3);
        pulled.sources = None;
        pull_into(&mut there, pulled, 9_000);
        assert_eq!(number(&there, "SELECT COUNT(*) FROM matches"), 0);
        // Parked (#830): nothing recorded as synced, only the refused copy.
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM synced_rows
                 WHERE hash IS NULL AND parked_reason = 'no_sources'"
            ),
            1
        );
        assert_eq!(number(&there, "SELECT COUNT(*) FROM synced_rows"), 1);
    }

    #[test]
    fn own_pulled_bundle_leaves_live_match_alone() {
        let (_a, mut here) = db("arena-pull-own");
        recorded_brawl(&mut here);
        let before = pulled_rows(&here, ARENA);
        let revision = number(&here, "SELECT revision FROM matches");
        let own = pulled_from(&here, ARENA);
        pull_into(&mut here, own, 9_000);
        assert_eq!(source_of(&here, ARENA), "log");
        assert_eq!(pulled_rows(&here, ARENA), before);
        assert_eq!(number(&here, "SELECT revision FROM matches"), revision);
        assert_eq!(number(&here, "SELECT COUNT(*) FROM synced_sources"), 0);
    }

    #[test]
    fn local_recording_takes_over_and_drops_synced_sources() {
        let (_a, mut here) = db("arena-pull-takeover-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-takeover-there");
        pull_into(&mut there, pulled_from(&here, BO3), 9_000);
        assert_eq!(number(&there, "SELECT COUNT(*) FROM synced_sources"), 1);

        // The same match recorded on this device too.
        upsert(&mut there, arena_view(BO3), 1);
        assert_eq!(source_of(&there, BO3), "log");
        assert_eq!(number(&there, "SELECT COUNT(*) FROM synced_sources"), 0);
        let id = match_id(&there, BO3);
        assert_eq!(
            arena_matches_to_sync(&there).expect("due"),
            vec![id],
            "the local sync takes it over"
        );
        store_messages(&mut there, BO3, &[1, 2]);
        archive_and_sync(&mut there, BO3);
        assert_eq!(stamp(&there, id).expect("stamp").0, "derived");
        assert_eq!(pulled_rows(&there, BO3), pulled_rows(&here, BO3));
    }

    /// A raise of `ARENA_GAME_SYNC_VERSION` derives a pulled match again
    /// from the source it came with, through the batch the tracker sends at
    /// start; its sync row (hash, touch key) is not changed.
    #[test]
    fn arena_version_raise_rederives_pulled_matches() {
        let (_a, mut here) = db("arena-raise-pulled-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-raise-pulled-there");
        pull_into(&mut there, pulled_from(&here, BO3), 9_000);
        let id = match_id(&there, BO3);
        let rows = pulled_rows(&there, BO3);
        let synced = dump(&there, "synced_rows");
        let key = crate::sync::match_touch_key(&there, id).expect("key");
        assert!(arena_matches_to_sync(&there).expect("due").is_empty());

        // Derived under older rules: the rows differ from today's.
        there
            .execute_batch(
                "DELETE FROM game_cards; DELETE FROM game_mulligan_cards;
                 UPDATE arena_game_syncs SET derived_version = 1;",
            )
            .expect("an older derive");
        assert_eq!(arena_matches_to_sync(&there).expect("due"), vec![id]);
        archive_and_sync(&mut there, BO3);

        assert_eq!(pulled_rows(&there, BO3), rows);
        assert_eq!(
            stamp(&there, id),
            Some(("derived".to_string(), ARENA_GAME_SYNC_VERSION, 5_000))
        );
        assert!(arena_matches_to_sync(&there).expect("due").is_empty());
        assert_eq!(source_of(&there, BO3), "imported");
        assert_eq!(dump(&there, "synced_rows"), synced);
        assert_eq!(
            crate::sync::match_touch_key(&there, id).expect("key"),
            key,
            "no push"
        );
    }

    /// A pulled match whose stored source does not read back is skipped and
    /// stamped at the current version, so it waits for the next raise
    /// instead of failing every start; a log match in the same batch still
    /// syncs.
    #[test]
    fn a_corrupt_pulled_arena_source_is_skipped_until_the_next_raise() {
        let (_a, mut here) = db("arena-raise-corrupt-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-raise-corrupt-there");
        pull_into(&mut there, pulled_from(&here, BO3), 9_000);
        let pulled = match_id(&there, BO3);
        let local = arena_match(&mut there, ARENA);
        there
            .execute_batch(
                "UPDATE arena_game_syncs SET derived_version = 1;
                 UPDATE synced_sources SET payload = x'00';",
            )
            .expect("an older derive of a corrupt source");
        assert_eq!(
            arena_matches_to_sync(&there).expect("due"),
            vec![pulled, local]
        );
        let tokens = vec![BO3.to_string(), ARENA.to_string()];
        commit(
            &mut there,
            &WriteBatch {
                arena_archive_tokens: tokens.clone(),
                arena_sync_tokens: tokens,
                now_ms: 5_000,
                ..WriteBatch::default()
            },
        )
        .expect("the batch commits");
        assert_eq!(stamp(&there, local).expect("stamp").0, "derived");
        assert_eq!(
            stamp(&there, pulled),
            Some(("no_messages".to_string(), ARENA_GAME_SYNC_VERSION, 5_000))
        );
        assert!(arena_matches_to_sync(&there).expect("due").is_empty());
    }

    /// The commander cards of `recorded_brawl`, as the cards worker fills
    /// them.
    fn commander_cards(conn: &Connection) {
        for (grp_id, oracle_id, name, identity) in [
            (80_001, "q-oracle-1", "Qcommander One", "G"),
            (80_002, "q-oracle-2", "Qcommander Two", "U,R"),
        ] {
            conn.execute(
                "INSERT INTO cards (catalog_id, oracle_id, name, color_identity)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (catalog_id) DO UPDATE SET oracle_id = excluded.oracle_id,
                     name = excluded.name, color_identity = excluded.color_identity",
                params![offset(grp_id), oracle_id, name, identity],
            )
            .expect("card");
        }
    }

    fn commander_uuid(oracle_id: &str) -> String {
        let key = archetypes::format_key("ARENA_HISTORIC_BRAWL").expect("a format key");
        archetypes::commander::commander_uuid(&key, &[oracle_id.to_string()])
    }

    /// Device A's Brawl match with its commander archetypes detected.
    fn detected_brawl(conn: &mut Connection) {
        recorded_brawl(conn);
        commander_cards(conn);
        let key = archetypes::format_key("ARENA_HISTORIC_BRAWL").expect("a format key");
        let id = match_id(conn, ARENA);
        for (oracle_id, name, opponent) in [
            ("q-oracle-1", "Qcommander One", false),
            ("q-oracle-2", "Qcommander Two", true),
        ] {
            let archetype_id: i64 = conn
                .query_row(
                    "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'commander', 1, 1) RETURNING id",
                    params![commander_uuid(oracle_id), name, key],
                    |row| row.get(0),
                )
                .expect("archetype");
            conn.execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence,
                     manual)
                 VALUES (?1, ?2, ?3, 1.0, 0)",
                params![id, opponent, archetype_id],
            )
            .expect("seat");
        }
    }

    #[test]
    fn pulled_brawl_match_gets_commander_archetype() {
        let (_a, mut here) = db("arena-pull-brawl-here");
        detected_brawl(&mut here);
        let (_b, mut there) = db("arena-pull-brawl-there");
        commander_cards(&there);
        pull_into(&mut there, pulled_from(&here, ARENA), 9_000);
        let key = archetypes::format_key("ARENA_HISTORIC_BRAWL").expect("a format key");
        let rows: Vec<(String, String, String, Option<String>, String, bool)> = {
            let mut stmt = there
                .prepare(
                    "SELECT a.uuid, a.name, a.format, a.color_identity, a.source, x.is_opponent
                     FROM match_archetypes x JOIN archetypes a ON a.id = x.archetype_id
                     ORDER BY x.is_opponent",
                )
                .expect("prepare");
            let found = stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .expect("query");
            found.map(|row| row.expect("row")).collect()
        };
        assert_eq!(
            rows,
            vec![
                (
                    commander_uuid("q-oracle-1"),
                    "Qcommander One".to_string(),
                    key.clone(),
                    Some("G".to_string()),
                    "commander".to_string(),
                    false
                ),
                (
                    commander_uuid("q-oracle-2"),
                    "Qcommander Two".to_string(),
                    key,
                    Some("U,R".to_string()),
                    "commander".to_string(),
                    true
                ),
            ]
        );
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM archetypes WHERE source = 'downloaded'"
            ),
            0
        );
    }

    #[test]
    fn a_pulled_brawl_match_without_card_data_leaves_its_commanders_to_detection() {
        let (_a, mut here) = db("arena-pull-brawl-waits-here");
        detected_brawl(&mut here);
        let (_b, mut there) = db("arena-pull-brawl-waits-there");
        pull_into(&mut there, pulled_from(&here, ARENA), 9_000);
        assert_eq!(
            number(
                &there,
                &format!(
                    "SELECT COUNT(*) FROM archetypes WHERE uuid IN ('{}', '{}')",
                    commander_uuid("q-oracle-1"),
                    commander_uuid("q-oracle-2")
                )
            ),
            0,
            "no stub that is not a commander row"
        );
        assert_eq!(number(&there, "SELECT COUNT(*) FROM match_archetypes"), 0);
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NULL"
            ),
            1,
            "detection makes them once the cards are known"
        );
    }

    // ---- Review fixes (XS4 fix round 1) ----

    #[test]
    fn a_pulled_corrected_arena_match_keeps_the_senders_play_side_and_turns() {
        let (_a, mut here) = db("arena-pull-corrected-here");
        recorded_bo3(&mut here);
        here.execute("UPDATE matches SET corrected_at = 1", [])
            .expect("corrected");
        here.execute(
            "UPDATE games SET on_play = NOT on_play, turn_count = 42 WHERE game_number = 1",
            [],
        )
        .expect("the player's fix");
        let (_b, mut there) = db("arena-pull-corrected-there");
        pull_into(&mut there, pulled_from(&here, BO3), 9_000);
        let games = |conn: &Connection| -> Vec<String> {
            pulled_rows(conn, BO3)
                .into_iter()
                .filter(|row| row.starts_with("game "))
                .collect()
        };
        assert_eq!(games(&there), games(&here));
    }

    #[test]
    fn a_pulled_arena_match_with_no_player_takes_the_account_of_its_user_id() {
        let (_a, mut here) = db("arena-pull-no-player-id-here");
        recorded_bo3(&mut here);
        let (_b, mut there) = db("arena-pull-no-player-id-there");
        // Another Arena account is the current one (it has the newest match).
        there
            .execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                 VALUES (1, 'Qother#11111', 0, 'arena', 'Qother-id'),
                        (2, 'Qarena#12345', 0, 'arena', 'Qarena-id');
                 INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES ('q-other-match', 1, 'imported', 'complete', 'win', 99999999,
                     99999999, 1, 0);",
            )
            .expect("accounts");
        let mut pulled = pulled_from(&here, BO3);
        pulled.imported.local_username = String::new();
        pull_into(&mut there, pulled, 9_000);
        assert_eq!(seats(&there, BO3).0.0, "Qarena#12345");
    }

    /// What the detection worker does for the due commander format matches:
    /// each seat's commander archetype found or made, then the seats set.
    fn detect_commanders(conn: &mut Connection) {
        use crate::archetype_edit::{
            ArchetypeEdit, ArchetypeEditOutcome, MatchDetection, SeatResult, edit_archetypes,
        };
        let due = crate::archetypes::due_matches(conn, 10).expect("due");
        assert!(!due.is_empty(), "the match is due");
        for match_id in due {
            let input = crate::archetypes::detection_input(conn, match_id)
                .expect("input")
                .expect("ready");
            let seats = input.commanders.clone().expect("a commander format match");
            let key =
                archetypes::format_key(input.format.as_deref().expect("format")).expect("a key");
            let mut ensure = |set: &crate::archetypes::CommanderSet| -> i64 {
                let edit = ArchetypeEdit::EnsureCommander {
                    uuid: archetypes::commander::commander_uuid(&key, &set.oracle_ids),
                    name: archetypes::commander::commander_name(&set.names),
                    format: key.clone(),
                    color_identity: archetypes::commander::commander_colors(&set.color_identities),
                    now_ms: 10_000,
                };
                match edit_archetypes(conn, &edit).expect("ensure") {
                    ArchetypeEditOutcome::Commander { archetype_id } => archetype_id,
                    other => panic!("{other:?}"),
                }
            };
            let player = seats.player.as_ref().map(&mut ensure);
            let opponent = seats.opponent.as_ref().map(&mut ensure);
            let seat = |archetype_id: i64| SeatResult {
                archetype_id,
                confidence: 1.0,
            };
            let edit = ArchetypeEdit::SetDetections {
                results: vec![MatchDetection {
                    match_id,
                    deck_archetype_id: input.deck_archetype_id,
                    deck_archetype: player,
                    player: player.map(seat),
                    opponent: opponent.map(seat),
                }],
                now_ms: 10_000,
            };
            edit_archetypes(conn, &edit).expect("detections");
        }
    }

    /// The uuid of each seat's archetype, player first.
    fn seat_uuids(conn: &Connection) -> Vec<(bool, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT x.is_opponent, a.uuid FROM match_archetypes x
                 JOIN archetypes a ON a.id = x.archetype_id ORDER BY x.is_opponent",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    #[test]
    fn a_pulled_brawl_seat_waiting_for_its_card_leaves_the_match_to_detection() {
        let (_a, mut here) = db("arena-pull-brawl-half-here");
        detected_brawl(&mut here);
        let (_b, mut there) = db("arena-pull-brawl-half-there");
        // Only the opponent's commander is known here, and its row is held.
        there
            .execute(
                "INSERT INTO cards (catalog_id, oracle_id, name, color_identity)
                 VALUES (?1, 'q-oracle-2', 'Qcommander Two', 'U,R')",
                [offset(80_002)],
            )
            .expect("card");
        let key = archetypes::format_key("ARENA_HISTORIC_BRAWL").expect("a format key");
        there
            .execute(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES (?1, 'Qcommander Two', ?2, 'commander', 1, 1)",
                params![commander_uuid("q-oracle-2"), key],
            )
            .expect("held");
        pull_into(&mut there, pulled_from(&here, ARENA), 9_000);
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM matches WHERE archetypes_detected_at IS NULL"
            ),
            1,
            "a seat was skipped: detection still runs"
        );

        commander_cards(&there);
        detect_commanders(&mut there);
        assert_eq!(
            seat_uuids(&there),
            vec![
                (false, commander_uuid("q-oracle-1")),
                (true, commander_uuid("q-oracle-2")),
            ]
        );
    }

    // #827: a hand-set commander side this device does not hold is still
    // the player's choice: it lands as a manual seat (nothing is carried),
    // detection leaves it alone, and once the commanders' cards are known
    // its archetype becomes the commander row.
    #[test]
    fn a_manual_commander_side_not_held_here_becomes_a_seat() {
        let (_a, mut here) = db("arena-pull-brawl-manual-here");
        detected_brawl(&mut here);
        here.execute(
            "UPDATE match_archetypes SET manual = 1 WHERE is_opponent = 0",
            [],
        )
        .expect("hand-set");
        let (_b, mut there) = db("arena-pull-brawl-manual-there");
        pull_into(&mut there, pulled_from(&here, ARENA), 9_000);
        let carry: String = there
            .query_row("SELECT carry FROM synced_rows", [], |row| row.get(0))
            .expect("carry");
        let carry: serde_json::Value = serde_json::from_str(&carry).expect("json");
        assert!(
            carry.get(sync_bundles::UNPLACED_ARCHETYPES).is_none(),
            "{carry}"
        );
        assert_eq!(
            seat_uuids(&there),
            vec![(false, commander_uuid("q-oracle-1"))]
        );
        let manual_seat =
            "SELECT COUNT(*) FROM match_archetypes WHERE is_opponent = 0 AND manual = 1";
        assert_eq!(number(&there, manual_seat), 1);

        commander_cards(&there);
        detect_commanders(&mut there);
        assert_eq!(
            seat_uuids(&there),
            vec![
                (false, commander_uuid("q-oracle-1")),
                (true, commander_uuid("q-oracle-2")),
            ]
        );
        assert_eq!(number(&there, manual_seat), 1, "detection left it");
        assert_eq!(
            number(
                &there,
                "SELECT COUNT(*) FROM archetypes WHERE source = 'commander' AND name <> uuid"
            ),
            2,
            "both seats name commander rows"
        );
    }
}
