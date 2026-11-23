//! The server's rules evaluated locally (the API `ConditionEvaluator` and
//! `ClassifyDeck`, survey section 8; spec section 6.1 steps 1 and 5).

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::detect::FALLBACK_CONFIDENCE;
use crate::{Candidate, Condition, ConditionKind, Definition, ObservedCard};
use crate::{deck_identity, name_key, name_keys, normalize_colors};

/// Whether every condition passes over the observed name keys (`main` and
/// `side` hold `name_keys` of each card, faces included). In* and
/// DoesNotContain* read only the first card; OneOrMore* and TwoOrMore* read
/// the whole list.
pub fn satisfies(
    conditions: &[Condition],
    main: &BTreeSet<String>,
    side: &BTreeSet<String>,
) -> bool {
    conditions
        .iter()
        .all(|condition| passes(condition, main, side))
}

fn passes(condition: &Condition, main: &BTreeSet<String>, side: &BTreeSet<String>) -> bool {
    let in_main = |card: &String| main.contains(&name_key(card));
    let in_side = |card: &String| side.contains(&name_key(card));
    let present = |card: &String| in_main(card) || in_side(card);
    let first = condition.cards.first();
    let count = |test: &dyn Fn(&String) -> bool| condition.cards.iter().filter(|c| test(c)).count();

    match &condition.kind {
        ConditionKind::InMainboard => first.is_some_and(in_main),
        ConditionKind::InSideboard => first.is_some_and(in_side),
        ConditionKind::InMainOrSideboard => first.is_some_and(present),
        ConditionKind::OneOrMoreInMainboard => count(&in_main) >= 1,
        ConditionKind::OneOrMoreInSideboard => count(&in_side) >= 1,
        ConditionKind::OneOrMoreInMainOrSideboard => count(&present) >= 1,
        ConditionKind::TwoOrMoreInMainboard => count(&in_main) >= 2,
        ConditionKind::TwoOrMoreInSideboard => count(&in_side) >= 2,
        ConditionKind::TwoOrMoreInMainOrSideboard => count(&present) >= 2,
        ConditionKind::DoesNotContain => !first.is_some_and(present),
        ConditionKind::DoesNotContainMainboard => !first.is_some_and(in_main),
        ConditionKind::DoesNotContainSideboard => !first.is_some_and(in_side),
        ConditionKind::Unknown(_) => false,
    }
}

/// The exact rules answer (spec section 6.1 step 1): among candidates whose
/// definition is not a fallback, has conditions and passes them all, the
/// best by `rank`, then within its colour fork by colour, then the lowest
/// uuid. Confidence 1.0.
pub fn rules_match(observed: &[ObservedCard], candidates: &[Candidate]) -> Option<(i64, f64)> {
    let (main, side) = name_sets(observed);
    let mut passing: Vec<(&Candidate, &Definition)> = Vec::new();
    for candidate in candidates {
        let Some(definition) = &candidate.definition else {
            continue;
        };
        if definition.fallback || definition.conditions.is_empty() {
            continue;
        }
        if satisfies(&definition.conditions, &main, &side) {
            passing.push((candidate, definition));
        }
    }

    let mut best: Option<(&Candidate, &Definition)> = None;
    for &(candidate, definition) in &passing {
        let better = match best {
            None => true,
            Some((best_candidate, best_definition)) => {
                match compare_rank(&definition.rank, &best_definition.rank) {
                    Ordering::Greater => true,
                    Ordering::Less => false,
                    Ordering::Equal => candidate.uuid < best_candidate.uuid,
                }
            }
        };
        if better {
            best = Some((candidate, definition));
        }
    }

    let (_, best_definition) = best?;
    let fork: Vec<&Candidate> = passing
        .iter()
        .filter(|(_, definition)| same_fork(definition, best_definition))
        .map(|(candidate, _)| *candidate)
        .collect();
    let chosen = closest_colour(&fork, observed, &anchor_cards(best_definition))?;
    Some((chosen.archetype_id, 1.0))
}

/// The offline fallback answer (spec section 6.1 step 5, the API
/// `bestFallback`): the fallback definition sharing the most of its
/// `common_cards` with the observed distinct names, when that share is at
/// least its `min_overlap`; then by colour within its colour fork.
/// Confidence 0.5.
pub fn fallback_match(observed: &[ObservedCard], candidates: &[Candidate]) -> Option<(i64, f64)> {
    let distinct = distinct_names(observed);
    if distinct.is_empty() {
        return None;
    }

    let mut fallbacks: Vec<(&Candidate, &Definition)> = Vec::new();
    for candidate in candidates {
        if let Some(definition) = &candidate.definition
            && definition.fallback
        {
            fallbacks.push((candidate, definition));
        }
    }
    // Lowest uuid first, so a tie on overlap is never decided by load order.
    fallbacks.sort_by(|a, b| a.0.uuid.cmp(&b.0.uuid));

    let mut best: Option<(&Candidate, &Definition)> = None;
    let mut best_overlap = 0;
    for &(candidate, definition) in &fallbacks {
        let overlap = definition
            .common_cards
            .iter()
            .filter(|card| distinct.contains(&name_key(card)))
            .count();
        if overlap > best_overlap {
            best_overlap = overlap;
            best = Some((candidate, definition));
        }
    }

    let (_, best_definition) = best?;
    let share = best_overlap as f64 / distinct.len() as f64;
    if share < best_definition.min_overlap {
        return None;
    }
    let fork: Vec<&Candidate> = fallbacks
        .iter()
        .filter(|(_, definition)| same_fork(definition, best_definition))
        .map(|(candidate, _)| *candidate)
        .collect();
    // A fallback definition has no conditions, so no anchors.
    let chosen = closest_colour(&fork, observed, &[])?;
    Some((chosen.archetype_id, FALLBACK_CONFIDENCE))
}

/// The main and side name keys of the observed cards, faces included
/// (the API `ClassifyDeck::nameSets`). Cards without a name are skipped.
fn name_sets(observed: &[ObservedCard]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut main = BTreeSet::new();
    let mut side = BTreeSet::new();
    for card in observed {
        let Some(name) = &card.name else {
            continue;
        };
        for key in name_keys(name) {
            if card.sideboard {
                side.insert(key);
            } else {
                main.insert(key);
            }
        }
    }
    (main, side)
}

/// The distinct full name keys of the observed cards, main and side.
fn distinct_names(observed: &[ObservedCard]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for card in observed {
        if let Some(name) = &card.name {
            names.insert(name_key(name));
        }
    }
    names
}

/// Ranks compare element by element; a longer rank that agrees on every
/// shared element is the greater one.
fn compare_rank(a: &[f64], b: &[f64]) -> Ordering {
    for (left, right) in a.iter().zip(b.iter()) {
        let order = left.total_cmp(right);
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// Two definitions are in the same colour fork when they share a
/// `colour_fork` key; a definition without one is a fork of its own.
fn same_fork(a: &Definition, b: &Definition) -> bool {
    match (&a.colour_fork, &b.colour_fork) {
        (Some(left), Some(right)) => left == right,
        _ => std::ptr::eq(a, b),
    }
}

/// The name keys of the cards a definition requires in the main deck: the
/// first card of each `InMainboard` and `InMainOrSideboard` condition
/// (the API `ClassifyDeck::anchorCards`, `requiredMainboardCards`). The
/// server adds its variant's; a local definition's conditions hold both.
fn anchor_cards(definition: &Definition) -> Vec<String> {
    let mut anchors: Vec<String> = Vec::new();
    for condition in &definition.conditions {
        if !matches!(
            condition.kind,
            ConditionKind::InMainboard | ConditionKind::InMainOrSideboard
        ) {
            continue;
        }
        if let Some(card) = condition.cards.first() {
            let key = name_key(card);
            if !anchors.contains(&key) {
                anchors.push(key);
            }
        }
    }
    anchors
}

/// Within one colour fork: the candidate whose colour identity equals the
/// deck's colours as the server reads them (`deck_identity`: main deck only,
/// splashes left out, `anchors` always counted), else the one with the
/// fewest colours the deck lacks, then the lowest uuid. The server's
/// `nearestByColor` breaks ties by symmetric distance and prefers archetypes
/// with deck lists instead.
fn closest_colour<'a>(
    fork: &[&'a Candidate],
    observed: &[ObservedCard],
    anchors: &[String],
) -> Option<&'a Candidate> {
    let seen = deck_identity(observed, anchors).unwrap_or_default();
    let mut best: Option<(&Candidate, (bool, usize))> = None;
    for candidate in fork {
        let colours = normalize_colors(candidate.color_identity.as_deref()).unwrap_or_default();
        let key = (colours != seen, extra_colours(&colours, &seen));
        let better = match &best {
            None => true,
            Some((best_candidate, best_key)) => {
                key < *best_key || (key == *best_key && candidate.uuid < best_candidate.uuid)
            }
        };
        if better {
            best = Some((candidate, key));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// How many colours of `colours` ("W,U") are missing from `seen`.
fn extra_colours(colours: &str, seen: &str) -> usize {
    let seen_letters: Vec<&str> = seen.split(',').collect();
    colours
        .split(',')
        .filter(|letter| !letter.is_empty() && !seen_letters.contains(letter))
        .count()
}
