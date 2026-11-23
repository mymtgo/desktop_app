//! Gaps in the stored messages (arena replays spec 4.3; standard sections
//! 9.8 and 14.4).
//!
//! Arena's log summarizes a message whose game states hold more than 50
//! objects or 50 annotations, and writes no JSON for it
//! (`[Message summarized because one or more GameStateMessages exceeded
//! the 50 GameObject or 50 Annotation limit.]`), so those states are lost
//! to every log reader (owner logs: each hole in the stored messages is one
//! such entry). A full state comes about once per game, so the record does
//! not wait for one (controller ruling, superseding spec 4.3's "the game
//! ends at the gap").
//!
//! A gap is a game state whose `prevGameStateId` is a state the fold never
//! saw. A full state that rewinds to an earlier state seen (Arena's undo)
//! is no gap, and neither is a state after the game is over (Arena sends a
//! few more as the match ends, some of them stored with the next game).
//!
//! At a gap the record writes `incomplete`, forgets what it was following
//! (mana, priority, triggers), and goes on folding the diffs after it: a
//! diff carries every object it changes whole, so only objects changed in
//! the lost states alone can be stale. The next turn's keyframe then
//! replaces the record's state, as a reader's (spec 14.4).

use std::collections::BTreeMap;

use ::mtgr::{Line, Statement, Zone};

use super::GameWriter;
use super::mana::ManaBook;
use crate::Fold;

/// The game state ids missing before state `index`, first and last, when
/// it follows a state the fold never saw.
pub(super) fn missing_before(fold: &Fold, index: usize) -> Option<(u32, u32)> {
    let before = fold.states[..index].last()?;
    if before.game_over {
        return None;
    }
    let previous = fold.states[index].prev_state_id?;
    if fold.states[..index]
        .iter()
        .any(|state| state.state_id == previous)
    {
        return None;
    }
    let first = before.state_id + 1;
    if first <= previous {
        Some((first, previous))
    } else {
        Some((previous, previous))
    }
}

impl GameWriter<'_> {
    /// Write the gap before the current state: game states `first` to
    /// `last` are missing.
    pub(super) fn write_gap(&mut self, first: u32, last: u32) {
        // Every hole in the owner's logs is a summarized message, so the
        // reason says so (`message summarized`, the trap's marker).
        let reason = if first == last {
            format!("Arena did not log game state {first} (message summarized)")
        } else {
            format!("Arena did not log game states {first} to {last} (message summarized)")
        };
        self.flush_payment();
        self.event(Statement::Incomplete { reason }, None);
        self.mana = ManaBook::default();
        self.pending_triggers.clear();
        self.resolving_now.clear();
        self.priority = None;
        self.announced.clear();
        self.selecting.clear();
        self.passed.clear();
        self.passed_stack.clear();
        self.after_gap = true;
    }
}

/// Each player's library size as a reader replays the record's lines (spec
/// 14.2): set by keyframes, one less for each move out of a library, one
/// more for each move into one. A card introduced in a library comes from
/// its untracked pool, so the size does not change.
#[derive(Debug, Default)]
pub(super) struct LibraryView {
    /// Lines already applied.
    applied: usize,
    /// Each object's owner, by id.
    owners: BTreeMap<u32, String>,
    sizes: BTreeMap<String, i64>,
}

impl LibraryView {
    /// Apply the lines written since the last call.
    pub fn catch_up(&mut self, lines: &[Line]) {
        for line in &lines[self.applied..] {
            self.apply(&line.statement);
        }
        self.applied = lines.len();
    }

    /// A player's library size, once a keyframe gave it.
    pub fn size(&self, player: &str) -> Option<i64> {
        self.sizes.get(player).copied()
    }

    pub fn apply(&mut self, statement: &Statement) {
        match statement {
            Statement::KeyframePlayer {
                player, library, ..
            } => {
                self.sizes.insert(player.clone(), i64::from(*library));
            }
            Statement::NewCard { object, owner, .. }
            | Statement::NewToken { object, owner, .. } => {
                self.owners.insert(object.id, owner.clone());
            }
            Statement::NewHidden { id, owner, .. } | Statement::NewCopy { id, owner, .. } => {
                self.owners.insert(*id, owner.clone());
            }
            Statement::Move {
                object, from, to, ..
            } => {
                self.changed(object.id, *from, -1);
                self.changed(object.id, *to, 1);
            }
            Statement::Play { object, from, .. } | Statement::Cast { object, from, .. } => {
                self.changed(object.id, *from, -1);
            }
            _ => {}
        }
    }

    fn changed(&mut self, id: u32, zone: Zone, by: i64) {
        if zone != Zone::Library {
            return;
        }
        let Some(owner) = self.owners.get(&id) else {
            return;
        };
        if let Some(size) = self.sizes.get_mut(owner) {
            *size += by;
        }
    }
}

impl GameWriter<'_> {
    /// Before the keyframe of a turn that begins in state `index`: when a
    /// library's size differs from what the record's own lines give, a
    /// change was missed (after a gap, Arena re-sends a library only when it
    /// changes again), so the record says it is incomplete (spec 9.8) and
    /// the keyframe resyncs.
    pub(super) fn check_libraries(&mut self, index: usize) {
        self.library_view.catch_up(&self.lines);
        let libraries = self.keyframe_libraries(index);
        let missed = libraries.iter().any(|(seat, size)| {
            let player = self.roster.name(*seat);
            self.library_view
                .size(&player)
                .is_some_and(|replayed| replayed != i64::from(*size))
        });
        if missed {
            self.write_incomplete(
                "Arena changed a library with no move the record saw".to_string(),
            );
        }
    }
}
