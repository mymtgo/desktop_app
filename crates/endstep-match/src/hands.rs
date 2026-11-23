//! Each seat's hands in one game: the hands sent back (mulligans), the
//! opening hand (the last one drawn, kept) and the cards bottomed from it.
//!
//! Fed the game's states and events in frame order. Seats stay as Endstep
//! sends them and the user's seat is applied at the end, so a match body that
//! arrives after the game frames still gives the same hands.
//!
//! The only hand ever listed by name is the user's: Forge hides the
//! opponent's, so the opponent's sizes come from its `HAND_SETTLED` event.

use std::collections::{BTreeMap, BTreeSet};

use endstep_protocol::{GameEvent, GameState, StateCard, Zone};

/// A hand card: its id in this game and its name.
pub(crate) type HandCard = (u64, String);

/// One seat's hands so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SeatHands {
    /// The named hand cards of the latest full state.
    current: Vec<HandCard>,
    sent_back: Vec<Vec<HandCard>>,
    /// Fixed by the first `HAND_KEPT` that had a hand before it.
    opening: Option<Vec<HandCard>>,
    /// `HAND_SETTLED`: (amount, mulligans), each `None` when the event left
    /// it out. Only the first one counts.
    settled: Option<(Option<u32>, Option<u32>)>,
    /// The current hand came from a state of turn 0 (or no turn), the only
    /// kind of state a keep can fix as the opening hand.
    current_from_turn_zero: bool,
    /// The first `HAND_SETTLED` came after the opening hand was fixed, so
    /// it settles that hand (not a later game's, trap 35).
    settled_after_opening: bool,
    /// A `HAND_SETTLED` came and the next full state fixes the bottomed cards.
    bottom_pending: bool,
    bottomed: Vec<HandCard>,
}

/// One game's hands, keyed by seat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HandTally {
    seats: BTreeMap<u32, SeatHands>,
}

/// What a game's frames say about its hands, from the user's seat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Hands {
    pub opening: Vec<HandCard>,
    pub bottomed: Vec<HandCard>,
    /// The hands sent back, the first one first.
    pub sent_back: Vec<Vec<HandCard>>,
    /// The `amount` of the user's `HAND_SETTLED`: the starting hand size,
    /// known even when no state shows which card was bottomed.
    pub settled_size: Option<u32>,
    pub opp_opening_size: Option<u32>,
    pub opp_starting_size: Option<u32>,
}

impl Hands {
    /// True when the opening hand is known (`opening` is never empty then).
    pub(crate) fn has_opening(&self) -> bool {
        !self.opening.is_empty()
    }
}

impl HandTally {
    /// Notes one full state. A sideboarding state is not a game state: its
    /// zones are empty, so it fixes and resets nothing.
    pub(crate) fn read_state(&mut self, state: &GameState) {
        if state.sideboarding {
            return;
        }
        let mut seats: BTreeSet<u32> = state.player_states.iter().map(|p| p.seat).collect();
        seats.extend(state.cards.iter().filter_map(|card| card.owner_seat));
        for seat in seats {
            let hands = self.seats.entry(seat).or_default();
            if hands.bottom_pending {
                hands.bottom_pending = false;
                hands.bottomed = bottomed_cards(hands, &state.cards);
            }
            hands.current = hand_of(seat, &state.cards);
            hands.current_from_turn_zero = matches!(state.turn_number, None | Some(0));
        }
    }

    /// Notes a `MULLIGAN` choice, `HAND_KEPT` or `HAND_SETTLED` event.
    pub(crate) fn read_event(&mut self, event: &GameEvent) {
        let Some(seat) = event.player_index else {
            return;
        };
        match event.kind.as_str() {
            // A choice after a state of a later turn is the next game's (its
            // sideboarding state was lost): this game's hand stays.
            "MULLIGAN" if event.amount.is_none() => {
                let hands = self.seats.entry(seat).or_default();
                if !hands.current_from_turn_zero {
                    return;
                }
                let hand = std::mem::take(&mut hands.current);
                if !hand.is_empty() {
                    hands.sent_back.push(hand);
                }
            }
            "HAND_KEPT" => {
                let hands = self.seats.entry(seat).or_default();
                if hands.opening.is_none()
                    && !hands.current.is_empty()
                    && hands.current_from_turn_zero
                {
                    hands.opening = Some(hands.current.clone());
                }
            }
            "HAND_SETTLED" => {
                let hands = self.seats.entry(seat).or_default();
                if hands.settled.is_none() {
                    hands.settled = Some((event.amount, event.mulligans));
                    hands.settled_after_opening = hands.opening.is_some();
                    hands.bottom_pending = hands.settled_after_opening;
                }
            }
            _ => {}
        }
    }

    /// The hands of the user's `seat`, and the opponent's sizes from the
    /// lowest other seat that settled.
    pub(crate) fn hands_of(&self, seat: u32) -> Hands {
        let mut out = Hands::default();
        if let Some(hands) = self.seats.get(&seat) {
            out.opening = hands.opening.clone().unwrap_or_default();
            out.bottomed = hands.bottomed.clone();
            out.sent_back = hands.sent_back.clone();
            if hands.settled_after_opening {
                out.settled_size = hands.settled.and_then(|(amount, _)| amount);
            }
        }
        if let Some(SeatHands {
            settled: Some((amount, mulligans)),
            ..
        }) = self.opponent(seat)
        {
            out.opp_starting_size = *amount;
            out.opp_opening_size = match (amount, mulligans) {
                (Some(amount), Some(mulligans)) => Some(amount.saturating_add(*mulligans)),
                _ => None,
            };
        }
        out
    }

    /// The lowest seat other than the user's `seat` that settled.
    fn opponent(&self, seat: u32) -> Option<&SeatHands> {
        self.seats
            .iter()
            .find(|(other, hands)| **other != seat && hands.settled.is_some())
            .map(|(_, hands)| hands)
    }

    /// The mulligans the hands show for the user's `seat`: the hands sent
    /// back or the `mulligans` of a `HAND_SETTLED` that settled this game's
    /// opening hand, whichever is larger (either can be missing from a
    /// capture, neither counts one too many).
    pub(crate) fn local_mulligans(&self, seat: u32) -> u32 {
        let Some(hands) = self.seats.get(&seat) else {
            return 0;
        };
        let sent_back = u32::try_from(hands.sent_back.len()).unwrap_or(u32::MAX);
        let settled = hands
            .settled
            .filter(|_| hands.settled_after_opening)
            .and_then(|(_, mulligans)| mulligans);
        sent_back.max(settled.unwrap_or(0))
    }

    /// The opponent's `HAND_SETTLED` `mulligans` (its hands are hidden, so
    /// none are sent back by name), 0 when it did not settle.
    pub(crate) fn opponent_mulligans(&self, seat: u32) -> u32 {
        self.opponent(seat)
            .and_then(|hands| hands.settled)
            .and_then(|(_, mulligans)| mulligans)
            .unwrap_or(0)
    }
}

/// The named, typed, non-token cards in `seat`'s hand.
fn hand_of(seat: u32, cards: &[StateCard]) -> Vec<HandCard> {
    cards
        .iter()
        .filter(|card| {
            card.zone == Zone::Hand
                && card.owner_seat == Some(seat)
                && card.has_types
                && !card.is_token
        })
        .map(|card| (card.id, card.name.clone()))
        .collect()
}

/// The opening hand's cards that `cards` lists in no zone, when there are as
/// many as the seat's mulligans (else none: a card that left the hand another
/// way must not read as bottomed).
fn bottomed_cards(hands: &SeatHands, cards: &[StateCard]) -> Vec<HandCard> {
    let (Some(opening), Some((_, Some(mulligans)))) = (&hands.opening, hands.settled) else {
        return Vec::new();
    };
    let listed: BTreeSet<u64> = cards.iter().map(|card| card.id).collect();
    let gone: Vec<HandCard> = opening
        .iter()
        .filter(|(id, _)| !listed.contains(id))
        .cloned()
        .collect();
    if u32::try_from(gone.len()) == Ok(mulligans) {
        gone
    } else {
        Vec::new()
    }
}
