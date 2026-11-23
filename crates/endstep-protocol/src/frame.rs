//! One captured line: its arrival time and what it holds.

use serde_json::Value;

use crate::game::{self, GameAction, GameDelta, GameEvent, GameState};
use crate::rest::{self, DeckBody, LobbyBody, MatchBody, RankedResult};

/// A captured frame. `at` is the extension's arrival stamp, kept for display
/// only: order is the line order inside the unit, never `at`.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub at: i64,
    pub kind: FrameKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameKind {
    MatchBody(MatchBody),
    LobbyBody(LobbyBody),
    DeckBody(DeckBody),
    RankedResult(RankedResult),
    GameState(GameState),
    GameDelta(GameDelta),
    GameEvent(GameEvent),
    GameOver(GameState),
    /// An outgoing action of the user's.
    Action(GameAction),
    Other,
}

/// Decodes one frame line. Never fails: anything unknown is `Other`, and a
/// line that is not JSON (or has no integer `at`) has `at` 0.
pub fn decode_frame(line: &str) -> Frame {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            return Frame {
                at: 0,
                kind: FrameKind::Other,
            };
        }
    };
    let at = value.get("at").and_then(Value::as_i64).unwrap_or(0);
    Frame {
        at,
        kind: frame_kind(&value),
    }
}

/// Incoming frames carry the game; of the outgoing ones only the user's
/// actions on the game and lobby sockets are read, the rest is `Other`.
fn frame_kind(value: &Value) -> FrameKind {
    let Some(body) = value.get("body") else {
        return FrameKind::Other;
    };
    match value.get("dir").and_then(Value::as_str) {
        Some("in") => incoming_kind(value, body),
        Some("out") => outgoing_kind(value, body),
        _ => FrameKind::Other,
    }
}

fn outgoing_kind(value: &Value, body: &Value) -> FrameKind {
    let src = value.get("src").and_then(Value::as_str);
    if src != Some("game_ws") && src != Some("lobby_ws") {
        return FrameKind::Other;
    }
    match game::decode_action(body) {
        Some(action) => FrameKind::Action(action),
        None => FrameKind::Other,
    }
}

fn incoming_kind(value: &Value, body: &Value) -> FrameKind {
    let url = value.get("url").and_then(Value::as_str).unwrap_or("");
    match value.get("src").and_then(Value::as_str) {
        Some("rest") => rest::decode_rest(url, body),
        Some("game_ws") => game::decode_game_socket(body),
        Some("lobby_ws") => game::decode_lobby_socket(body),
        _ => FrameKind::Other,
    }
}
