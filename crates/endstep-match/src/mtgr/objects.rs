//! The objects of one game as the record's lines leave them: what a reader
//! replaying the record holds (standard section 14). The builder's own ids
//! count from 1 in order of first appearance; a Forge card id is only a key
//! here and is never written (it restarts each game, and Forge reissues the
//! ids of hidden cards).
//!
//! Every change goes through the methods below, which follow the reader's
//! rules: a move clears what a permanent had, a library keeps its size and
//! the positions known in it, the stack keeps its order, and an ended id is
//! never used again.
//!
//! An ability on the stack is an object too: it takes the next id, has no
//! name, and sits in the stack zone until it ceases.

use std::collections::BTreeMap;

use ::mtgr::{CardType, LibraryEnd, ObjRef, Position, Zone};

/// One object as a reader replaying the record holds it. This is the record
/// the state diff compares with what a full state shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tracked {
    pub id: u32,
    /// `None` for a hidden card.
    pub name: Option<String>,
    pub owner: u32,
    pub controller: u32,
    pub zone: Zone,
    pub token: bool,
    pub tapped: bool,
    pub types: Vec<CardType>,
    pub pt: Option<(i32, i32)>,
    pub counters: BTreeMap<String, u32>,
    /// The id of the object it is attached to.
    pub attached_to: Option<u32>,
    /// Its place in its owner's library, when the record's lines fix it.
    pub position: Option<Position>,
    /// Set for an ability on the stack.
    pub ability: Option<Ability>,
}

/// What the record knows of an ability on the stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ability {
    /// The Forge id of its source card: a later event names the ability by
    /// it.
    pub source: u64,
    /// Forge's whole text for it, when a stack entry gave one: a later
    /// event repeats it.
    pub text: Option<String>,
}

impl Tracked {
    pub(super) fn obj(&self) -> ObjRef {
        ObjRef {
            id: self.id,
            name: self.name.clone(),
        }
    }

    fn is_hidden_in_hand(&self, seat: u32) -> bool {
        self.name.is_none() && self.owner == seat && self.zone == Zone::Hand
    }
}

/// What a permanent has on the battlefield, set as it enters and compared
/// while it stays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Permanent {
    pub tapped: bool,
    pub types: Vec<CardType>,
    pub pt: Option<(i32, i32)>,
}

/// The live objects of one game, by id.
#[derive(Debug, Default)]
pub(super) struct Objects {
    live: BTreeMap<u32, Tracked>,
    by_forge: BTreeMap<u64, u32>,
    last_id: u32,
    /// Each seat's library size as a reader counts it: set by a keyframe,
    /// one less for each move out of the library, one more for each move in.
    libraries: BTreeMap<u32, i64>,
    /// The ids on the stack, the top first, as a reader keeps them.
    stack: Vec<u32>,
}

impl Objects {
    /// The id of the object a Forge card id stands for.
    pub(super) fn id_of(&self, forge_id: u64) -> Option<u32> {
        self.by_forge.get(&forge_id).copied()
    }

    pub(super) fn get(&self, id: u32) -> Option<&Tracked> {
        self.live.get(&id)
    }

    pub(super) fn get_mut(&mut self, id: u32) -> Option<&mut Tracked> {
        self.live.get_mut(&id)
    }

    /// How many objects are live.
    pub(super) fn count(&self) -> usize {
        self.live.len()
    }

    /// The live objects in id order.
    pub(super) fn all(&self) -> impl Iterator<Item = &Tracked> {
        self.live.values()
    }

    /// The ids of the live objects that pass `keep`, in id order.
    pub(super) fn ids_where(&self, keep: impl Fn(&Tracked) -> bool) -> Vec<u32> {
        self.live
            .values()
            .filter(|object| keep(object))
            .map(|object| object.id)
            .collect()
    }

    pub(super) fn library(&self, seat: u32) -> i64 {
        self.libraries.get(&seat).copied().unwrap_or(0)
    }

    /// A keyframe gives the library size.
    pub(super) fn set_library(&mut self, seat: u32, size: i64) {
        self.libraries.insert(seat, size);
    }

    /// The ids on the stack, the top first.
    pub(super) fn stack(&self) -> &[u32] {
        &self.stack
    }

    pub(super) fn top_of_stack(&self) -> Option<u32> {
        self.stack.first().copied()
    }

    fn push_on_stack(&mut self, id: u32) {
        self.stack.retain(|other| *other != id);
        self.stack.insert(0, id);
    }

    /// A new ability on top of the stack, with the next id.
    pub(super) fn add_ability(&mut self, ability: Ability, controller: u32) -> u32 {
        let id = self.add(None, None, controller, Zone::Stack, false);
        if let Some(object) = self.live.get_mut(&id) {
            object.ability = Some(ability);
        }
        id
    }

    /// A new object with the next id, in `zone`. Introducing an object in a
    /// library takes it from the cards there the record does not track, so
    /// the size stays.
    pub(super) fn add(
        &mut self,
        forge_id: Option<u64>,
        name: Option<String>,
        owner: u32,
        zone: Zone,
        token: bool,
    ) -> u32 {
        self.last_id = self.last_id.saturating_add(1);
        let id = self.last_id;
        if let Some(forge_id) = forge_id {
            self.by_forge.insert(forge_id, id);
        }
        self.live.insert(
            id,
            Tracked {
                id,
                name,
                owner,
                controller: owner,
                zone,
                token,
                tapped: false,
                types: Vec::new(),
                pt: None,
                counters: BTreeMap::new(),
                attached_to: None,
                position: None,
                ability: None,
            },
        );
        if zone == Zone::Stack {
            self.push_on_stack(id);
        }
        id
    }

    /// An object is named, and now stands for the Forge card `forge_id`
    /// alone: an id it stood for before is dropped (Forge reissues ids, so
    /// an old one may come back on another card).
    pub(super) fn identify(&mut self, id: u32, forge_id: u64, name: &str) {
        if let Some(object) = self.live.get_mut(&id) {
            object.name = Some(name.to_string());
            self.by_forge.retain(|_, object| *object != id);
            self.by_forge.insert(forge_id, id);
        }
    }

    /// The Forge id no longer stands for its object.
    pub(super) fn forget(&mut self, forge_id: u64) {
        self.by_forge.remove(&forge_id);
    }

    /// The lowest object named `name` in a seat's hand.
    pub(super) fn named_in_hand(&self, seat: u32, name: &str) -> Option<u32> {
        self.live
            .values()
            .find(|object| {
                object.owner == seat
                    && object.zone == Zone::Hand
                    && object.name.as_deref() == Some(name)
            })
            .map(|object| object.id)
    }

    /// The lowest hidden object in a seat's hand.
    pub(super) fn lowest_hidden_in_hand(&self, seat: u32) -> Option<u32> {
        self.live
            .values()
            .find(|object| object.is_hidden_in_hand(seat))
            .map(|object| object.id)
    }

    /// The lowest object, hidden or named, in a seat's hand.
    pub(super) fn lowest_in_hand(&self, seat: u32) -> Option<u32> {
        self.live
            .values()
            .find(|object| object.owner == seat && object.zone == Zone::Hand)
            .map(|object| object.id)
    }

    /// How many objects, hidden or named, a seat has in hand.
    pub(super) fn hand_count(&self, seat: u32) -> u32 {
        let count = self
            .live
            .values()
            .filter(|object| object.owner == seat && object.zone == Zone::Hand)
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Moves an object as a reader does: what it had as a permanent is
    /// cleared, its controller stays, a library it leaves or enters changes
    /// size, and it goes on top of a stack it enters. `draw` says the move
    /// takes the top card of the library (a draw or a mill); `bottom` puts
    /// the object on the bottom of the library it enters.
    pub(super) fn relocate(&mut self, id: u32, to: Zone, draw: bool, bottom: bool) {
        let Some(object) = self.live.get(&id) else {
            return;
        };
        let owner = object.owner;
        if object.zone == Zone::Library {
            let position = object.position;
            self.leave_library(owner, id, position, draw);
        }
        self.stack.retain(|other| *other != id);
        if to == Zone::Stack {
            self.push_on_stack(id);
        }
        if let Some(object) = self.live.get_mut(&id) {
            object.zone = to;
            object.tapped = false;
            object.types = Vec::new();
            object.pt = None;
            object.counters = BTreeMap::new();
            object.attached_to = None;
            object.position = None;
        }
        if to == Zone::Library {
            self.enter_library(owner, id, bottom);
        }
    }

    /// What a permanent has as it enters the battlefield.
    pub(super) fn enter(&mut self, id: u32, permanent: &Permanent) {
        if let Some(object) = self.live.get_mut(&id) {
            object.tapped = permanent.tapped;
            object.types = permanent.types.clone();
            object.pt = permanent.pt;
        }
    }

    /// The id ends: the object is no longer tracked, and the Forge card it
    /// stood for is new if it shows again.
    pub(super) fn end(&mut self, id: u32) {
        self.live.remove(&id);
        self.stack.retain(|other| *other != id);
        self.by_forge.retain(|_, object| *object != id);
    }

    /// A shuffle ends the id of every object in the seat's library.
    pub(super) fn shuffle(&mut self, seat: u32) {
        let ids = self.ids_where(|object| object.owner == seat && object.zone == Zone::Library);
        for id in ids {
            self.end(id);
        }
    }

    /// The other tracked objects in a seat's library.
    fn library_mates(&self, owner: u32, except: u32) -> Vec<u32> {
        self.ids_where(|object| {
            object.owner == owner && object.zone == Zone::Library && object.id != except
        })
    }

    /// Standard 14.2: positions beyond the leaver move one closer; a leaver
    /// whose place is unknown makes every position unknown. A drawn card
    /// whose place is unknown was the top card.
    fn leave_library(&mut self, owner: u32, id: u32, position: Option<Position>, draw: bool) {
        let top = Position {
            end: LibraryEnd::Top,
            n: 1,
        };
        let left = position.or(draw.then_some(top));
        for mate in self.library_mates(owner, id) {
            let Some(object) = self.live.get_mut(&mate) else {
                continue;
            };
            match (left, object.position) {
                (None, _) => object.position = None,
                (Some(left), Some(at)) if at.end == left.end && at.n > left.n => {
                    object.position = Some(Position {
                        end: at.end,
                        n: at.n - 1,
                    });
                }
                _ => {}
            }
        }
        let size = self.library(owner);
        self.libraries.insert(owner, size - 1);
    }

    /// Standard 14.2: an object put on the bottom pushes the positions
    /// counted from the bottom one further; an object put at an unknown
    /// place makes every position unknown.
    fn enter_library(&mut self, owner: u32, id: u32, bottom: bool) {
        for mate in self.library_mates(owner, id) {
            let Some(object) = self.live.get_mut(&mate) else {
                continue;
            };
            match object.position {
                _ if !bottom => object.position = None,
                Some(at) if at.end == LibraryEnd::Bottom => {
                    object.position = Some(Position {
                        end: at.end,
                        n: at.n.saturating_add(1),
                    });
                }
                _ => {}
            }
        }
        if bottom && let Some(object) = self.live.get_mut(&id) {
            object.position = Some(Position {
                end: LibraryEnd::Bottom,
                n: 1,
            });
        }
        let size = self.library(owner);
        self.libraries.insert(owner, size + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bottom(n: u32) -> Option<Position> {
        Some(Position {
            end: LibraryEnd::Bottom,
            n,
        })
    }

    fn hand_card(objects: &mut Objects, forge_id: u64, name: &str) -> u32 {
        objects.add(Some(forge_id), Some(name.to_string()), 0, Zone::Hand, false)
    }

    #[test]
    fn ids_count_from_one_and_forge_ids_are_only_keys() {
        let mut objects = Objects::default();
        let first = hand_card(&mut objects, 900, "Forest");
        let hidden = objects.add(None, None, 1, Zone::Hand, false);
        assert_eq!((first, hidden), (1, 2));
        assert_eq!(objects.id_of(900), Some(1));
        assert_eq!(objects.lowest_hidden_in_hand(1), Some(2));
        assert_eq!(objects.lowest_hidden_in_hand(0), None);

        objects.identify(hidden, 77, "Island");
        assert_eq!(objects.id_of(77), Some(2));
        assert_eq!(objects.lowest_hidden_in_hand(1), None);
        assert_eq!(objects.hand_count(1), 1);
    }

    #[test]
    fn a_move_clears_the_permanent_and_counts_the_library() {
        let mut objects = Objects::default();
        objects.set_library(0, 53);
        let id = hand_card(&mut objects, 1, "Forest");
        objects.relocate(id, Zone::Battlefield, false, false);
        objects.enter(
            id,
            &Permanent {
                tapped: true,
                types: vec![CardType::Land],
                pt: None,
            },
        );
        assert!(objects.get(id).is_some_and(|object| object.tapped));

        objects.relocate(id, Zone::Library, false, false);
        let object = objects.get(id).expect("the object");
        assert!(!object.tapped && object.types.is_empty());
        assert_eq!(objects.library(0), 54);

        objects.relocate(id, Zone::Hand, true, false);
        assert_eq!(objects.library(0), 53);
    }

    #[test]
    fn bottom_positions_follow_the_reader() {
        let mut objects = Objects::default();
        let first = hand_card(&mut objects, 1, "Forest");
        let second = hand_card(&mut objects, 2, "Island");
        let third = hand_card(&mut objects, 3, "Swamp");
        objects.relocate(first, Zone::Library, false, true);
        objects.relocate(second, Zone::Library, false, true);
        assert_eq!(objects.get(first).map(|o| o.position), Some(bottom(2)));
        assert_eq!(objects.get(second).map(|o| o.position), Some(bottom(1)));

        // A draw takes the top card: the bottom keeps its places.
        let drawn = objects.add(Some(4), Some("Plains".to_string()), 0, Zone::Library, false);
        objects.relocate(drawn, Zone::Hand, true, false);
        assert_eq!(objects.get(first).map(|o| o.position), Some(bottom(2)));

        // The bottom card leaves: the one above it is now the bottom.
        objects.relocate(second, Zone::Graveyard, false, false);
        assert_eq!(objects.get(first).map(|o| o.position), Some(bottom(1)));

        // A card put somewhere unknown makes every place unknown.
        objects.relocate(third, Zone::Library, false, false);
        assert_eq!(objects.get(first).map(|o| o.position), Some(None));
    }

    #[test]
    fn the_stack_keeps_the_last_object_on_top() {
        let mut objects = Objects::default();
        let spell = hand_card(&mut objects, 1, "Bolt");
        objects.relocate(spell, Zone::Stack, false, false);
        let ability = objects.add_ability(
            Ability {
                source: 1,
                text: None,
            },
            0,
        );
        assert_eq!((spell, ability), (1, 2));
        assert_eq!(objects.stack().to_vec(), vec![ability, spell]);
        assert_eq!(objects.top_of_stack(), Some(ability));
        // An ability is not a hidden card.
        assert_eq!(objects.lowest_hidden_in_hand(0), None);

        objects.end(ability);
        assert_eq!(objects.top_of_stack(), Some(spell));
        objects.relocate(spell, Zone::Graveyard, false, false);
        assert_eq!(objects.top_of_stack(), None);
    }

    #[test]
    fn a_shuffle_ends_the_library_ids() {
        let mut objects = Objects::default();
        let id = hand_card(&mut objects, 1, "Forest");
        let kept = hand_card(&mut objects, 2, "Island");
        objects.relocate(id, Zone::Library, false, false);
        objects.shuffle(0);
        assert!(objects.get(id).is_none());
        assert_eq!(objects.id_of(1), None);
        assert!(objects.get(kept).is_some());
        // The card shows again as a new object with the next id.
        assert_eq!(hand_card(&mut objects, 1, "Forest"), 3);
    }
}
