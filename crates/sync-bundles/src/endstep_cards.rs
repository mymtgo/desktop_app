//! Portable Endstep card references (cross-device sync spec section 5.3).
//!
//! An Endstep catalog id (`ENDSTEP_ID_OFFSET` plus an `endstep_cards` row
//! id) means something only on the device that made the row. An Endstep
//! bundle therefore names each card the way Endstep does, by its name and
//! printing, and the receiving device maps it to its own row.

use serde_json::{Value, json};

use crate::canonical::canonical_json;

/// The start of a portable Endstep deck version signature. A local
/// signature is base64, which never holds `:`, so the two never mix up.
pub const ENDSTEP_SIGNATURE_PREFIX: &str = "endstep:";

/// One Endstep card printing: its name as Endstep shows it (the front
/// face), its set code (lower case) and its collector number. Set and
/// number are empty when the printing is unknown.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndstepCardRef {
    pub name: String,
    pub set: String,
    pub number: String,
}

impl EndstepCardRef {
    /// The ref as bundle JSON: `{"name", "number", "set"}`, written where an
    /// MTGO or Arena bundle writes the catalog id.
    pub fn to_value(&self) -> Value {
        json!({ "name": self.name, "set": self.set, "number": self.number })
    }

    /// A ref read back from bundle JSON: an object with a non-empty text
    /// `name`; a missing `set` or `number` reads as empty. Anything else
    /// (a catalog id number, say) is `None`.
    pub fn from_value(value: &Value) -> Option<EndstepCardRef> {
        let name = value.get("name")?.as_str()?;
        if name.is_empty() {
            return None;
        }
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Some(EndstepCardRef {
            name: name.to_string(),
            set: text("set"),
            number: text("number"),
        })
    }
}

/// One row of an Endstep deck version, its card named by ref (a local
/// signature row, `decks::DeckCard`, names it by catalog id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepDeckCard {
    pub card: EndstepCardRef,
    pub quantity: u32,
    pub sideboard: bool,
}

/// An Endstep deck version's signature as an Endstep bundle writes it in
/// `deck_version_signature`: `endstep:` and the canonical JSON list of the
/// rows (`name`, `number`, `quantity`, `set`, `sideboard`), sorted by card
/// ref then main before side, so every device writes the same text for
/// the same list. The receiver reads it back with `read_endstep_signature`,
/// maps each ref to its own catalog id and encodes its local signature
/// (`decks::signature`) from those.
pub fn endstep_signature(cards: &[EndstepDeckCard]) -> String {
    let mut sorted = cards.to_vec();
    sorted.sort_by(|a, b| (&a.card, a.sideboard).cmp(&(&b.card, b.sideboard)));
    let rows: Vec<Value> = sorted
        .iter()
        .map(|row| {
            json!({
                "name": row.card.name,
                "set": row.card.set,
                "number": row.card.number,
                "quantity": row.quantity,
                "sideboard": row.sideboard,
            })
        })
        .collect();
    format!(
        "{ENDSTEP_SIGNATURE_PREFIX}{}",
        canonical_json(&Value::Array(rows))
    )
}

/// The rows of a portable Endstep signature, in its order. `None` for any
/// other text (a local base64 signature, say) or a row that does not read
/// (no name, or a quantity that is not a whole number).
pub fn read_endstep_signature(text: &str) -> Option<Vec<EndstepDeckCard>> {
    let list = text.strip_prefix(ENDSTEP_SIGNATURE_PREFIX)?;
    let value: Value = serde_json::from_str(list).ok()?;
    let mut cards = Vec::new();
    for row in value.as_array()? {
        let quantity = u32::try_from(row.get("quantity")?.as_u64()?).ok()?;
        cards.push(EndstepDeckCard {
            card: EndstepCardRef::from_value(row)?,
            quantity,
            sideboard: row.get("sideboard").and_then(Value::as_bool)?,
        });
    }
    Some(cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bolt() -> EndstepCardRef {
        EndstepCardRef {
            name: "Lightning Bolt".to_string(),
            set: "m10".to_string(),
            number: "146".to_string(),
        }
    }

    #[test]
    fn a_ref_round_trips_through_bundle_json() {
        let value = bolt().to_value();
        assert_eq!(
            canonical_json(&value),
            r#"{"name":"Lightning Bolt","number":"146","set":"m10"}"#
        );
        assert_eq!(EndstepCardRef::from_value(&value), Some(bolt()));
    }

    #[test]
    fn an_unknown_printing_is_empty_text() {
        let value = json!({ "name": "Lightning Bolt" });
        assert_eq!(
            EndstepCardRef::from_value(&value),
            Some(EndstepCardRef {
                name: "Lightning Bolt".to_string(),
                set: String::new(),
                number: String::new(),
            })
        );
    }

    fn deck_card(name: &str, quantity: u32, sideboard: bool) -> EndstepDeckCard {
        EndstepDeckCard {
            card: EndstepCardRef {
                name: name.to_string(),
                set: "m10".to_string(),
                number: "1".to_string(),
            },
            quantity,
            sideboard,
        }
    }

    #[test]
    fn a_portable_signature_round_trips_in_one_order() {
        let cards = vec![
            deck_card("Qzeta", 4, false),
            deck_card("Qalpha", 1, true),
            deck_card("Qalpha", 3, false),
        ];
        let signature = endstep_signature(&cards);
        assert_eq!(
            signature,
            "endstep:[{\"name\":\"Qalpha\",\"number\":\"1\",\"quantity\":3,\"set\":\"m10\",\"sideboard\":false},\
             {\"name\":\"Qalpha\",\"number\":\"1\",\"quantity\":1,\"set\":\"m10\",\"sideboard\":true},\
             {\"name\":\"Qzeta\",\"number\":\"1\",\"quantity\":4,\"set\":\"m10\",\"sideboard\":false}]"
        );
        let mut reversed = cards.clone();
        reversed.reverse();
        assert_eq!(
            endstep_signature(&reversed),
            signature,
            "input order never matters"
        );
        assert_eq!(
            read_endstep_signature(&signature),
            Some(vec![
                deck_card("Qalpha", 3, false),
                deck_card("Qalpha", 1, true),
                deck_card("Qzeta", 4, false),
            ])
        );
        assert_eq!(endstep_signature(&[]), "endstep:[]");
        assert_eq!(read_endstep_signature("endstep:[]"), Some(Vec::new()));
    }

    #[test]
    fn a_local_signature_is_not_a_portable_one() {
        // Base64 of `200000001:4:false`: a local signature.
        assert_eq!(read_endstep_signature("MjAwMDAwMDAxOjQ6ZmFsc2U="), None);
        assert_eq!(read_endstep_signature("endstep:{}"), None);
        assert_eq!(read_endstep_signature("endstep:[{\"quantity\":1}]"), None);
    }

    #[test]
    fn a_number_or_a_nameless_object_is_no_ref() {
        assert_eq!(EndstepCardRef::from_value(&json!(200_000_001)), None);
        assert_eq!(EndstepCardRef::from_value(&json!({ "name": "" })), None);
        assert_eq!(EndstepCardRef::from_value(&json!({ "set": "m10" })), None);
    }
}
