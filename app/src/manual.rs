//! Manual match commands (Plan L, dashboard spec section 5): add a match the
//! tracker never saw (0.x `StoreManualMatchRequest`, `CreateManualMatch`),
//! the league runs it can join (0.x `GetManualMatchLeagueOptions`), the
//! per-game hand, sideboard and reveals edits (0.x `UpdateManualGame*`), and
//! the card search of the reveals picker (0.x `SearchCards`), and "Edit
//! match": a player's fix of any match, in place (edit a match spec).
//!
//! Every write goes through the tracker (the only writer), which checks the
//! rules again in its transaction. A form the rules refuse comes back as
//! 0.x's messages by field (`games.0.turns`, `league_id`), the first message
//! per field, the way the dashboard layout save answers. A missing match or
//! game is a refusal with a message.
//!
//! A manual match joins a league run in one of two ways:
//! - a run the fold makes (derived from the log, or a 0.x run it continues;
//!   the stored runs): the match carries the run's league token, and the
//!   fold places it like any league match, by its start (it has no join
//!   line). The fold gives a match only to an Active run that holds fewer
//!   than five, so only those are offered;
//! - a manual league: a link (`matches.league_id` with `league_linked`).
//!
//! Ids from the UI are `u32` (the TypeScript export refuses `i64`
//! arguments); league choices are run keys (`manual:{id}` for a manual
//! league).

use std::collections::BTreeMap;

use leagues::{CONSTRUCTED_ROUNDS, LeagueKind, RunState};
use serde::{Deserialize, Serialize};
use store::decks::search_cards;
use store::leagues::{MANUAL_MAX_MATCHES, RunFilter, StoredRun, stored_runs};
use store::manual::{
    CardCount, Direction, ManualEdit, ManualEditOutcome, ManualGame, ManualMatch, ManualPlacement,
    MatchCorrection, SideboardChange,
};
use store::manual_rules::{
    self, CreateInput, FieldError, GameInput, HAND_SAVED, MATCH_ADDED, REVEALS_SAVED,
    SIDEBOARD_SAVED, SIDEBOARD_SAVED_HAND_CLEARED,
};
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager, State};

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::leagues::{accounts_or_all, deck_accounts, owners_of};
use crate::logging::Timer;
use crate::matches::SideboardKind;
use crate::platform::{BestOfView, PlatformView};
use crate::rank::{RankGapKey, RankGapOption, arena_account_of_deck, played_elsewhere_gaps};

/// At most this many leagues are offered (0.x `limit(50)`).
pub const LEAGUE_OPTIONS_LIMIT: usize = 50;
/// At most this many cards a search finds (0.x `SearchCards` `$limit`).
pub const CARD_SEARCH_LIMIT: u32 = 20;
/// The shortest and longest card search, in characters (0.x `min:2`,
/// `max:100`).
pub const MIN_SEARCH_CHARS: usize = 2;
pub const MAX_SEARCH_CHARS: usize = 100;

/// 0.x's form keys for the deck and the league.
const DECK_FIELD: &str = "deck_id";
const LEAGUE_FIELD: &str = "league_id";
const STARTED_AT_FIELD: &str = "started_at";
const EARLY_START: &str = "The match must start after this league's first match.";
/// The toast once "Edit match" is saved (0.x had no such edit).
pub const MATCH_CORRECTED: &str = "Match updated.";

// ---- Types the UI sends ------------------------------------------------------

/// One game of the form, in the order played.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualGameInput {
    pub won: bool,
    pub on_play: bool,
    /// 1 to 99, when the player knows.
    pub turns: Option<u32>,
}

/// The "Add manual match" form (0.x `StoreManualMatchRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualMatchInput {
    pub deck_id: u32,
    pub opponent_name: String,
    /// The `key` of a `manual_league_options` entry; `null` for no league.
    pub league: Option<String>,
    /// Local date and time (OS timezone), `YYYY-MM-DDTHH:MM`, as a
    /// `datetime-local` input sends it.
    pub started_at: String,
    pub ended_at: String,
    /// One to three games.
    pub games: Vec<ManualGameInput>,
    /// The opponent's archetype (0.x `archetype_id`); `null` for none.
    #[specta(optional)]
    #[serde(default)]
    pub archetype_id: Option<u32>,
    /// Arena only: ranked (the default) or unranked. MTGO ignores it.
    #[specta(optional)]
    #[serde(default)]
    pub ranked: Option<bool>,
    /// Arena only: best of one or three; `null` is best of three. MTGO
    /// ignores it.
    #[specta(optional)]
    #[serde(default)]
    pub best_of: Option<BestOfView>,
    /// Arena only: the `manual_rank_gaps` gap the match was played in; only
    /// for a ranked match. MTGO ignores it.
    #[specta(optional)]
    #[serde(default)]
    pub rank_gap: Option<RankGapKey>,
}

/// The opening hand dialog (0.x `UpdateManualGameHandRequest`): catalog ids,
/// one entry per copy. An empty starting hand with no mulligans clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualHandInput {
    pub mulligan_count: u32,
    /// The starting hand: the cards kept, without the bottomed ones (0.x
    /// `kept_hand`).
    pub starting_hand: Vec<u32>,
    /// Put on the bottom after the last mulligan.
    pub bottomed: Vec<u32>,
    /// One per mulligan, empty when the player does not remember it.
    pub mulliganed_hands: Vec<Vec<u32>>,
}

/// One line of the sideboard dialog (0.x `changes.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualSideboardChangeInput {
    pub catalog_id: u32,
    pub quantity: u32,
    pub kind: SideboardKind,
}

/// One card of the reveals dialog (0.x `cards.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualRevealInput {
    pub catalog_id: u32,
    pub quantity: u32,
}

/// The "Edit match" form: the Add sheet's fields without the deck, league
/// and archetype. A `null` (or blank) opponent keeps a match the log gave
/// none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchCorrectionInput {
    pub opponent_name: Option<String>,
    /// Local date and time, as `ManualMatchInput`'s; `null` when the player
    /// left it alone, so the stored start (to the millisecond) is kept.
    pub started_at: Option<String>,
    /// As `started_at`; `null` (or blank) keeps the stored end, or none.
    pub ended_at: Option<String>,
    /// One to three games.
    pub games: Vec<ManualGameInput>,
}

// ---- Types the UI receives ---------------------------------------------------

/// One game as the "Edit match" sheet opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchEditGameView {
    pub game_number: u32,
    /// `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    pub on_play: Option<bool>,
    pub turns: Option<u32>,
}

/// What the "Edit match" sheet opens with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchEditView {
    pub opponent_name: Option<String>,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub ended_at_ms: Option<i64>,
    /// By game number; empty for a match known only by its game counts.
    pub games: Vec<MatchEditGameView>,
    /// The player has fixed the match before.
    pub corrected: bool,
    /// The match's account's platform: an Arena match's opponent is
    /// optional and named by its Arena name.
    pub platform: PlatformView,
}

/// What adding a manual match did: added (its token and 0.x's flash
/// message), or refused with 0.x's messages by field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ManualMatchSaveView {
    Created { token: String, message: String },
    Invalid { errors: BTreeMap<String, String> },
}

/// What a hand, sideboard or reveals edit did: saved, with 0.x's flash
/// message (a sideboard edit that cleared the hand says so), or refused
/// with 0.x's messages by field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ManualEditView {
    Saved { message: String },
    Invalid { errors: BTreeMap<String, String> },
}

/// A league a manual match can be added to (0.x
/// `GetManualMatchLeagueOptions`). The sheet shows `name (matchCount/roundCount)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualLeagueOption {
    /// The run key to send back as `league`.
    pub key: String,
    pub name: String,
    /// The deck the league was played with, if known.
    #[specta(type = Option<specta_typescript::Number>)]
    pub deck_id: Option<i64>,
    /// Its complete matches.
    pub match_count: u32,
    pub round_count: u32,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
}

/// A card the reveals picker found (0.x `SearchCards`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardSearchView {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// The card's type line.
    pub type_line: Option<String>,
}

// ---- League options ----------------------------------------------------------

/// A stored run the fold made, with what the options and the create check
/// need.
struct FoldedRun {
    /// Its `leagues.id`.
    id: i64,
    /// `None` for a 0.x run the import could give no account.
    account_id: Option<i64>,
    key: String,
    token: String,
    name: String,
    state: RunState,
    started_at: i64,
    deck_version_id: Option<i64>,
    /// The deck of its version (`None` without one, or when it is gone).
    deck_id: Option<i64>,
    /// Its first match's game structure (`Modern`).
    game_structure: Option<String>,
    /// Every listed match it holds, and the complete ones.
    matches: usize,
    complete: usize,
}

/// Whether the fold would give this run another match: it is Active and
/// holds fewer than five.
fn takes_a_match(run: &FoldedRun) -> bool {
    run.state == RunState::Active && run.matches < CONSTRUCTED_ROUNDS
}

/// The stored constructed runs of `accounts` and, when `manual`, the manual
/// leagues (with the account-less ones), or else the runs the fold makes
/// (derived and 0.x runs, with the 0.x runs of no account: 0.x offered every
/// run, issue #162; a match placed in one gives it the match's account).
fn constructed_runs(
    conn: &Connection,
    accounts: &[i64],
    manual: bool,
) -> Result<Vec<StoredRun>, StoreError> {
    let filter = RunFilter {
        owners: owners_of(accounts),
        kinds: vec![LeagueKind::Constructed],
        deck_id: None,
        run_key: None,
    };
    let mut runs = Vec::new();
    for run in stored_runs(conn, &filter)? {
        let is_manual = run.source == "manual";
        let has_owner = run.account_id.is_some() || run.source == "imported";
        if is_manual == manual && (manual || has_owner) {
            runs.push(run);
        }
    }
    Ok(runs)
}

/// Every run the fold makes for `accounts`.
fn folded_runs(conn: &Connection, accounts: &[i64]) -> Result<Vec<FoldedRun>, StoreError> {
    let mut runs = Vec::new();
    for run in constructed_runs(conn, accounts, false)? {
        let complete = run
            .matches
            .iter()
            .filter(|row| row.state == "complete")
            .count();
        let game_structure = run.matches.iter().find_map(|row| {
            row.game_structure
                .clone()
                .filter(|structure| !structure.trim().is_empty())
        });
        runs.push(FoldedRun {
            id: run.id,
            account_id: run.account_id,
            matches: run.matches.len(),
            complete,
            game_structure,
            key: run.key,
            token: run.token.unwrap_or_default(),
            name: run.name,
            state: run.state,
            started_at: run.started_at,
            deck_version_id: run.deck_version_id,
            deck_id: run.deck_id,
        });
    }
    Ok(runs)
}

/// 0.x `GetManualMatchLeagueOptions`: constructed runs that can still take
/// a match, of this deck or with no version, and manual leagues with fewer
/// than five complete matches, of this deck or of none; newest first, at
/// most 50. Without a deck: the current account's (every account's when
/// there is none).
pub fn league_options(
    conn: &Connection,
    deck_id: Option<i64>,
) -> Result<Vec<ManualLeagueOption>, StoreError> {
    let accounts = match deck_id {
        Some(id) => match deck_accounts(conn, id)? {
            Some(accounts) => accounts,
            None => return Ok(Vec::new()),
        },
        None => accounts_or_all(
            conn,
            store::Platform::Mtgo,
            crate::platform::page_scope(conn)?.account,
        )?,
    };
    let mut options = Vec::new();
    for run in folded_runs(conn, &accounts)? {
        if !takes_a_match(&run) {
            continue;
        }
        // 0.x: no version, or a version of this deck.
        if let Some(wanted) = deck_id
            && run.deck_version_id.is_some()
            && run.deck_id != Some(wanted)
        {
            continue;
        }
        options.push(ManualLeagueOption {
            key: run.key,
            name: run.name,
            deck_id: run.deck_id,
            match_count: run.complete as u32,
            round_count: CONSTRUCTED_ROUNDS as u32,
            started_at_ms: run.started_at,
        });
    }

    for league in constructed_runs(conn, &accounts, true)? {
        let complete = league
            .matches
            .iter()
            .filter(|row| row.state == "complete")
            .count();
        if complete >= MANUAL_MAX_MATCHES {
            continue;
        }
        if let Some(wanted) = deck_id
            && league
                .deck_id
                .is_some_and(|league_deck| league_deck != wanted)
        {
            continue;
        }
        options.push(ManualLeagueOption {
            key: league.key,
            name: league.name,
            deck_id: league.deck_id,
            match_count: complete as u32,
            round_count: MANUAL_MAX_MATCHES as u32,
            started_at_ms: league.started_at,
        });
    }
    // Stable: on a tie, runs before manual leagues.
    options.sort_by_key(|option| std::cmp::Reverse(option.started_at_ms));
    options.truncate(LEAGUE_OPTIONS_LIMIT);
    Ok(options)
}

// ---- Adding a match ----------------------------------------------------------

/// Where a new match goes, or why it cannot (by 0.x field). A run is
/// checked against the fold, as 0.x `validateLeague` checks a league: the
/// deck first, then the room. The store checks a manual league.
fn placement(
    conn: &Connection,
    deck_id: i64,
    league: Option<&str>,
    started_at_ms: i64,
) -> Result<Result<ManualPlacement, BTreeMap<String, String>>, StoreError> {
    let Some(key) = league.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok(Ok(ManualPlacement::None));
    };
    if let Some(id) = key.strip_prefix("manual:") {
        return Ok(match id.parse::<i64>() {
            Ok(id) => Ok(ManualPlacement::ManualLeague { id }),
            Err(_) => Err(refusal_error(ManualEditOutcome::InvalidLeague)),
        });
    }
    let Some(accounts) = deck_accounts(conn, deck_id)? else {
        return Ok(Err(refusal_error(ManualEditOutcome::InvalidDeck)));
    };
    let runs = folded_runs(conn, &accounts)?;
    let Some(run) = runs.into_iter().find(|run| run.key == key) else {
        return Ok(Err(refusal_error(ManualEditOutcome::InvalidLeague)));
    };
    if run.deck_id.is_some_and(|run_deck| run_deck != deck_id) {
        return Ok(Err(refusal_error(ManualEditOutcome::WrongLeagueDeck)));
    }
    if run.complete >= CONSTRUCTED_ROUNDS || run.matches >= CONSTRUCTED_ROUNDS {
        return Ok(Err(refusal_error(ManualEditOutcome::LeagueFull)));
    }
    // A run that is over (dropped, or left Partial) takes no more matches.
    if run.state != RunState::Active {
        return Ok(Err(refusal_error(ManualEditOutcome::InvalidLeague)));
    }
    // The fold places a manual match by its start. An earlier one would start
    // a new run and orphan this run's notes and manual drops.
    if started_at_ms < run.started_at {
        let mut errors = BTreeMap::new();
        errors.insert(STARTED_AT_FIELD.to_string(), EARLY_START.to_string());
        return Ok(Err(errors));
    }
    Ok(Ok(ManualPlacement::Run {
        league_token: run.token,
        game_structure: run.game_structure,
        deck_version_id: run.deck_version_id,
        // A 0.x run with no account becomes the match's account's, so the
        // fold places the match in it.
        adopt_league_id: run.account_id.is_none().then_some(run.id),
    }))
}

/// An Arena match is in no league: a league given is refused.
fn arena_placement(league: Option<&str>) -> Result<ManualPlacement, BTreeMap<String, String>> {
    match league.map(str::trim).filter(|key| !key.is_empty()) {
        None => Ok(ManualPlacement::None),
        Some(_) => {
            let mut errors = BTreeMap::new();
            errors.insert(
                LEAGUE_FIELD.to_string(),
                store::manual::ARENA_NO_LEAGUES.to_string(),
            );
            Err(errors)
        }
    }
}

/// Field errors as the UI reads them: the first message per field.
fn error_map(errors: &[FieldError]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for error in errors {
        map.entry(error.field.clone())
            .or_insert_with(|| error.message.clone());
    }
    map
}

/// The 0.x field a refusal of a new match belongs to, `None` when it is not
/// a form error.
fn refusal_field(outcome: &ManualEditOutcome) -> Option<&'static str> {
    match outcome {
        ManualEditOutcome::InvalidDeck | ManualEditOutcome::NoDeckVersion => Some(DECK_FIELD),
        ManualEditOutcome::InvalidLeague
        | ManualEditOutcome::WrongLeagueDeck
        | ManualEditOutcome::LeagueFull => Some(LEAGUE_FIELD),
        _ => None,
    }
}

/// A refusal of a new match as its field's error.
fn refusal_error(outcome: ManualEditOutcome) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let (Some(field), Some(message)) = (refusal_field(&outcome), outcome.refusal()) {
        map.insert(field.to_string(), message);
    }
    map
}

/// A refusal that is not a form error, as a message for the user.
fn refused(outcome: ManualEditOutcome) -> CommandError {
    CommandError::Refused(
        outcome
            .refusal()
            .unwrap_or_else(|| "Nothing changed.".to_string()),
    )
}

/// Apply an edit through the tracker. The tracker lock is held only to
/// take the editor, never while waiting for the answer.
fn send(state: &AppState, edit: ManualEdit) -> Result<ManualEditOutcome, CommandError> {
    let editor = state.tracker_part(tracker::TrackerHandle::manual_editor)?;
    editor.edit(edit).map_err(CommandError::Tracker)
}

/// 0.x `StoreController` (`CreateManualMatch`): check the form, find the
/// league, and add the match through the tracker.
pub fn create_match(
    state: &AppState,
    input: &ManualMatchInput,
    now_ms: i64,
) -> Result<ManualMatchSaveView, CommandError> {
    let mut games = Vec::new();
    for game in &input.games {
        games.push(GameInput {
            won: game.won,
            on_play: game.on_play,
            turns: game.turns,
        });
    }
    let deck_id = i64::from(input.deck_id);
    // An Arena deck's match may have no opponent and has no league.
    let arena = arena_account_of_deck(&lock(&state.reader), deck_id)
        .map_err(storage)?
        .is_some();
    let form = CreateInput {
        opponent_name: input.opponent_name.clone(),
        started_at: input.started_at.clone(),
        ended_at: input.ended_at.clone(),
        games,
        opponent_required: !arena,
    };
    let valid = match manual_rules::validate_create(&form, state.timezone) {
        Ok(valid) => valid,
        Err(errors) => {
            // 0.x returned every group at once: the form's rules, then the
            // league and deck. The start is not known here, so the early
            // start check waits until the form is valid.
            let mut map = error_map(&errors);
            let conn = lock(&state.reader);
            if deck_accounts(&conn, deck_id).map_err(storage)?.is_none() {
                for (field, message) in refusal_error(ManualEditOutcome::InvalidDeck) {
                    map.entry(field).or_insert(message);
                }
            }
            let placed = if arena {
                arena_placement(input.league.as_deref())
            } else {
                placement(&conn, deck_id, input.league.as_deref(), i64::MAX).map_err(storage)?
            };
            if let Err(placement_errors) = placed {
                for (field, message) in placement_errors {
                    map.entry(field).or_insert(message);
                }
            }
            return Ok(ManualMatchSaveView::Invalid { errors: map });
        }
    };
    let placement = {
        let conn = lock(&state.reader);
        let placed = if arena {
            arena_placement(input.league.as_deref())
        } else {
            placement(&conn, deck_id, input.league.as_deref(), valid.started_at_ms)
                .map_err(storage)?
        };
        match placed {
            Ok(placement) => placement,
            Err(errors) => return Ok(ManualMatchSaveView::Invalid { errors }),
        }
    };
    let mut manual_games = Vec::new();
    for game in valid.games {
        manual_games.push(ManualGame {
            won: game.won,
            on_play: game.on_play,
            turns: game.turns,
        });
    }
    let outcome = send(
        state,
        ManualEdit::Create(ManualMatch {
            deck_id,
            opponent: valid.opponent,
            started_at_ms: valid.started_at_ms,
            ended_at_ms: valid.ended_at_ms,
            games: manual_games,
            placement,
            opponent_archetype_id: input.archetype_id.map(i64::from),
            ranked: input.ranked.unwrap_or(true),
            best_of: input
                .best_of
                .map_or(store::BestOf::Three, store::BestOf::from),
            rank_gap: input.rank_gap.map(|key| (key.file_id, key.byte_offset)),
            now_ms,
        }),
    )?;
    match outcome {
        ManualEditOutcome::Created { token } => Ok(ManualMatchSaveView::Created {
            token,
            message: MATCH_ADDED.to_string(),
        }),
        ManualEditOutcome::Invalid(errors) => Ok(ManualMatchSaveView::Invalid {
            errors: error_map(&errors),
        }),
        other if refusal_field(&other).is_some() => Ok(ManualMatchSaveView::Invalid {
            errors: refusal_error(other),
        }),
        other => Err(refused(other)),
    }
}

/// What an edit of a game did, with `saved` as its flash message.
fn edit_view(outcome: ManualEditOutcome, saved: &str) -> Result<ManualEditView, CommandError> {
    match outcome {
        ManualEditOutcome::Done => Ok(ManualEditView::Saved {
            message: saved.to_string(),
        }),
        ManualEditOutcome::HandCleared => Ok(ManualEditView::Saved {
            message: SIDEBOARD_SAVED_HAND_CLEARED.to_string(),
        }),
        ManualEditOutcome::Invalid(errors) => Ok(ManualEditView::Invalid {
            errors: error_map(&errors),
        }),
        other => Err(refused(other)),
    }
}

// ---- Editing a game ------------------------------------------------------------

/// 0.x `UpdateHandController`.
pub fn set_hand(
    state: &AppState,
    token: &str,
    game: u32,
    hand: &ManualHandInput,
) -> Result<ManualEditView, CommandError> {
    let outcome = send(
        state,
        ManualEdit::SetHand {
            token: token.to_string(),
            game,
            mulligan_count: hand.mulligan_count,
            starting: hand.starting_hand.clone(),
            bottomed: hand.bottomed.clone(),
            mulligans: hand.mulliganed_hands.clone(),
        },
    )?;
    edit_view(outcome, HAND_SAVED)
}

/// 0.x `UpdateSideboardController`.
pub fn set_sideboard(
    state: &AppState,
    token: &str,
    game: u32,
    changes: &[ManualSideboardChangeInput],
) -> Result<ManualEditView, CommandError> {
    let mut store_changes = Vec::new();
    for change in changes {
        store_changes.push(SideboardChange {
            catalog_id: change.catalog_id,
            quantity: change.quantity,
            direction: match change.kind {
                SideboardKind::In => Direction::In,
                SideboardKind::Out => Direction::Out,
            },
        });
    }
    let outcome = send(
        state,
        ManualEdit::SetSideboard {
            token: token.to_string(),
            game,
            changes: store_changes,
        },
    )?;
    edit_view(outcome, SIDEBOARD_SAVED)
}

/// 0.x `UpdateRevealsController`.
pub fn set_reveals(
    state: &AppState,
    token: &str,
    game: u32,
    cards: &[ManualRevealInput],
) -> Result<ManualEditView, CommandError> {
    let mut counts = Vec::new();
    for card in cards {
        counts.push(CardCount {
            catalog_id: card.catalog_id,
            quantity: card.quantity,
        });
    }
    let outcome = send(
        state,
        ManualEdit::SetReveals {
            token: token.to_string(),
            game,
            cards: counts,
        },
    )?;
    edit_view(outcome, REVEALS_SAVED)
}

// ---- Editing a match -----------------------------------------------------------

/// The listed match `match_id` as the "Edit match" sheet opens it; a
/// refusal when it is missing, deleted or detached.
pub fn edit_form(conn: &Connection, match_id: u32) -> Result<MatchEditView, CommandError> {
    let Some(view) = store::manual::match_edit_view(conn, i64::from(match_id)).map_err(storage)?
    else {
        return Err(refused(ManualEditOutcome::NotFound));
    };
    let mut games = Vec::new();
    for game in view.games {
        games.push(MatchEditGameView {
            game_number: game.game_number,
            result: game.result,
            on_play: game.on_play,
            turns: game.turns,
        });
    }
    Ok(MatchEditView {
        opponent_name: view.opponent_name,
        started_at_ms: view.started_at_ms,
        ended_at_ms: view.ended_at_ms,
        games,
        corrected: view.corrected,
        platform: PlatformView::from(view.platform),
    })
}

/// A local date and time of the form (`label` is 0.x's field label, as in
/// "The started at field is required."), as UTC milliseconds.
fn form_time(text: &str, label: &str, tz: chrono_tz::Tz) -> Result<i64, String> {
    if text.trim().is_empty() {
        return Err(format!("The {label} field is required."));
    }
    manual_rules::local_to_utc_ms(text, tz)
        .ok_or_else(|| format!("The {label} field must be a valid date."))
}

/// "Edit match": check the form with Add's rules (an opponent left out
/// stays unknown, a time left out keeps the stored one) and correct the
/// match through the tracker.
pub fn correct(
    state: &AppState,
    match_id: u32,
    input: &MatchCorrectionInput,
    now_ms: i64,
) -> Result<ManualEditView, CommandError> {
    let opponent_name = input
        .opponent_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let mut games = Vec::new();
    for game in &input.games {
        games.push(ManualGame {
            won: game.won,
            on_play: game.on_play,
            turns: game.turns,
        });
    }
    let started = match input.started_at.as_deref() {
        Some(text) => form_time(text, "started at", state.timezone).map(Some),
        None => Ok(None),
    };
    let ended = match input.ended_at.as_deref() {
        Some(text) if !text.trim().is_empty() => {
            form_time(text, "ended at", state.timezone).map(Some)
        }
        _ => Ok(None),
    };
    let (started_at_ms, ended_at_ms) = match (started, ended) {
        (Ok(started), Ok(ended)) => (started, ended),
        (started, ended) => {
            // Every error at once, as Add answers: the times, then the rest.
            let mut map = BTreeMap::new();
            if let Err(message) = &started {
                map.insert(STARTED_AT_FIELD.to_string(), message.clone());
            }
            if let Err(message) = &ended {
                map.insert("ended_at".to_string(), message.clone());
            }
            let rest = manual_rules::correction_errors(
                opponent_name.as_deref(),
                started.clone().ok().flatten().unwrap_or(i64::MIN),
                ended.clone().unwrap_or(None),
                &games,
            );
            for (field, message) in error_map(&rest) {
                map.entry(field).or_insert(message);
            }
            return Ok(ManualEditView::Invalid { errors: map });
        }
    };
    let outcome = send(
        state,
        ManualEdit::Correct {
            match_id: i64::from(match_id),
            correction: MatchCorrection {
                opponent_name,
                started_at_ms,
                ended_at_ms,
                games,
                now_ms,
            },
        },
    )?;
    edit_view(outcome, MATCH_CORRECTED)
}

// ---- Card search -------------------------------------------------------------

/// 0.x `SearchController`: the term is trimmed and 2 to 100 characters.
pub fn search(conn: &Connection, q: &str) -> Result<Vec<CardSearchView>, CommandError> {
    let term = q.trim();
    let chars = term.chars().count();
    let refused = |message: String| Err(CommandError::Refused(message));
    if chars == 0 {
        return refused("The q field is required.".to_string());
    }
    if chars < MIN_SEARCH_CHARS {
        return refused(format!(
            "The q field must be at least {MIN_SEARCH_CHARS} characters."
        ));
    }
    if chars > MAX_SEARCH_CHARS {
        return refused(format!(
            "The q field must not be greater than {MAX_SEARCH_CHARS} characters."
        ));
    }
    let mut found = Vec::new();
    for card in search_cards(conn, term, CARD_SEARCH_LIMIT).map_err(storage)? {
        found.push(CardSearchView {
            catalog_id: card.catalog_id,
            oracle_id: card.oracle_id.filter(|oracle| !oracle.is_empty()),
            name: card.name,
            image: card.image.filter(|image| !image.is_empty()),
            local_image: card.local_image,
            type_line: card.card_type,
        });
    }
    Ok(found)
}

// ---- Commands ----------------------------------------------------------------

/// The leagues the "Add manual match" sheet offers; `deck_id` narrows them
/// to one deck's.
#[tauri::command]
#[specta::specta]
pub fn manual_league_options(
    deck_id: Option<u32>,
    startup: State<'_, Startup>,
) -> Result<Vec<ManualLeagueOption>, CommandError> {
    let _timer = Timer::start("manual_league_options");
    let state = ready(&startup)?;
    league_options(&lock(&state.reader), deck_id.map(i64::from)).map_err(storage)
}

/// Add a manual match. Async so the wait for the tracker's commit never
/// blocks the window. The tracker pushes the new match and `DecksChanged`
/// and `LeaguesChanged` once committed.
#[tauri::command]
#[specta::specta]
pub async fn create_manual_match(
    app: AppHandle,
    input: ManualMatchInput,
) -> Result<ManualMatchSaveView, CommandError> {
    let _timer = Timer::start("create_manual_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        create_match(ready(&startup)?, &input, crate::wall_clock_ms())
    })
    .await
}

/// Record a manual game's opening hand (`game_number` 1-based). Async, as
/// above; the tracker pushes the updated match.
#[tauri::command]
#[specta::specta]
pub async fn set_manual_hand(
    app: AppHandle,
    token: String,
    game_number: u32,
    hand: ManualHandInput,
) -> Result<ManualEditView, CommandError> {
    let _timer = Timer::start("set_manual_hand");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_hand(ready(&startup)?, &token, game_number, &hand)
    })
    .await
}

/// Record a manual game's sideboard changes. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn set_manual_sideboard(
    app: AppHandle,
    token: String,
    game_number: u32,
    changes: Vec<ManualSideboardChangeInput>,
) -> Result<ManualEditView, CommandError> {
    let _timer = Timer::start("set_manual_sideboard");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_sideboard(ready(&startup)?, &token, game_number, &changes)
    })
    .await
}

/// Record the cards the opponent showed in a manual game. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn set_manual_reveals(
    app: AppHandle,
    token: String,
    game_number: u32,
    cards: Vec<ManualRevealInput>,
) -> Result<ManualEditView, CommandError> {
    let _timer = Timer::start("set_manual_reveals");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_reveals(ready(&startup)?, &token, game_number, &cards)
    })
    .await
}

/// What the "Edit match" sheet opens with for match row `match_id`.
#[tauri::command]
#[specta::specta]
pub fn match_edit_view(
    match_id: u32,
    startup: State<'_, Startup>,
) -> Result<MatchEditView, CommandError> {
    let _timer = Timer::start("match_edit_view");
    edit_form(&lock(&ready(&startup)?.reader), match_id)
}

/// "Edit match": fix any match in place. Async, as above. The tracker
/// pushes `DecksChanged` and `LeaguesChanged` once committed (which also
/// refresh the overlays), and a manual match's new revision.
#[tauri::command]
#[specta::specta]
pub async fn correct_match(
    app: AppHandle,
    match_id: u32,
    correction: MatchCorrectionInput,
) -> Result<ManualEditView, CommandError> {
    let _timer = Timer::start("correct_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        correct(
            ready(&startup)?,
            match_id,
            &correction,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// The rank gaps an Arena deck's manual match can be claimed into (none for
/// an MTGO deck), newest first.
pub fn rank_gap_options(
    conn: &Connection,
    deck_id: i64,
    tz: chrono_tz::Tz,
) -> Result<Vec<RankGapOption>, StoreError> {
    match arena_account_of_deck(conn, deck_id)? {
        Some(account_id) => played_elsewhere_gaps(conn, account_id, tz),
        None => Ok(Vec::new()),
    }
}

/// The "Rank gap" choices of the "Add manual match" sheet for `deck_id`.
#[tauri::command]
#[specta::specta]
pub fn manual_rank_gaps(
    deck_id: u32,
    startup: State<'_, Startup>,
) -> Result<Vec<RankGapOption>, CommandError> {
    let _timer = Timer::start("manual_rank_gaps");
    let state = ready(&startup)?;
    rank_gap_options(&lock(&state.reader), i64::from(deck_id), state.timezone).map_err(storage)
}

/// Cards whose name contains `q`, for the reveals picker.
#[tauri::command]
#[specta::specta]
pub fn card_search(
    q: String,
    startup: State<'_, Startup>,
) -> Result<Vec<CardSearchView>, CommandError> {
    let _timer = Timer::start("card_search");
    search(&lock(&ready(&startup)?.reader), &q)
}

#[cfg(test)]
mod tests;
