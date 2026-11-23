//! 0.x `tests/Feature/Archetypes/EstimateArchetypeLocallyTest.php`, all
//! eighteen cases, as pure cases. 0.x resolved the input through the
//! `cards` table; here the observed cards carry the card row's oracle id and
//! type, looked up in a small card table built from the lists.

use archetypes::{Candidate, ListCard, ObservedCard, Source, format_key, score};

fn card(oracle: &str, catalog: i64, quantity: u32, card_type: &str) -> ListCard {
    ListCard {
        catalog_id: catalog,
        oracle_id: Some(oracle.to_string()),
        card_type: Some(card_type.to_string()),
        quantity,
        sideboard: false,
    }
}

fn spell(oracle: &str, catalog: i64, quantity: u32) -> ListCard {
    card(oracle, catalog, quantity, "Instant")
}

fn archetype(id: i64, lists: Vec<Vec<ListCard>>) -> Candidate {
    Candidate {
        archetype_id: id,
        uuid: format!("6f1c2e4a-0000-4000-8000-{id:012}"),
        source: Source::Downloaded,
        color_identity: None,
        definition: None,
        lists,
    }
}

/// The input as 0.x's `cards` lookup saw it: a catalog id known to the card
/// table gets its oracle id and type, an unknown one stays unresolved.
fn observe(input: &[(i64, u32)], table: &[ListCard]) -> Vec<ObservedCard> {
    let mut observed = Vec::new();
    for (catalog_id, quantity) in input {
        let known = table.iter().find(|c| c.catalog_id == *catalog_id);
        observed.push(ObservedCard {
            catalog_id: *catalog_id,
            quantity: *quantity,
            sideboard: false,
            oracle_id: known.and_then(|c| c.oracle_id.clone()),
            name: None,
            card_type: known.and_then(|c| c.card_type.clone()),
            color_identity: None,
            mana_cost: None,
        });
    }
    observed
}

fn table_of(candidates: &[Candidate]) -> Vec<ListCard> {
    let mut table = Vec::new();
    for candidate in candidates {
        for list in &candidate.lists {
            table.extend(list.iter().cloned());
        }
    }
    table
}

/// The held candidates of a raw match format, as the store's
/// `held_candidates(conn, format_key)` selects them.
fn of_format(all: &[(&str, Candidate)], raw_format: &str) -> Vec<Candidate> {
    let key = format_key(raw_format);
    all.iter()
        .filter(|(format, _)| Some(format.to_string()) == key)
        .map(|(_, candidate)| candidate.clone())
        .collect()
}

fn burn_four() -> Candidate {
    archetype(
        1,
        vec![vec![
            spell("bolt", 100, 4),
            spell("spike", 101, 4),
            spell("guide", 102, 4),
            spell("swift", 103, 4),
        ]],
    )
}

#[test]
fn matches_a_deck_against_a_local_archetype() {
    let candidates = vec![burn_four()];
    let observed = observe(
        &[(100, 4), (101, 4), (102, 4), (103, 4)],
        &table_of(&candidates),
    );

    let result = score(&observed, &candidates).expect("an estimate");

    assert!(result.confidence > 0.5);
}

#[test]
fn returns_null_when_no_archetypes_exist_for_the_format() {
    let all = vec![("modern", archetype(1, vec![vec![spell("bolt", 100, 4)]]))];
    let table = table_of(&[all[0].1.clone()]);
    let observed = observe(&[(100, 4)], &table);

    assert_eq!(score(&observed, &of_format(&all, "legacy")), None);
}

#[test]
fn returns_null_when_no_cards_overlap() {
    let candidates = vec![archetype(1, vec![vec![spell("bolt", 100, 4)]])];
    let observed = observe(&[(999, 4)], &table_of(&candidates));

    assert_eq!(score(&observed, &candidates), None);
}

#[test]
fn picks_the_best_matching_archetype() {
    let burn = archetype(
        1,
        vec![vec![
            spell("bolt", 100, 4),
            spell("spike", 101, 4),
            spell("guide", 102, 4),
        ]],
    );
    let control = archetype(
        2,
        vec![vec![spell("counter", 200, 4), spell("verdict", 201, 2)]],
    );
    let candidates = vec![burn, control];
    let observed = observe(&[(100, 4), (101, 4), (102, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert_eq!(result.archetype_id, 1);
    assert_eq!(result.list_index, 0);
}

#[test]
fn skips_archetypes_without_downloaded_decklists() {
    let candidates = vec![archetype(1, Vec::new())];
    let observed = observe(&[(100, 4)], &[]);

    assert_eq!(score(&observed, &candidates), None);
}

#[test]
fn never_reports_confidence_above_one() {
    let candidates = vec![archetype(
        1,
        vec![vec![spell("bolt", 100, 4), spell("spike", 101, 4)]],
    )];
    let observed = observe(&[(100, 4), (101, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert!(result.confidence <= 1.0);
}

#[test]
fn normalizes_mtgo_format_codes_like_cpioneer_to_pioneer() {
    let phoenix = archetype(
        1,
        vec![vec![spell("phoenix", 400, 4), spell("pieces", 401, 4)]],
    );
    let all = vec![("pioneer", phoenix)];
    let candidates = of_format(&all, "CPIONEER");
    let observed = observe(&[(400, 4), (401, 4)], &table_of(&candidates));

    assert!(score(&observed, &candidates).is_some());
}

#[test]
fn normalizes_mtgo_format_codes_like_cmodern_to_modern() {
    let burn = archetype(1, vec![vec![spell("bolt", 100, 4), spell("spike", 101, 4)]]);
    let all = vec![("modern", burn)];
    let candidates = of_format(&all, "CMODERN");
    let observed = observe(&[(100, 4), (101, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert!(result.confidence > 0.5);
}

#[test]
fn applies_ambiguity_penalty_when_top_two_scores_are_close() {
    let deck_a = archetype(
        1,
        vec![vec![spell("shared1", 100, 4), spell("unique_a", 102, 4)]],
    );
    let deck_b = archetype(
        2,
        vec![vec![spell("shared1", 100, 4), spell("unique_b", 103, 4)]],
    );
    let candidates = vec![deck_a, deck_b];
    let observed = observe(&[(100, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    // (1.0 + 0.35 + 0.10 * 0.5) / 1.45, times 0.7 (ambiguity), times 0.5
    // (evidence: one of eight cards, half the deck covered).
    assert!(result.confidence < 1.0);
    assert_eq!(result.confidence, 0.3379);
}

#[test]
fn picks_the_best_matching_deck_variant_under_an_archetype() {
    let variant_1 = vec![spell("a", 1, 4), spell("b", 2, 4)];
    let variant_2 = vec![spell("a", 1, 4), spell("c", 3, 4)];
    let candidates = vec![archetype(1, vec![variant_1, variant_2])];
    let observed = observe(&[(1, 4), (3, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert_eq!(result.archetype_id, 1);
    assert_eq!(result.list_index, 1);
}

#[test]
fn returns_null_when_no_deck_has_any_card_overlap() {
    let candidates = vec![archetype(1, vec![vec![spell("a", 1, 4)]])];
    let mut table = table_of(&candidates);
    table.push(spell("unrelated", 999, 1));
    let observed = observe(&[(999, 4)], &table);

    assert_eq!(score(&observed, &candidates), None);
}

#[test]
fn ignores_basic_lands_when_matching() {
    let murktide = archetype(
        1,
        vec![vec![
            card("island", 93534, 4, "Basic Land - Island"),
            spell("murktide", 500, 4),
            spell("dragon", 501, 4),
        ]],
    );
    let candidates = vec![murktide];
    let observed = observe(&[(93534, 3)], &table_of(&candidates));

    assert_eq!(score(&observed, &candidates), None);
}

#[test]
fn uses_nonbasic_lands_to_discriminate_land_defined_archetypes_like_tron() {
    let tron = archetype(
        1,
        vec![vec![
            card("tower", 300, 4, "Land - Urza's Tower"),
            card("mine", 301, 4, "Land - Urza's Mine"),
            card("plant", 302, 4, "Land - Urza's Power-Plant"),
            card("karn", 303, 3, "Legendary Planeswalker - Karn"),
        ]],
    );
    let burn = archetype(2, vec![vec![spell("bolt", 100, 4)]]);
    let candidates = vec![tron, burn];
    let observed = observe(&[(300, 2), (301, 2), (302, 1)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert_eq!(result.archetype_id, 1);
}

#[test]
fn does_not_count_lands_toward_quantity_overlap() {
    let burn = archetype(
        1,
        vec![vec![
            spell("bolt", 100, 4),
            spell("spike", 101, 4),
            card("mountain", 102, 12, "Basic Land"),
        ]],
    );
    let candidates = vec![burn];
    let observed = observe(&[(100, 4), (101, 4), (102, 20)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert_eq!(result.archetype_id, 1);
    assert!(result.confidence > 0.9);
}

#[test]
fn skips_archetypes_with_no_decks() {
    let candidates = vec![archetype(1, Vec::new())];
    let observed = observe(&[(1, 4)], &[spell("a", 1, 1)]);

    assert_eq!(score(&observed, &candidates), None);
}

fn large_deck() -> Candidate {
    let mut list = Vec::new();
    for i in 1..=15 {
        list.push(spell(&format!("c{i}"), 1000 + i, 4));
    }
    archetype(1, vec![list])
}

#[test]
fn does_not_short_circuit_on_thin_observations_of_a_large_deck() {
    let candidates = vec![large_deck()];
    let observed = observe(&[(1001, 4), (1002, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert!(result.confidence < 0.8);
}

#[test]
fn still_short_circuits_when_many_distinct_cards_of_a_large_deck_are_observed() {
    let candidates = vec![large_deck()];
    let input: Vec<(i64, u32)> = (1..=9).map(|i| (1000 + i, 4)).collect();
    let observed = observe(&input, &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert!(result.confidence >= 0.8);
}

#[test]
fn stays_confident_when_a_small_deck_is_fully_observed() {
    let candidates = vec![archetype(
        1,
        vec![vec![spell("a", 2001, 4), spell("b", 2002, 4)]],
    )];
    let observed = observe(&[(2001, 4), (2002, 4)], &table_of(&candidates));

    let result = score(&observed, &candidates).expect("an estimate");

    assert!(result.confidence >= 0.8);
}

#[test]
fn duplicate_input_rows_are_collapsed_by_catalog_id() {
    let candidates = vec![archetype(1, vec![vec![spell("a", 1, 4), spell("b", 2, 4)]])];
    let split = observe(&[(1, 2), (1, 2), (2, 4)], &table_of(&candidates));
    let whole = observe(&[(1, 4), (2, 4)], &table_of(&candidates));

    assert_eq!(score(&split, &candidates), score(&whole, &candidates));
}

/// #300 F10 (pinned): a card in a list's maindeck and its sideboard counts
/// its copies summed, so 2 main and 2 side score as 4 main.
#[test]
fn a_lists_main_and_side_copies_of_a_card_are_summed() {
    let mut side_bolts = spell("bolt", 100, 2);
    side_bolts.sideboard = true;
    let split = vec![archetype(
        1,
        vec![vec![
            spell("bolt", 100, 2),
            side_bolts,
            spell("spike", 101, 4),
        ]],
    )];
    let whole = vec![archetype(
        1,
        vec![vec![spell("bolt", 100, 4), spell("spike", 101, 4)]],
    )];
    let observed = observe(&[(100, 4), (101, 4)], &table_of(&whole));

    let summed = score(&observed, &split).expect("an estimate");
    assert_eq!(Some(summed.clone()), score(&observed, &whole));
    assert_eq!(summed.confidence, 1.0);
}
