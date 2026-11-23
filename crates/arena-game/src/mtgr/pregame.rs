//! The pregame (spec section 9.1): the keyframe, the die rolls, who chose to
//! play first, the revealed companions (`outside.rs`), the opening hands,
//! mulligans and keeps.
//!
//! Arena does not annotate the opening hands or a mulligan: a hand appears,
//! or is replaced by a new one, between two states. A card leaving a hand
//! with no zone transfer goes back to the library (`rule:mulligan`, then the
//! shuffle), and a card arriving with none is drawn (`rule:opening-hand`
//! for the first hand, `rule:mulligan` after).
//!
//! Arena reports each decision in the players' entries: a seat deciding has
//! `pendingMessageType` `ClientMessageType_MulliganResp` (or
//! `ChooseStartingPlayerResp` while choosing who plays first). A seat whose
//! `mulliganCount` rises took a mulligan; a seat whose pending `MulliganResp`
//! clears with its count unchanged kept, in that state.

use std::collections::BTreeSet;

use ::mtgr::{Cause, LibraryEnd, MoveAction, ObjRef, Position, Statement, Zone};

use super::GameWriter;
use crate::GameState;
use crate::message::{OBJECT_ID_CHANGED, ZONE_TRANSFER};

const HAND: &str = "ZoneType_Hand";
const MULLIGAN_RESP: &str = "ClientMessageType_MulliganResp";
pub(super) const CHOOSE_STARTING_PLAYER: &str = "ClientMessageType_ChooseStartingPlayerResp";

/// Whether `seat` was waited on for `message` in `previous` and is not in
/// `state`.
fn cleared(previous: &GameState, state: &GameState, seat: u32, message: &str) -> bool {
    let pending = |state: &GameState| state.pending.get(&seat).is_some_and(|m| m == message);
    pending(previous) && !pending(state)
}

impl GameWriter<'_> {
    /// `pregame`, its keyframe and the die rolls (game 1).
    pub(super) fn write_pregame_head(&mut self, state: &GameState) {
        self.line(0, Statement::Pregame, None);
        self.write_pregame_keyframe(state);
        for (seat, value) in self.fold.die_rolls.clone() {
            let player = self.roster.name(seat);
            self.line(
                1,
                Statement::Roll {
                    player,
                    sides: None,
                    value,
                },
                None,
            );
        }
    }

    /// `"P" choose first "Q"`, once, then the revealed companions.
    pub(super) fn write_choose_first(&mut self) {
        if self.chose_first {
            return;
        }
        self.chose_first = true;
        if let Some((chooser, first)) = self.choice {
            let player = self.roster.name(chooser);
            let first = self.roster.name(first);
            self.line(1, Statement::ChooseFirst { player, first }, None);
        }
        self.write_companions();
    }

    /// Each seat's decisions in state `index`, then the hands each player
    /// drew or sent back since the previous state.
    pub(super) fn pregame_state(&mut self, index: usize) {
        let fold = self.fold;
        let state = &fold.states[index];
        let previous = index.checked_sub(1).map(|i| &fold.states[i]);
        if let Some(previous) = previous {
            for seat in self.seat_order() {
                if cleared(previous, state, seat, CHOOSE_STARTING_PLAYER) {
                    self.write_choose_first();
                }
                self.write_decision(previous, state, seat);
            }
        }
        let annotated = self.annotated_ids(index);
        for seat in self.seat_order() {
            let old = previous
                .map(|state| hand_of(state, seat))
                .unwrap_or_default();
            let new = hand_of(state, seat);
            let sent_back: Vec<u32> = old
                .iter()
                .filter(|id| !new.contains(id) && !annotated.contains(id))
                .copied()
                .collect();
            let drawn: Vec<u32> = new
                .iter()
                .filter(|id| !old.contains(id) && !annotated.contains(id))
                .copied()
                .collect();
            self.send_back(seat, &sent_back);
            self.draw_hand(index, seat, &drawn);
        }
    }

    /// `"P" mulligan` for each rise of the seat's mulligan count, else
    /// `"P" keep` when its pending `MulliganResp` cleared.
    fn write_decision(&mut self, previous: &GameState, state: &GameState, seat: u32) {
        let before = previous.mulligans.get(&seat).copied().unwrap_or(0);
        let now = state.mulligans.get(&seat).copied().unwrap_or(0);
        let player = self.roster.name(seat);
        if now > before {
            for _ in before..now {
                self.line(
                    1,
                    Statement::Mulligan {
                        player: player.clone(),
                    },
                    None,
                );
            }
        } else if cleared(previous, state, seat, MULLIGAN_RESP) {
            self.line(1, Statement::Keep { player }, None);
        }
    }

    /// A mulligan: the hand goes back into the library, which is shuffled.
    fn send_back(&mut self, seat: u32, instance_ids: &[u32]) {
        if instance_ids.is_empty() {
            return;
        }
        let mut objects: Vec<(u32, ObjRef)> = Vec::new();
        for instance_id in instance_ids {
            let key = self.fold.identity.key(*instance_id);
            if let Some(object) = self.ids.obj(key) {
                objects.push((object.id, object));
                self.ids.moved(key, Zone::Library);
            }
        }
        objects.sort_by_key(|(id, _)| *id);
        for (_, object) in objects {
            self.line(
                1,
                Statement::Move {
                    object,
                    from: Zone::Hand,
                    to: Zone::Library,
                    action: None,
                    position: None,
                    entering: None,
                    with: None,
                },
                Some(Cause::Rule("mulligan".into())),
            );
        }
        let player = self.roster.name(seat);
        self.line(
            1,
            Statement::Shuffle { player },
            Some(Cause::Rule("mulligan".into())),
        );
        self.ids.end_library(seat);
    }

    /// A hand drawn: each card taken from the library and drawn.
    fn draw_hand(&mut self, index: usize, seat: u32, instance_ids: &[u32]) {
        if instance_ids.is_empty() {
            return;
        }
        self.write_choose_first();
        let rule = if self.hand_drawn.insert(seat) {
            "opening-hand"
        } else {
            "mulligan"
        };
        let mut sorted = instance_ids.to_vec();
        sorted.sort_unstable();
        let state = &self.fold.states[index];
        for instance_id in sorted {
            let grp_id = state
                .objects
                .get(&instance_id)
                .map(|object| object.card_grp_id())
                .unwrap_or(0);
            let name = self.visible_name(grp_id, seat, Zone::Hand);
            let key = self.fold.identity.key(instance_id);
            // A draw takes the top card (CR 121.2).
            let top = Position {
                end: LibraryEnd::Top,
                n: 1,
            };
            let object = self.introduce_at(key, name, seat, Zone::Library, Some(top));
            self.line(
                1,
                Statement::Move {
                    object,
                    from: Zone::Library,
                    to: Zone::Hand,
                    action: Some(MoveAction::Draw),
                    position: None,
                    entering: None,
                    with: None,
                },
                Some(Cause::Rule(rule.into())),
            );
            self.ids.moved(key, Zone::Hand);
        }
    }

    /// Every instance id a zone transfer or id change of state `index`
    /// names: those moves are written from the annotations, not as hand
    /// changes.
    fn annotated_ids(&self, index: usize) -> BTreeSet<u32> {
        let mut ids = BTreeSet::new();
        for annotation in &self.fold.steps[index].annotations {
            if annotation.is(ZONE_TRANSFER) {
                ids.extend(annotation.affected_ids.iter().copied());
            }
            if annotation.is(OBJECT_ID_CHANGED) {
                for key in ["orig_id", "new_id"] {
                    if let Some(id) = annotation.int(key).and_then(|id| u32::try_from(id).ok()) {
                        ids.insert(id);
                    }
                }
            }
        }
        ids
    }
}

/// The instance ids in `seat`'s hand.
fn hand_of(state: &GameState, seat: u32) -> Vec<u32> {
    let mut ids = Vec::new();
    for zone in state.zones.values() {
        if zone.kind == HAND && zone.owner_seat == Some(seat) {
            ids.extend(zone.object_ids.iter().copied());
        }
    }
    ids
}
