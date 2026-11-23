//! Deck colour identity (0.x `ComputeDeckIdentity` and the manual override in
//! `UpdateColorIdentityController`).

use crate::{DeckCard, php_truthy};

/// Card types that never count towards colour identity (exact matches only, so
/// `Artifact Creature` counts).
const IGNORED_TYPES: [&str; 3] = ["Artifact", "Land", "Basic Land"];

/// A colour needs this many cards to count.
const MIN_COUNT: u64 = 4;

/// The order manual picks are written in.
const MANUAL_ORDER: [&str; 6] = ["W", "U", "B", "R", "G", "C"];

/// What colour identity needs to know about a card.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CardInfo {
    /// The card's type line class (`Creature`, `Land`, `Basic Land`, ...).
    pub card_type: Option<String>,
    /// Comma separated colours (`U,R`); empty or missing means colourless.
    pub color_identity: Option<String>,
}

/// The colours a deck plays, from its latest version's cards. Cards with no
/// card data, no type, or a type of exactly `Artifact`, `Land` or `Basic Land`
/// are skipped. Every other card adds its quantity (main and side) to each of
/// its colours (`C` when it has none). Colours with a total of 4 or more are
/// kept in first-seen order and joined with `,`; none gives `None`.
///
/// An empty card list also gives `None`. 0.x leaves the stored identity alone
/// in that case, so callers should not overwrite it.
pub fn color_identity(
    cards: &[DeckCard],
    info_of: &dyn Fn(u32) -> Option<CardInfo>,
) -> Option<String> {
    // Colour totals in first-seen order.
    let mut counts: Vec<(String, u64)> = Vec::new();

    for card in cards {
        let Some(info) = info_of(card.catalog_id) else {
            continue;
        };
        let card_type = info.card_type.unwrap_or_default();
        if !php_truthy(&card_type) || IGNORED_TYPES.contains(&card_type.as_str()) {
            continue;
        }

        let identity = info.color_identity.unwrap_or_default();
        let identity = identity.trim();
        let colors: Vec<&str> = if php_truthy(identity) {
            identity.split(',').collect()
        } else {
            vec!["C"]
        };

        for color in colors {
            let color = color.trim();
            if !php_truthy(color) {
                continue;
            }
            match counts.iter_mut().find(|(seen, _)| seen == color) {
                Some((_, total)) => *total += u64::from(card.quantity),
                None => counts.push((color.to_string(), u64::from(card.quantity))),
            }
        }
    }

    let kept: Vec<String> = counts
        .into_iter()
        .filter(|(_, total)| *total >= MIN_COUNT)
        .map(|(color, _)| color)
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(","))
    }
}

/// A colour identity picked by hand: `W U B R G C`, deduplicated, in that
/// order, joined with `,`; `None` when nothing is picked. Anything else is
/// ignored here; 0.x rejects the request, so callers validate first.
pub fn normalize_manual_identity(letters: &[String]) -> Option<String> {
    let picked: Vec<&str> = MANUAL_ORDER
        .into_iter()
        .filter(|color| letters.iter().any(|letter| letter == color))
        .collect();
    if picked.is_empty() {
        None
    } else {
        Some(picked.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    fn info(card_type: Option<&str>, colors: Option<&str>) -> Option<CardInfo> {
        Some(CardInfo {
            card_type: card_type.map(str::to_string),
            color_identity: colors.map(str::to_string),
        })
    }

    fn table(catalog_id: u32) -> Option<CardInfo> {
        match catalog_id {
            1 => info(Some("Creature"), Some("U")),
            2 => info(Some("Instant"), Some("R")),
            3 => info(Some("Creature"), Some("U,R")),
            4 => info(Some("Land"), Some("U,R")),
            5 => info(Some("Basic Land"), Some("R")),
            6 => info(Some("Artifact"), Some("")),
            7 => info(Some("Artifact Creature"), None),
            8 => info(None, Some("G")),
            9 => info(Some("Sorcery"), Some(" W , B ")),
            10 => info(Some(""), Some("G")),
            11 => info(Some("Enchantment"), Some("")),
            _ => None,
        }
    }

    fn letters(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn keeps_colours_with_four_or_more_in_first_seen_order() {
        let cards = [card(2, 4, false), card(1, 4, false)];
        assert_eq!(color_identity(&cards, &table).as_deref(), Some("R,U"));
    }

    #[test]
    fn drops_colours_under_four() {
        let cards = [card(1, 4, false), card(2, 3, false)];
        assert_eq!(color_identity(&cards, &table).as_deref(), Some("U"));
    }

    #[test]
    fn counts_the_sideboard_too() {
        let cards = [card(1, 2, false), card(1, 2, true)];
        assert_eq!(color_identity(&cards, &table).as_deref(), Some("U"));
    }

    #[test]
    fn a_multicolour_card_adds_to_each_colour() {
        let cards = [card(3, 4, false)];
        assert_eq!(color_identity(&cards, &table).as_deref(), Some("U,R"));
    }

    #[test]
    fn skips_plain_artifacts_lands_and_basics() {
        let cards = [card(4, 20, false), card(5, 20, false), card(6, 20, false)];
        assert_eq!(color_identity(&cards, &table), None);
    }

    #[test]
    fn an_artifact_creature_with_no_colours_is_colourless() {
        let cards = [card(7, 4, false), card(11, 1, false)];
        assert_eq!(color_identity(&cards, &table).as_deref(), Some("C"));
    }

    #[test]
    fn skips_cards_with_no_type_or_no_card_data() {
        let cards = [card(8, 4, false), card(10, 4, false), card(99, 4, false)];
        assert_eq!(color_identity(&cards, &table), None);
    }

    #[test]
    fn trims_colour_letters() {
        let cards = [card(9, 4, false)];
        assert_eq!(color_identity(&cards, &table).as_deref(), Some("W,B"));
    }

    #[test]
    fn an_empty_list_has_no_identity() {
        assert_eq!(color_identity(&[], &table), None);
    }

    #[test]
    fn manual_identity_is_sorted_wubrgc() {
        // UpdateColorIdentityTest cases.
        assert_eq!(
            normalize_manual_identity(&letters(&["G", "W", "U"])).as_deref(),
            Some("W,U,G")
        );
        assert_eq!(normalize_manual_identity(&letters(&[])), None);
        assert_eq!(
            normalize_manual_identity(&letters(&["R", "R", "G"])).as_deref(),
            Some("R,G")
        );
        assert_eq!(
            normalize_manual_identity(&letters(&["C", "U"])).as_deref(),
            Some("U,C")
        );
        assert_eq!(
            normalize_manual_identity(&letters(&["C"])).as_deref(),
            Some("C")
        );
    }

    #[test]
    fn manual_identity_ignores_unknown_letters() {
        assert_eq!(normalize_manual_identity(&letters(&["X"])), None);
        assert_eq!(
            normalize_manual_identity(&letters(&["g", "B"])).as_deref(),
            Some("B")
        );
    }
}
