//! Zone moves (spec section 7.1): each `ZoneTransfer` as `play` (`PlayLand`),
//! `cast` (`CastSpell`), or `move` with its action (`Draw`, `Discard`,
//! `Sacrifice`, `Destroy`, `Mill`), and shuffles.
//!
//! A card seen for the first time is introduced in the zone it leaves:
//! `new card` when the local player knows its name by the time it leaves
//! (spec 6.5, decision 0019), else `new hidden`. The local player knows the
//! cards in a public zone and in their own hand, a library card as it goes
//! to a public zone (Collected Company's cards), and their own library card
//! as they draw it. A hidden card already introduced (an opponent's card in
//! hand) is identified just before the move that makes it public, so no
//! card is named while it stays hidden.
//!
//! A spell Arena moves as `Countered` is written `countered`, then its move,
//! both by the counterspell.
//!
//! A move into a library goes on the `top` or `bottom` when the state
//! shows it there (`library.rs`).
//!
//! Tokens move like cards once the record tracks them (`objects.rs`
//! introduces them). Abilities are not moved.

use ::mtgr::{
    CardType, Cause, Entering, LibraryEnd, LibraryPlace, MoveAction, ObjRef, Position, Statement,
    Zone,
};

use super::ids::Kind;
use super::library::{LIBRARY, random_end, transfers_of};
use super::objects::TAPPED_UNTAPPED;
use super::{GameWriter, Named};
use crate::{Annotation, CARD, CardKey, GameObject, Transfer};

/// Arena's persistent annotation that shows a card under the card it is
/// exiled with (affector: the source, affected: the exiled card).
const DISPLAY_CARD_UNDER_CARD: &str = "AnnotationType_DisplayCardUnderCard";

/// The object kind of a token (`GameObject::kind`).
pub(super) const TOKEN: &str = "GameObjectType_Token";

/// A `ZoneType_*` as a `.mtgr` zone (Limbo, Revealed, Pending, Sideboard and
/// Suppressed are not zones the record has).
pub(super) fn zone_of(kind: &str) -> Option<Zone> {
    match kind {
        "ZoneType_Library" => Some(Zone::Library),
        "ZoneType_Hand" => Some(Zone::Hand),
        "ZoneType_Graveyard" => Some(Zone::Graveyard),
        "ZoneType_Battlefield" => Some(Zone::Battlefield),
        "ZoneType_Stack" => Some(Zone::Stack),
        "ZoneType_Exile" => Some(Zone::Exile),
        "ZoneType_Command" => Some(Zone::Command),
        "ZoneType_Sideboard" => Some(Zone::OutsideSideboard),
        _ => None,
    }
}

/// A permanent's card types, in the standard's order.
pub(super) fn card_types(object: &GameObject) -> Vec<CardType> {
    let known = [
        ("CardType_Artifact", CardType::Artifact),
        ("CardType_Battle", CardType::Battle),
        ("CardType_Creature", CardType::Creature),
        ("CardType_Enchantment", CardType::Enchantment),
        ("CardType_Kindred", CardType::Kindred),
        ("CardType_Land", CardType::Land),
        ("CardType_Planeswalker", CardType::Planeswalker),
    ];
    let mut types = Vec::new();
    for (arena, card_type) in known {
        if object.card_types.iter().any(|t| t == arena) {
            types.push(card_type);
        }
    }
    types
}

/// Where a card first seen as it leaves a zone is introduced: as it was on
/// the battlefield, at a known library position, or just in its zone.
pub(super) enum Introduced {
    Anywhere,
    Entering(Entering),
    At(Position),
}

/// Whether a zone's cards are public (CR 400.2).
pub(super) fn is_public(zone: Zone) -> bool {
    matches!(
        zone,
        Zone::Battlefield | Zone::Stack | Zone::Graveyard | Zone::Exile | Zone::Command
    )
}

/// A creature's power and toughness (Arena leaves zero values out).
pub(super) fn creature_pt(object: &GameObject) -> Option<(i32, i32)> {
    if card_types(object).contains(&CardType::Creature) {
        Some((object.power.unwrap_or(0), object.toughness.unwrap_or(0)))
    } else {
        None
    }
}

impl GameWriter<'_> {
    /// A card's name when the local player knows it: their own cards, and
    /// any card in a public zone (a `?` name when the card data does not
    /// list it). `None` for a hidden card.
    pub(super) fn visible_name(
        &mut self,
        grp_id: u32,
        owner_seat: u32,
        zone: Zone,
    ) -> Option<Named> {
        if grp_id == 0 {
            return None;
        }
        if owner_seat == self.roster.local_seat || is_public(zone) {
            Some(self.named(grp_id))
        } else {
            None
        }
    }

    /// Introduce a card in `zone`: `new card` with its name (`?` when the
    /// card data does not list it), or `new hidden`.
    /// A card introduced on the battlefield (after `incomplete`) says how it
    /// is there (`entering`, spec 5.3).
    pub(super) fn introduce(
        &mut self,
        key: CardKey,
        name: Option<Named>,
        owner_seat: u32,
        zone: Zone,
        entering: Option<Entering>,
    ) -> ObjRef {
        let object = self.introduce_card(key, name, owner_seat, zone, None, entering.clone());
        if let Some(entering) = entering {
            self.entered(key, &entering);
        }
        object
    }

    /// Introduce a card at a known position in a library (`library.rs`).
    pub(super) fn introduce_at(
        &mut self,
        key: CardKey,
        name: Option<Named>,
        owner_seat: u32,
        zone: Zone,
        position: Option<Position>,
    ) -> ObjRef {
        self.introduce_card(key, name, owner_seat, zone, position, None)
    }

    fn introduce_card(
        &mut self,
        key: CardKey,
        name: Option<Named>,
        owner_seat: u32,
        zone: Zone,
        position: Option<Position>,
        entering: Option<Entering>,
    ) -> ObjRef {
        let hidden = name.is_none();
        let sources = name.as_ref().map(Named::sources).unwrap_or_default();
        let name = name.and_then(|named| named.name);
        let id = self
            .ids
            .introduce(key, Kind::Card, name.clone(), hidden, zone, owner_seat);
        let owner = self.roster.name(owner_seat);
        let object = ObjRef { id, name };
        let statement = if hidden {
            Statement::NewHidden {
                id,
                owner,
                zone,
                position,
            }
        } else {
            Statement::NewCard {
                object: object.clone(),
                owner,
                zone,
                position,
                entering,
                sources,
            }
        };
        self.event(statement, None);
        object
    }

    /// The transfers of one `ZoneTransfer` annotation of state `index` (one
    /// per affected id, in the fold's order).
    pub(super) fn write_transfers(&mut self, index: usize, annotation: &Annotation) {
        let fold = self.fold;
        let transfers = &fold.transfers;
        let mut taken = 0;
        while taken < annotation.affected_ids.len()
            && self.next_transfer < transfers.len()
            && transfers[self.next_transfer].state_index == index
        {
            let transfer = &transfers[self.next_transfer];
            self.next_transfer += 1;
            taken += 1;
            self.write_transfer(index, transfer, annotation);
        }
    }

    fn write_transfer(&mut self, index: usize, transfer: &Transfer, annotation: &Annotation) {
        let tracked_token = transfer.kind == TOKEN && self.ids.get(transfer.card).is_some();
        if !(transfer.kind == CARD || transfer.kind.is_empty() || tracked_token)
            || self.untold_copies.contains(&transfer.card)
        {
            return;
        }
        let (Some(from), Some(to)) = (zone_of(&transfer.from_zone), zone_of(&transfer.to_zone))
        else {
            self.unmapped_move(transfer);
            return;
        };
        if self.is_new_copy(index, transfer) {
            // Arena's `Copy` transfer makes the copy where it goes; a copy
            // found elsewhere (no transfer made it) is introduced where it
            // leaves, cause unknown.
            if transfer.category == "Copy" {
                let cause = self.move_cause("Copy", annotation);
                self.introduce_copy(index, transfer, to, cause);
                return;
            }
            if !self.introduce_copy(index, transfer, from, Cause::Unknown) {
                return;
            }
        }
        if from == Zone::OutsideSideboard {
            self.write_companion_special(transfer);
        }
        let seat = transfer.owner_seat;
        // Cards put on the bottom after a mulligan.
        let bottomed = self.turn == 0 && from == Zone::Hand && to == Zone::Library;
        let known_in_source = self.known_in(seat, from, to);
        // A tracked token already has its name (and no card data). A card
        // is named by the card it shows a face or half of.
        let name = if tracked_token {
            None
        } else {
            let grp_id = self.card_grp_at(index, transfer.instance_id, transfer.grp_id);
            self.visible_name(grp_id, seat, to)
        };
        if to == Zone::Stack {
            self.targeted.remove(&transfer.card);
            self.target_zones.remove(&transfer.card);
            self.x_chosen.remove(&transfer.card);
        }
        if from == Zone::Stack {
            self.left_stack(transfer.card);
        }
        // A card the record never saw enter the battlefield (after a gap) is
        // introduced there as it was before it moved.
        let on_battlefield = (from == Zone::Battlefield && self.ids.get(transfer.card).is_none())
            .then(|| self.entering_before(index, transfer));
        // A card first seen as it leaves a library is introduced where it
        // was, when that is known (`library.rs`).
        let leaving_at = (from == Zone::Library && self.ids.get(transfer.card).is_none())
            .then(|| {
                let action = match transfer.category.as_str() {
                    "PlayLand" | "CastSpell" | "Countered" => None,
                    category => move_action(category),
                };
                self.leaving_position(index, transfer, action)
            })
            .flatten();
        let at = match (on_battlefield, leaving_at) {
            (Some(entering), _) => Introduced::Entering(entering),
            (None, Some(position)) => Introduced::At(position),
            (None, None) => Introduced::Anywhere,
        };
        let object = self.object_leaving(transfer.card, name, known_in_source, seat, from, at);
        // A spell with targets that Arena moves off the stack with category
        // `nil` and no affector, without resolving, fizzled
        // (`abilities::fizzles`).
        let fizzled = from == Zone::Stack
            && transfer.category == "nil"
            && annotation.affector_id == 0
            && !self
                .ids
                .get(transfer.card)
                .is_some_and(|tracked| tracked.resolving)
            && self.fizzles(transfer.card);
        if fizzled {
            self.write_fizzle(object.clone());
        }
        let fold = self.fold;
        let state = &fold.states[index];
        let entered = state.objects.get(&transfer.instance_id);
        let entering = self.entering(index, entered);
        let position = if to == Zone::Library {
            self.library_end(index, transfer)
        } else {
            None
        };
        let exiled_with = if to == Zone::Exile {
            self.exiled_with(index, transfer)
        } else {
            None
        };
        let (statement, cause) = match (transfer.category.as_str(), to) {
            ("PlayLand", Zone::Battlefield) => (
                Statement::Play {
                    object,
                    from,
                    entering,
                },
                None,
            ),
            ("CastSpell", Zone::Stack) => {
                if self.actions.contains_key(&transfer.instance_id) {
                    // Being cast: no one receives priority until it is.
                    self.announced.insert(transfer.card);
                }
                let face = entered.and_then(|cast| self.cast_face(state, cast));
                (Statement::Cast { object, from, face }, None)
            }
            ("Countered", _) => {
                // Arena reports the counter: the spell is countered by the
                // resolving counterspell, then leaves the stack by it.
                let cause = self.move_cause("Countered", annotation);
                self.event(
                    Statement::Countered {
                        object: object.clone(),
                    },
                    Some(cause.clone()),
                );
                (
                    Statement::Move {
                        object,
                        from,
                        to,
                        action: None,
                        position,
                        entering: (to == Zone::Battlefield).then_some(entering),
                        with: exiled_with,
                    },
                    Some(cause),
                )
            }
            (category, _) => (
                Statement::Move {
                    object,
                    from,
                    to,
                    action: move_action(category),
                    position,
                    entering: (to == Zone::Battlefield).then_some(entering),
                    with: exiled_with,
                },
                Some(if bottomed {
                    Cause::Rule("mulligan".into())
                } else if fizzled {
                    Cause::Rule("fizzle".into())
                } else {
                    self.move_cause(category, annotation)
                }),
            ),
        };
        let entered_as = match &statement {
            Statement::Play { entering, .. } => Some(entering.clone()),
            Statement::Move { entering, .. } => entering.clone(),
            _ => None,
        };
        // An Adventure that resolves goes on an adventure (CR 715.3d), and
        // its controller may cast the card from exile.
        let adventure_cast = self
            .ids
            .get(transfer.card)
            .and_then(|tracked| tracked.adventure_cast);
        let on_adventure = adventure_cast
            .filter(|_| from == Zone::Stack && to == Zone::Exile && transfer.category == "Resolve");
        let cast_as_adventure = (to == Zone::Stack && transfer.category == "CastSpell")
            .then_some(entered)
            .flatten()
            .filter(|object| self.is_adventure(state, object))
            .map(|object| object.controller_seat);
        self.event(statement, cause);
        self.ids.moved(transfer.card, to);
        if let Some(tracked) = self.ids.get_mut(transfer.card) {
            tracked.adventure = on_adventure;
            tracked.adventure_cast = cast_as_adventure;
            // A copy of a permanent spell becomes a token as it resolves,
            // and keeps its id (CR 608.3f, decision 0023 item 4). It is
            // tracked as a token only when Arena shows it as one, so that
            // Arena's `TokenDeleted` ends it; a copy Arena still shows as a
            // card stays a copy and ceases as copies do.
            let shown_as_token = entered.is_some_and(|object| object.kind == TOKEN);
            if tracked.kind == Kind::Copy
                && from == Zone::Stack
                && to == Zone::Battlefield
                && shown_as_token
            {
                tracked.kind = Kind::Token;
            }
        }
        if let Some(source) = exiled_with {
            self.ids.link(transfer.card, source);
        }
        if let Some(entering) = entered_as {
            self.entered(transfer.card, &entering);
        }
    }

    /// The id of the object a card moved to exile is exiled with: Arena
    /// shows the card under its source (a `DisplayCardUnderCard` persistent
    /// annotation from the source to the card, in the state that exiles it).
    /// `None` when the state has no such annotation or the record does not
    /// track the source.
    fn exiled_with(&self, index: usize, transfer: &Transfer) -> Option<u32> {
        let fold = self.fold;
        fold.states[index]
            .persistent
            .values()
            .find(|annotation| {
                annotation.is(DISPLAY_CARD_UNDER_CARD)
                    && annotation.affected_ids.contains(&transfer.instance_id)
            })
            .and_then(|annotation| self.ids.get(fold.identity.key(annotation.affector_id)))
            .map(|source| source.id)
    }

    /// Whether the local player knows a card's name by the time it leaves
    /// the zone it is introduced in (spec 6.5, decision 0019): a public
    /// zone, their own hand, a library card that goes to a public zone, or
    /// their own library card they draw.
    pub(super) fn known_in(&self, owner_seat: u32, from: Zone, to: Zone) -> bool {
        let local = owner_seat == self.roster.local_seat;
        is_public(from)
            || (local && from == Zone::Hand)
            || (from == Zone::Library && is_public(to))
            || (local && from == Zone::Library && to == Zone::Hand)
    }

    /// Record how a permanent entered the battlefield.
    pub(super) fn entered(&mut self, key: CardKey, entering: &Entering) {
        if let Some(tracked) = self.ids.get_mut(key) {
            tracked.face = entering.face.clone();
            tracked.tapped = entering.tapped;
            tracked.types = entering.types.clone();
            tracked.pt = entering.pt;
        }
    }

    /// The card a move takes from `from`: introduced there if it is new
    /// (named only when `known_in_source`), identified first if the move
    /// makes its hidden name known.
    pub(super) fn object_leaving(
        &mut self,
        key: CardKey,
        name: Option<Named>,
        known_in_source: bool,
        owner_seat: u32,
        from: Zone,
        at: Introduced,
    ) -> ObjRef {
        let Some(tracked) = self.ids.get(key) else {
            let shown = if known_in_source { name.clone() } else { None };
            let object = match at {
                Introduced::At(position) => {
                    self.introduce_at(key, shown, owner_seat, from, Some(position))
                }
                Introduced::Entering(entering) => {
                    self.introduce(key, shown, owner_seat, from, Some(entering))
                }
                Introduced::Anywhere => self.introduce(key, shown, owner_seat, from, None),
            };
            let hidden = self.ids.get(key).is_some_and(|tracked| tracked.hidden);
            if hidden && let Some(named) = name {
                self.identify(key, object.id, named);
            }
            return self.ids.obj(key).unwrap_or(object);
        };
        let id = tracked.id;
        if tracked.hidden
            && let Some(named) = name
        {
            self.identify(key, id, named);
        }
        self.ids.obj(key).unwrap_or(ObjRef { id, name: None })
    }

    /// `#8 identify "Name" src arena:N` (or `identify ?`): a hidden card
    /// becomes known.
    pub(super) fn identify(&mut self, key: CardKey, id: u32, named: Named) {
        self.event(
            Statement::Identify {
                id,
                name: named.name.clone(),
                sources: named.sources(),
            },
            None,
        );
        self.ids.identify(key, named.name);
    }

    /// A permanent as it was before a transfer moved it: the object under its
    /// previous id in the state before, else the moved object.
    pub(super) fn entering_before(&mut self, index: usize, transfer: &Transfer) -> Entering {
        let fold = self.fold;
        let mut found = None;
        if let (Some(previous), Some(at)) = (transfer.previous_id, index.checked_sub(1)) {
            found = fold.states[at]
                .objects
                .get(&previous)
                .map(|object| (at, object));
        }
        if found.is_none() {
            found = fold.states[index]
                .objects
                .get(&transfer.instance_id)
                .map(|object| (index, object));
        }
        let Some((at, object)) = found else {
            return Entering::default();
        };
        Entering {
            face: self.shown_face(&fold.states[at], object),
            tapped: object.tapped,
            types: card_types(object),
            pt: creature_pt(object),
        }
    }

    /// How a permanent enters: tapped when the state shows it tapped and no
    /// annotation of the state tapped it.
    pub(super) fn entering(&mut self, index: usize, object: Option<&GameObject>) -> Entering {
        let Some(object) = object else {
            return Entering::default();
        };
        let tapped_later = self.fold.steps[index].annotations.iter().any(|annotation| {
            annotation.is(TAPPED_UNTAPPED)
                && annotation.int("tapped") == Some(1)
                && annotation.affected_ids.contains(&object.instance_id)
        });
        let fold = self.fold;
        Entering {
            face: self.shown_face(&fold.states[index], object),
            tapped: object.tapped && !tapped_later,
            types: card_types(object),
            pt: creature_pt(object),
        }
    }

    /// A move's cause: `rule:resolve` for a resolving spell, `rule:sba` for
    /// a state-based action (`SBA_*`), `rule:draw` for the draw step's
    /// draw, `rule:cleanup` for the cleanup step's discard to hand size
    /// (CR 514.1), else what Arena names: the cost of an ability being activated
    /// (a sacrifice), or the object, or unknown.
    pub(super) fn move_cause(&self, category: &str, annotation: &Annotation) -> Cause {
        if category == "Resolve" {
            return Cause::Rule("resolve".into());
        }
        if category.starts_with("SBA_") {
            return Cause::Rule("sba".into());
        }
        if category == "Draw" && annotation.affector_id == 0 && self.last_step == Some((1, 3)) {
            return Cause::Rule("draw".into());
        }
        if category == "Discard" && annotation.affector_id == 0 && self.last_step == Some((5, 10)) {
            return Cause::Rule("cleanup".into());
        }
        self.cost_or_effect(annotation.affector_id)
    }

    /// `by #N` for an Arena instance id with a live id, else `by ?`.
    pub(super) fn object_cause(&self, instance_id: u32) -> Cause {
        if instance_id == 0 {
            return Cause::Unknown;
        }
        let key = self.fold.identity.key(instance_id);
        match self.ids.get(key) {
            Some(tracked) => Cause::Object(tracked.id),
            None => Cause::Unknown,
        }
    }

    /// `AnnotationType_Shuffle`: the library of the affected seat. Its
    /// tracked cards lose their ids.
    ///
    /// Arena also reports "put the rest on the bottom of your library in a
    /// random order" as a `Shuffle` of only those cards (three-games log
    /// line 12186, `library::random_end`): that is no shuffle of the
    /// library (CR 701.24a), so no line is written, and a tracked card
    /// among two or more can no longer be told apart (`untracked`). One
    /// card renumbered on an end is in a known place (`one_card_at_an_end`).
    pub(super) fn write_shuffle(&mut self, index: usize, annotation: &Annotation) {
        let Some(seat) = annotation.affected_ids.first().copied() else {
            return;
        };
        let old_ids = annotation.ints("OldIds");
        let random = if old_ids.is_empty() {
            None
        } else {
            random_end(self.fold, index, annotation)
        };
        if let Some((end, count)) = random {
            if count == 1 {
                self.one_card_at_an_end(index, annotation, end);
                return;
            }
            for old_id in old_ids {
                let Ok(old_id) = u32::try_from(old_id) else {
                    continue;
                };
                let key = self.fold.identity.key(old_id);
                let in_library = self
                    .ids
                    .get(key)
                    .is_some_and(|tracked| tracked.zone == Zone::Library);
                if in_library && let Some(object) = self.ids.obj(key) {
                    self.event(Statement::Untracked { object }, None);
                    self.ids.end(key);
                }
            }
            return;
        }
        let player = self.roster.name(seat);
        let cause = self.object_cause(annotation.affector_id);
        self.event(Statement::Shuffle { player }, Some(cause));
        self.ids.end_library(seat);
    }

    /// One card a `Shuffle` renumbered on an end of its library keeps its
    /// identity (the fold follows `OldIds` to `NewIds`). When a transfer of
    /// the message put it there, its move already says where
    /// (`library_end`); otherwise Arena moved it without a transfer ("put
    /// the other on the bottom", `bo1-wins-and-concede` line 10654): a
    /// tracked card moves to that end when the perspective player can tell
    /// it went there (their own card, or one public in the library).
    /// Otherwise which hidden card went there is not theirs to know (the
    /// opponent's scried card, then "one into your hand, the other on the
    /// bottom"), and the card is `untracked`.
    fn one_card_at_an_end(&mut self, index: usize, annotation: &Annotation, end: LibraryEnd) {
        let fold = self.fold;
        let Some(old_id) = annotation
            .ints("OldIds")
            .first()
            .and_then(|id| u32::try_from(*id).ok())
        else {
            return;
        };
        let put_in = transfers_of(fold, index)
            .iter()
            .any(|transfer| transfer.to_zone == LIBRARY && transfer.instance_id == old_id);
        if put_in {
            return;
        }
        let key = fold.identity.key(old_id);
        let in_library = self
            .ids
            .get(key)
            .is_some_and(|tracked| tracked.zone == Zone::Library);
        if !in_library {
            return;
        }
        let Some(object) = self.ids.obj(key) else {
            return;
        };
        let seat = annotation.affected_ids.first().copied();
        let public = [index.checked_sub(1), Some(index)]
            .into_iter()
            .flatten()
            .filter_map(|at| fold.states[at].objects.get(&old_id))
            .any(|card| card.public);
        if seat != Some(self.roster.local_seat) && !public {
            self.event(Statement::Untracked { object }, None);
            self.ids.end(key);
            return;
        }
        let place = match end {
            LibraryEnd::Top => LibraryPlace::Top,
            LibraryEnd::Bottom => LibraryPlace::Bottom,
        };
        let cause = self.object_cause(annotation.affector_id);
        self.event(
            Statement::Move {
                object,
                from: Zone::Library,
                to: Zone::Library,
                action: None,
                position: Some(place),
                entering: None,
                with: None,
            },
            Some(cause),
        );
        self.ids.moved(key, Zone::Library);
    }
}

/// The keyword action of a transfer category.
fn move_action(category: &str) -> Option<MoveAction> {
    match category {
        "Draw" => Some(MoveAction::Draw),
        "Discard" => Some(MoveAction::Discard),
        "Sacrifice" => Some(MoveAction::Sacrifice),
        "Destroy" => Some(MoveAction::Destroy),
        "Mill" => Some(MoveAction::Mill),
        _ => None,
    }
}
