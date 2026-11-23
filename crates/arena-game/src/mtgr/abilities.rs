//! Abilities, targets and resolution (spec sections 6.5, 7.2 and 7.4).
//!
//! - **Abilities** come from `AbilityInstanceCreated`. Their kind is what
//!   Arena reports: `UserActionTaken` action type 4 is a mana ability, 2 an
//!   activated one (a loyalty ability when its source is a planeswalker),
//!   and an ability put on the stack with no action is triggered. An
//!   ability that no action activated and that never reaches the stack is
//!   a triggered mana ability when its source pays with mana while it is
//!   open (CR 605.1b, `mana.rs`). Any other ability instance is not
//!   written. Arena sends no rules text, so the text is `?`. An activated
//!   ability is written as it is activated. A triggered
//!   ability is written when a state first shows it on the stack (CR 603.3),
//!   abilities put there together bottom first, in the order the state's
//!   stack zone gives (top first). An ability on the stack ceases at
//!   `AbilityInstanceDeleted` (`rule:resolve` once it resolved, else `by ?`).
//! - **Targets** are the persistent `TargetSpec` annotations of a spell or
//!   ability on the stack, written when the player submits them, or at the
//!   end of the state message that shows them.
//! - **Resolution**: `ResolutionStart` writes `resolve`; the object
//!   resolves until its `ResolutionComplete`.
//! - **Countering**: a stack ability removed while another object resolves
//!   was countered by it (a ward trigger): `countered`, then `cease`, both
//!   by the resolving object. A spell Arena moves as `Countered` is in
//!   `moves.rs`. Arena reports no fizzle; an object with targets that
//!   leaves the stack unresolved and uncountered fizzled.
//! - **Costs**: what an ability that has not started resolving does is its
//!   cost (`by cost:#N`: tapping, sacrificing, loyalty).

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{AbilityKind, Cause, ObjRef, Statement, TargetRef, Zone};

use super::GameWriter;
use super::ids::Kind;
use super::mana::OpenManaAbility;
use crate::{Annotation, CardKey, Fold, GameObject};

pub(super) const ABILITY_CREATED: &str = "AnnotationType_AbilityInstanceCreated";
pub(super) const ABILITY_DELETED: &str = "AnnotationType_AbilityInstanceDeleted";
pub(super) const RESOLUTION_START: &str = "AnnotationType_ResolutionStart";
pub(super) const RESOLUTION_COMPLETE: &str = "AnnotationType_ResolutionComplete";
pub(super) const SUBMITTED_TARGETS: &str = "AnnotationType_PlayerSubmittedTargets";
pub(super) const USER_ACTION: &str = "AnnotationType_UserActionTaken";
const TARGET_SPEC: &str = "AnnotationType_TargetSpec";
const PLANESWALKER: &str = "CardType_Planeswalker";
/// `UserActionTaken` action types (`actionType`).
const ACTION_ACTIVATE: i64 = 2;
const ACTION_ACTIVATE_MANA: i64 = 4;

/// The action type of each instance a `UserActionTaken` names.
pub(super) fn user_actions(fold: &Fold) -> BTreeMap<u32, i64> {
    let mut actions = BTreeMap::new();
    for step in &fold.steps {
        for annotation in &step.annotations {
            if !annotation.is(USER_ACTION) {
                continue;
            }
            if let Some(action) = annotation.int("actionType") {
                for id in &annotation.affected_ids {
                    actions.insert(*id, action);
                }
            }
        }
    }
    actions
}

/// Every instance id some state shows in a stack zone.
pub(super) fn stacked_ids(fold: &Fold) -> BTreeSet<u32> {
    let mut ids = BTreeSet::new();
    for state in &fold.states {
        for zone in state.zones.values() {
            if zone.kind == "ZoneType_Stack" {
                ids.extend(zone.object_ids.iter().copied());
            }
        }
    }
    ids
}

/// A triggered ability created and not yet shown on the stack.
#[derive(Debug, Clone)]
pub(super) struct PendingTrigger {
    instance_id: u32,
    key: CardKey,
    source: CardKey,
    of: ObjRef,
    controller_seat: u32,
}

/// A new ability: its Arena instance, its object and its source's.
struct NewAbility {
    instance_id: u32,
    key: CardKey,
    source: CardKey,
    kind: AbilityKind,
    of: ObjRef,
    seat: u32,
}

impl GameWriter<'_> {
    /// The kind of ability instance `instance_id` (whose source is
    /// `source`), as Arena reports it; `None` when it does not say.
    fn ability_kind(&self, instance_id: u32, source: Option<&GameObject>) -> Option<AbilityKind> {
        match self.actions.get(&instance_id) {
            Some(&ACTION_ACTIVATE_MANA) => Some(AbilityKind::Mana),
            Some(&ACTION_ACTIVATE) => {
                let planeswalker = source
                    .is_some_and(|object| object.card_types.iter().any(|t| t == PLANESWALKER));
                Some(if planeswalker {
                    AbilityKind::Loyalty
                } else {
                    AbilityKind::Activated
                })
            }
            _ if self.stacked.contains(&instance_id) => Some(AbilityKind::Triggered),
            _ if self.mana_triggers.contains(&instance_id) => Some(AbilityKind::Mana),
            _ => None,
        }
    }

    /// `AbilityInstanceCreated`: each ability of the affector, when the
    /// record tracks its source and Arena tells its kind.
    pub(super) fn write_abilities_created(&mut self, index: usize, annotation: &Annotation) {
        let fold = self.fold;
        let source_key = fold.identity.key(annotation.affector_id);
        let Some(of) = self.ids.obj(source_key) else {
            return;
        };
        let source = fold.states[index].objects.get(&annotation.affector_id);
        let source_controller = source
            .map(|object| object.controller_seat)
            .filter(|seat| *seat != 0);
        for instance_id in &annotation.affected_ids {
            let Some(kind) = self.ability_kind(*instance_id, source) else {
                continue;
            };
            let key = fold.identity.key(*instance_id);
            if self.ids.get(key).is_some() {
                continue;
            }
            let controller_seat = fold
                .states
                .iter()
                .skip(index)
                .find_map(|state| state.objects.get(instance_id))
                .map(|object| object.controller_seat)
                .filter(|seat| *seat != 0)
                .or(source_controller)
                .unwrap_or_else(|| {
                    self.ids
                        .get(source_key)
                        .map(|tracked| tracked.owner_seat)
                        .unwrap_or(self.roster.local_seat)
                });
            if kind == AbilityKind::Triggered {
                if !self.pending_triggers.iter().any(|t| t.key == key) {
                    self.pending_triggers.push(PendingTrigger {
                        instance_id: *instance_id,
                        key,
                        source: source_key,
                        of: of.clone(),
                        controller_seat,
                    });
                }
                continue;
            }
            if self.actions.get(instance_id) == Some(&ACTION_ACTIVATE) {
                // Announced: no one receives priority until it is activated.
                self.announced.insert(key);
            }
            self.write_new_ability(NewAbility {
                instance_id: *instance_id,
                key,
                source: source_key,
                kind,
                of: of.clone(),
                seat: controller_seat,
            });
        }
    }

    fn write_new_ability(&mut self, ability: NewAbility) {
        let in_stack = ability.kind != AbilityKind::Mana;
        let id = self.ids.introduce(
            ability.key,
            Kind::Ability { on_stack: in_stack },
            None,
            false,
            Zone::Stack,
            ability.seat,
        );
        self.ability_sources.insert(ability.key, ability.source);
        if !in_stack {
            self.mana.open_abilities.push(OpenManaAbility {
                key: ability.key,
                source: ability.source,
                id,
                seat: ability.seat,
                grp_id: self
                    .ability_grp_ids
                    .get(&ability.instance_id)
                    .copied()
                    .unwrap_or(0),
            });
        }
        self.event(
            Statement::NewAbility {
                id,
                kind: ability.kind,
                of: ability.of,
                text: None,
                controller: self.roster.name(ability.seat),
                in_stack,
            },
            None,
        );
    }

    /// The pending triggered abilities state `index` shows on the stack,
    /// bottom first.
    pub(super) fn write_pending_triggers(&mut self, index: usize) {
        if self.pending_triggers.is_empty() {
            return;
        }
        let fold = self.fold;
        let mut placed: Vec<(usize, PendingTrigger)> = Vec::new();
        let mut waiting: Vec<PendingTrigger> = Vec::new();
        for trigger in std::mem::take(&mut self.pending_triggers) {
            let position = fold.states[index]
                .zones
                .values()
                .filter(|zone| zone.kind == "ZoneType_Stack")
                .find_map(|zone| {
                    zone.object_ids
                        .iter()
                        .position(|id| *id == trigger.instance_id)
                });
            match position {
                Some(position) => placed.push((position, trigger)),
                None => waiting.push(trigger),
            }
        }
        self.pending_triggers = waiting;
        // The stack zone lists the top first.
        placed.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
        for (_, trigger) in placed {
            self.write_new_ability(NewAbility {
                instance_id: trigger.instance_id,
                key: trigger.key,
                source: trigger.source,
                kind: AbilityKind::Triggered,
                of: trigger.of,
                seat: trigger.controller_seat,
            });
        }
    }

    /// `AbilityInstanceDeleted`: an ability that was on the stack ceases
    /// (`rule:resolve` once it resolved). A mana ability's id just ends.
    pub(super) fn write_abilities_deleted(&mut self, index: usize, annotation: &Annotation) {
        self.write_pending_triggers(index);
        for instance_id in &annotation.affected_ids {
            let key = self.fold.identity.key(*instance_id);
            // A trigger that never reached the stack.
            self.pending_triggers.retain(|trigger| trigger.key != key);
            self.left_stack(key);
            if self.ids.get(key).is_none() {
                continue;
            }
            // Mana the ability made that floats is added before its id ends.
            self.sync_pool(index, Some(key));
            let Some(tracked) = self.ids.get(key) else {
                continue;
            };
            if tracked.kind == (Kind::Ability { on_stack: true }) {
                self.flush_payment();
                let Some(tracked) = self.ids.get(key) else {
                    continue;
                };
                let object = ObjRef {
                    id: tracked.id,
                    name: None,
                };
                let cause = if tracked.resolving {
                    Cause::Rule("resolve".into())
                } else if let Some(counter) = self.resolver_other_than(key) {
                    let cause = Cause::Object(counter);
                    self.event(
                        Statement::Countered {
                            object: object.clone(),
                        },
                        Some(cause.clone()),
                    );
                    cause
                } else if self.fizzles(key) {
                    self.write_fizzle(object.clone());
                    Cause::Rule("fizzle".into())
                } else {
                    Cause::Unknown
                };
                self.event(Statement::Cease { object }, Some(cause));
                self.ids.end(key);
            } else if tracked.kind == (Kind::Ability { on_stack: false }) {
                self.ids.end(key);
                self.close_mana_ability(key);
            }
        }
    }

    /// Whether a tracked object is a spell or ability on the stack.
    fn on_stack(&self, key: CardKey) -> bool {
        self.ids.get(key).is_some_and(|tracked| match tracked.kind {
            Kind::Ability { on_stack } => on_stack,
            _ => tracked.zone == Zone::Stack,
        })
    }

    /// `ResolutionStart`: the spell or ability on the stack resolves, once
    /// every player passed in succession (CR 117.4, 608.1).
    pub(super) fn write_resolution_start(&mut self, index: usize, annotation: &Annotation) {
        self.write_resolve(index, annotation);
        self.write_effects_of_resolution(index, annotation.affector_id);
    }

    /// `#N resolve`, after the passes that let it resolve. An object the
    /// record does not show on the stack (or shows resolving already)
    /// writes nothing, passes included: the record's stack does not change.
    fn write_resolve(&mut self, index: usize, annotation: &Annotation) {
        self.write_pending_triggers(index);
        let key = self.fold.identity.key(annotation.affector_id);
        self.resolving_now.push(key);
        if !self.on_stack(key) {
            return;
        }
        let Some(object) = self.ids.obj(key) else {
            return;
        };
        if let Some(tracked) = self.ids.get_mut(key) {
            if tracked.resolving {
                return;
            }
            tracked.resolving = true;
        }
        self.pass_before_resolution();
        self.event(Statement::Resolve { object }, None);
    }

    /// `ResolutionComplete`: the object's resolution is over. Mana it added
    /// that floats is added by it first.
    pub(super) fn write_resolution_complete(&mut self, index: usize, annotation: &Annotation) {
        let key = self.fold.identity.key(annotation.affector_id);
        self.sync_pool(index, Some(key));
        if let Some(at) = self.resolving_now.iter().rposition(|k| *k == key) {
            self.resolving_now.remove(at);
        }
    }

    /// The live id of the object resolving now, unless it is `key`.
    pub(super) fn resolver_other_than(&self, key: CardKey) -> Option<u32> {
        let resolver = *self.resolving_now.last()?;
        if resolver == key {
            return None;
        }
        self.ids.get(resolver).map(|tracked| tracked.id)
    }

    /// A player or an object Arena names by id: seats are the state's
    /// players, anything else an instance the record tracks.
    pub(super) fn target_ref(&self, index: usize, id: u32) -> Option<TargetRef> {
        if self.fold.states[index].players.contains_key(&id) {
            return Some(TargetRef::Player(self.roster.name(id)));
        }
        let key = self.fold.identity.key(id);
        self.ids.obj(key).map(TargetRef::Object)
    }

    /// The targets of the spell or ability `instance_id`, from the
    /// persistent `TargetSpec` annotations of state `index`, once.
    pub(super) fn write_targets_of(&mut self, index: usize, instance_id: u32) {
        let fold = self.fold;
        let key = fold.identity.key(instance_id);
        if self.targeted.contains(&key) || !self.on_stack(key) {
            return;
        }
        let Some(object) = self.ids.obj(key) else {
            return;
        };
        let mut specs: Vec<&Annotation> = fold.states[index]
            .persistent
            .values()
            .filter(|annotation| {
                annotation.is(TARGET_SPEC) && fold.identity.key(annotation.affector_id) == key
            })
            .collect();
        if specs.is_empty() {
            return;
        }
        specs.sort_by_key(|annotation| (annotation.int("index").unwrap_or(0), annotation.id));
        self.targeted.insert(key);
        let mut zones = Vec::new();
        for spec in specs {
            for target_id in &spec.affected_ids {
                let target_key = fold.identity.key(*target_id);
                let is_player = fold.states[index].players.contains_key(target_id);
                let tracked = self.ids.get(target_key).filter(|_| !is_player);
                zones.push(tracked.map(|tracked| (target_key, tracked.zone)));
                if let Some(target) = self.target_ref(index, *target_id) {
                    self.event(
                        Statement::Target {
                            object: object.clone(),
                            target,
                        },
                        None,
                    );
                }
            }
        }
        self.target_zones.insert(key, zones);
    }

    /// Whether the spell or ability `key` fizzles as it leaves the stack
    /// without resolving or being countered (CR 608.2b): it had targets.
    /// Arena removes a spell or ability whose targets are all illegal
    /// without a `ResolutionStart`, whether the targets left their zones or
    /// became illegal where they stand (protection, hexproof), so having
    /// targets is what tells a fizzle from other unresolved exits.
    pub(super) fn fizzles(&self, key: CardKey) -> bool {
        self.target_zones
            .get(&key)
            .is_some_and(|targets| !targets.is_empty())
    }

    /// `#N fizzle`: the top of the stack does not resolve because its
    /// targets are illegal, once every player passed (CR 117.4, 608.2b).
    pub(super) fn write_fizzle(&mut self, object: ObjRef) {
        self.pass_before_resolution();
        self.event(Statement::Fizzle { object }, None);
    }

    /// `PlayerSubmittedTargets`: the targets of each affected object.
    pub(super) fn write_submitted_targets(&mut self, index: usize, annotation: &Annotation) {
        self.write_pending_triggers(index);
        for instance_id in &annotation.affected_ids {
            let key = self.fold.identity.key(*instance_id);
            self.selecting.remove(&key);
            self.write_targets_of(index, *instance_id);
        }
    }

    /// The targets of every spell or ability on the stack that state
    /// `index` shows with targets the record has not written.
    pub(super) fn write_pending_targets(&mut self, index: usize) {
        let fold = self.fold;
        let mut affectors: Vec<u32> = fold.states[index]
            .persistent
            .values()
            .filter(|annotation| annotation.is(TARGET_SPEC))
            .map(|annotation| annotation.affector_id)
            .collect();
        affectors.sort_unstable();
        affectors.dedup();
        for affector in affectors {
            let key = fold.identity.key(affector);
            let resolving = self.ids.get(key).is_some_and(|tracked| tracked.resolving);
            if !resolving {
                self.write_targets_of(index, affector);
            }
        }
    }

    /// `cost:#N` for an ability that has not started resolving (its costs
    /// are being paid: tapping, loyalty), else the affector.
    pub(super) fn cost_or_effect(&self, affector_id: u32) -> Cause {
        let key = self.fold.identity.key(affector_id);
        match self.ids.get(key) {
            Some(tracked) if matches!(tracked.kind, Kind::Ability { .. }) && !tracked.resolving => {
                Cause::Cost(tracked.id)
            }
            _ => self.object_cause(affector_id),
        }
    }
}

/// `ChoiceResult`: a choice a player made (`affectedIds`: the seat,
/// `Choice_Value`: what was chosen).
pub(super) const CHOICE_RESULT: &str = "AnnotationType_ChoiceResult";
/// `CastingTimeOption`, a persistent annotation on a spell or ability
/// being cast or activated; `type` 2 is the client's
/// `CastingTimeOptionType_ChooseX`, with X as its `value` (Bo1 fixture: a
/// `CastingTimeOptionsResp` answering `numericInputValue` 1, then
/// `value` 1 on March of Otherworldly Light).
const CASTING_TIME_OPTION: &str = "AnnotationType_CastingTimeOption";
const CHOOSE_X: i64 = 2;

impl GameWriter<'_> {
    /// `choose x V` for each spell or ability on the stack that state
    /// `index` first shows with an X (`CastingTimeOption` type 2). Arena
    /// sends it after the cast and before the targets, as the rules order
    /// them (CR 601.2b, 601.2c).
    pub(super) fn write_x_choices(&mut self, index: usize) {
        let fold = self.fold;
        let mut chosen: Vec<(CardKey, u32)> = Vec::new();
        for annotation in fold.states[index].persistent.values() {
            if !annotation.is(CASTING_TIME_OPTION) || annotation.int("type") != Some(CHOOSE_X) {
                continue;
            }
            let Some(value) = annotation.int("value").and_then(|v| u32::try_from(v).ok()) else {
                continue;
            };
            for instance_id in &annotation.affected_ids {
                let key = fold.identity.key(*instance_id);
                if self.on_stack(key) && !self.x_chosen.contains(&key) {
                    chosen.push((key, value));
                }
            }
        }
        for (key, value) in chosen {
            let Some(object) = self.ids.obj(key) else {
                continue;
            };
            self.x_chosen.insert(key);
            self.event(Statement::ChooseX { object, value }, None);
        }
    }

    /// `ChoiceResult` with no `Choice_Domain`, while a spell or ability of
    /// the affector resolves: the player chose the object whose instance id
    /// is the value, without targeting (spec 7.2, `choose object`), as the
    /// sealed fixture shows each time (a permanent then tapped, a creature
    /// or a token then sacrificed by the resolving ability). A choice with a
    /// domain, of nothing (-1), or of an object the record lacks is not
    /// written: the record does not know what it chose.
    pub(super) fn write_choice_result(&mut self, annotation: &Annotation) {
        if annotation.int("Choice_Domain").is_some() {
            return;
        }
        let Some(value) = annotation
            .int("Choice_Value")
            .and_then(|value| u32::try_from(value).ok())
        else {
            return;
        };
        let Some(resolving) = self.resolving_now.last().copied() else {
            return;
        };
        let affector = self.fold.identity.key(annotation.affector_id);
        let of_affector =
            resolving == affector || self.ability_sources.get(&resolving) == Some(&affector);
        let chosen = self.ids.obj(self.fold.identity.key(value));
        let (true, Some(object), Some(chosen), Some(tracked)) = (
            of_affector,
            self.ids.obj(resolving),
            chosen,
            self.ids.get(resolving),
        ) else {
            return;
        };
        let controller = tracked.controller_seat;
        let cause = annotation
            .affected_ids
            .first()
            .filter(|seat| **seat != controller)
            .map(|seat| Cause::Player(self.roster.name(*seat)));
        self.event(Statement::ChooseObject { object, chosen }, cause);
    }
}
