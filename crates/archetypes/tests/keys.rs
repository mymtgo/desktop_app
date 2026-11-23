//! Format keys (0.x `tests/Unit/Support/MtgoFormatTest.php`), colour
//! identity (`tests/Unit/Data/ArchetypeDataTest.php` colour cases,
//! `ResolveCardsFromDekTest` "excludes lands from colour identity
//! derivation") and card name keys (the API `CardName`).

use archetypes::{
    ObservedCard, colors_of, format_key, format_label, is_limited, name_key, name_keys,
    normalize_colors,
};

#[test]
fn humanises_a_raw_mtgo_format_code() {
    let cases = [
        ("CMODERN", "Modern"),
        ("CSTANDARD", "Standard"),
        ("CPIONEER", "Pioneer"),
        ("CPAUPER", "Pauper"),
        ("CLEGACY", "Legacy"),
        ("CVINTAGE", "Vintage"),
        ("CPREMODERN", "Premodern"),
        ("CModern", "Modern"),
    ];
    for (code, label) in cases {
        assert_eq!(format_label(code), label, "{code}");
    }
}

#[test]
fn labels_an_underscored_arena_key_as_words() {
    assert_eq!(format_label("historic_brawl"), "Historic Brawl");
    assert_eq!(format_label("ARENA_HISTORIC_BRAWL"), "Historic Brawl");
    assert_eq!(format_label("brawl"), "Brawl");
}

#[test]
fn leaves_an_already_humanised_label_alone() {
    assert_eq!(format_label("Modern"), "Modern");
    assert_eq!(format_label("standard"), "Standard");
    assert_eq!(format_label("Limited"), "Limited");
}

#[test]
fn does_not_strip_a_leading_c_from_a_real_format_name() {
    assert_eq!(format_label("Commander"), "Commander");
    assert_eq!(format_key("Commander"), Some("commander".to_string()));
}

#[test]
fn gives_the_archetypes_format_key_for_a_code_or_a_label() {
    let cases = [
        ("CSTANDARD", "standard"),
        ("CPIONEER", "pioneer"),
        ("CMODERN", "modern"),
        ("Modern", "modern"),
        ("standard", "standard"),
    ];
    for (input, key) in cases {
        assert_eq!(format_key(input), Some(key.to_string()), "{input}");
    }
}

#[test]
fn returns_nothing_for_a_missing_format() {
    assert_eq!(format_label(""), "");
    assert_eq!(format_key(""), None);
}

#[test]
fn a_limited_code_has_no_archetype_key() {
    // 0.x `MtgoMatch::isLimitedFormatCode`: `^(D[A-Z]|S[0-9])`.
    for code in ["DHOBHOBHOB", "DMH3", "S1"] {
        assert!(is_limited(code), "{code}");
        assert_eq!(format_key(code), None, "{code}");
    }
    for code in ["CMODERN", "Dominaria", "Standard", ""] {
        assert!(!is_limited(code), "{code}");
    }
}

#[test]
fn exposes_colour_identity_in_canonical_comma_separated_form() {
    assert_eq!(normalize_colors(Some("UB")), Some("U,B".to_string()));
    assert_eq!(normalize_colors(Some("U,B")), Some("U,B".to_string()));
}

#[test]
fn exposes_a_blank_colour_identity_as_null() {
    assert_eq!(normalize_colors(Some("")), None);
    assert_eq!(normalize_colors(None), None);
}

#[test]
fn colour_identity_is_upper_cased_deduplicated_and_in_wubrg_order() {
    assert_eq!(normalize_colors(Some("gw")), Some("W,G".to_string()));
    assert_eq!(normalize_colors(Some("R,R,x")), Some("R".to_string()));
    assert_eq!(normalize_colors(Some("C")), Some("C".to_string()));
}

fn seen(card_type: &str, identity: &str) -> ObservedCard {
    ObservedCard {
        catalog_id: 1,
        quantity: 4,
        sideboard: false,
        oracle_id: None,
        name: None,
        card_type: Some(card_type.to_string()),
        color_identity: Some(identity.to_string()),
        mana_cost: None,
    }
}

#[test]
fn excludes_lands_from_colour_identity_derivation() {
    let cards = vec![seen("Basic Land", "R"), seen("Instant", "R")];

    assert_eq!(colors_of(&cards), Some("R".to_string()));
}

#[test]
fn colours_of_cards_are_in_wubrg_order_and_lands_never_count() {
    let cards = vec![
        seen("Creature", "G"),
        seen("Instant", "U,R"),
        seen("Land", "W"),
        seen("Sorcery", "U"),
    ];

    assert_eq!(colors_of(&cards), Some("U,R,G".to_string()));
    assert_eq!(colors_of(&[seen("Land", "W")]), None);
}

#[test]
fn a_name_key_is_trimmed_collapsed_and_lower_cased() {
    assert_eq!(name_key("  Ral,   Monsoon\tMage "), "ral, monsoon mage");
    assert_eq!(name_key("Lightning Bolt"), "lightning bolt");
}

#[test]
fn name_keys_add_the_front_face_of_a_split_name() {
    assert_eq!(
        name_keys("Fire // Ice"),
        vec!["fire // ice".to_string(), "fire".to_string()]
    );
    assert_eq!(
        name_keys("Lightning Bolt"),
        vec!["lightning bolt".to_string()]
    );
    assert_eq!(name_keys(" // Ice"), vec!["// ice".to_string()]);
}

/// #300 F10: the format helpers are the ones `decks` has (0.x
/// `MtgoFormat::display`, `Str::title`, and `isLimitedFormatCode`), so the
/// pages never disagree on a label or on what is limited.
#[test]
fn the_format_helpers_agree_with_the_decks_ones() {
    for code in [
        "CMODERN",
        "CModern",
        "Commander",
        "pauper commander",
        "Limited",
        "o'brien 3rd",
        "",
        "DHOBHOBHOB",
        "S6FRA",
        "Sealed",
        "D",
    ] {
        assert_eq!(format_label(code), decks::display_format(code), "{code}");
        assert_eq!(
            is_limited(code),
            decks::is_limited_match_format(code),
            "{code}"
        );
    }
}

#[test]
fn arena_codes_drop_their_prefix() {
    for (code, key) in [
        ("ARENA_HISTORIC", "historic"),
        ("ARENA_TIMELESS", "timeless"),
        ("ARENA_ALCHEMY", "alchemy"),
    ] {
        assert_eq!(format_key(code), Some(key.to_string()), "{code}");
    }
}

#[test]
fn arena_and_endstep_historic_share_a_key() {
    assert_eq!(format_key("ARENA_HISTORIC"), format_key("Historic"));
}

#[test]
fn mtgo_codes_are_unchanged() {
    for (code, key) in [
        ("CMODERN", "modern"),
        ("CPIONEER", "pioneer"),
        ("CSTANDARD", "standard"),
        ("Commander", "commander"),
    ] {
        assert_eq!(format_key(code), Some(key.to_string()), "{code}");
    }
    assert_eq!(format_key("DHOBHOBHOB"), None);
}
