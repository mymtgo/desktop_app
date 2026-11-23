//! Enum values as stored in CHECK-constrained text columns.

use mtgo_match::{EndReason, GameResult, MatchState, Outcome};

pub fn state_str(state: MatchState) -> &'static str {
    match state {
        MatchState::InProgress => "in_progress",
        MatchState::Ended => "ended",
        MatchState::Complete => "complete",
        MatchState::Abandoned => "abandoned",
    }
}

pub fn outcome_str(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Win => "win",
        Outcome::Loss => "loss",
        Outcome::Draw => "draw",
        Outcome::Unknown => "unknown",
    }
}

pub fn game_result_str(result: GameResult) -> &'static str {
    match result {
        GameResult::Win => "win",
        GameResult::Loss => "loss",
        GameResult::Unknown => "unknown",
    }
}

pub fn end_reason_str(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Win => "win",
        EndReason::Concede => "concede",
        EndReason::Forfeit => "forfeit",
        EndReason::Unknown => "unknown",
    }
}

// ---- Decoders ------------------------------------------------------------------
// The reverse of the encoders above: `None` for text outside the column's
// CHECK set, so each caller picks its own fallback.

pub fn state_from_str(text: &str) -> Option<MatchState> {
    match text {
        "in_progress" => Some(MatchState::InProgress),
        "ended" => Some(MatchState::Ended),
        "complete" => Some(MatchState::Complete),
        "abandoned" => Some(MatchState::Abandoned),
        _ => None,
    }
}

pub fn outcome_from_str(text: &str) -> Option<Outcome> {
    match text {
        "win" => Some(Outcome::Win),
        "loss" => Some(Outcome::Loss),
        "draw" => Some(Outcome::Draw),
        "unknown" => Some(Outcome::Unknown),
        _ => None,
    }
}

pub fn game_result_from_str(text: &str) -> Option<GameResult> {
    match text {
        "win" => Some(GameResult::Win),
        "loss" => Some(GameResult::Loss),
        "unknown" => Some(GameResult::Unknown),
        _ => None,
    }
}

pub fn end_reason_from_str(text: &str) -> Option<EndReason> {
    match text {
        "win" => Some(EndReason::Win),
        "concede" => Some(EndReason::Concede),
        "forfeit" => Some(EndReason::Forfeit),
        "unknown" => Some(EndReason::Unknown),
        _ => None,
    }
}

/// `matches.state` for a pure crate's input. `pending` (0.x `Started`, the
/// one stored state `MatchState` lacks) reads as InProgress: every rule
/// treats the two alike.
pub fn stored_state(text: &str) -> MatchState {
    state_from_str(text).unwrap_or(MatchState::InProgress)
}

/// `matches.outcome` for a pure crate's input.
pub fn stored_outcome(text: &str) -> Outcome {
    outcome_from_str(text).unwrap_or(Outcome::Unknown)
}

/// `games.result` for a pure crate's input: a drawn game reads as Unknown
/// (neither won nor lost).
pub fn stored_game_result(text: &str) -> GameResult {
    game_result_from_str(text).unwrap_or(GameResult::Unknown)
}

/// `games.result` as won (`Some(true)`), lost (`Some(false)`) or not known
/// (`None`).
pub fn game_won(result: &str) -> Option<bool> {
    match game_result_from_str(result) {
        Some(GameResult::Win) => Some(true),
        Some(GameResult::Loss) => Some(false),
        Some(GameResult::Unknown) | None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoders_read_back_what_the_encoders_write() {
        for state in [
            MatchState::InProgress,
            MatchState::Ended,
            MatchState::Complete,
            MatchState::Abandoned,
        ] {
            assert_eq!(state_from_str(state_str(state)), Some(state));
        }
        for outcome in [Outcome::Win, Outcome::Loss, Outcome::Draw, Outcome::Unknown] {
            assert_eq!(outcome_from_str(outcome_str(outcome)), Some(outcome));
        }
        for result in [GameResult::Win, GameResult::Loss, GameResult::Unknown] {
            assert_eq!(game_result_from_str(game_result_str(result)), Some(result));
        }
        for reason in [
            EndReason::Win,
            EndReason::Concede,
            EndReason::Forfeit,
            EndReason::Unknown,
        ] {
            assert_eq!(end_reason_from_str(end_reason_str(reason)), Some(reason));
        }
    }

    #[test]
    fn decoders_refuse_text_outside_the_check_set() {
        assert_eq!(state_from_str("Complete"), None);
        assert_eq!(outcome_from_str("won"), None);
        assert_eq!(game_result_from_str(""), None);
        assert_eq!(end_reason_from_str("timeout"), None);
    }

    #[test]
    fn stored_readers_fall_back_for_text_outside_the_enums() {
        assert_eq!(stored_state("pending"), MatchState::InProgress);
        assert_eq!(stored_state("complete"), MatchState::Complete);
        assert_eq!(stored_outcome("draw"), Outcome::Draw);
        assert_eq!(stored_outcome(""), Outcome::Unknown);
        assert_eq!(stored_game_result("draw"), GameResult::Unknown);
        assert_eq!(stored_game_result("win"), GameResult::Win);
    }

    #[test]
    fn game_won_reads_only_win_and_loss() {
        assert_eq!(game_won("win"), Some(true));
        assert_eq!(game_won("loss"), Some(false));
        assert_eq!(game_won("unknown"), None);
        assert_eq!(game_won("draw"), None);
    }
}
