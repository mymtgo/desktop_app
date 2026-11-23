//! The Metagame page's numbers (archetypes spec section 9) and the archetype
//! detail's cards seen (section 10).
//!
//! - `Timeframe`: "Last 30 days", "Last 7 days" and "All time", each read
//!   against the website's 30, 7 and 90 day windows (ruling 7).
//! - `tiles`: one tile per opponent archetype faced: the player's share of
//!   the format's matches, the global share, the difference and the record.
//! - `group_seen` and `mark_typical`: the opponent cards seen against an
//!   archetype, grouped by oracle id, and whether a typical list holds them.
//!
//! Pure: the store reads the rows and the app passes the API's shares in.

use std::collections::BTreeMap;

use decks::php_round;
use decks::stats::Record;

use crate::Source;

/// The page's timeframes. The player's window is in local days; the global
/// column reads the website's window of `global_days`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    Last30,
    Last7,
    AllTime,
}

impl Timeframe {
    /// In the order the switch shows them.
    pub const ALL: [Timeframe; 3] = [Timeframe::Last30, Timeframe::Last7, Timeframe::AllTime];

    /// Reads a key from the route; anything else is the default, 30 days.
    pub fn parse(key: &str) -> Timeframe {
        match key {
            "last7" => Timeframe::Last7,
            "alltime" => Timeframe::AllTime,
            _ => Timeframe::Last30,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Timeframe::Last30 => "last30",
            Timeframe::Last7 => "last7",
            Timeframe::AllTime => "alltime",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Timeframe::Last30 => "Last 30 days",
            Timeframe::Last7 => "Last 7 days",
            Timeframe::AllTime => "All time",
        }
    }

    /// The website window the global column reads (`?days=`).
    pub fn global_days(self) -> u32 {
        match self {
            Timeframe::Last30 => 30,
            Timeframe::Last7 => 7,
            Timeframe::AllTime => 90,
        }
    }

    /// Local days in the player's window, today included; `None` for all
    /// time.
    pub fn local_days(self) -> Option<u32> {
        match self {
            Timeframe::Last30 => Some(30),
            Timeframe::Last7 => Some(7),
            Timeframe::AllTime => None,
        }
    }
}

/// One opponent archetype faced in the window, with the record against it.
#[derive(Debug, Clone, PartialEq)]
pub struct Faced {
    pub archetype_id: i64,
    pub uuid: String,
    pub name: String,
    pub color_identity: Option<String>,
    pub source: Source,
    pub retired: bool,
    /// A server archetype held as Custom by the offline download.
    pub offline_held: bool,
    pub record: Record,
}

/// One tile of the Metagame page.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub archetype_id: i64,
    pub uuid: String,
    pub name: String,
    pub color_identity: Option<String>,
    pub source: Source,
    pub retired: bool,
    /// A server archetype held as Custom by the offline download.
    pub offline_held: bool,
    /// Matches against it; the win rate counts draws (0.x `MatchRecord`).
    pub record: Record,
    /// Percent of the format's complete matches in the window, one decimal.
    pub share: f64,
    /// Whether the website can list it: downloaded and not retired. The
    /// others show "-" for global and difference.
    pub comparable: bool,
    /// The website's share, one decimal; `None` when not comparable, not
    /// listed, or the shares are not here.
    pub global_share: Option<f64>,
    /// `share - global_share`, one decimal.
    pub difference: Option<f64>,
}

/// `matches` of `total` as a percent with one decimal; 0 without matches.
pub fn share_of(matches: u32, total: u32) -> f64 {
    if total == 0 {
        return 0.0;
    }
    php_round(100.0 * f64::from(matches) / f64::from(total), 1)
}

/// The tiles for the archetypes faced (`total` is every complete listed
/// match of the format in the window), sorted by share then name. `global`
/// is the website's `(uuid, share)` list, `None` while it is not here.
pub fn tiles(faced: Vec<Faced>, total: u32, global: Option<&[(String, Option<f64>)]>) -> Vec<Tile> {
    let mut out = Vec::new();
    for one in faced {
        let share = share_of(one.record.total, total);
        let comparable = one.source == Source::Downloaded && !one.retired;
        let mut global_share = None;
        if comparable && let Some(rows) = global {
            for (uuid, row_share) in rows {
                if *uuid == one.uuid {
                    global_share = *row_share;
                    break;
                }
            }
        }
        let difference = global_share.map(|global| php_round(share - global, 1));
        out.push(Tile {
            archetype_id: one.archetype_id,
            uuid: one.uuid,
            name: one.name,
            color_identity: one.color_identity,
            source: one.source,
            retired: one.retired,
            offline_held: one.offline_held,
            record: one.record,
            share,
            comparable,
            global_share,
            difference,
        });
    }
    out.sort_by(|a, b| {
        b.record
            .total
            .cmp(&a.record.total)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.archetype_id.cmp(&b.archetype_id))
    });
    out
}

/// How many cards a tile fans out (the API `GetMetagameIndex::CARDS`).
pub const TILE_CARDS: usize = 4;

/// One main-deck card of an archetype's stored list, with what its tile
/// needs to know about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListCard {
    pub catalog_id: i64,
    pub name: Option<String>,
    /// The type line; a land never shows.
    pub card_type: Option<String>,
    pub quantity: u32,
    /// Whether there is a picture to draw.
    pub has_image: bool,
}

/// The cards a tile shows when the website sends none (a manual or
/// fallback archetype, one outside the website's window, or offline): up
/// to `TILE_CARDS` catalog ids, the most copies first, then by name. Lands,
/// cards without a name and cards without a picture are left out. The
/// printings of one card count as one, shown by the lowest catalog id.
///
/// The website also ranks by how much a card sets the deck apart, which
/// the app does not know, so its picks can differ.
pub fn local_key_cards(cards: &[ListCard]) -> Vec<i64> {
    // (name, copies, lowest catalog id)
    let mut picked: Vec<(String, u32, i64)> = Vec::new();
    for card in cards {
        let Some(name) = card.name.as_deref().filter(|name| !name.is_empty()) else {
            continue;
        };
        let is_land = card
            .card_type
            .as_deref()
            .is_some_and(|card_type| card_type.contains("Land"));
        if is_land || !card.has_image {
            continue;
        }
        match picked.iter_mut().find(|(seen, _, _)| seen == name) {
            Some((_, copies, lowest)) => {
                *copies += card.quantity;
                *lowest = (*lowest).min(card.catalog_id);
            }
            None => picked.push((name.to_string(), card.quantity, card.catalog_id)),
        }
    }
    picked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    picked
        .into_iter()
        .take(TILE_CARDS)
        .map(|(_, _, catalog_id)| catalog_id)
        .collect()
}

/// One opponent card row of one game (`game_cards`, opponent side, `seen`).
#[derive(Debug, Clone, PartialEq)]
pub struct SeenRow {
    pub match_id: i64,
    pub game_id: i64,
    pub catalog_id: i64,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    pub seen: u32,
}

/// One card seen against an archetype.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenCard {
    /// The oracle id, or `id:{catalog_id}` when the card is not resolved.
    pub key: String,
    pub oracle_id: Option<String>,
    /// The lowest printing seen (for the image).
    pub catalog_id: i64,
    /// The card's name, "Card {id}" without one.
    pub name: String,
    /// Matches it was seen in.
    pub matches: u32,
    /// The most copies seen in one game.
    pub most_copies: u32,
    /// Whether a typical list holds it (`mark_typical`).
    pub in_typical: bool,
}

/// The grouping key of a card: its oracle id, else its catalog id.
pub fn card_key(oracle_id: Option<&str>, catalog_id: i64) -> String {
    match oracle_id {
        Some(oracle) if !oracle.is_empty() => oracle.to_string(),
        _ => format!("id:{catalog_id}"),
    }
}

/// Groups the rows by oracle id (catalog id when unresolved): printings of
/// one card in one game add up. Sorted by matches, then copies, then name.
pub fn group_seen(rows: &[SeenRow]) -> Vec<SeenCard> {
    // key -> (card, matches seen in, copies per game)
    let mut cards: BTreeMap<String, SeenCard> = BTreeMap::new();
    let mut matches: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut per_game: BTreeMap<(String, i64), u32> = BTreeMap::new();
    for row in rows {
        if row.seen == 0 {
            continue;
        }
        let key = card_key(row.oracle_id.as_deref(), row.catalog_id);
        let card = cards.entry(key.clone()).or_insert_with(|| SeenCard {
            key: key.clone(),
            oracle_id: row.oracle_id.clone().filter(|oracle| !oracle.is_empty()),
            catalog_id: row.catalog_id,
            name: String::new(),
            matches: 0,
            most_copies: 0,
            in_typical: false,
        });
        if row.catalog_id < card.catalog_id {
            card.catalog_id = row.catalog_id;
        }
        if card.name.is_empty()
            && let Some(name) = &row.name
        {
            card.name = name.clone();
        }
        let seen_in = matches.entry(key.clone()).or_default();
        if !seen_in.contains(&row.match_id) {
            seen_in.push(row.match_id);
        }
        *per_game.entry((key, row.game_id)).or_insert(0) += row.seen;
    }
    for ((key, _), copies) in per_game {
        if let Some(card) = cards.get_mut(&key)
            && copies > card.most_copies
        {
            card.most_copies = copies;
        }
    }
    let mut out = Vec::new();
    for (key, mut card) in cards {
        card.matches = matches.get(&key).map_or(0, |ids| ids.len() as u32);
        if card.name.is_empty() {
            card.name = format!("Card {}", card.catalog_id);
        }
        out.push(card);
    }
    out.sort_by(|a, b| {
        b.matches
            .cmp(&a.matches)
            .then_with(|| b.most_copies.cmp(&a.most_copies))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// One card of a typical list (a live decklist or a stored list).
#[derive(Debug, Clone, PartialEq)]
pub struct TypicalCard {
    pub catalog_id: i64,
    pub oracle_id: Option<String>,
}

/// Marks the cards a typical list holds: the same oracle id, or the same
/// catalog id. The rest go under "Not in typical lists".
pub fn mark_typical(cards: &mut [SeenCard], typical: &[TypicalCard]) {
    for card in cards.iter_mut() {
        card.in_typical = typical.iter().any(|listed| {
            let same_oracle = match (&card.oracle_id, &listed.oracle_id) {
                (Some(seen), Some(listed)) => !listed.is_empty() && seen == listed,
                _ => false,
            };
            same_oracle || listed.catalog_id == card.catalog_id
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_card(catalog_id: i64, name: &str, card_type: &str, quantity: u32) -> ListCard {
        ListCard {
            catalog_id,
            name: Some(name.to_string()),
            card_type: Some(card_type.to_string()),
            quantity,
            has_image: true,
        }
    }

    // The website's tile leads with the four-ofs; locally the most copies
    // lead, then the name. Lands and cards without a picture never show.
    #[test]
    fn local_key_cards_are_the_most_played_spells_with_pictures() {
        let mut no_image = list_card(7, "Qhidden", "Instant", 4);
        no_image.has_image = false;
        let mut nameless = list_card(8, "", "Instant", 4);
        nameless.name = None;
        let cards = vec![
            list_card(1, "Qbolt", "Instant", 4),
            list_card(2, "Qmountain", "Basic Land - Mountain", 20),
            list_card(3, "Qvial", "Artifact", 2),
            list_card(4, "Qguide", "Creature - Human Scout", 4),
            list_card(5, "Qhelix", "Instant", 3),
            list_card(6, "Qcharm", "Instant", 1),
            no_image,
            nameless,
        ];
        assert_eq!(local_key_cards(&cards), vec![1, 4, 5, 3]);
    }

    // Two printings of one card are one card: their copies add up and the
    // lowest catalog id shows.
    #[test]
    fn local_key_cards_add_up_printings() {
        let cards = vec![
            list_card(1, "Qbolt", "Instant", 3),
            list_card(9, "Qhelix", "Instant", 2),
            list_card(4, "Qhelix", "Instant", 2),
        ];
        assert_eq!(local_key_cards(&cards), vec![4, 1]);
        assert!(local_key_cards(&[]).is_empty());
    }

    fn faced(id: i64, name: &str, source: Source, wins: u32, losses: u32, draws: u32) -> Faced {
        Faced {
            archetype_id: id,
            uuid: format!("q-uuid-{id}"),
            name: name.to_string(),
            color_identity: None,
            source,
            retired: false,
            offline_held: false,
            record: Record::from_counts(wins, losses, draws),
        }
    }

    #[test]
    fn the_timeframes_read_the_websites_windows() {
        assert_eq!(Timeframe::parse("last30"), Timeframe::Last30);
        assert_eq!(Timeframe::parse("last7"), Timeframe::Last7);
        assert_eq!(Timeframe::parse("alltime"), Timeframe::AllTime);
        assert_eq!(Timeframe::parse("monthly"), Timeframe::Last30);
        assert_eq!(Timeframe::parse(""), Timeframe::Last30);
        let read: Vec<(&str, &str, u32, Option<u32>)> = Timeframe::ALL
            .iter()
            .map(|t| (t.key(), t.label(), t.global_days(), t.local_days()))
            .collect();
        assert_eq!(
            read,
            vec![
                ("last30", "Last 30 days", 30, Some(30)),
                ("last7", "Last 7 days", 7, Some(7)),
                ("alltime", "All time", 90, None),
            ]
        );
    }

    #[test]
    fn shares_have_one_decimal_rounded_half_away_from_zero() {
        assert_eq!(share_of(1, 3), 33.3);
        assert_eq!(share_of(2, 3), 66.7);
        // 1 of 8 is 12.5 exactly; 1 of 2000 is 0.05.
        assert_eq!(share_of(1, 8), 12.5);
        assert_eq!(share_of(1, 2000), 0.1);
        assert_eq!(share_of(0, 0), 0.0);
    }

    #[test]
    fn a_tile_has_the_share_the_global_share_and_the_difference() {
        let global = vec![("q-uuid-1".to_string(), Some(6.2))];
        let out = tiles(
            vec![faced(1, "Qstorm", Source::Downloaded, 2, 1, 1)],
            40,
            Some(&global),
        );
        assert_eq!(out.len(), 1);
        let tile = &out[0];
        assert_eq!(tile.share, 10.0);
        assert_eq!(tile.global_share, Some(6.2));
        assert_eq!(tile.difference, Some(3.8));
        assert!(tile.comparable);
        // Draws count in the win rate (0.x MatchRecord): 2 of 4.
        assert_eq!(tile.record.win_rate, 50);
        assert_eq!(tile.record.total, 4);
    }

    #[test]
    fn the_difference_is_signed() {
        let global = vec![("q-uuid-1".to_string(), Some(12.3))];
        let out = tiles(
            vec![faced(1, "Qstorm", Source::Downloaded, 1, 0, 0)],
            10,
            Some(&global),
        );
        assert_eq!(out[0].difference, Some(-2.3));
    }

    #[test]
    fn manual_fallback_retired_and_unlisted_tiles_have_no_global_share() {
        let mut retired = faced(4, "Qretired", Source::Downloaded, 1, 0, 0);
        retired.retired = true;
        let global = vec![
            ("q-uuid-1".to_string(), Some(5.0)),
            ("q-uuid-2".to_string(), Some(5.0)),
            ("q-uuid-3".to_string(), Some(5.0)),
            ("q-uuid-4".to_string(), Some(5.0)),
            ("q-uuid-6".to_string(), None),
        ];
        let out = tiles(
            vec![
                faced(1, "Qmanual", Source::Manual, 1, 0, 0),
                faced(2, "Homebrew", Source::Fallback, 1, 0, 0),
                faced(3, "Qlisted", Source::Downloaded, 1, 0, 0),
                retired,
                faced(5, "Qunlisted", Source::Downloaded, 1, 0, 0),
                faced(6, "Qnoshare", Source::Downloaded, 1, 0, 0),
            ],
            6,
            Some(&global),
        );
        let read: Vec<(&str, bool, Option<f64>, Option<f64>)> = out
            .iter()
            .map(|t| (t.name.as_str(), t.comparable, t.global_share, t.difference))
            .collect();
        assert_eq!(
            read,
            vec![
                ("Homebrew", false, None, None),
                ("Qlisted", true, Some(5.0), Some(11.7)),
                ("Qmanual", false, None, None),
                ("Qnoshare", true, None, None),
                ("Qretired", false, None, None),
                ("Qunlisted", true, None, None),
            ]
        );
    }

    #[test]
    fn without_the_global_shares_the_column_is_empty() {
        let out = tiles(
            vec![faced(1, "Qstorm", Source::Downloaded, 1, 0, 0)],
            1,
            None,
        );
        assert_eq!(out[0].global_share, None);
        assert_eq!(out[0].difference, None);
        assert!(out[0].comparable);
        assert_eq!(out[0].share, 100.0);
    }

    #[test]
    fn tiles_sort_by_share_then_name() {
        let out = tiles(
            vec![
                faced(1, "qburn", Source::Downloaded, 1, 0, 0),
                faced(2, "Qaffinity", Source::Downloaded, 1, 0, 0),
                faced(3, "Qstorm", Source::Downloaded, 2, 1, 0),
            ],
            5,
            None,
        );
        let names: Vec<&str> = out.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Qstorm", "Qaffinity", "qburn"]);
        assert_eq!(out[0].share, 60.0);
        assert_eq!(out[1].share, 20.0);
    }

    fn row(
        match_id: i64,
        game_id: i64,
        catalog_id: i64,
        oracle: Option<&str>,
        name: Option<&str>,
        seen: u32,
    ) -> SeenRow {
        SeenRow {
            match_id,
            game_id,
            catalog_id,
            oracle_id: oracle.map(str::to_string),
            name: name.map(str::to_string),
            seen,
        }
    }

    #[test]
    fn cards_seen_group_by_oracle_then_catalog_id() {
        let rows = vec![
            // Two printings of one card in one game add up.
            row(1, 10, 500, Some("o-bolt"), Some("Qbolt"), 1),
            row(1, 10, 400, Some("o-bolt"), Some("Qbolt"), 2),
            row(2, 20, 500, Some("o-bolt"), Some("Qbolt"), 1),
            // Unresolved: its catalog id.
            row(2, 20, 77, None, None, 1),
            row(2, 21, 77, None, None, 2),
            // Not seen: left out.
            row(2, 21, 88, Some("o-hidden"), Some("Qhidden"), 0),
        ];
        let out = group_seen(&rows);
        let read: Vec<(&str, i64, &str, u32, u32)> = out
            .iter()
            .map(|c| {
                (
                    c.key.as_str(),
                    c.catalog_id,
                    c.name.as_str(),
                    c.matches,
                    c.most_copies,
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                ("o-bolt", 400, "Qbolt", 2, 3),
                ("id:77", 77, "Card 77", 1, 2),
            ]
        );
    }

    #[test]
    fn cards_in_no_typical_list_are_set_apart() {
        let mut cards = group_seen(&[
            row(1, 10, 500, Some("o-bolt"), Some("Qbolt"), 1),
            row(1, 10, 77, None, None, 1),
            row(1, 10, 90, Some("o-tech"), Some("Qtech"), 1),
        ]);
        mark_typical(
            &mut cards,
            &[
                TypicalCard {
                    catalog_id: 999,
                    oracle_id: Some("o-bolt".to_string()),
                },
                TypicalCard {
                    catalog_id: 77,
                    oracle_id: None,
                },
            ],
        );
        let read: Vec<(&str, bool)> = cards
            .iter()
            .map(|c| (c.name.as_str(), c.in_typical))
            .collect();
        assert_eq!(
            read,
            vec![("Card 77", true), ("Qbolt", true), ("Qtech", false)]
        );
    }
}
