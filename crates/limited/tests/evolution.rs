//! Ports 0.x `BuildDeckEvolutionTest`.

mod common;

use std::collections::BTreeMap;

use common::{card, drafted, facts, reg, registered};
use limited::{
    CardFactsMap, CardQuantity, CardStatus, CompletedMatch, DeckEvolution, EvolutionInput,
    PoolCard, RegisteredCard, RegisteredDeck, RunPool, ZoneDiff, deck_evolution, pool_statuses,
    registered_cards_from_json, run_pool,
};
use serde_json::json;

fn quantity(catalog_id: u32, quantity: u32) -> CardQuantity {
    CardQuantity {
        catalog_id,
        quantity,
    }
}

fn tokens(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// Every pool card of a set of groups, by catalog id.
fn pool_cards(groups: &[limited::PoolGroup]) -> BTreeMap<u32, PoolCard> {
    groups
        .iter()
        .flat_map(|group| group.cards.iter().cloned())
        .map(|card| (card.catalog_id, card))
        .collect()
}

/// 0.x `limitedDeckFixture`: a HOB draft run with two matches (a loss, then
/// a win with two games) and two registered versions.
struct DeckFixture {
    cards: CardFactsMap,
    pool: RunPool,
    decks: Vec<RegisteredDeck>,
    match_order: Vec<String>,
    completed: Vec<CompletedMatch>,
}

impl DeckFixture {
    fn new() -> Self {
        let cards = facts(vec![
            (1, card("Creature", "W")),
            (2, card("Creature", "U")),
            (3, card("Instant", "U")),
            (4, card("Creature", "U")),
            (5, card("Artifact", "")),
            (9, card("Basic Land", "")),
        ]);
        let draft = drafted(&[1, 1, 2, 3, 4, 5]);
        let v1 = vec![
            reg(1, 1, false),
            reg(2, 1, false),
            reg(9, 17, false),
            reg(3, 1, true),
            reg(5, 1, true),
        ];
        let v2 = vec![
            reg(1, 2, false),
            reg(2, 1, false),
            reg(9, 16, false),
            reg(3, 1, true),
            reg(5, 1, true),
        ];
        let decks = vec![
            registered("q-m1", "v1", 10, v1),
            registered("q-m2", "v2", 30, v2.clone()),
        ];
        let game_two = vec![
            reg(1, 2, false),
            reg(2, 0, false),
            reg(3, 1, false),
            reg(9, 16, false),
        ];
        let completed = vec![
            CompletedMatch {
                token: "q-m1".to_string(),
                won: Some(false),
                opponent_name: Some("Qorin".to_string()),
                game_decks: Vec::new(),
            },
            CompletedMatch {
                token: "q-m2".to_string(),
                won: Some(true),
                opponent_name: Some("Qadira".to_string()),
                game_decks: vec![Some(v2), Some(game_two)],
            },
        ];
        let pool = run_pool(false, Some(&draft), &decks, &cards, None);

        DeckFixture {
            cards,
            pool,
            decks,
            match_order: tokens(&["q-m1", "q-m2"]),
            completed,
        }
    }

    fn evolution(&self) -> DeckEvolution {
        deck_evolution(&EvolutionInput {
            pool: &self.pool,
            decks: &self.decks,
            match_order: &self.match_order,
            completed: &self.completed,
            cards: &self.cards,
        })
    }
}

#[test]
fn builds_versions_with_diffs_pool_status_and_per_game_sideboarding() {
    let evolution = DeckFixture::new().evolution();

    let summary = &evolution.summary;
    assert_eq!(
        (
            summary.drafted,
            summary.main_spells,
            summary.basics,
            summary.sideboard,
            summary.version_count
        ),
        (6, 3, 16, 2, 2)
    );
    assert_eq!(evolution.versions.len(), 2);
    assert!(evolution.versions[1].is_current);
    assert!(!evolution.versions[0].is_current);
    assert_eq!(
        evolution.versions[1].diff_main,
        ZoneDiff {
            added: vec![quantity(1, 1)],
            removed: vec![quantity(9, 1)],
        }
    );
    assert_eq!(evolution.versions[0].diff_main, ZoneDiff::default());
    assert_eq!(evolution.versions[0].colors, "WU");

    let by_id = pool_cards(&evolution.pool_groups);
    assert_eq!(by_id[&1].status, CardStatus::Main);
    assert_eq!(by_id[&1].main_qty, 2);
    assert_eq!(by_id[&3].status, CardStatus::Side);
    assert_eq!(by_id[&4].status, CardStatus::Cut);
    assert!(!by_id.contains_key(&9));
    let blue = evolution
        .pool_groups
        .iter()
        .find(|group| group.key == "U")
        .unwrap();
    assert_eq!((blue.label.as_str(), blue.count), ("Blue", 3));

    let won = evolution
        .games
        .iter()
        .find(|changes| changes.result.as_deref() == Some("W"))
        .unwrap();
    assert_eq!(won.label, "Match 2");
    assert_eq!(won.opponent_name.as_deref(), Some("Qadira"));
    assert_eq!(won.games[0].note.as_deref(), Some("registered deck"));
    assert_eq!(won.games[1].added, vec![quantity(3, 1)]);
    assert_eq!(won.games[1].removed, vec![quantity(2, 1)]);
    assert_eq!(won.games[1].note, None);
    let lost = &evolution.games[0];
    assert_eq!(
        (lost.label.as_str(), lost.result.as_deref()),
        ("Match 1", Some("L"))
    );
}

#[test]
fn marks_every_pool_card_as_pool_while_nothing_is_registered() {
    let cards = facts(vec![(1, card("Creature", "W")), (2, card("Creature", "U"))]);
    let pool = run_pool(false, Some(&drafted(&[1, 2])), &[], &cards, None);

    let evolution = deck_evolution(&EvolutionInput {
        pool: &pool,
        decks: &[],
        match_order: &[],
        completed: &[],
        cards: &cards,
    });

    let statuses: Vec<CardStatus> = pool_cards(&evolution.pool_groups)
        .values()
        .map(|card| card.status)
        .collect();
    assert_eq!(statuses, vec![CardStatus::Pool, CardStatus::Pool]);
}

#[test]
fn folds_consecutive_identical_snapshots_into_one_version_covering_both_matches() {
    let cards = facts(vec![
        (1, card("Creature", "W")),
        (2, card("Creature", "U")),
        (3, card("Instant", "U")),
    ]);
    let v1 = vec![reg(1, 1, false), reg(3, 1, true)];
    let v2 = vec![reg(1, 1, false), reg(2, 1, false)];
    let decks = vec![
        registered("q-m1", "v1", 10, v1),
        registered("q-m2", "v2", 20, v2.clone()),
        registered("q-m3", "v2", 30, v2),
    ];
    let pool = RunPool::default();

    let evolution = deck_evolution(&EvolutionInput {
        pool: &pool,
        decks: &decks,
        match_order: &tokens(&["q-m1", "q-m2", "q-m3"]),
        completed: &[],
        cards: &cards,
    });

    assert_eq!(evolution.versions.len(), 2);
    assert_eq!(evolution.summary.version_count, 2);
    assert_eq!(evolution.versions[0].match_labels, tokens(&["Match 1"]));
    assert_eq!(
        evolution.versions[1].match_tokens,
        tokens(&["q-m2", "q-m3"])
    );
    assert_eq!(
        evolution.versions[1].match_labels,
        tokens(&["Match 2", "Match 3"])
    );
    assert!(evolution.versions[1].is_current);
    assert_eq!(
        evolution.versions[1].diff_main,
        ZoneDiff {
            added: vec![quantity(2, 1)],
            removed: Vec::new(),
        }
    );
    assert_eq!(
        evolution.versions[1].diff_side,
        ZoneDiff {
            added: Vec::new(),
            removed: vec![quantity(3, 1)],
        }
    );
}

#[test]
fn numbers_matches_across_the_whole_league_including_one_still_in_progress() {
    let cards = facts(vec![(1, card("Creature", "W"))]);
    let decks = vec![registered("q-live", "v1", 55, vec![reg(1, 1, false)])];
    let completed = vec![
        CompletedMatch {
            token: "q-m1".to_string(),
            won: Some(true),
            opponent_name: None,
            game_decks: Vec::new(),
        },
        CompletedMatch {
            token: "q-m2".to_string(),
            won: Some(false),
            opponent_name: None,
            game_decks: Vec::new(),
        },
    ];

    let evolution = deck_evolution(&EvolutionInput {
        pool: &RunPool::default(),
        decks: &decks,
        match_order: &tokens(&["q-m1", "q-m2", "q-live"]),
        completed: &completed,
        cards: &cards,
    });

    assert_eq!(evolution.versions[0].match_labels, tokens(&["Match 3"]));
    assert_eq!(evolution.games.len(), 2);
}

#[test]
fn a_registered_deck_for_an_unknown_match_is_labelled_with_a_question_mark() {
    let decks = vec![registered("q-gone", "v1", 5, vec![reg(1, 1, false)])];

    let evolution = deck_evolution(&EvolutionInput {
        pool: &RunPool::default(),
        decks: &decks,
        match_order: &[],
        completed: &[],
        cards: &facts(vec![]),
    });

    assert_eq!(evolution.versions[0].match_labels, tokens(&["Match ?"]));
}

#[test]
fn reports_no_board_data_for_game_1_of_a_match_with_no_registered_snapshot() {
    let completed = vec![CompletedMatch {
        token: "q-m1".to_string(),
        won: Some(true),
        opponent_name: None,
        game_decks: vec![None],
    }];

    let evolution = deck_evolution(&EvolutionInput {
        pool: &RunPool::default(),
        decks: &[],
        match_order: &tokens(&["q-m1"]),
        completed: &completed,
        cards: &facts(vec![]),
    });

    assert_eq!(
        evolution.games[0].games[0].note.as_deref(),
        Some("no board data")
    );
}

#[test]
fn a_later_game_without_board_data_says_so() {
    let mut fixture = DeckFixture::new();
    fixture.completed[1].game_decks[1] = None;

    let evolution = fixture.evolution();

    assert_eq!(
        evolution.games[1].games[1].note.as_deref(),
        Some("no board data")
    );
}

#[test]
fn a_game_with_the_registered_board_has_no_changes() {
    let mut fixture = DeckFixture::new();
    let registered_main = fixture.decks[1].cards.clone();
    fixture.completed[1].game_decks[1] = Some(registered_main);

    let evolution = fixture.evolution();

    assert_eq!(
        evolution.games[1].games[1].note.as_deref(),
        Some("no changes")
    );
}

#[test]
fn returns_an_empty_structure_when_nothing_is_registered() {
    let evolution = deck_evolution(&EvolutionInput {
        pool: &RunPool::default(),
        decks: &[],
        match_order: &[],
        completed: &[],
        cards: &facts(vec![]),
    });

    assert!(evolution.versions.is_empty());
    assert_eq!(evolution.summary.version_count, 0);
    assert_eq!(evolution.summary.first_registered_at_ms, None);
    assert!(evolution.games.is_empty());
}

#[test]
fn derives_pool_statuses_without_games() {
    let fixture = DeckFixture::new();

    let expected: BTreeMap<u32, CardStatus> = pool_cards(&fixture.evolution().pool_groups)
        .into_iter()
        .map(|(id, card)| (id, card.status))
        .collect();

    assert_eq!(
        pool_statuses(&fixture.pool, &fixture.decks, &fixture.cards),
        expected
    );
}

#[test]
fn skips_snapshot_rows_that_carry_no_quantity() {
    let cards = facts(vec![(1, card("Creature", "W")), (2, card("Creature", "U"))]);
    let rows: Vec<RegisteredCard> = registered_cards_from_json(&json!([
        {"catalog_id": 1, "quantity": 2, "sideboard": false},
        {"catalog_id": 2, "sideboard": false}
    ]));
    let decks = vec![registered("q-m1", "v1", 30, rows)];

    let evolution = deck_evolution(&EvolutionInput {
        pool: &RunPool::default(),
        decks: &decks,
        match_order: &tokens(&["q-m1"]),
        completed: &[],
        cards: &cards,
    });

    assert_eq!(evolution.versions[0].main, 2);
    assert_eq!(evolution.summary.main_spells, 2);
}

#[test]
fn carries_a_pool_grouping_per_registered_version_so_older_builds_can_be_inspected() {
    let evolution = DeckFixture::new().evolution();

    let v1 = pool_cards(&evolution.versions[0].pool_groups);
    let v2 = pool_cards(&evolution.versions[1].pool_groups);

    assert_eq!(v1[&1].main_qty, 1);
    assert_eq!(v2[&1].main_qty, 2);
    assert_eq!(v1[&3].status, CardStatus::Side);
    assert_eq!(v1[&4].status, CardStatus::Cut);
    assert_eq!(evolution.versions[1].pool_groups, evolution.pool_groups);
    assert_eq!(
        evolution.versions[0].main_cards,
        vec![quantity(1, 1), quantity(2, 1), quantity(9, 17)]
    );
    assert_eq!(
        evolution.versions[1].side_cards,
        vec![quantity(3, 1), quantity(5, 1)]
    );
}

/// 0.x `sealedDeckFixture`: no draft, so the pool is whatever the registered
/// decks hold; the second deck carries the booster added after match 3.
fn sealed_fixture(both: bool) -> (CardFactsMap, Vec<RegisteredDeck>) {
    let cards = facts(vec![
        (1, card("Creature", "W")),
        (2, card("Creature", "U")),
        (3, card("Instant", "U")),
        (6, card("Creature", "R")),
        (9, card("Basic Land", "")),
    ]);
    let before = vec![
        reg(1, 1, false),
        reg(9, 17, false),
        reg(2, 1, true),
        reg(3, 1, true),
    ];
    let after = vec![
        reg(1, 1, false),
        reg(6, 1, false),
        reg(9, 16, false),
        reg(2, 1, true),
        reg(3, 2, true),
    ];
    let mut decks = vec![registered("q-m1", "s1", 10, before)];
    if both {
        decks.push(registered("q-m4", "s2", 40, after));
    }
    (cards, decks)
}

fn sealed_evolution(both: bool) -> DeckEvolution {
    let (cards, decks) = sealed_fixture(both);
    let pool = run_pool(true, None, &decks, &cards, None);
    deck_evolution(&EvolutionInput {
        pool: &pool,
        decks: &decks,
        match_order: &tokens(&["q-m1", "q-m4"]),
        completed: &[],
        cards: &cards,
    })
}

#[test]
fn builds_a_sealed_pool_from_the_registered_decks_and_tags_booster_cards_as_added() {
    let evolution = sealed_evolution(true);

    let by_id = pool_cards(&evolution.pool_groups);
    assert_eq!((evolution.summary.drafted, evolution.summary.added), (5, 2));
    assert!(!by_id.contains_key(&9));
    assert_eq!(
        (by_id[&1].quantity, by_id[&1].added, by_id[&1].status),
        (1, 0, CardStatus::Main)
    );
    assert_eq!(
        (by_id[&3].quantity, by_id[&3].added, by_id[&3].status),
        (2, 1, CardStatus::Side)
    );
    assert_eq!(
        (by_id[&6].quantity, by_id[&6].added, by_id[&6].status),
        (1, 1, CardStatus::Main)
    );
}

#[test]
fn tags_nothing_as_added_for_a_sealed_run_with_one_registered_deck() {
    let evolution = sealed_evolution(false);

    assert_eq!((evolution.summary.drafted, evolution.summary.added), (3, 0));
}

#[test]
fn never_tags_draft_pool_cards_as_added() {
    let evolution = DeckFixture::new().evolution();

    assert_eq!(evolution.summary.added, 0);
    assert!(
        pool_cards(&evolution.pool_groups)
            .values()
            .all(|card| card.added == 0)
    );
}

#[test]
fn unknown_cards_group_as_colourless_and_count_as_spells() {
    let pool = run_pool(false, Some(&drafted(&[77, 78])), &[], &facts(vec![]), None);
    let decks = vec![registered("q-m1", "v1", 10, vec![reg(77, 1, false)])];

    let evolution = deck_evolution(&EvolutionInput {
        pool: &pool,
        decks: &decks,
        match_order: &tokens(&["q-m1"]),
        completed: &[],
        cards: &facts(vec![]),
    });

    assert_eq!(evolution.pool_groups.len(), 1);
    assert_eq!(
        (
            evolution.pool_groups[0].key.as_str(),
            evolution.pool_groups[0].label.as_str()
        ),
        ("C", "Colourless")
    );
    assert_eq!(evolution.summary.main_spells, 1);
    assert_eq!(evolution.versions[0].colors, "");
    // Main first, then cut; within a status by catalog id.
    let order: Vec<(u32, CardStatus)> = evolution.pool_groups[0]
        .cards
        .iter()
        .map(|card| (card.catalog_id, card.status))
        .collect();
    assert_eq!(order, vec![(77, CardStatus::Main), (78, CardStatus::Cut)]);
}
