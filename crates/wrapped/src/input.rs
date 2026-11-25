//! What the builder reads: a plain copy of the counted year's rows. The store
//! fills it (`store::wrapped_read`); this crate never touches a database. Opponent
//! names, account names and match tokens never enter it.

use crate::bundle::{ArchetypeLook, Art, KeyCard, YearWindow};

/// The fewest counted matches a player needs before Wrapped exists at all.
pub const WRAPPED_FLOOR: u32 = 30;

/// Everything the builder needs for one player's year.
#[derive(Debug, Clone, PartialEq)]
pub struct WrappedInput {
    pub year: i32,
    pub window: YearWindow,
    pub built_at_ms: i64,
    /// The player's timezone: every hour, weekday and day comes from it.
    pub tz: chrono_tz::Tz,
    pub first_year: bool,
    pub matches: Vec<InputMatch>,
    /// Every deck a match used.
    pub decks: Vec<InputDeck>,
    /// Every opponent archetype faced.
    pub archetypes: Vec<InputArchetype>,
    /// Per deck and card, summed over the window's games.
    pub cards: Vec<InputCard>,
    /// Per deck, the baselines for `cards`.
    pub deck_games: Vec<InputDeckGames>,
    /// League and course runs started in the window.
    pub runs: Vec<InputRun>,
    /// Tournament standings finished in the window.
    pub finishes: Vec<InputFinish>,
    /// Limited runs in the window.
    pub drafts: Vec<InputDraft>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchResult {
    Win,
    Loss,
    Draw,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputMatch {
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// `decks::display_format`; "Limited" for every limited match.
    pub format_label: String,
    pub limited: bool,
    pub deck_id: Option<i64>,
    /// Unknown outcomes are not read at all.
    pub result: MatchResult,
    pub opponent_archetype_id: Option<i64>,
    /// Game rows, or the imported games won plus lost when there are none.
    pub game_count: u32,
    /// Empty for imported and history matches.
    pub games: Vec<InputGame>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputGame {
    pub won: Option<bool>,
    pub on_play: Option<bool>,
    pub mulligans: u32,
    pub turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputDeck {
    pub id: i64,
    pub name: String,
    pub format_label: String,
    pub colors: Vec<String>,
    pub art: Art,
    pub key_cards: Vec<KeyCard>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputArchetype {
    pub id: i64,
    pub name: String,
    pub look: ArchetypeLook,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputCard {
    pub deck_id: i64,
    pub name: String,
    pub is_land: bool,
    pub cast_games: u32,
    pub cast_won: u32,
    pub total_cast: u32,
    pub opening_hand_games: u32,
    pub sided_in_games: u32,
    pub art: Art,
    pub image: Art,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputDeckGames {
    pub deck_id: i64,
    pub games: u32,
    pub won: u32,
    pub opening_hand_games: u32,
    pub postboard_games: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputRun {
    pub deck_id: Option<i64>,
    pub limited: bool,
    pub wins: u32,
    pub losses: u32,
    pub trophy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputFinish {
    pub top_cut: bool,
    /// Like "Top 8, Modern Challenge, 14 Sep".
    pub text: String,
    pub finished_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputDraft {
    pub colour_pair: Option<String>,
    pub wins: u32,
    pub losses: u32,
}
