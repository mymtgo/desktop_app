//! Ports 0.x `DeckFitsLeaguePoolTest`, the pool and pack count cases of
//! `SealedPoolTest`, and the overlap rule of `RegisteredDeckMatchesDraftPool`.

mod common;

use std::collections::BTreeMap;

use common::{card, drafted, facts, reg, registered};
use limited::{
    CardFactsMap, MIN_POOL_OVERLAP, RegisteredCard, RunPool, deck_fits_pool, main_deck,
    pool_counts, registered_cards_from_json, registered_deck_matches_pool, run_pool,
    sealed_pack_count, sealed_pool,
};
use serde_json::json;

const ISLAND: u32 = 153894;

/// Known spells (type `Creature`) for these ids, plus an Island.
fn known_spells(ids: impl IntoIterator<Item = u32>) -> CardFactsMap {
    let mut cards: CardFactsMap = ids
        .into_iter()
        .map(|id| (id, card("Creature", "")))
        .collect();
    cards.insert(ISLAND, card("Basic Land", ""));
    cards
}

fn one_each(ids: impl IntoIterator<Item = u32>) -> BTreeMap<u32, u32> {
    ids.into_iter().map(|id| (id, 1)).collect()
}

#[test]
fn accepts_a_deck_built_from_the_pool_plus_basics() {
    let pool = pool_counts(&drafted(&(1000..=1041).collect::<Vec<_>>()));
    let cards = known_spells(1000..1023);
    let mut main = one_each(1000..1023);
    main.insert(ISLAND, 17);

    assert!(deck_fits_pool(&pool, &main, &cards));
}

#[test]
fn rejects_a_deck_from_a_different_draft() {
    let pool = pool_counts(&drafted(&(1000..=1041).collect::<Vec<_>>()));
    let cards = known_spells(2000..=2022);
    let mut main = one_each(2000..=2022);
    main.insert(ISLAND, 17);

    assert!(!deck_fits_pool(&pool, &main, &cards));
}

#[test]
fn accepts_when_there_is_no_draft_pool_to_compare_against() {
    let main = BTreeMap::from([(1, 40)]);

    assert!(deck_fits_pool(&BTreeMap::new(), &main, &known_spells([1])));
}

#[test]
fn accepts_a_deck_with_unresolved_catalog_ids_alongside_fitting_spells() {
    // Eight unresolved ids would drag a fully covered 23 spell deck below 0.75
    // (23 / 31) if they counted as uncovered, so this only passes when they are
    // left out of the ratio. 160000 has card facts but no type yet.
    let pool_ids: Vec<u32> = (1000..=1022).collect();
    let pool = pool_counts(&drafted(&pool_ids));
    let mut cards = known_spells(pool_ids.clone());
    cards.insert(160000, Default::default());

    let mut main = one_each(pool_ids);
    main.insert(ISLAND, 17);
    main.extend(one_each(9000..=9007));
    main.insert(160000, 1);

    assert!(deck_fits_pool(&pool, &main, &cards));
}

#[test]
fn coverage_of_exactly_three_quarters_fits() {
    let pool = pool_counts(&drafted(&[1, 2, 3]));
    let cards = known_spells([1, 2, 3, 4]);

    assert!(deck_fits_pool(&pool, &one_each([1, 2, 3, 4]), &cards));
    assert!(!deck_fits_pool(
        &pool,
        &BTreeMap::from([(1, 1), (2, 1), (4, 2)]),
        &cards
    ));
}

#[test]
fn reads_the_main_deck_summing_duplicate_rows() {
    let cards = vec![reg(1, 2, false), reg(1, 1, false), reg(2, 1, true)];

    assert_eq!(main_deck(&cards), BTreeMap::from([(1, 3)]));
}

/// A registered deck of `spells` (one each, main) plus 17 Islands.
fn registered_spells(spells: impl IntoIterator<Item = u32>) -> Vec<RegisteredCard> {
    let mut cards: Vec<RegisteredCard> = spells.into_iter().map(|id| reg(id, 1, false)).collect();
    cards.push(reg(ISLAND, 17, false));
    cards.push(reg(5000, 3, true));
    cards
}

#[test]
fn a_registered_deck_built_from_the_pool_matches_it() {
    let pool = pool_counts(&drafted(&(1000..=1041).collect::<Vec<_>>()));
    let cards = known_spells(1000..1023);

    assert!(registered_deck_matches_pool(
        &pool,
        Some(&registered_spells(1000..1023)),
        &cards
    ));
}

#[test]
fn a_registered_deck_needs_15_cards_of_overlap_even_when_nothing_is_resolved() {
    let pool = pool_counts(&drafted(&(1000..=1041).collect::<Vec<_>>()));
    let unresolved = facts(vec![]);
    let overlap = MIN_POOL_OVERLAP;

    // With no card facts every card is neutral, so the coverage check alone
    // would pass anything: the raw overlap is the evidence.
    assert!(!registered_deck_matches_pool(
        &pool,
        Some(&registered_spells(1000..1000 + overlap - 1)),
        &unresolved
    ));
    assert!(registered_deck_matches_pool(
        &pool,
        Some(&registered_spells(1000..1000 + overlap)),
        &unresolved
    ));
}

#[test]
fn a_registered_deck_with_overlap_but_poor_coverage_does_not_match() {
    let pool = pool_counts(&drafted(&(1000..=1041).collect::<Vec<_>>()));
    // 15 pool cards and 10 cards from elsewhere: 15 / 25 is below 0.75.
    let cards = known_spells((1000..1015).chain(2000..2010));

    assert!(!registered_deck_matches_pool(
        &pool,
        Some(&registered_spells((1000..1015).chain(2000..2010))),
        &cards
    ));
}

#[test]
fn no_registered_deck_never_matches() {
    let pool = pool_counts(&drafted(&(1000..=1041).collect::<Vec<_>>()));

    assert!(!registered_deck_matches_pool(&pool, None, &facts(vec![])));
}

/// A sealed run of six FRA boosters (0.x `sealedPoolLeague`). Each registered
/// deck holds the whole pool; with `booster_added` the second deck also holds
/// the booster added after match 3.
fn sealed_decks(booster_added: bool) -> Vec<limited::RegisteredDeck> {
    let first = vec![reg(1, 1, false), reg(9, 17, false), reg(2, 1, true)];
    let mut decks = vec![registered("q-match-1", "p1", 10, first.clone())];
    if booster_added {
        let mut later = first;
        later.push(reg(6, 1, true));
        decks.push(registered("q-match-4", "p2", 40, later));
    }
    decks
}

fn sealed_cards() -> CardFactsMap {
    facts(vec![
        (1, card("Creature", "W")),
        (2, card("Creature", "U")),
        (6, card("Creature", "R")),
        (9, card("Basic Land", "")),
    ])
}

#[test]
fn reads_the_sealed_pool_without_basics_and_marks_booster_copies_as_added() {
    let result = sealed_pool(&sealed_decks(true), &sealed_cards());

    assert_eq!(result.pool, BTreeMap::from([(1, 1), (2, 1), (6, 1)]));
    assert_eq!(result.added, BTreeMap::from([(6, 1)]));
}

#[test]
fn reads_the_sealed_pool_in_capture_order_whatever_the_input_order() {
    let mut decks = sealed_decks(true);
    decks.reverse();

    assert_eq!(
        sealed_pool(&decks, &sealed_cards()).added,
        BTreeMap::from([(6, 1)])
    );
}

#[test]
fn unknown_cards_stay_in_the_sealed_pool() {
    let result = sealed_pool(&sealed_decks(true), &facts(vec![]));

    assert_eq!(
        result.pool,
        BTreeMap::from([(1, 1), (2, 1), (6, 1), (9, 17)])
    );
}

#[test]
fn reads_an_empty_pool_before_any_deck_is_registered() {
    assert_eq!(sealed_pool(&[], &sealed_cards()), RunPool::default());
}

#[test]
fn counts_the_boosters_a_sealed_pool_was_opened_from() {
    let six = sealed_pool(&sealed_decks(false), &sealed_cards());
    let seven = sealed_pool(&sealed_decks(true), &sealed_cards());

    assert_eq!(sealed_pack_count("S6FRA", &six), Some(6));
    assert_eq!(sealed_pack_count("S6FRA", &seven), Some(7));
}

#[test]
fn reads_the_booster_count_from_either_format_spelling() {
    let empty = RunPool::default();

    assert_eq!(sealed_pack_count("S6FRA", &empty), Some(6));
    assert_eq!(sealed_pack_count("FRAx6", &empty), Some(6));
    assert_eq!(sealed_pack_count("Sealed", &empty), None);
}

#[test]
fn has_no_pack_count_for_a_draft_format() {
    assert_eq!(sealed_pack_count("DFRAFRAFRA", &RunPool::default()), None);
}

#[test]
fn a_run_pool_is_the_draft_picks_unless_a_sealed_run_has_no_draft() {
    let draft = drafted(&[1, 1, 2]);
    let decks = sealed_decks(true);
    let cards = sealed_cards();

    let from_draft = run_pool(false, Some(&draft), &decks, &cards, None);
    assert_eq!(from_draft.pool, BTreeMap::from([(1, 2), (2, 1)]));
    assert!(from_draft.added.is_empty());

    let sealed_with_draft = run_pool(true, Some(&draft), &decks, &cards, None);
    assert_eq!(sealed_with_draft, from_draft);

    let sealed = run_pool(true, None, &decks, &cards, None);
    assert_eq!(sealed.added, BTreeMap::from([(6, 1)]));

    assert_eq!(
        run_pool(false, None, &decks, &cards, None),
        RunPool::default()
    );
}

#[test]
fn a_stated_pool_wins() {
    let draft = drafted(&[1, 1, 2]);
    let decks = sealed_decks(true);
    let cards = sealed_cards();
    let stated = [7, 7, 8];
    let want = RunPool {
        pool: BTreeMap::from([(7, 2), (8, 1)]),
        added: BTreeMap::new(),
    };

    assert_eq!(run_pool(true, None, &decks, &cards, Some(&stated)), want);
    assert_eq!(run_pool(false, None, &decks, &cards, Some(&stated)), want);
    assert_eq!(
        run_pool(false, Some(&draft), &decks, &cards, Some(&stated)),
        want
    );
}

#[test]
fn reads_registered_cards_from_a_0x_snapshot() {
    let value = json!([
        {"catalog_id": 1, "quantity": 2, "sideboard": false},
        {"catalog_id": "2", "sideboard": "1"},
        {"quantity": 1, "sideboard": false},
        {"catalog_id": 4, "quantity": 1}
    ]);

    // A row with no quantity reads as zero copies (0.x skipped it when
    // counting); a row with no catalog id is dropped.
    assert_eq!(
        registered_cards_from_json(&value),
        vec![reg(1, 2, false), reg(2, 0, true), reg(4, 1, false)]
    );
}
