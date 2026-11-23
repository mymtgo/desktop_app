//! Whether the current game is still before the play or draw choice, when
//! the local hand must not be read yet.

use game_detail::frame::Frame;

/// Whether the game is still before the play or draw choice. Frames carry
/// the local hand as soon as it is dealt, which can be before the player
/// chooses, so the draw odds must not read them yet. The choice is known
/// once logged (`on_play`), or once the game is visibly under way: a kept
/// hand logged (`local_mulligans`) or a card of either player on the
/// battlefield, the stack, in a graveyard or in exile (a game whose choice
/// line was never logged still gets its odds).
pub fn before_play_draw(
    on_play: Option<bool>,
    local_mulligans: Option<u32>,
    frame: Option<&Frame>,
) -> bool {
    if on_play.is_some() || local_mulligans.is_some() {
        return false;
    }
    let under_way = frame.is_some_and(|frame| {
        frame.cards.iter().any(|card| {
            ["Battlefield", "Stack", "Graveyard", "Exile"].contains(&card.zone.as_str())
        })
    });
    !under_way
}

#[cfg(test)]
mod tests {
    use game_detail::frame::FrameCard;

    use super::*;

    fn frame_card(catalog_id: u32, owner: i64, zone: &str) -> FrameCard {
        FrameCard {
            catalog_id,
            owner,
            zone: zone.to_string(),
            ..FrameCard::default()
        }
    }

    fn frame(cards: Vec<FrameCard>) -> Frame {
        Frame {
            players: Vec::new(),
            cards,
        }
    }

    #[test]
    fn the_hand_is_dealt_before_the_play_draw_choice() {
        let dealt = frame(vec![frame_card(1, 1, "Hand"), frame_card(2, 1, "Library")]);

        assert!(before_play_draw(None, None, Some(&dealt)));
        assert!(before_play_draw(None, None, None));
    }

    #[test]
    fn the_choice_is_made_once_logged_or_the_game_is_under_way() {
        let dealt = frame(vec![frame_card(1, 1, "Hand")]);
        assert!(!before_play_draw(Some(false), None, Some(&dealt)));
        assert!(!before_play_draw(None, Some(0), Some(&dealt)));

        let played = frame(vec![
            frame_card(1, 1, "Hand"),
            frame_card(2, 1, "Battlefield"),
        ]);
        assert!(!before_play_draw(None, None, Some(&played)));
        // The opponent played first.
        let theirs = frame(vec![
            frame_card(1, 1, "Hand"),
            frame_card(3, 2, "Battlefield"),
        ]);
        assert!(!before_play_draw(None, None, Some(&theirs)));
    }
}
