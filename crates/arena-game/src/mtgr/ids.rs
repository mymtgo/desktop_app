//! `.mtgr` instance ids (spec section 6.1): one id per tracked object (card,
//! token or ability), given in order of first appearance from `#1`,
//! following the fold's `Identity` across Arena's instance id changes. An id
//! ends when its library is shuffled or the object ceases; the object gets a
//! new id if it is seen again.
//!
//! Each id also keeps what the record has said about the object so far
//! (tapped, card types, power and toughness, counters), so a change Arena
//! reports is written from the value the record last gave.

use std::collections::BTreeMap;

use ::mtgr::{CardType, ObjRef, Zone};

use crate::CardKey;

/// What kind of object an id stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Card,
    Token,
    /// A copy of a card or spell (`new copy`).
    Copy,
    /// An ability; `on_stack` is false for a mana ability.
    Ability {
        on_stack: bool,
    },
}

/// What the writer has said about one live id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tracked {
    pub id: u32,
    pub kind: Kind,
    /// The object's name once written (`new card`, `new token` or
    /// `identify`); `None` while it is hidden, for a card named `?`
    /// (decision 0048) and for an ability. References give only the id
    /// then.
    pub name: Option<String>,
    /// A card introduced `new hidden` and not identified yet.
    pub hidden: bool,
    /// The zone the last line put it in (abilities: the stack).
    pub zone: Zone,
    pub owner_seat: u32,
    /// The controller the record last gave it: its owner until a
    /// `control` line or a keyframe's battlefield entry says otherwise
    /// (spec 6.5). A move does not change it, as for a reader.
    pub controller_seat: u32,
    /// The permanent state the record gave it on the battlefield: the face
    /// up when it is not the front, tapped, types, and power and toughness
    /// (`pt_unknown` when the record wrote `?`).
    pub face: Option<String>,
    pub tapped: bool,
    pub types: Vec<CardType>,
    pub pt: Option<(i32, i32)>,
    pub pt_unknown: bool,
    /// Counters by written name.
    pub counters: BTreeMap<String, u32>,
    /// An ability or spell whose resolution started (`resolve` written).
    pub resolving: bool,
    /// A revealed companion still outside the game.
    pub companion: bool,
    /// For a card in exile: the id of the object it is exiled with (spec
    /// 7.1), while the link holds (decision 0024).
    pub with: Option<u32>,
    /// For a spell cast as its Adventure: the seat of its controller.
    pub adventure_cast: Option<u32>,
    /// For a card in exile on an adventure: the seat who may cast it from
    /// there (CR 715.3d).
    pub adventure: Option<u32>,
}

/// The live ids by object.
#[derive(Debug, Default)]
pub(super) struct Ids {
    last: u32,
    live: BTreeMap<CardKey, Tracked>,
}

impl Ids {
    pub fn get(&self, key: CardKey) -> Option<&Tracked> {
        self.live.get(&key)
    }

    pub fn get_mut(&mut self, key: CardKey) -> Option<&mut Tracked> {
        self.live.get_mut(&key)
    }

    /// The reference lines use for a live object: its id and known name.
    pub fn obj(&self, key: CardKey) -> Option<ObjRef> {
        self.live.get(&key).map(|tracked| ObjRef {
            id: tracked.id,
            name: tracked.name.clone(),
        })
    }

    /// Give an object the next id (`hidden` for `new hidden`).
    pub fn introduce(
        &mut self,
        key: CardKey,
        kind: Kind,
        name: Option<String>,
        hidden: bool,
        zone: Zone,
        owner_seat: u32,
    ) -> u32 {
        self.last += 1;
        self.live.insert(
            key,
            Tracked {
                id: self.last,
                kind,
                name,
                hidden,
                zone,
                owner_seat,
                controller_seat: owner_seat,
                face: None,
                tapped: false,
                types: Vec::new(),
                pt: None,
                pt_unknown: false,
                counters: BTreeMap::new(),
                resolving: false,
                companion: false,
                with: None,
                adventure_cast: None,
                adventure: None,
            },
        );
        self.last
    }

    /// The next id, for an object no Arena instance stands for (a lasting
    /// effect).
    pub fn allocate(&mut self) -> u32 {
        self.last += 1;
        self.last
    }

    /// The object moved to `zone`: a new object as far as the rules go
    /// (CR 400.7), so its permanent state starts over.
    ///
    /// Its links end with it (decision 0024): the card it was exiled with
    /// and the cards exiled with it, except that cards exiled to pay a
    /// permanent spell's cost stay linked as the spell moves to the
    /// battlefield (CR 607.2q).
    pub fn moved(&mut self, key: CardKey, zone: Zone) {
        let Some(tracked) = self.live.get_mut(&key) else {
            return;
        };
        let id = tracked.id;
        let keeps_links = tracked.zone == Zone::Stack && zone == Zone::Battlefield;
        tracked.zone = zone;
        tracked.with = None;
        tracked.face = None;
        tracked.tapped = false;
        tracked.types.clear();
        tracked.pt = None;
        tracked.pt_unknown = false;
        tracked.counters.clear();
        tracked.resolving = false;
        tracked.companion = false;
        tracked.adventure_cast = None;
        tracked.adventure = None;
        if !keeps_links {
            self.unlink(id);
        }
    }

    /// End every link whose source is the object `id`.
    fn unlink(&mut self, id: u32) {
        for tracked in self.live.values_mut() {
            if tracked.with == Some(id) {
                tracked.with = None;
            }
        }
    }

    /// `card` is exiled with the object `source` (`with ID`).
    pub fn link(&mut self, card: CardKey, source: u32) {
        if let Some(tracked) = self.live.get_mut(&card) {
            tracked.with = Some(source);
        }
    }

    /// A hidden object is named (`None`: named `?`, decision 0048).
    pub fn identify(&mut self, key: CardKey, name: Option<String>) {
        if let Some(tracked) = self.live.get_mut(&key) {
            tracked.name = name;
            tracked.hidden = false;
        }
    }

    /// End an object's id (it ceased to exist).
    pub fn end(&mut self, key: CardKey) {
        if let Some(tracked) = self.live.remove(&key) {
            self.unlink(tracked.id);
        }
    }

    /// Every live object, by key.
    pub fn keys(&self) -> Vec<CardKey> {
        self.live.keys().copied().collect()
    }

    /// End the id of every card in `seat`'s library (a shuffle).
    pub fn end_library(&mut self, seat: u32) {
        self.live.retain(|_, tracked| {
            !(tracked.zone == Zone::Library
                && tracked.owner_seat == seat
                && tracked.kind == Kind::Card)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_with_link() -> (Ids, CardKey, CardKey) {
        let mut ids = Ids::default();
        let source = CardKey(1);
        let card = CardKey(2);
        ids.introduce(source, Kind::Card, None, false, Zone::Battlefield, 1);
        ids.introduce(card, Kind::Card, None, false, Zone::Exile, 2);
        ids.link(card, 1);
        (ids, source, card)
    }

    /// Decision 0024: the link ends when either object leaves its zone.
    #[test]
    fn a_link_ends_when_the_source_leaves() {
        let (mut ids, source, card) = ids_with_link();
        ids.moved(source, Zone::Hand);
        assert_eq!(ids.get(card).and_then(|tracked| tracked.with), None);
    }

    #[test]
    fn a_link_ends_when_the_card_leaves() {
        let (mut ids, _, card) = ids_with_link();
        ids.moved(card, Zone::Battlefield);
        assert_eq!(ids.get(card).and_then(|tracked| tracked.with), None);
    }

    #[test]
    fn a_link_ends_when_the_source_ceases() {
        let (mut ids, source, card) = ids_with_link();
        ids.end(source);
        assert_eq!(ids.get(card).and_then(|tracked| tracked.with), None);
    }

    /// CR 607.2q: cards exiled to pay a permanent spell's cost stay linked
    /// to the permanent it becomes.
    #[test]
    fn a_link_survives_a_permanent_spell_resolving() {
        let (mut ids, source, card) = ids_with_link();
        ids.moved(source, Zone::Stack);
        ids.link(card, 1);
        ids.moved(source, Zone::Battlefield);
        assert_eq!(ids.get(card).and_then(|tracked| tracked.with), Some(1));
    }
}
