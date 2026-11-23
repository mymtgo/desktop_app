//! Combat, damage and life (spec sections 7.3 and 9.5).
//!
//! - **Attacks and blocks** come from the objects' attack and block state.
//!   While a player declares, Arena's states show their choices as they
//!   change (`AttackState_Declared`, `BlockState_Declared`), and the whole
//!   declaration once it is submitted (`AttackState_Attacking`,
//!   `BlockState_Blocking`). The declaration is written before the first
//!   other event of its step (the first attacker tapping by
//!   `rule:declare-attackers`, a "whenever this blocks" trigger, a mana
//!   ability paying an attack tax while the player still chooses), or as
//!   the step ends, and always whole: from the first state of the step,
//!   from the current one on, that shows it submitted. A declaration no
//!   state shows submitted (the game ended mid-declaration, or the state
//!   is lost in a gap) is not written. Each creature attacks or blocks
//!   once a turn.
//! - **Damage** comes from `DamageDealt`: `rule:combat-damage` for an
//!   attacking or blocking source in a combat damage step, else the source.
//! - **Life** comes from `ModifiedLife`, with the cause of the damage that
//!   changed it (lifelink included). A life total that changes with no
//!   annotation is written `by ?` at the end of the state message.

use ::mtgr::{Cause, Statement};

use super::GameWriter;
use crate::{Annotation, GameObject};

pub(super) const DAMAGE_DEALT: &str = "AnnotationType_DamageDealt";
pub(super) const MODIFIED_LIFE: &str = "AnnotationType_ModifiedLife";

const STEP_DECLARE_ATTACK: &str = "Step_DeclareAttack";
const STEP_DECLARE_BLOCK: &str = "Step_DeclareBlock";

impl GameWriter<'_> {
    /// The state whose declaration the record writes: the current state
    /// when it shows an object `submitted` (as the step ends, the state
    /// that ends it still shows the declaration), else the first later
    /// state of this turn's declare step `step` that does. `None` when none
    /// does (the game ended while a player chose, or the submitting state
    /// is lost in a gap): a choice never submitted is never written, as it
    /// may be the opponent's.
    fn submitted_declaration(
        &self,
        step: &str,
        submitted: fn(&GameObject) -> bool,
    ) -> Option<usize> {
        let states = &self.fold.states;
        let current = &states[self.index];
        if current.objects.values().any(submitted) {
            return Some(self.index);
        }
        states
            .iter()
            .enumerate()
            .skip(self.index + 1)
            .take_while(|(_, state)| state.turn == current.turn && state.step == step)
            .find(|(_, state)| state.objects.values().any(submitted))
            .map(|(index, _)| index)
    }

    /// The attacks or blocks of the current declare step not yet written.
    pub(super) fn write_declarations(&mut self) {
        match self.last_step {
            Some((3, 5)) => self.write_attacks(),
            Some((3, 6)) => self.write_blocks(),
            _ => {}
        }
    }

    /// The creatures declared as attackers in the current state that the
    /// record has not written this turn, by Arena instance id.
    pub(super) fn write_attacks(&mut self) {
        let fold = self.fold;
        let Some(index) =
            self.submitted_declaration(STEP_DECLARE_ATTACK, |object| object.attack_submitted)
        else {
            return;
        };
        for object in fold.states[index].objects.values() {
            if !object.attacking {
                continue;
            }
            let key = fold.identity.key(object.instance_id);
            if self.attacks.contains(&key) {
                continue;
            }
            let (Some(attacker), Some(target_id)) = (self.ids.obj(key), object.attack_target)
            else {
                continue;
            };
            let Some(target) = self.target_ref(index, target_id) else {
                continue;
            };
            self.attacks.insert(key);
            self.line(
                2,
                Statement::Attack {
                    object: attacker,
                    target,
                },
                None,
            );
        }
    }

    /// The creatures declared as blockers in the current state that the
    /// record has not written this turn, one line per attacker blocked.
    pub(super) fn write_blocks(&mut self) {
        let fold = self.fold;
        let Some(index) =
            self.submitted_declaration(STEP_DECLARE_BLOCK, |object| object.block_submitted)
        else {
            return;
        };
        for object in fold.states[index].objects.values() {
            if !object.blocking {
                continue;
            }
            let key = fold.identity.key(object.instance_id);
            if self.blocks.contains(&key) {
                continue;
            }
            let Some(blocker) = self.ids.obj(key) else {
                continue;
            };
            self.blocks.insert(key);
            for attacker_id in &object.block_attackers {
                let attacker_key = fold.identity.key(*attacker_id);
                if let Some(attacker) = self.ids.obj(attacker_key) {
                    self.line(
                        2,
                        Statement::Block {
                            object: blocker.clone(),
                            attacker,
                        },
                        None,
                    );
                }
            }
        }
    }

    /// The cause of damage from `source_id` and of the life it changes:
    /// combat damage from an attacking or blocking creature in a combat
    /// damage step, else the source. The creature may have died from the
    /// same damage, so the state before is read too.
    fn damage_cause(&self, source_id: u32) -> Cause {
        let combat_step = matches!(self.last_step, Some((3, 7)) | Some((3, 11)));
        let first = self.index.saturating_sub(1);
        let in_combat = self.fold.states[first..=self.index].iter().any(|state| {
            state
                .objects
                .get(&source_id)
                .is_some_and(|object| object.attacking || object.blocking)
        });
        if combat_step && in_combat {
            Cause::Rule("combat-damage".into())
        } else {
            self.object_cause(source_id)
        }
    }

    /// `DamageDealt`: the source deals damage to each affected player or
    /// object.
    pub(super) fn write_damage(&mut self, annotation: &Annotation) {
        let key = self.fold.identity.key(annotation.affector_id);
        let Some(source) = self.ids.obj(key) else {
            return;
        };
        let amount = annotation
            .int("damage")
            .and_then(|amount| u32::try_from(amount).ok())
            .unwrap_or(0);
        let cause = self.damage_cause(annotation.affector_id);
        for target_id in &annotation.affected_ids {
            if let Some(target) = self.target_ref(self.index, *target_id) {
                self.event(
                    Statement::Damage {
                        object: source.clone(),
                        target,
                        amount,
                    },
                    Some(cause.clone()),
                );
            }
        }
    }

    /// `ModifiedLife`: each affected player's life changes by `life`.
    pub(super) fn write_life(&mut self, annotation: &Annotation) {
        let change = annotation
            .int("life")
            .and_then(|change| i32::try_from(change).ok())
            .unwrap_or(0);
        let cause = self.damage_cause(annotation.affector_id);
        for seat in &annotation.affected_ids {
            let Some(from) = self.life.get(seat).copied() else {
                continue;
            };
            self.write_life_change(*seat, from, from + change, cause.clone());
        }
    }

    fn write_life_change(&mut self, seat: u32, from: i32, to: i32, cause: Cause) {
        if from == to {
            return;
        }
        self.life.insert(seat, to);
        self.event(
            Statement::Life {
                player: self.roster.name(seat),
                from,
                to,
            },
            Some(cause),
        );
    }

    /// Life totals that changed with no annotation, `by ?`.
    pub(super) fn sync_life(&mut self, life: &[(u32, i32)]) {
        for (seat, to) in life {
            if let Some(from) = self.life.get(seat).copied() {
                self.write_life_change(*seat, from, *to, Cause::Unknown);
            }
        }
    }
}
