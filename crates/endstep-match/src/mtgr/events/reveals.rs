//! `CARD_REVEALED` and `SHUFFLE`.

use ::mtgr::{Statement, Zone};
use endstep_protocol::GameEvent;

use super::super::GameWriter;
use super::{identity::zone_named, seat_of};

impl<'a> GameWriter<'a> {
    /// `CARD_REVEALED`: each card is revealed where it is, or looked at by
    /// the viewer when it is in a library. A card the record has not seen
    /// is named or introduced first. Also read in the pregame.
    pub(in crate::mtgr) fn read_reveal(&mut self, event: &GameEvent) {
        let Some(zone) = event.to_zone.as_deref().and_then(zone_named) else {
            return;
        };
        // Only a hand or a library hides a card. A reveal in any other zone
        // (none has been captured) is of a card that is face up there and
        // named by the states already, so there is nothing to tell a reader.
        if zone != Zone::Hand && zone != Zone::Library {
            return;
        }
        let viewer = seat_of(event.player_index).unwrap_or(self.user_seat);
        let cause = self.effect_cause();
        for (forge_id, name) in event.card_ids.iter().zip(&event.card_names) {
            if name.is_empty() {
                continue;
            }
            if zone == Zone::Hand {
                self.board.hints.found.insert(*forge_id);
            }
            let owner = self.owner_of_revealed(*forge_id, event);
            let mut out = Vec::new();
            let found = self
                .board
                .object_of_event(&mut out, *forge_id, name, owner, zone);
            let Some(id) = found else {
                continue;
            };
            self.events(out);
            let object = self.board.obj(id);
            let statement = if zone == Zone::Library {
                Statement::Look {
                    object,
                    player: self.board.name(viewer),
                }
            } else {
                Statement::Reveal { object }
            };
            self.event(statement, Some(cause.clone()));
        }
    }

    /// `SHUFFLE` names its player only in its text. A shuffle ends the id
    /// of every object in that library.
    pub(super) fn read_shuffle(&mut self, event: &GameEvent) {
        let name = event
            .message
            .as_deref()
            .and_then(|message| message.strip_suffix(" shuffled their library"));
        let Some(seat) = name.and_then(|name| self.player_named(name)) else {
            return;
        };
        let player = self.board.name(seat);
        let cause = self.effect_cause();
        self.event(Statement::Shuffle { player }, Some(cause));
        self.board.objects.shuffle(seat);
    }
}
