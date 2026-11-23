//! The refs that link a unit to a match: a session unit holds the match REST
//! body, match units hold the game frames, and both name the Forge game id.
//! A session unit can also close after the lobby and deck bodies but before
//! the match body, so lobby and deck ids are refs too: the match body names
//! its lobby, and the lobby names its seat decks. A deck id is a ref by its
//! role: `deck:` where a lobby names it, `deckbody:` where its body is, so a
//! deck body reaches the matches whose lobbies name it and nothing else.
//! A match body also names its Endstep match id, so a stored match (by its
//! token) finds its Forge game again.

use std::collections::BTreeSet;

use crate::{FrameKind, Unit};

/// `game:<forge game id>` for every state, event and match body;
/// `match:<match id>` for every match body;
/// `db:<db id>` for every match body with a db id and every ranked result;
/// `lobby:<lobby id>` for every lobby body and match body with a lobby id;
/// `deck:<deck id>` for every lobby seat deck that is not hidden
/// (`hidden-deck`); `deckbody:<deck id>` for every deck body. Sorted and
/// deduplicated.
pub fn unit_refs(unit: &Unit) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for frame in &unit.frames {
        match &frame.kind {
            FrameKind::GameState(state) | FrameKind::GameOver(state) => {
                refs.insert(format!("game:{}", state.game_id));
            }
            FrameKind::GameEvent(event) => {
                refs.insert(format!("game:{}", event.game_id));
            }
            FrameKind::MatchBody(body) => {
                refs.insert(format!("game:{}", body.forge_game_id));
                refs.insert(format!("match:{}", body.id));
                if let Some(db_id) = &body.db_id {
                    refs.insert(format!("db:{db_id}"));
                }
                if let Some(lobby_id) = &body.lobby_id {
                    refs.insert(format!("lobby:{lobby_id}"));
                }
            }
            FrameKind::LobbyBody(lobby) => {
                refs.insert(format!("lobby:{}", lobby.id));
                for (_, deck_id) in &lobby.seat_decks {
                    if !deck_id.starts_with("hidden") {
                        refs.insert(format!("deck:{deck_id}"));
                    }
                }
            }
            FrameKind::DeckBody(deck) => {
                refs.insert(format!("deckbody:{}", deck.id));
            }
            FrameKind::RankedResult(result) => {
                refs.insert(format!("db:{}", result.db_id));
            }
            _ => {}
        }
    }
    refs.into_iter().collect()
}
