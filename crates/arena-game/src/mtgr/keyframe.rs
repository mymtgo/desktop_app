//! Keyframes (spec section 10): the full game state at the pregame and at
//! the start of each turn, from the fold's state.
//!
//! A turn begins part way through the state message that begins it, before
//! that turn's untap step actions (section 10.1). The state is the one
//! after the whole message, so the keyframe undoes what the message's
//! annotations after the turn's start did: permanents they tapped or
//! untapped, and cards they moved.
//!
//! Object entries list the cards and tokens the record tracks (the ones
//! with a live id), in every zone but the stack, by id. Their names are the
//! names the record has given them. Life totals undo the message's
//! `ModifiedLife` annotations after the turn's start, and a permanent's
//! types and power and toughness are as the turn began
//! (`objects::at_turn_start`). A battlefield entry lists what it is
//! attached to (Arena's persistent `Attachment`) and the face up when it
//! is not the front (decision 0022). Day or night follows the players
//! when the record made it either. After a keyframe the record's own view
//! of each permanent (face, tapped, types, power and toughness, counters)
//! is the keyframe's, as it is for a reader (spec section 14.4). After a
//! gap in the messages, the keyframe also ends the ids of objects it does
//! not list and moves the others to where it lists them, as a reader does
//! after `incomplete`.

use std::collections::BTreeMap;

use ::mtgr::{ObjRef, PtValue, Statement, Zone};

use super::GameWriter;
use super::combat::MODIFIED_LIFE;
use super::ids::Kind;
use super::moves::{card_types, creature_pt, zone_of};
use super::objects::{TAPPED_UNTAPPED, at_turn_start, named_counters};
use crate::{CardKey, GameObject, GameState, Transfer};

/// Where a card was at the keyframe, with its object.
struct Placed<'a> {
    key: CardKey,
    zone: Zone,
    owner_seat: u32,
    object: Option<&'a GameObject>,
}

impl GameWriter<'_> {
    /// The pregame keyframe: each player's life and library, no objects.
    pub(super) fn write_pregame_keyframe(&mut self, state: &GameState) {
        self.line(1, Statement::Keyframe, None);
        let life: Vec<(u32, i32)> = state
            .players
            .iter()
            .map(|(seat, player)| (*seat, player.life))
            .collect();
        self.write_players(state, &life, &BTreeMap::new());
    }

    /// Each seat's life total at annotation `cut` of state `index`: the
    /// state's, less the `ModifiedLife` changes after `cut`.
    pub(super) fn life_at(&self, index: usize, cut: usize) -> Vec<(u32, i32)> {
        let mut life: BTreeMap<u32, i32> = self.fold.states[index]
            .players
            .iter()
            .map(|(seat, player)| (*seat, player.life))
            .collect();
        for annotation in &self.fold.steps[index].annotations[cut..] {
            if !annotation.is(MODIFIED_LIFE) {
                continue;
            }
            let change = annotation
                .int("life")
                .and_then(|change| i32::try_from(change).ok())
                .unwrap_or(0);
            for seat in &annotation.affected_ids {
                if let Some(total) = life.get_mut(seat) {
                    *total -= change;
                }
            }
        }
        life.into_iter().collect()
    }

    /// Each seat's library size for the keyframe of a turn that begins in
    /// state `index`: the state's, less the moves after the turn began.
    pub(super) fn keyframe_libraries(&self, index: usize) -> BTreeMap<u32, u32> {
        let fold = self.fold;
        let mut sizes: BTreeMap<u32, i64> = fold.states[index]
            .players
            .iter()
            .map(|(seat, player)| (*seat, i64::from(player.library_size)))
            .collect();
        let after = fold.transfers[self.next_transfer..]
            .iter()
            .take_while(|transfer| transfer.state_index == index);
        for transfer in after {
            if let Some(size) = sizes.get_mut(&transfer.owner_seat) {
                if transfer.from_zone == "ZoneType_Library" {
                    *size += 1;
                }
                if transfer.to_zone == "ZoneType_Library" {
                    *size -= 1;
                }
            }
        }
        sizes
            .into_iter()
            .map(|(seat, size)| (seat, u32::try_from(size).unwrap_or(0)))
            .collect()
    }

    /// The keyframe of a turn that begins at annotation `cut` of state
    /// `index`.
    pub(super) fn write_turn_keyframe(&mut self, index: usize, cut: usize) {
        let fold = self.fold;
        let state = &fold.states[index];
        let after: Vec<&Transfer> = fold.transfers[self.next_transfer..]
            .iter()
            .take_while(|transfer| transfer.state_index == index)
            .collect();
        let mut moved: BTreeMap<CardKey, &Transfer> = BTreeMap::new();
        for transfer in &after {
            moved.entry(transfer.card).or_insert(transfer);
        }
        let tapped_before = tapped_before(&fold.steps[index].annotations[cut..]);
        // Library positions as the lines before the keyframe leave them.
        self.positions.catch_up(&self.lines);

        self.line(1, Statement::Keyframe, None);
        let life = self.life_at(index, cut);
        let libraries = self.keyframe_libraries(index);
        self.write_players(state, &life, &libraries);
        if let Some(night) = self.day_night {
            self.line(2, Statement::DayNight { night }, None);
        }
        let placed_all = placed_cards(state, &moved, &fold.identity);
        if self.after_gap {
            self.after_gap = false;
            self.resync_zones(&placed_all);
        }
        let mut entries: BTreeMap<u32, Statement> = BTreeMap::new();
        let mut keys: BTreeMap<u32, CardKey> = BTreeMap::new();
        for placed in placed_all {
            let Some(object) = self.ids.obj(placed.key) else {
                continue;
            };
            let id = object.id;
            // Outside the game, only a revealed companion is listed.
            let companion = self.ids.get(placed.key).is_some_and(|t| t.companion);
            if placed.zone == Zone::OutsideSideboard && !companion {
                continue;
            }
            keys.insert(id, placed.key);
            if placed.zone == Zone::Battlefield
                && let Some(tracked) = self.ids.get_mut(placed.key)
            {
                tracked.controller_seat = battlefield_controller(&placed);
            }
            entries
                .entry(id)
                .or_insert_with(|| self.object_entry(object, &placed, &tapped_before, index, cut));
        }
        for effect in self.keyframe_effects(index, cut) {
            if let Statement::KeyframeEffect { id, .. } = &effect {
                entries.insert(*id, effect);
            }
        }
        for entry in entries.into_values() {
            self.resync(&entry, &keys);
            self.line(2, entry, None);
        }
    }

    /// After a gap, the keyframe replaces the replayed state (spec 14.4):
    /// objects it does not list end (abilities and objects outside the game
    /// aside), and the others are where it lists them.
    fn resync_zones(&mut self, placed: &[Placed]) {
        let mut zones: BTreeMap<CardKey, Zone> = BTreeMap::new();
        for card in placed {
            zones.entry(card.key).or_insert(card.zone);
        }
        for key in self.ids.keys() {
            let Some(tracked) = self.ids.get(key) else {
                continue;
            };
            if matches!(tracked.kind, Kind::Ability { .. })
                || tracked.zone == Zone::OutsideSideboard
            {
                continue;
            }
            match zones.get(&key) {
                Some(zone) if *zone != tracked.zone => {
                    let zone = *zone;
                    self.ids.moved(key, zone);
                }
                Some(_) => {}
                None => self.ids.end(key),
            }
        }
    }

    /// Take a battlefield entry's state as the record's own.
    fn resync(&mut self, entry: &Statement, keys: &BTreeMap<u32, CardKey>) {
        let Statement::KeyframeObject {
            object,
            zone: Zone::Battlefield,
            face,
            types,
            pt,
            tapped,
            counters,
            ..
        } = entry
        else {
            return;
        };
        let Some(tracked) = keys.get(&object.id).and_then(|key| self.ids.get_mut(*key)) else {
            return;
        };
        tracked.face = face.clone();
        tracked.pt_unknown = *pt == Some(PtValue::Unknown);
        tracked.tapped = *tapped;
        tracked.types = types.clone();
        tracked.pt = match pt {
            Some(PtValue::Pt(power, toughness)) => Some((*power, *toughness)),
            _ => None,
        };
        tracked.counters = counters.iter().cloned().collect();
    }

    fn write_players(
        &mut self,
        state: &GameState,
        life: &[(u32, i32)],
        libraries: &BTreeMap<u32, u32>,
    ) {
        self.life = life.iter().copied().collect();
        for (seat, player) in &state.players {
            let entry = Statement::KeyframePlayer {
                player: self.roster.name(*seat),
                life: self.life.get(seat).copied().unwrap_or(player.life),
                poison: 0,
                library: libraries.get(seat).copied().unwrap_or(player.library_size),
            };
            self.line(2, entry, None);
        }
    }

    fn object_entry(
        &mut self,
        object: ObjRef,
        placed: &Placed,
        tapped_before: &BTreeMap<u32, bool>,
        index: usize,
        cut: usize,
    ) -> Statement {
        let on_battlefield = placed.zone == Zone::Battlefield;
        let player = match placed.zone {
            Zone::Library | Zone::Hand | Zone::Graveyard | Zone::OutsideSideboard => {
                Some(placed.owner_seat)
            }
            Zone::Battlefield => Some(battlefield_controller(placed)),
            _ => None,
        };
        let attached = placed
            .object
            .filter(|_| on_battlefield)
            .and_then(|arena| arena.attached_to)
            .and_then(|target| self.ids.get(self.fold.identity.key(target)))
            .map(|tracked| tracked.id);
        let with = (placed.zone == Zone::Exile)
            .then(|| self.ids.get(placed.key).and_then(|tracked| tracked.with))
            .flatten();
        let adventure = (placed.zone == Zone::Exile)
            .then(|| {
                self.ids
                    .get(placed.key)
                    .and_then(|tracked| tracked.adventure)
            })
            .flatten()
            .map(|seat| self.roster.name(seat));
        let state = &self.fold.states[index];
        let (face, types, pt, tapped, counters) = match placed.object {
            Some(arena) if on_battlefield => {
                // Face, types and power and toughness as the turn began.
                let shown = at_turn_start(self.fold, index, cut, arena);
                let pt = if shown.pt_unknown {
                    Some(PtValue::Unknown)
                } else {
                    creature_pt(shown.object)
                        .map(|(power, toughness)| PtValue::Pt(power, toughness))
                };
                (
                    self.shown_face(state, shown.object),
                    card_types(shown.object),
                    pt,
                    tapped_before
                        .get(&arena.instance_id)
                        .copied()
                        .unwrap_or(arena.tapped),
                    named_counters(arena),
                )
            }
            _ => (None, Vec::new(), None, false, Vec::new()),
        };
        let position = (placed.zone == Zone::Library)
            .then(|| self.positions.position(object.id))
            .flatten();
        Statement::KeyframeObject {
            object,
            zone: placed.zone,
            player: player.map(|seat| self.roster.name(seat)),
            face,
            types,
            pt,
            tapped,
            attached,
            with,
            adventure,
            companion: placed.zone == Zone::OutsideSideboard,
            counters,
            position,
        }
    }
}

/// A permanent's controller at a keyframe: the object's, else its owner.
fn battlefield_controller(placed: &Placed) -> u32 {
    placed
        .object
        .map(|object| object.controller_seat)
        .filter(|seat| *seat != 0)
        .unwrap_or(placed.owner_seat)
}

/// Each card in a zone of `state` other than the stack, where it was
/// before the `moved` transfers.
fn placed_cards<'a>(
    state: &'a GameState,
    moved: &BTreeMap<CardKey, &Transfer>,
    identity: &crate::Identity,
) -> Vec<Placed<'a>> {
    let mut placed = Vec::new();
    for zone in state.zones.values() {
        let Some(kind) = zone_of(&zone.kind) else {
            continue;
        };
        for instance_id in &zone.object_ids {
            let key = identity.key(*instance_id);
            if kind == Zone::Stack || moved.contains_key(&key) {
                continue;
            }
            placed.push(Placed {
                key,
                zone: kind,
                owner_seat: zone.owner_seat.unwrap_or(0),
                object: state.objects.get(instance_id),
            });
        }
    }
    for (key, transfer) in moved {
        let Some(from) = zone_of(&transfer.from_zone) else {
            continue;
        };
        if from == Zone::Stack {
            continue;
        }
        placed.push(Placed {
            key: *key,
            zone: from,
            owner_seat: transfer.owner_seat,
            object: state.objects.get(&transfer.instance_id),
        });
    }
    placed
}

/// Whether each permanent the annotations tap or untap was tapped before
/// the first of them (`tapped` 1 taps; Arena leaves 0 out).
fn tapped_before(annotations: &[crate::Annotation]) -> BTreeMap<u32, bool> {
    let mut before = BTreeMap::new();
    for annotation in annotations {
        if !annotation.is(TAPPED_UNTAPPED) {
            continue;
        }
        let taps = annotation.int("tapped") == Some(1);
        for id in &annotation.affected_ids {
            before.entry(*id).or_insert(!taps);
        }
    }
    before
}
