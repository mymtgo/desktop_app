//! Draws and searches: the card a draw gave, found by looking ahead.

use std::collections::BTreeSet;

use ::mtgr::{Statement, Zone};
use endstep_protocol::GameEvent;

use super::super::GameWriter;
use super::super::board::How;
use super::super::shown::shown_cards;

/// A `CARD_ZONE_CHANGE` with no card: `seat` drew a card the viewer cannot
/// see (Forge names no card on the user's own draws either).
fn is_unnamed_draw(event: &GameEvent, seat: u32) -> bool {
    event.kind == "CARD_ZONE_CHANGE"
        && event.card_id.is_none()
        && event.from_zone.as_deref() == Some("Library")
        && event.to_zone.as_deref() == Some("Hand")
        && event.zone_owner == Some(seat)
}

/// A reveal of one card into a hand: a search shows what it found.
pub(super) fn names_a_found_card(event: &GameEvent) -> bool {
    event.kind == "CARD_REVEALED"
        && event.to_zone.as_deref() == Some("Hand")
        && event.card_ids.len() == 1
}

impl<'a> GameWriter<'a> {
    /// A card went from `seat`'s library to their hand unnamed: a card a
    /// search found (the reveal that follows names it), the user's draw
    /// (the next full state names it), or a hidden draw.
    pub(super) fn read_unnamed_draw(&mut self, seat: u32) {
        if let Some((forge_id, name)) = self.found_ahead(seat) {
            self.write_found(forge_id, &name, seat);
        } else if seat == self.user_seat {
            self.write_user_draw(seat);
        } else {
            self.write_hidden_draw(seat);
        }
    }

    /// The card the very next event reveals into `seat`'s hand.
    pub(super) fn found_ahead(&self, seat: u32) -> Option<(u64, String)> {
        let ahead = self.events_ahead();
        let next = ahead.first().filter(|event| names_a_found_card(event))?;
        let forge_id = *next.card_ids.first()?;
        let name = next.card_names.first().filter(|name| !name.is_empty())?;
        (self.owner_of_revealed(forge_id, next) == seat).then(|| (forge_id, name.clone()))
    }

    /// A card found by a search: a move with no action, caused by what
    /// resolves.
    pub(super) fn write_found(&mut self, forge_id: u64, name: &str, seat: u32) {
        self.board.hints.found.insert(forge_id);
        let mut out = Vec::new();
        let found = self
            .board
            .object_of_event(&mut out, forge_id, name, seat, Zone::Library);
        let Some(id) = found else {
            return;
        };
        let cause = self.effect_cause();
        self.board
            .move_object(&mut out, id, Zone::Hand, How::Plain, None, cause);
        self.events(out);
    }

    pub(super) fn write_user_draw(&mut self, seat: u32) {
        let Some((forge_id, name)) = self.drawn_ahead(seat) else {
            return;
        };
        let mut out = Vec::new();
        let found = self
            .board
            .object_of_event(&mut out, forge_id, &name, seat, Zone::Library);
        let Some(id) = found else {
            return;
        };
        let cause = self.draw_cause();
        self.board
            .move_object(&mut out, id, Zone::Hand, How::Draw, None, cause);
        self.events(out);
    }

    /// The card this draw gave `seat`: the first card of the next full
    /// state's hand that the record does not have in hand. Only when that
    /// state shows exactly as many such cards as draws are left before it,
    /// else which card is which is not known and the diff writes them.
    pub(super) fn drawn_ahead(&self, seat: u32) -> Option<(u64, String)> {
        let state = self.next_state()?;
        let ahead = self.events_ahead();
        let mut draws = 1;
        let mut found: BTreeSet<u64> = BTreeSet::new();
        for (position, event) in ahead.iter().enumerate() {
            if names_a_found_card(event) {
                found.extend(event.card_ids.iter().copied());
            }
            let finds = ahead
                .get(position + 1)
                .is_some_and(|next| names_a_found_card(next));
            if is_unnamed_draw(event, seat) && !finds {
                draws += 1;
            }
        }
        let objects = &self.board.objects;
        let mut new_in_hand = Vec::new();
        for card in shown_cards(state) {
            if card.owner != seat || card.zone != Zone::Hand || card.token {
                continue;
            }
            if found.contains(&card.forge_id) {
                continue;
            }
            let zone = objects
                .id_of(card.forge_id)
                .and_then(|id| objects.get(id))
                .map(|object| object.zone);
            if matches!(zone, None | Some(Zone::Library)) {
                new_in_hand.push((card.forge_id, card.name));
            }
        }
        if new_in_hand.len() != draws {
            return None;
        }
        new_in_hand.into_iter().next()
    }

    pub(super) fn write_hidden_draw(&mut self, seat: u32) {
        let id = self
            .board
            .objects
            .add(None, None, seat, Zone::Library, false);
        let hidden = Statement::NewHidden {
            id,
            owner: self.board.name(seat),
            zone: Zone::Library,
            position: None,
        };
        let mut out = vec![(hidden, None)];
        let cause = self.draw_cause();
        self.board
            .move_object(&mut out, id, Zone::Hand, How::Draw, None, cause);
        self.events(out);
    }
}
