//! The turn estimate of a game with no turn count from its game log (0.x
//! `BuildMatchGameData::estimateTurns`).

use std::collections::HashSet;

use crate::frame::Frame;

/// The number of lands on the battlefield in the last frame, both players
/// together; `None` when there are none. `land_ids` are the catalog ids the
/// caller knows to be lands (type line containing `Land`).
pub fn estimate_turns(last_frame: &Frame, land_ids: &HashSet<u32>) -> Option<u32> {
    let mut lands = 0;
    for card in &last_frame.cards {
        if card.zone == "Battlefield" && land_ids.contains(&card.catalog_id) {
            lands += 1;
        }
    }
    if lands > 0 { Some(lands) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameCard;

    fn card(catalog_id: u32, owner: i64, zone: &str) -> FrameCard {
        FrameCard {
            id: 0,
            catalog_id,
            zone: zone.to_string(),
            actual_zone: zone.to_string(),
            owner,
            controller: owner,
        }
    }

    #[test]
    fn counts_lands_on_the_battlefield_for_both_players() {
        let last = Frame {
            players: Vec::new(),
            cards: vec![
                card(1, 0, "Battlefield"),
                card(1, 1, "Battlefield"),
                card(2, 1, "Battlefield"),
                // Not a land.
                card(9, 0, "Battlefield"),
                // A land, but not on the battlefield.
                card(1, 0, "Hand"),
                card(2, 1, "Graveyard"),
            ],
        };
        let lands: HashSet<u32> = [1, 2].into_iter().collect();

        assert_eq!(estimate_turns(&last, &lands), Some(3));
    }

    #[test]
    fn no_lands_gives_none() {
        let last = Frame {
            players: Vec::new(),
            cards: vec![card(9, 0, "Battlefield")],
        };
        let lands: HashSet<u32> = [1].into_iter().collect();

        assert_eq!(estimate_turns(&last, &lands), None);
        assert_eq!(estimate_turns(&Frame::default(), &lands), None);
    }
}
