//! The cards of a pulled Endstep manual match (cross-device sync spec
//! section 5.3). An Endstep bundle names every card by ref (name and
//! printing), never by catalog id, but the manual reader (`manual_read`)
//! reads catalog ids. So each ref is numbered by its place in a list (plus
//! one), the manual reader reads those numbers, and the store maps each
//! number back to this device's own catalog id through the list
//! (`PulledMatch::endstep_manual_cards`).

use std::collections::BTreeMap;

use serde_json::Value;
use sync_bundles::EndstepCardRef;
use sync_bundles::endstep_cards::read_endstep_signature;

/// The refs numbered so far, in order: ref `n - 1` is number `n`.
#[derive(Debug, Default)]
pub(crate) struct NumberedCards {
    pub(crate) cards: Vec<EndstepCardRef>,
}

impl NumberedCards {
    /// The number of `card`, given one when new.
    fn number(&mut self, card: EndstepCardRef) -> u32 {
        let place = match self.cards.iter().position(|held| *held == card) {
            Some(place) => place,
            None => {
                self.cards.push(card);
                self.cards.len() - 1
            }
        };
        u32::try_from(place + 1).unwrap_or(u32::MAX)
    }

    /// A card value: a ref becomes its number; anything else is null (an
    /// Endstep bundle's number would be another device's catalog id).
    fn card(&mut self, value: &Value) -> Value {
        match EndstepCardRef::from_value(value) {
            Some(card) => Value::from(self.number(card)),
            None => Value::Null,
        }
    }

    /// A `deck_json` list with each row's `mtgo_id` ref numbered.
    pub(crate) fn deck_json(&mut self, value: &Value) -> Value {
        let Some(rows) = value.as_array() else {
            return value.clone();
        };
        let mut numbered = Vec::new();
        for row in rows {
            let mut row = row.clone();
            if let Some(card) = row.get("mtgo_id").map(|card| self.card(card)) {
                row["mtgo_id"] = card;
            }
            numbered.push(row);
        }
        Value::Array(numbered)
    }

    /// An `opening_hand_json` with every card of `kept`, `bottomed` and
    /// each of `mulligans` numbered.
    pub(crate) fn hand_json(&mut self, value: &Value) -> Value {
        let mut hand = value.clone();
        for key in ["kept", "bottomed"] {
            if let Some(list) = value.get(key) {
                hand[key] = self.list(list);
            }
        }
        if let Some(Value::Array(hands)) = value.get("mulligans") {
            let numbered: Vec<Value> = hands.iter().map(|list| self.list(list)).collect();
            hand["mulligans"] = Value::Array(numbered);
        }
        hand
    }

    fn list(&mut self, value: &Value) -> Value {
        match value.as_array() {
            Some(cards) => Value::Array(cards.iter().map(|card| self.card(card)).collect()),
            None => value.clone(),
        }
    }

    /// The registered maindeck of a portable Endstep signature, copies by
    /// number. Empty when the signature does not read.
    pub(crate) fn registered(&mut self, signature: &str) -> BTreeMap<u32, u32> {
        let mut copies = BTreeMap::new();
        for row in read_endstep_signature(signature).unwrap_or_default() {
            if !row.sideboard {
                *copies.entry(self.number(row.card)).or_insert(0) += row.quantity;
            }
        }
        copies
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(name: &str) -> Value {
        json!({"name": name, "set": "qqq", "number": "7"})
    }

    #[test]
    fn each_ref_gets_one_number_wherever_it_appears() {
        let mut numbered = NumberedCards::default();
        let deck = numbered.deck_json(&json!([
            {"mtgo_id": card("Qalpha"), "quantity": 4, "sideboard": false},
            {"mtgo_id": null, "quantity": 1, "sideboard": false},
            {"mtgo_id": card("Qbeta"), "quantity": 2, "sideboard": true},
        ]));
        assert_eq!(
            deck,
            json!([
                {"mtgo_id": 1, "quantity": 4, "sideboard": false},
                {"mtgo_id": null, "quantity": 1, "sideboard": false},
                {"mtgo_id": 2, "quantity": 2, "sideboard": true},
            ])
        );
        let hand = numbered.hand_json(&json!({
            "kept": [card("Qbeta")],
            "bottomed": [],
            "mulligans": [[card("Qgamma"), card("Qalpha")]],
        }));
        assert_eq!(
            hand,
            json!({"kept": [2], "bottomed": [], "mulligans": [[3, 1]]})
        );
        assert_eq!(numbered.cards.len(), 3);
        assert_eq!(numbered.cards[2].name, "Qgamma");
    }

    #[test]
    fn a_value_that_is_no_ref_is_null() {
        let mut numbered = NumberedCards::default();
        let deck = numbered.deck_json(&json!([
            {"mtgo_id": 200_000_001, "quantity": 4},
            {"mtgo_id": "12", "quantity": 1},
            {"mtgo_id": {"set": "qqq"}, "quantity": 1},
        ]));
        assert_eq!(
            deck,
            json!([
                {"mtgo_id": null, "quantity": 4},
                {"mtgo_id": null, "quantity": 1},
                {"mtgo_id": null, "quantity": 1},
            ])
        );
        let hand = numbered.hand_json(&json!({"kept": [5, card("Qalpha")]}));
        assert_eq!(hand, json!({"kept": [null, 1]}));
        assert_eq!(numbered.cards.len(), 1);
    }

    #[test]
    fn the_registered_main_counts_main_rows_by_number() {
        let mut numbered = NumberedCards::default();
        numbered.deck_json(&json!([{"mtgo_id": card("Qbeta"), "quantity": 1}]));
        let signature = sync_bundles::endstep_cards::endstep_signature(&[
            sync_bundles::endstep_cards::EndstepDeckCard {
                card: EndstepCardRef::from_value(&card("Qalpha")).expect("ref"),
                quantity: 4,
                sideboard: false,
            },
            sync_bundles::endstep_cards::EndstepDeckCard {
                card: EndstepCardRef::from_value(&card("Qbeta")).expect("ref"),
                quantity: 2,
                sideboard: true,
            },
        ]);
        assert_eq!(numbered.registered(&signature), BTreeMap::from([(2, 4)]));
        assert!(numbered.registered("MjAwOjQ6ZmFsc2U=").is_empty());
    }
}
