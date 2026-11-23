//! Builders shared by the analytics tests: drafts, picks, card facts and
//! registered decks, with 0.x's column defaults.

#![allow(dead_code)]

use limited::{CardFacts, CardFactsMap, Draft, DraftState, Pick, RegisteredCard, RegisteredDeck};

pub const SECOND_MS: i64 = 1000;
/// 2026-08-22 11:00:00 UTC.
pub const ELEVEN_AM: i64 = 1_787_396_400_000;

/// A pick with nothing but its ordinal set.
pub fn pick(ordinal: u32) -> Pick {
    Pick {
        ordinal,
        pack_number: 1,
        pick_number: ordinal,
        pack_id: None,
        direction: None,
        cards_available: Vec::new(),
        picked_catalog_id: None,
        picked_card_id: None,
        reservations: Vec::new(),
        shown_at_ms: None,
        deadline_at_ms: None,
        picked_at_ms: None,
    }
}

/// A pick of `picked` from pack `pack_id` showing `available` (0.x
/// `picksFrom` in `ComputeSeenWheelTest`).
pub fn pack_pick(ordinal: u32, pack_id: u64, available: &[u32], picked: u32) -> Pick {
    Pick {
        pack_id: Some(pack_id),
        cards_available: available.to_vec(),
        picked_catalog_id: Some(picked),
        ..pick(ordinal)
    }
}

/// A finished draft with 0.x's defaults (8 seats, 14 card packs).
pub fn draft(token: &str, picks: Vec<Pick>) -> Draft {
    Draft {
        token: token.to_string(),
        league_event_id: None,
        course_id: None,
        draft_id: None,
        pod_token: None,
        seat_index: None,
        seat_count: 8,
        booster_catalog_id: None,
        state: DraftState::Finished,
        pack_size: 14,
        picks_expected: 42,
        started_at_ms: None,
        ended_at_ms: None,
        picks,
    }
}

/// A draft that picked these catalog ids in order, from ordinal 1.
pub fn drafted(ids: &[u32]) -> Draft {
    let picks = ids
        .iter()
        .enumerate()
        .map(|(index, id)| Pick {
            picked_catalog_id: Some(*id),
            ..pick(index as u32 + 1)
        })
        .collect();
    draft("q-draft", picks)
}

/// Card facts with a type and colours.
pub fn card(card_type: &str, colors: &str) -> CardFacts {
    CardFacts {
        card_type: Some(card_type.to_string()),
        colors: Some(colors.to_string()),
        ..CardFacts::default()
    }
}

/// Card facts with a name, oracle id and set code.
pub fn named(name: &str, oracle_id: &str, set_code: &str) -> CardFacts {
    CardFacts {
        name: Some(name.to_string()),
        oracle_id: Some(oracle_id.to_string()),
        set_code: Some(set_code.to_string()),
        ..CardFacts::default()
    }
}

pub fn facts(entries: Vec<(u32, CardFacts)>) -> CardFactsMap {
    entries.into_iter().collect()
}

/// One row of a registered deck.
pub fn reg(catalog_id: u32, quantity: u32, sideboard: bool) -> RegisteredCard {
    RegisteredCard {
        catalog_id,
        quantity,
        sideboard,
    }
}

/// A registered deck for a match, captured `minutes` after the hour.
pub fn registered(
    match_token: &str,
    signature: &str,
    minutes: i64,
    cards: Vec<RegisteredCard>,
) -> RegisteredDeck {
    RegisteredDeck {
        match_token: Some(match_token.to_string()),
        signature: signature.to_string(),
        captured_at_ms: ELEVEN_AM + minutes * 60 * SECOND_MS,
        cards,
    }
}
