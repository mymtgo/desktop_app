//! Pure parsing of Endstep captures: the frames and REST bodies the browser
//! extension stored in a capture unit, decoded into typed values, and the
//! refs that link a unit to a match. No IO and no clock access.
//!
//! Endstep is hostile input: a frame that does not decode becomes
//! `FrameKind::Other`, never an error. Only a bad unit header is an error.

pub mod frame;
pub mod game;
mod json;
pub mod refs;
pub mod rest;

pub use frame::{Frame, FrameKind, decode_frame};
pub use game::{
    GameAction, GameDelta, GameEvent, GameState, MAX_PROMPT_CARDS, PlayerState, Prompt, PromptCard,
    StackEntry, StateCard, Zone,
};
pub use refs::unit_refs;
pub use rest::{DeckBody, DeckLine, LobbyBody, MatchBody, Participant, RankedResult};

/// One capture unit: its header fields and its frames in line order.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub unit_id: String,
    pub started_at: i64,
    pub counter: i64,
    pub frames: Vec<Frame>,
}

/// A card printing as Endstep names it: name, set code and collector number.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EndstepCardKey {
    pub name: String,
    pub set_code: String,
    pub collector_number: String,
}

/// Decodes a capture unit. The header must pass `endstep_capture::parse_capture`;
/// every other line becomes a frame (`FrameKind::Other` when it does not decode).
pub fn decode_unit(text: &str) -> Result<Unit, String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let parsed = endstep_capture::parse_capture(first).map_err(|error| error.to_string())?;

    let mut frames = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        frames.push(decode_frame(line));
    }
    Ok(Unit {
        unit_id: parsed.header.unit_id,
        started_at: parsed.header.started_at,
        counter: parsed.header.counter,
        frames,
    })
}
