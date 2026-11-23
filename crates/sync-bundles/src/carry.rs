//! The carry (07 spec section 10, ruling 5): the JSON of the bundle parts
//! v1 has no home for, kept from the last pulled or fetched bundle of a row
//! (`synced_rows.carry`) and put back into every rebuild, so v1 never
//! uploads a bundle that drops another device's data.
//!
//! A carry maps a JSON pointer (`/league/deck_change_detected`) to the value
//! found there. Since #9 the deck archetype, sideboard guides, matchup notes
//! and match archetypes are built from rows (archetypes spec section 14), so
//! only the league carry is left; a carry stored with their old pointers is
//! read without them (`Carry::from_json`).

use serde_json::{Map, Value};

/// The deck bundle's parts without a v1 home: none (#9 gave the archetype,
/// guides and notes their rows).
pub const DECK_CARRY: &[&str] = &[];
/// The keys of a deck's stored carry (`synced_rows.carry`) that hold the
/// pulled guides and notes this device could not place as rows: their
/// archetype uuid is unknown and the deck has no constructed format to stub
/// it in (a limited or blank-format deck, final review F17). They are not
/// bundle pointers, so `Carry` never reads or applies them; the store adds
/// them back to the deck's rebuilt guides and notes, so the next push never
/// deletes them on the server.
pub const UNPLACED_GUIDES: &str = "unplaced_guides";
pub const UNPLACED_NOTES: &str = "unplaced_notes";
/// The same for a pulled deck's `archetype_uuid` (a text), sent back while
/// the deck has no archetype of its own.
pub const UNPLACED_ARCHETYPE_UUID: &str = "unplaced_archetype_uuid";
/// The same for a pulled match's archetype rows (`[{uuid, confidence,
/// manual, opponent}]`, a limited match), sent back for every seat without
/// a row of its own.
pub const UNPLACED_ARCHETYPES: &str = "unplaced_archetypes";
/// The match bundle's parts without a v1 home: none (#9 gave the
/// archetypes their rows).
pub const MATCH_CARRY: &[&str] = &[];
/// The league bundle's `deck_change_detected` (the column is dropped,
/// target schema section 8).
pub const LEAGUE_CARRY: &[&str] = &["/league/deck_change_detected"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carry {
    parts: Map<String, Value>,
}

impl Carry {
    /// The parts at `pointers` of a pulled bundle (a part it does not have
    /// is not carried).
    pub fn from_bundle(bundle: &Value, pointers: &[&str]) -> Carry {
        let mut parts = Map::new();
        for pointer in pointers {
            if let Some(found) = bundle.pointer(pointer) {
                parts.insert((*pointer).to_string(), found.clone());
            }
        }
        Carry { parts }
    }

    /// Put the carried parts into a rebuilt bundle, replacing what the
    /// build wrote there. A part whose parent is missing is left out.
    pub fn apply(&self, bundle: &mut Value) {
        for (pointer, part) in &self.parts {
            if let Some(slot) = bundle.pointer_mut(pointer) {
                *slot = part.clone();
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The stored form (`synced_rows.carry`).
    pub fn to_json(&self) -> Value {
        Value::Object(self.parts.clone())
    }

    /// A stored carry, keeping only the parts at `pointers` (a part stored
    /// before its rows took it over is ignored); `None` when it is not a
    /// map.
    pub fn from_json(stored: &Value, pointers: &[&str]) -> Option<Carry> {
        let stored = stored.as_object()?;
        let mut parts = Map::new();
        for (pointer, part) in stored {
            if pointers.contains(&pointer.as_str()) {
                parts.insert(pointer.clone(), part.clone());
            }
        }
        Some(Carry { parts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_carry_takes_the_parts_without_a_home_and_puts_them_back() {
        let pulled = json!({"league": {"token": "q", "deck_change_detected": true}, "matches": []});
        let carry = Carry::from_bundle(&pulled, LEAGUE_CARRY);
        assert_eq!(
            carry.to_json(),
            json!({"/league/deck_change_detected": true})
        );
        let mut rebuilt =
            json!({"league": {"token": "q", "deck_change_detected": false}, "matches": []});
        carry.apply(&mut rebuilt);
        assert_eq!(rebuilt, pulled);
        assert_eq!(
            Carry::from_json(&carry.to_json(), LEAGUE_CARRY),
            Some(carry.clone())
        );
        assert_eq!(Carry::from_json(&json!(["not a map"]), LEAGUE_CARRY), None);
    }

    #[test]
    fn a_missing_part_is_not_carried_and_an_empty_carry_changes_nothing() {
        let carry = Carry::from_bundle(&json!({"league": {}}), LEAGUE_CARRY);
        assert!(carry.is_empty());
        let mut bundle = json!({"league": {"deck_change_detected": false}});
        let before = bundle.clone();
        carry.apply(&mut bundle);
        assert_eq!(bundle, before);
    }

    // Archetypes spec section 14: the deck archetype, guides, notes and the
    // match archetypes are built from rows, so only the league carry is
    // left.
    #[test]
    fn only_the_league_carry_names_parts() {
        assert!(DECK_CARRY.is_empty());
        assert!(MATCH_CARRY.is_empty());
        assert_eq!(LEAGUE_CARRY, ["/league/deck_change_detected"]);
        let pulled = json!({
            "deck": {"mtgo_id": "1", "archetype_uuid": "q-arch"},
            "guides": [{"archetype_uuid": "q-a", "cards": []}],
            "notes": [],
            "archetypes": [{"uuid": "q-b"}],
        });
        assert!(Carry::from_bundle(&pulled, DECK_CARRY).is_empty());
        assert!(Carry::from_bundle(&pulled, MATCH_CARRY).is_empty());
    }

    // A carry stored before the rows took these parts over (no migrations
    // before beta) is read without its old keys: the rows win.
    #[test]
    fn a_stored_carrys_old_keys_are_ignored() {
        let stored = json!({
            "/deck/archetype_uuid": "q-stale",
            "/guides": [{"archetype_uuid": "q-stale", "cards": []}],
            "/notes": [],
        });
        let carry = Carry::from_json(&stored, DECK_CARRY).expect("a map");
        assert!(carry.is_empty());
        let mut bundle = json!({"deck": {"archetype_uuid": "q-row"}, "guides": [], "notes": []});
        let before = bundle.clone();
        carry.apply(&mut bundle);
        assert_eq!(bundle, before);
        let old_match = json!({"/archetypes": [{"uuid": "q-stale"}]});
        assert!(
            Carry::from_json(&old_match, MATCH_CARRY)
                .expect("a map")
                .is_empty()
        );
        let league = json!({"/league/deck_change_detected": true, "/guides": []});
        assert_eq!(
            Carry::from_json(&league, LEAGUE_CARRY)
                .expect("a map")
                .to_json(),
            json!({"/league/deck_change_detected": true})
        );
    }
}
