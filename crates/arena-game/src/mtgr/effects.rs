//! Lasting effects (spec 9.10, decisions 0043 and 0044): Arena reports them
//! in part, so the record lists `effects` in `Partial` and writes the ones
//! Arena shows.
//!
//! - **What is one.** A continuous effect created by a resolving spell or
//!   ability (CR 611.2): Arena's `LayeredEffectCreated` whose affector is
//!   the object resolving now, an ability or an instant or sorcery spell. A
//!   permanent spell's effects as it resolves are its static abilities (CR
//!   611.3; three-games log lines 9625, 18369, 20346: an Aura), not lasting
//!   effects.
//! - **Only past cleanup.** An effect is written only when it is still in
//!   force as the next turn begins (Arena destroys it later, or never). One
//!   Arena ends in the cleanup step lasted "until end of turn" and is not
//!   recorded (spec 9.10): three-games log lines 3422 (Reckless
//!   Stormseeker's trigger) and 21446 (The Wandering Emperor's +1).
//! - **One effect per resolution.** Arena gives each layer of an effect its
//!   own id (power, abilities, types). One resolution's ids are one effect
//!   (CR 613.6), written once: `new effect #N of SOURCE ? controller "P" by
//!   #M`, the source being the ability's source or the spell itself, the
//!   text `?` (Arena sends none), the cause the resolving object.
//! - **Not written** either: an effect Arena destroys in the turn it was
//!   created, such as one whose object leaves play that turn (`Partial`
//!   allows a missing effect).
//! - **Written first.** As its creator starts to resolve, before the lines
//!   its resolution causes (counters, types, power and toughness).
//! - **End.** When Arena destroys the last of its ids: `cease by ?`, as
//!   Arena does not say why (a stated duration ending, or its object
//!   leaving). Keyframes list each effect still in force; one whose end
//!   Arena did not report (its ids gone after a gap) is left out, and the
//!   keyframe ends it (decision 0043).

use std::collections::BTreeSet;

use ::mtgr::{Cause, ObjRef, Statement};

use super::GameWriter;
use super::ids::Kind;
use super::turns::turn_cut;
use crate::{Annotation, CardKey, Fold};

pub(super) const LAYERED_EFFECT_CREATED: &str = "AnnotationType_LayeredEffectCreated";
pub(super) const LAYERED_EFFECT_DESTROYED: &str = "AnnotationType_LayeredEffectDestroyed";
pub(super) const LAYERED_EFFECT: &str = "AnnotationType_LayeredEffect";

/// A lasting effect the record wrote and has not ended.
#[derive(Debug, Clone)]
pub(super) struct LiveEffect {
    pub id: u32,
    pub of: ObjRef,
    pub controller: String,
    /// Arena's effect ids still in force.
    pub arena_ids: BTreeSet<u32>,
    /// The resolving object that created it, and the state it did.
    creator: CardKey,
    state_index: usize,
}

/// The Arena effect ids still in force as a later turn begins than the one
/// they were created in (destroyed after that turn's start, or never while
/// the game reached it).
pub(super) fn lasting_effect_ids(fold: &Fold) -> BTreeSet<u32> {
    let mut turn = 0;
    let mut created: Vec<(u32, u32)> = Vec::new();
    let mut lasting = BTreeSet::new();
    for (index, state) in fold.states.iter().enumerate() {
        let annotations = &fold.steps[index].annotations;
        let cut = (state.turn > turn).then(|| turn_cut(annotations));
        for (position, annotation) in annotations.iter().enumerate() {
            let now = if cut.is_some_and(|cut| position >= cut) {
                state.turn
            } else {
                turn
            };
            if annotation.is(LAYERED_EFFECT_CREATED) {
                for id in &annotation.affected_ids {
                    created.push((*id, now));
                }
            }
            if annotation.is(LAYERED_EFFECT_DESTROYED) {
                for id in &annotation.affected_ids {
                    if let Some(at) = created.iter().position(|(effect, _)| effect == id) {
                        let (_, born) = created.remove(at);
                        if now > born {
                            lasting.insert(*id);
                        }
                    }
                }
            }
        }
        turn = turn.max(state.turn);
    }
    for (id, born) in created {
        if turn > born {
            lasting.insert(id);
        }
    }
    lasting
}

/// The Arena effect ids any state shows in a persistent `LayeredEffect`.
pub(super) fn shown_effect_ids(fold: &Fold) -> BTreeSet<u32> {
    let mut shown = BTreeSet::new();
    for state in &fold.states {
        shown.extend(persistent_effect_ids(state));
    }
    shown
}

/// The effect ids of a state's persistent `LayeredEffect` annotations.
fn persistent_effect_ids(state: &crate::GameState) -> BTreeSet<u32> {
    state
        .persistent
        .values()
        .filter(|annotation| annotation.is(LAYERED_EFFECT))
        .filter_map(|annotation| annotation.int("effect_id"))
        .filter_map(|id| u32::try_from(id).ok())
        .collect()
}

impl GameWriter<'_> {
    /// `LayeredEffectCreated`: a lasting effect of the resolving object.
    pub(super) fn write_effects_created(&mut self, index: usize, annotation: &Annotation) {
        let lasting: Vec<u32> = annotation
            .affected_ids
            .iter()
            .filter(|id| self.lasting_effects.contains(id))
            .copied()
            .collect();
        if lasting.is_empty() {
            return;
        }
        let key = self.fold.identity.key(annotation.affector_id);
        if !self.resolving_now.contains(&key)
            || !self.resolves_without_a_permanent(index, key, annotation.affector_id)
        {
            return;
        }
        if let Some(effect) = self
            .effects
            .iter_mut()
            .find(|effect| effect.creator == key && effect.state_index == index)
        {
            effect.arena_ids.extend(lasting);
            return;
        }
        let Some(tracked) = self.ids.get(key) else {
            return;
        };
        let cause = tracked.id;
        let (source, seat) = match tracked.kind {
            Kind::Ability { .. } => (
                self.ability_sources
                    .get(&key)
                    .and_then(|source| self.ids.obj(*source)),
                tracked.owner_seat,
            ),
            _ => {
                let controller = self.fold.states[index]
                    .objects
                    .get(&annotation.affector_id)
                    .map(|object| object.controller_seat)
                    .filter(|seat| *seat != 0)
                    .unwrap_or(tracked.owner_seat);
                (self.ids.obj(key), controller)
            }
        };
        let Some(of) = source else {
            return;
        };
        let id = self.ids.allocate();
        let controller = self.roster.name(seat);
        self.event(
            Statement::NewEffect {
                id,
                of: of.clone(),
                text: None,
                controller: controller.clone(),
            },
            Some(Cause::Object(cause)),
        );
        self.effects.push(LiveEffect {
            id,
            of,
            controller,
            arena_ids: lasting.into_iter().collect(),
            creator: key,
            state_index: index,
        });
    }

    /// As object `affector_id` starts to resolve in state `index`: the
    /// lasting effects that state's annotations show it creating, before
    /// the lines its resolution causes.
    pub(super) fn write_effects_of_resolution(&mut self, index: usize, affector_id: u32) {
        let fold = self.fold;
        for annotation in &fold.steps[index].annotations {
            if annotation.is(LAYERED_EFFECT_CREATED) && annotation.affector_id == affector_id {
                self.write_effects_created(index, annotation);
            }
        }
    }

    /// Whether the resolving object is an ability or an instant or sorcery
    /// spell (a permanent spell's effects are its static abilities).
    fn resolves_without_a_permanent(&self, index: usize, key: CardKey, instance_id: u32) -> bool {
        let Some(tracked) = self.ids.get(key) else {
            return false;
        };
        match tracked.kind {
            Kind::Ability { .. } => true,
            Kind::Token => false,
            Kind::Card | Kind::Copy => {
                let fold = self.fold;
                let object = fold.states[index].objects.get(&instance_id).or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|at| fold.states[at].objects.get(&instance_id))
                });
                object.is_some_and(|object| {
                    object
                        .card_types
                        .iter()
                        .any(|kind| kind == "CardType_Instant" || kind == "CardType_Sorcery")
                })
            }
        }
    }

    /// `LayeredEffectDestroyed`: an effect whose last id Arena destroys
    /// ends.
    pub(super) fn write_effects_destroyed(&mut self, annotation: &Annotation) {
        for arena_id in &annotation.affected_ids {
            let Some(at) = self
                .effects
                .iter()
                .position(|effect| effect.arena_ids.contains(arena_id))
            else {
                continue;
            };
            self.effects[at].arena_ids.remove(arena_id);
            if !self.effects[at].arena_ids.is_empty() {
                continue;
            }
            let effect = self.effects.remove(at);
            self.event(
                Statement::Cease {
                    object: ObjRef {
                        id: effect.id,
                        name: None,
                    },
                },
                Some(Cause::Unknown),
            );
        }
    }

    /// The keyframe entries of the effects in force as the turn begins at
    /// annotation `cut` of state `index`. An effect whose ids are gone from
    /// the state with no `LayeredEffectDestroyed` the record saw (after a
    /// gap) is left out, and the keyframe ends it (decision 0043).
    pub(super) fn keyframe_effects(&mut self, index: usize, cut: usize) -> Vec<Statement> {
        let state = &self.fold.states[index];
        let in_force = persistent_effect_ids(state);
        let mut destroyed_later: BTreeSet<u32> = BTreeSet::new();
        for annotation in &self.fold.steps[index].annotations[cut..] {
            if annotation.is(LAYERED_EFFECT_DESTROYED) {
                destroyed_later.extend(annotation.affected_ids.iter().copied());
            }
        }
        let shown = &self.shown_effects;
        self.effects.retain(|effect| {
            effect.arena_ids.iter().any(|id| {
                !shown.contains(id) || in_force.contains(id) || destroyed_later.contains(id)
            })
        });
        self.effects
            .iter()
            .map(|effect| Statement::KeyframeEffect {
                id: effect.id,
                of: effect.of.clone(),
                text: None,
                controller: effect.controller.clone(),
            })
            .collect()
    }
}
