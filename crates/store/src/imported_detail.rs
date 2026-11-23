//! The per-game detail of a match imported from 0.x (Plan O, issue #177;
//! target schema section 6): each game's deck list and the opponent's cards
//! seen (0.x `game_player.deck_json`), the recorded hand (`opening_hand_json`)
//! and starting hand sizes, the card stats (`card_game_stats`), the game
//! state frames (0.x `game_timelines`) and the match's decoded game log
//! (0.x `game_logs.decoded_entries`).
//!
//! An imported match has no log events in v1, so this is all the match page
//! has. It is stored as the games' rows: `game_cards`, `game_card_casts`,
//! `game_mulligan_cards` and the hand sizes (built by
//! `game_sync::build_game_cards` from the frames, the lists and the log, then
//! 0.x's own hand and card stats laid over them), `game_timelines` (the
//! frames) and `game_logs` (the log).
//!
//! Only a 0.x import's own row gets them: never a live, manual or history
//! import row. Live data wins, so they go when a live match takes the token
//! or the MTGO match ID over (`write::upsert_match` and
//! `write::hand_over_records`).

use std::collections::BTreeMap;

use decks::DeckCard;
use game_detail::{
    GameCardCast, GameCardRow, GameCards, GameLogCards, GameLogEntry, Side, apply_imported_rule,
    extract_log_cards,
};
use rusqlite::{OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::game_sync::{
    FrameText, GameSource, build_game_cards, clear_match_detail, game_log, read_game_cards,
    store_game_log, store_timeline, timeline_frames, write_game_cards,
};
use crate::{Connection, StoreError};

/// One `deck_json` row: a printing, its copies, and whether they are in the
/// sideboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailCard {
    pub mtgo_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
}

/// The local seat's recorded hand (0.x `opening_hand_json`): catalog ids,
/// one per copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedHand {
    /// 0.x `kept`: the starting hand.
    pub starting: Vec<u32>,
    pub bottomed: Vec<u32>,
    /// Each hand sent back, empty when not recorded.
    pub mulligans: Vec<Vec<u32>>,
}

/// One 0.x `card_game_stats` row, before its oracle id is mapped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedCardStat {
    pub oracle_id: String,
    /// The opponent's card (0.x `opponent`).
    pub opponent: bool,
    /// 0.x `quantity`: the maindeck copies.
    pub quantity: u32,
    /// 0.x `kept`: copies in the starting hand.
    pub kept: u32,
    pub seen: u32,
    pub sided_in: bool,
    pub sided_out: bool,
    pub cast: u32,
    pub played: Option<u32>,
    pub activated: Option<u32>,
    pub discarded: u32,
    pub cast_turn: Option<u32>,
    pub hand_seen: u32,
    pub graveyard_seen: u32,
    pub exile_seen: u32,
    pub battlefield_seen: u32,
    pub has_zone_data: bool,
    pub pregame_revealed: bool,
    pub pregame_played: bool,
    /// The alternative casts with a count above 0: `(0.x column, count)`.
    pub methods: Vec<(String, u32)>,
}

/// One game of an imported match, numbered as the import numbers its games.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedGameDetail {
    /// The MTGO Game ID, as `games.mtgo_game_id` holds it.
    pub mtgo_game_id: Option<u64>,
    pub game_number: u32,
    /// The local seat's deck list for this game, main and sideboard.
    pub local_deck: Vec<DetailCard>,
    /// The opponent seat's cards seen.
    pub opponent_cards: Vec<DetailCard>,
    /// The game state frames as JSON text (`{"Cards":[...],"Players":[...]}`)
    /// with their time text (0.x `game_timelines.timestamp`, normalised by
    /// `replay::normalise_time`), in game order, consecutive repeats dropped
    /// by `replay::drop_repeats`.
    pub frames: Vec<FrameText>,
    /// The local seat's recorded hand.
    pub local_hand: Option<ImportedHand>,
    /// `game_player.starting_hand_size` of each seat.
    pub local_starting_hand_size: Option<u32>,
    pub opp_starting_hand_size: Option<u32>,
    /// The game's `card_game_stats` rows.
    pub card_stats: Vec<ImportedCardStat>,
}

impl ImportedGameDetail {
    /// Whether the game has nothing to show.
    pub fn is_empty(&self) -> bool {
        self.local_deck.is_empty()
            && self.opponent_cards.is_empty()
            && self.frames.is_empty()
            && self.local_hand.is_none()
            && self.local_starting_hand_size.is_none()
            && self.opp_starting_hand_size.is_none()
            && self.card_stats.is_empty()
    }
}

/// One decoded game log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailLogEntry {
    /// UTC unix ms.
    pub at_ms: i64,
    pub message: String,
}

/// Everything 0.x kept about an imported match's games.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedDetail {
    pub games: Vec<ImportedGameDetail>,
    pub log: Vec<DetailLogEntry>,
}

impl ImportedDetail {
    /// Whether no game has anything to show and there is no log.
    pub fn is_empty(&self) -> bool {
        self.log.is_empty() && self.games.iter().all(ImportedGameDetail::is_empty)
    }
}

/// What storing one match's detail did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailChange {
    Added,
    Updated,
    /// The stored detail is the same: nothing was written.
    Unchanged,
    /// 0.x has nothing for the match any more: the stored detail was deleted.
    Removed,
    /// Nothing stored: the token has no 0.x imported row (a live, manual or
    /// history import row, or none), or nothing to store and nothing stored.
    Skipped,
}

/// What storing one match's detail did, and how many `card_game_stats`
/// rows had an oracle id no stored card has (left out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetailImport {
    pub change: DetailChange,
    pub card_stats_skipped: u32,
}

/// How many matches each `DetailChange` happened to, and the card stats
/// rows left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DetailImportCounts {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub skipped: u32,
    pub card_stats_skipped: u32,
}

impl DetailImportCounts {
    pub fn add(&mut self, change: DetailChange) {
        match change {
            DetailChange::Added => self.added += 1,
            DetailChange::Updated => self.updated += 1,
            DetailChange::Unchanged => self.unchanged += 1,
            DetailChange::Removed => self.removed += 1,
            DetailChange::Skipped => self.skipped += 1,
        }
    }

    pub fn add_import(&mut self, import: DetailImport) {
        self.add(import.change);
        self.card_stats_skipped += import.card_stats_skipped;
    }
}

/// Store one match's detail in its own transaction, writing only what
/// changed. Only a 0.x imported row gets one; any other token is left
/// alone.
pub fn import_detail(
    conn: &mut Connection,
    token: &str,
    detail: &ImportedDetail,
) -> Result<DetailImport, StoreError> {
    let tx = conn.transaction()?;
    let import = store_detail(&tx, token, detail)?;
    tx.commit()?;
    Ok(import)
}

/// The imported match a detail belongs to.
struct ImportedRow {
    id: i64,
    local: String,
    opponent: Option<String>,
    /// 0.x's `imported` flag (`matches.legacy_history`, issue #313).
    legacy_history: bool,
}

fn imported_row(conn: &Connection, token: &str) -> Result<Option<ImportedRow>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT m.id, a.username, o.username, m.legacy_history FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.token = ?1 AND m.source = 'imported'",
        )?
        .query_row([token], |row| {
            Ok(ImportedRow {
                id: row.get(0)?,
                local: row.get(1)?,
                opponent: row.get(2)?,
                legacy_history: row.get(3)?,
            })
        })
        .optional()?)
}

/// Whether the match's games or log hold any detail.
fn has_detail(conn: &Connection, match_id: i64) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM game_logs WHERE match_id = ?1)
                 OR EXISTS (SELECT 1 FROM games g WHERE g.match_id = ?1 AND (
                     g.local_opening_hand_size IS NOT NULL
                     OR g.local_starting_hand_size IS NOT NULL
                     OR g.opp_opening_hand_size IS NOT NULL
                     OR g.opp_starting_hand_size IS NOT NULL
                     OR EXISTS (SELECT 1 FROM game_cards WHERE game_id = g.id)
                     OR EXISTS (SELECT 1 FROM game_mulligan_cards WHERE game_id = g.id)
                     OR EXISTS (SELECT 1 FROM game_timelines WHERE game_id = g.id)))",
        )?
        .query_row([match_id], |row| row.get(0))?)
}

/// A stored game: `(id, number, Game ID, on the play)`.
type StoredGame = (i64, u32, Option<u64>, Option<bool>);

fn stored_games(conn: &Connection, match_id: i64) -> Result<Vec<StoredGame>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, game_number, mtgo_game_id, on_play FROM games
         WHERE match_id = ?1 ORDER BY game_number",
    )?;
    let rows = stmt.query_map([match_id], |row| {
        let mtgo_game_id: Option<i64> = row.get(2)?;
        Ok((
            row.get(0)?,
            row.get(1)?,
            mtgo_game_id.and_then(|id| u64::try_from(id).ok()),
            row.get(3)?,
        ))
    })?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row?);
    }
    Ok(games)
}

/// The 0.x detail of a stored game: the one with its Game ID, else, when
/// either side has no Game ID, the one with its number (numbers can have
/// gaps, such as 1 and 3).
fn detail_of(
    games: &[ImportedGameDetail],
    number: u32,
    mtgo_game_id: Option<u64>,
) -> Option<&ImportedGameDetail> {
    if let Some(id) = mtgo_game_id
        && let Some(found) = games.iter().find(|game| game.mtgo_game_id == Some(id))
    {
        return Some(found);
    }
    games.iter().find(|game| {
        game.game_number == number && (mtgo_game_id.is_none() || game.mtgo_game_id.is_none())
    })
}

/// A 0.x deck list as deck rows.
fn deck_rows(cards: &[DetailCard]) -> Vec<DeckCard> {
    cards
        .iter()
        .map(|card| DeckCard {
            catalog_id: card.mtgo_id,
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect()
}

/// The opponent's cards 0.x saw, summed per card whatever the sideboard
/// flag, in order of first appearance (0.x `BuildMatchGameData`).
fn recorded_cards_seen(cards: &[DetailCard]) -> Vec<(u32, u32)> {
    let mut counts: Vec<(u32, u32)> = Vec::new();
    for card in cards {
        match counts.iter_mut().find(|(id, _)| *id == card.mtgo_id) {
            Some((_, quantity)) => *quantity += card.quantity,
            None => counts.push((card.mtgo_id, card.quantity)),
        }
    }
    counts
}

pub(crate) fn store_detail(
    tx: &Transaction,
    token: &str,
    detail: &ImportedDetail,
) -> Result<DetailImport, StoreError> {
    let skipped = DetailImport {
        change: DetailChange::Skipped,
        card_stats_skipped: 0,
    };
    // Any other row's games are its own (live, manual or history).
    let Some(row) = imported_row(tx, token)? else {
        return Ok(skipped);
    };
    let had_detail = has_detail(tx, row.id)?;
    if detail.is_empty() {
        if !had_detail {
            return Ok(skipped);
        }
        clear_match_detail(tx, row.id)?;
        return Ok(DetailImport {
            change: DetailChange::Removed,
            card_stats_skipped: 0,
        });
    }

    let stored = stored_games(tx, row.id)?;
    let mut sources = Vec::new();
    let mut found = Vec::new();
    for (game_id, number, mtgo_game_id, on_play) in &stored {
        let game = detail_of(&detail.games, *number, *mtgo_game_id);
        let mut source = GameSource {
            game_id: *game_id,
            on_play: *on_play,
            ..GameSource::default()
        };
        if let Some(game) = game {
            source.frames = timeline_frames(&game.frames);
            source.local_deck = deck_rows(&game.local_deck);
            source.recorded_opponent_cards = recorded_cards_seen(&game.opponent_cards);
        }
        sources.push(source);
        found.push(game);
    }
    // 0.x's decoded log stands in for the `.dat` file, as 0.x read it.
    let log: Vec<GameLogEntry> = detail
        .log
        .iter()
        .map(|entry| GameLogEntry {
            timestamp_ms: entry.at_ms,
            message: entry.message.clone(),
        })
        .collect();
    let built = build_game_cards(
        tx,
        &row.local,
        row.opponent.as_deref(),
        &sources,
        &log,
        &log,
    )?;

    let rule = rule_inputs(tx, row.id, &row.local, &log)?;
    let mut changed = false;
    let mut card_stats_skipped = 0;
    for (index, ((source, game), mut cards)) in sources.iter().zip(&found).zip(built).enumerate() {
        let mut frames: &[FrameText] = &[];
        if let Some(game) = game {
            frames = &game.frames;
            // A history game's deck lists hold only the cards seen (#313).
            if row.legacy_history {
                clear_sideboarding(&mut cards);
            }
            lay_over_hand(&mut cards, game);
            card_stats_skipped += lay_over_card_stats(tx, &mut cards, game)?;
            tidy(&mut cards);
        }
        if row.legacy_history
            && let Some(rule) = &rule
        {
            let number = stored[index].1;
            apply_rule(rule, index, number, &mut cards);
        }
        changed |= store_timeline(tx, source.game_id, frames)?;
        changed |= write_game_cards(tx, source.game_id, &cards)?;
    }
    if log.is_empty() {
        changed |= tx
            .prepare_cached("DELETE FROM game_logs WHERE match_id = ?1")?
            .execute([row.id])?
            > 0;
    } else {
        changed |= store_game_log(tx, row.id, &log)?;
    }
    let change = match (had_detail, changed) {
        (_, false) => DetailChange::Unchanged,
        (false, true) => DetailChange::Added,
        (true, true) => DetailChange::Updated,
    };
    Ok(DetailImport {
        change,
        card_stats_skipped,
    })
}

/// The per-game detail of a pulled Arena or Endstep match (cross-device
/// sync spec section 7): its raw source is kept (`synced_sources`) and its
/// rows are written from it by the writers the local game syncs use, so it
/// gets exactly what a recorded match gets. None of the MTGO steps run
/// (frames, hand and card stats overlays, timelines, the stored game log):
/// the source is the detail. Only this device's imported copy gets them;
/// any other token is left alone. Returns whether a game row changed.
pub(crate) fn store_sources(
    tx: &Transaction,
    token: &str,
    sources: &sync_bundles::MatchSources,
    now_ms: i64,
) -> Result<bool, StoreError> {
    let Some(row) = imported_row(tx, token)? else {
        return Ok(false);
    };
    crate::synced_sources::write(tx, row.id, sources)?;
    match sources {
        sync_bundles::MatchSources::Arena(arena) => {
            crate::arena_game_sync::derive_imported(tx, row.id, arena, now_ms)
        }
        sync_bundles::MatchSources::Endstep(endstep) => {
            crate::endstep_sync::write_imported_games(tx, row.id, endstep, now_ms)
        }
    }
}

/// Apply the imported postboard rule to a stored imported or history match
/// (`game_detail::apply_imported_rule`), for when its deck version is linked
/// after its rows were written, or to rebuild it. Returns whether any row
/// changed.
pub fn reapply_imported_rule(tx: &Transaction, match_id: i64) -> Result<bool, StoreError> {
    let local: Option<String> = tx
        .prepare_cached(
            "SELECT a.username FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    let Some(local) = local else {
        return Ok(false);
    };
    let log = game_log(tx, match_id)?.unwrap_or_default();
    let Some(inputs) = rule_inputs(tx, match_id, &local, &log)? else {
        return Ok(false);
    };
    // Only a history match: a v1 history import, or a 0.x copy with 0.x's
    // `imported` flag (issue #313).
    let history: bool = tx
        .prepare_cached(
            "SELECT source = 'history' OR legacy_history = 1 FROM matches WHERE id = ?1",
        )?
        .query_row([match_id], |row| row.get(0))?;
    if !history {
        return Ok(false);
    }
    let mut games: Vec<(i64, u32)> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT id, game_number FROM games g WHERE match_id = ?1 ORDER BY game_number",
        )?;
        let rows = stmt.query_map([match_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            games.push(row?);
        }
    }
    let mut changed = false;
    for (index, (game_id, number)) in games.into_iter().enumerate() {
        let mut cards = read_game_cards(tx, game_id)?;
        if apply_rule(&inputs, index, number, &mut cards) {
            changed |= write_game_cards(tx, game_id, &cards)?;
        }
    }
    Ok(changed)
}

/// What the imported postboard rule reads for a match: its registered deck
/// and the local player's cards in its game log.
pub(crate) struct RuleInputs {
    registered: Vec<DeckCard>,
    log: GameLogCards,
    local: String,
    /// The oracle ids of the registered cards and the local log cards, so
    /// another printing of a registered card is matched (S7-2).
    oracles: BTreeMap<u32, String>,
}

/// The rule's inputs for an imported or history match (cards spec ruling
/// 7): `None` when it has no deck version (or one nothing can read) or no
/// game log, so its rows stay as imported.
pub(crate) fn rule_inputs(
    conn: &Connection,
    match_id: i64,
    local: &str,
    log: &[GameLogEntry],
) -> Result<Option<RuleInputs>, StoreError> {
    if log.is_empty() {
        return Ok(None);
    }
    let signature: Option<String> = conn
        .prepare_cached(
            "SELECT v.signature FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE m.id = ?1 AND m.source IN ('imported', 'history')",
        )?
        .query_row([match_id], |row| row.get(0))
        .optional()?;
    let registered = signature
        .map(|signature| decks::decode_signature(&signature))
        .unwrap_or_default();
    if registered.is_empty() {
        return Ok(None);
    }
    let log = extract_log_cards(log);
    let mut oracles = BTreeMap::new();
    let ids = registered
        .iter()
        .map(|card| card.catalog_id)
        .chain(log.match_cards(local).iter().map(|card| card.mtgo_id));
    for catalog_id in ids {
        if oracles.contains_key(&catalog_id) {
            continue;
        }
        if let Some(oracle) = oracle_of_catalog(conn, catalog_id)? {
            oracles.insert(catalog_id, oracle);
        }
    }
    Ok(Some(RuleInputs {
        registered,
        log,
        local: local.to_string(),
        oracles,
    }))
}

/// Apply the rule to one game's rows (the game at `index` in game order,
/// numbered `number`) of a history match. A game the log has no entry for
/// keeps its rows (0.x "preserves stats for a game the log has no entry
/// for"). Call it only for a history match (a v1 history import, or a 0.x
/// copy with 0.x's `imported` flag): a tracked game's deck list is the deck
/// MTGO logged as played, not the cards seen (0.x applied the rule's premise
/// only to its own history imports, issue #313). Returns whether the rule
/// applied.
pub(crate) fn apply_rule(
    inputs: &RuleInputs,
    index: usize,
    number: u32,
    cards: &mut GameCards,
) -> bool {
    if inputs.log.games.get(index).is_none() {
        return false;
    }
    let log_cards = inputs.log.game_cards(index, &inputs.local);
    let oracle_of = |catalog_id: u32| inputs.oracles.get(&catalog_id).cloned();
    apply_imported_rule(number, &inputs.registered, log_cards, &oracle_of, cards);
    true
}

/// The row of one side's card, made when missing.
fn row_mut(cards: &mut GameCards, side: Side, catalog_id: u32) -> &mut GameCardRow {
    let index = match cards
        .cards
        .iter()
        .position(|row| row.side == side && row.catalog_id == catalog_id)
    {
        Some(index) => index,
        None => {
            cards.cards.push(GameCardRow {
                side,
                catalog_id,
                ..GameCardRow::default()
            });
            cards.cards.len() - 1
        }
    };
    &mut cards.cards[index]
}

/// No copies brought in or taken out (issues #294 and #313). A game of a
/// match with 0.x's `imported` flag is 0.x's own import from MTGO's match
/// history: its `deck_json` holds only the cards seen, so comparing it with
/// game 1 fakes sideboarding, which 0.x never detected for such matches
/// (`$trackSideboarding = false`). The
/// imported postboard rule, applied after, sets them from the log when the
/// match has a deck version; 0.x's own card stats are laid over after too.
fn clear_sideboarding(cards: &mut GameCards) {
    for row in &mut cards.cards {
        row.sided_in = 0;
        row.sided_out = 0;
    }
}

/// 0.x's recorded hand replaces the one read from the frames; its starting
/// hand sizes fill sizes the frames and the log left unknown (opening =
/// starting + bottomed).
fn lay_over_hand(cards: &mut GameCards, game: &ImportedGameDetail) {
    if let Some(hand) = &game.local_hand {
        for row in cards.cards.iter_mut().filter(|row| row.side == Side::Local) {
            row.opening_hand = 0;
            row.bottomed = 0;
            row.starting_hand = 0;
            row.first_draw = 0;
        }
        for catalog_id in &hand.starting {
            let row = row_mut(cards, Side::Local, *catalog_id);
            row.opening_hand += 1;
            row.starting_hand += 1;
        }
        for catalog_id in &hand.bottomed {
            let row = row_mut(cards, Side::Local, *catalog_id);
            row.opening_hand += 1;
            row.bottomed += 1;
        }
        cards.starting_hand_size = Some(hand.starting.len() as u32);
        cards.opening_hand_size = Some((hand.starting.len() + hand.bottomed.len()) as u32);
        cards.mulligans.clear();
        for (index, sent_back) in hand.mulligans.iter().enumerate() {
            let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
            for catalog_id in sent_back {
                *counts.entry(*catalog_id).or_insert(0) += 1;
            }
            for (catalog_id, quantity) in counts {
                cards
                    .mulligans
                    .push((index as u32 + 1, catalog_id, quantity));
            }
        }
    }
    if cards.starting_hand_size.is_none()
        && let Some(size) = game.local_starting_hand_size
    {
        let bottomed: u32 = cards
            .cards
            .iter()
            .filter(|row| row.side == Side::Local)
            .map(|row| row.bottomed)
            .sum();
        cards.starting_hand_size = Some(size);
        cards.opening_hand_size = Some(size + bottomed);
    }
    if cards.opp_starting_hand_size.is_none()
        && let Some(size) = game.opp_starting_hand_size
    {
        cards.opp_starting_hand_size = Some(size);
        cards.opp_opening_hand_size = Some(size);
    }
}

/// The lowest catalog id of an oracle in the card table, as deck signatures
/// map them.
fn catalog_of_oracle(conn: &Connection, oracle_id: &str) -> Result<Option<u32>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT MIN(catalog_id) FROM cards WHERE oracle_id = ?1")?
        .query_row([oracle_id], |row| row.get(0))?)
}

/// A printing's oracle id, `None` without card data.
fn oracle_of_catalog(conn: &Connection, catalog_id: u32) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT oracle_id FROM cards WHERE catalog_id = ?1")?
        .query_row([catalog_id], |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

/// The printing a stats row belongs to: the game's own row of that side
/// whose card has the oracle (the printing the lists and frames saw played),
/// else the oracle's lowest catalog id. `None` when no stored card has the
/// oracle.
fn catalog_for_stat(
    conn: &Connection,
    cards: &GameCards,
    side: Side,
    oracle_id: &str,
) -> Result<Option<u32>, StoreError> {
    for row in cards.cards.iter().filter(|row| row.side == side) {
        if oracle_of_catalog(conn, row.catalog_id)?.as_deref() == Some(oracle_id) {
            return Ok(Some(row.catalog_id));
        }
    }
    catalog_of_oracle(conn, oracle_id)
}

/// 0.x's card stats over the rows built from the lists, frames and log:
/// its counters, alternative casts, pregame flags and zone counts win (the
/// frames' zone counts are dropped, so a row 0.x had none for has none); the
/// local `seen` is 0.x's, the opponent's the larger; `quantity` fills the
/// maindeck and `kept` the hand only when the game's lists and hand are
/// unknown; a sideboarding flag counts one copy when the lists give none.
/// Each row lands on the printing the game's lists or frames show for its
/// oracle (`catalog_for_stat`). Returns how many rows were left out (no
/// oracle id, or no stored card has it).
fn lay_over_card_stats(
    conn: &Connection,
    cards: &mut GameCards,
    game: &ImportedGameDetail,
) -> Result<u32, StoreError> {
    let has_deck = !game.local_deck.is_empty();
    let has_hand = cards
        .cards
        .iter()
        .any(|row| row.side == Side::Local && row.opening_hand > 0);
    // 0.x's zone counters stand for the game (the cards spec section 8):
    // the local ones the frames give are dropped when 0.x has local card
    // stats (stats for the opponent's cards only say nothing about them).
    if game.card_stats.iter().any(|stat| !stat.opponent) {
        for row in cards.cards.iter_mut().filter(|row| row.side == Side::Local) {
            row.discarded = 0;
            row.hand_seen = 0;
            row.graveyard_seen = 0;
            row.exile_seen = 0;
            row.battlefield_seen = 0;
            row.has_zone_data = false;
        }
    }
    let mut skipped = 0;
    for stat in &game.card_stats {
        let side = if stat.opponent {
            Side::Opponent
        } else {
            Side::Local
        };
        let oracle_id = stat.oracle_id.trim();
        let found = if oracle_id.is_empty() {
            None
        } else {
            catalog_for_stat(conn, cards, side, oracle_id)?
        };
        let Some(catalog_id) = found else {
            skipped += 1;
            continue;
        };
        let row = row_mut(cards, side, catalog_id);
        if side == Side::Local {
            if !has_deck {
                row.main_qty = stat.quantity;
            }
            if !has_hand {
                row.opening_hand = stat.kept;
                row.starting_hand = stat.kept;
                row.bottomed = 0;
            }
            row.seen = stat.seen;
            if row.sided_in == 0 && stat.sided_in {
                row.sided_in = 1;
            }
            if row.sided_out == 0 && stat.sided_out {
                row.sided_out = 1;
            }
        } else {
            row.seen = row.seen.max(stat.seen);
        }
        row.cast = stat.cast;
        row.played = stat.played;
        row.activated = stat.activated;
        row.discarded = stat.discarded;
        row.cast_turn = stat.cast_turn;
        row.hand_seen = stat.hand_seen;
        row.graveyard_seen = stat.graveyard_seen;
        row.exile_seen = stat.exile_seen;
        row.battlefield_seen = stat.battlefield_seen;
        row.has_zone_data = stat.has_zone_data;
        row.pregame_revealed = row.pregame_revealed.max(u32::from(stat.pregame_revealed));
        row.pregame_played = row.pregame_played.max(u32::from(stat.pregame_played));
        cards
            .casts
            .retain(|cast| !(cast.side == side && cast.catalog_id == catalog_id));
        if side == Side::Local {
            clear_other_printings(conn, cards, catalog_id, oracle_id)?;
        }
        for (method, count) in &stat.methods {
            if *count > 0 {
                cards.casts.push(GameCardCast {
                    side,
                    catalog_id,
                    method: method.clone(),
                    count: *count,
                });
            }
        }
    }
    Ok(skipped)
}

/// 0.x's local row for an oracle counts every printing the player touched
/// (a warp cast logged under a variant's catalog id), so once it lands on
/// `catalog_id` the other local printings of the oracle lose their counters,
/// zone counts, cast turn, pregame flags and casts (review N1): the imported
/// rule's fold then adds nothing on top, and `tidy` drops a row left blank.
/// Deck and hand columns stay (they are per printing).
fn clear_other_printings(
    conn: &Connection,
    cards: &mut GameCards,
    catalog_id: u32,
    oracle_id: &str,
) -> Result<(), StoreError> {
    let mut others = Vec::new();
    for row in cards.cards.iter().filter(|row| row.side == Side::Local) {
        if row.catalog_id != catalog_id
            && oracle_of_catalog(conn, row.catalog_id)?.as_deref() == Some(oracle_id)
        {
            others.push(row.catalog_id);
        }
    }
    for other in others {
        let row = row_mut(cards, Side::Local, other);
        row.seen = 0;
        row.cast = 0;
        row.played = row.played.map(|_| 0);
        row.activated = row.activated.map(|_| 0);
        row.cast_turn = None;
        row.discarded = 0;
        row.hand_seen = 0;
        row.graveyard_seen = 0;
        row.exile_seen = 0;
        row.battlefield_seen = 0;
        row.has_zone_data = false;
        row.pregame_revealed = 0;
        row.pregame_played = 0;
        cards
            .casts
            .retain(|cast| !(cast.side == Side::Local && cast.catalog_id == other));
    }
    Ok(())
}

/// Rows in the order `read_game_cards` gives them, without rows that say
/// nothing (a hand card the recorded hand replaced).
fn tidy(cards: &mut GameCards) {
    let casts = cards.casts.clone();
    cards.cards.retain(|row| {
        let has_cast = casts
            .iter()
            .any(|cast| cast.side == row.side && cast.catalog_id == row.catalog_id);
        has_cast || !is_blank(row)
    });
    cards.cards.sort_by_key(|row| (row.side, row.catalog_id));
    cards
        .casts
        .sort_by(|a, b| (a.side, a.catalog_id, &a.method).cmp(&(b.side, b.catalog_id, &b.method)));
    cards.mulligans.sort_unstable();
}

fn is_blank(row: &GameCardRow) -> bool {
    let counts = [
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
        row.played.unwrap_or(0),
        row.activated.unwrap_or(0),
        row.discarded,
        row.hand_seen,
        row.graveyard_seen,
        row.exile_seen,
        row.battlefield_seen,
    ];
    counts.iter().all(|count| *count == 0) && row.cast_turn.is_none() && !row.has_zone_data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_sync::{game_log, read_game_cards, timeline};
    use crate::import::{ImportedMatch, import_matches};
    use crate::test_support::*;
    use crate::write::{MatchWrite, WriteBatch, commit};
    use fixture_tools::TestDir;
    use rusqlite::params;

    fn card(mtgo_id: u32, quantity: u32, sideboard: bool) -> DetailCard {
        DetailCard {
            mtgo_id,
            quantity,
            sideboard,
        }
    }

    /// Two games: game 1 (Game ID 7) with a deck, reveals and two frames,
    /// game 2 with nothing; and a two-line log.
    fn detail() -> ImportedDetail {
        ImportedDetail {
            games: vec![
                ImportedGameDetail {
                    mtgo_game_id: Some(7),
                    game_number: 1,
                    local_deck: vec![card(4001, 4, false), card(4003, 3, true)],
                    opponent_cards: vec![card(5001, 1, false)],
                    frames: vec![
                        FrameText {
                            time: "10:00:01".to_string(),
                            text: r#"{"Cards":[],"Players":[]}"#.to_string(),
                        },
                        FrameText {
                            time: "10:00:02.500".to_string(),
                            text: r#"{"Cards":[{"Id":1}],"Players":[]}"#.to_string(),
                        },
                    ],
                    ..ImportedGameDetail::default()
                },
                ImportedGameDetail {
                    mtgo_game_id: None,
                    game_number: 2,
                    ..ImportedGameDetail::default()
                },
            ],
            log: vec![
                DetailLogEntry {
                    at_ms: 1_000,
                    message: "Qlocal rolled a 6.".to_string(),
                },
                DetailLogEntry {
                    at_ms: 2_000,
                    message: "Qlocal plays first.".to_string(),
                },
            ],
        }
    }

    /// A match 0.x imported from MTGO's match history (0.x `imported`): its
    /// deck lists hold only the cards seen.
    fn import_row(conn: &mut Connection, token: &str, mtgo_match_id: Option<i64>) {
        let history = ImportedMatch {
            legacy_history: true,
            ..imported(token, mtgo_match_id)
        };
        import_matches(conn, &[history], 5_000).expect("import");
    }

    fn change(conn: &mut Connection, token: &str, detail: &ImportedDetail) -> DetailChange {
        import_detail(conn, token, detail).expect("store").change
    }

    fn game_id(conn: &Connection, token: &str, number: u32) -> i64 {
        conn.query_row(
            "SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
             WHERE m.token = ?1 AND g.game_number = ?2",
            params![token, number],
            |row| row.get(0),
        )
        .expect("game")
    }

    fn detail_rows(conn: &Connection) -> i64 {
        number(
            conn,
            "SELECT (SELECT COUNT(*) FROM game_cards) + (SELECT COUNT(*) FROM game_timelines)
                 + (SELECT COUNT(*) FROM game_logs) + (SELECT COUNT(*) FROM game_mulligan_cards)",
        )
    }

    #[test]
    fn the_detail_of_an_imported_match_becomes_game_rows() {
        let (_dir, mut conn) = db("detail-store");
        import_row(&mut conn, TOKEN, Some(42));
        assert_eq!(change(&mut conn, TOKEN, &detail()), DetailChange::Added);
        let first = game_id(&conn, TOKEN, 1);
        let cards = read_game_cards(&conn, first).expect("cards");
        let local = cards.row(Side::Local, 4001).expect("main");
        assert_eq!((local.main_qty, local.side_qty), (4, 0));
        assert_eq!(cards.row(Side::Local, 4003).expect("side").side_qty, 3);
        assert_eq!(cards.row(Side::Opponent, 5001).expect("seen").seen, 1);
        assert_eq!(
            timeline(&conn, first).expect("frames"),
            Some(detail().games[0].frames.clone())
        );
        assert_eq!(
            timeline(&conn, game_id(&conn, TOKEN, 2)).expect("none"),
            None
        );
        let log = game_log(&conn, match_id(&conn, TOKEN))
            .expect("log")
            .expect("a log");
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].timestamp_ms, 2_000);
    }

    #[test]
    fn a_rerun_with_the_same_detail_writes_nothing_and_a_change_rewrites_it() {
        let (_dir, mut conn) = db("detail-rerun");
        import_row(&mut conn, TOKEN, Some(42));
        change(&mut conn, TOKEN, &detail());
        let before = dump(&conn, "game_cards");
        assert_eq!(change(&mut conn, TOKEN, &detail()), DetailChange::Unchanged);
        assert_eq!(dump(&conn, "game_cards"), before);
        let mut changed = detail();
        changed.games[0].local_deck = vec![card(4001, 3, false)];
        assert_eq!(change(&mut conn, TOKEN, &changed), DetailChange::Updated);
        let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 1)).expect("cards");
        assert_eq!(cards.row(Side::Local, 4001).expect("main").main_qty, 3);
        assert!(cards.row(Side::Local, 4003).is_none(), "gone from the list");
    }

    #[test]
    fn an_empty_detail_is_not_stored_and_removes_a_stored_one() {
        let (_dir, mut conn) = db("detail-empty");
        import_row(&mut conn, TOKEN, Some(42));
        let empty = ImportedDetail::default();
        assert_eq!(change(&mut conn, TOKEN, &empty), DetailChange::Skipped);
        change(&mut conn, TOKEN, &detail());
        assert_eq!(change(&mut conn, TOKEN, &empty), DetailChange::Removed);
        assert_eq!(detail_rows(&conn), 0);
    }

    fn live_upsert(conn: &mut Connection, token: &str, mtgo_match_id: Option<u64>) {
        let mut live = view(token, "Qopponent", 20_000);
        live.mtgo_match_id = mtgo_match_id;
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(live),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("live");
    }

    #[test]
    fn only_a_0x_imported_row_gets_a_detail() {
        let (_dir, mut conn) = db("detail-only-imported");
        assert_eq!(change(&mut conn, TOKEN, &detail()), DetailChange::Skipped);
        live_upsert(&mut conn, TOKEN, Some(42));
        assert_eq!(change(&mut conn, TOKEN, &detail()), DetailChange::Skipped);
        insert_manual(&conn, OTHER);
        assert_eq!(change(&mut conn, OTHER, &detail()), DetailChange::Skipped);
        assert_eq!(detail_rows(&conn), 0);
    }

    #[test]
    fn a_live_takeover_by_token_deletes_the_detail() {
        let (_dir, mut conn) = db("detail-takeover-token");
        import_row(&mut conn, TOKEN, Some(42));
        change(&mut conn, TOKEN, &detail());
        live_upsert(&mut conn, TOKEN, Some(42));
        assert_eq!(detail_rows(&conn), 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM games WHERE local_opening_hand_size IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn a_live_takeover_by_match_id_deletes_the_detail_and_never_moves_it() {
        let (_dir, mut conn) = db("detail-takeover-id");
        import_row(&mut conn, OTHER, Some(42));
        change(&mut conn, OTHER, &detail());
        live_upsert(&mut conn, TOKEN, Some(42));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(detail_rows(&conn), 0);
    }

    #[test]
    fn a_row_that_becomes_a_manual_match_loses_its_detail() {
        let (_dir, mut conn) = db("detail-manual");
        import_row(&mut conn, TOKEN, Some(42));
        change(&mut conn, TOKEN, &detail());
        let mut manual = imported(TOKEN, None);
        manual.manual = Some(Vec::new());
        import_matches(&mut conn, &[manual], 6_000).expect("manual");
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM matches WHERE token = '{TOKEN}' AND source = 'manual'"
                )
            ),
            1
        );
        assert_eq!(detail_rows(&conn), 0);
    }

    #[test]
    fn deleting_history_imports_never_touches_a_0x_detail() {
        let (_dir, mut conn) = db("detail-history");
        import_row(&mut conn, TOKEN, Some(42));
        change(&mut conn, TOKEN, &detail());
        let before = detail_rows(&conn);
        crate::history_import::delete_history_imports(&mut conn).expect("delete");
        assert_eq!(detail_rows(&conn), before);
    }

    fn stat(oracle_id: &str, opponent: bool) -> ImportedCardStat {
        ImportedCardStat {
            oracle_id: oracle_id.to_string(),
            opponent,
            ..ImportedCardStat::default()
        }
    }

    #[test]
    fn card_game_stats_import_maps_oracle_ids_and_counters() {
        let (_dir, mut conn) = db("detail-card-stats");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
                 (4002, 'q-oracle-a', 'Qa'), (4001, 'q-oracle-a', 'Qa'), (5001, 'q-oracle-b', 'Qb');",
        )
        .expect("cards");
        import_row(&mut conn, TOKEN, Some(42));
        let local = ImportedCardStat {
            quantity: 4,
            kept: 2,
            seen: 3,
            cast: 2,
            played: Some(0),
            activated: Some(1),
            cast_turn: Some(3),
            sided_in: true,
            pregame_revealed: true,
            has_zone_data: true,
            hand_seen: 2,
            methods: vec![("kicked".to_string(), 1)],
            ..stat("q-oracle-a", false)
        };
        let opponent = ImportedCardStat {
            seen: 2,
            cast: 1,
            ..stat("q-oracle-b", true)
        };
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                card_stats: vec![local, opponent, stat("q-unknown", false)],
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        };
        let import = import_detail(&mut conn, TOKEN, &detail).expect("store");
        assert_eq!(import.change, DetailChange::Added);
        assert_eq!(import.card_stats_skipped, 1);
        let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 1)).expect("cards");
        let mine = cards.row(Side::Local, 4001).expect("the lowest catalog id");
        assert_eq!(
            (
                mine.main_qty,
                mine.starting_hand,
                mine.opening_hand,
                mine.seen
            ),
            (4, 2, 2, 3)
        );
        assert_eq!(
            (mine.cast, mine.played, mine.activated, mine.cast_turn),
            (2, Some(0), Some(1), Some(3))
        );
        assert_eq!(
            (mine.sided_in, mine.pregame_revealed, mine.hand_seen),
            (1, 1, 2)
        );
        assert!(mine.has_zone_data);
        let theirs = cards.row(Side::Opponent, 5001).expect("opponent");
        assert_eq!((theirs.seen, theirs.cast), (2, 1));
        assert_eq!(
            cards.casts,
            vec![GameCardCast {
                side: Side::Local,
                catalog_id: 4001,
                method: "kicked".to_string(),
                count: 1,
            }]
        );
        let again = import_detail(&mut conn, TOKEN, &detail).expect("again");
        assert_eq!(again.change, DetailChange::Unchanged);
    }

    #[test]
    fn card_stats_land_on_the_printing_the_game_played() {
        let (_dir, mut conn) = db("detail-card-stats-printing");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
                 (4001, 'q-oracle-a', 'Qa'), (4002, 'q-oracle-a', 'Qa'),
                 (5001, 'q-oracle-b', 'Qb'), (5002, 'q-oracle-b', 'Qb');",
        )
        .expect("cards");
        import_row(&mut conn, TOKEN, Some(42));
        // deck_json names the newer printings (4002, 5002); 0.x's stats rows
        // are by oracle, whose lowest printings are 4001 and 5001.
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                local_deck: vec![card(4002, 4, false)],
                opponent_cards: vec![card(5002, 2, false)],
                card_stats: vec![
                    ImportedCardStat {
                        quantity: 4,
                        seen: 3,
                        cast: 1,
                        ..stat("q-oracle-a", false)
                    },
                    ImportedCardStat {
                        seen: 2,
                        cast: 1,
                        ..stat("q-oracle-b", true)
                    },
                ],
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        };
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 1)).expect("cards");
        let ids: Vec<(Side, u32)> = cards
            .cards
            .iter()
            .map(|row| (row.side, row.catalog_id))
            .collect();
        assert_eq!(
            ids,
            vec![(Side::Local, 4002), (Side::Opponent, 5002)],
            "one row per card"
        );
        let mine = cards.row(Side::Local, 4002).expect("local");
        assert_eq!((mine.main_qty, mine.seen, mine.cast), (4, 3, 1));
        let theirs = cards.row(Side::Opponent, 5002).expect("opponent");
        assert_eq!((theirs.seen, theirs.cast), (2, 1), "reveals not doubled");
    }

    #[test]
    fn an_imported_game_keeps_0x_zone_counters() {
        let (_dir, mut conn) = db("detail-zones");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES (4001, 'q-oracle-a', 'Qa');",
        )
        .expect("cards");
        import_row(&mut conn, TOKEN, Some(42));
        // The frames show two copies of 4001 in hand and a 4005 in exile; 0.x
        // counted no zones for this game (it had no timeline when it ran).
        let frame = r#"{"Players":[{"Id":1,"Name":"Qlocal"},{"Id":2,"Name":"Qopponent"}],"Cards":[{"Id":10,"CatalogID":4001,"Zone":"Hand","Owner":1},{"Id":11,"CatalogID":4001,"Zone":"Hand","Owner":1},{"Id":12,"CatalogID":4005,"Zone":"Exile","Owner":1}]}"#;
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                local_deck: vec![card(4001, 4, false), card(4005, 1, false)],
                frames: vec![FrameText {
                    time: "10:00:01".to_string(),
                    text: frame.to_string(),
                }],
                card_stats: vec![ImportedCardStat {
                    quantity: 4,
                    seen: 2,
                    ..stat("q-oracle-a", false)
                }],
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        };
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 1)).expect("cards");
        let mine = cards.row(Side::Local, 4001).expect("local");
        assert_eq!((mine.hand_seen, mine.has_zone_data), (0, false));
        // S6-4: 4005 has no 0.x stats row: the frames' exile count is
        // cleared with the rest, not kept beside 0.x's numbers.
        let lacked = cards.row(Side::Local, 4005).expect("in the deck");
        assert_eq!(
            (lacked.main_qty, lacked.exile_seen, lacked.has_zone_data),
            (1, 0, false),
            "no zone count 0.x lacked"
        );
    }

    // S6-3: 0.x stats for the opponent's cards only say nothing about the
    // local zones, so the frames' local counts stay.
    #[test]
    fn opponent_only_0x_stats_keep_the_local_zone_counters() {
        let (_dir, mut conn) = db("detail-zones-opponent-only");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES (5001, 'q-oracle-b', 'Qb');",
        )
        .expect("cards");
        import_row(&mut conn, TOKEN, Some(42));
        let frame = r#"{"Players":[{"Id":1,"Name":"Qlocal"},{"Id":2,"Name":"Qopponent"}],"Cards":[{"Id":10,"CatalogID":4001,"Zone":"Hand","Owner":1},{"Id":11,"CatalogID":4001,"Zone":"Hand","Owner":1}]}"#;
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                local_deck: vec![card(4001, 4, false)],
                frames: vec![FrameText {
                    time: "10:00:01".to_string(),
                    text: frame.to_string(),
                }],
                card_stats: vec![ImportedCardStat {
                    seen: 1,
                    ..stat("q-oracle-b", true)
                }],
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        };
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 1)).expect("cards");
        let mine = cards.row(Side::Local, 4001).expect("local");
        assert_eq!((mine.hand_seen, mine.has_zone_data), (2, true));
    }

    /// Link the match to version 7 of deck 5 with these cards.
    fn link_version(conn: &Connection, token: &str, cards: &[(u32, u32, bool)]) {
        let rows: Vec<DeckCard> = cards
            .iter()
            .map(|&(catalog_id, quantity, sideboard)| DeckCard {
                catalog_id,
                quantity,
                sideboard,
            })
            .collect();
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (5, 'q-deck', (SELECT id FROM accounts WHERE username = 'Qlocal'), 'Qdeck',
                 'CMODERN', 0, 0, 'file')",
            [],
        )
        .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (7, 5, ?1, 0)",
            [decks::signature(&rows)],
        )
        .expect("version");
        conn.execute(
            "UPDATE matches SET deck_version_id = 7 WHERE token = ?1",
            [token],
        )
        .expect("link");
    }

    fn log_entry(at_ms: i64, message: &str) -> DetailLogEntry {
        DetailLogEntry {
            at_ms,
            message: message.to_string(),
        }
    }

    /// Two games: 0.x's seen-only `deck_json` (3001 at 1 copy, then 4), and a
    /// log where game 2 shows the sideboard card 3002 cast.
    fn postboard_detail() -> ImportedDetail {
        ImportedDetail {
            games: vec![
                ImportedGameDetail {
                    mtgo_game_id: Some(7),
                    game_number: 1,
                    local_deck: vec![card(3001, 1, false)],
                    ..ImportedGameDetail::default()
                },
                ImportedGameDetail {
                    mtgo_game_id: Some(8),
                    game_number: 2,
                    local_deck: vec![card(3001, 4, false)],
                    ..ImportedGameDetail::default()
                },
            ],
            log: vec![
                log_entry(1_000, "@P@PQlocal joined the game."),
                log_entry(1_001, "@P@PQopponent joined the game."),
                log_entry(1_002, "@PTurn 1: Qlocal"),
                log_entry(1_003, "@PQlocal casts @[Qmain@:6002,100:@]."),
                log_entry(1_004, "@PQlocal wins the game."),
                log_entry(2_000, "@P@PQlocal joined the game."),
                log_entry(2_001, "@P@PQopponent joined the game."),
                log_entry(2_002, "@PTurn 1: Qopponent"),
                log_entry(2_003, "@PQlocal casts @[Qside@:6004,200:@]."),
                log_entry(2_004, "@PQlocal wins the game."),
            ],
        }
    }

    /// `(catalog id, main, side, sided in, sided out, cast, inferred)` of a
    /// game's local rows.
    fn local_deck_of(
        conn: &Connection,
        token: &str,
        number: u32,
    ) -> Vec<(u32, u32, u32, u32, u32, u32, bool)> {
        read_game_cards(conn, game_id(conn, token, number))
            .expect("cards")
            .cards
            .iter()
            .filter(|row| row.side == Side::Local)
            .map(|row| {
                (
                    row.catalog_id,
                    row.main_qty,
                    row.side_qty,
                    row.sided_in,
                    row.sided_out,
                    row.cast,
                    row.inferred,
                )
            })
            .collect()
    }

    #[test]
    fn imported_matches_detect_sided_in_from_the_log_not_the_seen_only_deck_json() {
        // The inverse of 0.x "skips sided_in/sided_out detection for imported
        // matches because deck_json is seen-only" (issue #6, the deliberate
        // fix): the seen-only deck_json still never fakes a sided in, and the
        // sideboard card the log shows is sided in.
        let (_dir, mut conn) = db("detail-rule-sided-in");
        import_row(&mut conn, TOKEN, Some(42));
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        assert_eq!(
            local_deck_of(&conn, TOKEN, 2),
            vec![(3001, 4, 0, 0, 0, 0, true), (3002, 1, 1, 1, 0, 1, true)]
        );
        assert_eq!(
            local_deck_of(&conn, TOKEN, 1),
            vec![(3001, 4, 0, 0, 0, 1, false), (3002, 0, 2, 0, 0, 0, false)]
        );
        let again = import_detail(&mut conn, TOKEN, &postboard_detail()).expect("again");
        assert_eq!(again.change, DetailChange::Unchanged);
    }

    #[test]
    fn uses_deck_version_snapshot_quantities_for_imported_matches_instead_of_sparse_deck_json() {
        let (_dir, mut conn) = db("detail-rule-snapshot");
        import_row(&mut conn, TOKEN, Some(42));
        link_version(
            &conn,
            TOKEN,
            &[(9001, 4, false), (9002, 4, false), (9003, 2, false)],
        );
        // Sparse deck_json: only 9002 was seen this game.
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                local_deck: vec![card(9002, 1, false)],
                ..ImportedGameDetail::default()
            }],
            log: postboard_detail().log[..5].to_vec(),
        };
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        let quantities: Vec<(u32, u32)> = local_deck_of(&conn, TOKEN, 1)
            .iter()
            .map(|row| (row.0, row.1))
            .collect();
        assert_eq!(quantities, vec![(3001, 0), (9001, 4), (9002, 4), (9003, 2)]);
    }

    // S7-1, after issue #313: frames no longer decide, and the rows are the
    // same whichever came first, version or detail, even for a game whose
    // frames do not parse.
    #[test]
    fn unreadable_frames_change_nothing_at_import_or_reapply() {
        let (_dir, mut conn) = db("detail-rule-unreadable-frames");
        let mut detail = postboard_detail();
        detail.games[1].frames = vec![FrameText {
            time: "10:00:01".to_string(),
            text: "not json".to_string(),
        }];
        import_row(&mut conn, TOKEN, Some(42));
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        import_row(&mut conn, OTHER, Some(43));
        import_detail(&mut conn, OTHER, &detail).expect("store");
        conn.execute(
            "UPDATE matches SET deck_version_id = 7 WHERE token = ?1",
            [OTHER],
        )
        .expect("link later");
        let tx = conn.transaction().expect("tx");
        reapply_imported_rule(&tx, match_id(&tx, OTHER)).expect("reapply");
        tx.commit().expect("commit");
        assert_eq!(
            local_deck_of(&conn, TOKEN, 2),
            local_deck_of(&conn, OTHER, 2),
            "version first or detail first: the same rows"
        );
        assert!(
            local_deck_of(&conn, TOKEN, 2).iter().all(|row| row.6),
            "a history game gets the rule, frames or not"
        );
    }

    // S7-6: a match whose stored game log cannot be read keeps its rows and
    // its link; the rest of the deck import goes on.
    #[test]
    fn a_corrupt_game_log_does_not_fail_the_deck_import() {
        let (_dir, mut conn) = db("detail-rule-corrupt-log");
        import_row(&mut conn, TOKEN, Some(42));
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        conn.execute("UPDATE game_logs SET decoded_entries = X'00'", [])
            .expect("corrupt");
        let before = local_deck_of(&conn, TOKEN, 2);
        let signature = decks::signature(&[DeckCard {
            catalog_id: 3001,
            quantity: 4,
            sideboard: false,
        }]);
        let decks_data = crate::import_decks::ImportedDecks {
            cards: Vec::new(),
            decks: vec![crate::import_decks::ImportedDeck {
                mtgo_id: "q-deck".to_string(),
                account: Some("Qlocal".to_string()),
                name: "Qdeck".to_string(),
                original_name: None,
                format: "CMODERN".to_string(),
                color_identity: None,
                cover_catalog_id: None,
                archived_at_ms: None,
                created_at_ms: Some(0),
                updated_at_ms: Some(0),
                versions: vec![crate::import_decks::ImportedVersion {
                    signature: signature.clone(),
                    modified_at_ms: 0,
                }],
            }],
            links: vec![crate::import_decks::ImportedLink {
                token: TOKEN.to_string(),
                mtgo_match_id: Some(42),
                deck_mtgo_id: "q-deck".to_string(),
                signature,
            }],
        };
        let counts = crate::import_decks::import_decks(&mut conn, &decks_data, 0)
            .expect("the import goes on");
        assert_eq!((counts.links_added, counts.rules_failed), (1, 1));
        assert_eq!(counts.decks_added, 1);
        let linked: Option<i64> = conn
            .query_row(
                "SELECT deck_version_id FROM matches WHERE token = ?1",
                [TOKEN],
                |row| row.get(0),
            )
            .expect("match");
        assert!(linked.is_some(), "the link stays");
        assert_eq!(local_deck_of(&conn, TOKEN, 2), before, "rows as imported");
    }

    // S7-2: the store gives the rule the card data's oracle ids, so the log's
    // other printing of a registered sideboard card is sided in; its row
    // folds into the registered printing (review I1).
    #[test]
    fn another_printing_in_the_log_is_sided_in_by_oracle() {
        let (_dir, mut conn) = db("detail-rule-other-printing");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
                 (3002, 'q-oracle-side', 'Qside'), (3902, 'q-oracle-side', 'Qside');",
        )
        .expect("cards");
        import_row(&mut conn, TOKEN, Some(42));
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        let mut detail = postboard_detail();
        detail.log[8] = log_entry(2_003, "@PQlocal casts @[Qside@:7804,200:@].");
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        let game_two = local_deck_of(&conn, TOKEN, 2);
        let side = game_two
            .iter()
            .find(|row| row.0 == 3002)
            .expect("registered");
        assert_eq!((side.1, side.2, side.3), (1, 1, 1), "sided in");
        assert_eq!(side.5, 1, "its cast counted once, on the registered row");
        assert!(game_two.iter().all(|row| row.0 != 3902), "folded");
    }

    // Review N1: 0.x's row for an oracle already counts every printing the
    // player touched (a warp cast logged under a variant's id), so once it
    // lands the other printings' counters are cleared and the rule's fold
    // adds nothing on top, whichever printing has the lower catalog id.
    #[test]
    fn a_variant_cast_is_counted_once_when_0x_stats_land() {
        for variant in [3901_u32, 2901] {
            let (_dir, mut conn) = db(&format!("detail-variant-0x-stats-{variant}"));
            conn.execute(
                "INSERT INTO cards (catalog_id, oracle_id, name) VALUES
                     (3001, 'q-oracle-main', 'Qmain'), (?1, 'q-oracle-main', 'Qmain')",
                [variant],
            )
            .expect("cards");
            import_row(&mut conn, TOKEN, Some(42));
            link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
            let mut detail = postboard_detail();
            detail.log[8] = log_entry(
                2_003,
                &format!("@PQlocal casts @[Qmain@:{},200:@].", variant * 2),
            );
            detail.games[1].card_stats = vec![ImportedCardStat {
                quantity: 4,
                seen: 2,
                cast: 3,
                methods: vec![("warp".to_string(), 1)],
                ..stat("q-oracle-main", false)
            }];
            import_detail(&mut conn, TOKEN, &detail).expect("store");
            let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 2)).expect("cards");
            assert!(
                cards.row(Side::Local, variant).is_none(),
                "{variant}: folded"
            );
            let main = cards.row(Side::Local, 3001).expect("registered");
            assert_eq!((main.cast, main.seen), (3, 2), "{variant}: 0.x's numbers");
            let warp: Vec<(u32, u32)> = cards
                .casts
                .iter()
                .filter(|cast| cast.side == Side::Local && cast.method == "warp")
                .map(|cast| (cast.catalog_id, cast.count))
                .collect();
            assert_eq!(warp, vec![(3001, 1)], "{variant}");
        }
    }

    #[test]
    fn a_match_without_a_deck_version_is_left_as_imported() {
        let (_dir, mut conn) = db("detail-rule-no-version");
        import_row(&mut conn, TOKEN, Some(42));
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        let game_two = local_deck_of(&conn, TOKEN, 2);
        assert!(game_two.iter().all(|row| !row.6), "nothing inferred");
        let side = game_two
            .iter()
            .find(|row| row.0 == 3002)
            .expect("the log card");
        assert_eq!(
            (side.1, side.2, side.3, side.5),
            (0, 0, 0, 1),
            "not sided in"
        );
        // The 0.x import links the version after the details: the rule
        // applies then, as if the version was there first.
        let registered = vec![
            DeckCard {
                catalog_id: 3001,
                quantity: 4,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 3002,
                quantity: 2,
                sideboard: true,
            },
        ];
        let signature = decks::signature(&registered);
        let decks_data = crate::import_decks::ImportedDecks {
            cards: Vec::new(),
            decks: vec![crate::import_decks::ImportedDeck {
                mtgo_id: "q-deck".to_string(),
                account: Some("Qlocal".to_string()),
                name: "Qdeck".to_string(),
                original_name: None,
                format: "CMODERN".to_string(),
                color_identity: None,
                cover_catalog_id: None,
                archived_at_ms: None,
                created_at_ms: Some(0),
                updated_at_ms: Some(0),
                versions: vec![crate::import_decks::ImportedVersion {
                    signature: signature.clone(),
                    modified_at_ms: 0,
                }],
            }],
            links: vec![crate::import_decks::ImportedLink {
                token: TOKEN.to_string(),
                mtgo_match_id: Some(42),
                deck_mtgo_id: "q-deck".to_string(),
                signature,
            }],
        };
        let counts = crate::import_decks::import_decks(&mut conn, &decks_data, 0).expect("decks");
        assert_eq!(counts.links_added, 1);
        let (_dir2, mut fresh) = db("detail-rule-no-version-fresh");
        import_row(&mut fresh, TOKEN, Some(42));
        link_version(&fresh, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        import_detail(&mut fresh, TOKEN, &postboard_detail()).expect("store");
        for number in [1, 2] {
            assert_eq!(
                local_deck_of(&conn, TOKEN, number),
                local_deck_of(&fresh, TOKEN, number)
            );
        }
        let tx = conn.transaction().expect("tx");
        assert!(!reapply_imported_rule(&tx, match_id(&tx, TOKEN)).expect("again"));
    }

    /// Issue #294: 0.x never detected sideboarding for its own imported
    /// matches (`$trackSideboarding = false`, their `deck_json` is seen
    /// only). A game of such a match with no deck version or no log
    /// therefore shows no sided in or out from the seen-only lists; a match
    /// 0.x tracked keeps them, with or without frames (issue #313).
    #[test]
    fn a_frameless_game_without_the_rule_has_no_sideboarding_from_seen_only_lists() {
        let (_dir, mut conn) = db("detail-no-sideboarding");
        import_row(&mut conn, TOKEN, Some(42));
        let mut detail = postboard_detail();
        detail.log.clear();
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        for number in [1, 2] {
            let sided: Vec<(u32, u32, u32)> = local_deck_of(&conn, TOKEN, number)
                .iter()
                .map(|row| (row.0, row.3, row.4))
                .collect();
            assert!(
                sided.iter().all(|row| row.1 == 0 && row.2 == 0),
                "game {number}: {sided:?}"
            );
        }
        assert_eq!(local_deck_of(&conn, TOKEN, 2)[0].1, 4, "the list stays");

        // A tracked match: the lists are the decks MTGO logged, so
        // sideboarding stays (no frames needed).
        let (_dir2, mut tracked) = db("detail-sideboarding-tracked");
        import_flagged(&mut tracked, TOKEN, false);
        let mut lists_only = postboard_detail();
        lists_only.log.clear();
        import_detail(&mut tracked, TOKEN, &lists_only).expect("store");
        let game_two = local_deck_of(&tracked, TOKEN, 2);
        assert_eq!((game_two[0].0, game_two[0].3), (3001, 3), "sided in");
    }

    #[test]
    fn a_pulled_history_match_gets_the_rule_once_its_version_is_linked() {
        let (_dir, mut conn) = db("detail-rule-pulled");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0)",
            [],
        )
        .expect("account");
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        let signature: String = conn
            .query_row(
                "SELECT signature FROM deck_versions WHERE id = 7",
                [],
                |row| row.get(0),
            )
            .expect("signature");
        let pulled = crate::import::PulledMatch {
            platform: sync_bundles::BundlePlatform::Mtgo,
            sources: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            endstep_ranked: None,
            endstep_manual_cards: Vec::new(),
            game_structure: None,
            imported: ImportedMatch {
                legacy_history: true,
                ..imported(TOKEN, Some(42))
            },
            detail: postboard_detail(),
            deck: Some(("q-deck".to_string(), signature)),
            league_client_id: None,
            history: true,
            archetypes: Vec::new(),
            player: sync_bundles::PulledPlayerFields::default(),
        };
        let tx = conn.transaction().expect("tx");
        crate::import::import_pulled_match(&tx, &pulled, 5_000).expect("pulled");
        tx.commit().expect("commit");
        assert_eq!(
            local_deck_of(&conn, TOKEN, 2),
            vec![(3001, 4, 0, 0, 0, 0, true), (3002, 1, 1, 1, 0, 1, true)]
        );
    }

    /// A database with deck `q-deck` (account Qlocal) and no match.
    fn pull_db(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 0)",
            [],
        )
        .expect("account");
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        (dir, conn)
    }

    /// Pull the postboard match with this deck list (none for no deck),
    /// ending at `ended_at_ms` (a change makes a re-pull an update).
    fn pull(conn: &mut Connection, deck: Option<&[(u32, u32, bool)]>, ended_at_ms: i64) {
        let deck = deck.map(|cards| {
            let rows: Vec<DeckCard> = cards
                .iter()
                .map(|&(catalog_id, quantity, sideboard)| DeckCard {
                    catalog_id,
                    quantity,
                    sideboard,
                })
                .collect();
            ("q-deck".to_string(), decks::signature(&rows))
        });
        let mut match_row = imported(TOKEN, Some(42));
        match_row.ended_at_ms = Some(ended_at_ms);
        let pulled = crate::import::PulledMatch {
            platform: sync_bundles::BundlePlatform::Mtgo,
            sources: None,
            local_player_fields: Default::default(),
            opponent_player_fields: Default::default(),
            endstep_ranked: None,
            endstep_manual_cards: Vec::new(),
            game_structure: None,
            imported: match_row,
            detail: postboard_detail(),
            deck,
            league_client_id: None,
            history: false,
            archetypes: Vec::new(),
            player: sync_bundles::PulledPlayerFields::default(),
        };
        let tx = conn.transaction().expect("tx");
        let done = crate::import::import_pulled_match(&tx, &pulled, 5_000).expect("pulled");
        assert!(done.stored);
        tx.commit().expect("commit");
    }

    fn all_rows(conn: &Connection) -> Vec<GameCards> {
        [1, 2]
            .iter()
            .map(|number| read_game_cards(conn, game_id(conn, TOKEN, *number)).expect("cards"))
            .collect()
    }

    #[test]
    fn a_repulled_match_with_a_new_or_no_version_equals_a_fresh_pull() {
        let first: &[(u32, u32, bool)] = &[(3001, 4, false), (3002, 2, true)];
        let second: &[(u32, u32, bool)] = &[(3001, 4, false), (3003, 2, true)];
        for (label, next) in [("changed", Some(second)), ("cleared", None)] {
            let (_dir, mut repulled) = pull_db(&format!("detail-repull-{label}"));
            pull(&mut repulled, Some(first), 14_000);
            pull(&mut repulled, next, 15_000);
            let (_dir2, mut fresh) = pull_db(&format!("detail-repull-{label}-fresh"));
            pull(&mut fresh, next, 15_000);
            assert_eq!(all_rows(&repulled), all_rows(&fresh), "{label}");
        }
    }

    #[test]
    fn an_unreadable_game_log_fails_the_reapply_instead_of_skipping_the_rule() {
        let (_dir, mut conn) = db("detail-rule-bad-log");
        import_row(&mut conn, TOKEN, Some(42));
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        let id = match_id(&conn, TOKEN);
        conn.execute(
            "INSERT INTO game_logs (match_id, decoded_entries, entry_count, decoded_at)
             VALUES (?1, X'00010203', 1, 0)",
            [id],
        )
        .expect("a corrupt log");
        let tx = conn.transaction().expect("tx");
        assert!(reapply_imported_rule(&tx, id).is_err());
    }

    #[test]
    fn a_0x_tracked_game_keeps_its_logged_deck_whatever_its_frames_say() {
        // A 0.x tracked game's deck_json is the deck as played, not the
        // cards seen: the rule leaves it alone.
        let (_dir, mut conn) = db("detail-rule-frames");
        import_flagged(&mut conn, TOKEN, false);
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        let mut detail = postboard_detail();
        detail.games[1].local_deck = vec![
            card(3001, 3, false),
            card(3002, 1, false),
            card(3002, 1, true),
            card(3001, 1, true),
        ];
        detail.games[1].frames = vec![FrameText {
            time: "10:00:01".to_string(),
            text: r#"{"Cards":[],"Players":[]}"#.to_string(),
        }];
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        let game_two = local_deck_of(&conn, TOKEN, 2);
        assert!(game_two.iter().all(|row| !row.6), "nothing inferred");
        let side = game_two.iter().find(|row| row.0 == 3002).expect("side");
        assert_eq!((side.1, side.2), (1, 1), "the logged deck");
        // Game 1 has no frames: a tracked game all the same (issue #313).
        assert_eq!(local_deck_of(&conn, TOKEN, 1)[0].1, 1, "its own list");
    }

    /// A 0.x match carrying 0.x's `imported` flag (issue #313).
    fn import_flagged(conn: &mut Connection, token: &str, history: bool) {
        let flagged = ImportedMatch {
            legacy_history: history,
            ..imported(token, Some(42))
        };
        import_matches(conn, &[flagged], 5_000).expect("import");
    }

    // Issue #313: a game 0.x tracked from the log but stored without frames
    // (0.x skips timelines when its database is locked) keeps the sided in
    // and out counts of its deck lists; the rule leaves it, now and when
    // applied again.
    #[test]
    fn a_0x_tracked_game_without_frames_keeps_its_sideboarding() {
        let (_dir, mut conn) = db("detail-tracked-frameless");
        import_flagged(&mut conn, TOKEN, false);
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        let game_two = local_deck_of(&conn, TOKEN, 2);
        assert_eq!((game_two[0].0, game_two[0].3), (3001, 3), "sided in");
        assert!(game_two.iter().all(|row| !row.6), "nothing inferred");
        let tx = conn.transaction().expect("tx");
        assert!(!reapply_imported_rule(&tx, match_id(&tx, TOKEN)).expect("reapply"));
        tx.commit().expect("commit");
        assert_eq!(local_deck_of(&conn, TOKEN, 2), game_two);
    }

    // Issue #313: the reapply check also takes a v1 history import
    // (`source = 'history'`) with no 0.x flag, on its own.
    #[test]
    fn reapply_takes_a_v1_history_match_without_the_0x_flag() {
        let (_dir, mut conn) = db("detail-reapply-source-history");
        import_flagged(&mut conn, TOKEN, false);
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        let tracked = local_deck_of(&conn, TOKEN, 2);
        assert!(tracked.iter().all(|row| !row.6), "left alone while tracked");
        conn.execute(
            "UPDATE matches SET source = 'history' WHERE token = ?1",
            [TOKEN],
        )
        .expect("history source");
        let flag: bool = conn
            .query_row(
                "SELECT legacy_history FROM matches WHERE token = ?1",
                [TOKEN],
                |row| row.get(0),
            )
            .expect("flag");
        assert!(!flag, "no 0.x flag: the source alone decides");
        let tx = conn.transaction().expect("tx");
        assert!(reapply_imported_rule(&tx, match_id(&tx, TOKEN)).expect("reapply"));
        tx.commit().expect("commit");
        assert_ne!(local_deck_of(&conn, TOKEN, 2), tracked, "the rule applied");
    }

    // Issue #313: a match 0.x imported from MTGO's history gets the rule by
    // its flag, not by its missing frames.
    #[test]
    fn a_0x_history_game_gets_the_rule_by_its_flag() {
        let (_dir, mut conn) = db("detail-history-flag");
        import_flagged(&mut conn, TOKEN, true);
        link_version(&conn, TOKEN, &[(3001, 4, false), (3002, 2, true)]);
        let mut detail = postboard_detail();
        detail.games[1].frames = vec![FrameText {
            time: "10:00:01".to_string(),
            text: r#"{"Cards":[],"Players":[]}"#.to_string(),
        }];
        import_detail(&mut conn, TOKEN, &detail).expect("store");
        assert_eq!(
            local_deck_of(&conn, TOKEN, 2),
            vec![(3001, 4, 0, 0, 0, 0, true), (3002, 1, 1, 1, 0, 1, true)]
        );
    }

    /// A second version of deck 5 (id `id`) with these cards.
    fn add_version(conn: &Connection, id: i64, cards: &[(u32, u32, bool)]) {
        let rows: Vec<DeckCard> = cards
            .iter()
            .map(|&(catalog_id, quantity, sideboard)| DeckCard {
                catalog_id,
                quantity,
                sideboard,
            })
            .collect();
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (?1, 5, ?2, 1)",
            params![id, decks::signature(&rows)],
        )
        .expect("version");
    }

    fn set_deck(conn: &mut Connection, version: i64) {
        let edit = crate::match_edits::MatchEdit::SetDeckVersion {
            tokens: vec![TOKEN.to_string()],
            deck_version_id: version,
        };
        assert_eq!(
            crate::match_edits::edit_match(conn, &edit).expect("edit"),
            crate::match_edits::MatchEditOutcome::Done
        );
    }

    const LIST_A: [(u32, u32, bool); 2] = [(3001, 4, false), (3002, 2, true)];
    const LIST_B: [(u32, u32, bool); 2] = [(3001, 2, false), (3002, 4, true)];

    // Issue #332: "Set deck" on a history match applies the imported
    // postboard rule again, so its rows follow the new list as if the match
    // had been imported with it.
    #[test]
    fn setting_a_history_matchs_deck_rules_its_rows_by_the_new_list() {
        let (_dir, mut conn) = db("detail-set-deck-a-then-b");
        import_flagged(&mut conn, TOKEN, true);
        link_version(&conn, TOKEN, &LIST_A);
        add_version(&conn, 8, &LIST_B);
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        let with_a = (
            local_deck_of(&conn, TOKEN, 1),
            local_deck_of(&conn, TOKEN, 2),
        );

        set_deck(&mut conn, 8);
        let with_b = (
            local_deck_of(&conn, TOKEN, 1),
            local_deck_of(&conn, TOKEN, 2),
        );
        assert_ne!(with_b, with_a, "the rows follow the new list");

        // The same match imported with list B from the start.
        let (_other, mut fresh) = db("detail-set-deck-b-from-start");
        import_flagged(&mut fresh, TOKEN, true);
        link_version(&fresh, TOKEN, &LIST_B);
        import_detail(&mut fresh, TOKEN, &postboard_detail()).expect("store");
        assert_eq!(
            with_b,
            (
                local_deck_of(&fresh, TOKEN, 1),
                local_deck_of(&fresh, TOKEN, 2)
            )
        );

        set_deck(&mut conn, 7);
        assert_eq!(
            (
                local_deck_of(&conn, TOKEN, 1),
                local_deck_of(&conn, TOKEN, 2)
            ),
            with_a,
            "and back"
        );
    }

    // Issue #332: a new list the rule cannot read (no cards) leaves the rows
    // the previous list gave them.
    #[test]
    fn a_new_deck_whose_list_cannot_be_read_keeps_the_rows() {
        let (_dir, mut conn) = db("detail-set-deck-unreadable");
        import_flagged(&mut conn, TOKEN, true);
        link_version(&conn, TOKEN, &LIST_A);
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (9, 5, '', 1)",
            [],
        )
        .expect("empty version");
        import_detail(&mut conn, TOKEN, &postboard_detail()).expect("store");
        let with_a = (
            local_deck_of(&conn, TOKEN, 1),
            local_deck_of(&conn, TOKEN, 2),
        );

        set_deck(&mut conn, 9);
        assert_eq!(
            (
                local_deck_of(&conn, TOKEN, 1),
                local_deck_of(&conn, TOKEN, 2)
            ),
            with_a
        );
    }

    #[test]
    fn card_stats_rows_without_an_oracle_id_count_as_skipped() {
        let (_dir, mut conn) = db("detail-card-stats-blank");
        import_row(&mut conn, TOKEN, Some(42));
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                card_stats: vec![stat("", false), stat("  ", true)],
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        };
        let import = import_detail(&mut conn, TOKEN, &detail).expect("store");
        assert_eq!(import.card_stats_skipped, 2);
    }

    #[test]
    fn opening_hand_json_and_deck_json_become_rows() {
        let (_dir, mut conn) = db("detail-hand");
        import_row(&mut conn, TOKEN, Some(42));
        let detail = ImportedDetail {
            games: vec![ImportedGameDetail {
                mtgo_game_id: Some(7),
                game_number: 1,
                local_deck: vec![card(4001, 4, false), card(4002, 2, true)],
                opponent_cards: vec![card(5001, 2, false)],
                local_hand: Some(ImportedHand {
                    starting: vec![4001, 4001, 4001, 4001, 4004, 4004],
                    bottomed: vec![4005],
                    mulligans: vec![vec![4004; 7]],
                }),
                local_starting_hand_size: Some(6),
                opp_starting_hand_size: Some(5),
                ..ImportedGameDetail::default()
            }],
            log: Vec::new(),
        };
        assert_eq!(change(&mut conn, TOKEN, &detail), DetailChange::Added);
        let cards = read_game_cards(&conn, game_id(&conn, TOKEN, 1)).expect("cards");
        assert_eq!(cards.opening_hand_size, Some(7));
        assert_eq!(cards.starting_hand_size, Some(6));
        assert_eq!(cards.opp_opening_hand_size, Some(5));
        assert_eq!(cards.opp_starting_hand_size, Some(5));
        let bolt = cards.row(Side::Local, 4001).expect("bolt");
        assert_eq!(
            (bolt.main_qty, bolt.opening_hand, bolt.starting_hand),
            (4, 4, 4)
        );
        let bottomed = cards.row(Side::Local, 4005).expect("bottomed");
        assert_eq!(
            (
                bottomed.opening_hand,
                bottomed.bottomed,
                bottomed.starting_hand
            ),
            (1, 1, 0)
        );
        assert_eq!(cards.row(Side::Local, 4002).expect("side").side_qty, 2);
        assert_eq!(cards.row(Side::Opponent, 5001).expect("seen").seen, 2);
        assert_eq!(cards.mulligans, vec![(1, 4004, 7)]);
        assert_eq!(change(&mut conn, TOKEN, &detail), DetailChange::Unchanged);
    }
}
