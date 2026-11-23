//! Turns, steps and main phases (spec section 9.2).
//!
//! Step lines come from the `PhaseOrStepModified` annotations, in order with
//! the events, so a step that passes inside one state message (untap, then
//! upkeep) is still written. A state whose step no annotation announced
//! writes its step too, when it comes later in the turn.

use ::mtgr::{Statement, Step};

use super::GameWriter;
use crate::{Annotation, GameState};

pub(super) const PHASE_OR_STEP: &str = "AnnotationType_PhaseOrStepModified";
const NEW_TURN: &str = "AnnotationType_NewTurnStarted";

/// The steps and main phases, as Arena numbers them in
/// `PhaseOrStepModified` (phase, step), in turn order. A first strike
/// damage step comes before the combat damage step.
const STEPS: [((i64, i64), Step); 13] = [
    ((1, 1), Step::Untap),
    ((1, 2), Step::Upkeep),
    ((1, 3), Step::Draw),
    ((2, 0), Step::PrecombatMain),
    ((3, 4), Step::BeginningOfCombat),
    ((3, 5), Step::DeclareAttackers),
    ((3, 6), Step::DeclareBlockers),
    ((3, 11), Step::CombatDamage),
    ((3, 7), Step::CombatDamage),
    ((3, 8), Step::EndOfCombat),
    ((4, 0), Step::PostcombatMain),
    ((5, 9), Step::End),
    ((5, 10), Step::Cleanup),
];

/// A `PhaseOrStepModified` (phase, step) as a step of `STEPS`: Arena
/// announces the combat phase and the ending phase with step 0, and they
/// begin with the beginning of combat and end steps. The beginning phase
/// with step 0 is no step.
pub(super) fn step_pair(phase: i64, step: i64) -> Option<(i64, i64)> {
    let pair = match (phase, step) {
        (2, _) => (2, 0),
        (4, _) => (4, 0),
        (3, 0) => (3, 4),
        (5, 0) => (5, 9),
        other => other,
    };
    STEPS
        .iter()
        .any(|(known, _)| *known == pair)
        .then_some(pair)
}

/// A state's `turnInfo` phase and step as a step of `STEPS`.
pub(super) fn state_pair(state: &GameState) -> Option<(i64, i64)> {
    let pair = match (state.phase.as_str(), state.step.as_str()) {
        ("Phase_Beginning", "Step_Untap") => (1, 1),
        ("Phase_Beginning", "Step_Upkeep") => (1, 2),
        ("Phase_Beginning", "Step_Draw") => (1, 3),
        ("Phase_Main1", _) => (2, 0),
        ("Phase_Combat", "Step_BeginCombat") => (3, 4),
        ("Phase_Combat", "Step_DeclareAttack") => (3, 5),
        ("Phase_Combat", "Step_DeclareBlock") => (3, 6),
        ("Phase_Combat", "Step_FirstStrikeDamage") => (3, 11),
        ("Phase_Combat", "Step_CombatDamage") => (3, 7),
        ("Phase_Combat", "Step_EndCombat") => (3, 8),
        ("Phase_Main2", _) => (4, 0),
        ("Phase_Ending", "Step_End") => (5, 9),
        ("Phase_Ending", "Step_Cleanup") => (5, 10),
        _ => return None,
    };
    Some(pair)
}

fn step_of(pair: (i64, i64)) -> Step {
    STEPS
        .iter()
        .find(|(known, _)| *known == pair)
        .map(|(_, step)| *step)
        .unwrap_or(Step::Untap)
}

fn step_order(pair: (i64, i64)) -> usize {
    STEPS
        .iter()
        .position(|(known, _)| *known == pair)
        .unwrap_or(0)
}

/// Where a state that begins a turn begins it: at its `NewTurnStarted`, or
/// at its first beginning phase step (the first turn's `NewTurnStarted`
/// comes with the opening hands), else after its annotations. Annotations
/// before it end the previous turn (cleanup) or the pregame (cards put on
/// the bottom).
pub(super) fn turn_cut(annotations: &[Annotation]) -> usize {
    annotations
        .iter()
        .position(|annotation| {
            if annotation.is(NEW_TURN) {
                return true;
            }
            annotation.is(PHASE_OR_STEP)
                && annotation.int("phase") == Some(1)
                && matches!(annotation.int("step"), Some(1..=3))
        })
        .unwrap_or(annotations.len())
}

impl GameWriter<'_> {
    /// The turn line and keyframe.
    pub(super) fn start_turn(&mut self, index: usize, cut: usize) {
        // Permanent changes the message makes before the turn begins end the
        // previous turn (or the pregame); the keyframe shows them.
        self.sync_permanents_until(index, Some(cut));
        let life = self.life_at(index, cut);
        self.sync_life(&life);
        self.empty_pools();
        // A turn begins with an empty stack (CR 500.2).
        self.priority = None;
        self.announced.clear();
        self.selecting.clear();
        self.passed.clear();
        self.passed_stack.clear();
        if self.turn > 0 {
            self.check_libraries(index);
        }
        let state = &self.fold.states[index];
        self.turn = state.turn;
        self.last_step = None;
        self.attacks.clear();
        self.blocks.clear();
        let player = self
            .roster
            .name(state.active_seat.unwrap_or(self.starting_seat));
        self.line(
            0,
            Statement::Turn {
                number: state.turn,
                player,
            },
            None,
        );
        self.write_turn_keyframe(index, cut);
    }

    /// A step line, unless it is the step already written.
    pub(super) fn write_step(&mut self, pair: (i64, i64)) {
        if self.last_step == Some(pair) {
            return;
        }
        // Declarations are final as their step ends.
        self.write_declarations();
        // Mana empties from the pools as the step ends (CR 106.4).
        self.empty_pools();
        self.last_step = Some(pair);
        self.line(1, Statement::Step(step_of(pair)), None);
    }

    /// The step a state is in, when no annotation announced it (and it
    /// comes later in the turn than the last step written).
    pub(super) fn step_from_state(&mut self, state: &GameState) {
        let Some(pair) = state_pair(state) else {
            return;
        };
        let later = match self.last_step {
            Some(last) => step_order(pair) > step_order(last),
            None => true,
        };
        if later {
            self.write_step(pair);
        }
    }
}
