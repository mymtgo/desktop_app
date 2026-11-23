//! A game's log lines: the text of its `GAME_EVENT`s, from the same walk
//! that numbers the games in `fold_match`.

use endstep_protocol::{GameEvent, Unit};

use crate::fold::read_games;

/// Event kinds whose text is noise in a game log: phase steps, anonymous
/// zone moves and mana payments.
const LEFT_OUT_KINDS: [&str; 3] = ["TURN_PHASE", "CARD_ZONE_CHANGE", "MANA_LOG"];

/// One game log line: the event's text and its frame's arrival stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepLogLine {
    pub at_ms: i64,
    pub message: String,
}

/// The log lines of game `game_number` of the Forge game `forge_game_id`,
/// in event order. Units may come in any order. Callers pass game numbers
/// from the match view; another number may return lines of a record the view
/// drops (above the match's game limit).
pub fn game_log(forge_game_id: &str, units: &[Unit], game_number: u32) -> Vec<EndstepLogLine> {
    for game in read_games(forge_game_id, units) {
        if game.number == game_number {
            return game.log;
        }
    }
    Vec::new()
}

/// The line for an event with text, unless its kind is left out.
pub(crate) fn log_line(event: &GameEvent, at_ms: i64) -> Option<EndstepLogLine> {
    if LEFT_OUT_KINDS.contains(&event.kind.as_str()) {
        return None;
    }
    let message = event.message.as_ref()?;
    if message.trim().is_empty() {
        return None;
    }
    Some(EndstepLogLine {
        at_ms,
        message: message.clone(),
    })
}
