//! One match's capture units trimmed to that match (cross-device sync spec
//! section 5.4): what another device needs to fold, log and replay the
//! match, and nothing of another match that shares a unit.
//!
//! A line is kept by the same rules `fold_match` reads it by:
//! - the unit's header line (the first);
//! - match bodies of this Forge game;
//! - lobby bodies whose id one of those match bodies names;
//! - deck bodies whose id one of those lobbies seats;
//! - ranked results whose db id one of those match bodies names;
//! - game states, game overs and game events of this Forge game;
//! - game deltas and outgoing actions while the unit's last state or event
//!   was of this game (they name no game: `read_frames` ties them so).
//!
//! Every other line (site lists, queue, account and chat frames, frames of
//! other games) is dropped, and a unit left with no frame line is dropped
//! whole. Kept lines keep their order and their bytes.

use std::collections::BTreeSet;

use endstep_protocol::{FrameKind, Unit, decode_unit};

/// One unit trimmed to the match: its id and its kept lines, the header
/// first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimmedUnit {
    pub unit_id: String,
    pub lines: Vec<String>,
}

/// The ids that tie REST bodies to the match.
#[derive(Default)]
struct Links {
    lobby_ids: BTreeSet<String>,
    db_ids: BTreeSet<String>,
    deck_ids: BTreeSet<String>,
}

/// The units of the Forge game `forge_game_id` (their capture texts, in
/// fold order), each trimmed to the match, in the order given. A text that
/// does not decode is left out, as `decoded_unit` leaves it out of a fold.
pub fn trim_units(forge_game_id: &str, texts: &[String]) -> Vec<TrimmedUnit> {
    let mut decoded: Vec<(&String, Unit)> = Vec::new();
    for text in texts {
        if let Ok(unit) = decode_unit(text) {
            decoded.push((text, unit));
        }
    }
    let links = links_of(forge_game_id, &decoded);
    let mut out = Vec::new();
    for (text, unit) in &decoded {
        let lines = kept_lines(forge_game_id, text, unit, &links);
        // The header alone tells another device nothing.
        if lines.len() > 1 {
            out.push(TrimmedUnit {
                unit_id: unit.unit_id.clone(),
                lines,
            });
        }
    }
    out
}

/// The lobby and db ids this game's match bodies name, then the decks
/// those lobbies seat (any unit: bodies can arrive in any order).
fn links_of(forge_game_id: &str, units: &[(&String, Unit)]) -> Links {
    let mut links = Links::default();
    for (_, unit) in units {
        for frame in &unit.frames {
            if let FrameKind::MatchBody(body) = &frame.kind
                && body.forge_game_id == forge_game_id
            {
                links.lobby_ids.extend(body.lobby_id.clone());
                links.db_ids.extend(body.db_id.clone());
            }
        }
    }
    for (_, unit) in units {
        for frame in &unit.frames {
            if let FrameKind::LobbyBody(lobby) = &frame.kind
                && links.lobby_ids.contains(&lobby.id)
            {
                for (_, deck_id) in &lobby.seat_decks {
                    links.deck_ids.insert(deck_id.clone());
                }
            }
        }
    }
    links
}

/// The unit's header and the lines of this match, in line order. Frames
/// are the unit's non-blank lines after the header, one each, as
/// `decode_unit` reads them.
fn kept_lines(forge_game_id: &str, text: &str, unit: &Unit, links: &Links) -> Vec<String> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    let mut kept = vec![header.to_string()];
    let frame_lines: Vec<&str> = lines.filter(|line| !line.trim().is_empty()).collect();
    // `decode_unit` makes one frame per non-blank line (a line that does not
    // read is `Other`), so the zip below pairs each line with its frame.
    debug_assert_eq!(frame_lines.len(), unit.frames.len());
    // As `read_frames`: deltas and actions follow the unit's last state or
    // event.
    let mut in_our_game = false;
    for (line, frame) in frame_lines.iter().zip(&unit.frames) {
        let keep = match &frame.kind {
            FrameKind::MatchBody(body) => body.forge_game_id == forge_game_id,
            FrameKind::LobbyBody(lobby) => links.lobby_ids.contains(&lobby.id),
            FrameKind::DeckBody(deck) => links.deck_ids.contains(&deck.id),
            FrameKind::RankedResult(result) => links.db_ids.contains(&result.db_id),
            FrameKind::GameState(state) | FrameKind::GameOver(state) => {
                in_our_game = state.game_id == forge_game_id;
                in_our_game
            }
            FrameKind::GameEvent(event) => {
                in_our_game = event.game_id == forge_game_id;
                in_our_game
            }
            FrameKind::GameDelta(_) | FrameKind::Action(_) => in_our_game,
            FrameKind::Other => false,
        };
        if keep {
            kept.push(line.to_string());
        }
    }
    kept
}
