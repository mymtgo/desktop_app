//! The board by state diff.
//!
//! Endstep sends a full state with every board change. `Board::diff` compares
//! one with the objects the record holds (`objects.rs`) and writes what is
//! missing: new cards, moves, tokens, hidden cards, and what changed on a
//! permanent. Whatever wrote a change before the state came (a pregame event
//! here, a game event in `events.rs`) went through the same `Board`, so the
//! diff only writes what the record still lacks. For what it does write, the
//! events since the last state leave `Hints`: a card found by a search is not
//! drawn, a card an event put from a hand into a graveyard is discarded, and
//! a token takes the cause of what made it.
//!
//! Where an unseen card comes from or goes to is decided by Forge's library
//! sizes: a library that shrank more than the record's moves explain gave
//! the card, one that grew took it. A hidden hand is counted the same way
//! against `handSize`. When a draw and a play reach the record in one full
//! state the hand size is unchanged, so an opponent card shows as `new card
//! ... in library` plus a move from the library, not as a hidden draw then a
//! play from hand.
//!
//! What no statement can bring in line (a library size, a hidden hand that
//! shrank with nothing to show for it, a named card listed nowhere) is noted
//! as drift and repaired at the next keyframe (`keyframe.rs`).

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{Cause, Entering, LibraryPlace, MoveAction, ObjRef, PtValue, Statement, Zone};
use endstep_protocol::GameState;

use super::keyframe::Truth;
use super::objects::{Objects, Permanent, Tracked};
use super::shown::{Shown, hand_size, player_of, shown_cards};

/// A statement and its cause, before the writer gives it a time and level.
pub(super) type Written = (Statement, Option<Cause>);

/// A record has two players: seats 0 and 1.
pub(super) const SEATS: [u32; 2] = [0, 1];

/// How a move is written, beyond its zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum How {
    Plain,
    /// `as draw`: the top card of the library.
    Draw,
    /// `as mill`: the top card of the library, into the graveyard.
    Mill,
    /// `as discard`: from a hand into the graveyard.
    Discard,
    /// Onto the bottom of the library.
    Bottom,
}

impl How {
    fn action(self) -> Option<MoveAction> {
        match self {
            How::Draw => Some(MoveAction::Draw),
            How::Mill => Some(MoveAction::Mill),
            How::Discard => Some(MoveAction::Discard),
            How::Plain | How::Bottom => None,
        }
    }

    /// A draw and a mill take the top card of the library.
    fn takes_top(self) -> bool {
        matches!(self, How::Draw | How::Mill)
    }
}

/// What the events since the last full state say about the moves the next
/// diff writes. The diff clears them.
#[derive(Debug, Default)]
pub(super) struct Hints {
    /// Forge ids a reveal into a hand named: their move out of the library
    /// is a card found, not a draw.
    pub(super) found: BTreeSet<u64>,
    /// Forge ids of cards an event put from a hand into a graveyard.
    pub(super) discarded: BTreeSet<u64>,
    /// The spell or ability that resolved: the cause of what it did.
    pub(super) cause: Option<Cause>,
    /// A token was created.
    pub(super) token: bool,
}

/// One game's board as the record's lines leave it.
pub(super) struct Board {
    pub(super) user_seat: u32,
    names: BTreeMap<u32, String>,
    pub(super) objects: Objects,
    pub(super) life: BTreeMap<u32, i32>,
    pub(super) poison: BTreeMap<u32, u32>,
    /// Forge's numbers by seat, when the record's differ after the last
    /// full state (`keyframe.rs`).
    pub(super) truth: Option<Vec<Truth>>,
    pub(super) hints: Hints,
    /// The named objects (with their zone) the last full state listed
    /// nowhere, with nothing to explain it: drift (`keyframe.rs`).
    pub(super) unlisted: Vec<(u32, Zone)>,
    /// No turn has begun: a hand drawn now is an opening hand.
    pregame: bool,
    /// The seats that took a mulligan.
    mulliganed: BTreeSet<u32>,
}

impl Board {
    pub(super) fn new(user_seat: u32, names: BTreeMap<u32, String>) -> Board {
        Board {
            user_seat,
            names,
            objects: Objects::default(),
            life: BTreeMap::new(),
            poison: BTreeMap::new(),
            truth: None,
            hints: Hints::default(),
            unlisted: Vec::new(),
            pregame: true,
            mulliganed: BTreeSet::new(),
        }
    }

    /// The first turn begins: draws are no longer opening hands.
    pub(super) fn end_pregame(&mut self) {
        self.pregame = false;
    }

    /// Why `seat` draws right now: the pregame's rule, or unknown in a turn
    /// (the diff cannot tell a draw step from a spell).
    fn draw_cause(&self, seat: u32) -> Cause {
        if !self.pregame {
            Cause::Unknown
        } else if self.mulliganed.contains(&seat) {
            Cause::Rule("mulligan".to_string())
        } else {
            Cause::Rule("opening-hand".to_string())
        }
    }

    pub(super) fn name(&self, seat: u32) -> String {
        self.names.get(&seat).cloned().unwrap_or_default()
    }

    /// A mulligan: the seat's hand goes back into its library. A hidden
    /// hand cannot be told apart afterwards, so its library is shuffled
    /// (ending those ids); the user's cards keep their objects, as Forge
    /// keeps their ids.
    pub(super) fn send_hand_back(&mut self, seat: u32) -> Vec<Written> {
        self.mulliganed.insert(seat);
        let mut out = Vec::new();
        let hand = self
            .objects
            .ids_where(|object| object.owner == seat && object.zone == Zone::Hand);
        for id in hand {
            let cause = Cause::Rule("mulligan".to_string());
            self.move_object(&mut out, id, Zone::Library, How::Plain, None, cause);
        }
        if seat != self.user_seat {
            out.push((
                Statement::Shuffle {
                    player: self.name(seat),
                },
                Some(Cause::Rule("mulligan".to_string())),
            ));
            self.objects.shuffle(seat);
        }
        out
    }

    /// One card bottomed after a keep: the first of the user's `bottomed`
    /// Forge ids still in hand, or the lowest hidden card of another seat.
    pub(super) fn bottom(&mut self, seat: u32, bottomed: &[u64]) -> Vec<Written> {
        let id = if seat == self.user_seat {
            bottomed
                .iter()
                .filter_map(|forge_id| self.objects.id_of(*forge_id))
                .find(|id| {
                    self.objects
                        .get(*id)
                        .is_some_and(|object| object.zone == Zone::Hand)
                })
        } else {
            self.objects.lowest_hidden_in_hand(seat)
        };
        let mut out = Vec::new();
        if let Some(id) = id {
            let cause = Cause::Rule("mulligan".to_string());
            self.move_object(&mut out, id, Zone::Library, How::Bottom, None, cause);
        }
        out
    }

    /// A life total the record does not have yet.
    pub(super) fn set_life(&mut self, seat: u32, life: i32) -> Option<Written> {
        let old = self.life.get(&seat).copied()?;
        if old == life {
            return None;
        }
        self.life.insert(seat, life);
        Some((
            Statement::Life {
                player: self.name(seat),
                from: old,
                to: life,
            },
            Some(Cause::Unknown),
        ))
    }

    /// Poison counters the record does not have yet. No frame says what
    /// gave them.
    fn set_poison(&mut self, seat: u32, poison: u32) -> Option<Written> {
        let old = self.poison.get(&seat).copied()?;
        if old == poison {
            return None;
        }
        self.poison.insert(seat, poison);
        Some((
            Statement::Poison {
                player: self.name(seat),
                from: old,
                to: poison,
            },
            Some(Cause::Unknown),
        ))
    }

    /// Writes a move and applies it. Every zone change goes through here,
    /// whoever knows its cause.
    pub(super) fn move_object(
        &mut self,
        out: &mut Vec<Written>,
        id: u32,
        to: Zone,
        how: How,
        permanent: Option<&Permanent>,
        cause: Cause,
    ) {
        let Some(object) = self.objects.get(id) else {
            return;
        };
        let entering = match (to, permanent) {
            (Zone::Battlefield, Some(permanent)) => Some(entering(permanent)),
            _ => None,
        };
        out.push((
            Statement::Move {
                object: object.obj(),
                from: object.zone,
                to,
                action: how.action(),
                position: (how == How::Bottom).then_some(LibraryPlace::Bottom),
                entering,
                with: None,
            },
            Some(cause),
        ));
        self.objects
            .relocate(id, to, how.takes_top(), how == How::Bottom);
        if let (Zone::Battlefield, Some(permanent)) = (to, permanent) {
            self.objects.enter(id, permanent);
        }
    }

    /// Everything a full state shows that the record does not have yet.
    pub(super) fn diff(&mut self, state: &GameState) -> Vec<Written> {
        let shown = shown_cards(state);
        let mut out = Vec::new();
        self.move_known(&shown, &mut out);
        self.settle_unlisted(state, &shown, &mut out);
        self.introduce(state, &shown, &mut out);
        self.balance_hidden_hands(state, &mut out);
        self.sync_permanents(&shown, &mut out);
        for seat in SEATS {
            let player = player_of(state, seat);
            if let Some(life) = player.and_then(|player| player.life) {
                out.extend(self.set_life(seat, life));
            }
            if let Some(poison) = player.and_then(|player| player.poison) {
                out.extend(self.set_poison(seat, poison));
            }
        }
        self.note_truth(state);
        self.hints = Hints::default();
        out
    }

    /// How many more cards Forge's library holds than the record's: above
    /// zero a card went in unseen, below zero one came out.
    pub(super) fn library_gap(&self, state: &GameState, seat: u32) -> i64 {
        match player_of(state, seat).and_then(|player| player.library_size) {
            Some(size) => i64::from(size) - self.objects.library(seat),
            None => 0,
        }
    }

    /// A known object the state shows in another zone. A named card in a
    /// hand Forge hides is never listed, so a state that shows its Forge id
    /// under another name has given that id to another card (Forge reissues
    /// ids after a cast taken back): the id is dropped and the card is new.
    fn move_known(&mut self, shown: &[Shown], out: &mut Vec<Written>) {
        for card in shown {
            let Some(id) = self.objects.id_of(card.forge_id) else {
                continue;
            };
            let Some(object) = self.objects.get(id) else {
                continue;
            };
            let in_hidden_hand = object.owner != self.user_seat && object.zone == Zone::Hand;
            if in_hidden_hand && object.name.as_deref() != Some(card.name.as_str()) {
                self.objects.forget(card.forge_id);
                continue;
            }
            let from = object.zone;
            if from != card.zone {
                self.move_from(out, id, from, card);
            }
        }
    }

    /// The move of `card`'s object from `from` to where the state shows it.
    /// Out of the library into the hand it is a draw, unless a reveal named
    /// the card: a search found it, and what resolved is the cause. Out of
    /// the library into the graveyard it is milled, unless an event said it
    /// was discarded: it was drawn and discarded between two states, and the
    /// move is plain. Out of the hand into the graveyard it is discarded when
    /// an event said so (the diff alone cannot tell a discard from a spell
    /// cast and resolved unseen).
    fn move_from(&mut self, out: &mut Vec<Written>, id: u32, from: Zone, card: &Shown) {
        let found = self.hints.found.contains(&card.forge_id);
        let discarded = self.hints.discarded.contains(&card.forge_id);
        let (how, cause) = match (from, card.zone) {
            (Zone::Library, Zone::Hand) if found => (How::Plain, self.hinted_cause()),
            (Zone::Library, Zone::Hand) => (How::Draw, self.draw_cause(card.owner)),
            (Zone::Library, Zone::Graveyard) if discarded => (How::Plain, Cause::Unknown),
            (Zone::Library, Zone::Graveyard) => (How::Mill, Cause::Unknown),
            (Zone::Hand, Zone::Graveyard) if discarded => (How::Discard, Cause::Unknown),
            _ => (How::Plain, Cause::Unknown),
        };
        let permanent = Some(&card.permanent);
        self.move_object(out, id, card.zone, how, permanent, cause);
    }

    /// The spell or ability the events say resolved, else unknown.
    fn hinted_cause(&self) -> Cause {
        self.hints.cause.clone().unwrap_or(Cause::Unknown)
    }

    /// Named objects the state lists nowhere. A token ceased to exist. A
    /// card went into its owner's library when that grew unseen, or into a
    /// hidden hand that grew; otherwise it stays where the record has it
    /// (Forge lists a spell nowhere while its cost is paid, trap 39) and is
    /// noted in `unlisted`: if no later state shows it before the next turn,
    /// that keyframe ends it.
    fn settle_unlisted(&mut self, state: &GameState, shown: &[Shown], out: &mut Vec<Written>) {
        self.unlisted.clear();
        let listed: BTreeSet<u32> = shown
            .iter()
            .filter_map(|card| self.objects.id_of(card.forge_id))
            .collect();
        let unlisted = self.objects.ids_where(|object| {
            object.name.is_some() && object.zone != Zone::Library && !listed.contains(&object.id)
        });
        for id in unlisted {
            let Some(object) = self.objects.get(id) else {
                continue;
            };
            let (owner, zone) = (object.owner, object.zone);
            let hidden_hand = owner != self.user_seat;
            if object.token {
                out.push((
                    Statement::Cease {
                        object: object.obj(),
                    },
                    Some(Cause::Unknown),
                ));
                self.objects.end(id);
            } else if hidden_hand && zone == Zone::Hand {
                // A named card in a hidden hand is never listed.
            } else if self.library_gap(state, owner) > 0 {
                self.move_object(out, id, Zone::Library, How::Plain, None, Cause::Unknown);
            } else if hidden_hand && self.hand_gap(state, owner) > 0 {
                self.move_object(out, id, Zone::Hand, How::Plain, None, Cause::Unknown);
            } else {
                self.unlisted.push((id, zone));
            }
        }
    }

    /// How many more cards Forge's hand holds than the record's.
    pub(super) fn hand_gap(&self, state: &GameState, seat: u32) -> i64 {
        match hand_size(state, seat) {
            Some(size) => i64::from(size) - i64::from(self.objects.hand_count(seat)),
            None => 0,
        }
    }

    /// Cards the state shows for the first time.
    fn introduce(&mut self, state: &GameState, shown: &[Shown], out: &mut Vec<Written>) {
        for card in shown {
            if self.objects.id_of(card.forge_id).is_some() {
                continue;
            }
            if card.token {
                // What resolved made it, when an event said a token was
                // created.
                let cause = if self.hints.token {
                    self.hinted_cause()
                } else {
                    Cause::Unknown
                };
                self.introduce_token(card, cause, out);
            } else if self.library_gap(state, card.owner) < 0 {
                // A library that gave a card explains it before a hand that
                // shrank: that hand's card may be a spell being paid for,
                // which Forge lists nowhere (trap 39).
                self.introduce_from_library(card, out);
            } else if let Some(hidden) = self.hidden_to_name(state, card) {
                self.identify(hidden, card, out);
            } else {
                self.introduce_in_place(card, out);
            }
        }
    }

    /// The hand card a new card of another seat is, when the record's hand
    /// holds more cards than Forge's: one already named so (named under a
    /// Forge id since reissued), else the lowest hidden one.
    fn hidden_to_name(&self, state: &GameState, card: &Shown) -> Option<u32> {
        if card.owner == self.user_seat || self.hand_gap(state, card.owner) >= 0 {
            return None;
        }
        self.objects
            .named_in_hand(card.owner, &card.name)
            .or_else(|| self.objects.lowest_hidden_in_hand(card.owner))
    }

    /// The hand object `id` is the shown card: named, unless it already
    /// has that name, then moved where the state shows it.
    fn identify(&mut self, id: u32, card: &Shown, out: &mut Vec<Written>) {
        let named = self
            .objects
            .get(id)
            .is_some_and(|object| object.name.as_deref() == Some(card.name.as_str()));
        if !named {
            out.push((
                Statement::Identify {
                    id,
                    name: Some(card.name.clone()),
                    sources: Vec::new(),
                },
                None,
            ));
        }
        self.objects.identify(id, card.forge_id, &card.name);
        if card.zone != Zone::Hand {
            let permanent = Some(&card.permanent);
            self.move_object(out, id, card.zone, How::Plain, permanent, Cause::Unknown);
        }
    }

    /// A token is new where it is.
    pub(super) fn introduce_token(&mut self, card: &Shown, cause: Cause, out: &mut Vec<Written>) {
        let id = self.add_card(card, card.zone);
        out.push((
            Statement::NewToken {
                object: self.obj(id),
                owner: self.name(card.owner),
                zone: card.zone,
                entering: entering(&card.permanent),
                sources: Vec::new(),
            },
            Some(cause),
        ));
        self.objects.enter(id, &card.permanent);
    }

    /// A card from the cards of a library the record does not track.
    fn introduce_from_library(&mut self, card: &Shown, out: &mut Vec<Written>) {
        let id = self.add_card(card, Zone::Library);
        out.push((self.new_card(id, card.owner, Zone::Library, None), None));
        if card.zone != Zone::Library {
            self.move_from(out, id, Zone::Library, card);
        }
    }

    /// A card the record first sees where it is, with no library to have
    /// given it: a game joined late, or a card from outside the zones.
    fn introduce_in_place(&mut self, card: &Shown, out: &mut Vec<Written>) {
        let id = self.add_card(card, card.zone);
        let on_battlefield = card.zone == Zone::Battlefield;
        let entering = on_battlefield.then(|| entering(&card.permanent));
        out.push((self.new_card(id, card.owner, card.zone, entering), None));
        if on_battlefield {
            self.objects.enter(id, &card.permanent);
        }
    }

    fn add_card(&mut self, card: &Shown, zone: Zone) -> u32 {
        let name = Some(card.name.clone());
        self.objects
            .add(Some(card.forge_id), name, card.owner, zone, card.token)
    }

    pub(super) fn obj(&self, id: u32) -> ObjRef {
        match self.objects.get(id) {
            Some(object) => object.obj(),
            None => ObjRef { id, name: None },
        }
    }

    pub(super) fn new_card(
        &self,
        id: u32,
        owner: u32,
        zone: Zone,
        entering: Option<Entering>,
    ) -> Statement {
        Statement::NewCard {
            object: self.obj(id),
            owner: self.name(owner),
            zone,
            position: None,
            entering,
            sources: Vec::new(),
        }
    }

    /// The hands Forge hides, brought to `handSize`: a hand that grew drew
    /// hidden cards when the library gave them (else they are just there),
    /// and one that shrank put its lowest hidden card into a library that
    /// took one. A hand that shrank with nothing to show for it is left: the
    /// card is being cast, and is named when the stack shows it.
    fn balance_hidden_hands(&mut self, state: &GameState, out: &mut Vec<Written>) {
        for seat in SEATS {
            if seat == self.user_seat {
                continue;
            }
            while self.hand_gap(state, seat) > 0 {
                self.add_hidden(state, seat, out);
            }
            while self.hand_gap(state, seat) < 0 && self.library_gap(state, seat) > 0 {
                let Some(id) = self.objects.lowest_hidden_in_hand(seat) else {
                    break;
                };
                self.move_object(out, id, Zone::Library, How::Plain, None, Cause::Unknown);
            }
        }
    }

    fn add_hidden(&mut self, state: &GameState, seat: u32, out: &mut Vec<Written>) {
        let drawn = self.library_gap(state, seat) < 0;
        let zone = if drawn { Zone::Library } else { Zone::Hand };
        let id = self.objects.add(None, None, seat, zone, false);
        out.push((
            Statement::NewHidden {
                id,
                owner: self.name(seat),
                zone,
                position: None,
            },
            None,
        ));
        if drawn {
            let cause = self.draw_cause(seat);
            self.move_object(out, id, Zone::Hand, How::Draw, None, cause);
        }
    }

    /// The untaps of `seat`'s untap step: each permanent it controls that
    /// the record has tapped and `state` shows untapped.
    pub(super) fn untap_step(&mut self, seat: u32, state: &GameState) -> Vec<Written> {
        let mut out = Vec::new();
        for card in shown_cards(state) {
            if card.zone != Zone::Battlefield || card.permanent.tapped {
                continue;
            }
            let Some(id) = self.objects.id_of(card.forge_id) else {
                continue;
            };
            let Some(object) = self.objects.get_mut(id) else {
                continue;
            };
            if object.zone != Zone::Battlefield || !object.tapped || object.controller != seat {
                continue;
            }
            object.tapped = false;
            let object = object.obj();
            out.push((
                Statement::Untap { object },
                Some(Cause::Rule("untap".to_string())),
            ));
        }
        out
    }

    /// What changed on the permanents the state shows.
    fn sync_permanents(&mut self, shown: &[Shown], out: &mut Vec<Written>) {
        for card in shown {
            if card.zone != Zone::Battlefield {
                continue;
            }
            let Some(id) = self.objects.id_of(card.forge_id) else {
                continue;
            };
            let Some(before) = self.objects.get(id).cloned() else {
                continue;
            };
            if before.zone != Zone::Battlefield {
                continue;
            }
            let attached_to = card
                .attached_forge
                .and_then(|forge_id| self.objects.id_of(forge_id));
            let changes = permanent_changes(&before, card, attached_to, self);
            for statement in changes {
                out.push((statement, Some(Cause::Unknown)));
            }
            if let Some(object) = self.objects.get_mut(id) {
                object.controller = card.controller;
                object.tapped = card.permanent.tapped;
                object.types = card.permanent.types.clone();
                object.pt = card.permanent.pt;
                object.counters = card.counters.clone();
                object.attached_to = attached_to;
            }
        }
    }
}

/// The statements that bring a permanent from `before` to what the state
/// shows.
fn permanent_changes(
    before: &Tracked,
    card: &Shown,
    attached_to: Option<u32>,
    board: &Board,
) -> Vec<Statement> {
    let object = before.obj();
    let mut out = Vec::new();
    if before.controller != card.controller {
        out.push(Statement::Control {
            object: object.clone(),
            player: board.name(card.controller),
        });
    }
    if before.tapped != card.permanent.tapped {
        out.push(if card.permanent.tapped {
            Statement::Tap {
                object: object.clone(),
            }
        } else {
            Statement::Untap {
                object: object.clone(),
            }
        });
    }
    if before.types != card.permanent.types {
        out.push(Statement::TypesChange {
            object: object.clone(),
            from: before.types.clone(),
            to: card.permanent.types.clone(),
        });
    }
    if before.pt != card.permanent.pt {
        out.push(Statement::PtChange {
            object: object.clone(),
            from: pt_value(before.pt),
            to: pt_value(card.permanent.pt),
        });
    }
    let mut kinds: BTreeSet<&String> = before.counters.keys().collect();
    kinds.extend(card.counters.keys());
    for kind in kinds {
        let from = before.counters.get(kind).copied().unwrap_or(0);
        let to = card.counters.get(kind).copied().unwrap_or(0);
        if from != to {
            out.push(Statement::Counters {
                object: object.clone(),
                kind: kind.clone(),
                from,
                to,
            });
        }
    }
    if before.attached_to != attached_to {
        out.push(match attached_to {
            Some(host) => Statement::Attach {
                object,
                to: board.obj(host),
            },
            None => Statement::Unattach { object },
        });
    }
    out
}

fn pt_value(pt: Option<(i32, i32)>) -> PtValue {
    match pt {
        Some((power, toughness)) => PtValue::Pt(power, toughness),
        None => PtValue::None,
    }
}

pub(super) fn entering(permanent: &Permanent) -> Entering {
    Entering {
        face: None,
        tapped: permanent.tapped,
        types: permanent.types.clone(),
        pt: permanent.pt,
    }
}
