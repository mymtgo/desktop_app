//! Overlay rules ported from the 0.x app (overlay spec sections 5 and 7).
//!
//! - `draw_odds`: what is left in the local library (0.x `ComputeDrawOdds`).
//! - `game_info`: whether the play or draw is still being chosen.
//! - `reveals`: the opponent's revealed cards across a match's games
//!   (0.x `AggregateOpponentCards`, `GetOpponentReveals`) and their
//!   fingerprint.
//! - `sideboarding`: whether the player is between games, by log position
//!   (0.x `DetectSideboarding`, `DecideMatchPhase`).
//! - `height`: the game overlay's window heights (0.x
//!   `ComputeGameOverlayHeight`).
//! - `visibility`: which overlay windows are wanted, and the flip memo that
//!   keeps a window call from repeating (0.x `SyncGameOverlayVisibility`,
//!   `SyncDraftNotesWindowVisibility`).
//! - `screen`: where a window reopens (its saved bounds, or centred).
//! - `platform`: whether overlay windows are available at all (not on a
//!   native Wayland backend).
//! - `league_input`: what the league card is built from (the store's read).
//! - `league`: the league card's state (0.x `BuildOverlayState`).
//! - `art`: the league card's artwork mode and image (0.x
//!   `ResolveOverlayArt`).
//! - `publish`: the hosted OBS overlay's payload, failure reason, size and
//!   URL (0.x `PublishOverlayState`).
//! - `options`: the setting values the rules read (artwork, size, publish
//!   error).
//! - `opponent`: which source names the opponent's archetype, its label and
//!   the head-to-head text (0.x `ResolveOverlayOpponent`).
//! - `potential`: cards the opponent's archetype could still have (0.x
//!   `GetArchetypePotentialCards`).
//! - `sideboard`: the sideboard guide's rows in Plan and History scope,
//!   without community rates (0.x `BuildSideboardGuide`).
//!
//! Pure: no filesystem, clock, network, database or environment reads.
//! Callers pass in frames, card rows, settings and environment values.

pub mod art;
pub mod draw_odds;
pub mod event_choice;
pub mod game_info;
pub mod height;
pub mod league;
pub mod league_input;
pub mod opponent;
pub mod options;
pub mod platform;
pub mod potential;
pub mod publish;
pub mod reveals;
pub mod screen;
pub mod sideboard;
pub mod sideboarding;
pub mod size;
pub mod visibility;

use std::collections::BTreeMap;

pub use art::{art_mode, window_art};
pub use draw_odds::{DrawOdds, DrawOddsCard, draw_odds};
pub use event_choice::{Candidate, Choice, EventKey, choose_event};
pub use game_info::before_play_draw;
pub use league::{
    CardLook, EventKind, GamePip, GameRecord, LeagueDeck, LeagueEvent, LeagueMatch,
    LeagueOverlayState, LeagueProgress, LeagueRecord, LeagueStatus, LiveLife, OtherEvent,
    OtherMatch, OverlayArt, STALE_AFTER_MS, TournamentBlock, TournamentSeries, TournamentStage,
    UNPAIRED_AFTER_START_MS, changes_at, deck_payload, league_state, live_tokens,
    shown_live_match_id, shown_live_match_token, tournament_series, wubrg,
};
pub use league_input::{
    DEFAULT_FINISHED_MINUTES, GRACE_MS, LeagueCardInput, LeagueMatchInput, LeagueRunInput,
    MAX_FINISHED_MINUTES, MIN_FINISHED_MINUTES, OverlayDeck, TournamentInput, TournamentMatchInput,
    finished_ms,
};
pub use opponent::{
    ArchetypeRef, LIVE_CONFIDENCE_THRESHOLD, LiveEstimate, OpponentSources, Resolved, Source,
    head_to_head_text, resolve_opponent,
};
pub use options::{
    LeagueDisplay, LeagueGames, LeagueTheme, OverlayArtwork, OverlaySize, PublishError,
};
pub use platform::{Os, SessionEnv, overlay_windows_available};
pub use potential::{ListEntry, PotentialCard, PotentialCards, potential_cards};
pub use publish::{CARD_PADDING, obs_size_with_strip, public_url, publish_error, publish_state};
pub use reveals::{
    ESTIMATE_GAP_MS, ESTIMATE_MIN_NEW_CARDS, RevealedCard, aggregate_reveals, estimate_due,
    fingerprint, merge_by_name, new_cards,
};
pub use screen::{Placement, Rect, on_any_monitor, placement};
pub use sideboard::{
    GuideStats, LOCAL_MIN_GAMES, RECOMMEND_SHARE, SampleRate, SideboardGuide, SidedInCard,
    SidedOutCard, sideboard_rows,
};
pub use sideboarding::{MatchPhase, Position, is_sideboarding, match_phase};
pub use size::{card_and_strip_size, card_size, strip_height};
pub use visibility::{
    Action, LiveMatch, Memo, draft_window_wanted, game_overlay_wanted, league_window_wanted,
};

/// What the overlay knows about one printing (a `cards` row). Every field
/// is optional because a card stub may carry nothing but its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardInfo {
    pub name: Option<String>,
    pub oracle_id: Option<String>,
    pub card_type: Option<String>,
    pub color_identity: Option<String>,
    /// The Scryfall image URL.
    pub image: Option<String>,
    /// The Scryfall art crop URL.
    pub art_crop: Option<String>,
    /// The image file in v1's card images folder (`cards.local_image`).
    pub local_image: Option<String>,
    /// The art crop file in v1's card images folder (`cards.local_art_crop`).
    pub local_art_crop: Option<String>,
    /// The mana cost (`cards.mana_cost`): a live read names the opponent's
    /// colours from it.
    pub mana_cost: Option<String>,
}

/// Card rows by catalog id. The caller loads the rows of the deck's
/// printings and of every printing a frame or a reveal names, so ids the deck
/// does not hold can still be matched by oracle id or face name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardMeta {
    pub cards: BTreeMap<u32, CardInfo>,
}

impl CardMeta {
    /// The row of one printing, if the caller loaded it.
    pub fn get(&self, catalog_id: u32) -> Option<&CardInfo> {
        self.cards.get(&catalog_id)
    }

    /// The name of one printing, if known.
    pub fn name(&self, catalog_id: u32) -> Option<&str> {
        self.get(catalog_id)?.name.as_deref()
    }

    /// The oracle id of one printing, if known.
    pub fn oracle_id(&self, catalog_id: u32) -> Option<&str> {
        self.get(catalog_id)?.oracle_id.as_deref()
    }

    /// The art crop of one printing, if known.
    pub fn art_crop(&self, catalog_id: u32) -> Option<&str> {
        self.get(catalog_id)?.art_crop.as_deref()
    }
}

/// The name shown for a printing: its card name, else `#{catalog_id}` (0.x).
pub fn display_name(cards: &CardMeta, catalog_id: u32) -> String {
    match cards.name(catalog_id) {
        Some(name) => name.to_string(),
        None => format!("#{catalog_id}"),
    }
}
