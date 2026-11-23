//! What the league card is built from (overlay spec section 9.1): the runs
//! that could be on the card, each with its matches, games and deck, and the
//! last played deck for the idle card. Read by
//! `store::overlay::league_card_inputs`; the card's rules (which run, its
//! status and numbers) are applied to it here, in pure code.

use leagues::standing::{StandingFinish, StandingStage, StandingStatus};
use leagues::{LeagueKind, RunState};
use mtgo_match::{MatchState, Outcome};

use crate::sideboarding::Position;

/// How long a complete or dropped run stays on the card after it ended, by
/// default (0.x `BuildOverlayState::GRACE_MINUTES`, 5 minutes). The
/// `overlay_finished_minutes` setting chooses it, from
/// `MIN_FINISHED_MINUTES` to `MAX_FINISHED_MINUTES`.
pub const DEFAULT_FINISHED_MINUTES: u32 = 5;
pub const MIN_FINISHED_MINUTES: u32 = 1;
pub const MAX_FINISHED_MINUTES: u32 = 60;

/// The default time on the card in milliseconds.
pub const GRACE_MS: i64 = DEFAULT_FINISHED_MINUTES as i64 * 60 * 1000;

/// A finished run's time on the card in milliseconds, the minutes clamped
/// to the allowed range.
pub fn finished_ms(minutes: u32) -> i64 {
    i64::from(minutes.clamp(MIN_FINISHED_MINUTES, MAX_FINISHED_MINUTES)) * 60 * 1000
}

/// Everything the league card needs, read in a bounded number of queries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeagueCardInput {
    /// The account's runs that can be on the card: every `active` run with
    /// at least one listed match, and every `complete` or `dropped` run with
    /// listed matches that ended within the finished time of `now_ms`. Newest
    /// `started_at` first. `partial` runs are never read.
    pub runs: Vec<LeagueRunInput>,
    /// The deck of the most recently started match that has a deck version
    /// (0.x `FindLastPlayedDeck`), for the idle card.
    pub last_played_deck: Option<OverlayDeck>,
    /// The local player's username, read only when a run has an in-progress
    /// match (to find the local seat in its frames).
    pub local_username: Option<String>,
    /// The account's stored tournaments (`tournament_standings`) that can be
    /// on the card, with their matches and decks (event overlay spec 6).
    pub tournaments: Vec<TournamentInput>,
}

/// One stored league run (`leagues` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueRunInput {
    pub id: i64,
    pub kind: LeagueKind,
    /// The matches its league's panel states (`leagues.rounds`); `None`
    /// takes the kind's default.
    pub rounds: Option<usize>,
    /// Active, Complete or Dropped (Partial runs are never read).
    pub state: RunState,
    /// As stored (a minted name keeps its date suffix here).
    pub name: String,
    /// The format code (`CMODERN`), if known.
    pub format: Option<String>,
    pub started_at_ms: i64,
    /// When the run ended as the card shows it (a status the player set
    /// included): the finished time counts from it.
    pub completed_at_ms: Option<i64>,
    pub dropped_at_ms: Option<i64>,
    /// When the league fold itself ended the run (`leagues.completed_at` or
    /// `dropped_at`, the later): a real transition (event overlay spec 6).
    /// A status the player set is not one, so marking an old run Complete
    /// never takes the card from an active run.
    pub fold_ended_at_ms: Option<i64>,
    /// The run's listed matches (hidden and detached left out), abandoned
    /// ones included, oldest `started_at` first.
    pub matches: Vec<LeagueMatchInput>,
    /// The run's deck: `leagues.deck_id`, else the deck of its version, else
    /// the deck of its first non-abandoned match with a version.
    pub deck: Option<OverlayDeck>,
}

/// One match of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueMatchInput {
    pub id: i64,
    pub token: String,
    /// `matches.state`. The store reads a `pending` row as InProgress: both
    /// are live (0.x `Started` and `InProgress`).
    pub state: MatchState,
    pub outcome: Outcome,
    pub started_at_ms: i64,
    /// `matches.ended_at`: its end is a real transition (event overlay
    /// spec 6).
    pub ended_at_ms: Option<i64>,
    /// Each game by game number: won (`Some(true)`), lost (`Some(false)`),
    /// or neither (`None`: a draw or an unknown result).
    pub games: Vec<Option<bool>>,
    /// For an `in_progress` match only (empty otherwise): its
    /// `match_state_changed` events that name a sideboarding state, in log
    /// order (`store::overlay::sideboarding_events`).
    pub sideboarding_events: Vec<(Position, String)>,
    /// For an `in_progress` match only: the log position of the first state
    /// frame of its newest game, if that game has one.
    pub newest_game_start: Option<Position>,
    /// For the live match the card shows only (`shown_live_match_id`): its
    /// newest game's first and newest state frames, raw
    /// (`store::overlay::first_frame`, `latest_frame`).
    pub first_frame: Option<String>,
    pub newest_frame: Option<String>,
}

/// The card's deck block before it is shaped for the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayDeck {
    pub id: i64,
    pub name: String,
    /// The deck's archetype name (`decks.archetype_id`), `None` before
    /// detection: the card then shows the deck as not yet detected (spec
    /// 9.1, "null before detection").
    pub archetype: Option<String>,
    /// `decks.color_identity` as stored.
    pub color_identity: Option<String>,
    /// `decks.overlay_label`: the streamer's name for the deck.
    pub label: Option<String>,
    /// The cover card's Scryfall art crop URL.
    pub cover_art_crop: Option<String>,
    /// The cover's art crop file in v1's card images folder.
    pub cover_local_art_crop: Option<String>,
    /// The deck's own cover image, a file in the deck covers folder. The
    /// local card shows it before the cover card's art.
    pub cover_file: Option<String>,
}

/// One stored tournament standing of the account (`tournament_standings`,
/// the standing fold's output) and what the card needs beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentInput {
    /// The MTGO event id (the standing's key).
    pub mtgo_event_id: i64,
    /// The event name from the log (`Legacy Showcase Challenge`).
    pub name: Option<String>,
    /// The format code (`CLEGACY`).
    pub format: Option<String>,
    /// Never `Unregistered`: such an event has no stored standing.
    pub status: StandingStatus,
    /// `Some` exactly when `status` is `Finished`.
    pub finish: Option<StandingFinish>,
    /// The current round (playoff rounds continue the count).
    pub round: Option<u32>,
    pub stage: StandingStage,
    pub swiss_rounds: Option<u32>,
    pub cut_size: Option<u32>,
    pub rank: Option<u32>,
    pub players: Option<u32>,
    /// Open matches of the current round, every player's.
    pub matches_left: Option<u32>,
    pub byes: u32,
    pub place: Option<u32>,
    pub starts_at_ms: Option<i64>,
    /// When the current round's result was logged.
    pub round_result_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    /// The newest real transition and the newest fact of any kind, log
    /// times.
    pub last_transition_at_ms: i64,
    pub last_event_at_ms: i64,
    /// The account's matches of the tournament, oldest first.
    pub matches: Vec<TournamentMatchInput>,
    /// The deck registered for the tournament (`match_deck_registered` with
    /// its token), when known.
    pub registered_deck: Option<OverlayDeck>,
    /// The last played deck of the tournament's format.
    pub format_deck: Option<OverlayDeck>,
}

/// One of the account's matches in a tournament.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentMatchInput {
    /// `matches.tournament_round`.
    pub round: Option<u32>,
    /// The match's deck.
    pub deck: Option<OverlayDeck>,
    pub member: LeagueMatchInput,
}
