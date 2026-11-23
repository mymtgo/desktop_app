//! What each card did across a draft's packs (0.x `ComputeSeenWheel`).
//!
//! A card "wheels" when the same physical pack (`pack_id`) comes back
//! `seat_count` picks later and the card is still in it.

use std::collections::BTreeMap;

use crate::draft::Pick;

/// 0.x's fallback when a draft has no seat count (`seat_count ?: 8`).
const DEFAULT_SEATS: u32 = 8;

/// Per card facts from the packs shown at every pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeenFacts {
    /// How many packs showed it (a pack showing it twice counts twice).
    pub seen_count: u32,
    pub first_seen_ordinal: u32,
    /// It was still in a pack when that pack came back round.
    pub wheeled: bool,
    /// It came back round and the player took it.
    pub wheeled_to_me: bool,
    pub picked_count: u32,
    pub passed_count: u32,
}

/// What came back when a pick's pack wheeled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelView {
    pub return_ordinal: u32,
    /// Cards passed at this pick that were still there.
    pub survived: Vec<u32>,
    /// Cards passed at this pick that others took.
    pub taken: Vec<u32>,
}

/// The wheel distance for a draft's seat count (0 reads as 8, as 0.x).
pub fn seats(seat_count: u32) -> u32 {
    if seat_count == 0 {
        DEFAULT_SEATS
    } else {
        seat_count
    }
}

/// Per catalog id, what the packs showed (0.x `ComputeSeenWheel::run`).
pub fn seen_wheel(picks: &[Pick], seat_count: u32) -> BTreeMap<u32, SeenFacts> {
    let mut sorted: Vec<&Pick> = picks.iter().collect();
    sorted.sort_by_key(|pick| pick.ordinal);

    let mut facts: BTreeMap<u32, SeenFacts> = BTreeMap::new();
    for pick in sorted {
        let wheel = return_pick(picks, pick, seat_count);

        for &catalog_id in &pick.cards_available {
            let entry = facts.entry(catalog_id).or_insert(SeenFacts {
                seen_count: 0,
                first_seen_ordinal: pick.ordinal,
                wheeled: false,
                wheeled_to_me: false,
                picked_count: 0,
                passed_count: 0,
            });
            entry.seen_count = entry.seen_count.saturating_add(1);

            if pick.picked_catalog_id == Some(catalog_id) {
                entry.picked_count = entry.picked_count.saturating_add(1);
            } else {
                entry.passed_count = entry.passed_count.saturating_add(1);
            }

            if let Some(back) = wheel
                && back.cards_available.contains(&catalog_id)
            {
                entry.wheeled = true;
                if back.picked_catalog_id == Some(catalog_id) {
                    entry.wheeled_to_me = true;
                }
            }
        }
    }
    facts
}

/// What survived and what was taken when this pick's pack came back, or
/// `None` when it never did (0.x `ComputeSeenWheel::wheelForPick`).
pub fn wheel_for_pick(picks: &[Pick], pick: &Pick, seat_count: u32) -> Option<WheelView> {
    let back = return_pick(picks, pick, seat_count)?;

    let candidates = pick
        .cards_available
        .iter()
        .copied()
        .filter(|&catalog_id| pick.picked_catalog_id != Some(catalog_id));
    let (survived, taken): (Vec<u32>, Vec<u32>) =
        candidates.partition(|catalog_id| back.cards_available.contains(catalog_id));

    Some(WheelView {
        return_ordinal: back.ordinal,
        survived,
        taken,
    })
}

/// The pick `seat_count` ordinals later, when it shows the same physical
/// pack. A pick with no pack id never wheels.
fn return_pick<'a>(picks: &'a [Pick], pick: &Pick, seat_count: u32) -> Option<&'a Pick> {
    let pack_id = pick.pack_id?;
    // Absurd input (a huge seat count) never wraps onto a real pick.
    let back_ordinal = pick.ordinal.checked_add(seat_count)?;
    let back = picks.iter().find(|other| other.ordinal == back_ordinal)?;
    // 0.x compares `(int) pack_id`, so a missing pack id reads as 0.
    if back.pack_id.unwrap_or(0) == pack_id {
        Some(back)
    } else {
        None
    }
}
