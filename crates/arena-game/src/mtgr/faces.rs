//! Faces and halves (decision 0022): an object's name is always its card's
//! name, and `face` says which face or half is in use.
//!
//! Arena shows a card by the grpId of the face or half in use, and card
//! data has only the card's own grpId:
//!
//! - **Double-faced cards** (transforming and modal) send both faces'
//!   grpIds (`grpId` and `othersideGrpId`); the front is the smaller
//!   (`front_face`), and an object showing the other one shows its back
//!   face (the card's second face name).
//! - **Split cards and Rooms** show the card by the half being cast, with
//!   its halves as `GameObjectType_SplitLeft` and `GameObjectType_SplitRight`
//!   (Rooms: `GameObjectType_RoomLeft` and `GameObjectType_RoomRight`)
//!   objects whose `parentId` is the card. The card's own grpId comes from
//!   evidence: the same card (by `Identity`) shown whole in any state of
//!   the game, before or after (owner data: a Room on the battlefield, a
//!   split card in the graveyard). Without it, Arena's numbering (the card
//!   just before its left half, owner data: Claim // Fame 74013, halves
//!   74014 and 74015) is used only when the halves are numbered one after
//!   the other and the card data there has two faces. Otherwise the card
//!   is named by the grpId Arena showed, `?` when the card data does not
//!   list it (decision 0048). The source id (`src arena:N`, spec 6.4) is
//!   the grpId that names the card: the card's own grpId once it is found
//!   (Claim // Fame cast as Fame is `src arena:74013`), the half's grpId
//!   only when the card is not found and the half names it.
//!
//! - **Adventures** show the card with a `GameObjectType_Adventure` object
//!   whose `parentId` is the card, by the Adventure's grpId. Cast as its
//!   Adventure, the card itself shows that grpId too (the Bo1 fixture's
//!   Merfolk Secretkeeper, 70200, cast as Venture Deeper, 70481, from hand
//!   and back as the creature from exile). The card's own grpId comes from
//!   evidence only, as for a split card: the same card shown by another
//!   grpId in any state of the game. The Adventure is the card's second
//!   face (CR 715.2: "Bonecrusher Giant // Stomp").
//!
//! A face's name is the card data's `faces` entry (front first). A card
//! named `?` has no faces, so no `face` is written for it (decision 0048).
//! A face the record must name, of a card the card data names, with no name
//! there is missing card data: the record is refused (`face` has no `?`
//! form).

use std::collections::{BTreeMap, BTreeSet};

use crate::{CARD, CardKey, Fold, GameObject, GameState, front_face};

use super::GameWriter;

const LEFT_HALVES: [&str; 2] = ["GameObjectType_SplitLeft", "GameObjectType_RoomLeft"];
const RIGHT_HALVES: [&str; 2] = ["GameObjectType_SplitRight", "GameObjectType_RoomRight"];
const ADVENTURE: &str = "GameObjectType_Adventure";

/// The grpId of a card's Adventure, when `state` shows it.
fn adventure_of(state: &GameState, object: &GameObject) -> Option<u32> {
    state
        .objects
        .values()
        .find(|part| part.parent_id == object.instance_id && part.kind == ADVENTURE)
        .map(|part| part.grp_id)
}

/// The grpIds of a card's halves (left, right), when `state` shows them.
fn halves(state: &GameState, object: &GameObject) -> Option<(u32, Option<u32>)> {
    let mut left = None;
    let mut right = None;
    for part in state.objects.values() {
        if part.parent_id != object.instance_id {
            continue;
        }
        if LEFT_HALVES.contains(&part.kind.as_str()) {
            left = Some(part.grp_id);
        } else if RIGHT_HALVES.contains(&part.kind.as_str()) {
            right = Some(part.grp_id);
        }
    }
    left.map(|left| (left, right))
}

/// Every grpId each card is shown by, in any state of the game: the
/// evidence for a split card's or Room's own grpId.
pub(super) fn shown_grp_ids(fold: &Fold) -> BTreeMap<CardKey, BTreeSet<u32>> {
    let mut shown: BTreeMap<CardKey, BTreeSet<u32>> = BTreeMap::new();
    for state in &fold.states {
        for object in state.objects.values() {
            if object.kind == CARD && object.grp_id != 0 {
                shown
                    .entry(fold.identity.key(object.instance_id))
                    .or_default()
                    .insert(object.grp_id);
            }
        }
    }
    shown
}

impl GameWriter<'_> {
    /// The card an object shows, by the card's grpId, and the index of the
    /// face or half it shows (`None` for the front face or the whole card).
    pub(super) fn shown_card(
        &self,
        state: &GameState,
        object: &GameObject,
    ) -> (u32, Option<usize>) {
        if object.otherside_grp_id != 0 {
            let front = front_face(object.grp_id, object.otherside_grp_id);
            let face = (front != object.grp_id).then_some(1);
            return (front, face);
        }
        if let Some(adventure) = adventure_of(state, object) {
            if object.grp_id != adventure {
                return (object.grp_id, None);
            }
            return match self.evidenced_card(object.instance_id, adventure, None) {
                Some(card) => (card, Some(1)),
                None => (object.grp_id, None),
            };
        }
        let Some((left, right)) = halves(state, object) else {
            return (object.grp_id, None);
        };
        let face = if object.grp_id == left {
            0
        } else if Some(object.grp_id) == right {
            1
        } else {
            return (object.grp_id, None);
        };
        match self.split_card(object.instance_id, left, right) {
            Some(card) => (card, Some(face)),
            None => (object.grp_id, None),
        }
    }

    /// A split card's or Room's own grpId: shown whole somewhere in the game,
    /// else the checked numbering (see the module header).
    fn split_card(&self, instance_id: u32, left: u32, right: Option<u32>) -> Option<u32> {
        if let Some(card) = self.evidenced_card(instance_id, left, right) {
            return Some(card);
        }
        let card = left.checked_sub(1)?;
        let numbered = right == Some(left + 1)
            && self
                .cards
                .get(&card)
                .is_some_and(|data| data.faces.len() == 2);
        numbered.then_some(card)
    }

    /// The one grpId other than its parts' that the card is shown by in any
    /// state of the game, if there is exactly one.
    fn evidenced_card(&self, instance_id: u32, part: u32, other: Option<u32>) -> Option<u32> {
        let key = self.fold.identity.key(instance_id);
        let own: Vec<u32> = self
            .shown_grp_ids
            .get(&key)
            .into_iter()
            .flatten()
            .copied()
            .filter(|grp_id| *grp_id != part && Some(*grp_id) != other)
            .collect();
        match own.as_slice() {
            [card] => Some(*card),
            _ => None,
        }
    }

    /// Whether `object` shows a card cast as its Adventure (CR 715.3): the
    /// card shows its Adventure's grpId and is named by its card.
    pub(super) fn is_adventure(&self, state: &GameState, object: &GameObject) -> bool {
        adventure_of(state, object).is_some_and(|adventure| {
            object.grp_id == adventure && self.shown_card(state, object).1 == Some(1)
        })
    }

    /// The card grpId of the object with `instance_id` in state `index`, or
    /// `fallback` when the state does not show it.
    pub(super) fn card_grp_at(&self, index: usize, instance_id: u32, fallback: u32) -> u32 {
        let state = &self.fold.states[index];
        match state.objects.get(&instance_id) {
            Some(object) => self.shown_card(state, object).0,
            None => fallback,
        }
    }

    /// The name of the face or half `object` shows in `state`, when it is not
    /// the front face or the whole card.
    pub(super) fn shown_face(&mut self, state: &GameState, object: &GameObject) -> Option<String> {
        let (card, face) = self.shown_card(state, object);
        self.face_name(object, card, face?)
    }

    /// The face or half a card is cast as (decision 0022: always written
    /// on a cast of one face): a double-faced card cast as its front names
    /// its front face, which is the same as no face for a transforming card
    /// (spec 14.3).
    pub(super) fn cast_face(&mut self, state: &GameState, object: &GameObject) -> Option<String> {
        let (card, face) = self.shown_card(state, object);
        let face = match face {
            Some(face) => face,
            None if object.otherside_grp_id != 0 => 0,
            None => return None,
        };
        self.face_name(object, card, face)
    }

    /// Face `face` of `card`. A card the card data does not list is named
    /// `?` and has no faces (decision 0048). A card it lists without that
    /// face's name is recorded as missing: `face` has no `?` form (tokens
    /// are named without card data).
    fn face_name(&mut self, object: &GameObject, card: u32, face: usize) -> Option<String> {
        if object.kind != CARD {
            return None;
        }
        let data = self.cards.get(&card)?;
        let name = data.faces.get(face).cloned();
        if name.is_none() {
            self.missing.insert(card);
        }
        name
    }
}

/// `GainDesignation`.
pub(super) const GAIN_DESIGNATION: &str = "AnnotationType_GainDesignation";
/// `GainDesignation` `DesignationType`s: it became day, it became night,
/// and a Room's left or right door was unlocked (owner data: 19 as a Room
/// cast as its left door resolves, 20 with the special action that unlocks
/// its right door).
const DAY: i64 = 10;
const NIGHT: i64 = 11;
const DOORS_UNLOCKED: [i64; 2] = [19, 20];

impl GameWriter<'_> {
    /// `GainDesignation`: day and night, then the permanents they turn
    /// over; a Room's door unlocked, which the standard does not cover yet
    /// (spec section 15: Rooms), as `incomplete`.
    pub(super) fn write_designation(&mut self, index: usize, annotation: &crate::Annotation) {
        let designation = annotation.int("DesignationType").unwrap_or(0);
        if designation == DAY || designation == NIGHT {
            let night = designation == NIGHT;
            if self.day_night == Some(night) {
                return;
            }
            let cause = self.day_night_cause(index);
            self.event(::mtgr::Statement::DayNight { night }, Some(cause));
            self.day_night = Some(night);
            self.write_transforms(index, true);
        } else if DOORS_UNLOCKED.contains(&designation) {
            for instance_id in &annotation.affected_ids {
                let key = self.fold.identity.key(*instance_id);
                // A Room the record does not track has no id to name.
                let reason = match self.ids.get(key) {
                    Some(tracked) => format!(
                        "a door of the Room #{} was unlocked; the standard does not cover Rooms' doors yet",
                        tracked.id
                    ),
                    None => "a door of a Room was unlocked; the standard does not cover Rooms' doors yet"
                        .to_string(),
                };
                self.write_incomplete(reason);
            }
        }
    }

    /// Why it became day or night (CR 731), as Arena names no cause:
    /// - in the untap step, the turn-based rule (`rule:day-night`, CR 731.2);
    /// - while something resolves, that object (Tovolar's "it becomes
    ///   night"; Collected Company putting daybound creatures onto the
    ///   battlefield);
    /// - from neither, a daybound permanent entering (CR 702.145d): the one
    ///   double-faced permanent that entered in this message, when its faces
    ///   follow day and night later in the game;
    /// - otherwise unknown.
    fn day_night_cause(&self, index: usize) -> ::mtgr::Cause {
        if self.last_step == Some((1, 1)) {
            return ::mtgr::Cause::Rule("day-night".into());
        }
        if let Some(tracked) = self.resolving_now.last().and_then(|key| self.ids.get(*key)) {
            return ::mtgr::Cause::Object(tracked.id);
        }
        if self.day_night.is_some() {
            return ::mtgr::Cause::Unknown;
        }
        let entered: Vec<&crate::Transfer> = self.fold.transfers[..self.next_transfer]
            .iter()
            .filter(|transfer| {
                transfer.state_index == index
                    && transfer.to_zone == "ZoneType_Battlefield"
                    && transfer.other_face_grp_id != 0
            })
            .collect();
        let [transfer] = entered.as_slice() else {
            return ::mtgr::Cause::Unknown;
        };
        match self.ids.get(transfer.card) {
            Some(tracked) if self.follows_day_night(transfer.card, index) => {
                ::mtgr::Cause::Object(tracked.id)
            }
            _ => ::mtgr::Cause::Unknown,
        }
    }

    /// Whether a permanent shows its back face exactly when it is night,
    /// each time day or night changes after state `index` while it is on
    /// the battlefield (and it changes at least once): a daybound card.
    fn follows_day_night(&self, key: CardKey, index: usize) -> bool {
        let fold = self.fold;
        let mut agreed = 0;
        for at in index + 1..fold.states.len() {
            let mut night = None;
            for annotation in &fold.steps[at].annotations {
                if annotation.is(GAIN_DESIGNATION) {
                    match annotation.int("DesignationType") {
                        Some(DAY) => night = Some(false),
                        Some(NIGHT) => night = Some(true),
                        _ => {}
                    }
                }
            }
            let Some(night) = night else {
                continue;
            };
            let state = &fold.states[at];
            let shown = state.objects.values().find(|object| {
                fold.identity.key(object.instance_id) == key
                    && state.zone_kind(object.zone_id) == "ZoneType_Battlefield"
            });
            let Some(object) = shown else {
                break;
            };
            let back = self.shown_card(state, object).1 == Some(1);
            if back != night {
                return false;
            }
            agreed += 1;
        }
        agreed > 0
    }
}
