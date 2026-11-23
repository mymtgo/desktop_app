//! The 0.x rules both importers keep: the database readers (`read`,
//! `limited_read`, `league_read`, `manual_read`) and the pulled bundle reader
//! (`bundle_read`). One copy, so the two cannot drift apart.

use std::collections::BTreeMap;

use limited::DraftState;

/// 0.x match states v1 imports; `started` and `in_progress` are still being
/// worked on.
pub(crate) const FINISHED_STATES: [&str; 3] = ["complete", "ended", "abandoned"];

/// The 0.x match outcomes v1 keeps.
pub(crate) const OUTCOMES: [&str; 4] = ["win", "loss", "draw", "unknown"];

/// 0.x column defaults of a draft row.
pub(crate) const DEFAULT_SEAT_COUNT: u32 = 8;
pub(crate) const DEFAULT_PACK_SIZE: u32 = 14;
pub(crate) const DEFAULT_PICKS_EXPECTED: u32 = 42;

/// A format or match type code; empty and `Unknown` are `None`.
pub(crate) fn known_code(code: Option<String>) -> Option<String> {
    let code = code?;
    let trimmed = code.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        return None;
    }
    Some(trimmed.to_string())
}

/// A count or index that fits the fold's 32-bit numbers.
pub(crate) fn small(value: Option<i64>) -> Option<u32> {
    value.and_then(|number| u32::try_from(number).ok())
}

/// An id that fits the fold's unsigned 64-bit numbers.
pub(crate) fn large(value: Option<i64>) -> Option<u64> {
    value.and_then(|number| u64::try_from(number).ok())
}

/// A 0.x draft state (`drafts.state`); `None` for one v1 does not know. The
/// database reader skips such a draft, the bundle reader imports it finished.
pub(crate) fn draft_state(state: &str) -> Option<DraftState> {
    match state {
        "connecting" => Some(DraftState::Connecting),
        "picking" => Some(DraftState::Picking),
        "finished" => Some(DraftState::Finished),
        "abandoned" => Some(DraftState::Abandoned),
        _ => None,
    }
}

/// The maindeck copies of a deck version signature, by catalog id.
pub(crate) fn main_copies(signature: &str) -> BTreeMap<u32, u32> {
    let mut copies = BTreeMap::new();
    for card in decks::decode_signature(signature) {
        if !card.sideboard {
            *copies.entry(card.catalog_id).or_insert(0) += card.quantity;
        }
    }
    copies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_that_is_empty_or_unknown_is_none() {
        assert_eq!(known_code(None), None);
        assert_eq!(known_code(Some("  ".to_string())), None);
        assert_eq!(known_code(Some("UNKNOWN".to_string())), None);
        assert_eq!(
            known_code(Some(" CMODERN ".to_string())),
            Some("CMODERN".to_string())
        );
    }

    #[test]
    fn numbers_that_do_not_fit_are_none() {
        assert_eq!(small(Some(-1)), None);
        assert_eq!(small(Some(i64::from(u32::MAX) + 1)), None);
        assert_eq!(small(Some(42)), Some(42));
        assert_eq!(large(Some(-1)), None);
        assert_eq!(large(Some(35_746_768)), Some(35_746_768));
        assert_eq!(large(None), None);
    }

    #[test]
    fn a_draft_state_v1_does_not_know_is_none() {
        assert_eq!(draft_state("connecting"), Some(DraftState::Connecting));
        assert_eq!(draft_state("picking"), Some(DraftState::Picking));
        assert_eq!(draft_state("finished"), Some(DraftState::Finished));
        assert_eq!(draft_state("abandoned"), Some(DraftState::Abandoned));
        assert_eq!(draft_state("exploded"), None);
    }

    #[test]
    fn main_copies_add_up_the_maindeck_and_leave_out_the_sideboard() {
        let cards = [
            decks::DeckCard {
                catalog_id: 1,
                quantity: 2,
                sideboard: false,
            },
            decks::DeckCard {
                catalog_id: 1,
                quantity: 1,
                sideboard: false,
            },
            decks::DeckCard {
                catalog_id: 2,
                quantity: 3,
                sideboard: true,
            },
        ];
        let signature = decks::signature(&cards);
        assert_eq!(main_copies(&signature), BTreeMap::from([(1, 3)]));
        assert_eq!(main_copies("not a signature"), BTreeMap::new());
    }
}
