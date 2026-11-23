//! The game overlay's sideboard guide (overlay spec 7.6, 0.x
//! `BuildSideboardGuide` in Plan and History scope, without community
//! rates: spec ruling 6).
//!
//! Cards are listed from the latest version's zones; the numbers beside them
//! count every version of the deck against the archetype. Everything is
//! keyed by oracle id. The zones, plan cards and per-card numbers are the
//! `archetypes::guides` types the store already reads.
//!
//! - **Plan** scope when the plan lists at least one card: only the planned
//!   cards, with planned quantities, plus stale rows for planned cards their
//!   zone no longer holds; each list sorted by name.
//! - **History** scope otherwise: every sideboard card ("Bring in"), by
//!   sided-in games then name, and every maindeck card cut at least once
//!   ("Usually cut"), by sided-out games then name.
//!
//! The "In" and "Out" badges (`recommended`) come from the player's own
//! games: the share of postboard games the card was sided in (or out) must
//! reach `RECOMMEND_SHARE` percent over at least `LOCAL_MIN_GAMES` postboard
//! games.

use std::collections::BTreeMap;

use archetypes::guides::{Direction, GuideCard, SidedStat, Zones};
use serde::Serialize;

use crate::CardInfo;

/// The fewest postboard games the player's own history needs before it
/// earns a badge (0.x `LOCAL_MIN_GAMES`).
pub const LOCAL_MIN_GAMES: u32 = 3;

/// The share of postboard games, in percent, a card must be sided in (or
/// out) for a badge (0.x `RECOMMEND_SHARE`).
pub const RECOMMEND_SHARE: u32 = 50;

/// A count and the sample it was drawn from (0.x `SampleRate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleRate {
    pub count: u32,
    pub games: u32,
}

impl SampleRate {
    /// The count as a whole percentage of its sample, `None` without a
    /// sample (0.x `SampleRate::percentage`, divide first).
    pub fn percentage(self) -> Option<u32> {
        decks::stats::sample_rate_percent(self.count, self.games)
    }

    /// Whether the sample has at least `minimum_games` games (and any).
    pub fn is_confident(self, minimum_games: u32) -> bool {
        self.games > 0 && self.games >= minimum_games
    }

    /// Whether the rate reaches `minimum_share` percent on a confident sample.
    pub fn supports(self, minimum_share: u32, minimum_games: u32) -> bool {
        self.is_confident(minimum_games) && self.percentage().unwrap_or(0) >= minimum_share
    }
}

/// The numbers against the archetype over every version of the deck.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuideStats {
    /// Per oracle id; a card without a row has no history.
    pub cards: BTreeMap<String, SidedStat>,
    pub postboard_games: u32,
    pub postboard_wins: u32,
}

/// One "Bring in" row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SidedInCard {
    pub oracle_id: String,
    /// The card name, or "Unknown card" without a card row.
    pub name: String,
    pub card_type: Option<String>,
    pub color_identity: Option<String>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    /// The card's files in v1's card images folder.
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
    /// Plan scope: the planned copies; History: the sideboard's copies.
    pub quantity: u32,
    pub sided_in_games: u32,
    pub wins: u32,
    pub losses: u32,
    /// Win percentage of the sided-in games, `None` without any.
    pub winrate: Option<u32>,
    /// The "In" badge.
    pub recommended: bool,
    pub planned_quantity: Option<u32>,
    /// Planned but no longer in the sideboard ("(not in sideboard)").
    pub stale: bool,
}

/// One "Usually cut" (or "Take out") row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SidedOutCard {
    pub oracle_id: String,
    /// The card name, or "Unknown card" without a card row.
    pub name: String,
    pub card_type: Option<String>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    /// The card's files in v1's card images folder.
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
    /// Plan scope: the planned copies; History: the maindeck's copies.
    pub quantity: u32,
    pub sided_out_games: u32,
    /// The "Out" badge.
    pub recommended: bool,
    pub planned_quantity: Option<u32>,
    /// Planned but no longer in the maindeck ("(not in maindeck)").
    pub stale: bool,
}

/// The sideboard guide of one deck against one archetype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SideboardGuide {
    pub sided_in: Vec<SidedInCard>,
    pub sided_out: Vec<SidedOutCard>,
    pub postboard_games: u32,
    /// "W - L".
    pub postboard_record: String,
    /// The plan listed at least one card (Plan scope).
    pub has_plan: bool,
}

/// The guide's rows: Plan scope when `plan` lists at least one card, else
/// History. `card_meta` holds a card row per oracle id (the version's own
/// printing first).
pub fn sideboard_rows(
    zones: &Zones,
    plan: Option<&[GuideCard]>,
    stats: &GuideStats,
    card_meta: &BTreeMap<String, CardInfo>,
) -> SideboardGuide {
    let plan = plan.unwrap_or(&[]);
    let mut planned_in: BTreeMap<String, u32> = BTreeMap::new();
    let mut planned_out: BTreeMap<String, u32> = BTreeMap::new();
    for card in plan {
        match card.direction {
            Direction::In => planned_in.insert(card.oracle_id.clone(), card.quantity),
            Direction::Out => planned_out.insert(card.oracle_id.clone(), card.quantity),
        };
    }
    let has_plan = !plan.is_empty();

    let (sided_in, sided_out) = if has_plan {
        plan_rows(zones, &planned_in, &planned_out, stats, card_meta)
    } else {
        history_rows(zones, stats, card_meta)
    };

    let losses = stats.postboard_games.saturating_sub(stats.postboard_wins);
    SideboardGuide {
        sided_in,
        sided_out,
        postboard_games: stats.postboard_games,
        postboard_record: format!("{} - {losses}", stats.postboard_wins),
        has_plan,
    }
}

/// Plan scope: the planned cards of each zone, then the planned cards the
/// zone no longer holds (stale), with planned quantities, sorted by name.
fn plan_rows(
    zones: &Zones,
    planned_in: &BTreeMap<String, u32>,
    planned_out: &BTreeMap<String, u32>,
    stats: &GuideStats,
    card_meta: &BTreeMap<String, CardInfo>,
) -> (Vec<SidedInCard>, Vec<SidedOutCard>) {
    let mut sided_in = Vec::new();
    for (oracle_id, quantity) in planned_in {
        let stale = !zones.sideboard.contains_key(oracle_id);
        sided_in.push(sided_in_card(
            oracle_id,
            *quantity,
            Some(*quantity),
            stale,
            stats,
            card_meta,
        ));
    }
    let mut sided_out = Vec::new();
    for (oracle_id, quantity) in planned_out {
        let stale = !zones.maindeck.contains_key(oracle_id);
        sided_out.push(sided_out_card(
            oracle_id,
            *quantity,
            Some(*quantity),
            stale,
            stats,
            card_meta,
        ));
    }
    // Present rows before stale ones on a name tie, as 0.x's union.
    sided_in
        .sort_by(|a, b| (&a.name, a.stale, &a.oracle_id).cmp(&(&b.name, b.stale, &b.oracle_id)));
    sided_out
        .sort_by(|a, b| (&a.name, a.stale, &a.oracle_id).cmp(&(&b.name, b.stale, &b.oracle_id)));
    (sided_in, sided_out)
}

/// History scope: every sideboard card by sided-in games then name; every
/// maindeck card not also in the sideboard and cut at least once, by
/// sided-out games then name.
fn history_rows(
    zones: &Zones,
    stats: &GuideStats,
    card_meta: &BTreeMap<String, CardInfo>,
) -> (Vec<SidedInCard>, Vec<SidedOutCard>) {
    let mut sided_in = Vec::new();
    for (oracle_id, quantity) in &zones.sideboard {
        sided_in.push(sided_in_card(
            oracle_id, *quantity, None, false, stats, card_meta,
        ));
    }
    let mut sided_out = Vec::new();
    for (oracle_id, quantity) in &zones.maindeck {
        // A card split across zones is listed once, as a sideboard card.
        if zones.sideboard.contains_key(oracle_id) {
            continue;
        }
        let row = sided_out_card(oracle_id, *quantity, None, false, stats, card_meta);
        // A card never cut is not a cut suggestion.
        if row.sided_out_games > 0 {
            sided_out.push(row);
        }
    }
    sided_in.sort_by(|a, b| {
        b.sided_in_games
            .cmp(&a.sided_in_games)
            .then_with(|| (&a.name, &a.oracle_id).cmp(&(&b.name, &b.oracle_id)))
    });
    sided_out.sort_by(|a, b| {
        b.sided_out_games
            .cmp(&a.sided_out_games)
            .then_with(|| (&a.name, &a.oracle_id).cmp(&(&b.name, &b.oracle_id)))
    });
    (sided_in, sided_out)
}

/// The badge from the player's own games: the card's share of the
/// postboard games.
fn recommended(games: u32, postboard_games: u32) -> bool {
    SampleRate {
        count: games,
        games: postboard_games,
    }
    .supports(RECOMMEND_SHARE, LOCAL_MIN_GAMES)
}

/// The card's name, else "Unknown card".
fn card_name(info: Option<&CardInfo>) -> String {
    info.and_then(|info| info.name.clone())
        .unwrap_or_else(|| "Unknown card".to_string())
}

fn sided_in_card(
    oracle_id: &str,
    quantity: u32,
    planned_quantity: Option<u32>,
    stale: bool,
    stats: &GuideStats,
    card_meta: &BTreeMap<String, CardInfo>,
) -> SidedInCard {
    let history = stats.cards.get(oracle_id).copied().unwrap_or_default();
    let info = card_meta.get(oracle_id);
    // 0.x `BuildSideboardGuide`: `Winrate::percentage($wins, $losses)`.
    let winrate = if history.sided_in_games > 0 {
        Some(decks::stats::winrate_percent(
            history.sided_in_won,
            history.sided_in_lost,
            0,
        ))
    } else {
        None
    };
    SidedInCard {
        oracle_id: oracle_id.to_string(),
        name: card_name(info),
        card_type: info.and_then(|info| info.card_type.clone()),
        color_identity: info.and_then(|info| info.color_identity.clone()),
        image: info.and_then(|info| info.image.clone()),
        art_crop: info.and_then(|info| info.art_crop.clone()),
        local_image: info.and_then(|info| info.local_image.clone()),
        local_art_crop: info.and_then(|info| info.local_art_crop.clone()),
        quantity,
        sided_in_games: history.sided_in_games,
        wins: history.sided_in_won,
        losses: history.sided_in_lost,
        winrate,
        recommended: recommended(history.sided_in_games, stats.postboard_games),
        planned_quantity,
        stale,
    }
}

fn sided_out_card(
    oracle_id: &str,
    quantity: u32,
    planned_quantity: Option<u32>,
    stale: bool,
    stats: &GuideStats,
    card_meta: &BTreeMap<String, CardInfo>,
) -> SidedOutCard {
    let history = stats.cards.get(oracle_id).copied().unwrap_or_default();
    let info = card_meta.get(oracle_id);
    SidedOutCard {
        oracle_id: oracle_id.to_string(),
        name: card_name(info),
        card_type: info.and_then(|info| info.card_type.clone()),
        image: info.and_then(|info| info.image.clone()),
        art_crop: info.and_then(|info| info.art_crop.clone()),
        local_image: info.and_then(|info| info.local_image.clone()),
        local_art_crop: info.and_then(|info| info.local_art_crop.clone()),
        quantity,
        sided_out_games: history.sided_out_games,
        recommended: recommended(history.sided_out_games, stats.postboard_games),
        planned_quantity,
        stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The 0.x fixture cards: Rest in Peace (sideboard), Lightning Bolt
    // (maindeck), Cut Down (sideboard).
    fn meta() -> BTreeMap<String, CardInfo> {
        let mut cards = BTreeMap::new();
        for (oracle, name, card_type, colors) in [
            ("o-rip", "Rest in Peace", "Enchantment", "W"),
            ("o-bolt", "Lightning Bolt", "Instant", "R"),
            ("o-cut", "Cut Down", "Instant", "B"),
        ] {
            cards.insert(
                oracle.to_string(),
                CardInfo {
                    name: Some(name.to_string()),
                    card_type: Some(card_type.to_string()),
                    color_identity: Some(colors.to_string()),
                    ..CardInfo::default()
                },
            );
        }
        cards
    }

    fn zones(sideboard: &[(&str, u32)], maindeck: &[(&str, u32)]) -> Zones {
        Zones {
            sideboard: sideboard
                .iter()
                .map(|(oracle, qty)| (oracle.to_string(), *qty))
                .collect(),
            maindeck: maindeck
                .iter()
                .map(|(oracle, qty)| (oracle.to_string(), *qty))
                .collect(),
        }
    }

    fn history(sided_in_won: u32, sided_in_lost: u32, sided_out_games: u32) -> SidedStat {
        SidedStat {
            sided_in_games: sided_in_won + sided_in_lost,
            sided_in_won,
            sided_in_lost,
            sided_out_games,
        }
    }

    fn stats(rows: &[(&str, SidedStat)], games: u32, wins: u32) -> GuideStats {
        GuideStats {
            cards: rows
                .iter()
                .map(|(oracle, row)| (oracle.to_string(), *row))
                .collect(),
            postboard_games: games,
            postboard_wins: wins,
        }
    }

    fn planned(oracle: &str, direction: Direction, quantity: u32) -> GuideCard {
        GuideCard {
            oracle_id: oracle.to_string(),
            direction,
            quantity,
        }
    }

    fn sided_in<'a>(guide: &'a SideboardGuide, oracle: &str) -> Option<&'a SidedInCard> {
        guide.sided_in.iter().find(|card| card.oracle_id == oracle)
    }

    fn sided_out<'a>(guide: &'a SideboardGuide, oracle: &str) -> Option<&'a SidedOutCard> {
        guide.sided_out.iter().find(|card| card.oracle_id == oracle)
    }

    fn in_ids(guide: &SideboardGuide) -> Vec<&str> {
        guide
            .sided_in
            .iter()
            .map(|c| c.oracle_id.as_str())
            .collect()
    }

    fn out_ids(guide: &SideboardGuide) -> Vec<&str> {
        guide
            .sided_out
            .iter()
            .map(|c| c.oracle_id.as_str())
            .collect()
    }

    #[test]
    fn reports_sided_in_w_l_and_the_postboard_baseline_against_the_archetype() {
        let zones = zones(&[("o-rip", 2), ("o-cut", 2)], &[("o-bolt", 4)]);
        // Two postboard games, one won: Rest in Peace in both, Bolt out of both.
        let stats = stats(
            &[("o-rip", history(1, 1, 0)), ("o-bolt", history(0, 0, 2))],
            2,
            1,
        );

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert_eq!(guide.postboard_games, 2);
        assert_eq!(guide.postboard_record, "1 - 1");

        let rip = sided_in(&guide, "o-rip").unwrap();
        assert_eq!(rip.sided_in_games, 2);
        assert_eq!(rip.wins, 1);
        assert_eq!(rip.losses, 1);
        assert_eq!(rip.winrate, Some(50));

        // Sideboard cards with no sided-in games still appear, with no record.
        let cut = sided_in(&guide, "o-cut").unwrap();
        assert_eq!(cut.sided_in_games, 0);
        assert_eq!(cut.winrate, None);

        // Most-used first.
        assert_eq!(guide.sided_in[0].oracle_id, "o-rip");

        assert_eq!(out_ids(&guide), ["o-bolt"]);
        assert_eq!(guide.sided_out[0].sided_out_games, 2);
    }

    #[test]
    fn lists_cards_from_the_current_version_while_counting_games_from_every_version() {
        // Cut Down is gone from the current list; an older version's game
        // still counts toward Rest in Peace.
        let zones = zones(&[("o-rip", 2)], &[("o-bolt", 4)]);
        let stats = stats(
            &[("o-rip", history(1, 0, 0)), ("o-cut", history(1, 0, 0))],
            1,
            1,
        );

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert_eq!(in_ids(&guide), ["o-rip"]);
        assert_eq!(sided_in(&guide, "o-rip").unwrap().sided_in_games, 1);
    }

    #[test]
    fn names_every_sideboard_card_with_no_postboard_history_at_all() {
        let zones = zones(&[("o-rip", 2), ("o-cut", 2)], &[("o-bolt", 4)]);

        let guide = sideboard_rows(&zones, None, &GuideStats::default(), &meta());

        assert_eq!(guide.postboard_games, 0);
        let names: Vec<&str> = guide.sided_in.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Cut Down", "Rest in Peace"]);

        let rip = sided_in(&guide, "o-rip").unwrap();
        assert_eq!(rip.color_identity.as_deref(), Some("W"));
        assert_eq!(rip.sided_in_games, 0);
        assert_eq!(rip.winrate, None);
    }

    #[test]
    fn lists_a_card_split_between_the_maindeck_and_the_sideboard_once() {
        // Rest in Peace: 3 in the maindeck, 1 in the sideboard.
        let zones = zones(&[("o-rip", 1)], &[("o-rip", 3), ("o-bolt", 4)]);
        let stats = stats(&[("o-rip", history(1, 0, 0))], 1, 1);

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        let rips: Vec<&SidedInCard> = guide
            .sided_in
            .iter()
            .filter(|c| c.oracle_id == "o-rip")
            .collect();
        assert_eq!(rips.len(), 1);
        // The sideboard copies available to bring in, not the maindeck ones.
        assert_eq!(rips[0].quantity, 1);
        assert_eq!(rips[0].name, "Rest in Peace");
        assert!(sided_out(&guide, "o-rip").is_none());
    }

    #[test]
    fn keeps_sideboard_cards_out_of_the_sided_out_list() {
        let zones = zones(&[("o-rip", 2)], &[("o-bolt", 4)]);
        // A split card: flagged sideboard, but its quantity dropped in a game.
        let stats = stats(&[("o-rip", history(0, 0, 1))], 1, 1);

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert!(sided_out(&guide, "o-rip").is_none());
    }

    /// Spec 7.1: the downloaded files beside the remote URLs.
    #[test]
    fn carries_the_local_files_on_sided_in_and_sided_out_cards() {
        let mut cards = meta();
        cards.get_mut("o-rip").unwrap().local_image = Some("rip.jpg".to_string());
        cards.get_mut("o-bolt").unwrap().local_art_crop = Some("bolt_art.jpg".to_string());
        let zones = zones(&[("o-rip", 2)], &[("o-bolt", 4)]);
        let stats = stats(&[("o-bolt", history(0, 0, 1))], 1, 0);

        let guide = sideboard_rows(&zones, None, &stats, &cards);

        assert_eq!(
            sided_in(&guide, "o-rip").unwrap().local_image.as_deref(),
            Some("rip.jpg")
        );
        assert_eq!(
            sided_out(&guide, "o-bolt")
                .unwrap()
                .local_art_crop
                .as_deref(),
            Some("bolt_art.jpg")
        );
    }

    #[test]
    fn includes_the_art_crop_on_sided_in_and_sided_out_cards() {
        let mut cards = meta();
        cards.get_mut("o-rip").unwrap().art_crop = Some("https://img/rip-art.jpg".to_string());
        cards.get_mut("o-bolt").unwrap().art_crop = Some("https://img/bolt-art.jpg".to_string());
        let zones = zones(&[("o-rip", 2)], &[("o-bolt", 4)]);
        // Bolt is listed once it has been cut (no community rates).
        let stats = stats(&[("o-bolt", history(0, 0, 1))], 1, 0);

        let guide = sideboard_rows(&zones, None, &stats, &cards);

        assert_eq!(
            sided_in(&guide, "o-rip").unwrap().art_crop.as_deref(),
            Some("https://img/rip-art.jpg")
        );
        assert_eq!(
            sided_out(&guide, "o-bolt").unwrap().art_crop.as_deref(),
            Some("https://img/bolt-art.jpg")
        );
    }

    #[test]
    fn lists_only_the_planned_cards_in_plan_scope_with_planned_quantities_and_stats() {
        let zones = zones(&[("o-rip", 2), ("o-cut", 2)], &[("o-bolt", 4)]);
        let stats = stats(&[("o-rip", history(1, 0, 0))], 1, 1);
        let plan = [
            planned("o-rip", Direction::In, 1),
            planned("o-bolt", Direction::Out, 1),
        ];

        let guide = sideboard_rows(&zones, Some(&plan), &stats, &meta());

        assert!(guide.has_plan);

        // Cut Down is in the sideboard but not in the plan.
        assert_eq!(in_ids(&guide), ["o-rip"]);
        assert_eq!(guide.sided_in[0].quantity, 1);
        assert_eq!(guide.sided_in[0].planned_quantity, Some(1));
        assert_eq!(guide.sided_in[0].sided_in_games, 1);
        assert_eq!(guide.sided_in[0].wins, 1);

        // Never cut in history, but the plan cuts it.
        assert_eq!(out_ids(&guide), ["o-bolt"]);
        assert_eq!(guide.sided_out[0].quantity, 1);
        assert_eq!(guide.sided_out[0].planned_quantity, Some(1));
        assert_eq!(guide.sided_out[0].sided_out_games, 0);
    }

    #[test]
    fn flags_planned_cards_missing_from_the_current_version_as_stale() {
        // Rest in Peace is no longer in the deck.
        let zones = zones(&[("o-cut", 2)], &[("o-bolt", 4)]);
        let plan = [planned("o-rip", Direction::In, 2)];

        let guide = sideboard_rows(&zones, Some(&plan), &GuideStats::default(), &meta());

        assert_eq!(guide.sided_in.len(), 1);
        assert_eq!(guide.sided_in[0].oracle_id, "o-rip");
        assert_eq!(guide.sided_in[0].name, "Rest in Peace");
        assert!(guide.sided_in[0].stale);
        assert_eq!(guide.sided_in[0].planned_quantity, Some(2));
    }

    #[test]
    fn reports_has_plan_false_in_history_scope_and_for_an_empty_plan() {
        let zones = zones(&[("o-rip", 2)], &[("o-bolt", 4)]);

        let history = sideboard_rows(&zones, None, &GuideStats::default(), &meta());
        let empty = sideboard_rows(&zones, Some(&[]), &GuideStats::default(), &meta());

        assert!(!history.has_plan);
        assert!(!empty.has_plan);
        // An empty plan reads as History.
        assert_eq!(empty, history);
    }

    #[test]
    fn marks_a_planned_in_card_that_moved_to_the_maindeck_as_stale_while_still_listing_it_as_a_maindeck_row()
     {
        // Rest in Peace used to be in the sideboard; it is now maindeck only.
        // 0.x's case is Editor scope; in Plan scope the maindeck row shows
        // when the plan also cuts it.
        let zones = zones(&[("o-cut", 2)], &[("o-rip", 2), ("o-bolt", 4)]);
        let plan = [
            planned("o-rip", Direction::In, 2),
            planned("o-rip", Direction::Out, 1),
        ];

        let guide = sideboard_rows(&zones, Some(&plan), &GuideStats::default(), &meta());

        let stale_in = sided_in(&guide, "o-rip").unwrap();
        assert!(stale_in.stale);
        assert_eq!(stale_in.planned_quantity, Some(2));

        let main_rip = sided_out(&guide, "o-rip").unwrap();
        assert!(!main_rip.stale);
        assert_eq!(main_rip.quantity, 1);
        assert_eq!(main_rip.planned_quantity, Some(1));
    }

    #[test]
    fn falls_through_to_your_own_history_when_the_field_sample_is_too_thin() {
        // Three postboard games, Rest in Peace sided in for two: 67% of 3.
        let zones = zones(&[("o-rip", 2)], &[]);
        let stats = stats(&[("o-rip", history(1, 1, 0))], 3, 2);

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert!(guide.sided_in[0].recommended);
    }

    #[test]
    fn withholds_the_recommendation_when_neither_sample_is_big_enough() {
        // Two postboard games, sided in for both: 100% of too few.
        let zones = zones(&[("o-rip", 2)], &[]);
        let stats = stats(&[("o-rip", history(1, 1, 0))], 2, 1);

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert!(!guide.sided_in[0].recommended);
    }

    #[test]
    fn recommends_a_cut_from_your_own_history() {
        let zones = zones(&[], &[("o-bolt", 4), ("o-cut", 1)]);
        // Bolt cut in 2 of 4 (50%), Cut Down in 1 of 4 (25%).
        let stats = stats(
            &[("o-bolt", history(0, 0, 2)), ("o-cut", history(0, 0, 1))],
            4,
            2,
        );

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert!(sided_out(&guide, "o-bolt").unwrap().recommended);
        assert!(!sided_out(&guide, "o-cut").unwrap().recommended);
    }

    #[test]
    fn history_drops_a_card_never_cut() {
        let zones = zones(&[("o-rip", 2)], &[("o-bolt", 4), ("o-cut", 2)]);
        let stats = stats(&[("o-cut", history(0, 0, 1))], 1, 0);

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        assert_eq!(out_ids(&guide), ["o-cut"]);
    }

    #[test]
    fn history_orders_by_sided_games_then_name() {
        let mut cards = meta();
        cards.insert(
            "o-abr".to_string(),
            CardInfo {
                name: Some("Abrade".to_string()),
                ..CardInfo::default()
            },
        );
        let zones = zones(
            &[("o-rip", 1), ("o-cut", 1), ("o-abr", 1)],
            &[("o-bolt", 4), ("o-x", 2)],
        );
        let stats = stats(
            &[
                ("o-rip", history(2, 1, 0)),
                ("o-cut", history(1, 0, 0)),
                ("o-abr", history(0, 1, 0)),
                ("o-bolt", history(0, 0, 1)),
                ("o-x", history(0, 0, 3)),
            ],
            4,
            2,
        );

        let guide = sideboard_rows(&zones, None, &stats, &cards);

        // Rest in Peace 3 games, then Abrade and Cut Down at 1 by name.
        assert_eq!(in_ids(&guide), ["o-rip", "o-abr", "o-cut"]);
        // The card with no row is named "Unknown card" and leads on games.
        assert_eq!(out_ids(&guide), ["o-x", "o-bolt"]);
        assert_eq!(guide.sided_out[0].name, "Unknown card");
    }

    #[test]
    fn plan_scope_sorts_by_name() {
        let zones = zones(&[("o-rip", 2), ("o-cut", 2)], &[("o-bolt", 4)]);
        let stats = stats(&[("o-rip", history(3, 0, 0))], 3, 3);
        let plan = [
            planned("o-rip", Direction::In, 2),
            planned("o-cut", Direction::In, 1),
        ];

        let guide = sideboard_rows(&zones, Some(&plan), &stats, &meta());

        assert_eq!(in_ids(&guide), ["o-cut", "o-rip"]);
        assert!(guide.sided_out.is_empty());
        // The badge still comes from history in Plan scope.
        assert!(sided_in(&guide, "o-rip").unwrap().recommended);
    }

    #[test]
    fn the_sided_in_win_rate_multiplies_first_like_0x_winrate() {
        let zones = zones(&[("o-rip", 2)], &[]);
        let stats = stats(&[("o-rip", history(23, 17, 0))], 40, 23);

        let guide = sideboard_rows(&zones, None, &stats, &meta());

        // 100 * 23 / 40 is exactly 57.5.
        assert_eq!(sided_in(&guide, "o-rip").unwrap().winrate, Some(58));
    }

    #[test]
    fn sample_rate_matches_0x() {
        assert_eq!(SampleRate { count: 1, games: 0 }.percentage(), None);
        assert_eq!(SampleRate { count: 1, games: 8 }.percentage(), Some(13));
        assert_eq!(SampleRate { count: 2, games: 3 }.percentage(), Some(67));
        // Divide first: 23 / 40 * 100 is 57.49999999999999.
        assert_eq!(
            SampleRate {
                count: 23,
                games: 40
            }
            .percentage(),
            Some(57)
        );
        assert!(!SampleRate { count: 0, games: 0 }.is_confident(0));
        assert!(SampleRate { count: 0, games: 3 }.is_confident(3));
        assert!(SampleRate { count: 2, games: 4 }.supports(50, 3));
        assert!(!SampleRate { count: 1, games: 3 }.supports(50, 3));
        assert!(!SampleRate { count: 2, games: 2 }.supports(50, 3));
    }
}
