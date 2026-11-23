//! The detection decision (spec section 6.1): 0.x `DetermineDeckArchetypeTest`
//! and `OfflineMode/ArchetypeFloorTest` decision cases, with the API asked
//! first online (catalog picker addendum, #489).

use archetypes::{
    Candidate, ConditionKind, Decision, Definition, Estimate, ListCard, LocalAnswers, ObservedCard,
    Source, decide, decide_after_empty_estimate, local_answers,
};

fn answers(opponent: bool, rules: Option<(i64, f64)>, score: Option<(i64, f64)>) -> LocalAnswers {
    LocalAnswers {
        opponent,
        rules,
        score: score.map(|(archetype_id, confidence)| Estimate {
            archetype_id,
            list_index: 0,
            confidence,
        }),
        fallback: None,
        score_is_manual: false,
    }
}

fn manual(mut local: LocalAnswers) -> LocalAnswers {
    local.score_is_manual = true;
    local
}

#[test]
fn uses_local_match_when_confidence_is_above_threshold_offline() {
    let decision = decide(answers(true, None, Some((7, 0.8))), false);

    assert_eq!(
        decision,
        Decision::Accept {
            archetype_id: 7,
            confidence: 0.8
        }
    );
}

// #489: v1 holds only the archetypes it has met, so a confident local
// answer may be the only held one of its colours; the API knows them all.
#[test]
fn asks_the_api_before_a_confident_local_answer_while_online() {
    assert_eq!(
        decide(answers(true, None, Some((7, 0.95))), true),
        Decision::AskApi
    );
    assert_eq!(
        decide(answers(false, Some((3, 1.0)), Some((7, 0.95))), true),
        Decision::AskApi
    );
}

#[test]
fn a_confident_manual_archetype_wins_without_the_api_for_the_player() {
    let decision = decide(manual(answers(false, None, Some((7, 0.8)))), true);

    assert_eq!(
        decision,
        Decision::Accept {
            archetype_id: 7,
            confidence: 0.8
        }
    );
}

// A broad custom list (or one imported from 0.x) must not override the
// server for every opponent: the manual shortcut is the player's deck only.
#[test]
fn the_opponent_seat_asks_the_api_even_over_a_confident_manual_archetype() {
    assert_eq!(
        decide(manual(answers(true, None, Some((7, 1.0)))), true),
        Decision::AskApi
    );
}

#[test]
fn a_manual_archetype_below_the_threshold_still_asks_the_api() {
    assert_eq!(
        decide(manual(answers(true, None, Some((7, 0.79)))), true),
        Decision::AskApi
    );
}

#[test]
fn falls_back_to_api_when_local_confidence_is_too_low() {
    assert_eq!(
        decide(answers(true, None, Some((7, 0.79))), true),
        Decision::AskApi
    );
}

#[test]
fn falls_back_to_api_when_no_local_archetypes_exist() {
    assert_eq!(decide(answers(false, None, None), true), Decision::AskApi);
}

#[test]
fn an_exact_rules_match_wins_over_the_score() {
    let decision = decide(answers(true, Some((3, 1.0)), Some((7, 0.95))), false);

    assert_eq!(
        decision,
        Decision::Accept {
            archetype_id: 3,
            confidence: 1.0
        }
    );
}

#[test]
fn returns_a_sub_threshold_local_estimate_while_offline_instead_of_nothing() {
    let decision = decide(answers(false, None, Some((7, 0.25))), false);

    assert_eq!(
        decision,
        Decision::AcceptOffline {
            archetype_id: 7,
            confidence: 0.25
        }
    );
}

#[test]
fn never_asks_the_api_while_offline() {
    for local in [
        answers(true, None, None),
        answers(true, None, Some((7, 0.1))),
        answers(false, None, None),
    ] {
        assert_ne!(decide(local, false), Decision::AskApi);
    }
}

#[test]
fn still_defers_to_the_api_below_the_threshold_while_online() {
    assert_eq!(
        decide(answers(false, None, Some((7, 0.25))), true),
        Decision::AskApi
    );
}

#[test]
fn offline_without_a_score_takes_the_fallback_definition() {
    let mut local = answers(true, None, None);
    local.fallback = Some((9, 0.5));

    assert_eq!(
        decide(local, false),
        Decision::AcceptOffline {
            archetype_id: 9,
            confidence: 0.5
        }
    );
}

#[test]
fn offline_prefers_any_score_over_the_fallback_definition() {
    let mut local = answers(true, None, Some((7, 0.1)));
    local.fallback = Some((9, 0.5));

    assert_eq!(
        decide(local, false),
        Decision::AcceptOffline {
            archetype_id: 7,
            confidence: 0.1
        }
    );
}

// The API answered and named no archetype (a 200 `[]`): only a confident
// local answer stands, never the any-confidence offline one, which would
// label an opponent wrongly for good.
#[test]
fn an_empty_estimate_keeps_a_rules_match_or_a_confident_score() {
    assert_eq!(
        decide_after_empty_estimate(answers(true, Some((3, 1.0)), Some((7, 0.2)))),
        Decision::Accept {
            archetype_id: 3,
            confidence: 1.0
        }
    );
    assert_eq!(
        decide_after_empty_estimate(answers(false, None, Some((7, 0.8)))),
        Decision::Accept {
            archetype_id: 7,
            confidence: 0.8
        }
    );
}

#[test]
fn an_empty_estimate_drops_a_weak_score_and_the_fallback() {
    let mut opponent = answers(true, None, Some((7, 0.79)));
    opponent.fallback = Some((9, 0.5));
    assert_eq!(decide_after_empty_estimate(opponent), Decision::Homebrew);

    let mut player = answers(false, None, Some((7, 0.79)));
    player.fallback = Some((9, 0.5));
    assert_eq!(decide_after_empty_estimate(player), Decision::Nothing);
}

#[test]
fn returns_null_offline_when_there_are_no_candidate_decks_at_all() {
    // The player gets no row; the opponent gets Homebrew (0.x
    // `DetermineMatchArchetypes`).
    assert_eq!(decide(answers(false, None, None), false), Decision::Nothing);
    assert_eq!(decide(answers(true, None, None), false), Decision::Homebrew);
}

fn seen(catalog_id: i64, name: &str) -> ObservedCard {
    ObservedCard {
        catalog_id,
        quantity: 4,
        sideboard: false,
        oracle_id: Some(format!("q-oracle-{catalog_id}")),
        name: Some(name.to_string()),
        card_type: Some("Instant".to_string()),
        color_identity: Some("U".to_string()),
        mana_cost: None,
    }
}

#[test]
fn local_answers_gather_the_rules_the_score_and_the_fallback() {
    let observed = vec![seen(1, "Q One"), seen(2, "Q Two")];
    let ruled = Candidate {
        archetype_id: 1,
        uuid: "a".to_string(),
        source: Source::Downloaded,
        color_identity: Some("U".to_string()),
        definition: Some(Definition {
            rank: vec![1.0],
            conditions: vec![archetypes::Condition {
                kind: ConditionKind::InMainboard,
                cards: vec!["Q One".to_string()],
            }],
            ..Definition::default()
        }),
        lists: Vec::new(),
    };
    let listed = Candidate {
        archetype_id: 2,
        uuid: "b".to_string(),
        source: Source::Manual,
        color_identity: Some("U".to_string()),
        definition: None,
        lists: vec![vec![ListCard {
            catalog_id: 1,
            oracle_id: Some("q-oracle-1".to_string()),
            card_type: Some("Instant".to_string()),
            quantity: 4,
            sideboard: false,
        }]],
    };
    let fallback = Candidate {
        archetype_id: 3,
        uuid: "c".to_string(),
        source: Source::Downloaded,
        color_identity: None,
        definition: Some(Definition {
            fallback: true,
            common_cards: vec!["Q Two".to_string()],
            min_overlap: 0.1,
            ..Definition::default()
        }),
        lists: Vec::new(),
    };

    let local = local_answers(&observed, &[ruled, listed, fallback], true);

    assert!(local.opponent);
    assert_eq!(local.rules, Some((1, 1.0)));
    assert_eq!(local.score.map(|s| s.archetype_id), Some(2));
    assert!(local.score_is_manual, "the score's archetype is manual");
    assert_eq!(local.fallback, Some((3, 0.5)));
}
