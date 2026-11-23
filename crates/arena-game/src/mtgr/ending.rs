//! The end of a game (spec section 9.7): the concede or loss line, the
//! result and the termination, from the client's concede and the game's
//! `gameInfo.results`.

use ::mtgr::{Cause, GameResult, Statement};
use serde::Deserialize;

use super::{Ending, GameWriter, other_seat};
use crate::{GameInput, Source};

impl GameWriter<'_> {
    /// The end line and the result. A concede the client sent is already
    /// written; otherwise the game's result says who lost and why.
    pub(super) fn finish(&mut self, messages: &[GameInput], number: u32) -> Ending {
        let result = game_result(messages, number);
        let winner = result
            .as_ref()
            .filter(|result| result.result == "ResultType_WinLoss")
            .map(|result| result.winning_team_id);
        if let Some(seat) = self.conceded {
            let winner = winner.unwrap_or(other_seat(seat));
            return Ending {
                result: self.roster.result_for(winner),
                termination: Some("concession"),
            };
        }
        let Some(result) = result else {
            return Ending {
                result: GameResult::Unknown,
                termination: None,
            };
        };
        if result.result == "ResultType_Draw" {
            return Ending {
                result: GameResult::Draw,
                termination: None,
            };
        }
        let Some(winner) = winner else {
            return Ending {
                result: GameResult::Unknown,
                termination: None,
            };
        };
        let loser = other_seat(winner);
        let player = self.roster.name(loser);
        let termination = match result.reason.as_str() {
            "ResultReason_Concede" => {
                self.concede(loser);
                Some("concession")
            }
            "ResultReason_Game" if self.final_life(loser) <= 0 => {
                self.event(Statement::Lose { player }, Some(Cause::Rule("sba".into())));
                Some("life total")
            }
            "ResultReason_Timeout" => {
                self.event(Statement::Lose { player }, Some(Cause::Unknown));
                Some("time forfeit")
            }
            _ => {
                self.event(Statement::Lose { player }, Some(Cause::Unknown));
                None
            }
        };
        Ending {
            result: self.roster.result_for(winner),
            termination,
        }
    }

    fn final_life(&self, seat: u32) -> i32 {
        self.fold
            .states
            .last()
            .and_then(|state| state.players.get(&seat))
            .map(|player| player.life)
            .unwrap_or(1)
    }
}

/// One `gameInfo.results` entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawResult {
    #[serde(default)]
    scope: String,
    #[serde(default)]
    result: String,
    #[serde(default)]
    winning_team_id: u32,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawResultMessage {
    game_state_message: Option<RawResultState>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawResultState {
    game_info: Option<RawGameInfo>,
}

#[derive(Debug, Deserialize)]
struct RawGameInfo {
    #[serde(default)]
    results: Vec<RawResult>,
}

/// Game `number`'s result: entry `number` of the game results in the last
/// `gameInfo.results` the game's messages carry (Arena lists every game of
/// the match so far). In a two-player game a team is a seat.
fn game_result(messages: &[GameInput], number: u32) -> Option<RawResult> {
    let mut last: Vec<RawResult> = Vec::new();
    for input in messages {
        if input.source != Source::Gre {
            continue;
        }
        let Ok(message) = serde_json::from_str::<RawResultMessage>(&input.json) else {
            continue;
        };
        let results = message
            .game_state_message
            .and_then(|state| state.game_info)
            .map(|info| info.results)
            .unwrap_or_default();
        if !results.is_empty() {
            last = results;
        }
    }
    let index = usize::try_from(number).ok()?.checked_sub(1)?;
    last.into_iter()
        .filter(|result| result.scope == "MatchScope_Game")
        .nth(index)
}
