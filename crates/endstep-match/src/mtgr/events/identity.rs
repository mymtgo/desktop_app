//! Who and what a Forge card id is: owners, names and the object an event
//! names.

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{Statement, Zone};
use endstep_protocol::GameEvent;

use super::super::GameWriter;
use super::super::board::{Board, SEATS, Written};
use super::super::other_seat;
use crate::fold::ReplayGame;
use crate::games::KeptKind;

/// The owner each full state of the game gives a Forge card id.
pub(in crate::mtgr) fn owners(game: &ReplayGame) -> BTreeMap<u64, u32> {
    let mut owners = BTreeMap::new();
    for frame in &game.record.frames {
        let KeptKind::State(state) = &frame.kind else {
            continue;
        };
        for card in &state.cards {
            if let Some(owner) = seat_of(card.owner_seat) {
                owners.entry(card.id).or_insert(owner);
            }
        }
    }
    owners
}

/// Each Forge card id with every name the game gives it (Forge reissues
/// ids, so one id can name several cards).
pub(in crate::mtgr) type CardNames = BTreeMap<u64, BTreeSet<String>>;

/// The names the game's frames give each Forge card id: the cards and stack
/// entries of its full states, and the cards its events name (a card seen
/// only in an event, or in a hand Forge hides, is in no state).
pub(in crate::mtgr) fn card_names(game: &ReplayGame) -> CardNames {
    let mut names = CardNames::new();
    let mut add = |id: u64, name: &str| {
        if !name.is_empty() {
            names.entry(id).or_default().insert(name.to_string());
        }
    };
    for frame in &game.record.frames {
        match &frame.kind {
            KeptKind::State(state) => {
                for card in &state.cards {
                    add(card.id, &card.name);
                }
                for entry in &state.stack {
                    add(entry.source_id, &entry.source_name);
                }
            }
            KeptKind::Event(event) => {
                if let (Some(id), Some(name)) = (event.card_id, &event.card_name) {
                    add(id, name);
                }
                if let (Some(id), Some(name)) = (event.target_card_id, &event.target_card_name) {
                    add(id, name);
                }
                for (id, name) in event.card_ids.iter().zip(&event.card_names) {
                    add(*id, name);
                }
            }
            KeptKind::Delta(_) => {}
        }
    }
    names
}

pub(in crate::mtgr) fn seat_of(seat: Option<u32>) -> Option<u32> {
    seat.filter(|seat| SEATS.contains(seat))
}

/// Forge's zone names in events. A created object has no `fromZone`.
pub(super) fn zone_named(name: &str) -> Option<Zone> {
    match name {
        "Hand" => Some(Zone::Hand),
        "Library" => Some(Zone::Library),
        "Graveyard" => Some(Zone::Graveyard),
        "Battlefield" => Some(Zone::Battlefield),
        "Stack" => Some(Zone::Stack),
        "Exile" => Some(Zone::Exile),
        "Command" => Some(Zone::Command),
        _ => None,
    }
}

impl Board {
    /// The object an event names in the zone `from`. A Forge id that names
    /// a card of another name in a hidden hand was given to another card,
    /// and is dropped. A card the record has not seen is introduced where the event says it was: in a hand of
    /// another seat it is a card of that name already there (a cast taken
    /// back, cast again under a new id), else a hidden card is named; a
    /// library card is new there. Anything else (a card the record has
    /// elsewhere, or first seen in another zone) is `None`: the next full
    /// state's diff places it.
    pub(in crate::mtgr) fn object_of_event(
        &mut self,
        out: &mut Vec<Written>,
        forge_id: u64,
        name: &str,
        owner: u32,
        from: Zone,
    ) -> Option<u32> {
        if let Some(id) = self.objects.id_of(forge_id) {
            let object = self.objects.get(id)?;
            // A named card in a hand Forge hides keeps the Forge id it was
            // named under; an event that names that id otherwise is about
            // another card Forge gave the id to (trap 40).
            let in_hidden_hand = object.owner != self.user_seat && object.zone == Zone::Hand;
            let renamed = !name.is_empty() && object.name.as_deref() != Some(name);
            if !(in_hidden_hand && renamed) {
                return (object.zone == from).then_some(id);
            }
            self.objects.forget(forge_id);
        }
        match from {
            Zone::Hand if owner != self.user_seat => {
                if let Some(id) = self.objects.named_in_hand(owner, name) {
                    self.objects.identify(id, forge_id, name);
                    return Some(id);
                }
                let id = self.objects.lowest_hidden_in_hand(owner)?;
                let statement = Statement::Identify {
                    id,
                    name: Some(name.to_string()),
                    sources: Vec::new(),
                };
                out.push((statement, None));
                self.objects.identify(id, forge_id, name);
                Some(id)
            }
            Zone::Library => {
                let name = Some(name.to_string());
                let id = self
                    .objects
                    .add(Some(forge_id), name, owner, Zone::Library, false);
                out.push((self.new_card(id, owner, Zone::Library, None), None));
                Some(id)
            }
            _ => None,
        }
    }
}

impl<'a> GameWriter<'a> {
    /// Who owns a card: the record's object, else a state of the game, else
    /// `fallback`.
    pub(super) fn owner_of(&self, forge_id: u64, fallback: u32) -> u32 {
        let objects = &self.board.objects;
        let tracked = objects
            .id_of(forge_id)
            .and_then(|id| objects.get(id))
            .map(|object| object.owner);
        tracked
            .or_else(|| self.owners.get(&forge_id).copied())
            .unwrap_or(fallback)
    }

    /// Who owns a revealed card. `playerIndex` is the viewer, never the
    /// owner: with no object and no state to say, an `ownerEcho` is the
    /// user's own card and anything else the other seat's.
    pub(super) fn owner_of_revealed(&self, forge_id: u64, event: &GameEvent) -> u32 {
        let fallback = if event.owner_echo {
            self.user_seat
        } else {
            other_seat(self.user_seat)
        };
        self.owner_of(forge_id, fallback)
    }
}
