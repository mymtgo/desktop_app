//! One card's numbers on a deck's Card Stats tab (0.x `GetCardGameStats`,
//! cards survey section 9) and the card type categories (0.x
//! `IndexController::TYPE_CATEGORIES`).
//!
//! The store fills `CardStat` from `game_cards`; this module only holds the
//! shape and the small pure rules around it.

/// The card type categories in canonical precedence: a card's category is
/// the first one its type line contains, so an "Artifact Creature" is a
/// Creature (0.x `IndexController::TYPE_CATEGORIES`).
pub const TYPE_CATEGORIES: [&str; 8] = [
    "Creature",
    "Planeswalker",
    "Battle",
    "Instant",
    "Sorcery",
    "Enchantment",
    "Artifact",
    "Land",
];

/// The category of a card with no recognised type (never hidden).
pub const OTHER_CATEGORY: &str = "Other";

/// A card's type category: the first of `TYPE_CATEGORIES` its type line
/// contains (any case, as SQL `LIKE`), else `Other`.
pub fn type_category(type_line: Option<&str>) -> &'static str {
    let Some(line) = type_line else {
        return OTHER_CATEGORY;
    };
    let lower = line.to_ascii_lowercase();
    for category in TYPE_CATEGORIES {
        if lower.contains(&category.to_ascii_lowercase()) {
            return category;
        }
    }
    OTHER_CATEGORY
}

/// Which games a card stats read counts (0.x `is_postboard`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Board {
    /// Game 1 only.
    Game1,
    /// Games 2 and 3.
    Postboard,
}

/// One card's numbers over the games read (0.x `GetCardGameStats` row).
///
/// Printings of one card are one row, keyed by oracle id; a card without
/// card data is keyed by its catalog id (`#{id}`) and named "Card {id}"
/// (cards spec ruling 5). "Won" means the game's result is a win, so a draw
/// counts as not won (0.x `won` per game).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardStat {
    /// The oracle id, or `#{catalog id}` for a card without one.
    pub key: String,
    pub oracle_id: Option<String>,
    /// The printing shown: the highest catalog id with this oracle id.
    pub catalog_id: u32,
    pub name: String,
    /// The canonical form (`crate::colors::normalize_colors`).
    pub color_identity: Option<String>,
    /// The `type` column (0.x `type`).
    pub card_type: Option<String>,
    pub image: Option<String>,
    /// Its file in v1's images folder, when downloaded.
    pub local_image: Option<String>,
    /// In the sideboard of the chosen (or latest) version; never for the
    /// opponent's cards.
    pub is_sideboard: bool,
    /// Games with a row for the card; for the opponent's cards, every local
    /// game under the same filters (their rows exist only when a signal
    /// fired).
    pub total_games: u32,
    pub total_possible: u32,
    pub total_kept: u32,
    pub kept_games: u32,
    pub kept_won: u32,
    pub kept_lost: u32,
    pub total_seen: u32,
    pub seen_games: u32,
    pub seen_won: u32,
    pub seen_lost: u32,
    pub total_cast: u32,
    pub cast_games: u32,
    pub cast_won: u32,
    pub cast_lost: u32,
    pub postboard_games: u32,
    pub sided_out_games: u32,
    pub sided_in_games: u32,
    pub total_played: u32,
    pub played_games: u32,
    pub total_kicked: u32,
    pub total_activated: u32,
    pub total_flashback: u32,
    pub total_madness: u32,
    pub total_evoked: u32,
    pub total_warp: u32,
    pub total_free_cast: u32,
    pub total_bargained: u32,
    pub total_dashed: u32,
    pub total_bestowed: u32,
    pub total_replicated: u32,
    pub total_spectacle: u32,
    pub total_rebound: u32,
    pub total_escaped: u32,
    pub total_ninjutsu: u32,
    pub total_suspended: u32,
    pub total_buyback: u32,
    pub total_disturb: u32,
    pub total_foretold: u32,
    pub total_retraced: u32,
    pub total_mayhem: u32,
    pub total_miracle: u32,
    pub total_gifted: u32,
    pub total_casualty: u32,
    pub pregame_revealed_games: u32,
    pub pregame_played_games: u32,
    pub pregame_games: u32,
    pub pregame_won: u32,
    pub pregame_lost: u32,
    /// Games whose row is inferred (an imported postboard game, cards spec
    /// ruling 8): the SB In cell's marker.
    pub inferred_games: u32,
}

impl CardStat {
    /// Add casts by an alternative method (`game_card_casts.method`). An
    /// unknown method is ignored.
    pub fn add_casts(&mut self, method: &str, count: u32) {
        let total = match method {
            "kicked" => &mut self.total_kicked,
            "flashback" => &mut self.total_flashback,
            "madness" => &mut self.total_madness,
            "evoked" => &mut self.total_evoked,
            "warp" => &mut self.total_warp,
            "free_cast" => &mut self.total_free_cast,
            "bargained" => &mut self.total_bargained,
            "dashed" => &mut self.total_dashed,
            "bestowed" => &mut self.total_bestowed,
            "replicated" => &mut self.total_replicated,
            "spectacle" => &mut self.total_spectacle,
            "rebound" => &mut self.total_rebound,
            "escaped" => &mut self.total_escaped,
            "ninjutsu" => &mut self.total_ninjutsu,
            "suspended" => &mut self.total_suspended,
            "buyback" => &mut self.total_buyback,
            "disturb" => &mut self.total_disturb,
            "foretold" => &mut self.total_foretold,
            "retraced" => &mut self.total_retraced,
            "mayhem" => &mut self.total_mayhem,
            "miracle" => &mut self.total_miracle,
            "gifted" => &mut self.total_gifted,
            "casualty" => &mut self.total_casualty,
            _ => return,
        };
        *total += count;
    }
}

/// The key a card stat row groups by: the oracle id, else `#{catalog id}`.
pub fn stat_key(oracle_id: Option<&str>, catalog_id: u32) -> String {
    match oracle_id {
        Some(oracle) => oracle.to_string(),
        None => format!("#{catalog_id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_takes_the_first_category_its_type_line_holds() {
        assert_eq!(type_category(Some("Artifact Creature")), "Creature");
        assert_eq!(
            type_category(Some("Legendary Planeswalker")),
            "Planeswalker"
        );
        assert_eq!(type_category(Some("Basic Land")), "Land");
        assert_eq!(type_category(Some("Kindred Instant")), "Instant");
        assert_eq!(type_category(Some("Battle")), "Battle");
        assert_eq!(type_category(Some("artifact land")), "Artifact");
        assert_eq!(type_category(Some("Conspiracy")), "Other");
        assert_eq!(type_category(None), "Other");
    }

    #[test]
    fn casts_add_to_their_method_and_unknown_methods_are_ignored() {
        let mut stat = CardStat::default();
        stat.add_casts("kicked", 2);
        stat.add_casts("kicked", 1);
        stat.add_casts("free_cast", 4);
        stat.add_casts("casualty", 1);
        stat.add_casts("nonsense", 9);
        assert_eq!(stat.total_kicked, 3);
        assert_eq!(stat.total_free_cast, 4);
        assert_eq!(stat.total_casualty, 1);
    }

    #[test]
    fn an_unresolved_card_is_keyed_by_its_catalog_id() {
        assert_eq!(stat_key(Some("q-oracle"), 12), "q-oracle");
        assert_eq!(stat_key(None, 12), "#12");
    }
}
