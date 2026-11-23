//! Draw odds (overlay spec 7.4, 0.x `ComputeDrawOdds`): how many copies of
//! each maindeck card are still in the local player's library, from the
//! newest frame of the current game.
//!
//! The caller picks the deck (the current game's first `deck_used` list, else
//! the match's deck version) and the frame (the newest `game_state_update` of
//! the current game), and finds the local seat. Probabilities are computed by
//! the page.

use std::collections::BTreeMap;

use decks::DeckCard;
use game_detail::frame::{Frame, FrameCard};
use serde::Serialize;

use crate::{CardMeta, display_name};

/// Zones whose cards still count as in the library: `Library` (still there)
/// and `Sideboard` (never in it). A `Stack` entry is a spell being cast or
/// an ability, which shares its source's catalog id (`is_ability`).
const NOT_SEEN_ZONES: [&str; 2] = ["Library", "Sideboard"];

/// The draw odds of one match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DrawOdds {
    /// One row per card name (printings merged), most copies left first.
    pub cards: Vec<DrawOddsCard>,
    /// The sum of every row's `remaining`.
    pub library_size: u32,
    /// The frame's `LibraryCount` for the local seat (0 without a frame).
    #[specta(type = f64)]
    pub live_library_count: i64,
}

/// One card name of the maindeck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DrawOddsCard {
    /// The first printing of the name in the deck.
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
    /// Copies still in the library.
    pub remaining: u32,
    /// Copies in the maindeck.
    pub total: u32,
}

/// The draw odds of a deck at a frame. `deck` may hold sideboard rows; they
/// are left out. `None` when the deck has no maindeck rows ("No deck data for
/// this match"). Without a frame the whole maindeck is in the library.
pub fn draw_odds(
    deck: &[DeckCard],
    frame: Option<&Frame>,
    local_seat: i64,
    cards: &CardMeta,
) -> Option<DrawOdds> {
    let maindeck: Vec<&DeckCard> = deck.iter().filter(|card| !card.sideboard).collect();
    if maindeck.is_empty() {
        return None;
    }

    let quantities = deck_quantities(&maindeck);
    let seen = match frame {
        Some(frame) => seen_outside_library(frame, local_seat, &quantities, cards),
        None => BTreeMap::new(),
    };

    let mut rows: Vec<DrawOddsCard> = Vec::new();
    let mut library_size = 0;
    for (catalog_id, total) in &quantities {
        let seen_copies = seen.get(catalog_id).copied().unwrap_or(0);
        let remaining = total.saturating_sub(seen_copies);
        library_size += remaining;
        add_printing(&mut rows, *catalog_id, remaining, *total, cards);
    }

    // A stable sort, so rows with the same count keep their deck order (0.x).
    rows.sort_by_key(|row| std::cmp::Reverse(row.remaining));

    let live_library_count = frame
        .and_then(|frame| frame.player(local_seat))
        .map(|player| player.library_count)
        .unwrap_or(0);

    Some(DrawOdds {
        cards: rows,
        library_size,
        live_library_count,
    })
}

/// Quantities per catalog id in deck order. A printing listed twice keeps
/// its first position and its last quantity (0.x `mapWithKeys`). Rows without
/// a catalog id are skipped.
fn deck_quantities(maindeck: &[&DeckCard]) -> Vec<(u32, u32)> {
    let mut quantities: Vec<(u32, u32)> = Vec::new();
    for card in maindeck {
        if card.catalog_id == 0 {
            continue;
        }
        match quantities.iter_mut().find(|(id, _)| *id == card.catalog_id) {
            Some(entry) => entry.1 = card.quantity,
            None => quantities.push((card.catalog_id, card.quantity)),
        }
    }
    quantities
}

/// Adds one printing to the row of its name (0.x merges printings by name).
fn add_printing(
    rows: &mut Vec<DrawOddsCard>,
    catalog_id: u32,
    remaining: u32,
    total: u32,
    cards: &CardMeta,
) {
    let name = display_name(cards, catalog_id);
    let art_crop = cards.art_crop(catalog_id).map(str::to_string);
    let info = cards.get(catalog_id);
    let local_art_crop = info.and_then(|info| info.local_art_crop.clone());
    if let Some(row) = rows.iter_mut().find(|row| row.name == name) {
        row.remaining += remaining;
        row.total += total;
        if row.art_crop.is_none() && art_crop.is_some() {
            row.art_crop = art_crop;
            row.local_art_crop = local_art_crop;
        }
        return;
    }
    rows.push(DrawOddsCard {
        catalog_id,
        name,
        card_type: info
            .and_then(|info| info.card_type.clone())
            .unwrap_or_else(|| "Unknown".to_string()),
        color_identity: info.and_then(|info| info.color_identity.clone()),
        image: info.and_then(|info| info.image.clone()),
        art_crop,
        local_image: info.and_then(|info| info.local_image.clone()),
        local_art_crop,
        remaining,
        total,
    });
}

/// Copies of each deck printing seen outside the local library, keyed by the
/// deck's catalog id. Several printings of one card: the first with copies
/// left is charged (0.x).
fn seen_outside_library(
    frame: &Frame,
    local_seat: i64,
    quantities: &[(u32, u32)],
    cards: &CardMeta,
) -> BTreeMap<u32, u32> {
    let resolver = Resolver::new(quantities, cards);
    let mut seen: BTreeMap<u32, u32> = BTreeMap::new();
    for card in &frame.cards {
        if card.owner != local_seat || NOT_SEEN_ZONES.contains(&card.zone.as_str()) {
            continue;
        }
        let candidates = resolver.candidates(card.catalog_id, cards);
        if card.zone == "Stack" && is_ability(card, &candidates, frame, &resolver, cards) {
            continue;
        }
        let Some(first) = candidates.first() else {
            continue;
        };
        let mut target = *first;
        for id in &candidates {
            let charged = seen.get(id).copied().unwrap_or(0);
            if charged < quantity_of(quantities, *id) {
                target = *id;
                break;
            }
        }
        *seen.entry(target).or_insert(0) += 1;
    }
    seen
}

/// Whether a local `Stack` entry is an ability rather than a spell. Frames
/// do not say, so it is read from the card: an instant or sorcery is a
/// spell, a land is never cast, and any other card is an ability when a
/// copy of it is on the battlefield (an enter or activated ability; a
/// second copy being cast then counts late, once it resolves).
fn is_ability(
    card: &FrameCard,
    candidates: &[u32],
    frame: &Frame,
    resolver: &Resolver,
    cards: &CardMeta,
) -> bool {
    let types = front_face_types(
        cards
            .get(card.catalog_id)
            .and_then(|info| info.card_type.as_deref()),
    );
    if types
        .iter()
        .any(|kind| kind == "instant" || kind == "sorcery")
    {
        return false;
    }
    if types.iter().any(|kind| kind == "land") {
        return true;
    }
    frame.cards.iter().any(|other| {
        other.owner == card.owner
            && other.zone == "Battlefield"
            && resolver.candidates(other.catalog_id, cards) == candidates
    })
}

/// The card types of a type line's front face, lower case: the words before
/// any ` // ` and any dash (`Legendary Creature - Goblin` gives `legendary`
/// and `creature`).
fn front_face_types(type_line: Option<&str>) -> Vec<String> {
    let Some(line) = type_line else {
        return Vec::new();
    };
    let front = line.split(" // ").next().unwrap_or("");
    let types = front.split(['\u{2014}', '-']).next().unwrap_or("");
    types
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect()
}

fn quantity_of(quantities: &[(u32, u32)], catalog_id: u32) -> u32 {
    quantities
        .iter()
        .find(|(id, _)| *id == catalog_id)
        .map(|(_, quantity)| *quantity)
        .unwrap_or(0)
}

/// Matches a frame's catalog id to deck printings: the exact id, then a
/// shared oracle id, then the printing's name against the deck cards' names
/// and faces (`A // B`), case-insensitively. Frames carry no card names, so
/// the name of an id the deck does not hold comes from its card row (a
/// modal double-faced card's back face has its own id and face name).
struct Resolver {
    deck_ids: Vec<u32>,
    ids_by_oracle: BTreeMap<String, Vec<u32>>,
    ids_by_face: BTreeMap<String, Vec<u32>>,
}

impl Resolver {
    fn new(quantities: &[(u32, u32)], cards: &CardMeta) -> Resolver {
        let mut resolver = Resolver {
            deck_ids: Vec::new(),
            ids_by_oracle: BTreeMap::new(),
            ids_by_face: BTreeMap::new(),
        };
        for (catalog_id, _) in quantities {
            resolver.deck_ids.push(*catalog_id);
            if let Some(oracle) = cards.oracle_id(*catalog_id) {
                push_id(&mut resolver.ids_by_oracle, oracle.to_string(), *catalog_id);
            }
            if let Some(name) = cards.name(*catalog_id) {
                for face in faces(name) {
                    push_id(&mut resolver.ids_by_face, face, *catalog_id);
                }
            }
        }
        resolver
    }

    fn candidates(&self, catalog_id: u32, cards: &CardMeta) -> Vec<u32> {
        if self.deck_ids.contains(&catalog_id) {
            return vec![catalog_id];
        }
        if let Some(oracle) = cards.oracle_id(catalog_id)
            && let Some(ids) = self.ids_by_oracle.get(oracle)
        {
            return ids.clone();
        }
        match cards.name(catalog_id) {
            Some(name) => self
                .ids_by_face
                .get(&name.to_lowercase())
                .cloned()
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

fn push_id(map: &mut BTreeMap<String, Vec<u32>>, key: String, catalog_id: u32) {
    let ids = map.entry(key).or_default();
    if !ids.contains(&catalog_id) {
        ids.push(catalog_id);
    }
}

/// A card's full name and each face of an `A // B` name, lower case, without
/// repeats or empty names.
fn faces(name: &str) -> Vec<String> {
    let mut faces: Vec<String> = Vec::new();
    let mut add = |face: &str| {
        let face = face.to_lowercase();
        if !face.is_empty() && !faces.contains(&face) {
            faces.push(face);
        }
    };
    add(name);
    for face in name.split(" // ") {
        add(face);
    }
    faces
}

#[cfg(test)]
mod tests {
    use game_detail::frame::{FrameCard, FramePlayer};

    use super::*;
    use crate::CardInfo;

    fn card(name: &str, oracle: &str, card_type: &str) -> CardInfo {
        CardInfo {
            name: Some(name.to_string()),
            oracle_id: Some(oracle.to_string()),
            card_type: Some(card_type.to_string()),
            ..CardInfo::default()
        }
    }

    fn meta(rows: Vec<(u32, CardInfo)>) -> CardMeta {
        CardMeta {
            cards: rows.into_iter().collect(),
        }
    }

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

    fn frame_card(catalog_id: u32, owner: i64, zone: &str) -> FrameCard {
        FrameCard {
            catalog_id,
            owner,
            zone: zone.to_string(),
            ..FrameCard::default()
        }
    }

    fn frame(library_count: i64, cards: Vec<FrameCard>) -> Frame {
        Frame {
            players: vec![FramePlayer {
                id: 1,
                name: "QuietLocal".to_string(),
                library_count,
                ..FramePlayer::default()
            }],
            cards,
        }
    }

    fn row<'a>(odds: &'a DrawOdds, name: &str) -> &'a DrawOddsCard {
        odds.cards.iter().find(|card| card.name == name).unwrap()
    }

    fn mountain_and_bolt() -> CardMeta {
        meta(vec![
            (101, card("Mountain", "o-mountain", "Basic Land")),
            (102, card("Lightning Bolt", "o-bolt", "Instant")),
        ])
    }

    #[test]
    fn returns_null_when_the_match_has_no_deck_version() {
        assert_eq!(draw_odds(&[], None, 1, &CardMeta::default()), None);
    }

    #[test]
    fn computes_full_deck_draw_chances_when_there_is_no_game_timeline() {
        let odds = draw_odds(
            &[main(101, 20), main(102, 4)],
            None,
            1,
            &mountain_and_bolt(),
        )
        .unwrap();

        assert_eq!(odds.library_size, 24);
        let bolt = row(&odds, "Lightning Bolt");
        assert_eq!((bolt.remaining, bolt.total), (4, 4));
    }

    #[test]
    fn subtracts_cards_the_local_player_has_moved_out_of_library() {
        let frame = frame(
            21,
            vec![
                frame_card(102, 1, "Hand"),
                frame_card(102, 1, "Hand"),
                frame_card(101, 1, "Battlefield"),
                // A card still in the library is not subtracted.
                frame_card(101, 1, "Library"),
                // The opponent's card is ignored.
                frame_card(102, 2, "Hand"),
            ],
        );

        let odds = draw_odds(
            &[main(101, 20), main(102, 4)],
            Some(&frame),
            1,
            &mountain_and_bolt(),
        )
        .unwrap();

        let bolt = row(&odds, "Lightning Bolt");
        assert_eq!((bolt.remaining, bolt.total), (2, 4));
        assert_eq!(row(&odds, "Mountain").remaining, 19);
        assert_eq!(odds.library_size, 21);
        assert_eq!(odds.live_library_count, 21);
    }

    // 0.x picked the newest snapshot by timestamp; v1's caller passes the
    // newest frame by log position, so the rule here is "the frame given".
    #[test]
    fn uses_the_latest_timeline_snapshot_when_multiple_rows_exist() {
        let newest = frame(
            22,
            vec![
                frame_card(102, 1, "Hand"),
                frame_card(101, 1, "Battlefield"),
            ],
        );

        let odds = draw_odds(
            &[main(101, 20), main(102, 4)],
            Some(&newest),
            1,
            &mountain_and_bolt(),
        )
        .unwrap();

        assert_eq!(row(&odds, "Lightning Bolt").remaining, 3);
        assert_eq!(row(&odds, "Mountain").remaining, 19);
        assert_eq!(odds.library_size, 22);
        assert_eq!(odds.live_library_count, 22);
    }

    #[test]
    fn excludes_sideboard_cards() {
        let cards = meta(vec![
            (101, card("Mountain", "o-mountain", "Basic Land")),
            (301, card("Rest in Peace", "o-rip", "Enchantment")),
        ]);

        let odds = draw_odds(&[main(101, 20), side(301, 3)], None, 1, &cards).unwrap();

        assert_eq!(odds.library_size, 20);
        let names: Vec<&str> = odds.cards.iter().map(|card| card.name.as_str()).collect();
        assert_eq!(names, vec!["Mountain"]);
    }

    // The caller passes the game's `deck_used` list when it has one; this
    // checks the sided list is read as given (sided-in main, sided-out side).
    #[test]
    fn prefers_per_game_deck_json_over_the_match_deck_version() {
        let cards = meta(vec![
            (101, card("Mountain", "o-mountain", "Basic Land")),
            (102, card("Lightning Bolt", "o-bolt", "Instant")),
            (301, card("Rest in Peace", "o-rip", "Enchantment")),
        ]);
        let game_deck = [main(101, 20), main(102, 1), side(102, 3), main(301, 3)];

        let odds = draw_odds(&game_deck, None, 1, &cards).unwrap();

        assert_eq!(odds.library_size, 24);
        assert!(odds.cards.iter().any(|card| card.name == "Rest in Peace"));
        assert_eq!(row(&odds, "Lightning Bolt").total, 1);
    }

    #[test]
    fn does_not_subtract_stack_entries_from_remaining() {
        let cards = meta(vec![
            (501, card("Lembas", "o-lembas", "Artifact - Food")),
            (101, card("Mountain", "o-mountain", "Basic Land")),
        ]);
        let frame = frame(
            23,
            vec![
                frame_card(501, 1, "Battlefield"),
                frame_card(501, 1, "Stack"),
            ],
        );

        let odds = draw_odds(&[main(101, 20), main(501, 4)], Some(&frame), 1, &cards).unwrap();

        let lembas = row(&odds, "Lembas");
        assert_eq!((lembas.remaining, lembas.total), (3, 4));
    }

    // An instant or sorcery on the stack is a spell being cast: it left the
    // library (four Fatal Push, one in hand, one cast: two left).
    #[test]
    fn subtracts_an_instant_being_cast() {
        let cards = meta(vec![
            (601, card("Fatal Push", "o-push", "Instant")),
            (101, card("Mountain", "o-mountain", "Basic Land")),
        ]);
        let frame = frame(
            22,
            vec![frame_card(601, 1, "Hand"), frame_card(601, 1, "Stack")],
        );

        let odds = draw_odds(&[main(101, 20), main(601, 4)], Some(&frame), 1, &cards).unwrap();

        let push = row(&odds, "Fatal Push");
        assert_eq!((push.remaining, push.total), (2, 4));
    }

    #[test]
    fn subtracts_a_permanent_spell_with_no_copy_on_the_battlefield() {
        let cards = meta(vec![
            (
                602,
                card(
                    "Ragavan, Nimble Pilferer",
                    "o-ragavan",
                    "Legendary Creature",
                ),
            ),
            (101, card("Mountain", "o-mountain", "Basic Land")),
        ]);
        let frame = frame(23, vec![frame_card(602, 1, "Stack")]);

        let odds = draw_odds(&[main(101, 20), main(602, 4)], Some(&frame), 1, &cards).unwrap();

        assert_eq!(row(&odds, "Ragavan, Nimble Pilferer").remaining, 3);
    }

    // A land is never cast: on the stack it is an ability (a fetch land's,
    // sacrificed to the graveyard).
    #[test]
    fn does_not_subtract_a_land_ability_on_the_stack() {
        let cards = meta(vec![
            (603, card("Scalding Tarn", "o-tarn", "Land")),
            (101, card("Mountain", "o-mountain", "Basic Land")),
        ]);
        let frame = frame(
            23,
            vec![frame_card(603, 1, "Graveyard"), frame_card(603, 1, "Stack")],
        );

        let odds = draw_odds(&[main(101, 20), main(603, 4)], Some(&frame), 1, &cards).unwrap();

        assert_eq!(row(&odds, "Scalding Tarn").remaining, 3);
    }

    #[test]
    fn does_not_subtract_sideboard_zone_copies_from_remaining() {
        let cards = meta(vec![
            (401, card("Nihil Spellbomb", "o-nihil", "Artifact")),
            (101, card("Mountain", "o-mountain", "Basic Land")),
        ]);
        let frame = frame(22, vec![frame_card(401, 1, "Sideboard")]);

        let odds = draw_odds(
            &[main(101, 20), main(401, 2), side(401, 1)],
            Some(&frame),
            1,
            &cards,
        )
        .unwrap();

        let nihil = row(&odds, "Nihil Spellbomb");
        assert_eq!((nihil.remaining, nihil.total), (2, 2));
    }

    #[test]
    fn includes_card_identity_image_and_catalog_id_in_the_payload() {
        let cards = meta(vec![(
            201,
            CardInfo {
                name: Some("Snapcaster Mage".to_string()),
                oracle_id: Some("o-x".to_string()),
                card_type: Some("Creature".to_string()),
                color_identity: Some("U".to_string()),
                image: Some("https://img/snap.jpg".to_string()),
                ..CardInfo::default()
            },
        )]);

        let odds = draw_odds(&[main(201, 4)], None, 1, &cards).unwrap();

        let snap = row(&odds, "Snapcaster Mage");
        assert_eq!(snap.catalog_id, 201);
        assert_eq!(snap.color_identity.as_deref(), Some("U"));
        assert_eq!(snap.image.as_deref(), Some("https://img/snap.jpg"));
        assert_eq!(snap.card_type, "Creature");
    }

    #[test]
    fn merges_different_printings_of_the_same_card_into_one_row() {
        let mut rows: Vec<(u32, CardInfo)> = [201, 202, 203, 204]
            .into_iter()
            .map(|id| (id, card("Urza's Mine", "o-mine", "Land")))
            .collect();
        rows.push((205, card("Eldrazi Temple", "o-temple", "Land")));
        let deck = [
            main(201, 1),
            main(202, 1),
            main(203, 1),
            main(204, 1),
            main(205, 4),
        ];

        let odds = draw_odds(&deck, None, 1, &meta(rows)).unwrap();

        let mines: Vec<&DrawOddsCard> = odds
            .cards
            .iter()
            .filter(|card| card.name == "Urza's Mine")
            .collect();
        assert_eq!(mines.len(), 1);
        assert_eq!((mines[0].remaining, mines[0].total), (4, 4));
        assert_eq!(odds.library_size, 8);
    }

    #[test]
    fn subtracts_a_drawn_printing_from_the_merged_row() {
        let cards = meta(vec![
            (201, card("Urza's Mine", "o-mine", "Land")),
            (202, card("Urza's Mine", "o-mine", "Land")),
        ]);
        let frame = frame(1, vec![frame_card(202, 1, "Hand")]);

        let odds = draw_odds(&[main(201, 1), main(202, 1)], Some(&frame), 1, &cards).unwrap();

        assert_eq!(odds.cards.len(), 1);
        assert_eq!((odds.cards[0].remaining, odds.cards[0].total), (1, 2));
    }

    /// Spec 7.1: the downloaded files beside the remote URLs, the art crop's
    /// file from the printing whose art crop is shown.
    #[test]
    fn carries_the_local_files_of_the_shown_printing() {
        let mut first = card("Urza's Mine", "o-mine", "Land");
        first.local_image = Some("211.jpg".to_string());
        let mut with_art = card("Urza's Mine", "o-mine", "Land");
        with_art.art_crop = Some("https://img/mine-art.jpg".to_string());
        with_art.local_art_crop = Some("212_art.jpg".to_string());
        let cards = meta(vec![(211, first), (212, with_art)]);

        let odds = draw_odds(&[main(211, 1), main(212, 1)], None, 1, &cards).unwrap();

        let mine = row(&odds, "Urza's Mine");
        assert_eq!(mine.local_image.as_deref(), Some("211.jpg"));
        assert_eq!(mine.local_art_crop.as_deref(), Some("212_art.jpg"));
    }

    #[test]
    fn includes_the_art_crop_in_the_payload_taking_the_first_printing_that_has_one() {
        let mut with_art = card("Urza's Mine", "o-mine", "Land");
        with_art.art_crop = Some("https://img/mine-art.jpg".to_string());
        let cards = meta(vec![
            (211, card("Urza's Mine", "o-mine", "Land")),
            (212, with_art),
        ]);

        let odds = draw_odds(&[main(211, 1), main(212, 1)], None, 1, &cards).unwrap();

        assert_eq!(
            row(&odds, "Urza's Mine").art_crop.as_deref(),
            Some("https://img/mine-art.jpg")
        );
    }

    // 0.x read the back face's name from the snapshot; v1 frames carry no
    // names, so it comes from the face id's own card row (no oracle id).
    #[test]
    fn subtracts_a_modal_double_faced_card_played_as_its_back_face_when_the_face_id_has_no_oracle_link()
     {
        let cards = meta(vec![
            (
                126501,
                card(
                    "Witch Enchanter // Witch-Blessed Meadow",
                    "o-witch",
                    "Creature",
                ),
            ),
            (
                126503,
                CardInfo {
                    name: Some("Witch-Blessed Meadow".to_string()),
                    ..CardInfo::default()
                },
            ),
        ]);
        let frame = frame(
            20,
            vec![
                frame_card(126501, 1, "Hand"),
                frame_card(126503, 1, "Battlefield"),
            ],
        );

        let odds = draw_odds(&[main(126501, 4)], Some(&frame), 1, &cards).unwrap();

        let witch = row(&odds, "Witch Enchanter // Witch-Blessed Meadow");
        assert_eq!((witch.remaining, witch.total), (2, 4));
    }

    #[test]
    fn subtracts_a_card_seen_under_another_printing_of_the_same_oracle_card() {
        let name = "Witch Enchanter // Witch-Blessed Meadow";
        let cards = meta(vec![
            (126501, card(name, "o-witch", "Creature")),
            (126503, card(name, "o-witch", "Creature")),
        ]);
        let frame = frame(20, vec![frame_card(126503, 1, "Battlefield")]);

        let odds = draw_odds(&[main(126501, 4)], Some(&frame), 1, &cards).unwrap();

        assert_eq!(odds.cards.len(), 1);
        assert_eq!(odds.cards[0].remaining, 3);
        assert_eq!(odds.library_size, 3);
    }

    #[test]
    fn ignores_snapshot_cards_that_match_nothing_in_the_deck() {
        let cards = meta(vec![
            (102, card("Lightning Bolt", "o-bolt", "Instant")),
            (
                777777,
                CardInfo {
                    name: Some("Treasure".to_string()),
                    ..CardInfo::default()
                },
            ),
        ]);
        let frame = frame(20, vec![frame_card(777777, 1, "Battlefield")]);

        let odds = draw_odds(&[main(102, 4)], Some(&frame), 1, &cards).unwrap();

        assert_eq!(odds.cards[0].remaining, 4);
    }

    #[test]
    fn rows_are_sorted_by_copies_left_keeping_deck_order_on_ties() {
        let cards = meta(vec![
            (101, card("Mountain", "o-mountain", "Basic Land")),
            (102, card("Lightning Bolt", "o-bolt", "Instant")),
            (103, card("Lava Spike", "o-spike", "Sorcery")),
        ]);

        let odds = draw_odds(
            &[main(102, 4), main(103, 4), main(101, 20)],
            None,
            1,
            &cards,
        )
        .unwrap();

        let names: Vec<&str> = odds.cards.iter().map(|card| card.name.as_str()).collect();
        assert_eq!(names, vec!["Mountain", "Lightning Bolt", "Lava Spike"]);
    }

    #[test]
    fn a_card_without_a_row_shows_its_id_and_an_unknown_type() {
        let odds = draw_odds(&[main(999, 2)], None, 1, &CardMeta::default()).unwrap();

        assert_eq!(odds.cards[0].name, "#999");
        assert_eq!(odds.cards[0].card_type, "Unknown");
    }
}
