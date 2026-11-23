//! A game's local deck and the opponent's cards seen (0.x
//! `CreateGames::buildLocalDeck` and `CreateGames::buildOpponentDeck`).

use decks::DeckCard;

use crate::frame::Frame;

/// The local player's deck for one game: the game's `deck_used` quantities
/// summed per catalog id (the `InSideboard` flag ignored), split into main
/// and side by the first frame's local Sideboard-zone counts. Each catalog id
/// gives a main row (when more than its sideboard count) then a side row
/// (when any are in the sideboard), in `deck_used` order. An empty
/// `deck_used` gives an empty deck.
///
/// `deck_used` lists can lie after sideboarding (game 2 can repeat game 1's
/// list); the frame's Sideboard zone is the post-sideboard truth.
///
/// Every row in `deck_used` is summed, so a caller must pass only the first
/// `deck_used` entry per game (one game's rows), never rows from several
/// `deck_used` lists of the same game.
pub fn local_deck(deck_used: &[DeckCard], first_frame: &Frame, local_id: i64) -> Vec<DeckCard> {
    let mut totals: Vec<(u32, u32)> = Vec::new();
    for card in deck_used {
        add_count(&mut totals, card.catalog_id, card.quantity);
    }

    let mut sideboard: Vec<(u32, u32)> = Vec::new();
    for card in &first_frame.cards {
        if card.owner == local_id && card.zone == "Sideboard" {
            add_count(&mut sideboard, card.catalog_id, 1);
        }
    }

    let mut deck = Vec::new();
    for (catalog_id, total) in totals {
        let side = count_of(&sideboard, catalog_id);
        if total > side {
            deck.push(DeckCard {
                catalog_id,
                quantity: total - side,
                sideboard: false,
            });
        }
        if side > 0 {
            deck.push(DeckCard {
                catalog_id,
                quantity: side,
                sideboard: true,
            });
        }
    }
    deck
}

/// The opponent's cards in the last frame, every zone, counted per catalog
/// id in order of first appearance: `(catalog id, quantity)`.
pub fn opponent_cards(last_frame: &Frame, opponent_id: i64) -> Vec<(u32, u32)> {
    let mut counts: Vec<(u32, u32)> = Vec::new();
    for card in &last_frame.cards {
        if card.owner == opponent_id {
            add_count(&mut counts, card.catalog_id, 1);
        }
    }
    counts
}

/// The opponent's cards seen when two sources know them, `(catalog id,
/// quantity)`: `recorded` (0.x's `game_player.deck_json` of the opponent seat,
/// what 0.x showed) in its order, each with the larger of its two
/// quantities, then the cards only `in_last_frame` has, in that order.
pub fn merge_cards_seen(recorded: &[(u32, u32)], in_last_frame: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (catalog_id, quantity) in recorded {
        let in_frame = count_of(in_last_frame, *catalog_id);
        merged.push((*catalog_id, (*quantity).max(in_frame)));
    }
    for (catalog_id, quantity) in in_last_frame {
        if count_of(&merged, *catalog_id) == 0 {
            merged.push((*catalog_id, *quantity));
        }
    }
    merged
}

/// Adds `quantity` to the catalog id's count, appending it when new. Lists
/// are deck-sized, so a linear search is fine and keeps first-seen order.
fn add_count(counts: &mut Vec<(u32, u32)>, catalog_id: u32, quantity: u32) {
    match counts.iter_mut().find(|(id, _)| *id == catalog_id) {
        Some((_, count)) => *count = count.saturating_add(quantity),
        None => counts.push((catalog_id, quantity)),
    }
}

fn count_of(counts: &[(u32, u32)], catalog_id: u32) -> u32 {
    counts
        .iter()
        .find(|(id, _)| *id == catalog_id)
        .map(|(_, count)| *count)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameCard;

    fn card(id: i64, catalog_id: u32, owner: i64, zone: &str) -> FrameCard {
        FrameCard {
            id,
            catalog_id,
            zone: zone.to_string(),
            actual_zone: zone.to_string(),
            owner,
            controller: owner,
        }
    }

    fn row(catalog_id: u32, quantity: u32, sideboard: bool) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    fn frame(cards: Vec<FrameCard>) -> Frame {
        Frame {
            players: Vec::new(),
            cards,
        }
    }

    #[test]
    fn splits_deck_used_by_the_first_frames_sideboard_zone() {
        let deck_used = vec![
            row(100, 4, false),
            row(200, 2, false),
            // InSideboard is ignored: only the totals count.
            row(200, 1, true),
            row(300, 2, true),
            row(100, 1, false),
        ];
        let first = frame(vec![
            card(1, 200, 0, "Sideboard"),
            card(2, 300, 0, "Sideboard"),
            card(3, 300, 0, "Sideboard"),
            // Not local, or not the sideboard: ignored.
            card(4, 100, 1, "Sideboard"),
            card(5, 100, 0, "Hand"),
        ]);

        assert_eq!(
            local_deck(&deck_used, &first, 0),
            vec![
                row(100, 5, false),
                row(200, 2, false),
                row(200, 1, true),
                row(300, 2, true),
            ]
        );
    }

    #[test]
    fn sideboard_cards_missing_from_deck_used_are_not_added() {
        let deck_used = vec![row(100, 4, false)];
        let first = frame(vec![card(1, 999, 0, "Sideboard")]);
        assert_eq!(local_deck(&deck_used, &first, 0), vec![row(100, 4, false)]);
    }

    #[test]
    fn more_in_the_sideboard_than_listed_gives_only_a_side_row() {
        let deck_used = vec![row(100, 1, false)];
        let first = frame(vec![
            card(1, 100, 0, "Sideboard"),
            card(2, 100, 0, "Sideboard"),
        ]);
        assert_eq!(local_deck(&deck_used, &first, 0), vec![row(100, 2, true)]);
    }

    #[test]
    fn no_deck_used_gives_no_deck() {
        let first = frame(vec![card(1, 100, 0, "Sideboard")]);
        assert_eq!(local_deck(&[], &first, 0), Vec::new());
    }

    #[test]
    fn opponent_cards_count_every_zone_of_the_last_frame() {
        let last = frame(vec![
            card(1, 500, 1, "Battlefield"),
            card(2, 600, 1, "Graveyard"),
            card(3, 500, 1, "Exile"),
            card(4, 700, 0, "Battlefield"),
            card(5, 600, 1, "Stack"),
            card(6, 800, 1, "Battlefield"),
        ]);
        assert_eq!(opponent_cards(&last, 1), vec![(500, 2), (600, 2), (800, 1)]);
        assert_eq!(opponent_cards(&last, 3), Vec::new());
    }

    #[test]
    fn cards_seen_merge_the_recorded_list_with_the_last_frame() {
        let recorded = vec![(500, 2), (600, 1)];
        let in_last_frame = vec![(800, 1), (600, 3), (500, 1)];
        assert_eq!(
            merge_cards_seen(&recorded, &in_last_frame),
            vec![(500, 2), (600, 3), (800, 1)]
        );
        assert_eq!(merge_cards_seen(&recorded, &[]), recorded);
        assert_eq!(merge_cards_seen(&[], &in_last_frame), in_last_frame);
    }
}
