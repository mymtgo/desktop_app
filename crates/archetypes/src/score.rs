//! 0.x's local estimate (`EstimateArchetypeLocally`, survey section 3.3),
//! numbers unchanged.
//!
//! The observed cards are collapsed per catalog id and matched by oracle id
//! against every list of every candidate. Basic lands are dropped from both
//! sides; nonbasic lands stay (they define Tron and friends). An observed
//! card without an oracle id stays in the denominator but cannot match.

use std::collections::BTreeMap;

use crate::{Candidate, ListCard, ObservedCard};

const QUANTITY_WEIGHT: f64 = 1.0;
const DISTINCT_WEIGHT: f64 = 0.35;
const COVERAGE_WEIGHT: f64 = 0.10;
const WEIGHT_SUM: f64 = QUANTITY_WEIGHT + DISTINCT_WEIGHT + COVERAGE_WEIGHT;
const AMBIGUITY_THRESHOLD: f64 = 0.03;
const AMBIGUITY_PENALTY: f64 = 0.7;
/// Matched distinct cards needed for full confidence, unless most of a small
/// list is covered.
const MIN_CONFIDENT_MATCHES: f64 = 8.0;

/// The best list for the observed cards.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub archetype_id: i64,
    /// Index into the candidate's `lists`, 0-based: index 0 is stored
    /// list 1 (`archetype_cards.list` starts at 1).
    pub list_index: usize,
    /// 0 to 1, rounded to four places.
    pub confidence: f64,
}

/// One collapsed observed card: its oracle id when resolved, its copies.
struct InputCard {
    oracle_id: Option<String>,
    quantity: u32,
}

/// One scored list.
struct Scored {
    archetype_id: i64,
    list: usize,
    score: f64,
    matched_distinct: usize,
    deck_coverage: f64,
}

/// 0.x `EstimateArchetypeLocally::run` over the given candidates (the held
/// archetypes of the match's format). `None` when nothing is observed,
/// nothing resolves or no list shares a card.
pub fn score(observed: &[ObservedCard], candidates: &[Candidate]) -> Option<Estimate> {
    let input = collapse(observed);
    if input.is_empty() {
        return None;
    }
    let input_distinct = input.len();
    let input_total: u32 = input.iter().map(|card| card.quantity).sum();

    let by_oracle = quantities_by_oracle(&input);
    if by_oracle.is_empty() {
        return None;
    }

    let mut scores = Vec::new();
    for candidate in candidates {
        for (index, list) in candidate.lists.iter().enumerate() {
            let deck = list_by_oracle(list);
            if let Some(scored) = score_list(
                &by_oracle,
                input_distinct,
                input_total,
                &deck,
                candidate,
                index,
            ) {
                scores.push(scored);
            }
        }
    }
    if scores.is_empty() {
        return None;
    }

    // Stable sort: equal scores keep the candidates' order, as 0.x's usort.
    scores.sort_by(|a, b| b.score.total_cmp(&a.score));
    let best = &scores[0];
    let mut confidence = best.score;
    if let Some(second) = scores.get(1)
        && second.archetype_id != best.archetype_id
        && best.score - second.score < AMBIGUITY_THRESHOLD
    {
        confidence *= AMBIGUITY_PENALTY;
    }
    let by_matches = best.matched_distinct as f64 / MIN_CONFIDENT_MATCHES;
    let evidence = by_matches.max(best.deck_coverage).min(1.0);
    confidence *= evidence;

    Some(Estimate {
        archetype_id: best.archetype_id,
        list_index: best.list,
        confidence: round4(confidence),
    })
}

/// Sums the observed copies per catalog id and drops basic lands (only a
/// resolved card is known to be one).
fn collapse(observed: &[ObservedCard]) -> Vec<InputCard> {
    let mut order: Vec<i64> = Vec::new();
    let mut cards: BTreeMap<i64, (Option<String>, Option<String>, u32)> = BTreeMap::new();
    for card in observed {
        let entry = cards.entry(card.catalog_id).or_insert_with(|| {
            order.push(card.catalog_id);
            (card.oracle_id.clone(), card.card_type.clone(), 0)
        });
        entry.2 += card.quantity;
    }

    let mut input = Vec::new();
    for catalog_id in order {
        let (oracle_id, card_type, quantity) = cards.remove(&catalog_id).unwrap_or_default();
        let basic = oracle_id.is_some() && is_basic_land(card_type.as_deref());
        if !basic {
            input.push(InputCard {
                oracle_id,
                quantity,
            });
        }
    }
    input
}

/// The resolved input cards' copies per oracle id.
fn quantities_by_oracle(input: &[InputCard]) -> BTreeMap<String, u32> {
    let mut by_oracle = BTreeMap::new();
    for card in input {
        if let Some(oracle_id) = &card.oracle_id {
            *by_oracle.entry(oracle_id.clone()).or_insert(0) += card.quantity;
        }
    }
    by_oracle
}

/// A list's copies per oracle id, basic lands and unresolved cards out.
/// A card in both the maindeck and the sideboard counts its copies once
/// summed.
fn list_by_oracle(list: &[ListCard]) -> BTreeMap<String, u32> {
    let mut deck = BTreeMap::new();
    for card in list {
        let Some(oracle_id) = &card.oracle_id else {
            continue;
        };
        if is_basic_land(card.card_type.as_deref()) {
            continue;
        }
        *deck.entry(oracle_id.clone()).or_insert(0) += card.quantity;
    }
    deck
}

fn score_list(
    input: &BTreeMap<String, u32>,
    input_distinct: usize,
    input_total: u32,
    deck: &BTreeMap<String, u32>,
    candidate: &Candidate,
    list: usize,
) -> Option<Scored> {
    if deck.is_empty() {
        return None;
    }
    let mut matched_distinct = 0;
    let mut matched_quantity = 0;
    for (oracle_id, quantity) in input {
        if let Some(deck_quantity) = deck.get(oracle_id) {
            matched_distinct += 1;
            matched_quantity += (*quantity).min(*deck_quantity);
        }
    }
    if matched_distinct == 0 {
        return None;
    }

    let quantity_overlap = ratio(matched_quantity as f64, input_total as f64);
    let distinct_overlap = ratio(matched_distinct as f64, input_distinct as f64);
    let deck_coverage = ratio(matched_distinct as f64, deck.len() as f64);
    let score = (quantity_overlap * QUANTITY_WEIGHT
        + distinct_overlap * DISTINCT_WEIGHT
        + deck_coverage * COVERAGE_WEIGHT)
        / WEIGHT_SUM;

    Some(Scored {
        archetype_id: candidate.archetype_id,
        list,
        score,
        matched_distinct,
        deck_coverage,
    })
}

fn ratio(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { part / whole } else { 0.0 }
}

/// A basic land's type holds both "Basic" and "Land".
fn is_basic_land(card_type: Option<&str>) -> bool {
    let card_type = card_type.unwrap_or("");
    card_type.contains("Basic") && card_type.contains("Land")
}

/// PHP `round($value, 4)`.
fn round4(value: f64) -> f64 {
    decks::php_round(value, 4)
}
