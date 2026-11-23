//! Turning scanned history matches into stored matches (Plan M, 0.x
//! `ImportMatches::runFromScan`). With a paired game log a match gets its
//! games from the log (`game_detail::extract_game_results`) and the log's
//! token, so the match page finds the log again; without one it has no
//! games and keeps the history's game counts.

use std::path::Path;

use chrono_tz::Tz;
use game_detail::{
    GameLogEntry, LogEndReason, PlayerValue, extract_game_results, extract_log_cards,
    parse_game_log,
};
use serde::{Deserialize, Serialize};
use store::history_import::{HistoryGame, HistoryMatch};

use crate::history_import::scan::HistoryScanMatch;

/// The scan's opponent when the history named none.
const NO_OPPONENT: &str = "Unknown";

/// Matches sent to the tracker at a time, like the 0.x import.
pub const BATCH_SIZE: usize = 200;

/// What an import did (0.x `imported` and `skipped`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryImportResult {
    pub imported: u32,
    /// Already stored (by MTGO match id or token), or without a start time
    /// or an account to store it under.
    pub skipped: u32,
}

/// "Import all" takes the rows through the page's filters (0.x
/// `ImportAllController`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryImportFilters {
    /// A display format; blank for all.
    pub format: Option<String>,
    /// Above 0, only scored rows at least this confident.
    pub min_confidence: Option<f64>,
}

/// A game log file's entries, empty when it cannot be read.
pub fn read_log(path: &Path, tz: Tz) -> Vec<GameLogEntry> {
    match std::fs::read(path) {
        Ok(bytes) => parse_game_log(&bytes, tz)
            .map(|log| log.entries)
            .unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, "could not read a paired game log");
            Vec::new()
        }
    }
}

/// The value of `player` in `values`, if any.
fn value_of(values: &[PlayerValue], player: &str) -> Option<i64> {
    values
        .iter()
        .find(|kept| kept.player == player)
        .map(|kept| i64::from(kept.value))
}

/// The games of a paired log, scored for `local` (0.x `runFromScan`'s game
/// map). `game_ids` are the history's `GameIds`, by game.
pub fn games_from_log(entries: &[GameLogEntry], local: &str, game_ids: &[u64]) -> Vec<HistoryGame> {
    let results = extract_game_results(entries, local);
    let cards = extract_log_cards(entries);
    let opponent = results
        .players
        .iter()
        .find(|player| player.as_str() != local)
        .cloned();
    let mut games = Vec::new();
    for (index, game) in results.games.iter().enumerate() {
        // A trailing forfeit has no game of its own in the card read.
        let mut mulligans: &[PlayerValue] = &[];
        let mut dice_rolls: &[PlayerValue] = &[];
        let mut turn_count = None;
        if let Some(read) = cards.games.get(game.game_index) {
            mulligans = &read.meta.mulligans;
            dice_rolls = &read.meta.dice_rolls;
            turn_count = read.meta.turn_count;
        }
        let opponent_value = |values: &[PlayerValue]| {
            opponent
                .as_deref()
                .and_then(|player| value_of(values, player))
        };
        let result = match &game.winner {
            Some(winner) if winner == local => "win",
            Some(_) => "loss",
            None => "unknown",
        };
        let end_reason = match game.end_reason {
            LogEndReason::Win => "win",
            LogEndReason::Concede => "concede",
            LogEndReason::Unknown => "unknown",
        };
        games.push(HistoryGame {
            mtgo_game_id: game_ids.get(index).and_then(|id| i64::try_from(*id).ok()),
            game_number: u32::try_from(index + 1).unwrap_or(u32::MAX),
            result: result.to_string(),
            end_reason: end_reason.to_string(),
            on_play: game.on_play.as_ref().map(|player| player == local),
            // 0.x counted no mulligan line as none.
            local_mulligans: Some(value_of(mulligans, local).unwrap_or(0)),
            opp_mulligans: Some(opponent_value(mulligans).unwrap_or(0)),
            local_die_roll: value_of(dice_rolls, local),
            opp_die_roll: opponent_value(dice_rolls),
            turn_count: turn_count.map(i64::from),
            started_at_ms: game.started_at_ms,
            ended_at_ms: game.ended_at_ms,
        });
    }
    games
}

/// The match to store for `row`, linked to `deck_version_id`. `None` when it
/// has no start time, or no account to store it under (no paired log and no
/// `fallback_account`).
pub fn history_match(
    row: &HistoryScanMatch,
    deck_version_id: i64,
    fallback_account: Option<&str>,
    tz: Tz,
) -> Option<HistoryMatch> {
    let started_at_ms = row.started_at_ms?;
    let local_username = row
        .local_player
        .clone()
        .or_else(|| fallback_account.map(str::to_string))?;
    // 0.x read the log only with a local player, and kept its games only
    // when it had entries.
    let mut games = Vec::new();
    let mut log = Vec::new();
    if let (Some(path), Some(local)) = (&row.game_log_path, &row.local_player) {
        let entries = read_log(Path::new(path), tz);
        if !entries.is_empty() {
            games = games_from_log(&entries, local, &row.game_ids);
            log = entries;
        }
    }
    let ended_at_ms = games.iter().rev().find_map(|game| game.ended_at_ms);
    let opponent =
        (row.opponent != NO_OPPONENT && !row.opponent.is_empty()).then(|| row.opponent.clone());
    let format = (!row.format.is_empty()).then(|| row.format.clone());
    Some(HistoryMatch {
        token: row.game_log_token.clone(),
        mtgo_match_id: i64::try_from(row.history_id).ok()?,
        local_username,
        opponent,
        format,
        match_type: if row.round > 0 {
            "League"
        } else {
            "Constructed"
        }
        .to_string(),
        outcome: row.outcome.as_str().to_string(),
        started_at_ms,
        ended_at_ms,
        games_won: row.games_won,
        games_lost: row.games_lost,
        games,
        deck_version_id,
        // Stored in `game_logs`; the games' card rows are read from it.
        log,
    })
}

#[cfg(test)]
mod tests;
