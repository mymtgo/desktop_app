//! Cards the opponent's archetype could still have (overlay spec 7.5, 0.x
//! `GetArchetypePotentialCards`).
//!
//! The caller passes the archetype's lists: Plan T's live decklists when
//! online, else the held key list of a downloaded archetype or every variant
//! of a manual one, each entry with its card name and type. A card any list
//! plays main is a maindeck suggestion; a card the lists only sideboard is a
//! sideboard one. Each shows the copies the lists usually play less the
//! copies revealed, and drops out at 0. Basic lands are left out.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::CardMeta;

/// One card entry of an archetype list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    /// The printing, when the list names one (images come from its row).
    pub catalog_id: Option<u32>,
    pub name: String,
    pub card_type: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// One card the opponent could still have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PotentialCard {
    pub catalog_id: Option<u32>,
    pub name: String,
    /// The card type, or "Unknown" without one.
    pub card_type: String,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    /// The printing's files in v1's card images folder.
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
    /// The copies the lists usually play, less the copies revealed.
    pub quantity: u32,
}

/// "Could still have" (`maindeck`) and "Potential sideboard cards"
/// (`sideboard`), most lists first, then by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PotentialCards {
    pub maindeck: Vec<PotentialCard>,
    pub sideboard: Vec<PotentialCard>,
}

/// The potential cards of an archetype's lists, given the copies revealed so
/// far by card name (matched without case).
pub fn potential_cards(
    lists: &[Vec<ListEntry>],
    revealed: &BTreeMap<String, u32>,
    cards: &CardMeta,
) -> PotentialCards {
    let mut revealed_by_name: BTreeMap<String, u32> = BTreeMap::new();
    for (name, quantity) in revealed {
        revealed_by_name.insert(name.to_lowercase(), *quantity);
    }

    // Every entry with its list's index, basic lands out.
    let mut rows: Vec<(usize, &ListEntry)> = Vec::new();
    for (list, entries) in lists.iter().enumerate() {
        for entry in entries {
            let basic = entry
                .card_type
                .as_deref()
                .is_some_and(|card_type| card_type.contains("Basic"));
            if !basic {
                rows.push((list, entry));
            }
        }
    }

    let main_names: Vec<&str> = rows
        .iter()
        .filter(|(_, entry)| !entry.sideboard)
        .map(|(_, entry)| entry.name.as_str())
        .collect();

    let main_rows: Vec<(usize, &ListEntry)> = rows
        .iter()
        .filter(|(_, entry)| !entry.sideboard)
        .copied()
        .collect();
    let side_rows: Vec<(usize, &ListEntry)> = rows
        .iter()
        .filter(|(_, entry)| entry.sideboard && !main_names.contains(&entry.name.as_str()))
        .copied()
        .collect();

    PotentialCards {
        maindeck: rank(&main_rows, &revealed_by_name, cards),
        sideboard: rank(&side_rows, &revealed_by_name, cards),
    }
}

/// One card name's entries across the lists.
struct Group<'a> {
    first: &'a ListEntry,
    lists: Vec<usize>,
    quantities: Vec<u32>,
}

/// Rows grouped by name (first-seen order), less the revealed copies,
/// dropped at 0, most lists first, then by name without case.
fn rank(
    rows: &[(usize, &ListEntry)],
    revealed: &BTreeMap<String, u32>,
    cards: &CardMeta,
) -> Vec<PotentialCard> {
    let mut groups: Vec<Group> = Vec::new();
    for (list, entry) in rows {
        let position = groups
            .iter()
            .position(|group| group.first.name == entry.name);
        let group = match position {
            Some(index) => &mut groups[index],
            None => {
                groups.push(Group {
                    first: entry,
                    lists: Vec::new(),
                    quantities: Vec::new(),
                });
                groups.last_mut().expect("a group was just pushed")
            }
        };
        if !group.lists.contains(list) {
            group.lists.push(*list);
        }
        group.quantities.push(entry.quantity);
    }

    let mut ranked: Vec<(usize, PotentialCard)> = Vec::new();
    for group in &groups {
        let typical = typical_quantity(&group.quantities);
        let seen = revealed
            .get(&group.first.name.to_lowercase())
            .copied()
            .unwrap_or(0);
        if typical <= seen {
            continue;
        }
        let entry = group.first;
        let info = entry.catalog_id.and_then(|id| cards.get(id));
        ranked.push((
            group.lists.len(),
            PotentialCard {
                catalog_id: entry.catalog_id,
                name: entry.name.clone(),
                card_type: entry
                    .card_type
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string()),
                image: info.and_then(|info| info.image.clone()),
                art_crop: info.and_then(|info| info.art_crop.clone()),
                local_image: info.and_then(|info| info.local_image.clone()),
                local_art_crop: info.and_then(|info| info.local_art_crop.clone()),
                quantity: typical - seen,
            },
        ));
    }

    // Stable, as 0.x: equal entries keep their first-seen order.
    ranked.sort_by(|(a_lists, a), (b_lists, b)| {
        b_lists.cmp(a_lists).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    ranked.into_iter().map(|(_, card)| card).collect()
}

/// The copy count most lists play; an even split takes the higher count,
/// since overestimating a threat costs the player less than missing one.
fn typical_quantity(quantities: &[u32]) -> u32 {
    let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
    for quantity in quantities {
        *counts.entry(*quantity).or_insert(0) += 1;
    }
    let mut best = 0;
    let mut best_count = 0;
    for (quantity, count) in counts {
        // Ascending quantities, so `>=` lets a tie go to the higher count.
        if count >= best_count {
            best = quantity;
            best_count = count;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CardInfo;

    fn entry(name: &str, card_type: &str, sideboard: bool, quantity: u32) -> ListEntry {
        ListEntry {
            catalog_id: None,
            name: name.to_string(),
            card_type: Some(card_type.to_string()),
            quantity,
            sideboard,
        }
    }

    fn bolt(sideboard: bool) -> ListEntry {
        entry("Lightning Bolt", "Instant", sideboard, 2)
    }

    fn murktide(sideboard: bool) -> ListEntry {
        entry("Murktide Regent", "Creature", sideboard, 2)
    }

    fn heat(sideboard: bool) -> ListEntry {
        entry("Unholy Heat", "Instant", sideboard, 2)
    }

    fn moon(sideboard: bool) -> ListEntry {
        entry("Blood Moon", "Enchantment", sideboard, 2)
    }

    fn mountain() -> ListEntry {
        entry("Mountain", "Basic Land", false, 2)
    }

    fn revealed(rows: &[(&str, u32)]) -> BTreeMap<String, u32> {
        rows.iter()
            .map(|(name, quantity)| (name.to_string(), *quantity))
            .collect()
    }

    fn names(cards: &[PotentialCard]) -> Vec<String> {
        cards.iter().map(|card| card.name.clone()).collect()
    }

    #[test]
    fn lists_what_the_archetype_plays_most_common_first_minus_what_has_been_revealed() {
        let lists = vec![
            vec![bolt(false), murktide(false), heat(false)],
            vec![bolt(false), murktide(false)],
            vec![murktide(false)],
        ];

        let result = potential_cards(
            &lists,
            &revealed(&[("Lightning Bolt", 2)]),
            &CardMeta::default(),
        );

        assert_eq!(names(&result.maindeck), ["Murktide Regent", "Unholy Heat"]);
    }

    #[test]
    fn puts_cards_that_are_only_ever_sideboarded_in_their_own_group() {
        let lists = vec![vec![bolt(false), moon(true), heat(true)], vec![heat(false)]];

        let result = potential_cards(&lists, &BTreeMap::new(), &CardMeta::default());

        let mut main = names(&result.maindeck);
        main.sort();
        assert_eq!(main, ["Lightning Bolt", "Unholy Heat"]);
        assert_eq!(names(&result.sideboard), ["Blood Moon"]);
    }

    #[test]
    fn drops_revealed_cards_from_the_sideboard_group_too() {
        let lists = vec![vec![bolt(false), moon(true)]];

        let result = potential_cards(
            &lists,
            &revealed(&[("Blood Moon", 2)]),
            &CardMeta::default(),
        );

        assert!(result.sideboard.is_empty());
    }

    #[test]
    fn leaves_basic_lands_out() {
        let lists = vec![vec![bolt(false), mountain()]];

        let result = potential_cards(&lists, &BTreeMap::new(), &CardMeta::default());

        assert_eq!(names(&result.maindeck), ["Lightning Bolt"]);
    }

    #[test]
    fn returns_empty_groups_for_an_archetype_with_no_downloaded_lists() {
        let result = potential_cards(&[], &BTreeMap::new(), &CardMeta::default());

        assert!(result.maindeck.is_empty());
        assert!(result.sideboard.is_empty());
    }

    #[test]
    fn shows_the_copies_the_lists_usually_play_less_the_ones_already_revealed() {
        let lists = vec![
            vec![
                entry("Lightning Bolt", "Instant", false, 4),
                entry("Murktide Regent", "Creature", false, 2),
            ],
            vec![
                entry("Lightning Bolt", "Instant", false, 4),
                entry("Murktide Regent", "Creature", false, 3),
            ],
            vec![
                entry("Lightning Bolt", "Instant", false, 3),
                entry("Murktide Regent", "Creature", false, 3),
            ],
        ];

        let result = potential_cards(
            &lists,
            &revealed(&[("Lightning Bolt", 1)]),
            &CardMeta::default(),
        );

        // Bolt: usually 4, one seen. Murktide: usually 3.
        let counts: Vec<(String, u32)> = result
            .maindeck
            .iter()
            .map(|card| (card.name.clone(), card.quantity))
            .collect();
        assert_eq!(
            counts,
            [
                ("Lightning Bolt".to_string(), 3),
                ("Murktide Regent".to_string(), 3)
            ]
        );
    }

    #[test]
    fn takes_the_higher_count_when_lists_are_split_evenly() {
        let lists = vec![
            vec![entry("Unholy Heat", "Instant", false, 2)],
            vec![entry("Unholy Heat", "Instant", false, 3)],
        ];

        let result = potential_cards(&lists, &BTreeMap::new(), &CardMeta::default());

        assert_eq!(result.maindeck[0].quantity, 3);
    }

    #[test]
    fn keeps_a_sideboard_card_whose_lists_play_more_copies_than_have_been_revealed() {
        let lists = vec![vec![
            bolt(false),
            entry("Blood Moon", "Enchantment", true, 3),
        ]];

        let result = potential_cards(
            &lists,
            &revealed(&[("Blood Moon", 1)]),
            &CardMeta::default(),
        );

        assert_eq!(result.sideboard[0].quantity, 2);
    }

    #[test]
    fn matches_revealed_names_without_case_and_ties_sort_by_name_without_case() {
        let lists = vec![vec![
            entry("bolt of Doom", "Instant", false, 2),
            entry("Abrade", "Instant", false, 2),
            entry("Counterspell", "Instant", false, 2),
        ]];

        let result = potential_cards(
            &lists,
            &revealed(&[("COUNTERSPELL", 2)]),
            &CardMeta::default(),
        );

        assert_eq!(names(&result.maindeck), ["Abrade", "bolt of Doom"]);
    }

    /// Spec 7.1: the downloaded files beside the remote URLs.
    #[test]
    fn carries_the_local_files_of_the_named_printing() {
        let mut cards = CardMeta::default();
        cards.cards.insert(
            501,
            CardInfo {
                local_image: Some("501.jpg".to_string()),
                local_art_crop: Some("501_art.jpg".to_string()),
                ..CardInfo::default()
            },
        );
        let mut with_id = bolt(false);
        with_id.catalog_id = Some(501);

        let result = potential_cards(&[vec![with_id]], &BTreeMap::new(), &cards);

        assert_eq!(result.maindeck[0].local_image.as_deref(), Some("501.jpg"));
        assert_eq!(
            result.maindeck[0].local_art_crop.as_deref(),
            Some("501_art.jpg")
        );
    }

    #[test]
    fn carries_the_type_and_images_of_the_named_printing() {
        let mut cards = CardMeta::default();
        cards.cards.insert(
            501,
            CardInfo {
                image: Some("https://img/bolt.jpg".to_string()),
                art_crop: Some("https://img/bolt-art.jpg".to_string()),
                ..CardInfo::default()
            },
        );
        let mut with_id = bolt(false);
        with_id.catalog_id = Some(501);
        let mut no_type = heat(false);
        no_type.card_type = None;

        let result = potential_cards(&[vec![with_id, no_type]], &BTreeMap::new(), &cards);

        assert_eq!(result.maindeck[0].catalog_id, Some(501));
        assert_eq!(result.maindeck[0].card_type, "Instant");
        assert_eq!(
            result.maindeck[0].image.as_deref(),
            Some("https://img/bolt.jpg")
        );
        assert_eq!(
            result.maindeck[0].art_crop.as_deref(),
            Some("https://img/bolt-art.jpg")
        );
        assert_eq!(result.maindeck[1].card_type, "Unknown");
        assert_eq!(result.maindeck[1].image, None);
    }

    #[test]
    fn a_card_twice_in_one_list_counts_that_list_once() {
        // Two printings of Heat in one list, Bolt in two lists: Bolt leads.
        let lists = vec![
            vec![heat(false), heat(false), bolt(false)],
            vec![bolt(false)],
        ];

        let result = potential_cards(&lists, &BTreeMap::new(), &CardMeta::default());

        assert_eq!(names(&result.maindeck), ["Lightning Bolt", "Unholy Heat"]);
    }
}
