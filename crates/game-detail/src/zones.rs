//! Zone counters, port of 0.x `CountZonesByOracle`: per zone, the distinct
//! instances of the player's own cards seen there over the game, and the
//! copies that went from hand to the graveyard (discards).
//!
//! The stack is left out on purpose: a card on the stack is being cast, and
//! `cast` already counts that. MTGO reports a display zone (`Zone`) and a
//! real one (`ActualZone`); the real one wins wherever it is given (a card
//! exiled under Ugin's Labyrinth reads Zone "Battlefield" with ActualZone
//! "Exile", a companion reads Zone "Companion" with ActualZone "Sideboard",
//! which is no zone at all).
//!
//! Pure: the caller passes the frames, the owner's seat id and how a
//! catalog id maps to the row key.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::frame::Frame;

/// The zones counted, in the order a card usually meets them (0.x
/// `CountZonesByOracle::ZONES`).
pub const ZONES: [&str; 4] = ["hand", "graveyard", "exile", "battlefield"];

/// The index of the hand in `ZONES` and `ZoneCounts::zones`.
pub const HAND: usize = 0;
/// The index of the graveyard.
pub const GRAVEYARD: usize = 1;
/// The index of the exile.
pub const EXILE: usize = 2;
/// The index of the battlefield.
pub const BATTLEFIELD: usize = 3;

/// The counts of one player's cards in one game, by row key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoneCounts {
    /// Per zone (`ZONES` order): key to distinct instances seen there.
    pub zones: [BTreeMap<u32, u32>; 4],
    /// Key to distinct instances seen in hand and then in the graveyard.
    pub discarded: BTreeMap<u32, u32>,
}

impl ZoneCounts {
    /// The instances of `key` seen in zone `zone` (a `ZONES` index).
    pub fn count(&self, zone: usize, key: u32) -> u32 {
        self.zones[zone].get(&key).copied().unwrap_or(0)
    }

    /// The instances of `key` that were discarded.
    pub fn discards(&self, key: u32) -> u32 {
        self.discarded.get(&key).copied().unwrap_or(0)
    }
}

/// The zone of a frame card: `actual_zone` over `zone`, lower case; `None`
/// when it is not one of `ZONES`.
fn zone_index(zone: &str, actual_zone: &str) -> Option<usize> {
    let real = if actual_zone.is_empty() {
        zone
    } else {
        actual_zone
    };
    let lower = real.to_lowercase();
    ZONES.iter().position(|name| *name == lower)
}

/// Count `owner`'s cards per zone over the frames (in log order). `key_of`
/// maps a catalog id to the row key; a card it gives `None` for, or with no
/// instance id, is left out (0.x skipped a card without an oracle id or an
/// instance id).
pub fn count_zones(
    frames: &[Frame],
    owner: i64,
    key_of: &dyn Fn(u32) -> Option<u32>,
) -> ZoneCounts {
    let mut instances: [BTreeMap<u32, BTreeSet<i64>>; 4] = Default::default();
    let mut discarded: BTreeMap<u32, BTreeSet<i64>> = BTreeMap::new();
    let mut last_zone: HashMap<i64, usize> = HashMap::new();
    for frame in frames {
        for card in &frame.cards {
            if card.owner != owner || card.id == 0 {
                continue;
            }
            let Some(key) = key_of(card.catalog_id) else {
                continue;
            };
            let Some(zone) = zone_index(&card.zone, &card.actual_zone) else {
                continue;
            };
            instances[zone].entry(key).or_default().insert(card.id);
            if zone == GRAVEYARD && last_zone.get(&card.id) == Some(&HAND) {
                discarded.entry(key).or_default().insert(card.id);
            }
            last_zone.insert(card.id, zone);
        }
    }
    let mut counts = ZoneCounts::default();
    for (zone, by_key) in instances.into_iter().enumerate() {
        for (key, ids) in by_key {
            counts.zones[zone].insert(key, ids.len() as u32);
        }
    }
    for (key, ids) in discarded {
        counts.discarded.insert(key, ids.len() as u32);
    }
    counts
}

#[cfg(test)]
mod tests {
    //! Ports of 0.x `ZoneCountersTest`, run through `game_card_rows` as 0.x
    //! ran them through `ComputeCardGameStats`.

    use super::*;
    use crate::frame::{FrameCard, FramePlayer};
    use crate::game_cards::{GameCardRow, GameCardsInput, Side, game_card_rows};
    use crate::gamelog::GameLogEntry;
    use crate::log_cards::{GameLogCard, extract_log_cards};
    use decks::DeckCard;

    /// A card of the local player (seat 0) in `zone`, with no ActualZone.
    fn mine(id: i64, zone: &str) -> FrameCard {
        FrameCard {
            id,
            catalog_id: 1001,
            zone: zone.to_string(),
            actual_zone: String::new(),
            owner: 0,
            controller: 0,
        }
    }

    fn snapshot(cards: Vec<FrameCard>) -> Frame {
        let player = |id: i64, name: &str| FramePlayer {
            id,
            name: name.to_string(),
            library_count: 60,
            hand_count: 0,
            life: 20,
        };
        Frame {
            players: vec![player(0, "Qlocal"), player(1, "Qopponent")],
            cards,
        }
    }

    fn log(messages: &[&str]) -> Vec<GameLogCard> {
        let entries: Vec<GameLogEntry> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| GameLogEntry {
                timestamp_ms: index as i64 * 1000,
                message: message.to_string(),
            })
            .collect();
        extract_log_cards(&entries).game_cards(0, "Qlocal").to_vec()
    }

    fn name_of(id: u32) -> String {
        format!("Card {id}")
    }

    fn no_oracle(_: u32) -> Option<String> {
        None
    }

    /// The local row of card 1001 in a game with these snapshots, a deck of
    /// `quantity` copies and these log cards.
    fn zone_row(snapshots: Vec<Vec<FrameCard>>, quantity: u32, log: &[GameLogCard]) -> GameCardRow {
        let frames: Vec<Frame> = snapshots.into_iter().map(snapshot).collect();
        let deck = vec![DeckCard {
            catalog_id: 1001,
            quantity,
            sideboard: false,
        }];
        let input = GameCardsInput {
            frames: &frames,
            local_name: "Qlocal",
            on_play: Some(true),
            local_deck: &deck,
            game_one_deck: &[],
            recorded_opponent_cards: &[],
            reveal_log_cards: &[],
            has_log: !log.is_empty(),
            local_log_cards: log,
            opponent_log_cards: &[],
            local_pregame: &[],
            opponent_pregame: &[],
            log_starting_hands: (None, None),
            name_of: &name_of,
            oracle_of: &no_oracle,
        };
        game_card_rows(&input)
            .row(Side::Local, 1001)
            .expect("the deck card's row")
            .clone()
    }

    #[test]
    fn counts_each_zone_separately_rather_than_collapsing_them() {
        let row = zone_row(
            vec![
                vec![mine(10, "Hand"), mine(11, "Graveyard")],
                vec![mine(12, "Battlefield"), mine(13, "Exile")],
            ],
            4,
            &[],
        );
        assert_eq!(row.hand_seen, 1);
        assert_eq!(row.graveyard_seen, 1);
        assert_eq!(row.battlefield_seen, 1);
        assert_eq!(row.exile_seen, 1);
        assert!(row.has_zone_data);
        // seen is still the union, unchanged.
        assert_eq!(row.seen, 4);
    }

    #[test]
    fn counts_a_copy_that_went_from_hand_to_the_graveyard_as_discarded() {
        let row = zone_row(
            vec![vec![mine(10, "Hand")], vec![mine(10, "Graveyard")]],
            4,
            &[],
        );
        assert_eq!(row.discarded, 1);
    }

    const JOINS: [&str; 3] = [
        "@P@PQlocal joined the game.",
        "@P@PQopponent joined the game.",
        "@PTurn 1: Qlocal",
    ];

    #[test]
    fn does_not_call_a_cast_spell_a_discard() {
        let mut messages = JOINS.to_vec();
        messages.push("@PQlocal casts @[Card A@:2002,100:@].");
        messages.push("@PQlocal wins the game.");
        let row = zone_row(
            vec![vec![mine(10, "Hand")], vec![mine(10, "Graveyard")]],
            4,
            &log(&messages),
        );
        assert_eq!(row.cast, 1);
        assert_eq!(row.discarded, 0);
    }

    #[test]
    fn records_the_turn_a_card_was_first_cast_on() {
        let messages = [
            "@P@PQlocal joined the game.",
            "@P@PQopponent joined the game.",
            "@PTurn 1: Qlocal",
            "@PTurn 2: Qopponent",
            "@PTurn 3: Qlocal",
            "@PQlocal casts @[Card A@:2002,100:@].",
            "@PTurn 5: Qlocal",
            "@PQlocal casts @[Card A@:2002,100:@].",
            "@PQlocal wins the game.",
        ];
        let row = zone_row(vec![vec![mine(10, "Hand")]], 4, &log(&messages));
        // The first cast, not the last: how soon it came down.
        assert_eq!(row.cast_turn, Some(3));
    }

    #[test]
    fn leaves_the_cast_turn_empty_for_a_card_that_was_never_cast() {
        let row = zone_row(vec![vec![mine(10, "Hand")]], 4, &[]);
        assert_eq!(row.cast_turn, None);
    }

    #[test]
    fn reports_no_zone_data_for_a_game_with_no_timeline() {
        let mut messages = JOINS.to_vec();
        messages.push("@PQlocal casts @[Card A@:2002,100:@].");
        messages.push("@PQlocal wins the game.");
        let row = zone_row(Vec::new(), 4, &log(&messages));
        // Zero hand copies here is an absence of evidence; the flag says so.
        assert!(!row.has_zone_data);
        assert_eq!(row.hand_seen, 0);
        assert_eq!(row.cast, 1);
    }

    #[test]
    fn caps_a_zone_count_at_the_copies_in_the_deck() {
        // Five instances of a four-of: an instance id changed mid-game.
        let cards = [10, 11, 12, 13, 14]
            .iter()
            .map(|id| mine(*id, "Hand"))
            .collect();
        let row = zone_row(vec![cards], 4, &[]);
        assert_eq!(row.hand_seen, 4);
    }

    #[test]
    fn ignores_cards_owned_by_the_opponent() {
        let mut theirs = mine(20, "Hand");
        theirs.owner = 1;
        theirs.controller = 1;
        let row = zone_row(vec![vec![mine(10, "Hand"), theirs]], 4, &[]);
        assert_eq!(row.hand_seen, 1);
    }

    #[test]
    fn leaves_the_stack_out_of_the_zone_split() {
        // A card on the stack is one being cast, and `cast` records that.
        assert_eq!(ZONES, ["hand", "graveyard", "exile", "battlefield"]);
    }

    #[test]
    fn believes_actual_zone_over_the_display_zone() {
        // Exiled under Ugin's Labyrinth: Zone Battlefield, ActualZone Exile.
        let mut card = mine(10, "Battlefield");
        card.actual_zone = "Exile".to_string();
        let row = zone_row(vec![vec![card]], 4, &[]);
        assert_eq!(row.exile_seen, 1);
        assert_eq!(row.battlefield_seen, 0);
    }

    #[test]
    fn counts_a_companion_in_the_sideboard_as_no_zone_at_all() {
        let mut card = mine(10, "Companion");
        card.actual_zone = "Sideboard".to_string();
        let row = zone_row(vec![vec![card]], 4, &[]);
        assert_eq!(row.hand_seen, 0);
        assert_eq!(row.battlefield_seen, 0);
        assert_eq!(row.exile_seen, 0);
    }

    #[test]
    fn count_zones_keys_by_the_given_key_and_skips_unkeyed_cards() {
        let mut other = mine(11, "Hand");
        other.catalog_id = 1002;
        let frames = vec![snapshot(vec![mine(10, "Hand"), other])];
        let key_of = |id: u32| (id == 1001).then_some(7);
        let counts = count_zones(&frames, 0, &key_of);
        assert_eq!(counts.count(HAND, 7), 1);
        assert_eq!(counts.zones[HAND].len(), 1);
    }
}
