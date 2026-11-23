//! Combat, damage and life from the game events.
//!
//! - `ATTACKERS_DECLARED` and `BLOCKERS_DECLARED` write `attack` and `block`
//!   for the creatures the record has on the battlefield.
//! - `PLAYER_DAMAGED` and `CARD_DAMAGED` write `damage` when the event has
//!   an amount and the record has the source.
//! - `PLAYER_LIFE_CHANGED` writes `life`, with the cause of the damage that
//!   took it. (A life total the events never told is written by the diff.)

use ::mtgr::{Cause, Statement, Step, TargetRef, Zone};
use endstep_protocol::GameEvent;

use super::events::seat_of;
use super::{GameWriter, other_seat};

impl GameWriter<'_> {
    /// The record's object for a Forge id, when it is on the battlefield.
    fn permanent_id(&self, forge_id: u64) -> Option<u32> {
        let objects = &self.board.objects;
        let id = objects.id_of(forge_id)?;
        let object = objects.get(id)?;
        (object.zone == Zone::Battlefield).then_some(id)
    }

    /// `ATTACKERS_DECLARED`: each attacker attacks the other player (the
    /// event does not say when a planeswalker is attacked).
    pub(super) fn read_attackers(&mut self, event: &GameEvent) {
        let Some(seat) = seat_of(event.player_index) else {
            return;
        };
        let defender = self.board.name(other_seat(seat));
        for forge_id in &event.attacker_ids {
            let Some(id) = self.permanent_id(*forge_id) else {
                continue;
            };
            if !self.attackers.insert(id) {
                continue;
            }
            let statement = Statement::Attack {
                object: self.board.obj(id),
                target: TargetRef::Player(defender.clone()),
            };
            self.event(statement, None);
        }
    }

    /// `BLOCKERS_DECLARED`: each blocker blocks an attacker of this turn.
    pub(super) fn read_blockers(&mut self, event: &GameEvent) {
        for (blocker, attacker) in &event.blockers {
            let (Some(blocker), Some(attacker)) =
                (self.permanent_id(*blocker), self.permanent_id(*attacker))
            else {
                continue;
            };
            if !self.attackers.contains(&attacker) {
                continue;
            }
            self.blockers.insert(blocker);
            let statement = Statement::Block {
                object: self.board.obj(blocker),
                attacker: self.board.obj(attacker),
            };
            self.event(statement, None);
        }
    }

    /// The cause of damage from `source`: combat damage from a creature
    /// that attacked or blocked, in a combat damage step; else what
    /// resolves; else the source itself.
    fn damage_cause(&self, source: u32) -> Cause {
        let fought = self.attackers.contains(&source) || self.blockers.contains(&source);
        if self.step == Some(Step::CombatDamage) && fought {
            return Cause::Rule("combat-damage".to_string());
        }
        match self.effect_cause() {
            Cause::Unknown => Cause::Object(source),
            cause => cause,
        }
    }

    /// The live object that dealt the damage and the amount, when the event
    /// has both.
    fn damage_source(&self, event: &GameEvent) -> Option<(u32, u32)> {
        let source = self.board.objects.id_of(event.card_id?)?;
        let amount = event.amount.filter(|amount| *amount > 0)?;
        Some((source, amount))
    }

    pub(super) fn read_player_damage(&mut self, event: &GameEvent) {
        let (Some((source, amount)), Some(seat)) =
            (self.damage_source(event), seat_of(event.player_index))
        else {
            return;
        };
        let cause = self.damage_cause(source);
        self.batch.damaged_by.insert(seat, cause.clone());
        let statement = Statement::Damage {
            object: self.board.obj(source),
            target: TargetRef::Player(self.board.name(seat)),
            amount,
        };
        self.event(statement, Some(cause));
    }

    pub(super) fn read_card_damage(&mut self, event: &GameEvent) {
        let target = event
            .target_card_id
            .and_then(|forge_id| self.board.objects.id_of(forge_id));
        let (Some((source, amount)), Some(target)) = (self.damage_source(event), target) else {
            return;
        };
        let statement = Statement::Damage {
            object: self.board.obj(source),
            target: TargetRef::Object(self.board.obj(target)),
            amount,
        };
        let cause = self.damage_cause(source);
        self.event(statement, Some(cause));
    }

    /// `PLAYER_LIFE_CHANGED`: the life total, from the record's last one.
    /// The cause is that of the damage that took it (used once), else what
    /// resolves.
    pub(super) fn read_life(&mut self, event: &GameEvent) {
        let (Some(seat), Some(life)) = (seat_of(event.player_index), event.new_value) else {
            return;
        };
        let Some((statement, _)) = self.board.set_life(seat, life) else {
            return;
        };
        let cause = match self.batch.damaged_by.remove(&seat) {
            Some(cause) => cause,
            None => self.effect_cause(),
        };
        self.event(statement, Some(cause));
    }
}
