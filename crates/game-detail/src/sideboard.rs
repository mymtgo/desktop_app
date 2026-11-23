//! Sideboard changes of a game against the registered deck version (0.x
//! `BuildMatchGameData::computeSideboardChanges`).
//!
//! Main-deck quantities are compared per card. A card's key is
//! `oracle:{oracle id}` when it has one, so different printings of the same
//! card compare as equal, else `mtgo:{catalog id}`. The namespaces keep the
//! two kinds of key from colliding.

use decks::DeckCard;

/// Whether a card came into the main deck or went out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    In,
    Out,
}

/// One sideboard change: the printing (the first one seen for its key), how
/// many, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideboardChange {
    pub catalog_id: u32,
    pub quantity: u32,
    pub kind: ChangeKind,
}

/// The cards brought in (game main above registered main), in game-deck
/// order, then the cards taken out (registered main above game main), in
/// registered order. Sideboard rows and catalog id 0 are ignored. An empty
/// game deck or registered deck gives no changes. `oracle_of` gives a catalog
/// id's oracle id; an empty one counts as none.
pub fn sideboard_changes(
    game_deck: &[DeckCard],
    registered: &[DeckCard],
    oracle_of: &dyn Fn(u32) -> Option<String>,
) -> Vec<SideboardChange> {
    if game_deck.is_empty() || registered.is_empty() {
        return Vec::new();
    }

    let game_mains = main_tallies(game_deck, oracle_of);
    let registered_mains = main_tallies(registered, oracle_of);

    let mut changes = Vec::new();
    for entry in &game_mains {
        let registered_quantity = quantity_of(&registered_mains, &entry.key);
        if entry.quantity > registered_quantity {
            changes.push(SideboardChange {
                catalog_id: entry.catalog_id,
                quantity: entry.quantity - registered_quantity,
                kind: ChangeKind::In,
            });
        }
    }
    for entry in &registered_mains {
        let game_quantity = quantity_of(&game_mains, &entry.key);
        if entry.quantity > game_quantity {
            changes.push(SideboardChange {
                catalog_id: entry.catalog_id,
                quantity: entry.quantity - game_quantity,
                kind: ChangeKind::Out,
            });
        }
    }
    changes
}

/// The main-deck quantity of one card key, and the first printing seen for it.
struct Tally {
    key: String,
    catalog_id: u32,
    quantity: u32,
}

/// Main-deck rows summed per card key, in first-seen order.
fn main_tallies(cards: &[DeckCard], oracle_of: &dyn Fn(u32) -> Option<String>) -> Vec<Tally> {
    let mut tallies: Vec<Tally> = Vec::new();
    for card in cards {
        if card.sideboard || card.catalog_id == 0 {
            continue;
        }
        let key = match oracle_of(card.catalog_id) {
            Some(oracle) if !oracle.is_empty() => format!("oracle:{oracle}"),
            _ => format!("mtgo:{}", card.catalog_id),
        };
        match tallies.iter_mut().find(|tally| tally.key == key) {
            Some(tally) => tally.quantity = tally.quantity.saturating_add(card.quantity),
            None => tallies.push(Tally {
                key,
                catalog_id: card.catalog_id,
                quantity: card.quantity,
            }),
        }
    }
    tallies
}

fn quantity_of(tallies: &[Tally], key: &str) -> u32 {
    tallies
        .iter()
        .find(|tally| tally.key == key)
        .map(|tally| tally.quantity)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports of the sideboard cases in 0.x
    // tests/Feature/Actions/Matches/BuildMatchGameDataTest.php.

    fn main(catalog_id: u32, quantity: u32) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard: false,
        }
    }

    fn side(catalog_id: u32, quantity: u32) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard: true,
        }
    }

    fn oracles(pairs: &'static [(u32, &'static str)]) -> impl Fn(u32) -> Option<String> {
        move |catalog_id| {
            pairs
                .iter()
                .find(|(id, _)| *id == catalog_id)
                .map(|(_, oracle)| oracle.to_string())
        }
    }

    fn change(catalog_id: u32, quantity: u32, kind: ChangeKind) -> SideboardChange {
        SideboardChange {
            catalog_id,
            quantity,
            kind,
        }
    }

    #[test]
    fn reports_no_sideboard_changes_when_game_deck_matches_registered_deck() {
        let lookup = oracles(&[(1001, "oracle-1001"), (1002, "oracle-1002")]);
        let game = [main(1001, 4), main(1002, 3)];
        let registered = [main(1001, 4), main(1002, 3)];

        assert!(sideboard_changes(&game, &registered, &lookup).is_empty());
    }

    #[test]
    fn reports_no_sideboard_changes_when_oracle_id_is_missing_but_catalog_id_matches() {
        let lookup = oracles(&[(1001, "")]);
        let game = [main(1001, 4)];
        let registered = [main(1001, 4)];

        assert!(sideboard_changes(&game, &registered, &lookup).is_empty());
        assert!(sideboard_changes(&game, &registered, &|_| None).is_empty());
    }

    #[test]
    fn detects_sideboard_swap_one_card_in_one_card_out() {
        let lookup = oracles(&[(1001, "oracle-1001"), (1002, "oracle-1002")]);
        let registered = [main(1001, 4)];
        let game = [main(1001, 3), main(1002, 1)];

        assert_eq!(
            sideboard_changes(&game, &registered, &lookup),
            vec![
                change(1002, 1, ChangeKind::In),
                change(1001, 1, ChangeKind::Out)
            ]
        );
    }

    #[test]
    fn treats_different_printings_of_the_same_card_as_identical() {
        let lookup = oracles(&[(2001, "oracle-urzas-mine"), (2002, "oracle-urzas-mine")]);
        let game = [main(2002, 4)];
        let registered = [main(2001, 4)];

        assert!(sideboard_changes(&game, &registered, &lookup).is_empty());
    }

    #[test]
    fn sideboard_rows_are_ignored_and_printings_of_one_card_are_summed() {
        let lookup = oracles(&[(1, "a"), (2, "a"), (3, "b")]);
        let game = [main(1, 2), main(2, 2), side(3, 3), main(0, 9)];
        let registered = [main(1, 3), side(1, 1), main(3, 2), main(0, 1)];

        assert_eq!(
            sideboard_changes(&game, &registered, &lookup),
            vec![change(1, 1, ChangeKind::In), change(3, 2, ChangeKind::Out)]
        );
    }

    #[test]
    fn an_empty_side_gives_no_changes() {
        let lookup = |_| None;
        assert!(sideboard_changes(&[], &[main(1, 4)], &lookup).is_empty());
        assert!(sideboard_changes(&[main(1, 4)], &[], &lookup).is_empty());
    }
}
