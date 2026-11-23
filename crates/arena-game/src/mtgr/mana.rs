//! Mana (spec section 9.4, decisions 0006 and 0042).
//!
//! Arena reports every mana a player spends as a `ManaPaid` annotation: the
//! mana's type (`color`), its id, the permanent whose ability made it (the
//! affector) and what it paid for (the affected object). Mana that is not
//! spent at once floats in the player's `manaPool` with the same id until it
//! is spent or the pool empties.
//!
//! - **Adding.** Mana paid at once is added by the open mana ability of the
//!   permanent that made it, as it is paid. Mana that floats is added when a
//!   state first shows it in the pool, by the ability that made it: an open
//!   mana ability of its source, or the spell or ability resolving (a
//!   triggered ability that adds mana is not a mana ability, CR 605.5a).
//!   Mana from an ability Arena names but the record does not track is
//!   added `by ?`.
//! - **Restrictions.** Arena reports no spending restrictions (CR 106.6).
//!   Mana from the basic land types' intrinsic abilities (CR 305.6) and
//!   from `{T}: Add {C}` has none; any other mana is written
//!   `restriction ?` (decision 0042).
//! - **Paying.** The mana a spell, ability or special action is paid with
//!   is one `pay` line by its cost, after its mana abilities. Arena pays a
//!   combat tax (CR 508.1h, 509.1d) toward a seat: an attack tax is paid by
//!   the cost of the permanent whose `Qualification` (type 59) is on the
//!   defending player; a block tax names no permanent and is `by ?`.
//! - **Losing.** Mana left in a pool is lost as each step and phase ends
//!   (CR 106.4), before the next step or turn line. Mana Arena's pool stops
//!   showing without a payment is lost `by ?`.
//! - **Not mana.** A `ManaPaid` with a `substitution_grpid` is a card or
//!   permanent paying for mana (delve, convoke, improvise): those are moves
//!   and taps, not mana (decision 0034).

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{Cause, Mana, Statement};

use super::GameWriter;
use super::abilities::{ABILITY_CREATED, ABILITY_DELETED, USER_ACTION};
use crate::{Annotation, CardKey, Fold};

pub(super) const MANA_PAID: &str = "AnnotationType_ManaPaid";
const QUALIFICATION: &str = "AnnotationType_Qualification";
/// The `QualificationType` of an attack tax on a player: Archangel of
/// Tithes keeps one on its controller while it is untapped
/// (`bo3-three-games` line 13682, ability grpId 102181).
const ATTACK_TAX: i64 = 59;

/// Annotations that come between a payment's mana abilities and its
/// `ManaPaid`s: a payment stays open across them.
const PAYMENT_PARTS: [&str; 7] = [
    ABILITY_CREATED,
    ABILITY_DELETED,
    USER_ACTION,
    MANA_PAID,
    "AnnotationType_TappedUntappedPermanent",
    "AnnotationType_ObjectIdChanged",
    "AnnotationType_ChoiceResult",
];

/// Mana abilities with no spending restriction: `{T}: Add {W}` to
/// `{T}: Add {G}` (1001 to 1005), the intrinsic abilities of the basic land
/// types (CR 305.6), and `{T}: Add {C}` (1152). In the fixtures and the
/// owner's matches, every Plains, Island, Swamp, Mountain and Forest taps
/// with the first five, and each of the six adds only its one type.
const UNRESTRICTED_MANA_ABILITIES: [u32; 6] = [1001, 1002, 1003, 1004, 1005, 1152];

/// A `ManaPaid` `color` as a type of mana: 1 to 5 are white to green, 12 is
/// colorless (the fixtures' `{T}: Add {C}` ability, 1152).
fn paid_mana(color: i64) -> Option<Mana> {
    match color {
        1 => Some(Mana::W),
        2 => Some(Mana::U),
        3 => Some(Mana::B),
        4 => Some(Mana::R),
        5 => Some(Mana::G),
        12 => Some(Mana::C),
        _ => None,
    }
}

/// A `manaPool` entry's `ManaColor_*` as a type of mana.
fn pool_mana(color: &str) -> Option<Mana> {
    match color {
        "ManaColor_White" => Some(Mana::W),
        "ManaColor_Blue" => Some(Mana::U),
        "ManaColor_Black" => Some(Mana::B),
        "ManaColor_Red" => Some(Mana::R),
        "ManaColor_Green" => Some(Mana::G),
        "ManaColor_Colorless" => Some(Mana::C),
        _ => None,
    }
}

/// Whether a payment stays open across this annotation.
pub(super) fn is_payment_part(annotation: &Annotation) -> bool {
    PAYMENT_PARTS.iter().any(|kind| annotation.is(kind))
}

/// Each instance's ability grpId, from the `UserActionTaken` that names it.
pub(super) fn ability_grp_ids(fold: &Fold) -> BTreeMap<u32, u32> {
    let mut grp_ids = BTreeMap::new();
    for step in &fold.steps {
        for annotation in &step.annotations {
            if !annotation.is(USER_ACTION) {
                continue;
            }
            let grp_id = annotation
                .int("abilityGrpId")
                .and_then(|id| u32::try_from(id).ok())
                .unwrap_or(0);
            if grp_id == 0 {
                continue;
            }
            for id in &annotation.affected_ids {
                grp_ids.insert(*id, grp_id);
            }
        }
    }
    grp_ids
}

/// Triggered mana abilities (CR 605.1b): ability instances no action
/// activated whose source pays with mana while they are open (Nissa, Who
/// Shakes the World's extra {G}). Arena never shows them on the stack.
pub(super) fn triggered_mana_abilities(fold: &Fold, actions: &BTreeMap<u32, i64>) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    let mut open: BTreeMap<u32, u32> = BTreeMap::new();
    for step in &fold.steps {
        for annotation in &step.annotations {
            if annotation.is(ABILITY_CREATED) {
                for id in &annotation.affected_ids {
                    open.insert(*id, annotation.affector_id);
                }
            } else if annotation.is(ABILITY_DELETED) {
                for id in &annotation.affected_ids {
                    open.remove(id);
                }
            } else if annotation.is(MANA_PAID)
                && !annotation.details.contains_key("substitution_grpid")
            {
                for (id, source) in &open {
                    if *source == annotation.affector_id && !actions.contains_key(id) {
                        found.insert(*id);
                    }
                }
            }
        }
    }
    found
}

/// Mana the record added that Arena still shows floating, by Arena mana id.
#[derive(Debug, Clone)]
struct Floating {
    seat: u32,
    mana: Mana,
    count: u32,
}

/// The mana being paid for one spell, ability or special action.
#[derive(Debug, Clone)]
struct Payment {
    seat: u32,
    target: Option<CardKey>,
    mana: Vec<Mana>,
}

/// A mana ability the record wrote whose id has not ended.
#[derive(Debug, Clone)]
pub(super) struct OpenManaAbility {
    pub key: CardKey,
    pub source: CardKey,
    pub id: u32,
    pub seat: u32,
    /// The ability's grpId, when Arena names it.
    pub grp_id: u32,
}

/// The record's mana: each seat's pool as the lines leave it, the floating
/// mana Arena tracks by id, and the payment being written.
#[derive(Debug, Default)]
pub(super) struct ManaBook {
    pool: BTreeMap<u32, Vec<Mana>>,
    floating: BTreeMap<u32, Floating>,
    payment: Option<Payment>,
    pub open_abilities: Vec<OpenManaAbility>,
}

impl GameWriter<'_> {
    /// `ManaPaid`: the mana is added (unless it floats already) and joins
    /// the payment for the affected object.
    pub(super) fn write_mana_paid(&mut self, index: usize, annotation: &Annotation) {
        if annotation.details.contains_key("substitution_grpid") {
            return;
        }
        let Some(mana) = annotation.int("color").and_then(paid_mana) else {
            self.write_incomplete("Arena paid mana of a type the record cannot tell".to_string());
            return;
        };
        let mana_id = annotation.int("id").and_then(|id| u32::try_from(id).ok());
        let target = annotation.affected_ids.first().and_then(|id| {
            if self.fold.states[index].players.contains_key(id) {
                return self.attack_tax_source(index);
            }
            let key = self.fold.identity.key(*id);
            Some(self.paid_for.get(&key).copied().unwrap_or(key))
        });
        let floating = mana_id.and_then(|id| self.mana.floating.get_mut(&id));
        let seat = match floating {
            Some(floating) => {
                let seat = floating.seat;
                floating.count = floating.count.saturating_sub(1);
                if floating.count == 0
                    && let Some(id) = mana_id
                {
                    self.mana.floating.remove(&id);
                }
                seat
            }
            None => self.add_paid_mana(index, annotation.affector_id, mana),
        };
        self.pay_toward(seat, target, mana);
    }

    /// The permanent whose attack tax a payment toward a seat pays (CR
    /// 508.1h): in the declare attackers step, the one source of an attack
    /// tax `Qualification` in force on the defending player. Arena pays a
    /// combat tax toward a seat, never an object, and names a block tax's
    /// source nowhere, so any other payment toward a seat is `by ?`.
    fn attack_tax_source(&self, index: usize) -> Option<CardKey> {
        if self.last_step != Some((3, 5)) {
            return None;
        }
        let state = &self.fold.states[index];
        let defending = super::other_seat(state.active_seat?);
        let sources: BTreeSet<u32> = state
            .persistent
            .values()
            .filter(|annotation| {
                annotation.is(QUALIFICATION)
                    && annotation.int("QualificationType") == Some(ATTACK_TAX)
                    && annotation.affected_ids.contains(&defending)
            })
            .map(|annotation| annotation.affector_id)
            .collect();
        if sources.len() != 1 {
            return None;
        }
        let source = sources.first()?;
        Some(self.fold.identity.key(*source))
    }

    /// Mana a `ManaPaid` spends that did not float: added by the open mana
    /// ability of the permanent that made it. Returns the paying seat.
    fn add_paid_mana(&mut self, index: usize, source_id: u32, mana: Mana) -> u32 {
        let source = self.fold.identity.key(source_id);
        let producer = self
            .mana
            .open_abilities
            .iter()
            .rev()
            .find(|ability| ability.source == source)
            .cloned();
        let (seat, cause, restricted) = match producer {
            Some(ability) => (
                ability.seat,
                Cause::Object(ability.id),
                !UNRESTRICTED_MANA_ABILITIES.contains(&ability.grp_id),
            ),
            None => {
                let seat = self.fold.states[index]
                    .objects
                    .get(&source_id)
                    .map(|object| object.controller_seat)
                    .filter(|seat| *seat != 0)
                    .unwrap_or(self.roster.local_seat);
                (seat, Cause::Unknown, true)
            }
        };
        self.write_add(seat, vec![mana], cause, restricted);
        seat
    }

    fn write_add(&mut self, seat: u32, mana: Vec<Mana>, cause: Cause, restricted: bool) {
        self.mana
            .pool
            .entry(seat)
            .or_default()
            .extend(mana.iter().copied());
        let player = self.roster.name(seat);
        self.event(
            Statement::Add {
                player,
                mana,
                unknown_restriction: restricted,
            },
            Some(cause),
        );
    }

    /// Add `mana` to the payment for `target`, writing any other payment
    /// first.
    fn pay_toward(&mut self, seat: u32, target: Option<CardKey>, mana: Mana) {
        let other = self
            .mana
            .payment
            .as_ref()
            .is_some_and(|payment| payment.seat != seat || payment.target != target);
        if other {
            self.flush_payment();
        }
        self.mana
            .payment
            .get_or_insert(Payment {
                seat,
                target,
                mana: Vec::new(),
            })
            .mana
            .push(mana);
    }

    /// Write the payment being made: one `pay` line by the cost of what it
    /// paid for (`by ?` when the record does not track it).
    pub(super) fn flush_payment(&mut self) {
        let Some(payment) = self.mana.payment.take() else {
            return;
        };
        let pool = self.mana.pool.entry(payment.seat).or_default();
        for symbol in &payment.mana {
            if let Some(at) = pool.iter().position(|held| held == symbol) {
                pool.remove(at);
            }
        }
        let cause = payment
            .target
            .and_then(|key| self.ids.get(key))
            .map(|tracked| Cause::Cost(tracked.id))
            .unwrap_or(Cause::Unknown);
        let player = self.roster.name(payment.seat);
        self.event(
            Statement::Pay {
                player,
                mana: payment.mana,
            },
            Some(cause),
        );
    }

    /// The ability that made mana from `source`: its last open mana
    /// ability, or the object resolving now when it is the source or an
    /// ability of it.
    fn mana_producer(&self, source: CardKey) -> Option<(CardKey, u32, u32)> {
        if let Some(ability) = self
            .mana
            .open_abilities
            .iter()
            .rev()
            .find(|ability| ability.source == source)
        {
            return Some((ability.key, ability.id, ability.grp_id));
        }
        let resolver = *self.resolving_now.last()?;
        let made_it = resolver == source || self.ability_sources.get(&resolver) == Some(&source);
        if !made_it {
            return None;
        }
        self.ids
            .get(resolver)
            .map(|tracked| (resolver, tracked.id, 0))
    }

    /// Mana state `index` shows floating that the record has not added:
    /// added by the ability that made it. With `only`, just the mana that
    /// ability made (written before its id ends).
    pub(super) fn sync_pool(&mut self, index: usize, only: Option<CardKey>) {
        let fold = self.fold;
        for (seat, entries) in &fold.states[index].mana_pool {
            for entry in entries {
                let known = self
                    .mana
                    .floating
                    .get(&entry.mana_id)
                    .map(|floating| floating.count)
                    .unwrap_or(0);
                if entry.count <= known {
                    continue;
                }
                let source = fold.identity.key(entry.src_instance_id);
                let producer = self.mana_producer(source);
                if only.is_some() && producer.map(|(key, _, _)| key) != only {
                    continue;
                }
                let Some(mana) = pool_mana(&entry.color) else {
                    self.write_incomplete(
                        "Arena added mana of a type the record cannot tell".to_string(),
                    );
                    continue;
                };
                let extra = entry.count - known;
                let grp_id = if entry.ability_grp_id != 0 {
                    entry.ability_grp_id
                } else {
                    producer.map(|(_, _, grp_id)| grp_id).unwrap_or(0)
                };
                let cause = producer
                    .map(|(_, id, _)| Cause::Object(id))
                    .unwrap_or(Cause::Unknown);
                let restricted = !UNRESTRICTED_MANA_ABILITIES.contains(&grp_id);
                self.write_add(*seat, vec![mana; extra as usize], cause, restricted);
                self.mana.floating.insert(
                    entry.mana_id,
                    Floating {
                        seat: *seat,
                        mana,
                        count: entry.count,
                    },
                );
            }
        }
    }

    /// Floating mana that state `index` no longer shows and no payment
    /// spent: lost, for a reason Arena does not give.
    pub(super) fn lose_vanished_mana(&mut self, index: usize) {
        let pools = &self.fold.states[index].mana_pool;
        let mut lost: BTreeMap<u32, Vec<Mana>> = BTreeMap::new();
        let mut gone: Vec<u32> = Vec::new();
        for (mana_id, floating) in &mut self.mana.floating {
            let shown = pools
                .get(&floating.seat)
                .and_then(|entries| entries.iter().find(|entry| entry.mana_id == *mana_id))
                .map(|entry| entry.count)
                .unwrap_or(0);
            if shown >= floating.count {
                continue;
            }
            let missing = floating.count - shown;
            lost.entry(floating.seat)
                .or_default()
                .extend(std::iter::repeat_n(floating.mana, missing as usize));
            floating.count = shown;
            if shown == 0 {
                gone.push(*mana_id);
            }
        }
        for mana_id in gone {
            self.mana.floating.remove(&mana_id);
        }
        for (seat, mana) in lost {
            self.write_lose(seat, mana, Cause::Unknown);
        }
    }

    /// Take `mana` out of `seat`'s pool, returning what it held of it.
    fn take_from_pool(&mut self, seat: u32, mana: Vec<Mana>) -> Vec<Mana> {
        let pool = self.mana.pool.entry(seat).or_default();
        let mut taken = Vec::new();
        for symbol in mana {
            if let Some(at) = pool.iter().position(|held| *held == symbol) {
                pool.remove(at);
                taken.push(symbol);
            }
        }
        taken
    }

    fn write_lose(&mut self, seat: u32, mana: Vec<Mana>, cause: Cause) {
        let taken = self.take_from_pool(seat, mana);
        if taken.is_empty() {
            return;
        }
        let player = self.roster.name(seat);
        self.event(
            Statement::LoseMana {
                player,
                mana: taken,
            },
            Some(cause),
        );
    }

    /// Every pool empties as a step or phase ends (CR 106.4): unspent mana
    /// is lost before the next step or turn line, in the step it was added.
    pub(super) fn empty_pools(&mut self) {
        self.flush_payment();
        let seats: Vec<u32> = self.mana.pool.keys().copied().collect();
        for seat in seats {
            let mana = self.mana.pool.get(&seat).cloned().unwrap_or_default();
            let taken = self.take_from_pool(seat, mana);
            self.mana
                .floating
                .retain(|_, floating| floating.seat != seat);
            if taken.is_empty() {
                continue;
            }
            let level = if self.turn == 0 { 1 } else { 2 };
            let player = self.roster.name(seat);
            self.line(
                level,
                Statement::LoseMana {
                    player,
                    mana: taken,
                },
                Some(Cause::Rule("empty-pool".into())),
            );
        }
    }

    /// A mana ability's id ends: it no longer makes mana.
    pub(super) fn close_mana_ability(&mut self, key: CardKey) {
        self.mana
            .open_abilities
            .retain(|ability| ability.key != key);
    }
}
