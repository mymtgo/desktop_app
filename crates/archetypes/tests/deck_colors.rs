//! A deck's colours as the server reads them (the API
//! `DeckColorNamer::identity`); the cases mirror its `DeckColorNamerTest`.

use archetypes::{ObservedCard, deck_identity, name_key};

fn card(
    name: &str,
    identity: &str,
    cost: Option<&str>,
    quantity: u32,
    card_type: &str,
) -> ObservedCard {
    ObservedCard {
        catalog_id: 1,
        quantity,
        sideboard: false,
        oracle_id: None,
        name: Some(name.to_string()),
        card_type: Some(card_type.to_string()),
        color_identity: Some(identity.to_string()),
        mana_cost: cost.map(str::to_string),
    }
}

fn side(card: ObservedCard) -> ObservedCard {
    ObservedCard {
        sideboard: true,
        ..card
    }
}

fn identity(cards: &[ObservedCard]) -> Option<String> {
    deck_identity(cards, &[])
}

fn colours(text: &str) -> Option<String> {
    Some(text.to_string())
}

#[test]
fn a_full_list_ignores_small_splashes() {
    let cards = [
        card("Lightning Bolt", "R", Some("{R}"), 12, "Instant"),
        card("Counterspell", "U", Some("{U}{U}"), 12, "Instant"),
        card("Splash Card", "G", Some("{G}"), 1, "Instant"),
    ];
    assert_eq!(identity(&cards), colours("U,R"));
}

#[test]
fn a_four_of_off_colour_spell_counts_and_a_three_of_does_not() {
    let base = card("Lightning Bolt", "R", Some("{R}"), 20, "Instant");
    let four = [
        base.clone(),
        card("Through the Breach", "R", Some("{2}{R}{R}"), 4, "Instant"),
        card("Prime", "G", Some("{4}{G}{G}"), 4, "Creature"),
    ];
    assert_eq!(identity(&four), colours("R,G"));
    let three = [
        base,
        card("Cryptic Command", "U", Some("{1}{U}{U}{U}"), 3, "Instant"),
    ];
    assert_eq!(
        identity(&three),
        colours("R"),
        "once per colour, not per pip"
    );
}

#[test]
fn phyrexian_pips_count_for_nothing_and_hybrid_pips_for_both_halves() {
    let tron = [
        card("Karn Liberated", "", Some("{7}"), 4, "Planeswalker"),
        card("Dismember", "B", Some("{1}{B/P}{B/P}"), 4, "Instant"),
        card("Condescend", "U", Some("{X}{U}"), 4, "Instant"),
        card("Q Filler", "", Some("{2}"), 12, "Artifact"),
    ];
    assert_eq!(identity(&tron), colours("U"));

    let boros = [
        card("Boros Charm", "W,R", Some("{R/W}"), 4, "Instant"),
        card("Gitaxian Probe", "U", Some("{U/P}"), 4, "Sorcery"),
        card("Q Filler", "", Some("{2}"), 12, "Artifact"),
    ];
    assert_eq!(identity(&boros), colours("W,R"));
}

#[test]
fn rainbow_lands_make_a_deck_five_colour_at_four_copies() {
    let glimmervoid = [
        card("Glimmervoid", "", None, 4, "Land"),
        card("Memnite", "", Some("{0}"), 4, "Artifact Creature"),
    ];
    assert_eq!(identity(&glimmervoid), colours("W,U,B,R,G"));

    let one_cavern = [
        card(
            "Chancellor of the Dross",
            "B",
            Some("{4}{B}{B}{B}"),
            4,
            "Creature",
        ),
        card("Thud", "R", Some("{R}"), 4, "Sorcery"),
        card("Cavern of Souls", "", None, 1, "Land"),
        card("Q Filler", "", Some("{2}"), 12, "Artifact"),
    ];
    assert_eq!(identity(&one_cavern), colours("B,R"));
}

#[test]
fn sideboard_cards_never_count() {
    let cards = [
        card("Lightning Bolt", "R", Some("{R}"), 4, "Instant"),
        card("Counterspell", "U", Some("{U}{U}"), 4, "Instant"),
        side(card("Rest in Peace", "W", Some("{1}{W}"), 4, "Enchantment")),
        side(card("Glimmervoid", "", None, 4, "Land")),
    ];
    assert_eq!(identity(&cards), colours("U,R"));
}

#[test]
fn lands_never_colour_a_full_list() {
    let amulet = [
        card("Primeval Titan", "G", Some("{4}{G}{G}"), 4, "Creature"),
        card("Amulet of Vigor", "", Some("{1}"), 4, "Artifact"),
        card("Summoner's Pact", "G", Some("{0}"), 4, "Instant"),
        card(
            "Azusa, Lost but Seeking",
            "G",
            Some("{2}{G}"),
            4,
            "Creature",
        ),
        card("Explore", "G", Some("{1}{G}"), 4, "Sorcery"),
        card("Tolaria West", "U", None, 4, "Land"),
        card("Blooming Marsh", "B,G", None, 4, "Land"),
    ];
    assert_eq!(identity(&amulet), colours("G"));
}

#[test]
fn a_small_sample_counts_every_card_seen_once_lands_included() {
    let grixis = [
        card("Counterspell", "U", Some("{U}{U}"), 1, "Instant"),
        card("Ragavan, Nimble Pilferer", "R", Some("{R}"), 1, "Creature"),
        card("Orcish Bowmasters", "B", Some("{1}{B}"), 1, "Creature"),
        card("Watery Grave", "B,U", None, 1, "Land"),
    ];
    assert_eq!(identity(&grixis), colours("U,B,R"));

    let one_island = [
        card("Ragavan, Nimble Pilferer", "R", Some("{R}"), 4, "Creature"),
        card("Island", "U", None, 1, "Land"),
        card("Mountain", "R", None, 2, "Land"),
    ];
    assert_eq!(identity(&one_island), colours("U,R"));
}

#[test]
fn a_card_without_a_cost_falls_back_to_its_colour_identity() {
    let cards = [
        card(
            "Witch Enchanter // Witch-Blessed Meadow",
            "W",
            None,
            4,
            "Creature",
        ),
        card("Counterspell", "U", Some("{U}{U}"), 4, "Instant"),
    ];
    assert_eq!(identity(&cards), colours("W,U"));
}

#[test]
fn an_anchor_with_a_cost_counts_below_the_threshold() {
    let mut cards = vec![
        card(
            "Basim Ibn Ishaq",
            "U,B",
            Some("{U}{B}"),
            3,
            "Legendary Creature",
        ),
        card(
            "Pinnacle Emissary",
            "U,R",
            Some("{1}{U}{R}"),
            4,
            "Artifact Creature",
        ),
        card(
            "Kappa Cannoneer",
            "U",
            Some("{5}{U}"),
            4,
            "Artifact Creature",
        ),
    ];
    for name in ["Ornithopter", "Memnite", "Mishra's Bauble"] {
        cards.push(card(name, "", Some("{0}"), 4, "Artifact"));
    }
    assert_eq!(identity(&cards), colours("U,R"));
    let anchors = [name_key("Basim Ibn Ishaq")];
    assert_eq!(deck_identity(&cards, &anchors), colours("U,B,R"));
}

#[test]
fn an_anchor_without_a_cost_does_not_count_below_the_threshold() {
    let cards = [
        card(
            "Ajani, Nacatl Pariah // Ajani, Nacatl Avenger",
            "R,W",
            None,
            3,
            "Legendary Creature",
        ),
        card("Guide of Souls", "W", Some("{W}"), 4, "Creature"),
        card("Solitude", "W", Some("{3}{W}{W}"), 4, "Creature"),
        card("Fatal Push", "B", Some("{B}"), 4, "Instant"),
        card("Necrodominance", "B", Some("{B}{B}{B}"), 4, "Enchantment"),
        card("Ephemerate", "W", Some("{W}"), 4, "Instant"),
    ];
    let anchors = [name_key("Ajani, Nacatl Pariah")];
    assert_eq!(deck_identity(&cards, &anchors), colours("W,B"));
}

#[test]
fn a_colourless_deck_has_no_colours() {
    let cards = [card("Q Filler", "", Some("{2}"), 24, "Artifact")];
    assert_eq!(identity(&cards), None);
}

#[test]
fn huge_quantities_saturate_instead_of_overflowing() {
    let rainbow = [
        card("Cavern of Souls", "", None, u32::MAX, "Land"),
        card("Glimmervoid", "", None, u32::MAX, "Land"),
    ];
    assert_eq!(identity(&rainbow), colours("W,U,B,R,G"));
    let spells = [
        card("Lightning Bolt", "R", Some("{R}"), u32::MAX, "Instant"),
        card("Fireblast", "R", Some("{4}{R}{R}"), u32::MAX, "Instant"),
    ];
    assert_eq!(identity(&spells), colours("R"));
}
