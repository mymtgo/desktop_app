//! The opening hand, mulligans and London-mulligan bottoming of one game
//! (0.x `ParseOpeningHand`).
//!
//! The timeline form replays the game's frames:
//! - The opening phase ends at the first frame where a local card is on the
//!   Battlefield, Stack or Graveyard.
//! - A local mulligan is a hand with no instance id in common with the
//!   previous hand and at least 4 cards.
//! - Bottomed cards are the largest hand seen since the last mulligan minus
//!   the hand when the opening phase ends. Swapping the bottom selection can
//!   shrink the hand for a frame or two (7, 6, 5, 6); only the final choice
//!   counts.
//! - Opponent mulligans come from library counts: the starting library (the
//!   first frame with an empty hand and a non-empty library) against the
//!   highest library seen while the opponent holds a hand in the opening
//!   phase. Bottoming is the only thing that raises it.
//!
//! The stored form (`opening_hand_from_stored`) is for hand-entered games
//! (manual matches), which have no frames.

use crate::frame::Frame;

/// One card in a hand: the in-game instance id and the printing. In the
/// stored form, `instance_id` is the card's position in its list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandCard {
    pub instance_id: i64,
    pub catalog_id: u32,
}

/// A card of the hand 0.x shows, and whether it was put on the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownCard {
    pub catalog_id: u32,
    pub bottomed: bool,
}

/// A game's opening hand (0.x `ParseOpeningHand` output).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpeningHand {
    /// The opening hand: the cards drawn for the hand that was kept, before
    /// bottoming (equal to `starting` when nothing was bottomed). From frames,
    /// on the draw it can end with the turn 1 draw (see `game_cards`).
    pub opening: Vec<HandCard>,
    /// The starting hand: the opening hand minus the bottomed cards.
    pub starting: Vec<HandCard>,
    /// Instance ids of the cards put on the bottom.
    pub bottomed: Vec<i64>,
    /// Each hand the local player mulliganed away, in order.
    pub mulliganed_hands: Vec<Vec<HandCard>>,
    pub local_mulligans: u32,
    pub opponent_mulligans: u32,
    /// The index of the first frame that holds the kept hand (0 when no hand
    /// was seen). Cards seen before it were only in hands sent back.
    pub kept_from: usize,
}

impl OpeningHand {
    /// The hand 0.x shows (`BuildMatchGameData::parseHandData`): the opening
    /// hand with the bottomed cards marked.
    pub fn shown_hand(&self) -> Vec<ShownCard> {
        self.opening
            .iter()
            .map(|card| ShownCard {
                catalog_id: card.catalog_id,
                bottomed: self.bottomed.contains(&card.instance_id),
            })
            .collect()
    }
}

/// A hand-entered opening hand (0.x `opening_hand_json` on the game player
/// pivot): catalog ids of the starting hand (0.x `kept`), the bottomed cards
/// and each mulliganed hand. A mulliganed hand the user skipped is an empty
/// list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredHand {
    pub starting: Vec<u32>,
    pub bottomed: Vec<u32>,
    pub mulligans: Vec<Vec<u32>>,
}

/// The opening hand of a game from its frames, in event order. `local_id`
/// and `opponent_id` are the players' seat ids (the caller picks the local
/// player by name). No frames give an empty result.
pub fn opening_hand(frames: &[Frame], local_id: i64, opponent_id: i64) -> OpeningHand {
    let mut mulliganed_hands: Vec<Vec<HandCard>> = Vec::new();
    let mut current_hand: Vec<HandCard> = Vec::new();
    // The largest hand seen since the last mulligan, used to find bottomed cards.
    let mut latest_full_hand: Vec<HandCard> = Vec::new();
    let mut opening_phase = true;
    let mut kept_from = 0;

    // The kept library is the highest count seen while the opponent holds a
    // hand during the opening phase. The first hand frame is not enough on
    // its own: dense frames catch the initial 7-card draw before any
    // mulligan, which always reads as zero.
    let mut opponent_start_library: Option<i64> = None;
    let mut opponent_kept_library: Option<i64> = None;

    for (index, frame) in frames.iter().enumerate() {
        if let Some(opponent) = frame.player(opponent_id) {
            let hand_count = opponent.hand_count;
            let library_count = opponent.library_count;
            // A zero library with a zero hand is a placeholder frame from
            // before the decks load, not the starting library.
            if opponent_start_library.is_none() && hand_count == 0 && library_count > 0 {
                opponent_start_library = Some(library_count);
            } else if hand_count > 0 && (opening_phase || opponent_kept_library.is_none()) {
                let highest = opponent_kept_library
                    .unwrap_or(library_count)
                    .max(library_count);
                opponent_kept_library = Some(highest);
            }
        }

        if !opening_phase {
            if opponent_kept_library.is_some() {
                break;
            }
            continue;
        }

        if local_card_in_play(frame, local_id) {
            opening_phase = false;
            continue;
        }

        if frame.player(local_id).is_none() {
            continue;
        }

        let hand_now = local_hand(frame, local_id);

        if hand_now.is_empty() && current_hand.is_empty() {
            continue; // Before the draw.
        }

        if current_hand.is_empty() {
            current_hand = hand_now.clone();
            latest_full_hand = hand_now;
            kept_from = index;
            continue;
        }

        if hand_now.is_empty() {
            continue;
        }

        let overlaps = hand_now
            .iter()
            .any(|card| has_instance(&current_hand, card.instance_id));
        if !overlaps && hand_now.len() >= 4 {
            // A complete replacement is a mulligan. Reset the full-hand mark.
            mulliganed_hands.push(current_hand);
            current_hand = hand_now.clone();
            latest_full_hand = hand_now;
            kept_from = index;
        } else {
            // Bottoming selection swaps can shrink the hand for a moment
            // (7, 6, 5, 6); the high-water hand plus any card that comes
            // back gives the true hand before bottoming.
            for card in &hand_now {
                if !has_instance(&latest_full_hand, card.instance_id) {
                    latest_full_hand.push(*card);
                }
            }
            current_hand = hand_now;
        }
    }

    // Bottomed cards: the high-water hand minus the settled hand at the end
    // of the opening phase.
    let bottomed: Vec<i64> = latest_full_hand
        .iter()
        .filter(|card| !has_instance(&current_hand, card.instance_id))
        .map(|card| card.instance_id)
        .collect();
    let opening = if bottomed.is_empty() {
        current_hand.clone()
    } else {
        latest_full_hand
    };

    // A kept library above (start - 7) means cards were shuffled back.
    let mut opponent_mulligans = 0;
    if let (Some(start), Some(kept)) = (opponent_start_library, opponent_kept_library) {
        opponent_mulligans = (kept - (start - 7)).max(0);
    }

    OpeningHand {
        local_mulligans: mulliganed_hands.len() as u32,
        opening,
        starting: current_hand,
        bottomed,
        mulliganed_hands,
        opponent_mulligans: u32::try_from(opponent_mulligans).unwrap_or(0),
        kept_from,
    }
}

/// A stored (hand-entered) opening hand shaped like the timeline result
/// (0.x `ParseOpeningHand::fromPivot`). Bottomed cards follow the starting
/// cards in `opening`, and positions stand in for instance ids. The
/// mulligan counts come from the stored game, not from the lists.
pub fn opening_hand_from_stored(
    stored: &StoredHand,
    local_mulligans: u32,
    opponent_mulligans: u32,
) -> OpeningHand {
    let starting = positioned(&stored.starting);

    let mut all_cards = stored.starting.clone();
    all_cards.extend(stored.bottomed.iter().copied());
    let opening = positioned(&all_cards);

    let bottomed: Vec<i64> = (stored.starting.len()..all_cards.len())
        .map(|position| position as i64)
        .collect();

    let mulliganed_hands = stored
        .mulligans
        .iter()
        .map(|hand| positioned(hand))
        .collect();

    OpeningHand {
        opening,
        starting,
        bottomed,
        mulliganed_hands,
        local_mulligans,
        opponent_mulligans,
        kept_from: 0,
    }
}

/// Whether a local card is on the Battlefield, Stack or Graveyard, which
/// ends the opening phase.
fn local_card_in_play(frame: &Frame, local_id: i64) -> bool {
    frame.cards.iter().any(|card| {
        card.owner == local_id
            && (card.zone == "Battlefield" || card.zone == "Stack" || card.zone == "Graveyard")
    })
}

/// The local player's Hand-zone cards in frame order. A repeated instance id
/// keeps its first place and takes the later printing, as 0.x's keyed array did.
fn local_hand(frame: &Frame, local_id: i64) -> Vec<HandCard> {
    let mut hand: Vec<HandCard> = Vec::new();
    for card in &frame.cards {
        if card.zone != "Hand" || card.owner != local_id {
            continue;
        }
        match hand.iter_mut().find(|held| held.instance_id == card.id) {
            Some(held) => held.catalog_id = card.catalog_id,
            None => hand.push(HandCard {
                instance_id: card.id,
                catalog_id: card.catalog_id,
            }),
        }
    }
    hand
}

fn has_instance(hand: &[HandCard], instance_id: i64) -> bool {
    hand.iter().any(|card| card.instance_id == instance_id)
}

/// Catalog ids as hand cards whose instance ids are their positions.
fn positioned(catalog_ids: &[u32]) -> Vec<HandCard> {
    catalog_ids
        .iter()
        .enumerate()
        .map(|(position, &catalog_id)| HandCard {
            instance_id: position as i64,
            catalog_id,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameCard, FramePlayer};

    // Ports of 0.x tests/Feature/Actions/Matches/ParseOpeningHandTest.php and
    // tests/Feature/Matches/ParseOpeningHandPivotTest.php.

    /// `(id, hand count, library count)`.
    fn players(list: &[(i64, i64, i64)]) -> Vec<FramePlayer> {
        list.iter()
            .map(|&(id, hand_count, library_count)| FramePlayer {
                id,
                name: String::new(),
                library_count,
                hand_count,
                life: 20,
            })
            .collect()
    }

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

    /// Local (owner 1) hand cards `(instance id, catalog id)`.
    fn hand(cards: &[(i64, u32)]) -> Vec<FrameCard> {
        cards
            .iter()
            .map(|&(id, catalog_id)| card(id, catalog_id, 1, "Hand"))
            .collect()
    }

    fn frame(player_list: &[(i64, i64, i64)], cards: Vec<FrameCard>) -> Frame {
        Frame {
            players: players(player_list),
            cards,
        }
    }

    /// Instance ids 101..=107 with catalog ids 1001..=1007 (or 2xx/2xxx for `base` 2).
    fn seven(base: i64) -> Vec<(i64, u32)> {
        (1..=7)
            .map(|i| (base * 100 + i, (base * 1000 + i) as u32))
            .collect()
    }

    fn catalogs(cards: &[HandCard]) -> Vec<u32> {
        cards.iter().map(|card| card.catalog_id).collect()
    }

    fn ids(cards: &[HandCard]) -> Vec<i64> {
        cards.iter().map(|card| card.instance_id).collect()
    }

    fn pre_draw() -> Frame {
        frame(&[(1, 0, 60), (2, 0, 60)], Vec::new())
    }

    fn game_starts(local_hand_count: i64, battlefield: (i64, u32)) -> Frame {
        frame(
            &[(1, local_hand_count, 53), (2, 7, 53)],
            vec![card(battlefield.0, battlefield.1, 1, "Battlefield")],
        )
    }

    #[test]
    fn returns_no_mulligans_when_player_keeps_opening_7() {
        let frames = vec![
            pre_draw(),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&seven(1))),
            frame(
                &[(1, 6, 53), (2, 7, 53)],
                vec![card(101, 1001, 1, "Battlefield")],
            ),
        ];

        let result = opening_hand(&frames, 1, 2);

        assert!(result.mulliganed_hands.is_empty());
        assert_eq!(
            catalogs(&result.starting),
            vec![1001, 1002, 1003, 1004, 1005, 1006, 1007]
        );
        assert!(result.bottomed.is_empty());
        assert_eq!(result.opponent_mulligans, 0);
        assert_eq!(result.local_mulligans, 0);
    }

    #[test]
    fn detects_a_mulligan_with_complete_hand_replacement() {
        let frames = vec![
            pre_draw(),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&seven(1))),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&seven(2))),
            game_starts(6, (201, 2001)),
        ];

        let result = opening_hand(&frames, 1, 2);

        assert_eq!(result.mulliganed_hands.len(), 1);
        assert_eq!(
            catalogs(&result.mulliganed_hands[0]),
            vec![1001, 1002, 1003, 1004, 1005, 1006, 1007]
        );
        assert_eq!(
            catalogs(&result.starting),
            vec![2001, 2002, 2003, 2004, 2005, 2006, 2007]
        );
        assert_eq!(result.local_mulligans, 1);
    }

    #[test]
    fn detects_bottomed_cards_after_mulligan() {
        let mut six = seven(2);
        six.pop();
        let frames = vec![
            pre_draw(),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&seven(1))),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&seven(2))),
            // Bottom 1 card (instance 207 removed).
            frame(&[(1, 6, 54), (2, 7, 53)], hand(&six)),
            frame(
                &[(1, 5, 54), (2, 7, 53)],
                vec![card(201, 2001, 1, "Battlefield")],
            ),
        ];

        let result = opening_hand(&frames, 1, 2);

        assert_eq!(result.mulliganed_hands.len(), 1);
        assert_eq!(result.bottomed, vec![207]);
        // opening keeps the full hand including the bottomed card.
        assert_eq!(result.opening.len(), 7);
        assert!(result.opening.contains(&HandCard {
            instance_id: 207,
            catalog_id: 2007
        }));
        // starting is the hand after bottoming.
        assert_eq!(result.starting.len(), 6);
        assert!(!ids(&result.starting).contains(&207));
    }

    #[test]
    fn detects_opponent_mulligans_via_library_count_difference() {
        // Opponent library at 54 after drawing: drew 7, shuffled, drew 6 = 1 mulligan.
        let frames = vec![
            pre_draw(),
            frame(&[(1, 7, 53), (2, 6, 54)], hand(&seven(1))),
            frame(
                &[(1, 6, 53), (2, 6, 54)],
                vec![card(101, 1001, 1, "Battlefield")],
            ),
        ];

        assert_eq!(opening_hand(&frames, 1, 2).opponent_mulligans, 1);
    }

    /// Dense frames record every change, so the first frame with an opponent
    /// hand is the initial 7-card draw, before any mulligan. The London
    /// mulligan's redraw never shows a 0 hand either (7 stays 7), so only the
    /// cards bottomed afterwards move the library back up.
    /// `opponent_frames` are `(hand count, library count)`.
    fn dense_opening_frames(opponent_frames: &[(i64, i64)]) -> Vec<Frame> {
        let mut frames = vec![pre_draw()];
        for &(hand_count, library_count) in opponent_frames {
            frames.push(frame(
                &[(1, 7, 53), (2, hand_count, library_count)],
                hand(&seven(1)),
            ));
        }
        let &(last_hand, last_library) = opponent_frames.last().expect("frames");
        frames.push(frame(
            &[(1, 6, 53), (2, last_hand, last_library)],
            vec![card(101, 1001, 1, "Battlefield")],
        ));
        frames
    }

    fn dense_opponent_mulligans(opponent_frames: &[(i64, i64)]) -> u32 {
        opening_hand(&dense_opening_frames(opponent_frames), 1, 2).opponent_mulligans
    }

    #[test]
    fn counts_opponent_mulligans_from_dense_frames() {
        // keeps seven
        assert_eq!(dense_opponent_mulligans(&[(7, 53), (7, 53)]), 0);
        // one mulligan, bottoms one
        assert_eq!(dense_opponent_mulligans(&[(7, 53), (7, 53), (6, 54)]), 1);
        // two mulligans, bottoms two
        assert_eq!(
            dense_opponent_mulligans(&[(7, 53), (7, 53), (7, 53), (6, 54), (5, 55)]),
            2
        );
        // on the draw, draws before the local player acts
        assert_eq!(dense_opponent_mulligans(&[(7, 53), (6, 54), (7, 53)]), 1);
    }

    #[test]
    fn skips_the_empty_placeholder_frame_written_before_the_decks_load() {
        let mut frames = dense_opening_frames(&[(7, 53), (6, 54)]);
        frames.insert(0, frame(&[(1, 0, 0), (2, 0, 0)], Vec::new()));

        assert_eq!(opening_hand(&frames, 1, 2).opponent_mulligans, 1);
    }

    #[test]
    fn only_counts_the_final_bottomed_card_when_player_swaps_bottom_selection() {
        // Mulligan to 6: the user clicks card 207 to bottom (hand drops to 6),
        // then changes their mind and selects 202 (hand briefly drops to 5
        // before settling at 6).
        let second = seven(2);
        let without = |out: &[i64]| -> Vec<(i64, u32)> {
            second
                .iter()
                .copied()
                .filter(|(id, _)| !out.contains(id))
                .collect()
        };
        let mut settled = without(&[202, 207]);
        settled.push((207, 2007));

        let frames = vec![
            pre_draw(),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&seven(1))),
            frame(&[(1, 7, 53), (2, 7, 53)], hand(&second)),
            frame(&[(1, 6, 54), (2, 7, 53)], hand(&without(&[207]))),
            frame(&[(1, 5, 55), (2, 7, 53)], hand(&without(&[202, 207]))),
            frame(&[(1, 6, 54), (2, 7, 53)], hand(&settled)),
            frame(
                &[(1, 5, 54), (2, 7, 53)],
                vec![card(201, 2001, 1, "Battlefield")],
            ),
        ];

        let result = opening_hand(&frames, 1, 2);

        assert_eq!(result.mulliganed_hands.len(), 1);
        // Only 202 is bottomed: 207 went back to the hand.
        assert_eq!(result.bottomed, vec![202]);
        assert_eq!(result.opening.len(), 7);
        assert!(result.opening.contains(&HandCard {
            instance_id: 202,
            catalog_id: 2002
        }));
        assert_eq!(result.starting.len(), 6);
        assert!(!ids(&result.starting).contains(&202));
        assert!(ids(&result.starting).contains(&207));
    }

    #[test]
    fn returns_empty_structure_for_game_with_no_timeline() {
        assert_eq!(opening_hand(&[], 1, 2), OpeningHand::default());
    }

    #[test]
    fn a_hand_change_of_fewer_than_4_new_cards_is_not_a_mulligan() {
        let frames = vec![
            pre_draw(),
            frame(
                &[(1, 3, 53), (2, 7, 53)],
                hand(&[(1, 10), (2, 20), (3, 30)]),
            ),
            frame(
                &[(1, 3, 53), (2, 7, 53)],
                hand(&[(4, 40), (5, 50), (6, 60)]),
            ),
        ];

        let result = opening_hand(&frames, 1, 2);

        assert!(result.mulliganed_hands.is_empty());
        assert_eq!(ids(&result.starting), vec![4, 5, 6]);
        // The earlier cards count towards the high-water hand, so they read as bottomed.
        assert_eq!(result.bottomed, vec![1, 2, 3]);
    }

    #[test]
    fn opponent_library_stops_at_the_first_frame_after_the_opening_phase() {
        // The opponent's hand only shows after the local player has played a
        // card; the first such frame still counts, later ones do not.
        let frames = vec![
            pre_draw(),
            frame(&[(1, 7, 53), (2, 0, 60)], hand(&seven(1))),
            frame(
                &[(1, 6, 53), (2, 0, 60)],
                vec![card(101, 1001, 1, "Battlefield")],
            ),
            frame(&[(1, 6, 53), (2, 6, 54)], Vec::new()),
            frame(&[(1, 6, 53), (2, 7, 56)], Vec::new()),
        ];

        assert_eq!(opening_hand(&frames, 1, 2).opponent_mulligans, 1);
    }

    #[test]
    fn shown_hand_marks_bottomed_cards_of_the_opening_hand() {
        let kept_only = OpeningHand {
            opening: vec![HandCard {
                instance_id: 5,
                catalog_id: 50,
            }],
            starting: vec![HandCard {
                instance_id: 5,
                catalog_id: 50,
            }],
            ..OpeningHand::default()
        };
        assert_eq!(
            kept_only.shown_hand(),
            vec![ShownCard {
                catalog_id: 50,
                bottomed: false
            }]
        );

        let bottomed = OpeningHand {
            starting: vec![HandCard {
                instance_id: 5,
                catalog_id: 50,
            }],
            bottomed: vec![6],
            opening: vec![
                HandCard {
                    instance_id: 6,
                    catalog_id: 60,
                },
                HandCard {
                    instance_id: 5,
                    catalog_id: 50,
                },
            ],
            ..OpeningHand::default()
        };
        assert_eq!(
            bottomed.shown_hand(),
            vec![
                ShownCard {
                    catalog_id: 60,
                    bottomed: true
                },
                ShownCard {
                    catalog_id: 50,
                    bottomed: false
                },
            ]
        );
    }

    // ParseOpeningHandPivotTest.php

    #[test]
    fn stored_hand_shapes_like_the_timeline() {
        let stored = StoredHand {
            starting: vec![4001, 4001, 4002, 4004, 4004, 4004],
            bottomed: vec![4002],
            mulligans: vec![vec![4004, 4004, 4004, 4004, 4004, 4004, 4001]],
        };

        let parsed = opening_hand_from_stored(&stored, 1, 2);

        assert_eq!(
            catalogs(&parsed.starting),
            vec![4001, 4001, 4002, 4004, 4004, 4004]
        );
        assert_eq!(ids(&parsed.starting), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(parsed.mulliganed_hands.len(), 1);
        assert_eq!(
            catalogs(&parsed.mulliganed_hands[0]),
            vec![4004, 4004, 4004, 4004, 4004, 4004, 4001]
        );
        assert_eq!(parsed.local_mulligans, 1);
        assert_eq!(parsed.opponent_mulligans, 2);
        assert_eq!(
            catalogs(&parsed.opening),
            vec![4001, 4001, 4002, 4004, 4004, 4004, 4002]
        );
        assert_eq!(parsed.bottomed, vec![6]);
        assert_eq!(
            parsed.shown_hand().last(),
            Some(&ShownCard {
                catalog_id: 4002,
                bottomed: true
            })
        );
    }

    #[test]
    fn keeps_unrecorded_mulliganed_hands_as_empty_placeholders() {
        let stored = StoredHand {
            starting: vec![4004, 4004, 4004, 4004, 4004],
            bottomed: vec![4004, 4001],
            mulligans: vec![vec![], vec![4001, 4001, 4001, 4001, 4004, 4004, 4004]],
        };

        let parsed = opening_hand_from_stored(&stored, 2, 0);

        assert_eq!(parsed.mulliganed_hands.len(), 2);
        assert!(parsed.mulliganed_hands[0].is_empty());
        assert_eq!(
            catalogs(&parsed.mulliganed_hands[1]),
            vec![4001, 4001, 4001, 4001, 4004, 4004, 4004]
        );
        assert_eq!(parsed.local_mulligans, 2);
        assert_eq!(parsed.bottomed, vec![5, 6]);
    }

    #[test]
    fn stored_hand_with_nothing_bottomed_has_no_bottomed_ids() {
        let stored = StoredHand {
            starting: vec![1, 2, 3, 4, 5, 6, 7],
            bottomed: Vec::new(),
            mulligans: Vec::new(),
        };

        let parsed = opening_hand_from_stored(&stored, 0, 0);

        assert!(parsed.bottomed.is_empty());
        // 0.x keeps the starting cards here even with nothing bottomed.
        assert_eq!(catalogs(&parsed.opening), vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(parsed.shown_hand().len(), 7);
    }

    #[test]
    fn falls_back_to_the_timeline_form_for_a_single_hand_frame() {
        // Pivot test "falls back to the timeline when the pivot hand is null":
        // the local player is seat 0 here.
        let catalog_ids = [4004, 4004, 4004, 4001, 4001, 4002, 4002];
        let cards = catalog_ids
            .iter()
            .enumerate()
            .map(|(i, &catalog_id)| card(100 + i as i64, catalog_id, 0, "Hand"))
            .collect();
        let frames = vec![frame(&[(0, 7, 53), (1, 7, 53)], cards)];

        let parsed = opening_hand(&frames, 0, 1);

        assert_eq!(catalogs(&parsed.starting), catalog_ids.to_vec());
        assert_eq!(parsed.local_mulligans, 0);
    }
}
