//! What the game events add to a turn, each at its place in frame order:
//! casts, plays, abilities, combat, damage, life, reveals, shuffles, and the
//! moves an event names, with their action and cause.
//!
//! **Events and the diff.** A cast is written at `SPELL_CAST`, not at the
//! card's move onto the stack before it: that move is only the start of a
//! cast, which Forge takes back silently when the player cancels. An event
//! that moves a card applies the move to the `Board` as it writes it, so the next full state's diff (`board.rs`)
//! finds the card where the state shows it and writes nothing. An event the
//! record cannot place (a card it has no object for, or one that is not in
//! the zone the event names) writes nothing and is left to the diff; for
//! those the event leaves `Hints`.
//!
//! **Looking ahead.** Events arrive before the full state that shows their
//! result, and some need it: a permanent's types as it enters, the text of
//! an ability, the card a draw gave the user. Those are read from the next
//! full state in the game's frames.
//!
//! **Resolving.** Forge sends what a spell or ability does before the
//! `SPELL_RESOLVED` that names it. When one is coming before the next full
//! state for the object on top of the stack, `resolve` is written first and
//! the events up to it take that object as their cause (`stack.rs`, with
//! abilities and targets). Combat, damage and life are in `combat.rs`.
//!
//! Never written (the `Omits` and `Partial` tags say so): mana, costs,
//! priority, lasting effects, the cards a scry or surveil looked at.

mod ahead;
mod draws;
mod identity;
mod reveals;
mod zones;

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{AbilityKind, Cause, Step};
use endstep_protocol::GameEvent;

use super::GameWriter;

pub(super) use ahead::Ahead;
pub(super) use identity::{CardNames, card_names, owners, seat_of};

/// What the events since the last full state established.
#[derive(Debug, Default)]
pub(super) struct Batch {
    pub(super) resolving: Option<Resolving>,
    /// Objects put on the stack since the last full state: `resolve` is not
    /// written ahead of its event for them.
    pub(super) new_on_stack: BTreeSet<u32>,
    /// The cause of the last damage dealt to each seat: the life it took
    /// has the same cause.
    pub(super) damaged_by: BTreeMap<u32, Cause>,
}

/// The spell or ability that is resolving.
#[derive(Debug)]
pub(super) struct Resolving {
    pub(super) id: u32,
    /// Its text says it counters a spell.
    pub(super) counters: bool,
    /// Its `SPELL_RESOLVED` came: what follows is no longer its doing, but
    /// a spell's own card still leaves the stack by `rule:resolve`.
    pub(super) done: bool,
}

impl<'a> GameWriter<'a> {
    /// An event of a turn that is not a turn or step line.
    pub(super) fn read_turn_event(&mut self, event: &GameEvent) {
        if event.kind != "SPELL_RESOLVED" {
            self.resolve_ahead();
        }
        match event.kind.as_str() {
            "CARD_ZONE_CHANGE" => self.read_zone_change(event),
            "SPELL_CAST" => self.read_spell_cast(event),
            "SPELL_RESOLVED" => self.read_resolved(event),
            "ABILITY_ACTIVATED" => self.read_ability(event, AbilityKind::Activated),
            "TRIGGER_FIRED" => self.read_ability(event, AbilityKind::Triggered),
            "ATTACKERS_DECLARED" => self.read_attackers(event),
            "BLOCKERS_DECLARED" => self.read_blockers(event),
            "PLAYER_DAMAGED" => self.read_player_damage(event),
            "CARD_DAMAGED" => self.read_card_damage(event),
            "PLAYER_LIFE_CHANGED" => self.read_life(event),
            "CARD_REVEALED" => self.read_reveal(event),
            "SHUFFLE" => self.read_shuffle(event),
            "TOKEN_CREATED" => self.board.hints.token = true,
            _ => {}
        }
    }

    /// The spell or ability whose resolution is under way, else unknown.
    pub(super) fn effect_cause(&self) -> Cause {
        match &self.batch.resolving {
            Some(resolving) if !resolving.done => Cause::Object(resolving.id),
            _ => Cause::Unknown,
        }
    }

    /// Why a card is drawn now: the draw step's rule, else what resolves.
    fn draw_cause(&self) -> Cause {
        if self.step == Some(Step::Draw) {
            Cause::Rule("draw".to_string())
        } else {
            self.effect_cause()
        }
    }
}
