//! `CARD_ZONE_CHANGE` and `SPELL_CAST`: the moves an event names, with
//! their action and cause.

use ::mtgr::{Cause, Statement, Zone};
use endstep_protocol::GameEvent;

use super::super::GameWriter;
use super::super::board::{How, entering};
use super::super::objects::Permanent;
use super::super::shown::shown_cards;
use super::{identity::zone_named, seat_of};

impl<'a> GameWriter<'a> {
    pub(super) fn read_zone_change(&mut self, event: &GameEvent) {
        let from = event.from_zone.as_deref().and_then(zone_named);
        let to = event.to_zone.as_deref().and_then(zone_named);
        // A created object has no `fromZone`.
        if let (None, Some(Zone::Battlefield), Some(forge_id)) =
            (&event.from_zone, to, event.card_id)
        {
            self.write_token(forge_id);
            return;
        }
        let (Some(from), Some(to), Some(seat)) = (from, to, seat_of(event.zone_owner)) else {
            return;
        };
        let Some(forge_id) = event.card_id else {
            if (from, to) == (Zone::Library, Zone::Hand) {
                self.read_unnamed_draw(seat);
            }
            return;
        };
        let Some(name) = event.card_name.as_deref().filter(|name| !name.is_empty()) else {
            return;
        };
        if (from, to) == (Zone::Hand, Zone::Graveyard) {
            self.board.hints.discarded.insert(forge_id);
        }
        let owner = self.owner_of(forge_id, seat);
        self.move_named(forge_id, name, owner, from, to);
    }

    /// A token is new at the event that creates it, as the next full state
    /// shows it, caused by what is resolving. A created object that state
    /// does not show as a token on the battlefield (a Forge effect, a token
    /// already gone) writes nothing.
    pub(super) fn write_token(&mut self, forge_id: u64) {
        if self.board.objects.id_of(forge_id).is_some() {
            return;
        }
        let token = self.next_state().and_then(|state| {
            shown_cards(state).into_iter().find(|card| {
                card.forge_id == forge_id && card.zone == Zone::Battlefield && card.token
            })
        });
        let Some(token) = token else {
            return;
        };
        let cause = self.effect_cause();
        let mut out = Vec::new();
        self.board.introduce_token(&token, cause, &mut out);
        self.events(out);
    }

    /// A card an event names moves from `from` to `to`: a spell leaving the
    /// stack, a land played, or a plain move. A move onto the stack is only
    /// the start of a cast (the cost is not paid yet, and the cast may be
    /// taken back with no further event): the card is named and stays where
    /// it is until `SPELL_CAST`.
    pub(super) fn move_named(
        &mut self,
        forge_id: u64,
        name: &str,
        owner: u32,
        from: Zone,
        to: Zone,
    ) {
        // A permanent enters as the next full state shows it. One that is
        // gone again by then is left to the diff.
        let permanent = if to == Zone::Battlefield {
            match self.permanent_ahead(forge_id) {
                Some(permanent) => Some(permanent),
                None => return,
            }
        } else {
            None
        };
        let mut out = Vec::new();
        let found = self
            .board
            .object_of_event(&mut out, forge_id, name, owner, from);
        let Some(id) = found else {
            return;
        };
        self.events(out);
        if to == Zone::Stack {
            return;
        }
        if from == Zone::Stack {
            self.leave_stack(id, to, permanent.as_ref());
            return;
        }
        if let (Zone::Hand, Some(permanent)) = (from, &permanent)
            && self.is_land_play(forge_id)
        {
            self.write_play(id, permanent);
            return;
        }
        let (how, cause) = self.how_moved(forge_id, from, to);
        self.write_move(id, to, how, permanent.as_ref(), cause);
    }

    pub(super) fn write_move(
        &mut self,
        id: u32,
        to: Zone,
        how: How,
        permanent: Option<&Permanent>,
        cause: Cause,
    ) {
        let mut out = Vec::new();
        self.board
            .move_object(&mut out, id, to, how, permanent, cause);
        self.events(out);
    }

    /// The action and cause of a named move. Out of the library into the
    /// hand it is a draw, unless a reveal names the card next (a search
    /// found it).
    pub(super) fn how_moved(&self, forge_id: u64, from: Zone, to: Zone) -> (How, Cause) {
        match (from, to) {
            (Zone::Library, Zone::Hand) if self.revealed_ahead(forge_id) => {
                (How::Plain, self.effect_cause())
            }
            (Zone::Library, Zone::Hand) => (How::Draw, self.draw_cause()),
            (Zone::Library, Zone::Graveyard) => (How::Mill, self.effect_cause()),
            (Zone::Hand, Zone::Graveyard) => (How::Discard, self.effect_cause()),
            _ => (How::Plain, self.effect_cause()),
        }
    }

    /// `cast`: the object goes onto the stack from where the record has it.
    /// A spell cast while another resolves (a cascade) is cast by it. A card
    /// cast again (flashback, or after a bounce) has its targets and text
    /// read anew.
    pub(super) fn write_cast(&mut self, id: u32) {
        let Some(object) = self.board.objects.get(id) else {
            return;
        };
        self.targets_read.remove(&id);
        self.targeted.remove(&id);
        self.spell_texts.remove(&id);
        let statement = Statement::Cast {
            object: object.obj(),
            from: object.zone,
            face: None,
        };
        let cause = match self.effect_cause() {
            Cause::Unknown => None,
            cause => Some(cause),
        };
        self.event(statement, cause);
        self.board.objects.relocate(id, Zone::Stack, false, false);
        self.batch.new_on_stack.insert(id);
    }

    /// `play`: a land goes onto the battlefield.
    pub(super) fn write_play(&mut self, id: u32, permanent: &Permanent) {
        let Some(object) = self.board.objects.get(id) else {
            return;
        };
        let statement = Statement::Play {
            object: object.obj(),
            from: object.zone,
            entering: entering(permanent),
        };
        self.event(statement, None);
        self.board
            .objects
            .relocate(id, Zone::Battlefield, false, false);
        self.board.objects.enter(id, permanent);
    }

    /// A spell's card leaves the stack: because it resolved, because what
    /// is resolving countered or moved it, or because it fizzled.
    pub(super) fn leave_stack(&mut self, id: u32, to: Zone, permanent: Option<&Permanent>) {
        let object = self.board.obj(id);
        let resolving = self
            .batch
            .resolving
            .as_ref()
            .map(|resolving| (resolving.id, resolving.done, resolving.counters));
        let cause = match resolving {
            Some((resolved, _, _)) if resolved == id => Cause::Rule("resolve".to_string()),
            Some((other, false, counters)) => {
                let by = Cause::Object(other);
                if counters && to != Zone::Battlefield {
                    self.event(Statement::Countered { object }, Some(by.clone()));
                }
                by
            }
            _ => self.fizzle_cause(id, to),
        };
        self.write_move(id, to, How::Plain, permanent, cause);
    }

    /// A spell with targets that goes to the graveyard from the top of the
    /// stack with nothing resolving fizzled (CR 608.2b). Otherwise why it
    /// left is unknown.
    pub(super) fn fizzle_cause(&mut self, id: u32, to: Zone) -> Cause {
        let on_top = self.board.objects.top_of_stack() == Some(id);
        if to != Zone::Graveyard || !on_top || !self.targeted.contains(&id) {
            return Cause::Unknown;
        }
        let object = self.board.obj(id);
        self.event(Statement::Fizzle { object }, None);
        Cause::Rule("fizzle".to_string())
    }

    /// `SPELL_CAST`: the cost is paid and the spell is cast, from the zone
    /// the record has the card in. An opponent card the record has not seen
    /// was a hidden hand card.
    pub(super) fn read_spell_cast(&mut self, event: &GameEvent) {
        let Some(forge_id) = event.card_id else {
            return;
        };
        if let Some(id) = self.board.objects.id_of(forge_id) {
            let zone = self.board.objects.get(id).map(|object| object.zone);
            if !matches!(zone, Some(Zone::Stack | Zone::Battlefield) | None) {
                self.write_cast(id);
            }
            return;
        }
        let name = event.card_name.as_deref().filter(|name| !name.is_empty());
        let (Some(name), Some(caster)) = (name, seat_of(event.player_index)) else {
            return;
        };
        let owner = self.owner_of(forge_id, caster);
        let mut out = Vec::new();
        let found = self
            .board
            .object_of_event(&mut out, forge_id, name, owner, Zone::Hand);
        if let Some(id) = found {
            self.events(out);
            self.write_cast(id);
        }
    }
}
