//! Pure MTG Arena game rules: folds one game's stored GRE and client
//! messages (in log order) into the game states the GRE described, the zone
//! transfers it annotated, and the identity of each physical card across the
//! instance id changes Arena makes when a card changes zone. No IO and no
//! clock access: every time comes in with the messages.

pub mod identity;
pub mod log_lines;
mod message;
pub mod mtgr;
pub mod state;
pub mod summary;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use identity::{CardKey, Identity};
pub use log_lines::{LogLine, LogVerb, log_lines};
pub use state::{Annotation, GameObject, GameState, PlayerState, PoolMana, Zone, front_face};
pub use summary::{Commanders, GameColumns, GameSummary, local_main_deck, summarise_game};

use std::collections::{BTreeMap, BTreeSet};

use message::{
    OBJECT_ID_CHANGED, RawAnnotation, RawClient, RawDeck, RawState, SHUFFLE, ZONE_TRANSFER,
    parse_client, parse_gre,
};
use state::annotation_from;

/// The object kind of a card (`GameObject::kind`): tokens, abilities and
/// the other kinds are not cards.
pub const CARD: &str = "GameObjectType_Card";

const LIMBO: &str = "ZoneType_Limbo";

/// The catalog id of an Arena card (`ARENA_ID_OFFSET + grpId`); `None` for
/// grpId 0 (an unseen card) or one too large to be a grpId.
pub fn catalog_id(grp_id: u32) -> Option<u32> {
    if grp_id == 0 {
        return None;
    }
    cards::checked_arena_catalog_id(grp_id)
}

/// Who sent a game message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The game rules engine, to the client.
    Gre,
    /// The client, to the game rules engine.
    Client,
}

/// One stored game message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInput {
    pub source: Source,
    /// When the message was logged (UTC unix ms).
    pub logged_at: i64,
    /// The message's own JSON.
    pub json: String,
}

/// A card moving from one zone to another (`AnnotationType_ZoneTransfer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// The game state whose annotations carried the transfer.
    pub state_id: u32,
    /// That state's index in `Fold::states`.
    pub state_index: usize,
    pub logged_at: i64,
    pub card: CardKey,
    /// The card's grpId by its front face (`GameObject::card_grp_id`), or 0
    /// when no state shows the object.
    pub grp_id: u32,
    /// The card's other face (0 for a one-faced card or an unseen object).
    pub other_face_grp_id: u32,
    /// The object's kind (`GameObjectType_*`), looked up under the id it
    /// had before the move when no state shows the new id; empty when no
    /// state shows it at all.
    pub kind: String,
    /// The card's owner seat, or 0 when nothing tells it.
    pub owner_seat: u32,
    /// The card's instance id after the move.
    pub instance_id: u32,
    /// Its instance id before the move, when the state changed it.
    pub previous_id: Option<u32>,
    /// The source zone's kind (`ZoneType_*`), empty when the zone is unknown.
    pub from_zone: String,
    /// The destination zone's kind (`ZoneType_*`), empty when the zone is unknown.
    pub to_zone: String,
    /// The transfer's `category` detail (`Draw`, `CastSpell`, `PlayLand`, ...).
    pub category: String,
}

/// What one GRE game state message said beyond the state itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    /// The state's index in `Fold::states` (steps and states pair up).
    pub state_index: usize,
    pub logged_at: i64,
    /// The state message's own annotations, in order (not cumulative).
    pub annotations: Vec<Annotation>,
    /// The client messages sent since the previous state, in order: the
    /// actions this state answers. Client messages after the game's last
    /// state stay with that state.
    pub client: Vec<ClientAction>,
    /// The cards prompts sent in this state showed the local player, in
    /// order.
    pub shown: Vec<ShownCards>,
}

/// Cards a prompt showed the local player (a surveil, a "look at the top
/// N" choice): Arena's only report of a look (decision 0046).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShownCards {
    /// The object whose effect asks, or 0 when the prompt names none.
    pub source_id: u32,
    pub instance_ids: Vec<u32>,
}

/// A client message to the game rules engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientAction {
    /// `MulliganResp`: keep (`MulliganOption_AcceptHand`) or mulligan.
    Mulligan { keep: bool },
    /// `ChooseStartingPlayerResp`: the seat chosen to play first.
    ChooseStartingPlayer { seat: u32 },
    /// `ConcedeReq`: the match (`MatchScope_Match`) or only the game.
    Concede { match_scope: bool },
    /// `SubmitDeckResp`: (grpId, count) pairs by grpId.
    SubmitDeck {
        main: Vec<(u32, u32)>,
        side: Vec<(u32, u32)>,
    },
    /// Any other client message, by its type (`ClientMessageType_*`).
    Other(String),
}

/// Everything one game's messages told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fold {
    /// The local player's seat: the recipient of the first GRE message sent
    /// to exactly one seat (None if there is none).
    pub local_seat: Option<u32>,
    /// A full state was seen: the game began. States before a game's first
    /// full state are the previous game's tail (they reach the next game
    /// after an intermission) and are dropped with their transfers and ids.
    pub started: bool,
    /// One state per state message applied (full, diff or queued), in order.
    pub states: Vec<GameState>,
    /// Every zone transfer, in order.
    pub transfers: Vec<Transfer>,
    /// One step per state, in order.
    pub steps: Vec<Step>,
    /// Each seat's die roll (`DieRollResultsResp`), as (seat, roll).
    pub die_rolls: Vec<(u32, u32)>,
    pub identity: Identity,
    /// Each id change: the index of the state that made it and the new id.
    id_changes: Vec<(usize, u32)>,
}

/// Fold one game's messages, in log order. Malformed messages are skipped.
pub fn fold_game(messages: &[GameInput]) -> Fold {
    let mut fold = Fold::default();
    let empty = GameState::default();
    // Client actions since the last state.
    let mut pending: Vec<ClientAction> = Vec::new();
    for input in messages {
        if input.source == Source::Client {
            if let Some(raw) = parse_client(&input.json) {
                pending.push(client_action(raw));
            }
            continue;
        }
        let Some(mut message) = parse_gre(&input.json) else {
            continue;
        };
        // Broadcasts (timers, die rolls, some prompts) go to both seats; a
        // message to one seat names the local player.
        if fold.local_seat.is_none() && message.system_seat_ids.len() == 1 {
            fold.local_seat = Some(message.system_seat_ids[0]);
        }
        if let Some(rolls) = &message.die_roll_results_resp {
            for roll in &rolls.player_die_rolls {
                fold.die_rolls.push((roll.system_seat_id, roll.roll_value));
            }
        }
        // A prompt shows the local player cards only when it is sent to
        // their seat alone. A prompt before the local seat is known is
        // dropped. That is safe: the local seat is set by the first message
        // to one seat, Arena sends its game states to the local seat alone
        // (every fixture), and a game's first state comes before any prompt
        // that can show library cards.
        let (source_id, instance_ids) = message.shown_cards();
        let to_local = fold
            .local_seat
            .is_some_and(|seat| message.system_seat_ids == [seat]);
        if !instance_ids.is_empty()
            && to_local
            && let Some(step) = fold.steps.last_mut()
            && fold.states.last().map(|state| state.state_id) == message.game_state_id
        {
            step.shown.push(ShownCards {
                source_id,
                instance_ids,
            });
        }
        let Some(raw) = message.state_mut() else {
            continue;
        };
        let mid_game_full = raw.is_full() && fold.started;
        if raw.is_full() && !fold.started {
            fold.started = true;
            fold.states.clear();
            fold.transfers.clear();
            fold.steps.clear();
            fold.identity = Identity::default();
            fold.id_changes.clear();
        }
        if raw.is_full() {
            let full = empty.next(raw);
            let mut undone = roll_back(&mut fold, &full);
            undone.append(&mut pending);
            pending = undone;
        }
        if mid_game_full {
            drop_repeated_annotations(&fold, raw);
        }
        let raw = &*raw;
        let previous = fold.states.last().unwrap_or(&empty);
        let next = previous.next(raw);
        let state_index = fold.states.len();
        let changed = follow_id_changes(&mut fold.identity, &raw.annotations);
        for new_id in changed.keys() {
            fold.id_changes.push((state_index, *new_id));
        }
        for annotation in &raw.annotations {
            if annotation.is(ZONE_TRANSFER) {
                let moved = zone_transfers(annotation, previous, &next, &fold.identity);
                for mut transfer in moved {
                    transfer.previous_id = changed.get(&transfer.instance_id).copied();
                    if transfer.kind.is_empty()
                        && let Some(previous_id) = transfer.previous_id
                    {
                        transfer.kind = kind_of(previous_id, previous, &next);
                    }
                    fold.transfers.push(Transfer {
                        state_id: raw.game_state_id,
                        state_index,
                        logged_at: input.logged_at,
                        ..transfer
                    });
                }
            }
        }
        fold.states.push(next);
        fold.steps.push(Step {
            state_index,
            logged_at: input.logged_at,
            annotations: raw.annotations.iter().map(annotation_from).collect(),
            client: std::mem::take(&mut pending),
            shown: Vec::new(),
        });
    }
    if let Some(last) = fold.steps.last_mut() {
        last.client.append(&mut pending);
    }
    fold
}

fn client_action(raw: RawClient) -> ClientAction {
    if let Some(mulligan) = raw.mulligan_resp {
        return ClientAction::Mulligan {
            keep: mulligan.decision == "MulliganOption_AcceptHand",
        };
    }
    if let Some(choice) = raw.choose_starting_player_resp {
        return ClientAction::ChooseStartingPlayer {
            seat: choice.system_seat_id,
        };
    }
    if raw.kind == "ClientMessageType_ConcedeReq" {
        let scope = raw.concede_req.map(|req| req.scope).unwrap_or_default();
        return ClientAction::Concede {
            match_scope: scope == "MatchScope_Match",
        };
    }
    if let Some(submit) = raw.submit_deck_resp {
        let deck = submit.deck.unwrap_or(RawDeck {
            deck_cards: Vec::new(),
            sideboard_cards: Vec::new(),
        });
        return ClientAction::SubmitDeck {
            main: counted(&deck.deck_cards),
            side: counted(&deck.sideboard_cards),
        };
    }
    ClientAction::Other(raw.kind)
}

/// A list of grpIds as (grpId, count) pairs, by grpId.
fn counted(grp_ids: &[u32]) -> Vec<(u32, u32)> {
    let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
    for grp_id in grp_ids {
        *counts.entry(*grp_id).or_default() += 1;
    }
    counts.into_iter().collect()
}

/// The kind of the object a transfer moved, by its id before the move: a
/// token that leaves the battlefield gets a new id and ceases in the same
/// state, so no state shows it under the new id. Empty when unknown.
fn kind_of(previous_id: u32, previous: &GameState, next: &GameState) -> String {
    next.objects
        .get(&previous_id)
        .or_else(|| previous.objects.get(&previous_id))
        .map(|object| object.kind.clone())
        .unwrap_or_default()
}

/// The face of a two-faced card that `card_grp_id` did not pick (0 for none).
fn other_face(object: &GameObject) -> u32 {
    if object.otherside_grp_id == 0 {
        0
    } else if object.card_grp_id() == object.grp_id {
        object.otherside_grp_id
    } else {
        object.grp_id
    }
}

/// A full state in the middle of a game can undo an action the player
/// cancelled (a cast backed out of while paying): the card is back in the
/// zone it left, under the id it had, and the id it moved under is gone (Arena
/// issues it again later). Such transfers, and every state, transfer and id
/// change from the first of them on, never happened. Returns the client
/// actions of the dropped steps, in order: they were still sent.
fn roll_back(fold: &mut Fold, full: &GameState) -> Vec<ClientAction> {
    let undone = fold.transfers.iter().filter(|transfer| {
        let Some(previous_id) = transfer.previous_id else {
            return false;
        };
        let back = full.zones.values().any(|zone| {
            zone.kind == transfer.from_zone
                && zone.kind != LIMBO
                && zone.object_ids.contains(&previous_id)
        });
        let gone = !full.objects.contains_key(&transfer.instance_id)
            && !full
                .zones
                .values()
                .any(|zone| zone.object_ids.contains(&transfer.instance_id));
        back && gone
    });
    let Some(cut) = undone.map(|transfer| transfer.state_index).min() else {
        return Vec::new();
    };
    fold.states.truncate(cut);
    let mut client = Vec::new();
    for step in fold.steps.drain(cut..) {
        client.extend(step.client);
    }
    fold.transfers.retain(|transfer| transfer.state_index < cut);
    for (state_index, new_id) in &fold.id_changes {
        if *state_index >= cut {
            fold.identity.forget(*new_id);
        }
    }
    fold.id_changes
        .retain(|(state_index, _)| *state_index < cut);
    client
}

/// A full state that rewinds the game (Arena's undo) sends the annotations
/// of the state it rewinds to again, under the same ids (owner data: a shock
/// land's life payment repeated after a cancelled cast). Those were folded
/// with that state, so they are dropped here. Arena issues the ids of undone
/// states again, so only the ids of the states kept count.
fn drop_repeated_annotations(fold: &Fold, raw: &mut RawState) {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    for step in &fold.steps {
        for annotation in &step.annotations {
            seen.insert(annotation.id);
        }
    }
    raw.annotations
        .retain(|annotation| !seen.contains(&annotation.id));
}

/// Record every id change of a state, before its transfers name the new ids:
/// each `ObjectIdChanged`, and each library card a `Shuffle` renumbers
/// (`OldIds` to `NewIds`, pairwise), so a card shuffled back keeps its key.
/// Returns the state's changes, new id to old id.
fn follow_id_changes(identity: &mut Identity, annotations: &[RawAnnotation]) -> BTreeMap<u32, u32> {
    let mut changed = BTreeMap::new();
    for annotation in annotations {
        if annotation.is(SHUFFLE) {
            let old_ids = annotation.ints("OldIds");
            let new_ids = annotation.ints("NewIds");
            for (old, new) in old_ids.iter().zip(new_ids) {
                if let (Ok(old), Ok(new)) = (u32::try_from(*old), u32::try_from(*new)) {
                    identity.changed(old, new);
                    changed.insert(new, old);
                }
            }
            continue;
        }
        if !annotation.is(OBJECT_ID_CHANGED) {
            continue;
        }
        let orig = annotation
            .int("orig_id")
            .and_then(|id| u32::try_from(id).ok());
        let new = annotation
            .int("new_id")
            .and_then(|id| u32::try_from(id).ok());
        if let (Some(orig), Some(new)) = (orig, new) {
            identity.changed(orig, new);
            changed.insert(new, orig);
        }
    }
    changed
}

/// One transfer per affected id of a `ZoneTransfer` annotation (state id and
/// time left for the caller). The object is looked up in the new state, then
/// in the state before (it may have been deleted by the same diff).
fn zone_transfers(
    annotation: &RawAnnotation,
    previous: &GameState,
    next: &GameState,
    identity: &Identity,
) -> Vec<Transfer> {
    let zone_id = |key: &str| annotation.int(key).and_then(|id| u32::try_from(id).ok());
    let from = zone_id("zone_src");
    let to = zone_id("zone_dest");
    let zone_owner = |zone: Option<u32>| {
        zone.and_then(|id| next.zones.get(&id))
            .and_then(|zone| zone.owner_seat)
    };
    let mut transfers = Vec::new();
    for &instance_id in &annotation.affected_ids {
        let object = next
            .objects
            .get(&instance_id)
            .or_else(|| previous.objects.get(&instance_id));
        let owner_seat = object
            .map(|object| object.owner_seat)
            .filter(|seat| *seat != 0)
            .or_else(|| zone_owner(from))
            .or_else(|| zone_owner(to))
            .unwrap_or(0);
        transfers.push(Transfer {
            state_id: 0,
            state_index: 0,
            logged_at: 0,
            card: identity.key(instance_id),
            instance_id,
            previous_id: None,
            grp_id: object.map(|object| object.card_grp_id()).unwrap_or(0),
            other_face_grp_id: object.map(other_face).unwrap_or(0),
            kind: object.map(|object| object.kind.clone()).unwrap_or_default(),
            owner_seat,
            from_zone: from.map(|id| next.zone_kind(id)).unwrap_or_default(),
            to_zone: to.map(|id| next.zone_kind(id)).unwrap_or_default(),
            category: annotation.text("category").unwrap_or("").to_string(),
        });
    }
    transfers
}
