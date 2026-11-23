//! Keyframes: the first state's numbers, and the entries of each turn's
//! keyframe.
//!
//! A keyframe is written from the record's own objects, so it is what a
//! reader holds at that line (standard 14.5). After a full state the record
//! can still differ from Forge in ways no statement can fix: a library size,
//! a hidden hand holding more objects than `handSize` (a card went somewhere
//! Forge hides: foretold, face down), or a named card the state lists
//! nowhere with nothing to explain it. That is drift
//! (`Board::drifted`): the writer marks the record `incomplete`, and the next
//! keyframe takes Forge's numbers and ends the hand objects that are too
//! many and the cards listed nowhere (a reader ends every object a keyframe
//! does not list).
//!
//! The drift is kept as the difference from the record at that state, not as
//! Forge's numbers: what an event writes after the state (a card bottomed
//! after the keep) is then still counted when the keyframe is repaired.

use ::mtgr::{PtValue, Statement, Zone};
use endstep_protocol::GameState;

use super::board::{Board, SEATS};
use super::objects::Tracked;
use super::shown::{hand_size, player_of, shown_cards};

/// How Forge's numbers for one seat differed from the record's after a full
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Truth {
    pub seat: u32,
    /// How many more cards Forge's library held than the record's.
    pub library_gap: i64,
    /// How many more cards the record's hand held than `handSize`, for a
    /// seat whose hand Forge hides (0 for the user).
    pub hand_surplus: u32,
    /// The seat's named objects the state listed nowhere, with the zone the
    /// record had each in.
    pub unlisted: Vec<(u32, Zone)>,
}

impl Board {
    /// The first full state sets each player's numbers, written as the
    /// pregame keyframe's entries. In a pregame (`from_start`) every card the
    /// state shows is still to be drawn, so the library starts that much
    /// larger; a game joined later starts as the state has it.
    pub(super) fn start(&mut self, state: &GameState, from_start: bool) -> Vec<Statement> {
        let shown = shown_cards(state);
        for seat in SEATS {
            let player = player_of(state, seat);
            let mut library = player
                .and_then(|player| player.library_size)
                .map(i64::from)
                .unwrap_or(0);
            if from_start {
                let named = shown
                    .iter()
                    .filter(|card| card.owner == seat && card.zone != Zone::Library && !card.token)
                    .count();
                library += i64::try_from(named).unwrap_or(0);
                if seat != self.user_seat {
                    library += i64::from(hand_size(state, seat).unwrap_or(0));
                }
            }
            self.objects.set_library(seat, library);
            let life = player.and_then(|player| player.life).unwrap_or(20);
            self.life.insert(seat, life);
            let poison = player.and_then(|player| player.poison).unwrap_or(0);
            self.poison.insert(seat, poison);
        }
        self.player_entries()
    }

    /// Whether the record differs from Forge after the last full state in a
    /// way no statement can fix: a card moved unseen.
    pub(super) fn drifted(&self) -> bool {
        self.truth.is_some()
    }

    /// The keyframe entries: what a reader holds now. A reader ends every
    /// object a keyframe does not list, and a keyframe lists none on the
    /// stack, so those ids end here too. After a drift the keyframe is as
    /// Forge has it.
    pub(super) fn keyframe(&mut self) -> Vec<Statement> {
        for id in self.objects.ids_where(|object| object.zone == Zone::Stack) {
            self.objects.end(id);
        }
        for truth in self.truth.take().unwrap_or_default() {
            self.repair(truth);
        }
        let mut entries = self.player_entries();
        for object in self.objects.all() {
            entries.push(self.object_entry(object));
        }
        entries
    }

    fn player_entries(&self) -> Vec<Statement> {
        let mut entries = Vec::new();
        for seat in SEATS {
            let library = self.objects.library(seat).max(0);
            entries.push(Statement::KeyframePlayer {
                player: self.name(seat),
                life: self.life.get(&seat).copied().unwrap_or(20),
                poison: self.poison.get(&seat).copied().unwrap_or(0),
                library: u32::try_from(library).unwrap_or(u32::MAX),
            });
        }
        entries
    }

    fn object_entry(&self, object: &Tracked) -> Statement {
        let player = match object.zone {
            Zone::Library | Zone::Hand | Zone::Graveyard => Some(self.name(object.owner)),
            Zone::Battlefield => Some(self.name(object.controller)),
            _ => None,
        };
        let on_battlefield = object.zone == Zone::Battlefield;
        Statement::KeyframeObject {
            object: object.obj(),
            zone: object.zone,
            player,
            face: None,
            types: object.types.clone(),
            pt: object
                .pt
                .map(|(power, toughness)| PtValue::Pt(power, toughness)),
            tapped: on_battlefield && object.tapped,
            attached: object.attached_to,
            with: None,
            adventure: None,
            companion: false,
            counters: object
                .counters
                .iter()
                .map(|(name, count)| (name.clone(), *count))
                .collect(),
            position: object.position,
        }
    }

    /// Notes Forge's numbers when the record's differ from them: a library
    /// size, the size of a hand Forge hides, or named cards listed nowhere
    /// (`Board::unlisted`).
    pub(super) fn note_truth(&mut self, state: &GameState) {
        let mut truth = Vec::new();
        let mut differs = false;
        for seat in SEATS {
            let library_gap = self.library_gap(state, seat);
            let hand_gap = if seat == self.user_seat {
                0
            } else {
                self.hand_gap(state, seat)
            };
            let unlisted: Vec<(u32, Zone)> = self
                .unlisted
                .iter()
                .filter(|(id, _)| self.objects.get(*id).is_some_and(|o| o.owner == seat))
                .copied()
                .collect();
            if library_gap != 0 || hand_gap != 0 || !unlisted.is_empty() {
                differs = true;
            }
            truth.push(Truth {
                seat,
                library_gap,
                hand_surplus: u32::try_from(-hand_gap).unwrap_or(0),
                unlisted,
            });
        }
        self.truth = differs.then_some(truth);
    }

    /// Brings the record in line with Forge: the library size, a hand that
    /// held too many objects, and the cards listed nowhere that are still
    /// where the record had them (an event since may have moved one). The
    /// lowest hidden hand objects end first, then the lowest named ones (a
    /// card the record knew went back to that hand).
    fn repair(&mut self, truth: Truth) {
        let library = self.objects.library(truth.seat) + truth.library_gap;
        self.objects.set_library(truth.seat, library);
        for (id, zone) in truth.unlisted {
            if self
                .objects
                .get(id)
                .is_some_and(|object| object.zone == zone)
            {
                self.objects.end(id);
            }
        }
        for _ in 0..truth.hand_surplus {
            let hidden = self.objects.lowest_hidden_in_hand(truth.seat);
            let Some(id) = hidden.or_else(|| self.objects.lowest_in_hand(truth.seat)) else {
                break;
            };
            self.objects.end(id);
        }
    }
}
