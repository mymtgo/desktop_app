//! What the projection produces.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchState {
    InProgress,
    Ended,
    Complete,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Win,
    Loss,
    Draw,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameResult {
    Win,
    Loss,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Win,
    Concede,
    Forfeit,
    Unknown,
}

/// How a terminal state was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Signal,
    Disconnect,
    Abandoned,
    /// Decided (an end signal, or a score at the win threshold) and then quiet
    /// for `ABANDON_AFTER_MS` without a completion signal (spec 5.5, issue #26).
    Quiet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameView {
    /// `None` only for a synthetic forfeit game, which MTGO never gave an id.
    pub mtgo_game_id: Option<u64>,
    pub game_number: u32,
    pub result: GameResult,
    pub end_reason: EndReason,
    pub on_play: Option<bool>,
    pub local_mulligans: Option<u32>,
    pub opp_mulligans: Option<u32>,
    pub local_die_roll: Option<u32>,
    pub opp_die_roll: Option<u32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchView {
    pub token: String,
    pub mtgo_match_id: Option<u64>,
    pub local_username: String,
    pub opponent: Option<String>,
    pub format: Option<String>,
    pub match_type: Option<String>,
    pub tournament_event_id: Option<u64>,
    pub tournament_round: Option<u32>,
    /// The league this match was played in (join block `League Token`).
    pub league_token: Option<String>,
    /// The join block's `GameStructureCd` (for example `Modern`), which names
    /// a league run.
    pub game_structure: Option<String>,
    pub state: MatchState,
    pub resolution: Option<Resolution>,
    /// Final outcome; `Unknown` until the match is complete.
    pub outcome: Outcome,
    /// Live hint once a side reaches the win threshold, before completion.
    pub tentative_outcome: Option<Outcome>,
    pub wins: u32,
    pub losses: u32,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub last_activity_ms: i64,
    pub games: Vec<GameView>,
}
