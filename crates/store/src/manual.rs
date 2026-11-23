//! Manual matches (Plan L, dashboard spec section 5; 0.x `CreateManualMatch`
//! and the `UpdateManualGame*` actions): a match the player entered by hand
//! because the tracker never saw it.
//!
//! A manual match is a full `matches` row with `source = 'manual'`, its games
//! and its deck link, so every list, deck, dashboard and league read counts it
//! like any other match. The projection never derives, rebuilds, replaces or
//! removes one: it has a fresh UUID token no log ever carries and no MTGO
//! match id, and every projection write skips `source = 'manual'`. Each game's
//! hand, sideboard and the opponent's revealed cards are its game rows
//! (`game_cards`, `game_mulligan_cards` and the hand sizes on `games`), which
//! the game sync never touches for a manual match.

use std::collections::BTreeMap;

use chrono_tz::Tz;
use decks::decode_signature;
use game_detail::{GameCardRow, GameCards, Side};
use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::accounts::current_account_id;
use crate::archetype_edit::write_manual_seat;
use crate::arena::named_arena_opponent;
use crate::arena_rank::{claim_gap, gap_claimable, set_match_rank};
use crate::league_sync::{LeagueOwner, sync_owner};
use crate::manual_rules::{self, FieldError, HandInput};
use crate::names::{game_result_from_str, outcome_from_str, state_from_str};
use crate::platform::{BestOf, Platform};
use crate::write::registry_id;
use crate::{Connection, StoreError};

/// One game as the player entered it (0.x `games.*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualGame {
    pub won: bool,
    pub on_play: bool,
    /// How many turns the game took, when the player knows.
    pub turns: Option<u32>,
}

/// Where a new manual match is filed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualPlacement {
    /// In no league.
    None,
    /// In a league run found by its league token: the match carries the token
    /// and the run's game structure, so the league fold places it like any
    /// league match. `deck_version_id` is the run's deck version, when the
    /// run has one: the match is linked to it.
    Run {
        league_token: String,
        game_structure: Option<String>,
        deck_version_id: Option<i64>,
        /// The `leagues.id` of a 0.x run with no account (issue #162): the
        /// match's account takes it over in the same write.
        adopt_league_id: Option<i64>,
    },
    /// In a manual league (`leagues.id`, its run key `manual:{id}`): the
    /// match is linked by hand (`league_id`, `league_linked`) and takes the
    /// league's deck version.
    ManualLeague { id: i64 },
}

/// A match to create (0.x `StoreManualMatchRequest` once validated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualMatch {
    pub deck_id: i64,
    /// The opponent's name, trimmed. An Arena match may leave it blank: no
    /// opponent.
    pub opponent: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    /// One to three games, in the order played.
    pub games: Vec<ManualGame>,
    pub placement: ManualPlacement,
    /// The opponent's archetype (0.x `archetype_id`), hand-set; the player
    /// seat takes the deck's archetype (archetypes spec section 8).
    pub opponent_archetype_id: Option<i64>,
    /// Arena only: a ranked match (else unranked). MTGO decks ignore it.
    pub ranked: bool,
    /// Arena only: best of one or three. MTGO decks ignore it.
    pub best_of: BestOf,
    /// Arena only: the rank gap the match was played in, named by the
    /// constructed reading that ends it `(file id, byte offset)`. MTGO decks
    /// ignore it.
    pub rank_gap: Option<(i64, u64)>,
    pub now_ms: i64,
}

/// Copies of one printing (MTGO catalog id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardCount {
    pub catalog_id: u32,
    pub quantity: u32,
}

/// Which way a sideboard change moves cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the sideboard into the maindeck.
    In,
    /// From the maindeck to the sideboard.
    Out,
}

/// One line of a sideboard edit (0.x `changes.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideboardChange {
    pub catalog_id: u32,
    pub quantity: u32,
    pub direction: Direction,
}

/// A user edit of manual matches, applied by the tracker's writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualEdit {
    /// 0.x `CreateManualMatch`: a complete match with a fresh token.
    Create(ManualMatch),
    /// 0.x `UpdateManualGameHand`: the starting hand (0.x `kept`), the cards
    /// bottomed after the last mulligan, and each hand put back (one per
    /// mulligan, empty when the player does not remember it). Catalog ids,
    /// one entry per copy. An empty starting hand with no mulligans clears
    /// the record. Checked with 0.x `UpdateManualGameHandRequest`'s rules.
    SetHand {
        token: String,
        game: u32,
        mulligan_count: u32,
        starting: Vec<u32>,
        bottomed: Vec<u32>,
        mulligans: Vec<Vec<u32>>,
    },
    /// 0.x `UpdateManualGameSideboard`: the changes from the registered deck
    /// for this game. No changes resets the game to the registered deck. A
    /// recorded hand the new maindeck cannot hold is cleared
    /// (`HandCleared`).
    SetSideboard {
        token: String,
        game: u32,
        changes: Vec<SideboardChange>,
    },
    /// 0.x `UpdateManualGameReveals`: the cards the opponent showed. An
    /// empty list clears them.
    SetReveals {
        token: String,
        game: u32,
        cards: Vec<CardCount>,
    },
    /// "Edit match" (edit a match spec section 4): the player's fix of any
    /// listed match (log, imported, history or manual), written into its
    /// real columns and games and stamped `corrected_at`, so a rebuild or a
    /// re-import never undoes it.
    Correct {
        match_id: i64,
        correction: MatchCorrection,
    },
}

/// The player's fix of a match, as the edit sheet sends it. The games are
/// the Add sheet's (`ManualGame`), checked with the same rules
/// (`manual_rules::correction_errors`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchCorrection {
    /// The opponent's name; `None` keeps a match with no known opponent.
    pub opponent_name: Option<String>,
    /// `None`: the player left the start alone, and the stored one (to the
    /// millisecond: the sheet shows minutes) is kept.
    pub started_at_ms: Option<i64>,
    /// `None`: the player left the end alone, and the stored one is kept
    /// (a match with no known end keeps none).
    pub ended_at_ms: Option<i64>,
    /// One to three games, in the order played: games 1 to n.
    pub games: Vec<ManualGame>,
    pub now_ms: i64,
}

impl ManualEdit {
    /// The match an edit of an existing manual match changes (`None` for a
    /// create, and for a correction, which names its match by row id).
    pub fn token(&self) -> Option<&str> {
        match self {
            ManualEdit::Create(_) | ManualEdit::Correct { .. } => None,
            ManualEdit::SetHand { token, .. }
            | ManualEdit::SetSideboard { token, .. }
            | ManualEdit::SetReveals { token, .. } => Some(token),
        }
    }
}

/// What happened to a `ManualEdit`. Only `Created`, `Done` and
/// `HandCleared` changed anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualEditOutcome {
    /// The new match's token.
    Created {
        token: String,
    },
    Done,
    /// A sideboard edit was done and cleared the game's hand, which used a
    /// card the new maindeck lacks (0.x shows
    /// `manual_rules::SIDEBOARD_SAVED_HAND_CLEARED`).
    HandCleared,
    /// The edit broke 0.x's form rules: each field's message.
    Invalid(Vec<FieldError>),
    /// There is no such match, or it was removed from stats.
    NotFound,
    /// The match was not entered by hand (0.x answers 403).
    NotManual,
    /// The match has no game with this number.
    GameNotFound,
    /// The deck does not exist or is not the current account's.
    InvalidDeck,
    NoDeckVersion,
    /// The manual league does not exist.
    InvalidLeague,
    /// The league was played with another deck.
    WrongLeagueDeck,
    /// The manual league already has five complete matches.
    LeagueFull,
}

impl ManualEditOutcome {
    /// The message to show for a refusal (0.x's where 0.x has one; the first
    /// field's for `Invalid`), `None` for an edit that was done.
    pub fn refusal(&self) -> Option<String> {
        let message = match self {
            ManualEditOutcome::Created { .. }
            | ManualEditOutcome::Done
            | ManualEditOutcome::HandCleared => return None,
            ManualEditOutcome::Invalid(errors) => {
                return errors.first().map(|error| error.message.clone());
            }
            ManualEditOutcome::NotFound => "This match no longer exists.",
            ManualEditOutcome::NotManual => "Only a match entered by hand can be edited here.",
            ManualEditOutcome::GameNotFound => "This game no longer exists.",
            ManualEditOutcome::InvalidDeck => "The selected deck id is invalid.",
            ManualEditOutcome::NoDeckVersion => {
                "This deck has no saved version to attach the match to."
            }
            ManualEditOutcome::InvalidLeague => "The selected league id is invalid.",
            ManualEditOutcome::WrongLeagueDeck => "This league was played with a different deck.",
            ManualEditOutcome::LeagueFull => "This league already has a full run of matches.",
        };
        Some(message.to_string())
    }
}

/// A card of the opponent archetype's list (the reveals quick picks, 0.x
/// `GetArchetypeDecklistForMatch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchetypeListCard {
    pub catalog_id: u32,
    /// `None` for a card with no card data.
    pub name: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// The opponent archetype's list for the match `token`: list 1 of a
/// downloaded archetype, or the first variant of a manual one, maindeck
/// first, then by name (unnamed cards last). `None` without an opponent
/// archetype or a list (0.x answered null for an empty list too).
pub fn archetype_decklist(
    conn: &Connection,
    token: &str,
) -> Result<Option<Vec<ArchetypeListCard>>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT l.catalog_id, c.name, l.quantity, l.sideboard
         FROM matches m
         JOIN match_archetypes x ON x.match_id = m.id AND x.is_opponent = 1
         JOIN archetype_cards l ON l.archetype_id = x.archetype_id
         LEFT JOIN cards c ON c.catalog_id = l.catalog_id
         WHERE m.token = ?1
           AND l.list = (SELECT MIN(list) FROM archetype_cards WHERE archetype_id = x.archetype_id)
         ORDER BY l.sideboard, c.name IS NULL, c.name, l.catalog_id",
    )?;
    let rows = stmt.query_map([token], |row| {
        Ok(ArchetypeListCard {
            catalog_id: u32::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
            name: row.get(1)?,
            quantity: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
            sideboard: row.get::<_, i64>(3)? == 1,
        })
    })?;
    let mut cards = Vec::new();
    for row in rows {
        cards.push(row?);
    }
    Ok(if cards.is_empty() { None } else { Some(cards) })
}

/// One game's recorded detail. Lists are empty when nothing is recorded.
/// Stored as the game's rows: per-card counts, so lists read back in catalog
/// id order, not the order entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualGameDetail {
    pub game_number: u32,
    /// The starting hand (0.x `kept`); `None` when no hand is recorded
    /// ("unknown").
    pub starting_hand: Option<Vec<u32>>,
    pub bottomed: Vec<u32>,
    pub mulligans: Vec<Vec<u32>>,
    /// The maindeck after sideboarding, by catalog id; `None` when the game
    /// was played with the registered deck.
    pub postboard_main: Option<Vec<CardCount>>,
    /// The opponent's revealed cards.
    pub reveals: Vec<CardCount>,
}

impl ManualGameDetail {
    /// Nothing recorded for game `game_number`.
    pub fn empty(game_number: u32) -> Self {
        ManualGameDetail {
            game_number,
            starting_hand: None,
            bottomed: Vec::new(),
            mulligans: Vec::new(),
            postboard_main: None,
            reveals: Vec::new(),
        }
    }

    /// Whether the game has no record at all.
    pub fn is_empty(&self) -> bool {
        self.starting_hand.is_none() && self.postboard_main.is_none() && self.reveals.is_empty()
    }

    /// The game's rows: the hand (opening = starting plus bottomed), the
    /// hands sent back, the postboard maindeck (`main_qty`) and the
    /// opponent's reveals (`seen`). The opponent's hand sizes are unknown.
    pub fn game_cards(&self) -> GameCards {
        let mut rows: BTreeMap<(Side, u32), GameCardRow> = BTreeMap::new();
        let mut cards = GameCards::default();
        if let Some(starting) = &self.starting_hand {
            for catalog_id in starting {
                let entry = card_row(&mut rows, Side::Local, *catalog_id);
                entry.opening_hand += 1;
                entry.starting_hand += 1;
            }
            for catalog_id in &self.bottomed {
                let entry = card_row(&mut rows, Side::Local, *catalog_id);
                entry.opening_hand += 1;
                entry.bottomed += 1;
            }
            cards.starting_hand_size = Some(starting.len() as u32);
            cards.opening_hand_size = Some((starting.len() + self.bottomed.len()) as u32);
            for (index, hand) in self.mulligans.iter().enumerate() {
                let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
                for catalog_id in hand {
                    *counts.entry(*catalog_id).or_insert(0) += 1;
                }
                for (catalog_id, quantity) in counts {
                    cards
                        .mulligans
                        .push((index as u32 + 1, catalog_id, quantity));
                }
            }
        }
        for card in self.postboard_main.iter().flatten() {
            card_row(&mut rows, Side::Local, card.catalog_id).main_qty += card.quantity;
        }
        for card in &self.reveals {
            card_row(&mut rows, Side::Opponent, card.catalog_id).seen += card.quantity;
        }
        cards.cards = rows.into_values().collect();
        cards
    }

    /// The detail a game's rows hold. `local_mulligans` is the game's
    /// mulligan count: a hand sent back with no rows was not remembered.
    pub fn from_game_cards(game_number: u32, cards: &GameCards, local_mulligans: u32) -> Self {
        let mut detail = ManualGameDetail::empty(game_number);
        let local = || cards.cards.iter().filter(|row| row.side == Side::Local);
        if cards.starting_hand_size.is_some() {
            let mut starting = Vec::new();
            for row in local() {
                for _ in 0..row.starting_hand {
                    starting.push(row.catalog_id);
                }
                for _ in 0..row.bottomed {
                    detail.bottomed.push(row.catalog_id);
                }
            }
            detail.starting_hand = Some(starting);
            let hands = cards
                .mulligans
                .iter()
                .map(|(hand, _, _)| *hand)
                .max()
                .unwrap_or(0)
                .max(local_mulligans);
            for hand in 1..=hands {
                let mut cards_of_hand = Vec::new();
                for (number, catalog_id, quantity) in &cards.mulligans {
                    if *number == hand {
                        for _ in 0..*quantity {
                            cards_of_hand.push(*catalog_id);
                        }
                    }
                }
                detail.mulligans.push(cards_of_hand);
            }
        }
        let main: Vec<CardCount> = local()
            .filter(|row| row.main_qty > 0)
            .map(|row| CardCount {
                catalog_id: row.catalog_id,
                quantity: row.main_qty,
            })
            .collect();
        if !main.is_empty() {
            detail.postboard_main = Some(main);
        }
        detail.reveals = cards
            .cards
            .iter()
            .filter(|row| row.side == Side::Opponent && row.seen > 0)
            .map(|row| CardCount {
                catalog_id: row.catalog_id,
                quantity: row.seen,
            })
            .collect();
        detail
    }
}

/// The row of one side's card, made when missing.
fn card_row(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    side: Side,
    catalog_id: u32,
) -> &mut GameCardRow {
    rows.entry((side, catalog_id))
        .or_insert_with(|| GameCardRow {
            side,
            catalog_id,
            ..GameCardRow::default()
        })
}

/// Rounds in a constructed league run (0.x `LeagueKind::Constructed`).
const LEAGUE_ROUNDS: i64 = 5;

/// The form keys of the league and the rank gap.
const LEAGUE_FIELD: &str = "league_id";
const RANK_GAP_FIELD: &str = "rank_gap";
const GAMES_FIELD: &str = "games";
/// A Bo1 match with more or fewer than one game.
pub const BO1_ONE_GAME: &str = "A Bo1 match has one game.";
/// An Arena match placed in a league.
pub const ARENA_NO_LEAGUES: &str = "Arena matches have no leagues.";
/// A rank gap that is not one of the deck's account's, or on an unranked
/// match.
pub const RANK_GAP_UNAVAILABLE: &str = "That rank gap is not available.";
/// The form key of a correction's opponent.
const OPPONENT_FIELD: &str = "opponent_name";
/// A correction that renames an Endstep match's opponent (an Endstep
/// opponent is its Endstep user id; a name alone cannot make one).
pub const ENDSTEP_OPPONENT_KEPT: &str = "An Endstep match keeps its opponent.";

/// Apply a user's manual match edit in its own transaction. A match placed
/// in a league run is fold input: its account is synced (`tz`, the OS
/// timezone) before the edit commits, so the run shows it at once. So is a
/// corrected match: its account and its league's owner are synced.
pub fn edit_manual(
    conn: &mut Connection,
    edit: &ManualEdit,
    tz: Tz,
) -> Result<ManualEditOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = match edit {
        ManualEdit::Create(manual) => {
            let outcome = create(&tx, manual)?;
            if let ManualEditOutcome::Created { token } = &outcome
                && !matches!(manual.placement, ManualPlacement::None)
            {
                let account: i64 = tx.query_row(
                    "SELECT account_id FROM matches WHERE token = ?1",
                    [token],
                    |row| row.get(0),
                )?;
                sync_owner(&tx, LeagueOwner::Account(account), tz, manual.now_ms)?;
            }
            outcome
        }
        ManualEdit::SetHand {
            token,
            game,
            mulligan_count,
            starting,
            bottomed,
            mulligans,
        } => {
            let hand = HandInput {
                mulligan_count: *mulligan_count,
                starting_hand: starting.clone(),
                bottomed: bottomed.clone(),
                mulliganed_hands: mulligans.clone(),
            };
            set_hand(&tx, token, *game, &hand)?
        }
        ManualEdit::SetSideboard {
            token,
            game,
            changes,
        } => set_sideboard(&tx, token, *game, changes)?,
        ManualEdit::SetReveals { token, game, cards } => set_reveals(&tx, token, *game, cards)?,
        ManualEdit::Correct {
            match_id,
            correction,
        } => correct(&tx, *match_id, correction, tz)?,
    };
    // A refusal wrote nothing, so committing it changes nothing either.
    tx.commit()?;
    Ok(outcome)
}

/// A fresh random UUID (version 4 layout), from SQLite's random source: the
/// workspace has no UUID crate.
pub(crate) const NEW_UUID: &str =
    "SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2)))
    || '-4' || substr(lower(hex(randomblob(2))), 2)
    || '-' || substr('89ab', 1 + (random() & 3), 1) || substr(lower(hex(randomblob(2))), 2)
    || '-' || lower(hex(randomblob(6)))";

/// 0.x `CreateManualMatch::run`: a complete match with its games, the deck
/// link and the league placement.
fn create(tx: &Transaction, manual: &ManualMatch) -> Result<ManualEditOutcome, StoreError> {
    // A missing deck is refused below; until then it follows MTGO's rules.
    let platform = crate::platform::deck_platform(tx, manual.deck_id)?;
    // Manual matches are MTGO and Arena only.
    if platform == Platform::Endstep {
        return Ok(ManualEditOutcome::InvalidDeck);
    }
    let errors = manual_rules::match_errors(
        &manual.opponent,
        platform == Platform::Mtgo,
        manual.started_at_ms,
        manual.ended_at_ms,
        &manual.games,
    );
    if !errors.is_empty() {
        return Ok(ManualEditOutcome::Invalid(errors));
    }
    // 0.x `exists:archetypes,id`.
    if let Some(archetype_id) = manual.opponent_archetype_id {
        let exists: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM archetypes WHERE id = ?1)",
            [archetype_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(ManualEditOutcome::Invalid(vec![FieldError {
                field: "archetype_id".to_string(),
                message: "The selected archetype id is invalid.".to_string(),
            }]));
        }
    }
    // 0.x `exists:decks,id` for the current account.
    let deck: Option<(Option<i64>, String, Option<i64>)> = tx
        .query_row(
            "SELECT account_id, format, archetype_id FROM decks WHERE id = ?1",
            [manual.deck_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((Some(account_id), format, deck_archetype_id)) = deck else {
        return Ok(ManualEditOutcome::InvalidDeck);
    };
    if let Some(current) = current_account_id(tx, platform)?
        && current != account_id
    {
        return Ok(ManualEditOutcome::InvalidDeck);
    }
    if platform == Platform::Arena {
        if !matches!(manual.placement, ManualPlacement::None) {
            return Ok(invalid_field(LEAGUE_FIELD, ARENA_NO_LEAGUES));
        }
        if manual.best_of == BestOf::One && manual.games.len() != 1 {
            return Ok(invalid_field(GAMES_FIELD, BO1_ONE_GAME));
        }
        if let Some((file_id, byte_offset)) = manual.rank_gap
            && (!manual.ranked || !gap_claimable(tx, account_id, file_id, byte_offset)?)
        {
            return Ok(invalid_field(RANK_GAP_FIELD, RANK_GAP_UNAVAILABLE));
        }
    }

    let latest: Option<i64> = tx
        .query_row(
            "SELECT id FROM deck_versions WHERE deck_id = ?1
             ORDER BY modified_at DESC, id DESC LIMIT 1",
            [manual.deck_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(latest) = latest else {
        return Ok(ManualEditOutcome::NoDeckVersion);
    };

    let mut version = latest;
    let mut league_token: Option<&str> = None;
    let mut game_structure: Option<&str> = None;
    match &manual.placement {
        ManualPlacement::None => {}
        ManualPlacement::Run {
            league_token: token,
            game_structure: structure,
            deck_version_id,
            adopt_league_id: _,
        } => {
            // The app checked the room before this write; a live match may
            // have committed since, so count again here (0.x ran both in one
            // request).
            let held: i64 = tx.query_row(
                "SELECT COUNT(*) FROM listed_matches m
                 WHERE m.account_id = ?1 AND m.league_token = ?2
                   AND m.source IN ('log', 'manual')
                   AND m.tournament_event_id IS NULL
                   AND m.state IN ('in_progress', 'ended', 'complete', 'abandoned')",
                params![account_id, token],
                |row| row.get(0),
            )?;
            if held >= LEAGUE_ROUNDS {
                return Ok(ManualEditOutcome::LeagueFull);
            }
            league_token = Some(token);
            game_structure = structure.as_deref();
            if let Some(run_version) = deck_version_id {
                let run_deck: Option<i64> = tx
                    .query_row(
                        "SELECT deck_id FROM deck_versions WHERE id = ?1",
                        [run_version],
                        |row| row.get(0),
                    )
                    .optional()?;
                match run_deck {
                    Some(deck_id) if deck_id == manual.deck_id => version = *run_version,
                    Some(_) => return Ok(ManualEditOutcome::WrongLeagueDeck),
                    // The run's version is gone: the deck's latest, as for a
                    // run with no version.
                    None => {}
                }
            }
        }
        ManualPlacement::ManualLeague { id } => {
            if let Some(refusal) = check_manual_league(tx, *id, manual.deck_id)? {
                return Ok(refusal);
            }
            if let Some(league_version) = manual_league_version(tx, *id)? {
                version = league_version;
            }
        }
    }

    let token: String = tx.query_row(NEW_UUID, [], |row| row.get(0))?;
    let opponent = manual.opponent.trim();
    let opponent_id = match platform {
        Platform::Mtgo => Some(registry_id(tx, "opponents", opponent, manual.now_ms)?),
        Platform::Arena if opponent.is_empty() => None,
        Platform::Arena => Some(named_arena_opponent(tx, opponent, manual.now_ms)?),
        // Refused above.
        Platform::Endstep => return Ok(ManualEditOutcome::InvalidDeck),
    };
    let wins = manual.games.iter().filter(|game| game.won).count();
    let losses = manual.games.len() - wins;
    let in_league = !matches!(manual.placement, ManualPlacement::None);
    let match_type = match platform {
        Platform::Arena if manual.ranked => "ranked",
        Platform::Arena | Platform::Endstep => "unranked",
        Platform::Mtgo if in_league => "League",
        Platform::Mtgo => "Constructed",
    };
    // An Arena manual match is best of one or three, as its tracked ones are.
    if platform == Platform::Arena {
        game_structure = Some(manual.best_of.key());
    }
    tx.execute(
        "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source, format,
             match_type, tournament_event_id, tournament_round, state, outcome, started_at,
             ended_at, last_activity_at, needs_attention, attention_reason, revision,
             derived_version, league_token, game_structure)
         VALUES (?1, NULL, ?2, ?3, 'manual', ?4, ?5, NULL, NULL, 'complete', ?6, ?7, ?8, ?8, 0,
             NULL, 1, 0, ?9, ?10)",
        params![
            token,
            account_id,
            opponent_id,
            format,
            match_type,
            manual_rules::outcome(wins, losses),
            manual.started_at_ms,
            manual.ended_at_ms,
            league_token,
            game_structure,
        ],
    )?;
    let match_id = tx.last_insert_rowid();
    // 0.x `CreateManualMatch::createGames`. The match is new, so this only
    // inserts.
    write_corrected_games(
        tx,
        match_id,
        manual.started_at_ms,
        Some(manual.ended_at_ms),
        &manual.games,
    )?;
    tx.execute(
        "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
        params![token, version],
    )?;
    if let ManualPlacement::Run {
        adopt_league_id: Some(league_id),
        ..
    } = &manual.placement
    {
        tx.execute(
            "UPDATE leagues SET account_id = ?2
             WHERE id = ?1 AND account_id IS NULL AND source = 'imported'",
            params![league_id, account_id],
        )?;
    }
    if let ManualPlacement::ManualLeague { id } = manual.placement {
        tx.execute(
            "UPDATE matches SET league_id = ?2, league_linked = 1 WHERE token = ?1",
            params![token, id],
        )?;
    }
    // 0.x `CreateManualMatch` (decision C7: in the same transaction): the
    // opponent seat is hand-set and the match marked detected; the player
    // seat is the deck's archetype as an auto row, so it follows a later deck
    // archetype change (spec section 8).
    write_manual_seat(
        tx,
        match_id,
        true,
        manual.opponent_archetype_id,
        manual.now_ms,
    )?;
    if let Some(archetype_id) = deck_archetype_id {
        write_player_seat(tx, match_id, archetype_id)?;
    }
    if platform == Platform::Arena
        && let Some((file_id, byte_offset)) = manual.rank_gap
    {
        claim_gap(tx, &token, file_id, byte_offset, manual.now_ms)?;
    }
    set_match_rank(tx, match_id)?;
    Ok(ManualEditOutcome::Created { token })
}

/// A refusal of one form field.
fn invalid_field(field: &str, message: &str) -> ManualEditOutcome {
    ManualEditOutcome::Invalid(vec![FieldError {
        field: field.to_string(),
        message: message.to_string(),
    }])
}

/// A new manual match's player seat: the deck's archetype after one merge
/// hop (0.x `ResolveMergedArchetype`), an auto row at confidence 1 (0.x
/// `manual => false`).
fn write_player_seat(tx: &Transaction, match_id: i64, archetype_id: i64) -> Result<(), StoreError> {
    let parent: Option<Option<i64>> = tx
        .query_row(
            "SELECT merged_into_id FROM archetypes WHERE id = ?1",
            [archetype_id],
            |row| row.get(0),
        )
        .optional()?;
    let resolved = archetypes::merge_hop(archetype_id, |_| parent.flatten());
    tx.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         VALUES (?1, 0, ?2, 1.0, 0)",
        params![match_id, resolved],
    )?;
    Ok(())
}

/// 0.x `StoreManualMatchRequest::validateLeague` for a manual league: it
/// exists and is not deleted, was played with this deck (when it knows its
/// deck), and has room.
fn check_manual_league(
    tx: &Transaction,
    league_id: i64,
    deck_id: i64,
) -> Result<Option<ManualEditOutcome>, StoreError> {
    let league_deck: Option<Option<i64>> = tx
        .query_row(
            "SELECT deck_id FROM listed_leagues
             WHERE id = ?1 AND source = 'manual' AND detached_at IS NULL",
            [league_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(league_deck) = league_deck else {
        return Ok(Some(ManualEditOutcome::InvalidLeague));
    };
    if let Some(league_deck) = league_deck
        && league_deck != deck_id
    {
        return Ok(Some(ManualEditOutcome::WrongLeagueDeck));
    }
    let played: i64 = tx.query_row(
        "SELECT COUNT(*) FROM listed_matches m WHERE m.league_id = ?1 AND m.state = 'complete'",
        [league_id],
        |row| row.get(0),
    )?;
    if played >= LEAGUE_ROUNDS {
        return Ok(Some(ManualEditOutcome::LeagueFull));
    }
    Ok(None)
}

/// The deck version of a manual league (0.x linked a match to the league's
/// version): the one stored when the league was made (`leagues.deck_version_id`,
/// the Leagues page's label). `None` for a league with no deck or a deck with
/// no version.
fn manual_league_version(tx: &Transaction, league_id: i64) -> Result<Option<i64>, StoreError> {
    let version: Option<Option<i64>> = tx
        .query_row(
            "SELECT deck_version_id FROM leagues WHERE id = ?1 AND source = 'manual'",
            [league_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(version.flatten())
}

/// The id of the manual match `token` once it has game `game`, or why an
/// edit of it is refused (0.x: 404 for a missing match or game, 403 for a
/// match that was not entered by hand).
fn manual_game(
    tx: &Transaction,
    token: &str,
    game: u32,
) -> Result<Result<i64, ManualEditOutcome>, StoreError> {
    let found: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, source FROM listed_matches WHERE token = ?1",
            [token],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((match_id, source)) = found else {
        return Ok(Err(ManualEditOutcome::NotFound));
    };
    if source != "manual" {
        return Ok(Err(ManualEditOutcome::NotManual));
    }
    let has_game: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM games WHERE match_id = ?1 AND game_number = ?2)",
        params![match_id, game],
        |row| row.get(0),
    )?;
    if !has_game {
        return Ok(Err(ManualEditOutcome::GameNotFound));
    }
    Ok(Ok(match_id))
}

/// Every edit is a new revision of the match, so the pushed row replaces the
/// one a list holds.
fn bump_revision(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE matches SET revision = revision + 1 WHERE id = ?1",
        [match_id],
    )?;
    Ok(())
}

/// The id of game `game` of the match.
fn game_id_of(conn: &Connection, match_id: i64, game: u32) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        "SELECT id FROM games WHERE match_id = ?1 AND game_number = ?2",
        params![match_id, game],
        |row| row.get(0),
    )?)
}

/// What game `game` of the match has recorded (empty when nothing is).
pub(crate) fn stored_detail(
    conn: &Connection,
    match_id: i64,
    game: u32,
) -> Result<ManualGameDetail, StoreError> {
    let game_id = game_id_of(conn, match_id, game)?;
    let mulligans: Option<u32> = conn.query_row(
        "SELECT local_mulligans FROM games WHERE id = ?1",
        [game_id],
        |row| row.get(0),
    )?;
    let cards = crate::game_sync::read_game_cards(conn, game_id)?;
    Ok(ManualGameDetail::from_game_cards(
        game,
        &cards,
        mulligans.unwrap_or(0),
    ))
}

/// Store game `detail.game_number`'s record as its rows. Returns whether
/// anything changed.
pub(crate) fn write_detail(
    tx: &Transaction,
    match_id: i64,
    detail: &ManualGameDetail,
) -> Result<bool, StoreError> {
    let game_id = game_id_of(tx, match_id, detail.game_number)?;
    crate::game_sync::write_game_cards(tx, game_id, &detail.game_cards())
}

/// 0.x `UpdateManualGameHand`: the hand and the game's mulligan count,
/// once 0.x `UpdateManualGameHandRequest`'s rules pass against the game's
/// maindeck.
fn set_hand(
    tx: &Transaction,
    token: &str,
    game: u32,
    hand: &HandInput,
) -> Result<ManualEditOutcome, StoreError> {
    let match_id = match manual_game(tx, token, game)? {
        Ok(id) => id,
        Err(refusal) => return Ok(refusal),
    };
    let registered = registered_deck(tx, token)?;
    let mut detail = stored_detail(tx, match_id, game)?;
    let maindeck = manual_rules::game_maindeck(&registered.mains, detail.postboard_main.as_deref());
    let errors = manual_rules::validate_hand(hand, &maindeck);
    if !errors.is_empty() {
        return Ok(ManualEditOutcome::Invalid(errors));
    }
    if hand.is_clearing() {
        detail.starting_hand = None;
        detail.bottomed = Vec::new();
        detail.mulligans = Vec::new();
    } else {
        detail.starting_hand = Some(hand.starting_hand.clone());
        detail.bottomed = hand.bottomed.clone();
        detail.mulligans = hand.mulliganed_hands.clone();
    }
    write_detail(tx, match_id, &detail)?;
    set_mulligans(tx, match_id, game, hand.mulligan_count)?;
    bump_revision(tx, match_id)?;
    Ok(ManualEditOutcome::Done)
}

/// The game's mulligan count (0.x kept it on the local player's pivot).
fn set_mulligans(
    tx: &Transaction,
    match_id: i64,
    game: u32,
    mulligans: u32,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE games SET local_mulligans = ?3 WHERE match_id = ?1 AND game_number = ?2",
        params![match_id, game, mulligans],
    )?;
    Ok(())
}

/// 0.x `UpdateManualGameSideboard`: the game's maindeck after the changes
/// (no changes: the registered deck again), once 0.x
/// `UpdateManualGameSideboardRequest`'s rules pass. A recorded hand the new
/// maindeck cannot hold is cleared, with its mulligans.
fn set_sideboard(
    tx: &Transaction,
    token: &str,
    game: u32,
    changes: &[SideboardChange],
) -> Result<ManualEditOutcome, StoreError> {
    let match_id = match manual_game(tx, token, game)? {
        Ok(id) => id,
        Err(refusal) => return Ok(refusal),
    };
    let registered = registered_deck(tx, token)?;
    let errors =
        manual_rules::validate_sideboard(game, changes, &registered.mains, &registered.sideboard);
    if !errors.is_empty() {
        return Ok(ManualEditOutcome::Invalid(errors));
    }
    let mut detail = stored_detail(tx, match_id, game)?;
    detail.postboard_main = if changes.is_empty() {
        None
    } else {
        Some(manual_rules::postboard_main(&registered.mains, changes))
    };
    let maindeck = manual_rules::game_maindeck(&registered.mains, detail.postboard_main.as_deref());
    let mut outcome = ManualEditOutcome::Done;
    let fits = match &detail.starting_hand {
        Some(starting) => {
            manual_rules::hand_fits(starting, &detail.bottomed, &detail.mulligans, &maindeck)
        }
        None => true,
    };
    if !fits {
        detail.starting_hand = None;
        detail.bottomed = Vec::new();
        detail.mulligans = Vec::new();
        outcome = ManualEditOutcome::HandCleared;
    }
    write_detail(tx, match_id, &detail)?;
    if !fits {
        set_mulligans(tx, match_id, game, 0)?;
    }
    bump_revision(tx, match_id)?;
    Ok(outcome)
}

/// 0.x `UpdateManualGameReveals`, once 0.x `UpdateManualGameRevealsRequest`'s
/// rules pass.
fn set_reveals(
    tx: &Transaction,
    token: &str,
    game: u32,
    cards: &[CardCount],
) -> Result<ManualEditOutcome, StoreError> {
    let match_id = match manual_game(tx, token, game)? {
        Ok(id) => id,
        Err(refusal) => return Ok(refusal),
    };
    let mut known: Vec<u32> = Vec::new();
    for card in cards {
        let exists: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM cards WHERE catalog_id = ?1)",
            [card.catalog_id],
            |row| row.get(0),
        )?;
        if exists {
            known.push(card.catalog_id);
        }
    }
    let errors = manual_rules::validate_reveals(cards, &|catalog_id| known.contains(&catalog_id));
    if !errors.is_empty() {
        return Ok(ManualEditOutcome::Invalid(errors));
    }
    let mut detail = stored_detail(tx, match_id, game)?;
    let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
    for card in cards {
        *counts.entry(card.catalog_id).or_insert(0) += card.quantity;
    }
    detail.reveals = counts
        .into_iter()
        .map(|(catalog_id, quantity)| CardCount {
            catalog_id,
            quantity,
        })
        .collect();
    write_detail(tx, match_id, &detail)?;
    bump_revision(tx, match_id)?;
    Ok(ManualEditOutcome::Done)
}

/// "Edit match" (edit a match spec section 4), in the caller's transaction:
/// the opponent, times, outcome and games written into the real columns, a
/// start or end left alone kept as stored, the match made `complete` (the
/// projection never finishes a corrected match, and every stat reads only
/// complete ones), the imported game counts cleared (the match now has
/// games), stamped `corrected_at` and `edited_at` (so a history or pulled
/// row is this device's to push) with a new revision, a log match's game
/// rows synced again (which stamps `games_synced_at`, so detection takes
/// it), and the league owners refolded. A match that is not listed
/// (missing, removed from stats or detached) is refused.
fn correct(
    tx: &Transaction,
    match_id: i64,
    correction: &MatchCorrection,
    tz: Tz,
) -> Result<ManualEditOutcome, StoreError> {
    type Found = (i64, Option<i64>, i64, Option<i64>, Option<String>);
    let found: Option<Found> = tx
        .query_row(
            "SELECT account_id, league_id, started_at, ended_at, game_structure
             FROM listed_matches WHERE id = ?1",
            [match_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((account_id, league_id, stored_start, stored_end, structure)) = found else {
        return Ok(ManualEditOutcome::NotFound);
    };
    // A Bo1 match (tracked or manual) has exactly one game.
    if structure.as_deref() == Some(BestOf::One.key()) && correction.games.len() != 1 {
        return Ok(invalid_field(GAMES_FIELD, BO1_ONE_GAME));
    }
    let started_at_ms = correction.started_at_ms.unwrap_or(stored_start);
    let ended_at_ms = correction.ended_at_ms.or(stored_end);
    let errors = manual_rules::correction_errors(
        correction.opponent_name.as_deref(),
        started_at_ms,
        ended_at_ms,
        &correction.games,
    );
    if !errors.is_empty() {
        return Ok(ManualEditOutcome::Invalid(errors));
    }
    let opponent_id = match &correction.opponent_name {
        Some(name) => match corrected_opponent(tx, match_id, name.trim(), correction.now_ms)? {
            Some(opponent_id) => Some(opponent_id),
            None => return Ok(invalid_field(OPPONENT_FIELD, ENDSTEP_OPPONENT_KEPT)),
        },
        None => None,
    };
    let wins = correction.games.iter().filter(|game| game.won).count();
    let losses = correction.games.len() - wins;
    tx.execute(
        "UPDATE matches SET opponent_id = ?2, started_at = ?3, ended_at = ?4, outcome = ?5,
             state = 'complete', games_won_imported = NULL, games_lost_imported = NULL,
             corrected_at = ?6, edited_at = ?6
         WHERE id = ?1",
        params![
            match_id,
            opponent_id,
            started_at_ms,
            ended_at_ms,
            manual_rules::outcome(wins, losses),
            correction.now_ms,
        ],
    )?;
    write_corrected_games(tx, match_id, started_at_ms, ended_at_ms, &correction.games)?;
    // A manual match's rank follows its start time.
    set_match_rank(tx, match_id)?;
    // A new revision for every source; for a manual row it also keeps a
    // later 0.x import from filling its games (`fill_manual_games`).
    bump_revision(tx, match_id)?;
    sync_corrected_games(tx, match_id, tz, correction.now_ms)?;
    // The league fold reads the result: the account's runs and the runs of
    // the match's league's owner (a 0.x run may have no account).
    let mut owners = vec![LeagueOwner::Account(account_id)];
    if let Some(league_id) = league_id {
        let league_account: Option<Option<i64>> = tx
            .query_row(
                "SELECT account_id FROM leagues WHERE id = ?1",
                [league_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(league_account) = league_account {
            let owner = league_account.map_or(LeagueOwner::Nobody, LeagueOwner::Account);
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
    }
    for owner in owners {
        sync_owner(tx, owner, tz, correction.now_ms)?;
    }
    Ok(ManualEditOutcome::Done)
}

/// The opponent row a correction names. MTGO: the opponent of that
/// username. Arena: the match's own opponent when the name is the one the
/// edit sheet showed (its shown name, else its username: a tracked opponent
/// is kept by its Arena id), else the named opponent `arena:name:<name>`.
/// Endstep: the match's own opponent when the name is its shown one, else
/// `None` (refused: an Endstep opponent is its user id).
fn corrected_opponent(
    tx: &Transaction,
    match_id: i64,
    name: &str,
    now_ms: i64,
) -> Result<Option<i64>, StoreError> {
    let (platform, current) = edit_platform_and_opponent(tx, match_id)?;
    let kept = match current {
        Some((opponent_id, shown)) if shown == name => Some(opponent_id),
        _ => None,
    };
    Ok(match platform {
        Platform::Mtgo => Some(registry_id(tx, "opponents", name, now_ms)?),
        Platform::Arena => match kept {
            Some(opponent_id) => Some(opponent_id),
            None => Some(named_arena_opponent(tx, name, now_ms)?),
        },
        Platform::Endstep => kept,
    })
}

/// The platform of listed match `match_id`'s account (MTGO when unknown)
/// and its opponent as the edit sheet shows it: its id and name (an Arena
/// opponent's shown name, else its username).
fn edit_platform_and_opponent(
    conn: &Connection,
    match_id: i64,
) -> Result<(Platform, Option<(i64, String)>), StoreError> {
    // The account's platform, then the opponent's id, username and shown
    // name.
    type Row = (
        Option<Platform>,
        Option<i64>,
        Option<String>,
        Option<String>,
    );
    let found: Option<Row> = conn
        .query_row(
            "SELECT a.platform, o.id, o.username, o.display_name FROM listed_matches m
             LEFT JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.id = ?1",
            [match_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((platform, opponent_id, username, display_name)) = found else {
        return Ok((Platform::Mtgo, None));
    };
    let platform = platform.unwrap_or(Platform::Mtgo);
    let shown = match platform {
        Platform::Mtgo => username,
        Platform::Arena | Platform::Endstep => display_name.or(username),
    };
    let current = match (opponent_id, shown) {
        (Some(opponent_id), Some(shown)) => Some((opponent_id, shown)),
        _ => None,
    };
    Ok((platform, current))
}

/// A corrected log match's game rows built again from its log (a game added
/// or removed), as the tracker's game sync would: now complete, the sync
/// stamps `games_synced_at`, so detection reads it. MTGO reads its log
/// events, Arena its stored game messages (keeping the play side and turns
/// entered); each sync leaves the other platform's matches and any other
/// source alone. A match whose events cannot be read (a corrupt archive)
/// keeps its rows, as the game sync's own skip does; a database error fails
/// the correction.
fn sync_corrected_games(
    tx: &Transaction,
    match_id: i64,
    tz: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    tx.execute_batch("SAVEPOINT correct_game_sync")?;
    let synced = crate::game_sync::sync_match_games(tx, match_id, tz, None)
        .and_then(|_| crate::arena_game_sync::sync_corrected_arena_match(tx, match_id, now_ms));
    match synced {
        Ok(_) => {}
        Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error)),
        Err(error) => {
            tracing::warn!(match_id, %error, "a corrected match's game rows were not rebuilt");
            tx.execute_batch("ROLLBACK TO correct_game_sync")?;
        }
    }
    tx.execute_batch("RELEASE correct_game_sync")?;
    Ok(())
}

/// Games 1 to n of a new manual match or a correction, upserted by game
/// number: an existing game keeps its id, cards and every other column, and
/// takes the result, play or draw and turns; a new game gets its slice of
/// the match (0.x `CreateManualMatch::createGames`: evenly spaced
/// whole-second slices, so every read that orders games by time sees them
/// in the order entered; mulligans start at none, as 0.x attached them). A
/// game above n goes, with its cards (they cascade).
fn write_corrected_games(
    tx: &Transaction,
    match_id: i64,
    started_at_ms: i64,
    ended_at_ms: Option<i64>,
    games: &[ManualGame],
) -> Result<(), StoreError> {
    let ended_at_ms = ended_at_ms.unwrap_or(started_at_ms);
    let times = manual_rules::game_times(started_at_ms, ended_at_ms, games.len());
    let mut stmt = tx.prepare_cached(
        "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason, on_play,
             local_mulligans, opp_mulligans, turn_count, started_at, ended_at)
         VALUES (?1, NULL, ?2, ?3, 'unknown', ?4, 0, 0, ?5, ?6, ?7)
         ON CONFLICT (match_id, game_number) DO UPDATE SET result = excluded.result,
             on_play = excluded.on_play, turn_count = excluded.turn_count",
    )?;
    for (index, (game, (started_at, ended_at))) in games.iter().zip(times).enumerate() {
        stmt.execute(params![
            match_id,
            index as i64 + 1,
            if game.won { "win" } else { "loss" },
            game.on_play,
            game.turns,
            started_at,
            ended_at,
        ])?;
    }
    tx.execute(
        "DELETE FROM games WHERE match_id = ?1 AND game_number > ?2",
        params![match_id, games.len() as i64],
    )?;
    Ok(())
}

/// One game as the edit sheet opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEditGame {
    pub game_number: u32,
    /// `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    pub on_play: Option<bool>,
    pub turns: Option<u32>,
}

/// What the edit sheet opens with (edit a match spec section 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEditView {
    /// The opponent as the sheet shows it: an Arena opponent's shown name,
    /// else its username.
    pub opponent_name: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// By game number. Empty for a history row known only by its game
    /// counts.
    pub games: Vec<MatchEditGame>,
    /// The player has fixed the match before (`corrected_at` is set).
    pub corrected: bool,
    /// The platform of the match's account (MTGO when unknown).
    pub platform: Platform,
}

/// The listed match `match_id` as the edit sheet opens it; `None` when it is
/// missing, removed from stats or detached.
pub fn match_edit_view(
    conn: &Connection,
    match_id: i64,
) -> Result<Option<MatchEditView>, StoreError> {
    let found = conn
        .query_row(
            "SELECT m.started_at, m.ended_at, m.corrected_at IS NOT NULL
             FROM listed_matches m
             WHERE m.id = ?1",
            [match_id],
            |row| {
                Ok(MatchEditView {
                    opponent_name: None,
                    started_at_ms: row.get(0)?,
                    ended_at_ms: row.get(1)?,
                    games: Vec::new(),
                    corrected: row.get(2)?,
                    platform: Platform::Mtgo,
                })
            },
        )
        .optional()?;
    let Some(mut view) = found else {
        return Ok(None);
    };
    let (platform, opponent) = edit_platform_and_opponent(conn, match_id)?;
    view.platform = platform;
    view.opponent_name = opponent.map(|(_, shown)| shown);
    let mut stmt = conn.prepare_cached(
        "SELECT game_number, result, on_play, turn_count FROM games
         WHERE match_id = ?1 ORDER BY game_number, id",
    )?;
    let rows = stmt.query_map([match_id], |row| {
        Ok(MatchEditGame {
            game_number: row.get(0)?,
            result: row.get(1)?,
            on_play: row.get(2)?,
            turns: row.get(3)?,
        })
    })?;
    for row in rows {
        view.games.push(row?);
    }
    Ok(Some(view))
}

/// The token of match row `match_id`, if there is one.
pub fn match_token(conn: &Connection, match_id: i64) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT token FROM matches WHERE id = ?1",
            [match_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Whether the match with this token was entered by hand.
pub fn is_manual(conn: &Connection, token: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1 AND source = 'manual')",
        [token],
        |row| row.get(0),
    )?)
}

/// The recorded detail of each game of a manual match that has any, in game
/// order.
pub fn manual_game_rows(
    conn: &Connection,
    token: &str,
) -> Result<Vec<ManualGameDetail>, StoreError> {
    let mut games: Vec<(i64, u32, Option<u32>)> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT g.id, g.game_number, g.local_mulligans FROM games g
             JOIN matches m ON m.id = g.match_id
             WHERE m.token = ?1 ORDER BY g.game_number",
        )?;
        let rows = stmt.query_map([token], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        for row in rows {
            games.push(row?);
        }
    }
    let mut details = Vec::new();
    for (game_id, game_number, mulligans) in games {
        let cards = crate::game_sync::read_game_cards(conn, game_id)?;
        let detail = ManualGameDetail::from_game_cards(game_number, &cards, mulligans.unwrap_or(0));
        if !detail.is_empty() {
            details.push(detail);
        }
    }
    Ok(details)
}

/// The platform of the manual match `token`'s account and its opponent's
/// shown name (an Arena opponent's `display_name`), for its live push.
/// MTGO and no name for a match that is not there.
pub fn manual_push_parts(
    conn: &Connection,
    token: &str,
) -> Result<(Platform, Option<String>), StoreError> {
    let found: Option<(Platform, Option<String>)> = conn
        .query_row(
            "SELECT a.platform, o.display_name FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.token = ?1",
            [token],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match found {
        Some((platform, display_name)) => (platform, display_name),
        None => (Platform::Mtgo, None),
    })
}

/// A stored manual match as the tracker pushes a match (`MatchUpdated`), with
/// its revision; `None` when there is no manual match with this token.
pub fn manual_view(conn: &Connection, token: &str) -> Result<Option<(MatchView, u64)>, StoreError> {
    match_view(conn, token, true)
}

/// Any stored match as the tracker pushes a match, from its row and game
/// rows (no log ids or end reasons), with its revision; `None` when there
/// is no match with this token. An Arena match shown again is pushed so.
pub fn stored_view(conn: &Connection, token: &str) -> Result<Option<(MatchView, u64)>, StoreError> {
    match_view(conn, token, false)
}

/// The stored match `token` as a view, of a manual match only when
/// `manual_only`.
fn match_view(
    conn: &Connection,
    token: &str,
    manual_only: bool,
) -> Result<Option<(MatchView, u64)>, StoreError> {
    let found = conn
        .query_row(
            "SELECT m.id, a.username, o.username, m.format, m.match_type, m.league_token,
                 m.game_structure, m.state, m.outcome, m.started_at, m.ended_at,
                 m.last_activity_at, m.revision
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.token = ?1 AND (?2 = 0 OR m.source = 'manual')",
            params![token, manual_only],
            |row| {
                let view = MatchView {
                    token: token.to_string(),
                    mtgo_match_id: None,
                    local_username: row.get(1)?,
                    opponent: row.get(2)?,
                    format: row.get(3)?,
                    match_type: row.get(4)?,
                    tournament_event_id: None,
                    tournament_round: None,
                    league_token: row.get(5)?,
                    game_structure: row.get(6)?,
                    state: state_from_str(&row.get::<_, String>(7)?)
                        .unwrap_or(MatchState::Complete),
                    resolution: None,
                    outcome: outcome_from_str(&row.get::<_, String>(8)?)
                        .unwrap_or(Outcome::Unknown),
                    tentative_outcome: None,
                    wins: 0,
                    losses: 0,
                    started_at_ms: row.get(9)?,
                    ended_at_ms: row.get(10)?,
                    last_activity_ms: row.get(11)?,
                    games: Vec::new(),
                };
                Ok((row.get::<_, i64>(0)?, view, row.get::<_, i64>(12)?))
            },
        )
        .optional()?;
    let Some((match_id, mut view, revision)) = found else {
        return Ok(None);
    };
    let mut stmt = conn.prepare_cached(
        "SELECT game_number, result, on_play, local_mulligans, opp_mulligans, turn_count,
             started_at, ended_at
         FROM games WHERE match_id = ?1 ORDER BY game_number, id",
    )?;
    let rows = stmt.query_map([match_id], |row| {
        Ok(GameView {
            mtgo_game_id: None,
            game_number: row.get(0)?,
            result: game_result_from_str(&row.get::<_, String>(1)?).unwrap_or(GameResult::Unknown),
            end_reason: EndReason::Unknown,
            on_play: row.get(2)?,
            local_mulligans: row.get(3)?,
            opp_mulligans: row.get(4)?,
            local_die_roll: None,
            opp_die_roll: None,
            turn_count: row.get(5)?,
            started_at_ms: row.get(6)?,
            ended_at_ms: row.get(7)?,
        })
    })?;
    for row in rows {
        let game = row?;
        match game.result {
            GameResult::Win => view.wins += 1,
            GameResult::Loss => view.losses += 1,
            GameResult::Unknown => {}
        }
        view.games.push(game);
    }
    Ok(Some((view, revision as u64)))
}

/// A deck version's copies by catalog id, maindeck and sideboard apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisteredDeck {
    pub mains: BTreeMap<u32, u32>,
    pub sideboard: BTreeMap<u32, u32>,
}

/// The registered deck of a manual match (its linked deck version), split
/// into maindeck and sideboard copies by catalog id (0.x
/// `ResolveRegisteredDeckQuantities`). Empty without a link.
pub fn registered_deck(conn: &Connection, token: &str) -> Result<RegisteredDeck, StoreError> {
    let signature: Option<String> = conn
        .query_row(
            "SELECT v.signature FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE m.token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    let mut deck = RegisteredDeck::default();
    for card in decode_signature(signature.as_deref().unwrap_or("")) {
        let side = if card.sideboard {
            &mut deck.sideboard
        } else {
            &mut deck.mains
        };
        *side.entry(card.catalog_id).or_insert(0) += card.quantity;
    }
    Ok(deck)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::*;
    use decks::{DeckCard, signature};
    use fixture_tools::TestDir;
    use mtgo_match::{GameResult, MatchState, Outcome};

    /// 0.x `createManualMatchFixture`'s deck: 4 Lightning Bolt (4001), 4
    /// Tarmogoyf (4002) and 20 Mountain (4004) main, 3 Rest in Peace (4003)
    /// in the sideboard.
    fn fixture_signature() -> String {
        let card = |catalog_id, quantity, sideboard| DeckCard {
            catalog_id,
            quantity,
            sideboard,
        };
        signature(&[
            card(4001, 4, false),
            card(4002, 4, false),
            card(4004, 20, false),
            card(4003, 3, true),
        ])
    }

    /// Qlocal (account 1, the only one) with deck 1 (versions 10, older, and
    /// 11, latest, the fixture's list) and deck 2 with no version, and the
    /// fixture's four cards.
    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, username, first_seen_at, last_login_at)
             VALUES (1, 'Qlocal', 0, 9);
             INSERT INTO log_files (path, identity_hash, head_hash, head_len, day_anchor,
                 timezone, local_username, clock_date, first_seen_at, last_seen_at)
             VALUES ('p', '0', '0', 0, '2026-09-01', 'UTC', 'Qlocal', '2026-09-01', 0, 9);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (1, 'Q-deck', 1, 'Qdeck', 'CMODERN', 0, 0, 'file'),
                    (2, 'Q-bare', 1, 'Qbare', 'CMODERN', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (10, 1, 'Q-older', 1), (11, 1, '{}', 2);
             INSERT INTO cards (catalog_id, name)
             VALUES (4001, 'Lightning Bolt'), (4002, 'Tarmogoyf'), (4003, 'Rest in Peace'),
                    (4004, 'Mountain');",
            fixture_signature()
        ))
        .expect("deck");
        (dir, conn)
    }

    fn add_other_account(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (2, 'Qother', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (3, 'Q-foreign', 2, 'Qforeign', 'CMODERN', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (30, 3, 'Q-foreign', 1);",
        )
        .expect("another account");
    }

    fn game(won: bool, on_play: bool, turns: Option<u32>) -> ManualGame {
        ManualGame {
            won,
            on_play,
            turns,
        }
    }

    /// 2026-09-01 20:00 UTC.
    const STARTED: i64 = 1_788_292_800_000;

    /// 0.x `manualMatchPayload`: Qkai, 20:00 to 20:45, won, lost, won.
    fn payload() -> ManualMatch {
        ManualMatch {
            deck_id: 1,
            opponent: "Qkai".to_string(),
            started_at_ms: STARTED,
            ended_at_ms: STARTED + 45 * 60_000,
            games: vec![
                game(true, true, None),
                game(false, false, None),
                game(true, true, None),
            ],
            placement: ManualPlacement::None,
            opponent_archetype_id: None,
            ranked: false,
            best_of: BestOf::Three,
            rank_gap: None,
            now_ms: 9_000,
        }
    }

    fn create(conn: &mut Connection, manual: ManualMatch) -> String {
        match edit_manual(conn, &ManualEdit::Create(manual), chrono_tz::UTC).expect("edit") {
            ManualEditOutcome::Created { token } => token,
            other => panic!("not created: {other:?}"),
        }
    }

    fn one(conn: &Connection, token: &str, column: &str) -> rusqlite::types::Value {
        conn.query_row(
            &format!("SELECT {column} FROM matches WHERE token = ?1"),
            [token],
            |row| row.get(0),
        )
        .expect(column)
    }

    fn text(conn: &Connection, token: &str, column: &str) -> String {
        match one(conn, token, column) {
            rusqlite::types::Value::Text(text) => text,
            other => panic!("{column}: {other:?}"),
        }
    }

    fn int(conn: &Connection, token: &str, column: &str) -> i64 {
        match one(conn, token, column) {
            rusqlite::types::Value::Integer(value) => value,
            other => panic!("{column}: {other:?}"),
        }
    }

    /// `(game_number, result, on_play, turn_count, started_at, ended_at,
    /// local_mulligans)` of each game of `token`.
    #[allow(clippy::type_complexity)]
    fn games_of(
        conn: &Connection,
        token: &str,
    ) -> Vec<(i64, String, Option<i64>, Option<i64>, i64, i64, Option<i64>)> {
        let mut stmt = conn
            .prepare(
                "SELECT g.game_number, g.result, g.on_play, g.turn_count, g.started_at,
                     g.ended_at, g.local_mulligans
                 FROM games g JOIN matches m ON m.id = g.match_id
                 WHERE m.token = ?1 ORDER BY g.game_number",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([token], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    fn linked_version(conn: &Connection, token: &str) -> i64 {
        conn.query_row(
            "SELECT deck_version_id FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .expect("a deck link")
    }

    #[test]
    fn a_manual_match_is_a_complete_row_with_its_games_and_deck_link() {
        let (_dir, mut conn) = db("manual-create");
        let token = create(&mut conn, payload());

        // A fresh UUID (version 4 layout).
        assert_eq!(token.len(), 36, "{token}");
        let parts: Vec<&str> = token.split('-').collect();
        let lengths: Vec<usize> = parts.iter().map(|part| part.len()).collect();
        assert_eq!(lengths, vec![8, 4, 4, 4, 12], "{token}");
        assert!(parts[2].starts_with('4'), "{token}");
        assert!(
            token
                .chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{token}"
        );

        assert_eq!(text(&conn, &token, "source"), "manual");
        assert_eq!(
            one(&conn, &token, "mtgo_match_id"),
            rusqlite::types::Value::Null
        );
        assert_eq!(int(&conn, &token, "account_id"), 1, "the deck's account");
        assert_eq!(
            text(
                &conn,
                &token,
                "(SELECT username FROM opponents WHERE id = opponent_id)"
            ),
            "Qkai"
        );
        assert_eq!(text(&conn, &token, "format"), "CMODERN", "the deck's");
        assert_eq!(text(&conn, &token, "match_type"), "Constructed");
        assert_eq!(text(&conn, &token, "state"), "complete");
        assert_eq!(text(&conn, &token, "outcome"), "win");
        assert_eq!(int(&conn, &token, "started_at"), STARTED);
        assert_eq!(int(&conn, &token, "ended_at"), STARTED + 45 * 60_000);
        assert_eq!(
            int(&conn, &token, "last_activity_at"),
            STARTED + 45 * 60_000
        );
        assert_eq!(int(&conn, &token, "needs_attention"), 0);
        assert_eq!(int(&conn, &token, "revision"), 1);
        assert_eq!(
            one(&conn, &token, "league_token"),
            rusqlite::types::Value::Null
        );
        assert_eq!(linked_version(&conn, &token), 11, "the latest version");

        let minute = 60_000;
        assert_eq!(
            games_of(&conn, &token),
            vec![
                (
                    1,
                    "win".to_string(),
                    Some(1),
                    None,
                    STARTED,
                    STARTED + 15 * minute,
                    Some(0)
                ),
                (
                    2,
                    "loss".to_string(),
                    Some(0),
                    None,
                    STARTED + 15 * minute,
                    STARTED + 30 * minute,
                    Some(0)
                ),
                (
                    3,
                    "win".to_string(),
                    Some(1),
                    None,
                    STARTED + 30 * minute,
                    STARTED + 45 * minute,
                    Some(0)
                ),
            ]
        );
        assert!(is_manual(&conn, &token).expect("manual"));
        assert_eq!(
            manual_game_rows(&conn, &token).expect("details"),
            Vec::new()
        );
    }

    /// `(is_opponent, archetype_id, confidence, manual)` of the match's
    /// archetype rows, and its detection marker.
    type Seat = (i64, i64, f64, i64);

    fn seats(conn: &Connection, token: &str) -> (Vec<Seat>, Option<i64>) {
        let mut stmt = conn
            .prepare(
                "SELECT x.is_opponent, x.archetype_id, x.confidence, x.manual
                 FROM match_archetypes x JOIN matches m ON m.id = x.match_id
                 WHERE m.token = ?1 ORDER BY x.is_opponent",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([token], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        let marker = conn
            .query_row(
                "SELECT archetypes_detected_at FROM matches WHERE token = ?1",
                [token],
                |row| row.get(0),
            )
            .expect("marker");
        (rows, marker)
    }

    fn archetype(conn: &Connection, uuid: &str, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES (?1, ?2, 'modern', 'manual', 0, 0)",
            params![uuid, name],
        )
        .expect("archetype");
        conn.last_insert_rowid()
    }

    #[test]
    fn a_manual_match_takes_the_opponent_archetype_and_the_decks() {
        // 0.x StoreManualMatchTest "creates a complete manual match with
        // games, players and archetypes": the opponent seat is hand-set and
        // the player seat is the deck's archetype as an auto row (0.x
        // `manual => false`, spec section 8), both in the manual edit's
        // transaction (decision C7), and the match is marked detected.
        let (_dir, mut conn) = db("manual-create-archetypes");
        let deck_archetype = archetype(&conn, "q-deck-arch", "Qdeck arch");
        let opponent = archetype(&conn, "q-opp-arch", "Qopp arch");
        conn.execute(
            "UPDATE decks SET archetype_id = ?1 WHERE id = 1",
            [deck_archetype],
        )
        .expect("deck archetype");
        let token = create(
            &mut conn,
            ManualMatch {
                opponent_archetype_id: Some(opponent),
                ..payload()
            },
        );
        let (rows, marker) = seats(&conn, &token);
        assert_eq!(
            rows,
            vec![(0, deck_archetype, 1.0, 0), (1, opponent, 1.0, 1)]
        );
        assert_eq!(marker, Some(9_000));
    }

    #[test]
    fn a_manual_matchs_player_seat_follows_a_deck_archetype_change() {
        // Spec section 8: a deck archetype change clears the marker of the
        // deck's matches whose player seat is not hand-set, a manual match's
        // included; its hand-set opponent row stays.
        let (_dir, mut conn) = db("manual-player-seat-follows-deck");
        let first = archetype(&conn, "q-first", "Qfirst");
        let second = archetype(&conn, "q-second", "Qsecond");
        let opponent = archetype(&conn, "q-opp", "Qopp");
        conn.execute("UPDATE decks SET archetype_id = ?1 WHERE id = 1", [first])
            .expect("deck archetype");
        let token = create(
            &mut conn,
            ManualMatch {
                opponent_archetype_id: Some(opponent),
                ..payload()
            },
        );
        let outcome = crate::archetype_edit::edit_archetypes(
            &mut conn,
            &crate::archetype_edit::ArchetypeEdit::SetDeckArchetype {
                deck_ids: vec![1],
                archetype_id: Some(second),
            },
        )
        .expect("edit");
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches WHERE token = ?1", [&token], |row| {
                row.get(0)
            })
            .expect("id");
        assert_eq!(
            outcome,
            crate::archetype_edit::ArchetypeEditOutcome::Done {
                matches: vec![match_id],
                decks: vec![1],
            }
        );
        let (rows, marker) = seats(&conn, &token);
        assert_eq!(marker, None, "asks for detection again");
        assert_eq!(
            rows[1],
            (1, opponent, 1.0, 1),
            "the opponent stays hand-set"
        );
    }

    #[test]
    fn a_manual_match_without_an_opponent_archetype_has_no_opponent_row() {
        // 0.x "records a draw for a 1-1 match and skips the opponent
        // archetype when none is given"; a deck without an archetype gives
        // no player row either.
        let (_dir, mut conn) = db("manual-create-no-archetype");
        let token = create(&mut conn, payload());
        let (rows, marker) = seats(&conn, &token);
        assert!(rows.is_empty());
        assert_eq!(marker, Some(9_000), "nothing for detection to fill");
    }

    #[test]
    fn a_manual_match_follows_one_merge_and_refuses_an_unknown_archetype() {
        // 0.x CreateManualMatch resolves both seats through one merge hop;
        // StoreManualMatchRequest `exists:archetypes,id`.
        let (_dir, mut conn) = db("manual-create-merged");
        let parent = archetype(&conn, "q-parent", "Qparent");
        let merged = archetype(&conn, "q-merged", "Qmerged");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id = ?2",
            params![parent, merged],
        )
        .expect("merge");
        let token = create(
            &mut conn,
            ManualMatch {
                opponent_archetype_id: Some(merged),
                ..payload()
            },
        );
        assert_eq!(seats(&conn, &token).0, vec![(1, parent, 1.0, 1)]);

        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM matches", [], |row| row.get(0))
            .expect("count");
        let refused = edit_manual(
            &mut conn,
            &ManualEdit::Create(ManualMatch {
                opponent_archetype_id: Some(99_999),
                ..payload()
            }),
            chrono_tz::UTC,
        )
        .expect("edit");
        assert_eq!(
            invalid(refused),
            pair("archetype_id", "The selected archetype id is invalid.")
        );
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM matches", [], |row| row.get(0))
            .expect("count");
        assert_eq!(before, after, "nothing written");
    }

    #[test]
    fn the_quick_picks_are_the_opponent_archetypes_first_list() {
        // 0.x ManualMatchDataTest "includes the opponent archetype decklist
        // in manualEditing" and "builds manualEditing ... archetypeDecklist
        // null": list 1 of a downloaded archetype or the first variant of a
        // manual one, maindeck first then by name, null without a seat or a
        // list.
        let (_dir, mut conn) = db("manual-quick-picks");
        let token = create(&mut conn, payload());
        assert_eq!(archetype_decklist(&conn, &token).expect("none"), None);

        let opponent = archetype(&conn, "q-opp", "Qopp");
        conn.execute_batch(&format!(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES ({opponent}, 2, 4001, 4, 0),
                    ({opponent}, 1, 4003, 2, 1),
                    ({opponent}, 1, 4002, 4, 0),
                    ({opponent}, 1, 4001, 3, 0),
                    ({opponent}, 1, 77777, 1, 0);"
        ))
        .expect("lists");
        let picked = create(
            &mut conn,
            ManualMatch {
                opponent_archetype_id: Some(opponent),
                ..payload()
            },
        );
        let list = archetype_decklist(&conn, &picked)
            .expect("read")
            .expect("a list");
        let shown: Vec<(u32, Option<&str>, u32, bool)> = list
            .iter()
            .map(|card| {
                (
                    card.catalog_id,
                    card.name.as_deref(),
                    card.quantity,
                    card.sideboard,
                )
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                (4001, Some("Lightning Bolt"), 3, false),
                (4002, Some("Tarmogoyf"), 4, false),
                (77777, None, 1, false),
                (4003, Some("Rest in Peace"), 2, true),
            ]
        );
    }

    #[test]
    fn a_one_one_match_is_a_draw_and_turn_counts_are_kept() {
        let (_dir, mut conn) = db("manual-create-draw");
        let mut manual = payload();
        manual.games = vec![game(true, true, Some(8)), game(false, false, None)];
        let token = create(&mut conn, manual);
        assert_eq!(text(&conn, &token, "outcome"), "draw");
        let turns: Vec<Option<i64>> = games_of(&conn, &token)
            .into_iter()
            .map(|game| game.3)
            .collect();
        assert_eq!(turns, vec![Some(8), None]);
    }

    #[test]
    fn the_same_payload_twice_makes_two_matches_and_one_opponent() {
        let (_dir, mut conn) = db("manual-create-twice");
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qkai', 0)",
            [],
        )
        .expect("an existing opponent");
        let first = create(&mut conn, payload());
        let second = create(&mut conn, payload());
        assert_ne!(first, second);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE source = 'manual'"
            ),
            2
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM opponents WHERE username = 'Qkai'"
            ),
            1,
            "the existing player is reused"
        );
    }

    #[test]
    fn a_match_in_a_run_carries_its_league_token_and_the_runs_version() {
        let (_dir, mut conn) = db("manual-create-run");
        let mut manual = payload();
        manual.placement = ManualPlacement::Run {
            league_token: "Q-league".to_string(),
            game_structure: Some("Modern".to_string()),
            deck_version_id: Some(10),
            adopt_league_id: None,
        };
        let token = create(&mut conn, manual);
        assert_eq!(text(&conn, &token, "league_token"), "Q-league");
        assert_eq!(text(&conn, &token, "game_structure"), "Modern");
        assert_eq!(text(&conn, &token, "match_type"), "League");
        assert_eq!(linked_version(&conn, &token), 10, "the run's version");

        let mut manual = payload();
        manual.placement = ManualPlacement::Run {
            league_token: "Q-league".to_string(),
            game_structure: None,
            deck_version_id: None,
            adopt_league_id: None,
        };
        let token = create(&mut conn, manual);
        assert_eq!(
            linked_version(&conn, &token),
            11,
            "a run with no version: the latest"
        );
    }

    #[test]
    fn a_match_in_a_manual_league_is_linked_to_it() {
        let (_dir, mut conn) = db("manual-create-manual-league");
        insert_manual_league(&conn, 4, Some(1), Some(1), "Q manual", 0, 5);
        let mut manual = payload();
        manual.placement = ManualPlacement::ManualLeague { id: 4 };
        let token = create(&mut conn, manual);
        let league: (i64, bool) = conn
            .query_row(
                "SELECT league_id, league_linked FROM matches WHERE token = ?1",
                [&token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("linked");
        assert_eq!(league, (4, true));
        assert_eq!(text(&conn, &token, "match_type"), "League");
        assert_eq!(
            one(&conn, &token, "league_token"),
            rusqlite::types::Value::Null
        );
        assert_eq!(linked_version(&conn, &token), 11);
    }

    /// 0.x linked the match to the league's version. v1 shows a manual
    /// league with the deck's version when it was created
    /// (`app::leagues::version_at`), so the match takes that one, not a
    /// newer one saved since.
    #[test]
    fn a_match_in_a_manual_league_takes_the_leagues_version() {
        let (_dir, mut conn) = db("manual-create-manual-league-version");
        // Created when version 10 (saved at 1) was the latest.
        insert_manual_league(&conn, 4, Some(1), Some(1), "Q manual", 0, 1);
        insert_manual_league(&conn, 5, Some(1), Some(1), "Q before any version", 0, 0);
        insert_manual_league(&conn, 6, Some(1), None, "Q no deck", 0, 0);
        conn.execute_batch(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (12, 1, 'Q-newer', 3);",
        )
        .expect("a newer version");
        let token = create(&mut conn, into_league(4));
        assert_eq!(linked_version(&conn, &token), 10);
        let first = create(&mut conn, into_league(5));
        assert_eq!(linked_version(&conn, &first), 10, "else the first version");
        let no_deck = create(&mut conn, into_league(6));
        assert_eq!(linked_version(&conn, &no_deck), 12, "else the latest");
    }

    #[test]
    fn a_league_that_is_missing_full_or_of_another_deck_is_refused() {
        let (_dir, mut conn) = db("manual-create-league-refused");
        add_other_account(&conn);
        insert_manual_league(&conn, 4, Some(1), Some(1), "Q full", 0, 0);
        insert_manual_league(&conn, 5, Some(1), Some(2), "Q other deck", 0, 0);
        insert_manual_league(&conn, 6, None, None, "Q unknown deck", 0, 0);
        let into = |placement: ManualPlacement| {
            let mut manual = payload();
            manual.placement = placement;
            ManualEdit::Create(manual)
        };
        // League 4 gets five complete matches; a hidden one does not count.
        for _ in 0..4 {
            let outcome = edit(&mut conn, into(ManualPlacement::ManualLeague { id: 4 }));
            assert!(matches!(outcome, ManualEditOutcome::Created { .. }));
        }
        let hidden = create(&mut conn, into_league(4));
        hide(&conn, &hidden);
        create(&mut conn, into_league(4));
        assert_eq!(
            edit(&mut conn, into(ManualPlacement::ManualLeague { id: 4 })),
            ManualEditOutcome::LeagueFull
        );
        assert_eq!(
            edit(&mut conn, into(ManualPlacement::ManualLeague { id: 5 })),
            ManualEditOutcome::WrongLeagueDeck
        );
        assert_eq!(
            edit(&mut conn, into(ManualPlacement::ManualLeague { id: 99 })),
            ManualEditOutcome::InvalidLeague
        );
        assert!(matches!(
            edit(&mut conn, into(ManualPlacement::ManualLeague { id: 6 })),
            ManualEditOutcome::Created { .. }
        ));
        assert_eq!(
            edit(
                &mut conn,
                into(ManualPlacement::Run {
                    league_token: "Q-league".to_string(),
                    game_structure: None,
                    deck_version_id: Some(30),
                    adopt_league_id: None,
                })
            ),
            ManualEditOutcome::WrongLeagueDeck,
            "a run played with another deck's version"
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM matches"),
            7,
            "refusals wrote nothing"
        );
        assert_eq!(
            ManualEditOutcome::LeagueFull.refusal().as_deref(),
            Some("This league already has a full run of matches.")
        );
        assert_eq!(
            ManualEditOutcome::WrongLeagueDeck.refusal().as_deref(),
            Some("This league was played with a different deck.")
        );
    }

    #[test]
    fn a_removed_manual_league_takes_no_new_match() {
        let (_dir, mut conn) = db("manual-create-removed-league");
        insert_manual_league(&conn, 4, Some(1), Some(1), "Q removed", 0, 0);
        conn.execute("UPDATE leagues SET removed_at = 5 WHERE id = 4", [])
            .expect("removed");
        let before = number(&conn, "SELECT COUNT(*) FROM matches");
        let mut manual = payload();
        manual.placement = ManualPlacement::ManualLeague { id: 4 };
        assert_eq!(
            edit(&mut conn, ManualEdit::Create(manual)),
            ManualEditOutcome::InvalidLeague
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), before);
    }

    #[test]
    fn a_run_that_filled_up_since_the_check_refuses_inside_the_write() {
        let (_dir, mut conn) = db("manual-create-run-full");
        let into_run = || {
            let mut manual = payload();
            manual.placement = ManualPlacement::Run {
                league_token: "Q-run".to_string(),
                game_structure: None,
                deck_version_id: None,
                adopt_league_id: None,
            };
            manual
        };
        for _ in 0..5 {
            create(&mut conn, into_run());
        }
        assert_eq!(
            edit(&mut conn, ManualEdit::Create(into_run())),
            ManualEditOutcome::LeagueFull,
            "a sixth match in one run"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 5);
    }

    /// The in-write room count copies the fold's membership by hand: log and
    /// manual matches in the run's live and finished states count; imported
    /// and hidden ones do not.
    #[test]
    fn log_matches_in_the_run_use_up_its_room_inside_the_write() {
        let (_dir, mut conn) = db("manual-create-run-log-room");
        let log_match = |token: &str, source: &str, state: &str, hidden: bool| {
            conn.execute(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version, league_token, hidden_at)
                 VALUES (?1, 1, ?2, ?3, 'win', 1000, 1000, 1, 1, 'Q-run', ?4)",
                params![token, source, state, hidden.then_some(5_i64)],
            )
            .expect("log match");
        };
        log_match("log-live", "log", "in_progress", false);
        log_match("log-ended", "log", "ended", false);
        log_match("log-abandoned", "log", "abandoned", false);
        log_match("log-hidden", "log", "complete", true);
        log_match("imported", "imported", "complete", false);
        let into_run = || {
            let mut manual = payload();
            manual.placement = ManualPlacement::Run {
                league_token: "Q-run".to_string(),
                game_structure: None,
                deck_version_id: None,
                adopt_league_id: None,
            };
            manual
        };
        create(&mut conn, into_run());
        create(&mut conn, into_run());
        assert_eq!(
            edit(&mut conn, ManualEdit::Create(into_run())),
            ManualEditOutcome::LeagueFull,
            "three log and two manual matches fill the run"
        );
    }

    fn into_league(id: i64) -> ManualMatch {
        let mut manual = payload();
        manual.placement = ManualPlacement::ManualLeague { id };
        manual
    }

    #[test]
    fn a_deck_that_is_missing_foreign_or_without_a_version_is_refused() {
        let (_dir, mut conn) = db("manual-create-refused");
        add_other_account(&conn);
        for (deck_id, expected) in [
            (99, ManualEditOutcome::InvalidDeck),
            (3, ManualEditOutcome::InvalidDeck),
            (2, ManualEditOutcome::NoDeckVersion),
        ] {
            let mut manual = payload();
            manual.deck_id = deck_id;
            assert_eq!(
                edit_manual(&mut conn, &ManualEdit::Create(manual), chrono_tz::UTC).expect("edit"),
                expected,
                "deck {deck_id}"
            );
        }
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 0);
        assert_eq!(
            ManualEditOutcome::NoDeckVersion.refusal().as_deref(),
            Some("This deck has no saved version to attach the match to.")
        );
        assert_eq!(
            ManualEditOutcome::InvalidDeck.refusal().as_deref(),
            Some("The selected deck id is invalid.")
        );
    }

    fn set_hand(
        token: &str,
        game: u32,
        kept: &[u32],
        bottomed: &[u32],
        mulligans: &[&[u32]],
    ) -> ManualEdit {
        ManualEdit::SetHand {
            token: token.to_string(),
            game,
            starting: kept.to_vec(),
            bottomed: bottomed.to_vec(),
            mulligan_count: mulligans.len() as u32,
            mulligans: mulligans.iter().map(|hand| hand.to_vec()).collect(),
        }
    }

    fn change(catalog_id: u32, quantity: u32, direction: Direction) -> SideboardChange {
        SideboardChange {
            catalog_id,
            quantity,
            direction,
        }
    }

    fn count(catalog_id: u32, quantity: u32) -> CardCount {
        CardCount {
            catalog_id,
            quantity,
        }
    }

    fn edit(conn: &mut Connection, edit: ManualEdit) -> ManualEditOutcome {
        edit_manual(conn, &edit, chrono_tz::UTC).expect("edit")
    }

    #[test]
    fn a_hand_is_stored_with_its_mulligans_and_cleared_by_an_empty_one() {
        let (_dir, mut conn) = db("manual-hand");
        let token = create(&mut conn, payload());
        let hand = set_hand(
            &token,
            1,
            &[4001, 4001, 4002, 4004, 4004, 4004],
            &[4002],
            &[&[4004, 4004, 4004, 4004, 4004, 4004, 4001]],
        );
        assert_eq!(edit(&mut conn, hand), ManualEditOutcome::Done);
        assert_eq!(
            manual_game_rows(&conn, &token).expect("details"),
            vec![ManualGameDetail {
                game_number: 1,
                starting_hand: Some(vec![4001, 4001, 4002, 4004, 4004, 4004]),
                bottomed: vec![4002],
                // Per-card rows: the hand sent back reads in catalog order.
                mulligans: vec![vec![4001, 4004, 4004, 4004, 4004, 4004, 4004]],
                postboard_main: None,
                reveals: Vec::new(),
            }]
        );
        assert_eq!(games_of(&conn, &token)[0].6, Some(1), "one mulligan");
        assert_eq!(
            int(&conn, &token, "revision"),
            2,
            "an edit is a new revision"
        );

        assert_eq!(
            edit(&mut conn, set_hand(&token, 1, &[], &[], &[])),
            ManualEditOutcome::Done
        );
        let details = manual_game_rows(&conn, &token).expect("details");
        assert!(
            details.iter().all(|game| game.starting_hand.is_none()
                && game.bottomed.is_empty()
                && game.mulligans.is_empty()),
            "{details:?}"
        );
        assert_eq!(games_of(&conn, &token)[0].6, Some(0), "cleared to none");
    }

    #[test]
    fn manual_hand_sideboard_and_reveals_round_trip_through_rows() {
        let (_dir, mut conn) = db("manual-round-trip");
        let token = create(&mut conn, payload());
        let sideboard = ManualEdit::SetSideboard {
            token: token.clone(),
            game: 2,
            changes: vec![
                change(4001, 1, Direction::Out),
                change(4003, 1, Direction::In),
            ],
        };
        assert_eq!(edit(&mut conn, sideboard), ManualEditOutcome::Done);
        let hand = set_hand(
            &token,
            2,
            &[4001, 4003, 4004, 4004, 4004, 4004],
            &[4002],
            &[&[]],
        );
        assert_eq!(edit(&mut conn, hand), ManualEditOutcome::Done);
        let reveals = ManualEdit::SetReveals {
            token: token.clone(),
            game: 2,
            cards: vec![count(4004, 2), count(4002, 2)],
        };
        assert_eq!(edit(&mut conn, reveals), ManualEditOutcome::Done);

        let expected = ManualGameDetail {
            game_number: 2,
            starting_hand: Some(vec![4001, 4003, 4004, 4004, 4004, 4004]),
            bottomed: vec![4002],
            mulligans: vec![Vec::new()],
            postboard_main: Some(vec![
                count(4001, 3),
                count(4002, 4),
                count(4003, 1),
                count(4004, 20),
            ]),
            reveals: vec![count(4002, 2), count(4004, 2)],
        };
        assert_eq!(
            manual_game_rows(&conn, &token).expect("details"),
            vec![expected.clone()]
        );
        // The rows the page and the card stats read.
        let game = number(&conn, "SELECT id FROM games WHERE game_number = 2");
        let cards = crate::game_sync::read_game_cards(&conn, game).expect("rows");
        assert_eq!(cards.opening_hand_size, Some(7));
        assert_eq!(cards.starting_hand_size, Some(6));
        assert_eq!(
            (cards.opp_opening_hand_size, cards.opp_starting_hand_size),
            (None, None)
        );
        let bottomed = cards.row(Side::Local, 4002).expect("bottomed");
        assert_eq!(
            (
                bottomed.main_qty,
                bottomed.opening_hand,
                bottomed.bottomed,
                bottomed.starting_hand
            ),
            (4, 1, 1, 0)
        );
        assert_eq!(cards.row(Side::Opponent, 4004).expect("seen").seen, 2);
        assert!(
            cards.mulligans.is_empty(),
            "the hand sent back was not remembered"
        );
        assert_eq!(
            ManualGameDetail::from_game_cards(2, &expected.game_cards(), 1),
            expected
        );
    }

    #[test]
    fn a_sideboard_edit_stores_the_postboard_maindeck_and_no_changes_reset_it() {
        let (_dir, mut conn) = db("manual-sideboard");
        let token = create(&mut conn, payload());
        let sideboard = ManualEdit::SetSideboard {
            token: token.clone(),
            game: 2,
            changes: vec![
                change(4001, 2, Direction::Out),
                change(4003, 2, Direction::In),
            ],
        };
        assert_eq!(edit(&mut conn, sideboard), ManualEditOutcome::Done);
        let details = manual_game_rows(&conn, &token).expect("details");
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].game_number, 2);
        assert_eq!(
            details[0].postboard_main,
            Some(vec![
                count(4001, 2),
                count(4002, 4),
                count(4003, 2),
                count(4004, 20)
            ])
        );

        let reset = ManualEdit::SetSideboard {
            token: token.clone(),
            game: 2,
            changes: Vec::new(),
        };
        assert_eq!(edit(&mut conn, reset), ManualEditOutcome::Done);
        assert!(
            manual_game_rows(&conn, &token)
                .expect("details")
                .iter()
                .all(|detail| detail.postboard_main.is_none())
        );
    }

    #[test]
    fn reveals_are_stored_per_card_and_cleared_by_an_empty_list() {
        let (_dir, mut conn) = db("manual-reveals");
        let token = create(&mut conn, payload());
        let reveals = ManualEdit::SetReveals {
            token: token.clone(),
            game: 1,
            cards: vec![count(4004, 5), count(4002, 2)],
        };
        assert_eq!(edit(&mut conn, reveals), ManualEditOutcome::Done);
        // Per-card rows: the reveals read in catalog order.
        assert_eq!(
            manual_game_rows(&conn, &token).expect("details")[0].reveals,
            vec![count(4002, 2), count(4004, 5)]
        );
        let clear = ManualEdit::SetReveals {
            token: token.clone(),
            game: 1,
            cards: Vec::new(),
        };
        assert_eq!(edit(&mut conn, clear), ManualEditOutcome::Done);
        assert!(
            manual_game_rows(&conn, &token)
                .expect("details")
                .iter()
                .all(|detail| detail.reveals.is_empty())
        );
    }

    #[test]
    fn edits_need_a_manual_match_that_is_not_hidden_and_the_game() {
        let (_dir, mut conn) = db("manual-edit-refused");
        let token = create(&mut conn, payload());
        crate::write::commit(
            &mut conn,
            &crate::write::WriteBatch {
                matches: vec![crate::write::MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..crate::write::WriteBatch::default()
            },
        )
        .expect("a tracked match");
        let reveals = |token: &str, game: u32| ManualEdit::SetReveals {
            token: token.to_string(),
            game,
            cards: vec![count(4002, 1)],
        };
        assert_eq!(
            edit(&mut conn, reveals("Q-missing", 1)),
            ManualEditOutcome::NotFound
        );
        assert_eq!(
            edit(&mut conn, reveals(TOKEN, 1)),
            ManualEditOutcome::NotManual,
            "0.x forbids tracked and imported matches"
        );
        assert_eq!(
            edit(&mut conn, reveals(&token, 4)),
            ManualEditOutcome::GameNotFound
        );
        hide(&conn, &token);
        assert_eq!(
            edit(&mut conn, reveals(&token, 1)),
            ManualEditOutcome::NotFound
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_cards"), 0);
        assert!(!is_manual(&conn, TOKEN).expect("manual"));
        for outcome in [
            ManualEditOutcome::NotFound,
            ManualEditOutcome::NotManual,
            ManualEditOutcome::GameNotFound,
        ] {
            assert!(outcome.refusal().is_some(), "{outcome:?}");
        }
        assert_eq!(ManualEditOutcome::Done.refusal(), None);
    }

    fn invalid(outcome: ManualEditOutcome) -> Vec<(String, String)> {
        match outcome {
            ManualEditOutcome::Invalid(errors) => errors
                .into_iter()
                .map(|error| (error.field, error.message))
                .collect(),
            other => panic!("not refused by a rule: {other:?}"),
        }
    }

    fn pair(field: &str, message: &str) -> Vec<(String, String)> {
        vec![(field.to_string(), message.to_string())]
    }

    #[test]
    fn a_new_match_is_checked_with_the_rules_before_anything_is_written() {
        let (_dir, mut conn) = db("manual-create-rules");
        let mut three_wins = payload();
        three_wins.games = vec![game(true, true, None); 3];
        let outcome = edit(&mut conn, ManualEdit::Create(three_wins));
        assert_eq!(
            outcome.refusal().as_deref(),
            Some("A best-of-three match cannot have more than two wins or two losses.")
        );
        assert_eq!(
            invalid(outcome),
            pair(
                "games",
                "A best-of-three match cannot have more than two wins or two losses."
            )
        );
        let mut backwards = payload();
        backwards.ended_at_ms = backwards.started_at_ms - 1;
        assert_eq!(
            invalid(edit(&mut conn, ManualEdit::Create(backwards))),
            pair(
                "ended_at",
                "The ended at field must be a date after or equal to started at."
            )
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 0);
    }

    // UpdateManualGameHandTest, through the writer: the rules, the maindeck
    // of the game, and "is idempotent for the same payload".
    #[test]
    fn a_hand_is_checked_against_the_games_maindeck() {
        let (_dir, mut conn) = db("manual-hand-rules");
        let token = create(&mut conn, payload());
        assert_eq!(
            invalid(edit(&mut conn, set_hand(&token, 1, &[4004; 6], &[], &[]))),
            pair(
                "kept_hand",
                "The final hand is seven cards: the ones you kept plus the ones you bottomed."
            )
        );
        let mut one_short = set_hand(&token, 1, &[4004; 6], &[4004], &[]);
        if let ManualEdit::SetHand { mulligan_count, .. } = &mut one_short {
            *mulligan_count = 1;
        }
        assert_eq!(
            invalid(edit(&mut conn, one_short)),
            pair(
                "mulliganed_hands",
                "Record one hand per mulligan, or leave a hand empty if you do not remember it."
            )
        );
        let with_sideboard_card = || {
            set_hand(
                &token,
                2,
                &[4003, 4004, 4004, 4004, 4004, 4004, 4004],
                &[],
                &[],
            )
        };
        assert_eq!(
            invalid(edit(&mut conn, with_sideboard_card())),
            pair(
                "kept_hand",
                "Every card in a hand must be in the maindeck for this game."
            )
        );
        let sideboard = ManualEdit::SetSideboard {
            token: token.clone(),
            game: 2,
            changes: vec![change(4003, 3, Direction::In)],
        };
        assert_eq!(edit(&mut conn, sideboard), ManualEditOutcome::Done);
        assert_eq!(
            edit(&mut conn, with_sideboard_card()),
            ManualEditOutcome::Done
        );
        let first = manual_game_rows(&conn, &token).expect("details");
        assert_eq!(
            edit(&mut conn, with_sideboard_card()),
            ManualEditOutcome::Done
        );
        assert_eq!(manual_game_rows(&conn, &token).expect("details"), first);
        assert_eq!(
            ManualEditOutcome::Done.refusal(),
            None,
            "the caller shows its own success message"
        );
    }

    fn hand_of(conn: &Connection, token: &str, game: u32) -> Option<Vec<u32>> {
        manual_game_rows(conn, token)
            .expect("details")
            .into_iter()
            .find(|detail| detail.game_number == game)
            .and_then(|detail| detail.starting_hand)
    }

    fn sideboard_edit(token: &str, game: u32, changes: Vec<SideboardChange>) -> ManualEdit {
        ManualEdit::SetSideboard {
            token: token.to_string(),
            game,
            changes,
        }
    }

    // UpdateManualGameSideboardTest, through the writer.
    #[test]
    fn a_sideboard_edit_clears_a_hand_it_no_longer_fits_and_never_edits_game_one() {
        let (_dir, mut conn) = db("manual-sideboard-rules");
        let token = create(&mut conn, payload());
        assert_eq!(
            invalid(edit(
                &mut conn,
                sideboard_edit(&token, 1, vec![change(4001, 1, Direction::Out)])
            )),
            pair("changes", "Game 1 is played with the registered deck.")
        );
        assert_eq!(
            invalid(edit(
                &mut conn,
                sideboard_edit(&token, 2, vec![change(4001, 5, Direction::Out)])
            )),
            pair(
                "changes",
                "A change exceeds the registered number of copies."
            )
        );

        // "keeps a hand that still fits", then "clears a kept hand that used a
        // sided-out card".
        let bolt_hand = [4001, 4004, 4004, 4004, 4004, 4004, 4004];
        assert_eq!(
            edit(&mut conn, set_hand(&token, 2, &bolt_hand, &[], &[])),
            ManualEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                sideboard_edit(&token, 2, vec![change(4001, 2, Direction::Out)])
            ),
            ManualEditOutcome::Done
        );
        assert_eq!(hand_of(&conn, &token, 2), Some(bolt_hand.to_vec()));
        let cleared = edit(
            &mut conn,
            sideboard_edit(&token, 2, vec![change(4001, 4, Direction::Out)]),
        );
        assert_eq!(cleared, ManualEditOutcome::HandCleared);
        assert_eq!(cleared.refusal(), None, "a done edit");
        assert_eq!(hand_of(&conn, &token, 2), None);

        // "clears an opening hand whose mulliganed or bottomed cards were
        // sided out": its mulligans go back to none.
        assert_eq!(
            edit(&mut conn, sideboard_edit(&token, 2, Vec::new())),
            ManualEditOutcome::Done
        );
        let goyf_mulligan = [4002, 4004, 4004, 4004, 4004, 4004, 4004];
        assert_eq!(
            edit(
                &mut conn,
                set_hand(&token, 2, &[4004; 6], &[4004], &[&goyf_mulligan])
            ),
            ManualEditOutcome::Done
        );
        assert_eq!(games_of(&conn, &token)[1].6, Some(1));
        assert_eq!(
            edit(
                &mut conn,
                sideboard_edit(&token, 2, vec![change(4002, 4, Direction::Out)])
            ),
            ManualEditOutcome::HandCleared
        );
        assert_eq!(hand_of(&conn, &token, 2), None);
        assert_eq!(games_of(&conn, &token)[1].6, Some(0));

        // "clears an opening hand that only fit the postboard deck when
        // changes are reset".
        let postboard = vec![
            change(4001, 3, Direction::Out),
            change(4003, 3, Direction::In),
        ];
        assert_eq!(
            edit(&mut conn, sideboard_edit(&token, 2, postboard)),
            ManualEditOutcome::Done
        );
        let rip_hand = [4003, 4004, 4004, 4004, 4004, 4004, 4004];
        assert_eq!(
            edit(&mut conn, set_hand(&token, 2, &rip_hand, &[], &[])),
            ManualEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, sideboard_edit(&token, 2, Vec::new())),
            ManualEditOutcome::HandCleared
        );
        assert_eq!(hand_of(&conn, &token, 2), None);
    }

    // UpdateManualGameRevealsTest: "validates cards", through the writer.
    #[test]
    fn reveals_must_be_known_cards() {
        let (_dir, mut conn) = db("manual-reveals-rules");
        let token = create(&mut conn, payload());
        let reveals = ManualEdit::SetReveals {
            token: token.clone(),
            game: 1,
            cards: vec![count(9999, 1)],
        };
        assert_eq!(
            invalid(edit(&mut conn, reveals)),
            pair(
                "cards.0.mtgo_id",
                "The selected cards.0.mtgo_id is invalid."
            )
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_cards"), 0);
    }

    #[test]
    fn a_manual_view_is_the_stored_match_as_the_tracker_pushes_it() {
        let (_dir, mut conn) = db("manual-view");
        let mut manual = payload();
        manual.games[2].turns = Some(6);
        let token = create(&mut conn, manual);
        let (view, revision) = manual_view(&conn, &token)
            .expect("view")
            .expect("a manual match");
        assert_eq!(revision, 1);
        assert_eq!(view.token, token);
        assert_eq!(view.mtgo_match_id, None);
        assert_eq!(view.local_username, "Qlocal");
        assert_eq!(view.opponent.as_deref(), Some("Qkai"));
        assert_eq!(view.format.as_deref(), Some("CMODERN"));
        assert_eq!(view.match_type.as_deref(), Some("Constructed"));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!((view.wins, view.losses), (2, 1));
        assert_eq!(view.started_at_ms, STARTED);
        assert_eq!(view.ended_at_ms, Some(STARTED + 45 * 60_000));
        let results: Vec<GameResult> = view.games.iter().map(|game| game.result).collect();
        assert_eq!(
            results,
            vec![GameResult::Win, GameResult::Loss, GameResult::Win]
        );
        assert_eq!(view.games[2].turn_count, Some(6));
        assert_eq!(view.games[1].on_play, Some(false));

        insert_manual(&conn, OTHER);
        conn.execute(
            "UPDATE matches SET source = 'imported' WHERE token = ?1",
            [OTHER],
        )
        .expect("not manual");
        assert_eq!(manual_view(&conn, OTHER).expect("view"), None);
        assert_eq!(manual_view(&conn, "Q-missing").expect("view"), None);
        let (other, _) = stored_view(&conn, OTHER)
            .expect("view")
            .expect("any stored match");
        assert_eq!(other.token, OTHER);
        assert_eq!(stored_view(&conn, "Q-missing").expect("view"), None);
    }

    // Edit a match (spec section 4).

    /// A log match Qlocal lost 1-2 to Qopponent in league Q-league, linked
    /// to deck 1's latest version, with the league folded.
    fn lost_log_match(conn: &mut Connection) -> i64 {
        use crate::write::{MatchWrite, WriteBatch, commit};
        let mut played = view(TOKEN, "Qopponent", STARTED);
        played.mtgo_match_id = None;
        played.league_token = Some("Q-league".to_string());
        played.outcome = Outcome::Loss;
        played.wins = 1;
        played.losses = 2;
        played.games = vec![
            crate::test_support::game(1, None, GameResult::Loss),
            crate::test_support::game(2, None, GameResult::Win),
            crate::test_support::game(3, None, GameResult::Loss),
        ];
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(played),
                    revision: 1,
                }],
                now_ms: 9_000,
                ..WriteBatch::default()
            },
        )
        .expect("the live match");
        conn.execute("UPDATE matches SET deck_version_id = 11", [])
            .expect("deck link");
        sync_leagues(conn);
        int(conn, TOKEN, "id")
    }

    /// Won 2-0 against Qrival: on the play in 7 turns, then on the draw.
    fn two_nil(now_ms: i64) -> MatchCorrection {
        MatchCorrection {
            opponent_name: Some("  Qrival ".to_string()),
            started_at_ms: Some(STARTED),
            ended_at_ms: Some(STARTED + 30 * 60_000),
            games: vec![game(true, true, Some(7)), game(true, false, None)],
            now_ms,
        }
    }

    fn correct_match(
        conn: &mut Connection,
        match_id: i64,
        correction: MatchCorrection,
    ) -> ManualEditOutcome {
        edit(
            conn,
            ManualEdit::Correct {
                match_id,
                correction,
            },
        )
    }

    #[test]
    fn a_log_match_corrected_to_a_win_reads_as_a_win_in_the_list_deck_and_league() {
        use crate::leagues::{RunFilter, stored_runs};
        use crate::match_list::{MatchListFilter, Scope, match_list};
        let (_dir, mut conn) = db("manual-correct-log");
        let match_id = lost_log_match(&mut conn);
        // Game 3 has cards, which go with it.
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, seen)
             SELECT id, 'opponent', 4001, 1 FROM games WHERE game_number = 3",
            [],
        )
        .expect("game 3 cards");
        let game_1_id: i64 = conn
            .query_row("SELECT id FROM games WHERE game_number = 1", [], |row| {
                row.get(0)
            })
            .expect("game 1");
        let revision = int(&conn, TOKEN, "revision");

        assert_eq!(
            correct_match(&mut conn, match_id, two_nil(20_000)),
            ManualEditOutcome::Done
        );

        assert_eq!(text(&conn, TOKEN, "outcome"), "win");
        assert_eq!(text(&conn, TOKEN, "source"), "log");
        assert_eq!(int(&conn, TOKEN, "corrected_at"), 20_000);
        assert_eq!(int(&conn, TOKEN, "revision"), revision + 1);
        assert_eq!(int(&conn, TOKEN, "ended_at"), STARTED + 30 * 60_000);
        let opponent: String = conn
            .query_row(
                "SELECT o.username FROM matches m JOIN opponents o ON o.id = m.opponent_id",
                [],
                |row| row.get(0),
            )
            .expect("opponent");
        assert_eq!(opponent, "Qrival");
        let games = games_of(&conn, TOKEN);
        assert_eq!(games.len(), 2, "game 3 is gone");
        assert_eq!(
            games
                .iter()
                .map(|game| (game.0, game.1.as_str(), game.2, game.3))
                .collect::<Vec<_>>(),
            vec![(1, "win", Some(1), Some(7)), (2, "win", Some(0), None)]
        );
        let kept_id: i64 = conn
            .query_row("SELECT id FROM games WHERE game_number = 1", [], |row| {
                row.get(0)
            })
            .expect("game 1");
        assert_eq!(kept_id, game_1_id, "a game is updated in place");
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM game_cards"),
            0,
            "the removed game's cards went with it"
        );

        let list = match_list(
            &conn,
            Scope::All(crate::Platform::Mtgo),
            &MatchListFilter::newest_first(decks::stats::Range::all_time()),
            1,
            25,
        )
        .expect("list");
        assert_eq!(list.rows.len(), 1);
        assert_eq!(list.rows[0].outcome, "win");
        assert_eq!((list.rows[0].wins, list.rows[0].losses), (2, 0));

        let record = crate::deck_pages::deck_outcome_record(&conn, 1, None).expect("record");
        assert_eq!((record.wins, record.losses), (1, 0));

        let runs = stored_runs(
            &conn,
            &RunFilter {
                owners: vec![LeagueOwner::Account(1)],
                kinds: vec![crate::leagues::LeagueKind::Constructed],
                deck_id: None,
                run_key: None,
            },
        )
        .expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].matches.len(), 1);
        assert_eq!(runs[0].matches[0].outcome, "win");
        let results: Vec<GameResult> = runs[0].matches[0]
            .games
            .iter()
            .map(|game| game.result)
            .collect();
        assert_eq!(results, vec![GameResult::Win, GameResult::Win]);
    }

    #[test]
    fn a_history_row_known_by_its_counts_gets_games_and_loses_the_counts() {
        let (_dir, mut conn) = db("manual-correct-history");
        insert_manual(&conn, TOKEN);
        conn.execute_batch(
            "DELETE FROM games;
             UPDATE matches SET source = 'history', outcome = 'loss', games_won_imported = 0,
                 games_lost_imported = 2;",
        )
        .expect("a history row with counts");
        let match_id = int(&conn, TOKEN, "id");

        let mut correction = two_nil(20_000);
        correction.games = vec![
            game(true, true, None),
            game(false, false, Some(5)),
            game(true, true, None),
        ];
        assert_eq!(
            correct_match(&mut conn, match_id, correction),
            ManualEditOutcome::Done
        );
        assert_eq!(text(&conn, TOKEN, "outcome"), "win");
        assert_eq!(
            one(&conn, TOKEN, "games_won_imported"),
            rusqlite::types::Value::Null
        );
        assert_eq!(
            one(&conn, TOKEN, "games_lost_imported"),
            rusqlite::types::Value::Null
        );
        let games = games_of(&conn, TOKEN);
        assert_eq!(
            games
                .iter()
                .map(|game| (game.0, game.1.as_str(), game.3))
                .collect::<Vec<_>>(),
            vec![(1, "win", None), (2, "loss", Some(5)), (3, "win", None)]
        );
        // New games get their slice of the match, in order.
        assert!(games[0].4 < games[1].4 && games[1].4 < games[2].4);
    }

    #[test]
    fn a_corrected_manual_row_is_marked_edited_for_the_import() {
        let (_dir, mut conn) = db("manual-correct-manual");
        let token = create(&mut conn, payload());
        let match_id = int(&conn, &token, "id");
        assert_eq!(
            correct_match(&mut conn, match_id, two_nil(20_000)),
            ManualEditOutcome::Done
        );
        assert_eq!(text(&conn, &token, "outcome"), "win");
        assert!(int(&conn, &token, "revision") > int(&conn, &token, "derived_version"));
        assert_eq!(games_of(&conn, &token).len(), 2);
    }

    #[test]
    fn a_correction_is_refused_by_the_add_rules_and_for_a_missing_match() {
        let (_dir, mut conn) = db("manual-correct-refused");
        let match_id = lost_log_match(&mut conn);
        let before = dump(&conn, "matches");

        let mut blank = two_nil(20_000);
        blank.opponent_name = Some("  ".to_string());
        assert_eq!(
            invalid(correct_match(&mut conn, match_id, blank)),
            pair("opponent_name", "The opponent name field is required.")
        );
        let mut backwards = two_nil(20_000);
        backwards.ended_at_ms = Some(STARTED - 1);
        assert_eq!(
            invalid(correct_match(&mut conn, match_id, backwards)),
            pair(
                "ended_at",
                "The ended at field must be a date after or equal to started at."
            )
        );
        let mut none = two_nil(20_000);
        none.games.clear();
        assert_eq!(
            invalid(correct_match(&mut conn, match_id, none)),
            pair("games", "The games field is required.")
        );
        let mut decided = two_nil(20_000);
        decided.games.push(game(false, true, None));
        assert_eq!(
            invalid(correct_match(&mut conn, match_id, decided)),
            pair(
                "games",
                "The match was already decided before the last game."
            )
        );
        let mut turns = two_nil(20_000);
        turns.games[1].turns = Some(0);
        assert_eq!(
            invalid(correct_match(&mut conn, match_id, turns)),
            pair(
                "games.1.turns",
                "The games.1.turns field must be at least 1."
            )
        );
        assert_eq!(
            correct_match(&mut conn, match_id + 100, two_nil(20_000)),
            ManualEditOutcome::NotFound
        );
        hide(&conn, TOKEN);
        let hidden = dump(&conn, "matches");
        assert_eq!(
            correct_match(&mut conn, match_id, two_nil(20_000)),
            ManualEditOutcome::NotFound,
            "a match removed from stats is not listed"
        );
        assert_eq!(dump(&conn, "matches"), hidden, "a refusal writes nothing");
        assert_ne!(before, hidden);
    }

    #[test]
    fn a_correction_may_keep_no_opponent_and_no_end() {
        let (_dir, mut conn) = db("manual-correct-unknowns");
        let match_id = lost_log_match(&mut conn);
        conn.execute("UPDATE matches SET ended_at = NULL", [])
            .expect("no known end");
        let mut unknowns = two_nil(20_000);
        unknowns.opponent_name = None;
        unknowns.ended_at_ms = None;
        assert_eq!(
            correct_match(&mut conn, match_id, unknowns),
            ManualEditOutcome::Done
        );
        assert_eq!(
            one(&conn, TOKEN, "opponent_id"),
            rusqlite::types::Value::Null
        );
        assert_eq!(one(&conn, TOKEN, "ended_at"), rusqlite::types::Value::Null);
    }

    #[test]
    fn a_time_the_player_left_alone_keeps_its_stored_value() {
        // The sheet shows minutes only: a start or end it sends as `None`
        // keeps the stored milliseconds, so an outcome-only Save moves
        // neither.
        let (_dir, mut conn) = db("manual-correct-kept-times");
        let match_id = lost_log_match(&mut conn);
        conn.execute(
            "UPDATE matches SET started_at = ?1, ended_at = ?2",
            params![STARTED + 59_123, STARTED + 4_000_456],
        )
        .expect("times with seconds");
        let mut kept = two_nil(20_000);
        kept.started_at_ms = None;
        kept.ended_at_ms = None;
        assert_eq!(
            correct_match(&mut conn, match_id, kept),
            ManualEditOutcome::Done
        );
        assert_eq!(int(&conn, TOKEN, "started_at"), STARTED + 59_123);
        assert_eq!(int(&conn, TOKEN, "ended_at"), STARTED + 4_000_456);
        assert_eq!(text(&conn, TOKEN, "outcome"), "win");

        // A kept start still bounds a new end.
        let mut backwards = two_nil(30_000);
        backwards.started_at_ms = None;
        backwards.ended_at_ms = Some(STARTED);
        assert_eq!(
            invalid(correct_match(&mut conn, match_id, backwards)),
            pair(
                "ended_at",
                "The ended at field must be a date after or equal to started at."
            )
        );
    }

    #[test]
    fn an_ended_match_with_no_result_counts_once_corrected() {
        // The projection never finishes a corrected match, so the fix makes
        // it complete: the stats, the queue and the sync read only those.
        use crate::match_list::{MatchListFilter, Scope, match_list};
        let (_dir, mut conn) = db("manual-correct-ended");
        let match_id = lost_log_match(&mut conn);
        conn.execute(
            "UPDATE matches SET state = 'ended', outcome = 'unknown', games_synced_at = NULL",
            [],
        )
        .expect("ended with no result");
        let list = |conn: &Connection| {
            match_list(
                conn,
                Scope::Deck {
                    deck_id: 1,
                    version: None,
                    best_of: None,
                },
                &MatchListFilter::newest_first(decks::stats::Range::all_time()),
                1,
                25,
            )
            .expect("list")
            .rows
            .len()
        };
        let record = |conn: &Connection| {
            crate::deck_pages::deck_header(
                conn,
                1,
                decks::stats::Range::all_time(),
                chrono_tz::UTC,
                None,
            )
            .expect("header")
            .expect("the deck")
            .summary
            .record
        };
        assert_eq!(list(&conn), 0);
        assert_eq!(record(&conn), decks::stats::Record::from_counts(0, 0, 0));

        assert_eq!(
            correct_match(&mut conn, match_id, two_nil(20_000)),
            ManualEditOutcome::Done
        );
        assert_eq!(text(&conn, TOKEN, "state"), "complete");
        assert_eq!(list(&conn), 1);
        assert_eq!(record(&conn), decks::stats::Record::from_counts(1, 0, 0));
        // The game sync ran and stamped it, so detection takes it.
        assert_ne!(
            one(&conn, TOKEN, "games_synced_at"),
            rusqlite::types::Value::Null
        );
        assert_eq!(
            crate::archetypes::due_matches(&conn, 10).expect("queue"),
            vec![match_id]
        );
    }

    #[test]
    fn a_corrected_history_match_is_pushed() {
        // A history row is not this device's to send until the player edits
        // it: a fix is an edit (`edited_at`).
        let (_dir, mut conn) = db("manual-correct-pushed");
        insert_manual(&conn, TOKEN);
        conn.execute_batch(
            "UPDATE matches SET source = 'history', deck_version_id = 11;
             UPDATE decks SET cloud_sync_enabled = 1;",
        )
        .expect("a synced history row");
        let match_id = int(&conn, TOKEN, "id");
        let owned = |conn: &Connection| {
            let candidates =
                crate::sync::match_candidates(conn, mymtgo_api::Tier::Free, STARTED + 3_600_000)
                    .expect("rows");
            assert_eq!(candidates.len(), 1);
            (candidates[0].owned, candidates[0].edited)
        };
        assert_eq!(owned(&conn), (false, false));

        assert_eq!(
            correct_match(&mut conn, match_id, two_nil(20_000)),
            ManualEditOutcome::Done
        );
        assert_eq!(int(&conn, TOKEN, "edited_at"), 20_000);
        assert_eq!(owned(&conn), (true, true));
    }

    #[test]
    fn the_edit_view_and_the_match_header_show_the_fix() {
        let (_dir, mut conn) = db("manual-correct-view");
        let match_id = lost_log_match(&mut conn);
        let view = match_edit_view(&conn, match_id)
            .expect("view")
            .expect("listed");
        assert_eq!(view.opponent_name.as_deref(), Some("Qopponent"));
        assert_eq!(view.started_at_ms, STARTED);
        assert_eq!(view.ended_at_ms, Some(STARTED + 4_000));
        assert!(!view.corrected);
        assert_eq!(view.games.len(), 3);
        assert_eq!(
            view.games[0],
            MatchEditGame {
                game_number: 1,
                result: "loss".to_string(),
                on_play: Some(true),
                turns: Some(9),
            }
        );
        let header = crate::match_detail::match_header(&conn, TOKEN)
            .expect("header")
            .expect("listed");
        assert!(!header.corrected);

        correct_match(&mut conn, match_id, two_nil(20_000));
        let view = match_edit_view(&conn, match_id)
            .expect("view")
            .expect("listed");
        assert!(view.corrected);
        assert_eq!(view.opponent_name.as_deref(), Some("Qrival"));
        assert_eq!(
            view.games
                .iter()
                .map(|game| (game.result.as_str(), game.on_play, game.turns))
                .collect::<Vec<_>>(),
            vec![("win", Some(true), Some(7)), ("win", Some(false), None)]
        );
        let header = crate::match_detail::match_header(&conn, TOKEN)
            .expect("header")
            .expect("listed");
        assert!(header.corrected);
        assert_eq!(
            match_token(&conn, match_id).expect("token").as_deref(),
            Some(TOKEN)
        );

        hide(&conn, TOKEN);
        assert_eq!(match_edit_view(&conn, match_id).expect("view"), None);
        assert_eq!(match_edit_view(&conn, match_id + 100).expect("view"), None);
    }

    // ---- Arena ------------------------------------------------------------------

    /// Arena account 9 (Qarena#12345) with deck 5 (version 50) and
    /// constructed readings at offsets 100 and 300 of log file 1; Arena
    /// account 10 (Qother#99999) with a constructed reading at offset 500.
    fn arena_db(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
             VALUES (9, 'Qarena#12345', 0, 'arena', 'Qarena-id'),
                    (10, 'Qother#99999', 0, 'arena', 'Qother-id');
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (5, 'Q-arena-deck', 9, 'Qarena deck', 'CSTANDARD', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (50, 5, 'Q-arena', 1);
             INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
             INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                 class, level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
             VALUES (9, 'constructed', 95, 1000, 'Gold', 3, 0, 0, 0, 1, 100),
                    (9, 'constructed', 95, 2000, 'Gold', 2, 0, 1, 0, 1, 300),
                    (10, 'constructed', 95, 3000, 'Gold', 1, 0, 1, 0, 1, 500);",
        )
        .expect("arena");
        (dir, conn)
    }

    fn arena_payload(opponent: &str) -> ManualMatch {
        ManualMatch {
            deck_id: 5,
            opponent: opponent.to_string(),
            ranked: true,
            ..payload()
        }
    }

    fn claims(conn: &Connection) -> Vec<Vec<String>> {
        dump(conn, "arena_rank_gap_claims")
    }

    #[test]
    fn an_arena_manual_match_is_ranked_bo3_with_a_named_opponent() {
        let (_dir, mut conn) = arena_db("manual-arena-named");
        let token = create(&mut conn, arena_payload("Rival"));
        assert_eq!(text(&conn, &token, "match_type"), "ranked");
        assert_eq!(text(&conn, &token, "game_structure"), "bo3");
        assert_eq!(text(&conn, &token, "format"), "CSTANDARD");
        assert_eq!(int(&conn, &token, "account_id"), 9);
        assert_eq!(linked_version(&conn, &token), 50);
        let opponent: (String, String, String) = conn
            .query_row(
                "SELECT o.username, o.platform, o.display_name
                 FROM matches m JOIN opponents o ON o.id = m.opponent_id WHERE m.token = ?1",
                [&token],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("opponent");
        assert_eq!(
            opponent,
            (
                "arena:name:Rival".to_string(),
                "arena".to_string(),
                "Rival".to_string()
            )
        );
        // The same name again is the same opponent; unranked is kept.
        let again = create(
            &mut conn,
            ManualMatch {
                ranked: false,
                ..arena_payload(" Rival ")
            },
        );
        assert_eq!(text(&conn, &again, "match_type"), "unranked");
        assert_eq!(
            int(&conn, &again, "opponent_id"),
            int(&conn, &token, "opponent_id")
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 1);
        assert_eq!(
            manual_push_parts(&conn, &token).expect("parts"),
            (Platform::Arena, Some("Rival".to_string()))
        );
    }

    fn rank_place(conn: &Connection, token: &str) -> Option<(i64, i64)> {
        match (
            one(conn, token, "rank_file_id"),
            one(conn, token, "rank_byte_offset"),
        ) {
            (rusqlite::types::Value::Integer(file), rusqlite::types::Value::Integer(offset)) => {
                Some((file, offset))
            }
            _ => None,
        }
    }

    #[test]
    fn a_ranked_arena_manual_match_has_the_reading_before_its_start() {
        let (_dir, mut conn) = arena_db("manual-arena-rank");
        // Account 9's readings were observed at 1000 and 2000; account 10's
        // at 3000 is not this deck's account.
        let token = create(&mut conn, arena_payload("Rival"));
        assert_eq!(rank_place(&conn, &token), Some((1, 300)));
        let casual = create(
            &mut conn,
            ManualMatch {
                ranked: false,
                ..arena_payload("Rival")
            },
        );
        assert_eq!(rank_place(&conn, &casual), None, "a casual match has none");
        let early = create(
            &mut conn,
            ManualMatch {
                started_at_ms: 500,
                ended_at_ms: 900,
                ..arena_payload("Rival")
            },
        );
        assert_eq!(rank_place(&conn, &early), None, "no reading before it");

        // An edit moving the start before the second reading.
        let match_id = int(&conn, &token, "id");
        let outcome = edit(
            &mut conn,
            ManualEdit::Correct {
                match_id,
                correction: MatchCorrection {
                    opponent_name: Some("Rival".to_string()),
                    started_at_ms: Some(1_500),
                    ended_at_ms: Some(1_800),
                    games: arena_payload("Rival").games,
                    now_ms: 9_500,
                },
            },
        );
        assert_eq!(outcome, ManualEditOutcome::Done);
        assert_eq!(rank_place(&conn, &token), Some((1, 100)));
    }

    #[test]
    fn an_arena_manual_match_can_be_bo1() {
        let (_dir, mut conn) = arena_db("manual-arena-bo1");
        let mut one_game = arena_payload("Rival");
        one_game.best_of = BestOf::One;
        one_game.games = vec![game(true, true, None)];
        let token = create(&mut conn, one_game);
        assert_eq!(text(&conn, &token, "game_structure"), "bo1");
        assert_eq!(text(&conn, &token, "outcome"), "win");
    }

    #[test]
    fn an_arena_manual_match_defaults_to_bo3() {
        let (_dir, mut conn) = arena_db("manual-arena-default-bo3");
        assert_eq!(arena_payload("Rival").best_of, BestOf::Three);
        let token = create(&mut conn, arena_payload("Rival"));
        assert_eq!(text(&conn, &token, "game_structure"), "bo3");
    }

    #[test]
    fn a_bo1_manual_match_with_two_games_is_refused() {
        let (_dir, mut conn) = arena_db("manual-arena-bo1-two");
        let mut two_games = arena_payload("Rival");
        two_games.best_of = BestOf::One;
        two_games.games = vec![game(true, true, None), game(false, false, None)];
        let refused = edit(&mut conn, ManualEdit::Create(two_games));
        assert_eq!(invalid(refused), pair("games", "A Bo1 match has one game."));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
    }

    #[test]
    fn editing_a_bo1_match_to_two_games_is_refused() {
        let (_dir, mut conn) = arena_db("manual-arena-bo1-edit");
        let mut one_game = arena_payload("Rival");
        one_game.best_of = BestOf::One;
        one_game.games = vec![game(true, true, None)];
        let token = create(&mut conn, one_game);
        let match_id = int(&conn, &token, "id");
        let refused = correct_match(&mut conn, match_id, two_nil(20_000));
        assert_eq!(invalid(refused), pair("games", "A Bo1 match has one game."));
        // One game is still fine.
        let mut one = two_nil(20_000);
        one.games = vec![game(false, true, None)];
        assert_eq!(
            correct_match(&mut conn, match_id, one),
            ManualEditOutcome::Done
        );
        assert_eq!(text(&conn, &token, "outcome"), "loss");
        // A tracked Bo1 match is held to the same rule.
        conn.execute(
            "UPDATE matches SET game_structure = 'bo1', source = 'log' WHERE id = ?1",
            [match_id],
        )
        .expect("tracked bo1");
        let refused = correct_match(&mut conn, match_id, two_nil(30_000));
        assert_eq!(invalid(refused), pair("games", "A Bo1 match has one game."));
    }

    #[test]
    fn a_blank_arena_opponent_leaves_no_opponent() {
        let (_dir, mut conn) = arena_db("manual-arena-blank");
        let token = create(&mut conn, arena_payload("  "));
        assert_eq!(
            one(&conn, &token, "opponent_id"),
            rusqlite::types::Value::Null
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 0);
    }

    #[test]
    fn an_arena_match_refuses_a_league() {
        let (_dir, mut conn) = arena_db("manual-arena-league");
        let refused = edit(
            &mut conn,
            ManualEdit::Create(ManualMatch {
                placement: ManualPlacement::ManualLeague { id: 7 },
                ..arena_payload("Rival")
            }),
        );
        assert_eq!(
            invalid(refused),
            pair("league_id", "Arena matches have no leagues.")
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
    }

    // ---- Endstep ----------------------------------------------------------------

    /// Endstep account 20 with deck 6 (version 60) and a tracked match
    /// `q-endstep` against opponent 30, shown as Qfoe.
    fn endstep_db(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at, platform, display_name)
             VALUES (20, 'endstep:Qme-id', 0, 'endstep', 'Qme');
             INSERT INTO opponents (id, username, first_seen_at, platform, display_name)
             VALUES (30, 'endstep:Qfoe-id', 0, 'endstep', 'Qfoe');
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source)
             VALUES (6, 'endstep:Qdeck', 20, 'Qendstep deck', 'Modern', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (60, 6, 'Q-endstep', 1);
             INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, last_activity_at, revision, derived_version)
             VALUES ('q-endstep', 20, 30, 'log', 'Modern', 'ranked', 'complete', 'loss', 1, 1,
                 1, 0);",
        )
        .expect("endstep");
        (dir, conn)
    }

    #[test]
    fn an_endstep_deck_takes_no_manual_match() {
        let (_dir, mut conn) = endstep_db("manual-endstep-create");
        let refused = edit(
            &mut conn,
            ManualEdit::Create(ManualMatch {
                deck_id: 6,
                ..payload()
            }),
        );
        assert_eq!(refused, ManualEditOutcome::InvalidDeck);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
    }

    #[test]
    fn a_corrected_endstep_match_keeps_its_opponent() {
        let (_dir, mut conn) = endstep_db("manual-endstep-correct");
        let match_id = int(&conn, "q-endstep", "id");
        let renamed = correct_match(
            &mut conn,
            match_id,
            MatchCorrection {
                opponent_name: Some("Qsomeone".to_string()),
                ..two_nil(20_000)
            },
        );
        assert_eq!(
            invalid(renamed),
            pair("opponent_name", "An Endstep match keeps its opponent.")
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM opponents"), 1);

        let kept = correct_match(
            &mut conn,
            match_id,
            MatchCorrection {
                opponent_name: Some(" Qfoe ".to_string()),
                ..two_nil(20_000)
            },
        );
        assert_eq!(kept, ManualEditOutcome::Done);
        assert_eq!(int(&conn, "q-endstep", "opponent_id"), 30);
        assert_eq!(text(&conn, "q-endstep", "outcome"), "win");
    }

    #[test]
    fn a_claimed_gap_is_stored() {
        let (_dir, mut conn) = arena_db("manual-arena-claim");
        let token = create(
            &mut conn,
            ManualMatch {
                rank_gap: Some((1, 300)),
                ..arena_payload("Rival")
            },
        );
        assert_eq!(
            claims(&conn),
            vec![vec![
                format!("Text({token:?})"),
                "Integer(1)".to_string(),
                "Integer(300)".to_string(),
                "Integer(9000)".to_string(),
            ]]
        );
    }

    #[test]
    fn a_gap_of_another_account_is_refused() {
        let (_dir, mut conn) = arena_db("manual-arena-foreign-gap");
        for place in [(1, 500), (1, 200)] {
            let refused = edit(
                &mut conn,
                ManualEdit::Create(ManualMatch {
                    rank_gap: Some(place),
                    ..arena_payload("Rival")
                }),
            );
            assert_eq!(
                invalid(refused),
                pair("rank_gap", "That rank gap is not available.")
            );
        }
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
        assert!(claims(&conn).is_empty());
    }

    #[test]
    fn a_gap_the_sheet_would_not_offer_is_refused() {
        let (_dir, mut conn) = arena_db("manual-arena-unclaimable-gap");
        conn.execute(
            "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                 class, level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
             VALUES (9, 'constructed', 95, 2500, 'Gold', 2, 0, 1, 0, 1, 400)",
            [],
        )
        .expect("an unchanged reading");
        // The first reading ends no gap; the gap ending at 400 is quiet.
        for place in [(1, 100), (1, 400)] {
            let refused = edit(
                &mut conn,
                ManualEdit::Create(ManualMatch {
                    rank_gap: Some(place),
                    ..arena_payload("Rival")
                }),
            );
            assert_eq!(
                invalid(refused),
                pair("rank_gap", "That rank gap is not available.")
            );
        }
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
        assert!(claims(&conn).is_empty());
    }

    #[test]
    fn an_unranked_match_refuses_a_gap() {
        let (_dir, mut conn) = arena_db("manual-arena-unranked-gap");
        let refused = edit(
            &mut conn,
            ManualEdit::Create(ManualMatch {
                ranked: false,
                rank_gap: Some((1, 300)),
                ..arena_payload("Rival")
            }),
        );
        assert_eq!(
            invalid(refused),
            pair("rank_gap", "That rank gap is not available.")
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 0);
    }

    #[test]
    fn mtgo_manual_matches_are_unchanged() {
        // The Arena fields are ignored for an MTGO deck.
        let (_dir, mut conn) = arena_db("manual-mtgo-unchanged");
        let token = create(
            &mut conn,
            ManualMatch {
                ranked: true,
                rank_gap: Some((1, 300)),
                ..payload()
            },
        );
        assert_eq!(text(&conn, &token, "match_type"), "Constructed");
        assert_eq!(
            one(&conn, &token, "game_structure"),
            rusqlite::types::Value::Null
        );
        assert_eq!(text(&conn, &token, "format"), "CMODERN");
        assert!(claims(&conn).is_empty());
        assert_eq!(
            manual_push_parts(&conn, &token).expect("parts"),
            (Platform::Mtgo, None)
        );
        let blank = edit(
            &mut conn,
            ManualEdit::Create(ManualMatch {
                opponent: "  ".to_string(),
                ..payload()
            }),
        );
        assert_eq!(
            invalid(blank),
            pair("opponent_name", "The opponent name field is required.")
        );
    }
}
