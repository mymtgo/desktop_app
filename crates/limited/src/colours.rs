//! A limited run's colours and the names of colour combinations (dashboard
//! redesign spec section 4.4, Your colours).

use crate::cards::{COLORS, CardFactsMap, colour_letters};
use crate::pool::RegisteredDeck;

/// The colours of a run's deck: the colours of the main deck cards of the
/// latest registered deck, lands left out, in WUBRG order and comma
/// separated (`U,R`; empty for a colourless deck). A splash makes three
/// colours. `None` without a registered deck, or when no main deck card
/// that is not a land has card data.
pub fn run_deck_colours(decks: &[RegisteredDeck], cards: &CardFactsMap) -> Option<String> {
    let deck = decks.last()?;
    let mut letters = String::new();
    let mut known = false;
    for card in deck.cards.iter().filter(|card| !card.sideboard) {
        let Some(facts) = cards.get(&card.catalog_id) else {
            continue;
        };
        let is_land = facts
            .card_type
            .as_deref()
            .is_some_and(|card_type| card_type.contains("Land"));
        if is_land {
            continue;
        }
        known = true;
        letters.extend(colour_letters(cards, card.catalog_id));
    }
    if !known {
        return None;
    }
    // `colour_letters` keeps only WUBRG, so colourless never shows here.
    Some(::cards::normalize_colors(Some(&letters)).unwrap_or_default())
}

/// The name of a colour combination given as WUBRG letters, comma
/// separated or not (`U,B` or `UB`): the ten guilds, the ten three-colour
/// names, mono colours by name, `Colourless`, `Four colour`, `Five colour`.
pub fn colour_combination_name(colours: &str) -> String {
    let canonical = ::cards::normalize_colors(Some(colours)).unwrap_or_default();
    let key: String = canonical
        .chars()
        .filter(|letter| COLORS.contains(letter))
        .collect();
    let name = match key.as_str() {
        "" => "Colourless",
        "W" => "White",
        "U" => "Blue",
        "B" => "Black",
        "R" => "Red",
        "G" => "Green",
        "WU" => "Azorius",
        "UB" => "Dimir",
        "BR" => "Rakdos",
        "RG" => "Gruul",
        "WG" => "Selesnya",
        "WB" => "Orzhov",
        "UR" => "Izzet",
        "BG" => "Golgari",
        "WR" => "Boros",
        "UG" => "Simic",
        "WUB" => "Esper",
        "UBR" => "Grixis",
        "BRG" => "Jund",
        "WRG" => "Naya",
        "WUG" => "Bant",
        "WBG" => "Abzan",
        "WUR" => "Jeskai",
        "UBG" => "Sultai",
        "WBR" => "Mardu",
        "URG" => "Temur",
        _ if key.len() == 4 => "Four colour",
        _ => "Five colour",
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{CardFacts, CardFactsMap};
    use crate::pool::{RegisteredCard, RegisteredDeck};

    fn facts(card_type: &str, colors: Option<&str>) -> CardFacts {
        CardFacts {
            name: Some("Qcard".to_string()),
            card_type: Some(card_type.to_string()),
            colors: colors.map(str::to_string),
            ..CardFacts::default()
        }
    }

    fn deck(cards: &[(u32, bool)]) -> RegisteredDeck {
        RegisteredDeck {
            match_token: None,
            signature: String::new(),
            captured_at_ms: 0,
            cards: cards
                .iter()
                .map(|(catalog_id, sideboard)| RegisteredCard {
                    catalog_id: *catalog_id,
                    quantity: 1,
                    sideboard: *sideboard,
                })
                .collect(),
        }
    }

    fn cards() -> CardFactsMap {
        CardFactsMap::from([
            (1, facts("Creature", Some("R"))),
            (2, facts("Instant", Some("U"))),
            (3, facts("Basic Land", None)),
            (4, facts("Land", Some("G"))),
            (5, facts("Sorcery", Some("G"))),
            (6, facts("Artifact", None)),
        ])
    }

    #[test]
    fn the_colours_are_the_main_decks_in_wubrg_order_lands_left_out() {
        let found = run_deck_colours(
            &[deck(&[
                (1, false),
                (2, false),
                (3, false),
                (4, false),
                (5, true),
            ])],
            &cards(),
        );
        assert_eq!(found.as_deref(), Some("U,R"));
    }

    #[test]
    fn the_latest_registered_deck_counts_and_a_splash_makes_three() {
        let first = deck(&[(1, false)]);
        let latest = deck(&[(1, false), (2, false), (5, false)]);
        assert_eq!(
            run_deck_colours(&[first, latest], &cards()).as_deref(),
            Some("U,R,G")
        );
    }

    #[test]
    fn no_deck_or_no_card_data_gives_no_colours() {
        assert_eq!(run_deck_colours(&[], &cards()), None);
        assert_eq!(
            run_deck_colours(&[deck(&[(99, false)])], &cards()),
            None,
            "unknown cards"
        );
        assert_eq!(
            run_deck_colours(&[deck(&[(3, false)])], &cards()),
            None,
            "only lands"
        );
        assert_eq!(
            run_deck_colours(&[deck(&[(6, false)])], &cards()).as_deref(),
            Some(""),
            "colourless"
        );
    }

    #[test]
    fn combinations_have_their_names() {
        for (colours, name) in [
            ("", "Colourless"),
            ("R", "Red"),
            ("U,B", "Dimir"),
            ("WG", "Selesnya"),
            ("G,W", "Selesnya"),
            ("U,R", "Izzet"),
            ("W,R", "Boros"),
            ("W,U,B", "Esper"),
            ("U,R,G", "Temur"),
            ("W,B,R", "Mardu"),
            ("W,U,B,R", "Four colour"),
            ("W,U,B,R,G", "Five colour"),
        ] {
            assert_eq!(colour_combination_name(colours), name, "{colours}");
        }
    }
}
