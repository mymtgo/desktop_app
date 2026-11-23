//! Tournament runs (0.x `FormatTournamentRuns` and `AssignTournament`'s
//! synthesized name).

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use decks::display_format;
use mtgo_match::{MatchState, Outcome};

use crate::fold::GameResult;
use crate::numbers::{
    GameMark, Mark, WinLoss, avg_match_seconds, duration_seconds, game_marks, game_stats,
};

/// One of the deck's matches in a tournament.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentMatch {
    pub token: String,
    pub state: MatchState,
    pub outcome: Outcome,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub deck_version_id: Option<i64>,
    /// `Round:M` of the match description.
    pub round: Option<i64>,
    /// Games in play order.
    pub games: Vec<GameResult>,
}

/// A tournament: the account's matches sharing `tournament_event_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentInput {
    pub event_id: i64,
    /// The matches' format code.
    pub format: Option<String>,
    /// The account's first match in the tournament (any deck).
    pub started_at: i64,
    /// The deck's matches in the tournament.
    pub matches: Vec<TournamentMatch>,
}

/// A deck version and when it was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeckVersionDate {
    pub id: i64,
    pub modified_at: i64,
}

/// One row of a tournament's rounds table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentMatchRow {
    pub token: String,
    pub state: MatchState,
    /// `None` until the match is complete (a live dot).
    pub result: Option<Mark>,
    pub games: Vec<GameMark>,
    pub started_at: i64,
    pub duration_seconds: Option<i64>,
    pub round: Option<i64>,
}

/// A tournament card on the deck's Tournaments tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentRun {
    pub event_id: i64,
    /// `Tournament {eventId} ({d-m-Y})` until names come from the API (#7).
    pub name: String,
    /// The format label (`Modern`), empty when unknown.
    pub format: String,
    pub started_at: i64,
    /// `vN` or `vN\u{2013}vM`.
    pub version_label: Option<String>,
    /// Complete matches only, not padded.
    pub results: Vec<Mark>,
    pub matches: Vec<TournamentMatchRow>,
    pub avg_match_seconds: Option<i64>,
    pub game_wins: u32,
    pub game_losses: u32,
    pub on_play: WinLoss,
    pub on_draw: WinLoss,
    pub matches_count: u32,
    pub in_progress_count: u32,
    pub wins: u32,
    pub losses: u32,
}

/// The synthesized tournament name (0.x `AssignTournament::synthesizeName`):
/// `Tournament {eventId} ({d-m-Y})` with the local day of `started_at`.
pub fn tournament_name(event_id: i64, started_at: i64, tz: Tz) -> String {
    let day = DateTime::<Utc>::from_timestamp_millis(started_at)
        .unwrap_or_default()
        .with_timezone(&tz)
        .format("%d-%m-%Y");
    format!("Tournament {event_id} ({day})")
}

/// One tournament on a deck's Tournaments tab (0.x
/// `FormatTournamentRuns::formatRun`). `input.matches` are the deck's matches
/// in the tournament, in any state; `versions` are the deck's versions.
/// Results, W-L, the average duration and game numbers count complete
/// matches; the match rows list every match in start order.
pub fn tournament_run(
    input: &TournamentInput,
    versions: &[DeckVersionDate],
    tz: Tz,
) -> TournamentRun {
    let mut matches: Vec<&TournamentMatch> = input.matches.iter().collect();
    matches.sort_by_key(|played| played.started_at);
    let completed: Vec<&TournamentMatch> = matches
        .iter()
        .copied()
        .filter(|played| played.state == MatchState::Complete)
        .collect();

    let rows = matches
        .iter()
        .map(|played| TournamentMatchRow {
            token: played.token.clone(),
            state: played.state,
            result: (played.state == MatchState::Complete).then(|| mark(played)),
            games: game_marks(&played.games),
            started_at: played.started_at,
            duration_seconds: duration_seconds(played.started_at, played.ended_at),
            round: played.round,
        })
        .collect();

    let spans: Vec<(i64, Option<i64>)> = completed
        .iter()
        .map(|played| (played.started_at, played.ended_at))
        .collect();
    let games: Vec<&GameResult> = completed.iter().flat_map(|played| &played.games).collect();
    let stats = game_stats(&games);

    TournamentRun {
        event_id: input.event_id,
        name: tournament_name(input.event_id, input.started_at, tz),
        format: display_format(input.format.as_deref().unwrap_or("")),
        started_at: input.started_at,
        version_label: version_span(&matches, versions),
        results: completed.iter().map(|played| mark(played)).collect(),
        matches: rows,
        avg_match_seconds: avg_match_seconds(&spans),
        game_wins: stats.game_wins,
        game_losses: stats.game_losses,
        on_play: stats.on_play,
        on_draw: stats.on_draw,
        matches_count: matches.len() as u32,
        in_progress_count: (matches.len() - completed.len()) as u32,
        wins: count_outcome(&completed, Outcome::Win),
        losses: count_outcome(&completed, Outcome::Loss),
    }
}

fn mark(played: &TournamentMatch) -> Mark {
    if played.outcome == Outcome::Win {
        Mark::W
    } else {
        Mark::L
    }
}

fn count_outcome(matches: &[&TournamentMatch], outcome: Outcome) -> u32 {
    matches
        .iter()
        .filter(|played| played.outcome == outcome)
        .count() as u32
}

/// `vN`, or `vN\u{2013}vM` (0.x's en dash) when the matches used several
/// versions. A version's number is its place among the deck's versions by
/// `modified_at` (then id), from 1. `None` when no match has a known version.
fn version_span(matches: &[&TournamentMatch], versions: &[DeckVersionDate]) -> Option<String> {
    let mut ordered: Vec<DeckVersionDate> = versions.to_vec();
    ordered.sort_by_key(|version| (version.modified_at, version.id));

    let mut indexes: Vec<usize> = Vec::new();
    for played in matches {
        let Some(version_id) = played.deck_version_id else {
            continue;
        };
        if let Some(position) = ordered.iter().position(|version| version.id == version_id) {
            indexes.push(position + 1);
        }
    }
    let low = indexes.iter().min()?;
    let high = indexes.iter().max()?;
    if low == high {
        Some(format!("v{low}"))
    } else {
        Some(format!("v{low}\u{2013}v{high}"))
    }
}
