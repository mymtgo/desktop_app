//! Pure Endstep projection: one match folded from its capture units into the
//! shared `MatchView`, plus the account, opponent, deck and ranked result,
//! and a match's units trimmed to that match for cross-device sync
//! (`trim_units`). No IO and no clock access: `now_ms` is a parameter.
//!
//! Order is unit order `(started_at, counter)` then line order, never a
//! frame time, so folding the same units in any order gives the same match.

mod cards;
mod deck;
mod fold;
mod games;
mod hands;
mod log;
mod mtgr;
mod sideboard;
mod trim;

use endstep_protocol::{EndstepCardKey, RankedResult};
use game_detail::GameCardRow;

pub use fold::fold_match;
pub use log::{EndstepLogLine, game_log};
pub use mtgr::{ReplayNames, match_record};
pub use trim::{TrimmedUnit, trim_units};

/// The most games a match can have. Endstep's `gamesPerMatch` is clamped to
/// 1 to this, and a game number above the limit is hostile input.
pub const MAX_GAMES: u32 = 7;

/// An unfinished match quiet this long is abandoned.
pub const QUIET_MS: i64 = 30 * 60 * 1000;

/// What folding a match's units gives.
#[derive(Debug, Clone, PartialEq)]
pub enum Folded {
    Match(Box<EndstepMatch>),
    Dropped(DropReason),
}

/// Why a match is not imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// No `GET /api/matches/<id>` body for this Forge game.
    NoMatchBody,
    /// A participant is a bot or the AI.
    Bot,
    /// The user is not seated (`myPlayerIndex` missing or out of range).
    Spectator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndstepMatch {
    pub view: mtgo_match::MatchView,
    pub account: Person,
    pub opponent: Option<Person>,
    pub deck: Option<EndstepDeck>,
    pub ranked: Option<RankedResult>,
    /// The card rows of each game in `view.games`, in game order.
    pub game_cards: Vec<EndstepGameCards>,
    pub forge_game_id: String,
}

/// One game's `game_cards` rows, each keyed by its Endstep card. Rows are
/// sorted by side, then key, and have `catalog_id` 0: the store maps the
/// keys to catalog ids.
///
/// Local rows: every card of the match deck, plus any of the user's cards
/// seen outside it (`main_qty` 0). Their quantities are the game's own deck
/// (the answered sideboard prompt, see `sideboard.rs`), with `sided_in` and
/// `sided_out` against game 1's deck. They carry the opening hand, bottomed
/// and starting hand counts by card name, and their zone counters
/// (`has_zone_data`) and `seen` count from the kept hand's state on (every
/// state when no keep was captured). Opponent rows: one per card name the
/// opponent showed in a state, a reveal, a cast, a land play or a named move
/// out of the hand, keyed by name only. `seen` counts distinct cards; `cast`,
/// `played` and `activated` count that side's `SPELL_CAST`, `LAND_PLAYED` and
/// `ABILITY_ACTIVATED` events. Tokens, face-down cards and library cards make
/// no rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepGameCards {
    pub game_number: u32,
    pub rows: Vec<(EndstepCardKey, GameCardRow)>,
    /// The user's opening hand (the last hand drawn, kept) and the starting
    /// hand (the opening hand minus the bottomed cards), `None` when the
    /// game's frames did not show the keep.
    pub opening_hand_size: Option<u32>,
    pub starting_hand_size: Option<u32>,
    /// The opponent's, from its `HAND_SETTLED` (hidden cards are never named).
    pub opp_opening_hand_size: Option<u32>,
    pub opp_starting_hand_size: Option<u32>,
    /// The user's hands sent back: (hand number from 1, card, quantity),
    /// sorted.
    pub mulligans: Vec<(u32, EndstepCardKey, u32)>,
}

/// An Endstep user: their user id and Endstep username.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub user_id: String,
    pub username: String,
}

/// The deck the user played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepDeck {
    /// `endstep:<deck id>`, or `endstep:match:<match id>` when the deck body
    /// was not captured.
    pub key: String,
    pub name: String,
    pub format: String,
    pub main: Vec<(EndstepCardKey, u32)>,
    pub side: Vec<(EndstepCardKey, u32)>,
}

/// The key of the deck a match gets when its deck body was not captured
/// (`endstep:match:<match id>`; the match id is the match's token). A refold
/// that finds the deck body leaves it unused (#867).
pub fn match_deck_key(match_id: &str) -> String {
    format!("endstep:match:{match_id}")
}

/// The account and opponent username MyMTGO stores for an Endstep user.
pub fn account_key(user_id: &str) -> String {
    format!("endstep:{user_id}")
}
