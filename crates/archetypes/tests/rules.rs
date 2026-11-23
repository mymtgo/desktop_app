//! The server's definitions and rules, evaluated as the API's
//! `ConditionEvaluator` and `ClassifyDeck` do (survey section 8, spec
//! sections 6.1 and 16.3).

use std::collections::BTreeSet;

use archetypes::{
    Candidate, Condition, ConditionKind, Definition, ObservedCard, Source, fallback_match,
    name_keys, parse_definition, rules_match, satisfies,
};

const SPEC_DEFINITION: &str = r#"{
  "uuid": "6f1c2e4a-0000-4000-8000-000000000001",
  "name": "Ruby Storm",
  "format": "modern",
  "color_identity": "R",
  "version": "9c0f64",
  "rank": [2, 1, 4, -17, 1, 2, 0],
  "conditions": [
    {"type": "InMainboard", "cards": ["Ral, Monsoon Mage"]},
    {"type": "OneOrMoreInMainboard", "cards": ["Ruby Medallion", "Past in Flames"]}
  ],
  "fallback": false,
  "common_cards": [],
  "min_overlap": 0.1,
  "colour_fork": "41:Base",
  "cards": [{"mtgo_id": 12345, "oracle_id": "q-oracle-1", "name": "Ral, Monsoon Mage", "quantity": 4, "sideboard": false}]
}"#;

#[test]
fn the_spec_definition_parses() {
    let definition = parse_definition(SPEC_DEFINITION).expect("parses");

    assert_eq!(definition.uuid, "6f1c2e4a-0000-4000-8000-000000000001");
    assert_eq!(definition.name, "Ruby Storm");
    assert_eq!(definition.format.as_deref(), Some("modern"));
    assert_eq!(definition.color_identity.as_deref(), Some("R"));
    assert_eq!(definition.version.as_deref(), Some("9c0f64"));
    assert_eq!(definition.rank, vec![2.0, 1.0, 4.0, -17.0, 1.0, 2.0, 0.0]);
    assert_eq!(definition.conditions.len(), 2);
    assert_eq!(definition.conditions[0].kind, ConditionKind::InMainboard);
    assert_eq!(
        definition.conditions[1].kind,
        ConditionKind::OneOrMoreInMainboard
    );
    assert_eq!(
        definition.conditions[1].cards,
        vec!["Ruby Medallion", "Past in Flames"]
    );
    assert!(!definition.fallback);
    assert_eq!(definition.min_overlap, 0.1);
    assert_eq!(definition.colour_fork.as_deref(), Some("41:Base"));
    assert_eq!(definition.cards.len(), 1);
    assert_eq!(definition.cards[0].mtgo_id, 12345);
    assert_eq!(definition.cards[0].oracle_id.as_deref(), Some("q-oracle-1"));
    assert_eq!(definition.cards[0].quantity, 4);
    assert!(!definition.cards[0].sideboard);
}

#[test]
fn a_definition_with_only_cards_parses() {
    let json = r#"{"cards": [{"mtgo_id": 7, "oracle_id": null, "name": "Q Card", "quantity": 2, "sideboard": true}]}"#;

    let definition = parse_definition(json).expect("parses");

    assert_eq!(definition.uuid, "");
    assert_eq!(definition.version, None);
    assert!(definition.rank.is_empty());
    assert!(definition.conditions.is_empty());
    assert!(!definition.fallback);
    assert_eq!(definition.min_overlap, 0.1);
    assert_eq!(definition.colour_fork, None);
    assert_eq!(definition.cards.len(), 1);
    assert_eq!(definition.cards[0].name.as_deref(), Some("Q Card"));
    assert!(definition.cards[0].sideboard);
}

#[test]
fn a_definition_round_trips_through_its_stored_json() {
    let definition = parse_definition(SPEC_DEFINITION).expect("parses");

    let stored = serde_json::to_string(&definition).expect("serialises");
    let again = parse_definition(&stored).expect("parses again");

    assert_eq!(again, definition);
    assert!(stored.contains(r#""type":"InMainboard""#));
}

#[test]
fn a_definition_that_is_not_json_is_an_error() {
    assert!(parse_definition("not json").is_err());
}

#[test]
fn condition_types_are_case_insensitive_and_unknown_ones_are_kept() {
    let json = r#"{"conditions": [
        {"type": "inmainboard", "cards": ["A"]},
        {"type": "TWOORMOREINSIDEBOARD", "cards": ["A", "B"]},
        {"type": "SomethingNew", "cards": ["A"]}
    ]}"#;

    let definition = parse_definition(json).expect("parses");

    assert_eq!(definition.conditions[0].kind, ConditionKind::InMainboard);
    assert_eq!(
        definition.conditions[1].kind,
        ConditionKind::TwoOrMoreInSideboard
    );
    assert_eq!(
        definition.conditions[2].kind,
        ConditionKind::Unknown("SomethingNew".to_string())
    );
}

#[test]
fn a_condition_without_a_type_fails_only_itself() {
    // the API reads `$condition['Type'] ?? ''`: an untyped condition never
    // passes, and the rest of the definition still parses.
    let json = r#"{"uuid": "q-untyped", "conditions": [
        {"cards": ["A"]},
        {"type": "InMainboard", "cards": ["A"]}
    ]}"#;

    let definition = parse_definition(json).expect("parses");

    assert_eq!(definition.uuid, "q-untyped");
    assert_eq!(
        definition.conditions[0].kind,
        ConditionKind::Unknown(String::new())
    );
    assert_eq!(definition.conditions[1].kind, ConditionKind::InMainboard);
    assert!(!satisfies(
        &definition.conditions[..1],
        &keys(&["A"]),
        &keys(&[])
    ));
}

fn keys(names: &[&str]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for name in names {
        for key in name_keys(name) {
            set.insert(key);
        }
    }
    set
}

fn condition(kind: ConditionKind, cards: &[&str]) -> Condition {
    Condition {
        kind,
        cards: cards.iter().map(|c| c.to_string()).collect(),
    }
}

fn passes(kind: ConditionKind, cards: &[&str], main: &[&str], side: &[&str]) -> bool {
    satisfies(&[condition(kind, cards)], &keys(main), &keys(side))
}

#[test]
fn every_condition_kind_matches_the_api() {
    use ConditionKind::*;
    let main = ["Ruby Medallion", "Fire // Ice"];
    let side = ["Past in Flames", "Q Sideboard Card"];

    // In*: Cards[0] only, the rest is ignored.
    assert!(passes(
        InMainboard,
        &["Ruby Medallion", "Nope"],
        &main,
        &side
    ));
    assert!(!passes(
        InMainboard,
        &["Nope", "Ruby Medallion"],
        &main,
        &side
    ));
    assert!(!passes(InMainboard, &["Past in Flames"], &main, &side));
    assert!(!passes(InMainboard, &[], &main, &side));
    assert!(passes(InSideboard, &["Past in Flames"], &main, &side));
    assert!(!passes(InSideboard, &["Ruby Medallion"], &main, &side));
    assert!(!passes(InSideboard, &[], &main, &side));
    assert!(passes(InMainOrSideboard, &["Past in Flames"], &main, &side));
    assert!(passes(
        InMainOrSideboard,
        &["ruby  medallion "],
        &main,
        &side
    ));
    assert!(!passes(
        InMainOrSideboard,
        &["Nope", "Ruby Medallion"],
        &main,
        &side
    ));

    // OneOrMore*: the whole list.
    assert!(passes(
        OneOrMoreInMainboard,
        &["Nope", "Ruby Medallion"],
        &main,
        &side
    ));
    assert!(!passes(
        OneOrMoreInMainboard,
        &["Nope", "Past in Flames"],
        &main,
        &side
    ));
    assert!(passes(
        OneOrMoreInSideboard,
        &["Nope", "Past in Flames"],
        &main,
        &side
    ));
    assert!(!passes(
        OneOrMoreInSideboard,
        &["Ruby Medallion"],
        &main,
        &side
    ));
    assert!(passes(
        OneOrMoreInMainOrSideboard,
        &["Nope", "Past in Flames"],
        &main,
        &side
    ));
    assert!(!passes(OneOrMoreInMainOrSideboard, &["Nope"], &main, &side));
    assert!(!passes(OneOrMoreInMainOrSideboard, &[], &main, &side));

    // TwoOrMore*: the whole list, two hits.
    assert!(passes(
        TwoOrMoreInMainboard,
        &["Ruby Medallion", "Fire", "Nope"],
        &main,
        &side
    ));
    assert!(!passes(
        TwoOrMoreInMainboard,
        &["Ruby Medallion", "Past in Flames"],
        &main,
        &side
    ));
    assert!(passes(
        TwoOrMoreInSideboard,
        &["Past in Flames", "Q Sideboard Card"],
        &main,
        &side
    ));
    assert!(!passes(
        TwoOrMoreInSideboard,
        &["Past in Flames", "Ruby Medallion"],
        &main,
        &side
    ));
    assert!(passes(
        TwoOrMoreInMainOrSideboard,
        &["Past in Flames", "Ruby Medallion"],
        &main,
        &side
    ));
    assert!(!passes(
        TwoOrMoreInMainOrSideboard,
        &["Past in Flames", "Nope"],
        &main,
        &side
    ));

    // DoesNotContain*: Cards[0] only; an empty list passes.
    assert!(passes(
        DoesNotContain,
        &["Nope", "Ruby Medallion"],
        &main,
        &side
    ));
    assert!(!passes(DoesNotContain, &["Past in Flames"], &main, &side));
    assert!(passes(DoesNotContain, &[], &main, &side));
    assert!(passes(
        DoesNotContainMainboard,
        &["Past in Flames"],
        &main,
        &side
    ));
    assert!(!passes(
        DoesNotContainMainboard,
        &["Ruby Medallion"],
        &main,
        &side
    ));
    assert!(passes(DoesNotContainMainboard, &[], &main, &side));
    assert!(passes(
        DoesNotContainSideboard,
        &["Ruby Medallion"],
        &main,
        &side
    ));
    assert!(!passes(
        DoesNotContainSideboard,
        &["Past in Flames"],
        &main,
        &side
    ));
    assert!(passes(DoesNotContainSideboard, &[], &main, &side));

    // Faces: the front face of "A // B" and the full name both match.
    assert!(passes(InMainboard, &["Fire"], &main, &side));
    assert!(passes(InMainboard, &["Fire // Ice"], &main, &side));
    assert!(!passes(InMainboard, &["Ice"], &main, &side));

    // An unknown type fails (the API `default => false`).
    assert!(!passes(
        Unknown("Whatever".to_string()),
        &["Ruby Medallion"],
        &main,
        &side
    ));

    // Every condition must pass; no conditions pass.
    let both = [
        condition(InMainboard, &["Ruby Medallion"]),
        condition(InSideboard, &["Ruby Medallion"]),
    ];
    assert!(!satisfies(&both, &keys(&main), &keys(&side)));
    assert!(satisfies(&[], &keys(&main), &keys(&side)));
}

fn named(name: &str, sideboard: bool, card_type: &str, identity: &str) -> ObservedCard {
    ObservedCard {
        catalog_id: 1,
        quantity: 4,
        sideboard,
        oracle_id: None,
        name: Some(name.to_string()),
        card_type: Some(card_type.to_string()),
        color_identity: Some(identity.to_string()),
        mana_cost: None,
    }
}

fn defined(id: i64, uuid: &str, colours: &str, definition: Definition) -> Candidate {
    Candidate {
        archetype_id: id,
        uuid: uuid.to_string(),
        source: Source::Downloaded,
        color_identity: Some(colours.to_string()),
        definition: Some(definition),
        lists: Vec::new(),
    }
}

fn rule(rank: &[f64], fork: &str, cards: &[&str]) -> Definition {
    Definition {
        rank: rank.to_vec(),
        colour_fork: Some(fork.to_string()),
        conditions: vec![condition(ConditionKind::InMainboard, cards)],
        min_overlap: 0.1,
        ..Definition::default()
    }
}

#[test]
fn rules_prefer_rank_then_colour_then_uuid() {
    // Blue-red observed (a full list, so the land's white never counts).
    let observed = vec![
        named("Q Spell", false, "Instant", "U"),
        named("Q Burn", false, "Instant", "R"),
        named("Q Land", false, "Land", "W"),
        costed("Q Filler", 12, "Artifact", "", Some("{2}")),
    ];

    // A higher rank wins over a lower one, whatever the colours.
    let low = defined(
        1,
        "a",
        "U,R",
        rule(&[1.0, 0.0, 0.0], "1:Base", &["Q Spell"]),
    );
    let high = defined(2, "z", "G", rule(&[2.0, 0.0, 0.0], "2:Base", &["Q Burn"]));
    assert_eq!(
        rules_match(&observed, &[low.clone(), high.clone()]),
        Some((2, 1.0))
    );

    // Ranks compare as lists: [2, 0, 1] beats [2, 0, 0, 5].
    let longer = defined(
        3,
        "b",
        "U,R",
        rule(&[2.0, 0.0, 0.0, 5.0], "3:Base", &["Q Spell"]),
    );
    let later = defined(
        4,
        "c",
        "U,R",
        rule(&[2.0, 0.0, 1.0], "4:Base", &["Q Spell"]),
    );
    assert_eq!(rules_match(&observed, &[longer, later]), Some((4, 1.0)));

    // Within a colour fork: the exact colour identity wins.
    let fork = [1.0, 0.0, 0.0];
    let izzet = defined(10, "d", "U,R", rule(&fork, "7:Base", &["Q Spell"]));
    let mono = defined(11, "a", "U", rule(&fork, "7:Base", &["Q Spell"]));
    let grixis = defined(12, "b", "U,B,R", rule(&fork, "7:Base", &["Q Spell"]));
    assert_eq!(
        rules_match(&observed, &[mono.clone(), grixis.clone(), izzet]),
        Some((10, 1.0))
    );

    // No exact one: the fewest extra colours, here mono blue (none extra).
    assert_eq!(
        rules_match(&observed, &[grixis.clone(), mono]),
        Some((11, 1.0))
    );

    // A tie on colour: the lowest uuid.
    let jeskai_a = defined(20, "f", "W,U,R", rule(&fork, "8:Base", &["Q Spell"]));
    let grixis_b = defined(21, "e", "U,B,R", rule(&fork, "8:Base", &["Q Spell"]));
    assert_eq!(
        rules_match(&observed, &[jeskai_a, grixis_b]),
        Some((21, 1.0))
    );

    // Sideboard colours never count (the API DeckColorNamer reads the
    // mainboard): a black sideboard card keeps the fork on Izzet.
    let mut splashed = observed.clone();
    splashed.push(named("Q Side Removal", true, "Instant", "B"));
    let izzet_again = defined(13, "d", "U,R", rule(&fork, "7:Base", &["Q Spell"]));
    assert_eq!(
        rules_match(&splashed, &[grixis.clone(), izzet_again]),
        Some((13, 1.0))
    );

    // Nothing passes, or a candidate has no conditions: no rules match.
    let miss = defined(30, "g", "U", rule(&fork, "9:Base", &["Q Other"]));
    let empty = defined(31, "h", "U", Definition::default());
    assert_eq!(rules_match(&observed, &[miss, empty]), None);

    // A fallback definition never answers as a rule.
    let mut fallback = rule(&[9.0], "10:Base", &["Q Spell"]);
    fallback.fallback = true;
    assert_eq!(
        rules_match(&observed, &[defined(40, "i", "U", fallback)]),
        None
    );
}

#[test]
fn rules_read_main_and_side_names() {
    let observed = vec![
        named("Q Main", false, "Instant", "U"),
        named("Q Side", true, "Instant", "U"),
    ];
    let mut definition = rule(&[1.0], "1:Base", &["Q Main"]);
    definition
        .conditions
        .push(condition(ConditionKind::InSideboard, &["Q Side"]));

    assert_eq!(
        rules_match(&observed, &[defined(1, "a", "U", definition)]),
        Some((1, 1.0))
    );
}

fn fallback_definition(common: &[&str], min_overlap: f64) -> Definition {
    Definition {
        fallback: true,
        common_cards: common.iter().map(|c| c.to_string()).collect(),
        min_overlap,
        colour_fork: Some("50:Base".to_string()),
        ..Definition::default()
    }
}

#[test]
fn a_fallback_definition_needs_its_min_overlap() {
    // Ten distinct cards observed, one of them common: overlap 0.1.
    let mut observed = Vec::new();
    for i in 0..10 {
        observed.push(named(&format!("Q Card {i}"), false, "Instant", "U"));
    }

    let enough = defined(
        1,
        "a",
        "U",
        fallback_definition(&["Q Card 0", "Q Absent"], 0.1),
    );
    assert_eq!(fallback_match(&observed, &[enough]), Some((1, 0.5)));

    let too_few = defined(2, "b", "U", fallback_definition(&["Q Card 0"], 0.2));
    assert_eq!(fallback_match(&observed, &[too_few]), None);

    let none = defined(3, "c", "U", fallback_definition(&["Q Absent"], 0.0));
    assert_eq!(fallback_match(&observed, &[none]), None);

    // The largest overlap wins.
    let one = defined(4, "d", "U", fallback_definition(&["Q Card 1"], 0.1));
    let mut two_cards = fallback_definition(&["Q Card 1", "Q Card 2"], 0.1);
    two_cards.colour_fork = Some("51:Base".to_string());
    let two = defined(5, "e", "U", two_cards);
    assert_eq!(fallback_match(&observed, &[one, two]), Some((5, 0.5)));

    // A rules definition is never a fallback answer.
    let not_fallback = defined(6, "f", "U", rule(&[1.0], "1:Base", &["Q Card 0"]));
    assert_eq!(fallback_match(&observed, &[not_fallback]), None);

    // Nothing observed: nothing.
    let any = defined(7, "g", "U", fallback_definition(&["Q Card 0"], 0.0));
    assert_eq!(fallback_match(&[], &[any]), None);
}

fn costed(
    name: &str,
    quantity: u32,
    card_type: &str,
    identity: &str,
    cost: Option<&str>,
) -> ObservedCard {
    ObservedCard {
        quantity,
        mana_cost: cost.map(str::to_string),
        ..named(name, false, card_type, identity)
    }
}

// #354: a full Dimir Frog list with two Tamiyo (a transforming card, stored
// with no cost and a G,U identity) read Sultai: every non-land colour
// counted. The server's reading leaves a two-copy colour out as a splash.
#[test]
fn a_two_copy_splash_does_not_pick_a_wider_colour_in_the_fork() {
    let mut observed = vec![
        costed("Psychic Frog", 4, "Creature", "U,B", Some("{U}{B}")),
        costed("Fatal Push", 4, "Instant", "B", Some("{B}")),
        costed("Counterspell", 4, "Instant", "U", Some("{U}{U}")),
        costed("Thoughtseize", 4, "Sorcery", "B", Some("{B}")),
        costed("Consider", 4, "Instant", "U", Some("{U}")),
        costed("Q Draw", 4, "Instant", "U", Some("{1}{U}")),
        costed(
            "Tamiyo, Inquisitive Student // Tamiyo, Seasoned Scholar",
            2,
            "Legendary Creature",
            "G,U",
            None,
        ),
        costed("Watery Grave", 4, "Land", "U,B", None),
        costed("Misty Rainforest", 4, "Land", "G,U", None),
    ];
    let frog = |id: i64, uuid: &str, colours: &str| {
        defined(
            id,
            uuid,
            colours,
            rule(&[1.0, 0.0, 0.0], "7:Frog", &["Psychic Frog"]),
        )
    };
    let fork = [
        frog(1, "a", "U,B,G"),
        frog(2, "b", "U,B"),
        frog(3, "c", "U,B,R"),
        frog(4, "d", "U"),
        frog(5, "e", "W,U,B"),
    ];
    assert_eq!(rules_match(&observed, &fork), Some((2, 1.0)), "Dimir");

    // Tamiyo with its front face's cost reads the same.
    observed[6].mana_cost = Some("{U}".to_string());
    assert_eq!(rules_match(&observed, &fork), Some((2, 1.0)), "Dimir");
}

// The rule's own card counts its colours however few copies (the API
// `ClassifyDeck::anchorCards`).
#[test]
fn the_rules_anchor_card_keeps_its_colours_in_the_fork() {
    let mut observed = vec![costed(
        "Basim Ibn Ishaq",
        3,
        "Legendary Creature",
        "U,B",
        Some("{U}{B}"),
    )];
    for name in [
        "Pinnacle Emissary",
        "Kappa Cannoneer",
        "Q Thopter",
        "Q Bauble",
        "Q Golem",
    ] {
        observed.push(costed(
            name,
            4,
            "Artifact Creature",
            "U,R",
            Some("{1}{U}{R}"),
        ));
    }
    let basim = |id: i64, uuid: &str, colours: &str| {
        defined(
            id,
            uuid,
            colours,
            rule(&[1.0, 0.0, 0.0], "8:Basim", &["Basim Ibn Ishaq"]),
        )
    };
    let fork = [basim(1, "a", "U,R"), basim(2, "b", "U,B,R")];
    assert_eq!(rules_match(&observed, &fork), Some((2, 1.0)), "Grixis");
}
