//! Pools and registered decks: the pool guards (0.x `DeckFitsLeaguePool`
//! and `RegisteredDeckMatchesDraftPool`), the sealed pool (0.x
//! `ReadSealedPool`) and its booster count (0.x `CountSealedPacks`).
//!
//! Unknown cards are neutral everywhere: they never reject a pool and never
//! count as basics.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::cards::{CardFactsMap, is_basic};
use crate::draft::{Draft, pool_counts};
use crate::payload::as_u64;

/// The share of a main deck's known non-basic cards that must come from the
/// pool. A 40 card deck carries about 23 spells; 0.75 tolerates a couple of
/// odd cards without letting another draft's deck through.
pub const MIN_COVERAGE: f64 = 0.75;

/// The quantity-weighted overlap a registered deck needs with a draft's pool
/// before it is evidence that the deck was built from it.
pub const MIN_POOL_OVERLAP: u32 = 15;

/// `S6FRA`: the booster count of a sealed match format code.
static SEALED_PACKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^S(?<packs>\d+)[A-Z]").expect("valid regex"));
/// `FRAx6`: the booster count of a league panel format code.
static PANEL_PACKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"x(?<packs>\d+)$").expect("valid regex"));

/// One row of a registered deck (0.x `{catalog_id, quantity, sideboard}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredCard {
    pub catalog_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
}

/// The deck MTGO registered for one match of a limited run (0.x's
/// `limited_deck_snapshots` row with source `registered`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredDeck {
    pub match_token: Option<String>,
    pub signature: String,
    pub captured_at_ms: i64,
    pub cards: Vec<RegisteredCard>,
}

/// A run's pool: copies of each card, and (sealed only) how many copies
/// arrived after the first registered deck, from the booster MTGO lets a
/// sealed player add mid-run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunPool {
    pub pool: BTreeMap<u32, u32>,
    pub added: BTreeMap<u32, u32>,
}

/// Main deck catalog id to quantity, summing duplicate rows (0.x
/// `ReadRegisteredDeck::mainDeck`).
pub fn main_deck(cards: &[RegisteredCard]) -> BTreeMap<u32, u32> {
    let mut main = BTreeMap::new();
    for card in cards.iter().filter(|card| !card.sideboard) {
        *main.entry(card.catalog_id).or_insert(0) += card.quantity;
    }
    main
}

/// True when the main deck could have been built from the pool (0.x
/// `DeckFitsLeaguePool`): at least 75% of its known non-basic cards are
/// covered by the pool. Basics and cards without a type are left out of both
/// sides of the ratio. An empty pool (no draft, or nothing picked) never
/// rejects, nor does a deck with nothing left to check.
pub fn deck_fits_pool(
    pool: &BTreeMap<u32, u32>,
    main_deck: &BTreeMap<u32, u32>,
    cards: &CardFactsMap,
) -> bool {
    if pool.is_empty() {
        return true;
    }

    let mut needed: u32 = 0;
    let mut covered: u32 = 0;
    for (&catalog_id, &quantity) in main_deck {
        let known = cards
            .get(&catalog_id)
            .is_some_and(|card| card.card_type.is_some());
        if !known || is_basic(cards, catalog_id) {
            continue;
        }
        needed += quantity;
        covered += quantity.min(pool.get(&catalog_id).copied().unwrap_or(0));
    }

    if needed == 0 {
        return true;
    }
    f64::from(covered) / f64::from(needed) >= MIN_COVERAGE
}

/// Positive evidence that a run's registered deck was built from this draft's
/// pool (0.x `RegisteredDeckMatchesDraftPool`): its main deck shares at least
/// 15 cards (quantity-weighted) with the pool and fits it. No registered deck
/// is no evidence. The raw overlap matters because a fresh pool's cards often
/// have no card data yet, and `deck_fits_pool` waves unknown cards through.
pub fn registered_deck_matches_pool(
    pool: &BTreeMap<u32, u32>,
    registered: Option<&[RegisteredCard]>,
    cards: &CardFactsMap,
) -> bool {
    let Some(registered) = registered else {
        return false;
    };
    let main = main_deck(registered);

    let overlap: u32 = main
        .iter()
        .map(|(catalog_id, quantity)| (*quantity).min(pool.get(catalog_id).copied().unwrap_or(0)))
        .sum();
    if overlap < MIN_POOL_OVERLAP {
        return false;
    }
    deck_fits_pool(pool, &main, cards)
}

/// A sealed pool read off its registered decks (0.x `ReadSealedPool`). A
/// limited registered deck holds the whole pool (the sideboard is every card
/// not played), so the pool is the most copies of each card any deck held,
/// minus basics; whatever the first deck did not hold was added later.
/// Decks are taken in capture order.
pub fn sealed_pool(decks: &[RegisteredDeck], cards: &CardFactsMap) -> RunPool {
    let totals: Vec<BTreeMap<u32, u32>> = in_capture_order(decks)
        .into_iter()
        .map(|deck| both_zones(&deck.cards))
        .collect();

    let mut pool: BTreeMap<u32, u32> = BTreeMap::new();
    for total in &totals {
        for (&catalog_id, &quantity) in total {
            let most = pool.entry(catalog_id).or_insert(0);
            *most = (*most).max(quantity);
        }
    }
    pool.retain(|&catalog_id, _| !is_basic(cards, catalog_id));

    let empty = BTreeMap::new();
    let first = totals.first().unwrap_or(&empty);
    let mut added = BTreeMap::new();
    for (&catalog_id, &quantity) in &pool {
        let before = first.get(&catalog_id).copied().unwrap_or(0);
        if quantity > before {
            added.insert(catalog_id, quantity - before);
        }
    }

    RunPool { pool, added }
}

/// How many boosters a sealed pool was opened from (0.x `CountSealedPacks`):
/// the count in the format code (`S6FRA` from a match, `FRAx6` from the
/// league panel), plus one once the added booster shows up. `None` when the
/// format carries no count (a draft format never does). Callers ask only for
/// sealed runs.
pub fn sealed_pack_count(format: &str, pool: &RunPool) -> Option<u32> {
    let found = SEALED_PACKS
        .captures(format)
        .or_else(|| PANEL_PACKS.captures(format))?;
    let packs: u32 = found["packs"].parse().ok()?;
    Some(packs + u32::from(!pool.added.is_empty()))
}

/// A run's pool (0.x `BuildDeckEvolution::poolInputs`): the draft's picks,
/// except a sealed run with no draft, whose pool is read off its registered
/// decks. Nothing added for a draft. A `stated` pool (Arena's, one catalog
/// id per card) wins over both: Arena's registered deck also holds the basic
/// lands added in deckbuilding.
pub fn run_pool(
    is_sealed: bool,
    draft: Option<&Draft>,
    decks: &[RegisteredDeck],
    cards: &CardFactsMap,
    stated: Option<&[u32]>,
) -> RunPool {
    if let Some(stated) = stated {
        let mut pool = BTreeMap::new();
        for catalog_id in stated {
            *pool.entry(*catalog_id).or_insert(0) += 1;
        }
        return RunPool {
            pool,
            added: BTreeMap::new(),
        };
    }
    match draft {
        Some(draft) => RunPool {
            pool: pool_counts(draft),
            added: BTreeMap::new(),
        },
        None if is_sealed => sealed_pool(decks, cards),
        None => RunPool::default(),
    }
}

/// The cards of a 0.x registered snapshot (`[{catalog_id, quantity,
/// sideboard}]`, numbers or numeric strings). A row with no catalog id is
/// dropped; a row with no quantity reads as zero copies, which every count
/// ignores, as 0.x skipped it. `sideboard` uses PHP's boolean reading.
pub fn registered_cards_from_json(value: &Value) -> Vec<RegisteredCard> {
    let Some(rows) = value.as_array() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let catalog_id = as_u64(row.get("catalog_id"))?;
            Some(RegisteredCard {
                catalog_id: u32::try_from(catalog_id).ok()?,
                quantity: as_u64(row.get("quantity"))
                    .and_then(|quantity| u32::try_from(quantity).ok())
                    .unwrap_or(0),
                sideboard: php_bool(row.get("sideboard")),
            })
        })
        .collect()
}

/// Decks sorted by capture time; equal times keep their order.
pub(crate) fn in_capture_order(decks: &[RegisteredDeck]) -> Vec<&RegisteredDeck> {
    let mut sorted: Vec<&RegisteredDeck> = decks.iter().collect();
    sorted.sort_by_key(|deck| deck.captured_at_ms);
    sorted
}

/// Copies of each card across both zones of one deck, zero counts dropped.
fn both_zones(cards: &[RegisteredCard]) -> BTreeMap<u32, u32> {
    let mut total = BTreeMap::new();
    for card in cards {
        *total.entry(card.catalog_id).or_insert(0) += card.quantity;
    }
    total.retain(|_, quantity| *quantity > 0);
    total
}

/// PHP's `filter_var(..., FILTER_VALIDATE_BOOL)`: true for `true`, the
/// number 1, and `1`, `true`, `on`, `yes` as text.
fn php_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n == 1.0),
        Some(Value::String(text)) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        _ => false,
    }
}
