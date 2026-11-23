//! Ports 0.x `ComputeCrossDraftCardStatsTest`, the pure cases of
//! `BuildLimitedCardRowsTest` and the sealed card rows case of
//! `SealedPoolTest`.

mod common;

use std::collections::BTreeMap;

use common::{card, draft, facts, named, pack_pick, pick, reg, registered};
use limited::{
    CardFacts, CardFactsMap, CardRowsInput, CardRowsSummary, CardStatus, CastStats,
    CrossDraftStats, Draft, OtherDraft, Pick, RegisteredDeck, card_name, cross_draft_card_stats,
    limited_card_rows, registered_cards_from_json,
};
use serde_json::json;

/// A pick with its pack and pick numbers.
fn numbered(ordinal: u32, pack: u32, number: u32, available: &[u32], picked: u32) -> Pick {
    Pick {
        pack_number: pack,
        pick_number: number,
        cards_available: available.to_vec(),
        picked_catalog_id: Some(picked),
        ..pick(ordinal)
    }
}

fn creature(name: &str, oracle_id: &str) -> CardFacts {
    CardFacts {
        card_type: Some("Creature".to_string()),
        ..named(name, oracle_id, "HOB")
    }
}

fn no_cross() -> BTreeMap<String, CrossDraftStats> {
    BTreeMap::new()
}

fn no_cast() -> BTreeMap<String, CastStats> {
    BTreeMap::new()
}

fn rows_for(
    draft: Option<&Draft>,
    decks: &[RegisteredDeck],
    cross: &BTreeMap<String, CrossDraftStats>,
    cast: &BTreeMap<String, CastStats>,
    games_played: u32,
    cards: &CardFactsMap,
) -> limited::LimitedCardRows {
    limited_card_rows(&CardRowsInput {
        is_sealed: false,
        draft,
        decks,
        stated_pool: None,
        cross_draft: cross,
        cast_stats: cast,
        games_played,
        cards,
    })
}

#[test]
fn builds_one_row_per_drafted_card_with_status_seen_and_wheel_facts() {
    let cards = facts(vec![
        (1, creature("Qard", "bard")),
        (2, creature("Qarper", "harper")),
    ]);
    let mut picks = vec![
        numbered(1, 1, 1, &[1, 2], 1),
        numbered(9, 1, 9, &[2], 2),
        numbered(21, 2, 7, &[1], 1),
    ];
    picks[0].pack_id = Some(500);
    picks[1].pack_id = Some(500);
    picks[2].pack_id = Some(600);
    let draft = draft("q-draft", picks);
    let decks = vec![registered("q-m1", "s", 10, vec![reg(1, 2, false)])];

    let result = rows_for(Some(&draft), &decks, &no_cross(), &no_cast(), 0, &cards);

    assert_eq!(result.rows.len(), 2);
    let first = &result.rows[0];
    assert_eq!(first.catalog_id, 1);
    assert_eq!(first.oracle_id.as_deref(), Some("bard"));
    assert_eq!(first.ordinals, vec![1, 21]);
    assert_eq!(first.labels, vec!["P1p1", "P2p7"]);
    assert_eq!(first.status, CardStatus::Main);
    assert_eq!(
        (
            first.games_cast,
            first.cast_won,
            first.cast_lost,
            first.win_pct_cast
        ),
        (0, 0, 0, None)
    );
    assert_eq!((first.seen_count, first.wheeled), (2, false));
    let second = &result.rows[1];
    assert_eq!(second.catalog_id, 2);
    assert_eq!(second.ordinals, vec![9]);
    assert_eq!(second.labels, vec!["P1p9"]);
    assert_eq!(second.status, CardStatus::Cut);
    assert_eq!((second.seen_count, second.wheeled), (2, true));
    assert_eq!(
        result.summary,
        CardRowsSummary {
            distinct: 2,
            games: 0,
            other_drafts: 0,
        }
    );
    assert_eq!(card_name(&cards, 1), "Qard");
}

#[test]
fn reports_every_drafted_card_as_pool_while_nothing_is_registered_yet() {
    let cards = facts(vec![(1, creature("Qard", "bard"))]);
    let draft = draft("q-draft", vec![numbered(1, 1, 1, &[1], 1)]);

    let result = rows_for(Some(&draft), &[], &no_cross(), &no_cast(), 0, &cards);

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].status, CardStatus::Pool);
}

#[test]
fn returns_an_empty_payload_for_a_run_with_no_draft() {
    for is_sealed in [true, false] {
        let result = limited_card_rows(&CardRowsInput {
            is_sealed,
            draft: None,
            decks: &[],
            stated_pool: None,
            cross_draft: &no_cross(),
            cast_stats: &no_cast(),
            games_played: 0,
            cards: &facts(vec![]),
        });

        assert!(result.rows.is_empty());
        assert_eq!(result.summary, CardRowsSummary::default());
    }
}

#[test]
fn carries_prior_draft_facts_and_counts_the_other_drafts_they_came_from() {
    let cards = facts(vec![
        (1, creature("Qarper", "harper")),
        (2, creature("Qard", "bard")),
    ]);
    let current = draft("q-current", vec![numbered(1, 1, 1, &[1, 2], 1)]);
    let other = draft("q-other", vec![numbered(3, 1, 3, &[1, 2], 1)]);
    let cross = cross_draft_card_stats(
        Some("HOB"),
        &[OtherDraft {
            set_code: Some("HOB"),
            draft: &other,
            last_registered: None,
        }],
        &cards,
    );

    let result = rows_for(Some(&current), &[], &cross, &no_cast(), 0, &cards);

    let row = &result.rows[0];
    assert_eq!(row.catalog_id, 1);
    assert_eq!(
        (
            row.prior_taken,
            row.prior_avg_ordinal,
            row.prior_wheeled,
            row.prior_drafts
        ),
        (1, Some(3.0), 0, 1)
    );
    assert_eq!(result.summary.other_drafts, 1);
}

#[test]
fn reads_cast_game_stats_by_oracle_id() {
    let cards = facts(vec![(1, creature("Qard", "bard"))]);
    let draft = draft("q-draft", vec![numbered(1, 1, 1, &[1], 1)]);
    let cast = BTreeMap::from([(
        "bard".to_string(),
        CastStats {
            games_cast: 2,
            cast_won: 1,
            cast_lost: 1,
        },
    )]);

    let result = rows_for(Some(&draft), &[], &no_cross(), &cast, 2, &cards);

    let row = &result.rows[0];
    assert_eq!(
        (
            row.games_cast,
            row.cast_won,
            row.cast_lost,
            row.win_pct_cast
        ),
        (2, 1, 1, Some(50))
    );
    assert_eq!(result.summary.games, 2);
}

#[test]
fn an_unknown_card_has_a_row_and_a_hash_name() {
    let draft = draft("q-draft", vec![numbered(1, 1, 1, &[42], 42)]);

    let result = rows_for(
        Some(&draft),
        &[],
        &no_cross(),
        &no_cast(),
        0,
        &facts(vec![]),
    );

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].oracle_id, None);
    assert_eq!(card_name(&facts(vec![]), 42), "#42");
}

#[test]
fn builds_one_card_row_per_sealed_pool_card_tagging_the_added_booster() {
    let cards = facts(vec![
        (1, card("Creature", "W")),
        (2, card("Creature", "U")),
        (6, card("Creature", "R")),
        (9, card("Basic Land", "")),
    ]);
    let first = vec![reg(1, 1, false), reg(9, 17, false), reg(2, 1, true)];
    let mut later = first.clone();
    later.push(reg(6, 1, true));
    let decks = vec![
        registered("q-m1", "p1", 10, first),
        registered("q-m4", "p2", 40, later),
    ];

    let result = limited_card_rows(&CardRowsInput {
        is_sealed: true,
        draft: None,
        decks: &decks,
        stated_pool: None,
        cross_draft: &no_cross(),
        cast_stats: &no_cast(),
        games_played: 0,
        cards: &cards,
    });

    let by_id: BTreeMap<u32, &limited::LimitedCardRow> = result
        .rows
        .iter()
        .map(|row| (row.catalog_id, row))
        .collect();
    assert_eq!(by_id.keys().copied().collect::<Vec<_>>(), vec![1, 2, 6]);
    assert_eq!(
        (by_id[&1].status, by_id[&1].added, by_id[&1].labels.len()),
        (CardStatus::Main, false, 0)
    );
    assert_eq!(
        (by_id[&2].status, by_id[&2].added),
        (CardStatus::Side, false)
    );
    assert_eq!(
        (by_id[&6].status, by_id[&6].added),
        (CardStatus::Side, true)
    );
    assert_eq!(result.summary.distinct, 3);
}

/// An other draft of the set, linked to a run (0.x `hobLeagueWithDraft`).
fn other<'a>(
    set_code: &'a str,
    draft: &'a Draft,
    last_registered: Option<&'a [limited::RegisteredCard]>,
) -> OtherDraft<'a> {
    OtherDraft {
        set_code: Some(set_code),
        draft,
        last_registered,
    }
}

#[test]
fn aggregates_taken_passed_and_wheeled_per_oracle_across_other_drafts_of_the_set() {
    // Two printings of Harper share an oracle id.
    let cards = facts(vec![
        (1, named("Qarper", "harper", "HOB")),
        (11, named("Qarper", "harper", "HOB")),
        (2, named("Qard", "bard", "HOB")),
    ]);
    let draft_a = draft(
        "q-a",
        vec![pack_pick(1, 500, &[1, 2, 3], 2), pack_pick(9, 500, &[1], 1)],
    );
    let snapshot_a = vec![reg(2, 1, false), reg(1, 1, true)];
    let draft_b = draft("q-b", vec![numbered(4, 1, 4, &[11, 2], 11)]);
    let draft_msh = draft("q-msh", vec![numbered(1, 1, 1, &[2], 2)]);

    let stats = cross_draft_card_stats(
        Some("HOB"),
        &[
            other("HOB", &draft_a, Some(&snapshot_a)),
            other("HOB", &draft_b, None),
            other("MSH", &draft_msh, None),
        ],
        &cards,
    );

    assert_eq!(
        stats["harper"],
        CrossDraftStats {
            oracle_id: "harper".to_string(),
            drafts: 2,
            times_taken: 2,
            avg_ordinal: Some(6.5),
            times_passed: 1,
            times_wheeled: 1,
            made_deck: 0,
        }
    );
    assert_eq!(
        stats["bard"],
        CrossDraftStats {
            oracle_id: "bard".to_string(),
            drafts: 2,
            times_taken: 1,
            avg_ordinal: Some(1.0),
            times_passed: 1,
            times_wheeled: 0,
            made_deck: 1,
        }
    );
    assert_eq!(stats.len(), 2);
}

#[test]
fn returns_nothing_when_the_run_has_no_set_code() {
    let cards = facts(vec![(2, named("Qard", "bard", "HOB"))]);
    let draft_a = draft("q-a", vec![numbered(1, 1, 1, &[2], 2)]);

    assert!(cross_draft_card_stats(None, &[other("HOB", &draft_a, None)], &cards).is_empty());
}

#[test]
fn emits_a_complete_row_for_a_pick_whose_card_is_missing_from_its_own_pack() {
    let cards = facts(vec![(7, named("Qhost", "ghost", "HOB"))]);
    // A commit can name a card that never appeared in the pack it recorded.
    let draft_a = draft("q-a", vec![numbered(3, 1, 3, &[], 7)]);
    let draft_b = draft("q-b", vec![numbered(5, 1, 5, &[7], 7)]);

    let stats = cross_draft_card_stats(
        Some("HOB"),
        &[other("HOB", &draft_a, None), other("HOB", &draft_b, None)],
        &cards,
    );

    assert_eq!(
        stats["ghost"],
        CrossDraftStats {
            oracle_id: "ghost".to_string(),
            drafts: 1,
            times_taken: 1,
            avg_ordinal: Some(4.0),
            times_passed: 0,
            times_wheeled: 0,
            made_deck: 0,
        }
    );
}

#[test]
fn skips_registered_snapshot_rows_that_carry_no_catalog_id() {
    let cards = facts(vec![(2, named("Qard", "bard", "HOB"))]);
    let draft_a = draft("q-a", vec![numbered(1, 1, 1, &[2], 2)]);
    let snapshot = registered_cards_from_json(&json!([
        {"catalog_id": 2, "quantity": 1, "sideboard": false},
        {"quantity": 1, "sideboard": false}
    ]));

    let stats = cross_draft_card_stats(
        Some("HOB"),
        &[other("HOB", &draft_a, Some(&snapshot))],
        &cards,
    );

    assert_eq!(stats.len(), 1);
    assert_eq!(stats["bard"].made_deck, 1);
}

#[test]
fn cards_without_an_oracle_id_are_left_out() {
    let draft_a = draft("q-a", vec![numbered(1, 1, 1, &[2, 3], 2)]);

    let stats = cross_draft_card_stats(
        Some("HOB"),
        &[other("HOB", &draft_a, None)],
        &facts(vec![(2, card("Creature", "W"))]),
    );

    assert!(stats.is_empty());
}
