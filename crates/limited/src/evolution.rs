//! The story of one limited deck (0.x `BuildDeckEvolution`): what was in the
//! pool, which registered versions MTGO sent at each match start, and how the
//! board changed game to game.

use std::collections::{BTreeMap, BTreeSet};

use crate::cards::{CardFactsMap, colour_letters, is_basic};
use crate::pool::{RegisteredCard, RegisteredDeck, RunPool, in_capture_order};

/// The pool groups, in page order: key and label.
const GROUPS: [(&str, &str); 8] = [
    ("W", "White"),
    ("U", "Blue"),
    ("B", "Black"),
    ("R", "Red"),
    ("G", "Green"),
    ("M", "Multicolour"),
    ("C", "Colourless"),
    ("L", "Lands"),
];

/// Where a pool card ended up in a registered deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    /// In the main deck.
    Main,
    /// In the sideboard, or in an earlier registered deck.
    Side,
    /// Nothing is registered yet.
    Pool,
    /// Never registered.
    Cut,
}

impl CardStatus {
    /// 0.x's status string.
    pub fn as_str(self) -> &'static str {
        match self {
            CardStatus::Main => "main",
            CardStatus::Side => "side",
            CardStatus::Pool => "pool",
            CardStatus::Cut => "cut",
        }
    }

    /// Sort order within a pool group.
    fn rank(self) -> u8 {
        match self {
            CardStatus::Main => 0,
            CardStatus::Side => 1,
            CardStatus::Pool => 2,
            CardStatus::Cut => 3,
        }
    }
}

/// A card and a number of copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardQuantity {
    pub catalog_id: u32,
    pub quantity: u32,
}

/// What changed in one zone between two lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZoneDiff {
    pub added: Vec<CardQuantity>,
    pub removed: Vec<CardQuantity>,
}

/// One pool card in a pool group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCard {
    pub catalog_id: u32,
    pub quantity: u32,
    /// Copies from the booster added mid-run (sealed).
    pub added: u32,
    pub status: CardStatus,
    pub main_qty: u32,
    pub side_qty: u32,
}

/// The pool cards of one colour group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolGroup {
    /// `W`, `U`, `B`, `R`, `G`, `M`, `C` or `L`.
    pub key: String,
    pub label: String,
    /// Copies in the group.
    pub count: u32,
    pub cards: Vec<PoolCard>,
}

/// One registered version: consecutive identical registered decks folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredVersion {
    /// 1-based.
    pub index: u32,
    pub signature: String,
    pub captured_at_ms: i64,
    /// The matches it was registered for.
    pub match_tokens: Vec<String>,
    /// `Match N`, numbered over every match of the run (`Match ?` when the
    /// match is not one of them).
    pub match_labels: Vec<String>,
    /// Cards in the main deck.
    pub main: u32,
    /// Cards in the sideboard.
    pub side: u32,
    /// The main deck's colour letters in first-seen order (catalog id order).
    pub colors: String,
    /// Against the previous version; empty for the first.
    pub diff_main: ZoneDiff,
    pub diff_side: ZoneDiff,
    pub is_current: bool,
    /// Where every pool card sat in this build.
    pub pool_groups: Vec<PoolGroup>,
    pub main_cards: Vec<CardQuantity>,
    pub side_cards: Vec<CardQuantity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvolutionSummary {
    /// Non-basic pool copies.
    pub drafted: u32,
    /// Non-basic copies from the added booster.
    pub added: u32,
    /// Non-basic copies in the current main deck.
    pub main_spells: u32,
    /// Basic lands in the current main deck.
    pub basics: u32,
    /// Copies in the current sideboard.
    pub sideboard: u32,
    pub version_count: usize,
    pub first_registered_at_ms: Option<i64>,
    pub last_registered_at_ms: Option<i64>,
}

/// One game's board against the deck registered for its match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameChange {
    /// 1-based.
    pub number: u32,
    pub added: Vec<CardQuantity>,
    pub removed: Vec<CardQuantity>,
    /// `registered deck`, `no board data`, `no changes`, or `None` when
    /// there are changes.
    pub note: Option<String>,
}

/// Per completed match, how each game's board differed from its registered
/// deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchChanges {
    pub match_token: String,
    /// `Match N`, numbered over the completed matches.
    pub label: String,
    pub opponent_name: Option<String>,
    /// `W`, `L`, or `None`.
    pub result: Option<String>,
    pub games: Vec<GameChange>,
}

/// A completed match of the run, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedMatch {
    pub token: String,
    /// `Some(true)` won, `Some(false)` lost, `None` neither.
    pub won: Option<bool>,
    pub opponent_name: Option<String>,
    /// The local player's deck per game, in game order; `None` (or an empty
    /// list) when MTGO gave no board data for that game.
    pub game_decks: Vec<Option<Vec<RegisteredCard>>>,
}

/// Everything the Deck page needs about one run.
#[derive(Debug, Clone, Copy)]
pub struct EvolutionInput<'a> {
    /// `run_pool` for the run.
    pub pool: &'a RunPool,
    /// The run's registered decks.
    pub decks: &'a [RegisteredDeck],
    /// Every match of the run (finished or not) by start time: the numbering
    /// of the version labels.
    pub match_order: &'a [String],
    /// The run's completed matches by start time.
    pub completed: &'a [CompletedMatch],
    pub cards: &'a CardFactsMap,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeckEvolution {
    pub summary: EvolutionSummary,
    pub versions: Vec<RegisteredVersion>,
    /// The pool against the current registered deck.
    pub pool_groups: Vec<PoolGroup>,
    pub games: Vec<MatchChanges>,
}

/// The current deck: the last registered deck's zones, and every card any
/// registered deck listed.
struct CurrentZones {
    main: BTreeMap<u32, u32>,
    side: BTreeMap<u32, u32>,
    ever_seen: BTreeSet<u32>,
}

/// The Deck page (0.x `BuildDeckEvolution::run`).
pub fn deck_evolution(input: &EvolutionInput) -> DeckEvolution {
    let cards = input.cards;
    let decks = in_capture_order(input.decks);
    let current = current_zones(&decks);
    let versions = versions(input, &decks, &current.ever_seen);

    let non_basic = |counts: &BTreeMap<u32, u32>| -> u32 {
        counts
            .iter()
            .filter(|(id, _)| !is_basic(cards, **id))
            .map(|(_, quantity)| *quantity)
            .sum()
    };
    let basics: u32 = current
        .main
        .iter()
        .filter(|(id, _)| is_basic(cards, **id))
        .map(|(_, quantity)| *quantity)
        .sum();

    DeckEvolution {
        summary: EvolutionSummary {
            drafted: non_basic(&input.pool.pool),
            added: non_basic(&input.pool.added),
            main_spells: non_basic(&current.main),
            basics,
            sideboard: current.side.values().sum(),
            version_count: versions.len(),
            first_registered_at_ms: decks.first().map(|deck| deck.captured_at_ms),
            last_registered_at_ms: decks.last().map(|deck| deck.captured_at_ms),
        },
        versions,
        pool_groups: pool_groups(
            input.pool,
            &current.main,
            &current.side,
            &current.ever_seen,
            cards,
        ),
        games: games(input.completed, &decks),
    }
}

/// Where each pool card ended up in the current registered deck (0.x
/// `BuildDeckEvolution::poolStatuses`): the Deck page's pool statuses
/// without the versions and games.
pub fn pool_statuses(
    pool: &RunPool,
    decks: &[RegisteredDeck],
    cards: &CardFactsMap,
) -> BTreeMap<u32, CardStatus> {
    let decks = in_capture_order(decks);
    let current = current_zones(&decks);

    let mut statuses = BTreeMap::new();
    for group in pool_groups(
        pool,
        &current.main,
        &current.side,
        &current.ever_seen,
        cards,
    ) {
        for card in group.cards {
            statuses.insert(card.catalog_id, card.status);
        }
    }
    statuses
}

fn current_zones(decks: &[&RegisteredDeck]) -> CurrentZones {
    let current = decks.last();
    CurrentZones {
        main: current.map_or_else(BTreeMap::new, |deck| zone(&deck.cards, false)),
        side: current.map_or_else(BTreeMap::new, |deck| zone(&deck.cards, true)),
        ever_seen: decks
            .iter()
            .flat_map(|deck| deck.cards.iter().map(|card| card.catalog_id))
            .collect(),
    }
}

/// One zone of a card list as catalog id to quantity; zero counts read as
/// absent.
fn zone(cards: &[RegisteredCard], sideboard: bool) -> BTreeMap<u32, u32> {
    let mut zone = BTreeMap::new();
    for card in cards.iter().filter(|card| card.sideboard == sideboard) {
        *zone.entry(card.catalog_id).or_insert(0) += card.quantity;
    }
    zone.retain(|_, quantity| *quantity > 0);
    zone
}

/// Copies added and removed from `before` to `after`: ids of `before` first,
/// then ids only in `after`, each ascending.
fn diff(before: &BTreeMap<u32, u32>, after: &BTreeMap<u32, u32>) -> ZoneDiff {
    let mut ids: Vec<u32> = before.keys().copied().collect();
    ids.extend(after.keys().filter(|id| !before.contains_key(id)));

    let mut result = ZoneDiff::default();
    for catalog_id in ids {
        let was = i64::from(before.get(&catalog_id).copied().unwrap_or(0));
        let now = i64::from(after.get(&catalog_id).copied().unwrap_or(0));
        let delta = now - was;
        if delta > 0 {
            result.added.push(CardQuantity {
                catalog_id,
                quantity: delta as u32,
            });
        } else if delta < 0 {
            result.removed.push(CardQuantity {
                catalog_id,
                quantity: (-delta) as u32,
            });
        }
    }
    result
}

/// Registered decks in capture order, consecutive identical ones folded into
/// one version that lists every match it covered.
fn versions(
    input: &EvolutionInput,
    decks: &[&RegisteredDeck],
    ever_seen: &BTreeSet<u32>,
) -> Vec<RegisteredVersion> {
    let mut out: Vec<RegisteredVersion> = Vec::new();
    let mut previous_main = BTreeMap::new();
    let mut previous_side = BTreeMap::new();

    for deck in decks {
        let main = zone(&deck.cards, false);
        let side = zone(&deck.cards, true);

        if let Some(last) = out.last_mut()
            && main == previous_main
            && side == previous_side
        {
            last.match_tokens.extend(deck.match_token.clone());
            continue;
        }

        let is_first = out.is_empty();
        let mut colors = String::new();
        for &catalog_id in main.keys() {
            for letter in colour_letters(input.cards, catalog_id) {
                if !colors.contains(letter) {
                    colors.push(letter);
                }
            }
        }

        out.push(RegisteredVersion {
            index: out.len() as u32 + 1,
            signature: deck.signature.clone(),
            captured_at_ms: deck.captured_at_ms,
            match_tokens: deck.match_token.iter().cloned().collect(),
            match_labels: Vec::new(),
            main: main.values().sum(),
            side: side.values().sum(),
            colors,
            diff_main: if is_first {
                ZoneDiff::default()
            } else {
                diff(&previous_main, &main)
            },
            diff_side: if is_first {
                ZoneDiff::default()
            } else {
                diff(&previous_side, &side)
            },
            is_current: false,
            pool_groups: pool_groups(input.pool, &main, &side, ever_seen, input.cards),
            main_cards: card_list(&main),
            side_cards: card_list(&side),
        });
        previous_main = main;
        previous_side = side;
    }

    if let Some(last) = out.last_mut() {
        last.is_current = true;
    }
    for version in &mut out {
        version.match_tokens.retain(|token| !token.is_empty());
        version.match_labels = version
            .match_tokens
            .iter()
            .map(|token| match_label(input.match_order, token))
            .collect();
    }
    out
}

/// `Match N` by the match's place in the run, `Match ?` when it has none.
fn match_label(match_order: &[String], token: &str) -> String {
    match match_order.iter().position(|other| other == token) {
        Some(index) => format!("Match {}", index + 1),
        None => "Match ?".to_string(),
    }
}

/// The pool grouped by colour, each card tagged with where it sits in the
/// given deck. Basics are left out (never drafted). With nothing registered
/// every card reads as `pool`.
fn pool_groups(
    pool: &RunPool,
    main: &BTreeMap<u32, u32>,
    side: &BTreeMap<u32, u32>,
    ever_seen: &BTreeSet<u32>,
    cards: &CardFactsMap,
) -> Vec<PoolGroup> {
    let mut groups: Vec<PoolGroup> = GROUPS
        .iter()
        .map(|(key, label)| PoolGroup {
            key: key.to_string(),
            label: label.to_string(),
            count: 0,
            cards: Vec::new(),
        })
        .collect();

    for (&catalog_id, &quantity) in &pool.pool {
        if is_basic(cards, catalog_id) {
            continue;
        }
        let key = group_key(cards, catalog_id);
        let main_qty = main.get(&catalog_id).copied().unwrap_or(0);
        let side_qty = side.get(&catalog_id).copied().unwrap_or(0);
        let status = if ever_seen.is_empty() {
            CardStatus::Pool
        } else if main_qty > 0 {
            CardStatus::Main
        } else if side_qty > 0 || ever_seen.contains(&catalog_id) {
            CardStatus::Side
        } else {
            CardStatus::Cut
        };

        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.cards.push(PoolCard {
                catalog_id,
                quantity,
                added: pool.added.get(&catalog_id).copied().unwrap_or(0),
                status,
                main_qty,
                side_qty,
            });
            group.count += quantity;
        }
    }

    for group in &mut groups {
        group
            .cards
            .sort_by_key(|card| (card.status.rank(), card.catalog_id));
    }
    groups.retain(|group| !group.cards.is_empty());
    groups
}

/// `L` for any land, `M` for two or more colours, else the one colour, else
/// `C` (unknown cards too).
fn group_key(cards: &CardFactsMap, catalog_id: u32) -> String {
    let is_land = cards
        .get(&catalog_id)
        .and_then(|card| card.card_type.as_deref())
        .is_some_and(|card_type| card_type.contains("Land"));
    if is_land {
        return "L".to_string();
    }
    let letters = colour_letters(cards, catalog_id);
    if letters.len() > 1 {
        return "M".to_string();
    }
    letters
        .first()
        .map_or_else(|| "C".to_string(), |letter| letter.to_string())
}

fn card_list(zone: &BTreeMap<u32, u32>) -> Vec<CardQuantity> {
    zone.iter()
        .map(|(&catalog_id, &quantity)| CardQuantity {
            catalog_id,
            quantity,
        })
        .collect()
}

/// Per completed match, each game's board against the deck registered for
/// that match. Game 1 is the registered deck by definition.
fn games(completed: &[CompletedMatch], decks: &[&RegisteredDeck]) -> Vec<MatchChanges> {
    completed
        .iter()
        .enumerate()
        .map(|(index, played)| {
            // The last registered deck for the match, as 0.x's keyBy.
            let registered_main = decks
                .iter()
                .rev()
                .find(|deck| deck.match_token.as_deref() == Some(played.token.as_str()))
                .map(|deck| zone(&deck.cards, false));

            let games = played
                .game_decks
                .iter()
                .enumerate()
                .map(|(game_index, deck)| {
                    game_change(
                        game_index as u32 + 1,
                        registered_main.as_ref(),
                        deck.as_deref(),
                    )
                })
                .collect();

            MatchChanges {
                match_token: played.token.clone(),
                label: format!("Match {}", index + 1),
                opponent_name: played.opponent_name.clone(),
                result: played
                    .won
                    .map(|won| if won { "W" } else { "L" }.to_string()),
                games,
            }
        })
        .collect()
}

fn game_change(
    number: u32,
    registered_main: Option<&BTreeMap<u32, u32>>,
    deck: Option<&[RegisteredCard]>,
) -> GameChange {
    let note_only = |note: &str| GameChange {
        number,
        added: Vec::new(),
        removed: Vec::new(),
        note: Some(note.to_string()),
    };

    let Some(registered_main) = registered_main else {
        return note_only("no board data");
    };
    if number == 1 {
        return note_only("registered deck");
    }
    let Some(deck) = deck.filter(|deck| !deck.is_empty()) else {
        return note_only("no board data");
    };

    let changes = diff(registered_main, &zone(deck, false));
    let note = if changes.added.is_empty() && changes.removed.is_empty() {
        Some("no changes".to_string())
    } else {
        None
    };
    GameChange {
        number,
        added: changes.added,
        removed: changes.removed,
        note,
    }
}
