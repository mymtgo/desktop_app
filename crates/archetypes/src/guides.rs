//! Sideboard guides and matchup notes (archetypes spec section 21.2, survey
//! section 13): the plan's zones and checks, the editor rows, the plan
//! summary and balance, and the notes split.
//!
//! A guide is the player's plan for one deck against one opponent
//! archetype: copies to bring in from the sideboard and to take out of the
//! maindeck, by oracle id. Pure: the store reads the rows and passes them in.

use std::collections::BTreeMap;

use decks::stats::winrate_percent;

/// 0.x `UpdateSideboardGuideCardsRequest`: the same card twice in one
/// direction.
pub const LISTED_TWICE: &str = "This card is listed twice for the same direction.";

/// 0.x `UpdateSideboardGuideCardsRequest`: a card the zone does not hold
/// (accepted only when the saved plan already lists it unchanged).
pub const NOT_IN_ZONE: &str = "This card is not in that part of the current decklist.";

/// 0.x `StoreSideboardGuideRequest`'s unique message.
pub const GUIDE_EXISTS: &str = "A guide for this archetype already exists for this deck.";

/// Laravel's `required` message for a note's body.
pub const NOTE_REQUIRED: &str = "The body field is required.";

/// Laravel's `max:2000` message for a note's body.
pub const NOTE_TOO_LONG: &str = "The body field must not be greater than 2000 characters.";

/// The longest note body, in characters.
pub const NOTE_MAX_CHARS: usize = 2000;

/// The highest planned quantity of one card.
pub const MAX_QUANTITY: i64 = 99;

/// The longest oracle id.
pub const MAX_ORACLE_CHARS: usize = 64;

/// Which way a planned card goes (0.x `SideboardDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    /// Brought in from the sideboard.
    In,
    /// Taken out of the maindeck.
    Out,
}

impl Direction {
    /// The `sideboard_guide_cards.direction` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }

    /// Reads `in` or `out`; `None` for any other text.
    pub fn parse(text: &str) -> Option<Direction> {
        match text {
            "in" => Some(Direction::In),
            "out" => Some(Direction::Out),
            _ => None,
        }
    }
}

/// One card entry of a deck version (a signature segment), with the oracle
/// id of its printing when the card is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionCard {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// Copies per oracle id a plan may use: the sideboard's (to bring in) and
/// the maindeck's (to take out).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Zones {
    pub sideboard: BTreeMap<String, u32>,
    pub maindeck: BTreeMap<String, u32>,
}

impl Zones {
    /// The zone a direction draws from.
    pub fn of(&self, direction: Direction) -> &BTreeMap<String, u32> {
        match direction {
            Direction::In => &self.sideboard,
            Direction::Out => &self.maindeck,
        }
    }

    /// Copies in the sideboard (the balance chip's board size).
    pub fn sideboard_size(&self) -> u32 {
        self.sideboard.values().sum()
    }
}

/// One planned card (`sideboard_guide_cards`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideCard {
    pub oracle_id: String,
    pub direction: Direction,
    pub quantity: u32,
}

/// One planned card as the editor sends it, before the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideCardInput {
    pub oracle_id: String,
    pub direction: String,
    pub quantity: i64,
}

/// One refused entry of a submitted plan: its index, the field
/// (`oracle_id`, `direction` or `quantity`) and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    pub index: usize,
    pub field: String,
    pub message: String,
}

/// A card's numbers against the archetype over every version of the deck:
/// games it was sided in (and their wins and losses) and games it was
/// sided out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SidedStat {
    pub sided_in_games: u32,
    pub sided_in_won: u32,
    pub sided_in_lost: u32,
    pub sided_out_games: u32,
}

/// Postboard games against the archetype and how many were won.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Postboard {
    pub games: u32,
    pub wins: u32,
}

/// A card's data from the `cards` table (the version's own printing first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardMeta {
    pub catalog_id: Option<u32>,
    pub name: Option<String>,
    pub card_type: Option<String>,
    pub color_identity: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
}

/// One row of an editor column (0.x `SideboardCardData` and
/// `SidedOutCardData` in Editor scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorRow {
    pub oracle_id: String,
    pub catalog_id: Option<u32>,
    pub name: String,
    pub card_type: Option<String>,
    pub color_identity: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// Copies in the zone (the stepper's ceiling); 0 on a stale row.
    pub quantity: u32,
    /// The plan's copies, `None` when the plan does not list the card.
    pub planned: Option<u32>,
    /// Planned but no longer in this zone of the latest version.
    pub stale: bool,
    pub sided_in_games: u32,
    pub wins: u32,
    pub losses: u32,
    /// 0.x `Winrate::percentage`, `None` without sided-in games.
    pub winrate: Option<u32>,
    pub sided_out_games: u32,
}

/// The guide editor's columns (0.x `BuildSideboardGuide`, Editor scope).
/// There is no History or Plan scope and no community data; a plan is
/// always given, so "Editor scope without a plan" cannot be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideEditor {
    /// Every sideboard card, then stale planned in-cards, by name.
    pub bring_in: Vec<EditorRow>,
    /// Every maindeck card, then stale planned out-cards, by name.
    pub take_out: Vec<EditorRow>,
    pub postboard_games: u32,
    /// "W - L".
    pub postboard_record: String,
    /// The plan lists at least one card.
    pub has_plan: bool,
}

/// The balance chip (0.x `SideboardGuideBalance`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balance {
    /// The board after the plan: size minus in plus out.
    pub resulting: i64,
    /// "cut {n} more", "bring in {n} more", or `None` when balanced.
    pub hint: Option<String>,
}

/// One matchup note with its deck's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: i64,
    pub deck_id: i64,
    pub deck_name: String,
    pub body: String,
    pub created_at: i64,
}

/// An archetype's notes split by deck (0.x `GetArchetypeNotes`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchetypeNotes {
    /// This deck's, newest first.
    pub current: Vec<Note>,
    /// Every other deck's, newest first.
    pub other: Vec<Note>,
}

/// Copies per oracle id by zone (0.x `GetVersionZoneQuantities`): sideboard
/// copies can be brought in, maindeck copies taken out; a card split
/// across zones or held as two printings is summed per zone; a card
/// without an oracle id is skipped.
pub fn zone_quantities(cards: &[VersionCard]) -> Zones {
    let mut zones = Zones::default();
    for card in cards {
        let Some(oracle_id) = &card.oracle_id else {
            continue;
        };
        let zone = if card.sideboard {
            &mut zones.sideboard
        } else {
            &mut zones.maindeck
        };
        *zone.entry(oracle_id.clone()).or_insert(0) += card.quantity;
    }
    zones
}

fn plan_error(index: usize, field: &str, message: String) -> PlanError {
    PlanError {
        index,
        field: field.to_string(),
        message,
    }
}

/// Laravel's rule messages for one entry's shape (the attribute is shown
/// as Laravel shows it: `cards.0.oracle id`).
fn shape_errors(index: usize, card: &GuideCardInput) -> Vec<PlanError> {
    let mut errors = Vec::new();
    let oracle_id = card.oracle_id.trim();
    if oracle_id.is_empty() {
        errors.push(plan_error(
            index,
            "oracle_id",
            format!("The cards.{index}.oracle id field is required."),
        ));
    } else if oracle_id.chars().count() > MAX_ORACLE_CHARS {
        errors.push(plan_error(
            index,
            "oracle_id",
            format!(
                "The cards.{index}.oracle id field must not be greater than {MAX_ORACLE_CHARS} characters."
            ),
        ));
    }
    let direction = card.direction.trim();
    if direction.is_empty() {
        errors.push(plan_error(
            index,
            "direction",
            format!("The cards.{index}.direction field is required."),
        ));
    } else if Direction::parse(direction).is_none() {
        errors.push(plan_error(
            index,
            "direction",
            format!("The selected cards.{index}.direction is invalid."),
        ));
    }
    if card.quantity < 1 {
        errors.push(plan_error(
            index,
            "quantity",
            format!("The cards.{index}.quantity field must be at least 1."),
        ));
    } else if card.quantity > MAX_QUANTITY {
        errors.push(plan_error(
            index,
            "quantity",
            format!("The cards.{index}.quantity field must not be greater than {MAX_QUANTITY}."),
        ));
    }
    errors
}

/// Check a submitted plan against the latest version's zones and the saved
/// plan (0.x `UpdateSideboardGuideCardsRequest`). The shape errors come
/// first (Laravel's rules), then the zone checks (its `after` hook), each
/// in entry order. An entry may list a card its zone does not hold only
/// when the saved plan has it at the same quantity (a stale entry the
/// player has not removed yet).
pub fn check_plan(
    zones: &Zones,
    saved: &[GuideCard],
    submitted: &[GuideCardInput],
) -> Result<Vec<GuideCard>, Vec<PlanError>> {
    let mut errors = Vec::new();
    for (index, card) in submitted.iter().enumerate() {
        errors.extend(shape_errors(index, card));
    }

    let mut seen: Vec<(Direction, String)> = Vec::new();
    let mut checked = Vec::new();
    for (index, card) in submitted.iter().enumerate() {
        let oracle_id = card.oracle_id.trim().to_string();
        let Some(direction) = Direction::parse(card.direction.trim()) else {
            continue;
        };
        if oracle_id.is_empty() {
            continue;
        }
        let key = (direction, oracle_id.clone());
        if seen.contains(&key) {
            errors.push(plan_error(index, "oracle_id", LISTED_TWICE.to_string()));
            continue;
        }
        seen.push(key);

        match zones.of(direction).get(&oracle_id) {
            None => {
                let unchanged = saved.iter().any(|held| {
                    held.direction == direction
                        && held.oracle_id == oracle_id
                        && i64::from(held.quantity) == card.quantity
                });
                if !unchanged {
                    errors.push(plan_error(index, "oracle_id", NOT_IN_ZONE.to_string()));
                }
            }
            Some(ceiling) => {
                if card.quantity > i64::from(*ceiling) {
                    let noun = match direction {
                        Direction::In => "sideboard",
                        Direction::Out => "maindeck",
                    };
                    errors.push(plan_error(
                        index,
                        "quantity",
                        format!("Only {ceiling} in the {noun}."),
                    ));
                }
            }
        }
        if let Ok(quantity) = u32::try_from(card.quantity) {
            checked.push(GuideCard {
                oracle_id,
                direction,
                quantity,
            });
        }
    }

    if errors.is_empty() {
        Ok(checked)
    } else {
        Err(errors)
    }
}

/// One editor row from a zone's copies, the plan and the numbers.
fn editor_row(
    oracle_id: &str,
    quantity: u32,
    planned: Option<u32>,
    stale: bool,
    stats: &BTreeMap<String, SidedStat>,
    card_meta: &BTreeMap<String, CardMeta>,
) -> EditorRow {
    let stat = stats.get(oracle_id).copied().unwrap_or_default();
    let meta = card_meta.get(oracle_id).cloned().unwrap_or_default();
    // 0.x `BuildSideboardGuide`: `Winrate::percentage($wins, $losses)`.
    let winrate = if stat.sided_in_games > 0 {
        Some(winrate_percent(stat.sided_in_won, stat.sided_in_lost, 0))
    } else {
        None
    };
    EditorRow {
        oracle_id: oracle_id.to_string(),
        catalog_id: meta.catalog_id,
        name: meta.name.unwrap_or_else(|| "Unknown card".to_string()),
        card_type: meta.card_type,
        color_identity: meta.color_identity,
        image: meta.image,
        local_image: meta.local_image,
        quantity,
        planned,
        stale,
        sided_in_games: stat.sided_in_games,
        wins: stat.sided_in_won,
        losses: stat.sided_in_lost,
        winrate,
        sided_out_games: stat.sided_out_games,
    }
}

/// One editor column: every card of the zone, then the planned cards the
/// zone no longer holds (stale, quantity 0), sorted by name.
fn column(
    zone: &BTreeMap<String, u32>,
    planned: &BTreeMap<String, u32>,
    stats: &BTreeMap<String, SidedStat>,
    card_meta: &BTreeMap<String, CardMeta>,
) -> Vec<EditorRow> {
    let mut rows = Vec::new();
    for (oracle_id, quantity) in zone {
        let plan = planned.get(oracle_id).copied();
        rows.push(editor_row(
            oracle_id, *quantity, plan, false, stats, card_meta,
        ));
    }
    for (oracle_id, quantity) in planned {
        if !zone.contains_key(oracle_id) {
            rows.push(editor_row(
                oracle_id,
                0,
                Some(*quantity),
                true,
                stats,
                card_meta,
            ));
        }
    }
    rows.sort_by(|a, b| (&a.name, &a.oracle_id).cmp(&(&b.name, &b.oracle_id)));
    rows
}

/// The editor's columns (0.x `BuildSideboardGuide`, Editor scope): cards
/// are listed from the latest version's zones while the numbers count
/// every version of the deck against the archetype (`stats`, `postboard`).
pub fn editor_rows(
    zones: &Zones,
    planned: &[GuideCard],
    stats: &BTreeMap<String, SidedStat>,
    postboard: Postboard,
    card_meta: &BTreeMap<String, CardMeta>,
) -> GuideEditor {
    let mut planned_in = BTreeMap::new();
    let mut planned_out = BTreeMap::new();
    for card in planned {
        match card.direction {
            Direction::In => planned_in.insert(card.oracle_id.clone(), card.quantity),
            Direction::Out => planned_out.insert(card.oracle_id.clone(), card.quantity),
        };
    }
    let losses = postboard.games.saturating_sub(postboard.wins);
    GuideEditor {
        bring_in: column(&zones.sideboard, &planned_in, stats, card_meta),
        take_out: column(&zones.maindeck, &planned_out, stats, card_meta),
        postboard_games: postboard.games,
        postboard_record: format!("{} - {losses}", postboard.wins),
        has_plan: !planned.is_empty(),
    }
}

/// Planned copies summed per direction: `(in, out)`.
pub fn plan_summary(cards: &[GuideCard]) -> (u32, u32) {
    let mut cards_in = 0;
    let mut cards_out = 0;
    for card in cards {
        match card.direction {
            Direction::In => cards_in += card.quantity,
            Direction::Out => cards_out += card.quantity,
        }
    }
    (cards_in, cards_out)
}

/// The board after the plan and what is left to balance it (0.x
/// `SideboardGuideBalance`): cards cut from the maindeck go back to the
/// board, cards brought in leave it.
pub fn balance(total_in: u32, total_out: u32, sideboard_size: u32) -> Balance {
    let resulting = i64::from(sideboard_size) - i64::from(total_in) + i64::from(total_out);
    let shortfall = i64::from(sideboard_size) - resulting;
    let hint = if shortfall > 0 {
        Some(format!("cut {shortfall} more"))
    } else if shortfall < 0 {
        Some(format!("bring in {} more", -shortfall))
    } else {
        None
    };
    Balance { resulting, hint }
}

/// The archetype's notes split into this deck's and the others', newest
/// first by `created_at`, then id (0.x `GetArchetypeNotes`).
pub fn notes_for(deck_id: i64, mut notes: Vec<Note>) -> ArchetypeNotes {
    notes.sort_by_key(|note| std::cmp::Reverse((note.created_at, note.id)));
    let mut split = ArchetypeNotes::default();
    for note in notes {
        if note.deck_id == deck_id {
            split.current.push(note);
        } else {
            split.other.push(note);
        }
    }
    split
}

/// A note body as stored: the ends trimmed (Laravel's `TrimStrings`), then
/// 1 to 2000 characters, else Laravel's message.
pub fn check_note_body(body: &str) -> Result<String, &'static str> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(NOTE_REQUIRED);
    }
    if trimmed.chars().count() > NOTE_MAX_CHARS {
        return Err(NOTE_TOO_LONG);
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vc(catalog_id: u32, oracle: Option<&str>, quantity: u32, sideboard: bool) -> VersionCard {
        VersionCard {
            catalog_id,
            oracle_id: oracle.map(str::to_string),
            quantity,
            sideboard,
        }
    }

    fn map(entries: &[(&str, u32)]) -> BTreeMap<String, u32> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), *value))
            .collect()
    }

    // ---- GetVersionZoneQuantitiesTest -------------------------------------

    #[test]
    fn sums_copies_per_oracle_id_by_zone_collapsing_split_cards() {
        // Two Bolt printings in the main, one Bolt in the board, two RiP in
        // the board, and a card without an oracle id.
        let zones = zone_quantities(&[
            vc(202, Some("o-bolt"), 2, false),
            vc(302, Some("o-bolt"), 1, false),
            vc(202, Some("o-bolt"), 1, true),
            vc(201, Some("o-rip"), 2, true),
            vc(999, None, 4, false),
        ]);
        assert_eq!(zones.sideboard, map(&[("o-bolt", 1), ("o-rip", 2)]));
        assert_eq!(zones.maindeck, map(&[("o-bolt", 3)]));
        assert_eq!(zones.sideboard_size(), 3);
    }

    // ---- UpdateCardsControllerTest (as check_plan) ------------------------

    /// 0.x `savableGuide`: 4 Bolt main, 2 RiP board, 1 Cut Down board.
    fn savable() -> Zones {
        zone_quantities(&[
            vc(202, Some("o-bolt"), 4, false),
            vc(201, Some("o-rip"), 2, true),
            vc(203, Some("o-cut"), 1, true),
        ])
    }

    fn input(oracle: &str, direction: &str, quantity: i64) -> GuideCardInput {
        GuideCardInput {
            oracle_id: oracle.to_string(),
            direction: direction.to_string(),
            quantity,
        }
    }

    fn card(oracle: &str, direction: Direction, quantity: u32) -> GuideCard {
        GuideCard {
            oracle_id: oracle.to_string(),
            direction,
            quantity,
        }
    }

    fn error(index: usize, field: &str, message: &str) -> PlanError {
        PlanError {
            index,
            field: field.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn accepts_the_whole_plan_as_submitted() {
        let saved = [card("o-cut", Direction::In, 1)];
        let checked = check_plan(
            &savable(),
            &saved,
            &[input("o-rip", "in", 2), input("o-bolt", "out", 2)],
        );
        assert_eq!(
            checked,
            Ok(vec![
                card("o-rip", Direction::In, 2),
                card("o-bolt", Direction::Out, 2)
            ])
        );
    }

    #[test]
    fn an_empty_plan_clears_it() {
        let saved = [card("o-cut", Direction::In, 1)];
        assert_eq!(check_plan(&savable(), &saved, &[]), Ok(Vec::new()));
    }

    #[test]
    fn rejects_bringing_in_more_copies_than_the_sideboard_holds() {
        assert_eq!(
            check_plan(&savable(), &[], &[input("o-rip", "in", 3)]),
            Err(vec![error(0, "quantity", "Only 2 in the sideboard.")])
        );
    }

    #[test]
    fn rejects_taking_out_more_copies_than_the_maindeck_holds() {
        assert_eq!(
            check_plan(&savable(), &[], &[input("o-bolt", "out", 5)]),
            Err(vec![error(0, "quantity", "Only 4 in the maindeck.")])
        );
    }

    #[test]
    fn rejects_a_card_not_in_that_zone_unless_the_guide_already_lists_it() {
        // Bolt is maindeck only, so bringing it in is not possible.
        assert_eq!(
            check_plan(&savable(), &[], &[input("o-bolt", "in", 1)]),
            Err(vec![error(0, "oracle_id", NOT_IN_ZONE)])
        );
        // A stale entry already saved may be resubmitted unchanged.
        let saved = [card("o-gone", Direction::In, 2)];
        assert_eq!(
            check_plan(&savable(), &saved, &[input("o-gone", "in", 2)]),
            Ok(vec![card("o-gone", Direction::In, 2)])
        );
        // But not at another quantity.
        assert_eq!(
            check_plan(&savable(), &saved, &[input("o-gone", "in", 1)]),
            Err(vec![error(0, "oracle_id", NOT_IN_ZONE)])
        );
    }

    #[test]
    fn rejects_the_same_card_listed_twice_for_one_direction() {
        assert_eq!(
            check_plan(
                &savable(),
                &[],
                &[input("o-rip", "in", 1), input("o-rip", "in", 1)]
            ),
            Err(vec![error(1, "oracle_id", LISTED_TWICE)])
        );
    }

    #[test]
    fn validates_the_card_shape() {
        assert_eq!(
            check_plan(&savable(), &[], &[input("o-rip", "sideways", 0)]),
            Err(vec![
                error(0, "direction", "The selected cards.0.direction is invalid."),
                error(
                    0,
                    "quantity",
                    "The cards.0.quantity field must be at least 1."
                ),
            ])
        );
        let long = "o".repeat(65);
        assert_eq!(
            check_plan(
                &savable(),
                &[],
                &[input("", "in", 1), input(&long, "out", 100)]
            ),
            Err(vec![
                error(0, "oracle_id", "The cards.0.oracle id field is required."),
                error(
                    1,
                    "oracle_id",
                    "The cards.1.oracle id field must not be greater than 64 characters."
                ),
                error(
                    1,
                    "quantity",
                    "The cards.1.quantity field must not be greater than 99."
                ),
                error(1, "oracle_id", NOT_IN_ZONE),
            ])
        );
    }

    // ---- BuildSideboardGuideTest (Editor scope) ---------------------------

    fn meta(name: &str, card_type: &str, colors: &str) -> CardMeta {
        CardMeta {
            catalog_id: None,
            name: Some(name.to_string()),
            card_type: Some(card_type.to_string()),
            color_identity: Some(colors.to_string()),
            image: None,
            local_image: None,
        }
    }

    /// 0.x `beforeEach`: Rest in Peace, Lightning Bolt, Cut Down.
    fn metas() -> BTreeMap<String, CardMeta> {
        BTreeMap::from([
            (
                "o-rip".to_string(),
                meta("Rest in Peace", "Enchantment", "W"),
            ),
            ("o-bolt".to_string(), meta("Lightning Bolt", "Instant", "R")),
            ("o-cut".to_string(), meta("Cut Down", "Instant", "B")),
        ])
    }

    fn row<'a>(rows: &'a [EditorRow], oracle: &str) -> &'a EditorRow {
        rows.iter()
            .find(|row| row.oracle_id == oracle)
            .expect("the row is listed")
    }

    fn oracles(rows: &[EditorRow]) -> Vec<&str> {
        rows.iter().map(|row| row.oracle_id.as_str()).collect()
    }

    fn editor(
        zones: &Zones,
        planned: &[GuideCard],
        stats: &BTreeMap<String, SidedStat>,
        postboard: Postboard,
    ) -> GuideEditor {
        editor_rows(zones, planned, stats, postboard, &metas())
    }

    #[test]
    fn lists_every_sideboard_and_maindeck_card_in_editor_scope_with_planned_quantities_attached() {
        let zones = zone_quantities(&[
            vc(201, Some("o-rip"), 2, true),
            vc(202, Some("o-bolt"), 4, false),
            vc(203, Some("o-cut"), 2, true),
        ]);
        let plan = [card("o-rip", Direction::In, 2)];
        let guide = editor(&zones, &plan, &BTreeMap::new(), Postboard::default());

        assert_eq!(oracles(&guide.bring_in), ["o-cut", "o-rip"], "by name");
        let rip = row(&guide.bring_in, "o-rip");
        assert_eq!((rip.quantity, rip.planned), (2, Some(2)));
        assert_eq!(row(&guide.bring_in, "o-cut").planned, None);
        // A maindeck card with no history is still listed.
        assert_eq!(oracles(&guide.take_out), ["o-bolt"]);
        assert_eq!(
            (guide.take_out[0].quantity, guide.take_out[0].planned),
            (4, None)
        );
        assert!(guide.has_plan);
    }

    #[test]
    fn flags_planned_cards_missing_from_the_current_version_as_stale() {
        // Rest in Peace is no longer in the deck.
        let zones = zone_quantities(&[
            vc(202, Some("o-bolt"), 4, false),
            vc(203, Some("o-cut"), 2, true),
        ]);
        let plan = [card("o-rip", Direction::In, 2)];
        let guide = editor(&zones, &plan, &BTreeMap::new(), Postboard::default());

        let rip = row(&guide.bring_in, "o-rip");
        assert_eq!(rip.name, "Rest in Peace");
        assert!(rip.stale);
        assert_eq!((rip.quantity, rip.planned), (0, Some(2)));
        assert!(!row(&guide.bring_in, "o-cut").stale);
    }

    #[test]
    fn marks_a_planned_in_card_that_moved_to_the_maindeck_as_stale_while_still_listing_it_as_a_maindeck_row()
     {
        let zones = zone_quantities(&[
            vc(201, Some("o-rip"), 2, false),
            vc(202, Some("o-bolt"), 4, false),
            vc(203, Some("o-cut"), 2, true),
        ]);
        let plan = [card("o-rip", Direction::In, 2)];
        let guide = editor(&zones, &plan, &BTreeMap::new(), Postboard::default());

        let stale_in = row(&guide.bring_in, "o-rip");
        assert!(stale_in.stale);
        assert_eq!((stale_in.quantity, stale_in.planned), (0, Some(2)));
        let main_rip = row(&guide.take_out, "o-rip");
        assert!(!main_rip.stale);
        assert_eq!((main_rip.quantity, main_rip.planned), (2, None));
    }

    #[test]
    fn lists_a_card_split_across_zones_in_both_editor_columns_with_per_zone_copies_and_no_stale_flag()
     {
        // 3 Bolt main (two printings) + 1 Bolt board, 2 RiP board.
        let zones = zone_quantities(&[
            vc(202, Some("o-bolt"), 2, false),
            vc(302, Some("o-bolt"), 1, false),
            vc(202, Some("o-bolt"), 1, true),
            vc(201, Some("o-rip"), 2, true),
        ]);
        let plan = [card("o-bolt", Direction::Out, 3)];
        let guide = editor(&zones, &plan, &BTreeMap::new(), Postboard::default());

        let board = row(&guide.bring_in, "o-bolt");
        assert_eq!((board.quantity, board.stale), (1, false));
        let main = row(&guide.take_out, "o-bolt");
        assert_eq!(
            (main.quantity, main.planned, main.stale),
            (3, Some(3), false)
        );
    }

    #[test]
    fn lists_a_card_split_between_the_maindeck_and_the_sideboard_once() {
        // Rest in Peace: 3 main, 1 board.
        let zones = zone_quantities(&[
            vc(201, Some("o-rip"), 3, false),
            vc(201, Some("o-rip"), 1, true),
            vc(202, Some("o-bolt"), 4, false),
        ]);
        let guide = editor(&zones, &[], &BTreeMap::new(), Postboard::default());

        let rips: Vec<&EditorRow> = guide
            .bring_in
            .iter()
            .filter(|row| row.oracle_id == "o-rip")
            .collect();
        assert_eq!(rips.len(), 1);
        assert_eq!(
            rips[0].quantity, 1,
            "the sideboard copies, not the main ones"
        );
        assert_eq!(rips[0].name, "Rest in Peace");
        assert!(!guide.has_plan);
    }

    #[test]
    fn names_every_sideboard_card_with_no_postboard_history_at_all() {
        let zones = zone_quantities(&[
            vc(201, Some("o-rip"), 2, true),
            vc(203, Some("o-cut"), 2, true),
            vc(202, Some("o-bolt"), 4, false),
        ]);
        let guide = editor(&zones, &[], &BTreeMap::new(), Postboard::default());

        assert_eq!(guide.postboard_games, 0);
        let names: Vec<&str> = guide.bring_in.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["Cut Down", "Rest in Peace"]);
        let rip = row(&guide.bring_in, "o-rip");
        assert_eq!(rip.color_identity.as_deref(), Some("W"));
        assert_eq!((rip.sided_in_games, rip.winrate), (0, None));
    }

    #[test]
    fn reports_sided_in_w_l_and_the_postboard_baseline_against_the_archetype() {
        let zones = zone_quantities(&[
            vc(201, Some("o-rip"), 2, true),
            vc(202, Some("o-bolt"), 4, false),
            vc(203, Some("o-cut"), 2, true),
        ]);
        let stats = BTreeMap::from([
            (
                "o-rip".to_string(),
                SidedStat {
                    sided_in_games: 2,
                    sided_in_won: 1,
                    sided_in_lost: 1,
                    sided_out_games: 0,
                },
            ),
            (
                "o-bolt".to_string(),
                SidedStat {
                    sided_out_games: 2,
                    ..SidedStat::default()
                },
            ),
        ]);
        let guide = editor(&zones, &[], &stats, Postboard { games: 2, wins: 1 });

        assert_eq!(guide.postboard_games, 2);
        assert_eq!(guide.postboard_record, "1 - 1");
        let rip = row(&guide.bring_in, "o-rip");
        assert_eq!(
            (rip.sided_in_games, rip.wins, rip.losses, rip.winrate),
            (2, 1, 1, Some(50))
        );
        let cut = row(&guide.bring_in, "o-cut");
        assert_eq!((cut.sided_in_games, cut.winrate), (0, None));
        assert_eq!(oracles(&guide.take_out), ["o-bolt"]);
        assert_eq!(guide.take_out[0].sided_out_games, 2);
    }

    #[test]
    fn a_card_without_card_data_is_an_unknown_card() {
        let zones = zone_quantities(&[vc(777, Some("o-new"), 1, true)]);
        let guide = editor_rows(
            &zones,
            &[],
            &BTreeMap::new(),
            Postboard::default(),
            &BTreeMap::new(),
        );
        assert_eq!(guide.bring_in[0].name, "Unknown card");
    }

    // ---- Plan summary, balance, notes -------------------------------------

    #[test]
    fn the_plan_summary_sums_copies_per_direction() {
        let plan = [
            card("o-a", Direction::In, 2),
            card("o-b", Direction::In, 1),
            card("o-c", Direction::Out, 3),
        ];
        assert_eq!(plan_summary(&plan), (3, 3));
        assert_eq!(plan_summary(&[]), (0, 0));
    }

    #[test]
    fn the_balance_says_what_is_left_to_cut_or_bring_in() {
        assert_eq!(
            balance(3, 1, 15),
            Balance {
                resulting: 13,
                hint: Some("cut 2 more".to_string())
            }
        );
        assert_eq!(
            balance(1, 3, 15),
            Balance {
                resulting: 17,
                hint: Some("bring in 2 more".to_string())
            }
        );
        assert_eq!(
            balance(2, 2, 15),
            Balance {
                resulting: 15,
                hint: None
            }
        );
    }

    fn note(id: i64, deck_id: i64, body: &str, created_at: i64) -> Note {
        Note {
            id,
            deck_id,
            deck_name: format!("Qdeck {deck_id}"),
            body: body.to_string(),
            created_at,
        }
    }

    // ---- GetArchetypeNotesTest --------------------------------------------

    #[test]
    fn splits_notes_into_this_deck_and_other_decks_newest_first() {
        let notes = notes_for(
            1,
            vec![
                note(1, 1, "Older note", 1_000),
                note(2, 1, "Newer note", 2_000),
                note(3, 2, "From the other deck", 1_500),
                note(4, 1, "Same second, higher id", 2_000),
            ],
        );
        let ids: Vec<i64> = notes.current.iter().map(|note| note.id).collect();
        assert_eq!(ids, [4, 2, 1]);
        assert_eq!(notes.other.len(), 1);
        assert_eq!(notes.other[0].body, "From the other deck");
        assert_eq!(notes.other[0].deck_name, "Qdeck 2");
    }

    #[test]
    fn returns_empty_groups_when_the_archetype_has_no_notes() {
        assert_eq!(notes_for(1, Vec::new()), ArchetypeNotes::default());
    }

    // ---- StoreNoteControllerTest (the body rule) --------------------------

    #[test]
    fn a_note_body_is_trimmed_and_between_one_and_two_thousand_characters() {
        assert_eq!(
            check_note_body("  Mull aggressively \n"),
            Ok("Mull aggressively".to_string())
        );
        assert_eq!(check_note_body(""), Err(NOTE_REQUIRED));
        assert_eq!(check_note_body("   "), Err(NOTE_REQUIRED));
        assert_eq!(check_note_body(&"a".repeat(2001)), Err(NOTE_TOO_LONG));
        // Characters, not bytes.
        assert_eq!(
            check_note_body(&"é".repeat(2000)).map(|body| body.chars().count()),
            Ok(2000)
        );
    }
}
