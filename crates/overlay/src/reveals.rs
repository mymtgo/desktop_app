//! The opponent's revealed cards (overlay spec 7.5, 0.x
//! `AggregateOpponentCards` and `GetOpponentReveals`).
//!
//! The caller reads each game's newest frame and takes the opponent's cards
//! with `game_detail::opponent_cards`; this module sums them across games,
//! merges printings by name and fingerprints the set.

use serde::Serialize;

use crate::{CardMeta, display_name};

/// The most copies of a card a constructed deck can hold.
const MAX_COPIES: u32 = 4;

/// One revealed card name (printings merged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RevealedCard {
    /// The first printing of the name.
    pub catalog_id: u32,
    /// The card name, or `#{catalog_id}` without a card row.
    pub name: String,
    /// The card type, or "Unknown" without one.
    pub card_type: String,
    pub color_identity: Option<String>,
    pub image: Option<String>,
    /// The art crop of the first printing that has one.
    pub art_crop: Option<String>,
    /// The first printing's image file in v1's card images folder.
    pub local_image: Option<String>,
    /// The art crop file of the printing whose art crop is shown.
    pub local_art_crop: Option<String>,
    /// Copies revealed, at most 4.
    pub quantity: u32,
}

/// The opponent's cards across a match's games: `(catalog id, copies)`
/// summed per printing and capped at 4, in first-seen order. Catalog id 0 is
/// skipped.
pub fn aggregate_reveals(per_game: &[Vec<(u32, u32)>]) -> Vec<(u32, u32)> {
    let mut totals: Vec<(u32, u32)> = Vec::new();
    for game in per_game {
        for (catalog_id, quantity) in game {
            if *catalog_id == 0 {
                continue;
            }
            match totals.iter_mut().find(|(id, _)| id == catalog_id) {
                Some(entry) => entry.1 += quantity,
                None => totals.push((*catalog_id, *quantity)),
            }
        }
    }
    for entry in &mut totals {
        entry.1 = entry.1.min(MAX_COPIES);
    }
    totals
}

/// Revealed printings merged by card name (copies capped at 4), sorted by
/// name, with card details from `cards`.
pub fn merge_by_name(revealed: &[(u32, u32)], cards: &CardMeta) -> Vec<RevealedCard> {
    let mut rows: Vec<RevealedCard> = Vec::new();
    for (catalog_id, quantity) in revealed {
        let name = display_name(cards, *catalog_id);
        let art_crop = cards.art_crop(*catalog_id).map(str::to_string);
        let info = cards.get(*catalog_id);
        let local_art_crop = info.and_then(|info| info.local_art_crop.clone());
        if let Some(row) = rows.iter_mut().find(|row| row.name == name) {
            row.quantity += quantity;
            if row.art_crop.is_none() && art_crop.is_some() {
                row.art_crop = art_crop;
                row.local_art_crop = local_art_crop;
            }
            continue;
        }
        rows.push(RevealedCard {
            catalog_id: *catalog_id,
            name,
            card_type: info
                .and_then(|info| info.card_type.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            color_identity: info.and_then(|info| info.color_identity.clone()),
            image: info.and_then(|info| info.image.clone()),
            art_crop,
            local_image: info.and_then(|info| info.local_image.clone()),
            local_art_crop,
            quantity: *quantity,
        });
    }
    for row in &mut rows {
        row.quantity = row.quantity.min(MAX_COPIES);
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// A key for a set of revealed cards: `catalog:copies` sorted by catalog id,
/// joined by `|` (0.x hashed the same text with md5; v1 keeps the text, which
/// is only an in-memory cache key).
pub fn fingerprint(revealed: &[(u32, u32)]) -> String {
    let mut sorted: Vec<(u32, u32)> = revealed.to_vec();
    sorted.sort();
    let parts: Vec<String> = sorted
        .iter()
        .map(|(catalog_id, quantity)| format!("{catalog_id}:{quantity}"))
        .collect();
    parts.join("|")
}

/// New revealed cards that ask for a fresh API estimate at once (#502).
pub const ESTIMATE_MIN_NEW_CARDS: u32 = 2;

/// Since the last estimate asked, the wait after which one new card is
/// enough to ask again.
pub const ESTIMATE_GAP_MS: i64 = 20_000;

/// The copies in `revealed` beyond those in `asked`: the cards revealed
/// since the last estimate.
pub fn new_cards(asked: &[(u32, u32)], revealed: &[(u32, u32)]) -> u32 {
    let mut count = 0;
    for (catalog_id, quantity) in revealed {
        let before = asked
            .iter()
            .find(|(asked_id, _)| asked_id == catalog_id)
            .map_or(0, |(_, quantity)| *quantity);
        count += quantity.saturating_sub(before);
    }
    count
}

/// Whether a match's reveals call for a new API estimate (#502), given the
/// set its last estimate asked about and how long ago: at once after
/// `ESTIMATE_MIN_NEW_CARDS` new cards, after `ESTIMATE_GAP_MS` for one, never
/// for none. Each revealed card used to ask on its own.
pub fn estimate_due(asked: &[(u32, u32)], revealed: &[(u32, u32)], since_ms: i64) -> bool {
    let new = new_cards(asked, revealed);
    new >= ESTIMATE_MIN_NEW_CARDS || (new > 0 && since_ms >= ESTIMATE_GAP_MS)
}

#[cfg(test)]
mod tests {
    use game_detail::frame::{Frame, FrameCard, FramePlayer};
    use game_detail::opponent_cards;

    use super::*;
    use crate::CardInfo;

    fn card(name: &str, card_type: &str) -> CardInfo {
        CardInfo {
            name: Some(name.to_string()),
            card_type: Some(card_type.to_string()),
            ..CardInfo::default()
        }
    }

    fn meta(rows: Vec<(u32, CardInfo)>) -> CardMeta {
        CardMeta {
            cards: rows.into_iter().collect(),
        }
    }

    #[test]
    fn returns_revealed_opponent_cards_with_resolved_metadata() {
        let mut bolt = card("Lightning Bolt", "Instant");
        bolt.color_identity = Some("R".to_string());
        let cards = meta(vec![(102, bolt)]);

        let reveals = merge_by_name(&aggregate_reveals(&[vec![(102, 2)]]), &cards);

        assert_eq!(reveals.len(), 1);
        assert_eq!(reveals[0].name, "Lightning Bolt");
        assert_eq!(reveals[0].card_type, "Instant");
        assert_eq!(reveals[0].color_identity.as_deref(), Some("R"));
        assert_eq!(reveals[0].quantity, 2);
    }

    #[test]
    fn aggregates_reveals_across_games_and_caps_at_four_copies() {
        let cards = meta(vec![(102, card("Lightning Bolt", "Instant"))]);

        let totals = aggregate_reveals(&[vec![(102, 3)], vec![(102, 3)]]);
        let reveals = merge_by_name(&totals, &cards);

        assert_eq!(totals, vec![(102, 4)]);
        assert_eq!(reveals.len(), 1);
        assert_eq!(reveals[0].quantity, 4);
    }

    #[test]
    fn merges_different_printings_of_the_same_card_by_name() {
        let cards = meta(vec![
            (301, card("Urza's Mine", "Land")),
            (302, card("Urza's Mine", "Land")),
        ]);

        let reveals = merge_by_name(&aggregate_reveals(&[vec![(301, 1), (302, 1)]]), &cards);

        assert_eq!(reveals.len(), 1);
        assert_eq!(reveals[0].name, "Urza's Mine");
        assert_eq!(reveals[0].quantity, 2);
        assert_eq!(reveals[0].catalog_id, 301);
    }

    #[test]
    fn merged_printings_are_capped_at_four_copies() {
        let cards = meta(vec![
            (301, card("Urza's Mine", "Land")),
            (302, card("Urza's Mine", "Land")),
        ]);

        let reveals = merge_by_name(&[(301, 3), (302, 3)], &cards);

        assert_eq!(reveals[0].quantity, 4);
    }

    #[test]
    fn returns_an_empty_collection_when_nothing_has_been_revealed() {
        let reveals = merge_by_name(&aggregate_reveals(&[Vec::new()]), &CardMeta::default());

        assert!(reveals.is_empty());
    }

    // 0.x read only non-local players' decks; v1 reads the opponent seat's
    // cards from each game's newest frame, so the local seat's are never in.
    #[test]
    fn ignores_the_local_player_deck() {
        let frame = Frame {
            players: vec![
                FramePlayer {
                    id: 1,
                    name: "QuietLocal".to_string(),
                    ..FramePlayer::default()
                },
                FramePlayer {
                    id: 2,
                    name: "QuickRival".to_string(),
                    ..FramePlayer::default()
                },
            ],
            cards: vec![FrameCard {
                catalog_id: 102,
                owner: 1,
                zone: "Battlefield".to_string(),
                ..FrameCard::default()
            }],
        };
        let cards = meta(vec![(102, card("Lightning Bolt", "Instant"))]);

        let per_game = vec![opponent_cards(&frame, 2)];
        let reveals = merge_by_name(&aggregate_reveals(&per_game), &cards);

        assert!(reveals.is_empty());
    }

    /// Spec 7.1: the downloaded files beside the remote URLs.
    #[test]
    fn carries_the_local_files_of_the_card() {
        let mut bolt = card("Lightning Bolt", "Instant");
        bolt.local_image = Some("102.jpg".to_string());
        bolt.local_art_crop = Some("102_art.jpg".to_string());
        let cards = meta(vec![(102, bolt)]);

        let reveals = merge_by_name(&[(102, 1)], &cards);

        assert_eq!(reveals[0].local_image.as_deref(), Some("102.jpg"));
        assert_eq!(reveals[0].local_art_crop.as_deref(), Some("102_art.jpg"));
    }

    #[test]
    fn includes_the_art_crop_in_the_payload() {
        let mut bolt = card("Lightning Bolt", "Instant");
        bolt.art_crop = Some("https://img/bolt-art.jpg".to_string());
        let cards = meta(vec![(102, bolt)]);

        let reveals = merge_by_name(&[(102, 1)], &cards);

        assert_eq!(
            reveals[0].art_crop.as_deref(),
            Some("https://img/bolt-art.jpg")
        );
    }

    #[test]
    fn reveals_are_sorted_by_name() {
        let cards = meta(vec![
            (1, card("Thoughtseize", "Sorcery")),
            (2, card("Fatal Push", "Instant")),
        ]);

        let reveals = merge_by_name(&[(1, 1), (2, 1)], &cards);

        let names: Vec<&str> = reveals.iter().map(|card| card.name.as_str()).collect();
        assert_eq!(names, vec!["Fatal Push", "Thoughtseize"]);
    }

    #[test]
    fn the_fingerprint_sorts_by_catalog_id() {
        assert_eq!(fingerprint(&[(302, 1), (102, 4)]), "102:4|302:1");
        assert_eq!(fingerprint(&[]), "");
    }

    #[test]
    fn new_cards_are_the_copies_beyond_the_asked_set() {
        assert_eq!(new_cards(&[(1, 1)], &[(1, 1)]), 0);
        assert_eq!(new_cards(&[(1, 1)], &[(1, 2), (5, 1)]), 2);
        assert_eq!(new_cards(&[], &[(1, 3)]), 3);
        assert_eq!(new_cards(&[(1, 2)], &[(1, 1)]), 0, "fewer is not new");
    }

    // #502: two new cards ask at once; one waits 20 seconds; none never.
    #[test]
    fn an_estimate_is_due_after_two_new_cards_or_one_and_a_wait() {
        let asked = [(1, 1)];
        assert!(estimate_due(&asked, &[(1, 1), (2, 1), (3, 1)], 0));
        assert!(!estimate_due(&asked, &[(1, 1), (2, 1)], 19_999));
        assert!(estimate_due(&asked, &[(1, 1), (2, 1)], 20_000));
        assert!(!estimate_due(&asked, &[(1, 1)], 3_600_000));
    }
}
