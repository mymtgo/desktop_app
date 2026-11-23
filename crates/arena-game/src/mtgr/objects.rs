//! Objects other than zone moves and abilities (spec sections 6.5, 7.3,
//! 7.5 and 7.7): tokens, tapping, counters, and changes to a permanent's
//! card types or power and toughness.
//!
//! - **Tokens** come from `TokenCreated`: `new token` with the token's name
//!   (its card data by grpId, else its subtypes and "Token", CR 111.4) and
//!   how it enters. A token that left the battlefield ceases at
//!   `TokenDeleted` (`rule:sba`, CR 704.5d).
//! - **Tapping**: `TappedUntappedPermanent` writes `tap` or `untap`.
//! - **Attaching**: `AttachmentCreated` writes `attach` (`rule:resolve` for
//!   an Aura attaching as it resolves, else the resolving object).
//! - **Unattaching**: a persistent `Attachment` that ends while the
//!   attached object stays on the battlefield and is not attached again
//!   (Equipment whose creature left) writes `unattach`: `rule:sba` when
//!   what it was attached to is gone, else `by ?`.
//! - **Counters**: `CounterAdded` and `CounterRemoved`.
//! - **Types and power and toughness**: written as changes from what the
//!   record last gave, after the annotation that changed them (counters, a
//!   power and toughness effect), and at the end of each state message for
//!   any other change. There the cause is the source of a layered effect
//!   created or ended in the message that lists the permanent (a static
//!   ability or an Aura); an "until end of turn" effect ending in cleanup is
//!   `rule:cleanup`.
//!
//! Each line carries the cause Arena gives (`affectorId`) when it is an
//! object the record tracks, else `by ?`.

use std::collections::BTreeMap;

use ::mtgr::{CardType, Cause, ObjRef, PtValue, Statement, Zone};

use super::GameWriter;
use super::effects::{LAYERED_EFFECT, LAYERED_EFFECT_CREATED, LAYERED_EFFECT_DESTROYED};
use super::ids::Kind;
use super::moves::{card_types, creature_pt, zone_of};
use crate::message::ATTACHMENT;
use crate::{Annotation, CardKey, Fold, GameObject};

pub(super) const TOKEN_CREATED: &str = "AnnotationType_TokenCreated";
pub(super) const TOKEN_DELETED: &str = "AnnotationType_TokenDeleted";
pub(super) const TAPPED_UNTAPPED: &str = "AnnotationType_TappedUntappedPermanent";
pub(super) const COUNTER_ADDED: &str = "AnnotationType_CounterAdded";
pub(super) const COUNTER_REMOVED: &str = "AnnotationType_CounterRemoved";
pub(super) const ATTACHMENT_CREATED: &str = "AnnotationType_AttachmentCreated";
pub(super) const PT_MODIFIED: &str = "AnnotationType_PowerToughnessModCreated";
/// Where Arena keeps the ids a card had before it moved.
const LIMBO: &str = "ZoneType_Limbo";

/// Whether `annotations` change a permanent's types or power and toughness
/// (effects starting or ending, counters).
fn changes_permanents(annotations: &[Annotation]) -> bool {
    annotations.iter().any(|annotation| {
        annotation.is(LAYERED_EFFECT_CREATED)
            || annotation.is(LAYERED_EFFECT_DESTROYED)
            || annotation.is(PT_MODIFIED)
            || annotation.is(COUNTER_ADDED)
            || annotation.is(COUNTER_REMOVED)
    })
}

/// A permanent as the turn that begins at annotation `cut` of state
/// `index` began: the object whose face, types and power and toughness it
/// had, and whether its power and toughness then are unknown.
pub(super) struct TurnStart<'f> {
    pub object: &'f GameObject,
    pub pt_unknown: bool,
}

/// A permanent's object as the turn that begins at annotation `cut` of
/// state `index` began. The state is the one after the whole message, so a
/// permanent's types and power and toughness are the previous message's
/// when the message changed none before `cut` (the change came after the
/// turn began), or when the permanent changed face in it (day and night
/// turn permanents over as the turn begins, CR 731.2). Otherwise the
/// message's changes before `cut` (an "until end of turn" effect ending in
/// cleanup) are the earlier turn's.
///
/// A permanent that changed face in the message after effects on it ended
/// before `cut` had a power and toughness Arena never shows: the previous
/// message's is from before the effects ended, the state's from after it
/// turned over. That value is unknown (carried note: three-games #92).
pub(super) fn at_turn_start<'f>(
    fold: &'f Fold,
    index: usize,
    cut: usize,
    object: &'f GameObject,
) -> TurnStart<'f> {
    let Some(earlier) = index
        .checked_sub(1)
        .and_then(|before| fold.states[before].objects.get(&object.instance_id))
    else {
        return TurnStart {
            object,
            pt_unknown: false,
        };
    };
    let annotations = &fold.steps[index].annotations[..cut];
    if earlier.grp_id != object.grp_id {
        let changed = changes_permanent(fold, index, annotations, object.instance_id);
        return TurnStart {
            object: earlier,
            pt_unknown: changed && creature_pt(earlier).is_some(),
        };
    }
    let object = if changes_permanents(annotations) {
        object
    } else {
        earlier
    };
    TurnStart {
        object,
        pt_unknown: false,
    }
}

/// Whether `annotations` of state `index` change the permanent with
/// `instance_id`: counters or a power and toughness effect on it, or a
/// layered effect that lists it starting or ending.
fn changes_permanent(
    fold: &Fold,
    index: usize,
    annotations: &[Annotation],
    instance_id: u32,
) -> bool {
    let direct = annotations.iter().any(|annotation| {
        (annotation.is(PT_MODIFIED)
            || annotation.is(COUNTER_ADDED)
            || annotation.is(COUNTER_REMOVED))
            && annotation.affected_ids.contains(&instance_id)
    });
    direct || effect_sources_of(fold, index, annotations).contains_key(&instance_id)
}

/// The `.mtgr` name of an Arena counter type (decision 0039), for the
/// types seen in the fixtures, each checked against its card: 1 is the
/// +1/+1 counter (Youthful Valkyrie, Ascendant Packleader), 7 loyalty (The
/// Wandering Emperor enters with 3), 19 charge (Evendo, Waking Haven's
/// station). `None` for any other type: its counters are not written,
/// neither as lines nor in keyframes, and a change to them writes
/// `incomplete` (spec 9.8).
pub(super) fn counter_name(counter_type: &str) -> Option<&'static str> {
    match counter_type {
        "1" => Some("+1/+1"),
        "7" => Some("loyalty"),
        "19" => Some("charge"),
        _ => None,
    }
}

/// An object's counters with written names, in code point order of the
/// name (counters of unknown types left out).
pub(super) fn named_counters(object: &GameObject) -> Vec<(String, u32)> {
    let mut counters: BTreeMap<String, u32> = BTreeMap::new();
    for (counter_type, count) in &object.counters {
        if let Some(name) = counter_name(counter_type)
            && *count > 0
        {
            *counters.entry(name.to_string()).or_insert(0) += count;
        }
    }
    counters.into_iter().collect()
}

/// A permanent's card types and power and toughness, by key.
type Shown = (CardKey, Vec<CardType>, Option<(i32, i32)>);

/// A record zone as a word in a reason.
fn zone_word(zone: Zone) -> &'static str {
    match zone {
        Zone::Battlefield => "battlefield",
        Zone::Graveyard => "graveyard",
        Zone::Exile => "exile",
        _ => "command zone",
    }
}

fn pt_value(pt: Option<(i32, i32)>) -> PtValue {
    match pt {
        Some((power, toughness)) => PtValue::Pt(power, toughness),
        None => PtValue::None,
    }
}

impl GameWriter<'_> {
    /// A token's name: its card data by grpId, else its subtypes followed
    /// by "Token" (CR 111.4), listed in `subtype_tokens`.
    fn token_name(&mut self, object: &GameObject) -> String {
        if let Some(card) = self.cards.get(&object.grp_id) {
            return card.name.clone();
        }
        let subtypes: Vec<String> = object
            .subtypes
            .iter()
            .map(|subtype| subtype.trim_start_matches("SubType_").to_string())
            .collect();
        let mut words = subtypes.clone();
        words.push("Token".to_string());
        let name = words.join(" ");
        self.subtype_tokens.insert(
            object.grp_id,
            super::SubtypeToken {
                name: name.clone(),
                subtypes,
                grp_id: object.grp_id,
            },
        );
        name
    }

    /// `TokenCreated`: each created token, with how it enters.
    pub(super) fn write_tokens_created(&mut self, index: usize, annotation: &Annotation) {
        let fold = self.fold;
        for instance_id in &annotation.affected_ids {
            let Some(object) = fold.states[index].objects.get(instance_id) else {
                continue;
            };
            let Some(zone) = zone_of(&fold.states[index].zone_kind(object.zone_id)) else {
                continue;
            };
            let key = fold.identity.key(*instance_id);
            if self.ids.get(key).is_some() {
                continue;
            }
            let name = self.token_name(object);
            let entering = self.entering(index, Some(object));
            let cause = self.object_cause(annotation.affector_id);
            let owner_seat = object.owner_seat;
            let id = self.ids.introduce(
                key,
                Kind::Token,
                Some(name.clone()),
                false,
                zone,
                owner_seat,
            );
            self.event(
                Statement::NewToken {
                    object: ObjRef {
                        id,
                        name: Some(name),
                    },
                    owner: self.roster.name(owner_seat),
                    zone,
                    entering: entering.clone(),
                    sources: Vec::new(),
                },
                Some(cause),
            );
            if zone == Zone::Battlefield {
                self.entered(key, &entering);
            }
        }
    }

    /// `TokenDeleted`: a token outside the battlefield ceases to exist as
    /// a state-based action (CR 111.7, 704.5d).
    pub(super) fn write_tokens_deleted(&mut self, annotation: &Annotation) {
        for instance_id in &annotation.affected_ids {
            let key = self.fold.identity.key(*instance_id);
            let Some(tracked) = self.ids.get(key) else {
                continue;
            };
            if tracked.kind != Kind::Token {
                continue;
            }
            let cause = if tracked.zone == Zone::Battlefield {
                Cause::Unknown
            } else {
                Cause::Rule("sba".into())
            };
            let object = ObjRef {
                id: tracked.id,
                name: tracked.name.clone(),
            };
            self.event(Statement::Cease { object }, Some(cause));
            self.ids.end(key);
        }
    }

    /// `TappedUntappedPermanent`: each affected permanent taps (`tapped` 1)
    /// or untaps.
    pub(super) fn write_tapped(&mut self, annotation: &Annotation) {
        let taps = annotation.int("tapped") == Some(1);
        for instance_id in &annotation.affected_ids {
            let key = self.fold.identity.key(*instance_id);
            let Some(tracked) = self.ids.get(key) else {
                continue;
            };
            if tracked.zone != Zone::Battlefield || tracked.tapped == taps {
                continue;
            }
            let object = ObjRef {
                id: tracked.id,
                name: tracked.name.clone(),
            };
            let cause = self.tap_cause(annotation.affector_id, taps);
            let statement = if taps {
                Statement::Tap { object }
            } else {
                Statement::Untap { object }
            };
            self.event(statement, Some(cause));
            if let Some(tracked) = self.ids.get_mut(key) {
                tracked.tapped = taps;
            }
        }
    }

    /// Why a permanent tapped or untapped: the untap step's untap, attackers
    /// tapping as they are declared, else `cost_or_effect`.
    fn tap_cause(&self, affector_id: u32, taps: bool) -> Cause {
        if affector_id == 0 {
            return match (taps, self.last_step) {
                (false, Some((1, 1))) => Cause::Rule("untap".into()),
                (true, Some((3, 5))) => Cause::Rule("declare-attackers".into()),
                _ => Cause::Unknown,
            };
        }
        self.cost_or_effect(affector_id)
    }

    /// `CounterAdded` and `CounterRemoved`: the counters of each affected
    /// permanent, then the change they make to its power and toughness.
    ///
    /// Counters on a player (poison, energy, experience; the affected id is
    /// a seat) are written `incomplete`: no fixture or owner game has one,
    /// so no Arena counter type is known to name a player counter (spec
    /// 9.5), and the record does not guess.
    pub(super) fn write_counters(&mut self, index: usize, annotation: &Annotation) {
        let adds = annotation.is(COUNTER_ADDED);
        let counter_type = annotation.int("counter_type").unwrap_or(0).to_string();
        let players = &self.fold.states[index].players;
        let seats: Vec<u32> = annotation
            .affected_ids
            .iter()
            .copied()
            .filter(|id| players.contains_key(id))
            .collect();
        for seat in seats {
            let player = self.roster.name(seat);
            self.write_incomplete(format!(
                "Arena counter type {counter_type} on player {player} has no counter name here"
            ));
        }
        let Some(name) = counter_name(&counter_type) else {
            let tracked = annotation.affected_ids.iter().any(|id| {
                let key = self.fold.identity.key(*id);
                self.ids.get(key).is_some()
            });
            if tracked {
                self.write_incomplete(format!(
                    "Arena counter type {counter_type} has no counter name here"
                ));
            }
            return;
        };
        let amount = annotation
            .int("transaction_amount")
            .and_then(|amount| u32::try_from(amount).ok())
            .unwrap_or(0);
        let cause = self.cost_or_effect(annotation.affector_id);
        for instance_id in &annotation.affected_ids {
            let key = self.fold.identity.key(*instance_id);
            let Some(tracked) = self.ids.get_mut(key) else {
                continue;
            };
            if tracked.zone != Zone::Battlefield {
                continue;
            }
            let from = tracked.counters.get(name).copied().unwrap_or(0);
            let to = if adds {
                from + amount
            } else {
                from.saturating_sub(amount)
            };
            if to == 0 {
                tracked.counters.remove(name);
            } else {
                tracked.counters.insert(name.to_string(), to);
            }
            let object = ObjRef {
                id: tracked.id,
                name: tracked.name.clone(),
            };
            self.event(
                Statement::Counters {
                    object,
                    kind: name.to_string(),
                    from,
                    to,
                },
                Some(cause.clone()),
            );
            self.sync_permanent(index, key, cause.clone());
        }
    }

    /// `PowerToughnessModCreated`: the affected creatures' new power and
    /// toughness, caused by the affector.
    pub(super) fn write_pt_modified(&mut self, index: usize, annotation: &Annotation) {
        let cause = self.object_cause(annotation.affector_id);
        for instance_id in &annotation.affected_ids {
            let key = self.fold.identity.key(*instance_id);
            self.sync_permanent(index, key, cause.clone());
        }
    }

    /// Each tracked permanent of state `index`, by key.
    fn permanents(&self, index: usize) -> Vec<(CardKey, &GameObject)> {
        let fold = self.fold;
        let state = &fold.states[index];
        let mut permanents = Vec::new();
        for zone in state.zones.values() {
            if zone.kind != "ZoneType_Battlefield" {
                continue;
            }
            for instance_id in &zone.object_ids {
                if let Some(object) = state.objects.get(instance_id) {
                    permanents.push((fold.identity.key(*instance_id), object));
                }
            }
        }
        permanents
    }

    /// Write a permanent's card type and power and toughness changes since
    /// the record last gave them, as state `index` shows them.
    fn sync_permanent(&mut self, index: usize, key: CardKey, cause: Cause) {
        let found = self
            .permanents(index)
            .into_iter()
            .find(|(found, _)| *found == key)
            .map(|(_, object)| (card_types(object), creature_pt(object)));
        let Some((types, pt)) = found else {
            return;
        };
        self.sync_values(key, types, pt, false, cause);
    }

    /// Write the changes from what the record last gave a permanent to
    /// `types` and `pt` (an unknown power and toughness when `pt_unknown`).
    fn sync_values(
        &mut self,
        key: CardKey,
        types: Vec<CardType>,
        pt: Option<(i32, i32)>,
        pt_unknown: bool,
        cause: Cause,
    ) {
        let Some(tracked) = self.ids.get(key) else {
            return;
        };
        if tracked.zone != Zone::Battlefield || matches!(tracked.kind, Kind::Ability { .. }) {
            return;
        }
        let object = ObjRef {
            id: tracked.id,
            name: tracked.name.clone(),
        };
        let old_types = tracked.types.clone();
        let old_pt = if tracked.pt_unknown {
            PtValue::Unknown
        } else {
            pt_value(tracked.pt)
        };
        let new_pt = if pt_unknown {
            PtValue::Unknown
        } else {
            pt_value(pt)
        };
        if !types.is_empty() && types != old_types {
            self.event(
                Statement::TypesChange {
                    object: object.clone(),
                    from: old_types,
                    to: types.clone(),
                },
                Some(cause.clone()),
            );
            if let Some(tracked) = self.ids.get_mut(key) {
                tracked.types = types;
            }
        }
        if new_pt != old_pt {
            self.event(
                Statement::PtChange {
                    object,
                    from: old_pt,
                    to: new_pt,
                },
                Some(cause),
            );
            if let Some(tracked) = self.ids.get_mut(key) {
                tracked.pt = pt;
                tracked.pt_unknown = pt_unknown;
            }
        }
    }

    /// `control PLAYER` for each tracked permanent whose controller in state
    /// `index` (Arena's `controllerSeatId`) is not the one the record last
    /// gave it (spec 7.3). Arena sends no annotation for a change of
    /// control, so the cause is the object resolving, else unknown.
    pub(super) fn write_control(&mut self, index: usize) {
        let changed: Vec<(CardKey, u32)> = self
            .permanents(index)
            .into_iter()
            .filter(|(_, object)| object.controller_seat != 0)
            .filter_map(|(key, object)| {
                let tracked = self.ids.get(key)?;
                let differs = tracked.zone == Zone::Battlefield
                    && !matches!(tracked.kind, Kind::Ability { .. })
                    && tracked.controller_seat != object.controller_seat;
                differs.then_some((key, object.controller_seat))
            })
            .collect();
        for (key, seat) in changed {
            let Some(object) = self.ids.obj(key) else {
                continue;
            };
            let cause = match self.resolving_now.last().and_then(|key| self.ids.get(*key)) {
                Some(resolving) => Cause::Object(resolving.id),
                None => Cause::Unknown,
            };
            let player = self.roster.name(seat);
            self.event(Statement::Control { object, player }, Some(cause));
            if let Some(tracked) = self.ids.get_mut(key) {
                tracked.controller_seat = seat;
            }
        }
    }

    /// Every tracked permanent's type and power and toughness changes that
    /// no annotation explained: `rule:cleanup` in the cleanup step ("until
    /// end of turn" effects ending, CR 514.2), else `by ?`.
    pub(super) fn sync_permanents(&mut self, index: usize) {
        self.sync_permanents_until(index, None);
    }

    /// `sync_permanents` for the changes made before the turn that begins
    /// at annotation `cut` of state `index` (see `at_turn_start`), or for
    /// the whole state when `cut` is `None`.
    pub(super) fn sync_permanents_until(&mut self, index: usize, cut: Option<usize>) {
        let fold = self.fold;
        let cleanup = self.last_step == Some((5, 10));
        let sources = effect_sources(fold, index);
        let values: Vec<(Shown, bool, Cause)> = self
            .permanents(index)
            .into_iter()
            .map(|(key, object)| {
                let shown = match cut {
                    Some(cut) => at_turn_start(fold, index, cut, object),
                    None => TurnStart {
                        object,
                        pt_unknown: false,
                    },
                };
                let cause = match sources.get(&object.instance_id) {
                    Some(source) if source.ended && cleanup => Cause::Rule("cleanup".into()),
                    Some(source) => self.object_cause(source.affector_id),
                    None if cleanup => Cause::Rule("cleanup".into()),
                    None => Cause::Unknown,
                };
                let types = card_types(shown.object);
                let pt = creature_pt(shown.object);
                ((key, types, pt), shown.pt_unknown, cause)
            })
            .collect();
        for ((key, types, pt), pt_unknown, cause) in values {
            self.sync_values(key, types, pt, pt_unknown, cause);
        }
    }

    /// Each tracked permanent that shows another face than the record gave
    /// it turns over (`transform`, decision 0022), then its other changes
    /// follow by the same cause. Day and night turn daybound and nightbound
    /// permanents over by their own abilities (CR 702.145), so after a
    /// `day` or `night` line (`by_themselves`) each is its own cause;
    /// otherwise the cause is the object resolving, else unknown.
    pub(super) fn write_transforms(&mut self, index: usize, by_themselves: bool) {
        let fold = self.fold;
        let state = &fold.states[index];
        let permanents: Vec<(CardKey, u32)> = self
            .permanents(index)
            .into_iter()
            .map(|(key, object)| (key, object.instance_id))
            .collect();
        let mut turned: Vec<(CardKey, Option<String>)> = Vec::new();
        for (key, instance_id) in permanents {
            let Some(object) = state.objects.get(&instance_id) else {
                continue;
            };
            let Some(tracked) = self.ids.get(key) else {
                continue;
            };
            if tracked.zone != Zone::Battlefield || object.otherside_grp_id == 0 {
                continue;
            }
            let shown = tracked.face.clone();
            let face = self.shown_face(state, object);
            if face != shown {
                turned.push((key, face));
            }
        }
        for (key, face) in turned {
            let Some(object) = self.ids.obj(key) else {
                continue;
            };
            let cause = if by_themselves {
                Cause::Object(object.id)
            } else {
                match self.resolver_other_than(key) {
                    Some(id) => Cause::Object(id),
                    None => Cause::Unknown,
                }
            };
            self.event(Statement::Transform { object }, Some(cause.clone()));
            if let Some(tracked) = self.ids.get_mut(key) {
                tracked.face = face;
            }
            self.sync_permanent(index, key, cause);
        }
    }

    /// Cards and copies the record has in a public zone that state `index`
    /// shows in no zone, with no move to say where they went. A copy ceases
    /// (`copies.rs`). A card: the record cannot say why it left, so it says
    /// it is incomplete (silently after a gap, until the next keyframe), and
    /// ends its id.
    pub(super) fn write_removed_cards(&mut self, index: usize) {
        let fold = self.fold;
        let state = &fold.states[index];
        let mut present = std::collections::BTreeSet::new();
        for zone in state.zones.values() {
            if zone.kind == LIMBO {
                continue;
            }
            for instance_id in &zone.object_ids {
                present.insert(fold.identity.key(*instance_id));
            }
        }
        for key in self.ids.keys() {
            let Some(tracked) = self.ids.get(key) else {
                continue;
            };
            let public = matches!(
                tracked.zone,
                Zone::Battlefield | Zone::Graveyard | Zone::Exile | Zone::Command
            );
            let kind = tracked.kind;
            if !matches!(kind, Kind::Card | Kind::Copy) || !public || present.contains(&key) {
                continue;
            }
            let zone = zone_word(tracked.zone);
            let id = tracked.id;
            if kind == Kind::Copy {
                // A copy outside the stack and the battlefield ceases to
                // exist (CR 704.5e, 707.10a).
                if let Some(object) = self.ids.obj(key) {
                    self.event(Statement::Cease { object }, Some(Cause::Rule("sba".into())));
                }
            } else if !self.after_gap {
                // After a gap the card may have moved in the lost states
                // under an id the record never saw; the gap's `incomplete`
                // already covers it, and the next keyframe resyncs.
                self.write_incomplete(format!(
                    "Arena removed #{id} from the {zone} without a move"
                ));
            }
            self.ids.end(key);
        }
    }

    /// `AttachmentCreated`: the affector becomes attached to each affected
    /// object. An Aura that resolved in the same message (Arena completes
    /// the resolution, moves it, then attaches it) attaches as it resolves.
    pub(super) fn write_attachment(&mut self, index: usize, annotation: &Annotation) {
        let fold = self.fold;
        let key = fold.identity.key(annotation.affector_id);
        let Some(object) = self.ids.obj(key) else {
            return;
        };
        let resolved_now = fold.transfers.iter().any(|transfer| {
            transfer.state_index == index && transfer.card == key && transfer.category == "Resolve"
        });
        let cause = if resolved_now || self.resolving_now.last() == Some(&key) {
            Cause::Rule("resolve".into())
        } else {
            match self.resolver_other_than(key) {
                Some(id) => Cause::Object(id),
                None => Cause::Unknown,
            }
        };
        for target in &annotation.affected_ids {
            let target_key = self.fold.identity.key(*target);
            let Some(to) = self.ids.obj(target_key) else {
                continue;
            };
            self.event(
                Statement::Attach {
                    object: object.clone(),
                    to,
                },
                Some(cause.clone()),
            );
        }
    }
}

impl GameWriter<'_> {
    /// `unattach` for each `Attachment` of the state before `index` that
    /// ended in it, for an object still on the battlefield (a permanent that
    /// leaves it needs no line, spec 14.3) and not attached to something
    /// else in `index` (`attach` already says that).
    pub(super) fn write_unattached(&mut self, index: usize) {
        let fold = self.fold;
        let Some(before) = index.checked_sub(1).map(|earlier| &fold.states[earlier]) else {
            return;
        };
        let now = &fold.states[index].persistent;
        for annotation in before.persistent.values() {
            if !annotation.is(ATTACHMENT) || now.contains_key(&annotation.id) {
                continue;
            }
            let attached_again = now
                .values()
                .any(|other| other.is(ATTACHMENT) && other.affector_id == annotation.affector_id);
            let key = fold.identity.key(annotation.affector_id);
            let on_battlefield = self
                .ids
                .get(key)
                .is_some_and(|tracked| tracked.zone == Zone::Battlefield);
            if attached_again || !on_battlefield {
                continue;
            }
            let Some(object) = self.ids.obj(key) else {
                continue;
            };
            let target_gone = annotation.affected_ids.iter().all(|target| {
                self.ids
                    .get(fold.identity.key(*target))
                    .is_none_or(|tracked| tracked.zone != Zone::Battlefield)
            });
            let cause = if target_gone {
                Cause::Rule("sba".into())
            } else {
                Cause::Unknown
            };
            self.event(Statement::Unattach { object }, Some(cause));
        }
    }
}

/// The source of a layered effect that changed a permanent.
pub(super) struct EffectSource {
    pub affector_id: u32,
    /// The effect ended (else it started).
    pub ended: bool,
}

/// The permanents each layered effect created or ended in state `index`'s
/// message lists, by instance id, with the effect's source. The persistent
/// `LayeredEffect` annotations name the permanents an effect changes (by
/// `effect_id`); an ended effect's are read from the state before.
pub(super) fn effect_sources(fold: &Fold, index: usize) -> BTreeMap<u32, EffectSource> {
    effect_sources_of(fold, index, &fold.steps[index].annotations)
}

/// `effect_sources` for some of state `index`'s annotations.
fn effect_sources_of(
    fold: &Fold,
    index: usize,
    annotations: &[Annotation],
) -> BTreeMap<u32, EffectSource> {
    let mut sources = BTreeMap::new();
    for annotation in annotations {
        let ended = annotation.is(LAYERED_EFFECT_DESTROYED);
        if !(ended || annotation.is(LAYERED_EFFECT_CREATED)) {
            continue;
        }
        let state = if ended {
            index.checked_sub(1).map(|before| &fold.states[before])
        } else {
            Some(&fold.states[index])
        };
        let Some(state) = state else {
            continue;
        };
        for effect_id in &annotation.affected_ids {
            for effect in state.persistent.values() {
                if !effect.is(LAYERED_EFFECT)
                    || effect.int("effect_id") != Some(i64::from(*effect_id))
                {
                    continue;
                }
                let affector_id = if annotation.affector_id != 0 {
                    annotation.affector_id
                } else {
                    effect.affector_id
                };
                for permanent in &effect.affected_ids {
                    sources
                        .entry(*permanent)
                        .or_insert(EffectSource { affector_id, ended });
                }
            }
        }
    }
    sources
}
