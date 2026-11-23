//! Priority, passes and stack snapshots (spec sections 9.3 and 9.9,
//! decision 0007).
//!
//! Arena names the player with priority (`turnInfo.priorityPlayer`) in
//! every state, and sends a state each time priority moves: in the
//! fixtures every step shows both players receiving it, and every
//! resolution comes after both players held priority with the stack as it
//! was. So, while the stack is not empty:
//!
//! - a player **receives priority** when a state shows them holding it with
//!   a stack the record has not shown them yet (CR 117.3b, 117.3c, 117.3d);
//! - a player **passes** when priority moves to the other player with the
//!   stack unchanged (CR 117.3d), or when the top of the stack starts to
//!   resolve (all players passed in succession, CR 117.4, 608.1).
//!
//! No one receives priority while a spell or ability resolves, while a
//! spell is being cast or an ability activated (until its
//! `UserActionTaken`), or while a player chooses targets (CR 601.2). Each
//! `priority` line has the stack under it, built from the record's own
//! lines the way a reader replays them (spec 14.3), so it always equals
//! the replayed stack. Priority with an empty stack is not written (the
//! standard allows it, spec 9.3).

use std::collections::BTreeMap;

use ::mtgr::{AbilityKind, Choice, Line, ObjRef, Statement, Zone};

use super::GameWriter;
use crate::Annotation;

pub(super) const SELECTING_TARGETS: &str = "AnnotationType_PlayerSelectingTargets";
/// `UserActionTaken` action types that finish casting a spell (1) or
/// activating an ability (2).
const CAST_OR_ACTIVATE: [i64; 2] = [1, 2];

/// One object as the record's lines leave it.
#[derive(Debug, Clone, Default)]
struct ViewObject {
    name: Option<String>,
    /// The face or half a spell was cast as.
    face: Option<String>,
    controller: String,
    /// An ability's kind, source id and text.
    ability: Option<(AbilityKind, u32, Option<String>)>,
    /// Choices and targets written since it was put on the stack, in
    /// order.
    choices: Vec<Choice>,
    /// A copy still carries its original's targets: the first `target`
    /// line on it replaces them, and keeps its other choices (spec 7.2).
    copied_targets: bool,
}

/// The stack as a reader replays the record's lines (spec 14.3): only the
/// statements the writer uses that change the stack or what a snapshot
/// shows.
#[derive(Debug, Default)]
pub(super) struct StackView {
    /// Lines already applied.
    applied: usize,
    objects: BTreeMap<u32, ViewObject>,
    /// Object ids, bottom first.
    stack: Vec<u32>,
}

impl StackView {
    /// Apply the lines written since the last call.
    pub fn catch_up(&mut self, lines: &[Line]) {
        for line in &lines[self.applied..] {
            self.apply(&line.statement);
        }
        self.applied = lines.len();
    }

    /// The stack's object ids, bottom first.
    pub fn ids(&self) -> Vec<u32> {
        self.stack.clone()
    }

    fn create(&mut self, id: u32, object: ViewObject, in_stack: bool) {
        self.objects.insert(id, object);
        if in_stack {
            self.stack.push(id);
        }
    }

    fn put_on_stack(&mut self, id: u32, face: Option<String>) {
        self.stack.retain(|on| *on != id);
        self.stack.push(id);
        if let Some(object) = self.objects.get_mut(&id) {
            object.choices.clear();
            object.face = face;
        }
    }

    fn apply(&mut self, statement: &Statement) {
        match statement {
            Statement::NewCard {
                object,
                owner,
                zone,
                ..
            } => self.create(
                object.id,
                ViewObject {
                    name: object.name.clone(),
                    controller: owner.clone(),
                    ..ViewObject::default()
                },
                *zone == Zone::Stack,
            ),
            Statement::NewCopy {
                id,
                of,
                name,
                owner,
                zone,
                ..
            } => {
                let choices = self
                    .objects
                    .get(&of.id)
                    .map(|original| original.choices.clone())
                    .unwrap_or_default();
                self.create(
                    *id,
                    ViewObject {
                        name: name.clone(),
                        controller: owner.clone(),
                        copied_targets: choices
                            .iter()
                            .any(|choice| matches!(choice, Choice::Target(_))),
                        choices,
                        ..ViewObject::default()
                    },
                    *zone == Zone::Stack,
                );
            }
            Statement::NewHidden {
                id, owner, zone, ..
            } => self.create(
                *id,
                ViewObject {
                    controller: owner.clone(),
                    ..ViewObject::default()
                },
                *zone == Zone::Stack,
            ),
            Statement::NewToken {
                object,
                owner,
                zone,
                ..
            } => self.create(
                object.id,
                ViewObject {
                    name: object.name.clone(),
                    controller: owner.clone(),
                    ..ViewObject::default()
                },
                *zone == Zone::Stack,
            ),
            Statement::NewAbility {
                id,
                kind,
                of,
                text,
                controller,
                in_stack,
            } => self.create(
                *id,
                ViewObject {
                    controller: controller.clone(),
                    ability: Some((*kind, of.id, text.clone())),
                    ..ViewObject::default()
                },
                *in_stack,
            ),
            Statement::Identify { id, name, .. } => {
                if let Some(object) = self.objects.get_mut(id) {
                    object.name = name.clone();
                }
            }
            Statement::Cast { object, face, .. } => self.put_on_stack(object.id, face.clone()),
            Statement::Move {
                object, from, to, ..
            } => {
                if *from == Zone::Stack {
                    self.stack.retain(|on| *on != object.id);
                }
                if *to == Zone::Stack {
                    self.put_on_stack(object.id, None);
                }
            }
            Statement::Play { object, .. }
            | Statement::Cease { object }
            | Statement::Untracked { object } => {
                self.stack.retain(|on| *on != object.id);
            }
            Statement::Target { object, target } => {
                if let Some(viewed) = self.objects.get_mut(&object.id) {
                    if viewed.copied_targets {
                        viewed
                            .choices
                            .retain(|choice| !matches!(choice, Choice::Target(_)));
                        viewed.copied_targets = false;
                    }
                    viewed.choices.push(Choice::Target(target.clone()));
                }
            }
            Statement::ChooseX { object, value } => {
                if let Some(viewed) = self.objects.get_mut(&object.id) {
                    viewed.choices.push(Choice::X(*value));
                }
            }
            Statement::ChooseObject { object, chosen } => {
                if let Some(viewed) = self.objects.get_mut(&object.id) {
                    viewed.choices.push(Choice::Object(chosen.clone()));
                }
            }
            Statement::Keyframe => self.stack.clear(),
            _ => {}
        }
    }

    /// The `stack` entries, top first (spec 9.9).
    pub fn snapshot(&self) -> Vec<Statement> {
        let mut entries = Vec::new();
        for id in self.stack.iter().rev() {
            let Some(object) = self.objects.get(id) else {
                continue;
            };
            let entry = match &object.ability {
                Some((kind, source, text)) => Statement::StackAbility {
                    id: *id,
                    kind: *kind,
                    of: ObjRef {
                        id: *source,
                        name: self.objects.get(source).and_then(|s| s.name.clone()),
                    },
                    text: text.clone(),
                    controller: object.controller.clone(),
                    choices: object.choices.clone(),
                },
                None => Statement::StackEntry {
                    object: ObjRef {
                        id: *id,
                        name: object.name.clone(),
                    },
                    face: object.face.clone(),
                    controller: object.controller.clone(),
                    choices: object.choices.clone(),
                },
            };
            entries.push(entry);
        }
        entries
    }
}

impl GameWriter<'_> {
    /// `UserActionTaken`: a spell is cast or an ability activated, so its
    /// controller receives priority again (CR 117.3c).
    pub(super) fn write_user_action(&mut self, annotation: &Annotation) {
        let finished = annotation
            .int("actionType")
            .is_some_and(|action| CAST_OR_ACTIVATE.contains(&action));
        if !finished {
            return;
        }
        for id in &annotation.affected_ids {
            let key = self.fold.identity.key(*id);
            self.announced.remove(&key);
        }
    }

    /// `PlayerSelectingTargets`: a player chooses the object's targets.
    pub(super) fn write_selecting_targets(&mut self, annotation: &Annotation) {
        for id in &annotation.affected_ids {
            let key = self.fold.identity.key(*id);
            self.selecting.insert(key);
        }
    }

    /// The player Arena shows holding priority at the end of state `index`:
    /// the pass that gave it to them, and their `priority` line with the
    /// stack, when the stack is not empty and they did not hold it already.
    pub(super) fn priority_after_state(&mut self, index: usize) {
        if self.turn == 0 || self.conceded.is_some() {
            return;
        }
        let seat = self.fold.states[index].priority_seat;
        let busy = !self.resolving_now.is_empty()
            || !self.announced.is_empty()
            || !self.selecting.is_empty();
        if seat == 0 || busy {
            return;
        }
        self.stack_view.catch_up(&self.lines);
        let stack = self.stack_view.ids();
        if stack.is_empty() {
            self.priority = None;
            return;
        }
        if let Some((holder, held)) = self.priority.clone()
            && held == stack
        {
            if holder == seat {
                return;
            }
            self.write_pass(holder, held);
        }
        self.write_priority(seat, stack);
    }

    /// The player holding priority passes as the top of the stack starts
    /// to resolve (or fizzles): every player passed in succession (CR
    /// 117.4). Called only when the record writes that resolution, so the
    /// stack is not empty. When the record has not shown both players pass
    /// with this stack, or shows no one holding priority, the resolution
    /// is not backed by the passes it needs, so the record says it is
    /// incomplete rather than imply them.
    pub(super) fn pass_before_resolution(&mut self) {
        if self.turn == 0 {
            return;
        }
        let both_passed = match self.priority.clone() {
            Some((holder, held)) => {
                self.write_pass(holder, held.clone());
                self.passed_stack == held && self.passed.len() >= 2
            }
            None => false,
        };
        if !both_passed {
            self.write_incomplete(
                "Arena resolved the top of the stack without showing both players pass".to_string(),
            );
        }
    }

    /// `seat` passes with `stack` (ids, bottom first).
    fn write_pass(&mut self, seat: u32, stack: Vec<u32>) {
        if self.passed_stack != stack {
            self.passed.clear();
            self.passed_stack = stack;
        }
        self.passed.insert(seat);
        let player = self.roster.name(seat);
        self.event(Statement::Pass { player }, None);
        self.priority = None;
    }

    /// The object left the stack (or, for an ability, ended): it is no
    /// longer being cast, activated or targeted, and pays for nothing.
    pub(super) fn left_stack(&mut self, key: crate::CardKey) {
        self.announced.remove(&key);
        self.selecting.remove(&key);
        self.paid_for.remove(&key);
    }

    fn write_priority(&mut self, seat: u32, stack: Vec<u32>) {
        let player = self.roster.name(seat);
        self.event(Statement::Priority { player }, None);
        self.line(3, Statement::Stack, None);
        for entry in self.stack_view.snapshot() {
            self.line(4, entry, None);
        }
        self.priority = Some((seat, stack));
    }
}
