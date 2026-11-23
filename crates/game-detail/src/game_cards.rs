//! One game's card rows (target schema section 6, `game_cards`): per side and
//! card, the deck as played, the sideboarding against game 1, the opening
//! hand, the bottomed cards and the starting hand, the turn 1 draw, the
//! cards seen, the pregame actions and the game log counters; the hand sizes;
//! the casts by an alternative method; the hands sent back.
//!
//! Terms (owner, 2026-10-08): the opening hand is the hand drawn and kept,
//! before bottoming (normally 7 cards); bottomed cards go under the library
//! (London mulligan); the starting hand is the opening hand minus the
//! bottomed cards.
//!
//! Pure: the caller passes the game's frames, its deck as played, the game
//! log cards and pregame actions of that game, and card lookups.

use std::collections::BTreeMap;

use decks::DeckCard;

use crate::deck::{local_deck, merge_cards_seen, opponent_cards};
use crate::frame::Frame;
use crate::log_cards::{Counter, GameLogCard, PregameAction, PregameKind};
use crate::opening_hand::opening_hand;
use crate::reveals::{LogCard, RevealedCard, merge_log_only_cards};
use crate::sideboard::{ChangeKind, sideboard_changes};
use crate::zones::{BATTLEFIELD, EXILE, GRAVEYARD, HAND, count_zones};

/// The version of the stored game card rows. A higher version makes the
/// tracker sync every log match's games again.
pub const GAME_CARDS_VERSION: i64 = 3;

/// The size of a full opening hand.
const FULL_HAND: usize = 7;

/// The zones where the local player sees their own cards (0.x
/// `ComputeCardGameStats::LOCAL_VISIBLE_ZONES`).
const LOCAL_VISIBLE_ZONES: [&str; 5] = ["Hand", "Battlefield", "Graveyard", "Exile", "Stack"];

/// Whose card a row is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    #[default]
    Local,
    Opponent,
}

impl Side {
    /// The stored text (`game_cards.side`).
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Local => "local",
            Side::Opponent => "opponent",
        }
    }

    /// A stored side, `None` for any other text.
    pub fn parse(text: &str) -> Option<Side> {
        match text {
            "local" => Some(Side::Local),
            "opponent" => Some(Side::Opponent),
            _ => None,
        }
    }
}

/// One card in one game for one side (a `game_cards` row).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameCardRow {
    pub side: Side,
    pub catalog_id: u32,
    /// The deck as played that game (local side).
    pub main_qty: u32,
    pub side_qty: u32,
    /// Copies brought in or taken out against game 1.
    pub sided_in: u32,
    pub sided_out: u32,
    /// Copies in the opening hand (before bottoming).
    pub opening_hand: u32,
    pub bottomed: u32,
    /// `opening_hand - bottomed`.
    pub starting_hand: u32,
    /// 1 for the card drawn on turn 1 on the draw.
    pub first_draw: u32,
    /// Local: copies seen in visible zones. Opponent: copies revealed.
    pub seen: u32,
    pub pregame_revealed: u32,
    pub pregame_played: u32,
    /// Log counters; `played` and `activated` are `None` without a game log.
    pub cast: u32,
    pub played: Option<u32>,
    pub activated: Option<u32>,
    pub cast_turn: Option<u32>,
    /// Zone counters (06 computes them; a 0.x import brings 0.x's).
    pub discarded: u32,
    pub hand_seen: u32,
    pub graveyard_seen: u32,
    pub exile_seen: u32,
    pub battlefield_seen: u32,
    pub has_zone_data: bool,
    /// A local row of an imported postboard game whose deck is the
    /// registered maindeck plus the sideboard cards the game log showed
    /// (`apply_imported_rule`): "sided in" counts only cards that were seen.
    pub inferred: bool,
}

/// Casts of one card by an alternative method (a `game_card_casts` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameCardCast {
    pub side: Side,
    pub catalog_id: u32,
    /// 0.x's column name (`kicked`, `free_cast`, ...).
    pub method: String,
    pub count: u32,
}

/// Everything stored per game: the `games` hand sizes and the card rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameCards {
    pub opening_hand_size: Option<u32>,
    pub starting_hand_size: Option<u32>,
    pub opp_opening_hand_size: Option<u32>,
    pub opp_starting_hand_size: Option<u32>,
    /// Sorted by side, then catalog id; one row per side and card.
    pub cards: Vec<GameCardRow>,
    /// Sorted by side, catalog id, then method.
    pub casts: Vec<GameCardCast>,
    /// The hands sent back: `(hand, catalog id, quantity)`, sorted.
    pub mulligans: Vec<(u32, u32, u32)>,
}

impl GameCards {
    /// The row of one side's card, if any.
    pub fn row(&self, side: Side, catalog_id: u32) -> Option<&GameCardRow> {
        self.cards
            .iter()
            .find(|row| row.side == side && row.catalog_id == catalog_id)
    }

    /// Whether nothing at all is known about the game's cards.
    pub fn is_empty(&self) -> bool {
        self.opening_hand_size.is_none()
            && self.starting_hand_size.is_none()
            && self.opp_opening_hand_size.is_none()
            && self.opp_starting_hand_size.is_none()
            && self.cards.is_empty()
            && self.casts.is_empty()
            && self.mulligans.is_empty()
    }
}

/// What one game's card rows are built from.
pub struct GameCardsInput<'a> {
    /// The game's state frames, in log order.
    pub frames: &'a [Frame],
    /// The local player's name (picks the local seat in the frames).
    pub local_name: &'a str,
    /// Whether the local player was on the play; `None` counts as the draw,
    /// as 0.x's cast to false did.
    pub on_play: Option<bool>,
    /// The local deck as played that game, main and side rows (see
    /// `played_deck`); empty when unknown.
    pub local_deck: &'a [DeckCard],
    /// Game 1's deck as played, for the sideboarding counts; empty for game 1.
    pub game_one_deck: &'a [DeckCard],
    /// The opponent's cards a 0.x import recorded `(catalog id, quantity)`;
    /// empty for a live game.
    pub recorded_opponent_cards: &'a [(u32, u32)],
    /// The opponent's cards in the `.dat` game log, named (the log's name
    /// when the card is unknown), merged into the reveals.
    pub reveal_log_cards: &'a [LogCard],
    /// The game has a game log (`.dat` or MetaMessages): the log counters
    /// are 0 rather than unknown for cards it never names.
    pub has_log: bool,
    /// This game's log cards per side (counters).
    pub local_log_cards: &'a [GameLogCard],
    pub opponent_log_cards: &'a [GameLogCard],
    /// This game's pregame actions per side.
    pub local_pregame: &'a [PregameAction],
    pub opponent_pregame: &'a [PregameAction],
    /// Starting hand sizes the log states ("begins the game with N cards in
    /// hand"), `(local, opponent)`; used when the frames show no hand.
    pub log_starting_hands: (Option<u32>, Option<u32>),
    /// A card's name, `Card <id>` when unknown (for the reveals merge).
    pub name_of: &'a dyn Fn(u32) -> String,
    /// A card's oracle id (sideboarding compares cards by oracle).
    pub oracle_of: &'a dyn Fn(u32) -> Option<String>,
}

/// The local deck as played from a game's first `deck_used` list and its
/// first frame (`local_deck`); empty without frames, a local seat or a list.
pub fn played_deck(frames: &[Frame], local_name: &str, deck_used: &[DeckCard]) -> Vec<DeckCard> {
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    match seats(first, local_name) {
        Some((local, _)) if !deck_used.is_empty() => local_deck(deck_used, first, local),
        _ => Vec::new(),
    }
}

/// The local and opponent seat ids of a game: the local player is the frame
/// player with the local name.
pub fn seats(first: &Frame, local_name: &str) -> Option<(i64, i64)> {
    let local = first
        .players
        .iter()
        .find(|player| player.name == local_name)?;
    let opponent = first.players.iter().find(|player| player.id != local.id)?;
    Some((local.id, opponent.id))
}

/// One game's card rows (spec section 6, "Writers"):
/// - the hand from the frames (`opening_hand`): the opening hand with the
///   bottomed cards, and on the draw the turn 1 draw split out of it when
///   the hand holds more than `7 - mulligans` cards and its last card was not
///   bottomed (0.x's "T1 draw" pill); the hands sent back;
/// - the hand sizes from the frames (the opponent's from its library count),
///   else the log's starting sizes, with opening = starting;
/// - the local deck as played, and the copies brought in or taken out
///   against game 1 (`sideboard_changes`);
/// - the local cards seen in visible zones, capped at the copies played when
///   the deck is known, and the opponent's revealed cards: the last frame's,
///   merged with a 0.x record, then the `.dat` log's log-only cards
///   (`merge_log_only_cards`);
/// - the log counters, the casts by an alternative method and the pregame
///   actions of this game.
pub fn game_card_rows(input: &GameCardsInput) -> GameCards {
    let mut rows: BTreeMap<(Side, u32), GameCardRow> = BTreeMap::new();
    let mut cards = GameCards::default();
    let seat_ids = input
        .frames
        .first()
        .and_then(|first| seats(first, input.local_name));

    // The hand. Cards seen and the zone counters count from the frame that
    // holds the kept hand on, so a hand sent back by a mulligan is not seen.
    let mut has_hand = false;
    let mut kept_frames = input.frames;
    if let Some((local, opponent)) = seat_ids {
        let hand = opening_hand(input.frames, local, opponent);
        kept_frames = &input.frames[hand.kept_from..];
        let mut shown = hand.shown_hand();
        if !shown.is_empty() {
            has_hand = true;
            let expected = FULL_HAND.saturating_sub(hand.local_mulligans as usize);
            let on_the_draw = input.on_play != Some(true);
            let last_bottomed = shown.last().is_some_and(|card| card.bottomed);
            if on_the_draw
                && shown.len() > expected
                && !last_bottomed
                && let Some(drawn) = shown.pop()
            {
                row(&mut rows, Side::Local, drawn.catalog_id).first_draw += 1;
            }
            let mut bottomed_count = 0;
            for card in &shown {
                let entry = row(&mut rows, Side::Local, card.catalog_id);
                entry.opening_hand += 1;
                if card.bottomed {
                    entry.bottomed += 1;
                    bottomed_count += 1;
                }
            }
            for entry in rows.values_mut() {
                entry.starting_hand = entry.opening_hand - entry.bottomed;
            }
            cards.opening_hand_size = Some(shown.len() as u32);
            cards.starting_hand_size = Some(shown.len() as u32 - bottomed_count);
            cards.opp_opening_hand_size = Some(FULL_HAND as u32);
            cards.opp_starting_hand_size =
                Some((FULL_HAND as u32).saturating_sub(hand.opponent_mulligans));
            for (index, mulliganed) in hand.mulliganed_hands.iter().enumerate() {
                let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
                for card in mulliganed {
                    *counts.entry(card.catalog_id).or_insert(0) += 1;
                }
                for (catalog_id, quantity) in counts {
                    cards
                        .mulligans
                        .push((index as u32 + 1, catalog_id, quantity));
                }
            }
        }
    }
    if !has_hand {
        let (local, opponent) = input.log_starting_hands;
        cards.opening_hand_size = local;
        cards.starting_hand_size = local;
        cards.opp_opening_hand_size = opponent;
        cards.opp_starting_hand_size = opponent;
    }

    // The deck as played and the sideboarding against game 1.
    for card in input.local_deck {
        let entry = row(&mut rows, Side::Local, card.catalog_id);
        if card.sideboard {
            entry.side_qty = entry.side_qty.saturating_add(card.quantity);
        } else {
            entry.main_qty = entry.main_qty.saturating_add(card.quantity);
        }
    }
    for change in sideboard_changes(input.local_deck, input.game_one_deck, input.oracle_of) {
        let entry = row(&mut rows, Side::Local, change.catalog_id);
        match change.kind {
            ChangeKind::In => entry.sided_in = entry.sided_in.saturating_add(change.quantity),
            ChangeKind::Out => entry.sided_out = entry.sided_out.saturating_add(change.quantity),
        }
    }

    // Cards seen.
    if let Some((local, _)) = seat_ids {
        for (catalog_id, count) in local_cards_seen(kept_frames, local) {
            let entry = row(&mut rows, Side::Local, catalog_id);
            let played = entry.main_qty.saturating_add(entry.side_qty);
            entry.seen = if input.local_deck.is_empty() {
                count
            } else {
                count.min(played)
            };
        }
    }
    let mut seen: Vec<(u32, u32)> = Vec::new();
    if let (Some((_, opponent)), Some(last)) = (seat_ids, input.frames.last()) {
        seen = opponent_cards(last, opponent);
    }
    if !input.recorded_opponent_cards.is_empty() {
        seen = merge_cards_seen(input.recorded_opponent_cards, &seen);
    }
    let mut named = Vec::new();
    for (catalog_id, quantity) in seen {
        named.push(RevealedCard {
            catalog_id,
            name: (input.name_of)(catalog_id),
            quantity,
        });
    }
    for card in merge_log_only_cards(named, input.reveal_log_cards) {
        let entry = row(&mut rows, Side::Opponent, card.catalog_id);
        entry.seen = entry.seen.saturating_add(card.quantity);
    }

    // The game log.
    for (side, log_cards) in [
        (Side::Local, input.local_log_cards),
        (Side::Opponent, input.opponent_log_cards),
    ] {
        for card in log_cards {
            let entry = row(&mut rows, side, card.mtgo_id);
            entry.cast += card.count(Counter::Cast);
            entry.played = Some(entry.played.unwrap_or(0) + card.count(Counter::Played));
            entry.activated = Some(entry.activated.unwrap_or(0) + card.count(Counter::Activated));
            if entry.cast_turn.is_none() {
                entry.cast_turn = card.cast_turn;
            }
            for counter in Counter::ALL {
                if matches!(
                    counter,
                    Counter::Cast | Counter::Played | Counter::Activated
                ) {
                    continue;
                }
                let count = card.count(counter);
                if count > 0 {
                    add_cast(&mut cards.casts, side, card.mtgo_id, counter, count);
                }
            }
        }
    }
    for (side, actions) in [
        (Side::Local, input.local_pregame),
        (Side::Opponent, input.opponent_pregame),
    ] {
        for action in actions {
            let entry = row(&mut rows, side, action.mtgo_id);
            match action.kind {
                PregameKind::Revealed => entry.pregame_revealed += 1,
                PregameKind::Played => entry.pregame_played += 1,
            }
        }
    }
    // The zone counters of the local cards (0.x `zoneColumns`).
    if let Some((local, _)) = seat_ids {
        fill_zone_counters(&mut rows, kept_frames, local, !input.local_deck.is_empty());
    }
    // Zone data only when the frames had the local seat to count zones for.
    if seat_ids.is_some() {
        for entry in rows.values_mut() {
            if entry.side == Side::Local {
                entry.has_zone_data = true;
            }
        }
    }

    if input.has_log {
        for entry in rows.values_mut() {
            entry.played = Some(entry.played.unwrap_or(0));
            entry.activated = Some(entry.activated.unwrap_or(0));
        }
    } else {
        for entry in rows.values_mut() {
            entry.played = None;
            entry.activated = None;
        }
    }

    cards.cards = rows.into_values().collect();
    cards
        .casts
        .sort_by(|a, b| (a.side, a.catalog_id, &a.method).cmp(&(b.side, b.catalog_id, &b.method)));
    cards.mulligans.sort_unstable();
    cards
}

/// The row of one side's card, made when missing.
fn row(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    side: Side,
    catalog_id: u32,
) -> &mut GameCardRow {
    rows.entry((side, catalog_id))
        .or_insert_with(|| GameCardRow {
            side,
            catalog_id,
            ..GameCardRow::default()
        })
}

/// Add `count` casts of a card by one method.
fn add_cast(
    casts: &mut Vec<GameCardCast>,
    side: Side,
    catalog_id: u32,
    counter: Counter,
    count: u32,
) {
    let method = counter.field_name();
    match casts
        .iter_mut()
        .find(|cast| cast.side == side && cast.catalog_id == catalog_id && cast.method == method)
    {
        Some(cast) => cast.count += count,
        None => casts.push(GameCardCast {
            side,
            catalog_id,
            method: method.to_string(),
            count,
        }),
    }
}

/// The row key of a frame card: its catalog id, as `seen` keys it (0 is no
/// card).
fn frame_key(catalog_id: u32) -> Option<u32> {
    (catalog_id != 0).then_some(catalog_id)
}

/// Fill the local rows' zone counters from the frames: each count capped at
/// the row's copies when the deck is known (as `seen` is), and no discards
/// for a card the log shows cast (a cast spell reaches the graveyard from the
/// hand like a discarded one). A card only a zone shows gets a row only when
/// it has a count left after the cap.
fn fill_zone_counters(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    frames: &[Frame],
    local: i64,
    deck_known: bool,
) {
    let counts = count_zones(frames, local, &frame_key);
    let mut keys: Vec<u32> = Vec::new();
    for by_key in &counts.zones {
        keys.extend(by_key.keys());
    }
    keys.extend(counts.discarded.keys());
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        let existing = rows.get(&(Side::Local, key));
        let copies = existing.map_or(0, |entry| entry.main_qty.saturating_add(entry.side_qty));
        let cap = |count: u32| if deck_known { count.min(copies) } else { count };
        let was_cast = existing.is_some_and(|entry| entry.cast > 0);
        let hand = cap(counts.count(HAND, key));
        let graveyard = cap(counts.count(GRAVEYARD, key));
        let exile = cap(counts.count(EXILE, key));
        let battlefield = cap(counts.count(BATTLEFIELD, key));
        let discarded = if was_cast {
            0
        } else {
            cap(counts.discards(key))
        };
        if existing.is_none() && hand + graveyard + exile + battlefield + discarded == 0 {
            continue;
        }
        let entry = row(rows, Side::Local, key);
        entry.hand_seen = hand;
        entry.graveyard_seen = graveyard;
        entry.exile_seen = exile;
        entry.battlefield_seen = battlefield;
        entry.discarded = discarded;
    }
}

/// How many of the local player's own cards (distinct instances) each
/// printing showed in a visible zone over the game.
fn local_cards_seen(frames: &[Frame], local: i64) -> BTreeMap<u32, u32> {
    let mut instances: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    for frame in frames {
        for card in &frame.cards {
            if card.owner != local || !LOCAL_VISIBLE_ZONES.contains(&card.zone.as_str()) {
                continue;
            }
            let ids = instances.entry(card.catalog_id).or_default();
            if !ids.contains(&card.id) {
                ids.push(card.id);
            }
        }
    }
    instances
        .into_iter()
        .map(|(catalog_id, ids)| (catalog_id, ids.len() as u32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameCard, FramePlayer};

    fn players(local_hand: i64, opponent_hand: i64, opponent_library: i64) -> Vec<FramePlayer> {
        vec![
            FramePlayer {
                id: 1,
                name: "Qlocal".to_string(),
                library_count: 53,
                hand_count: local_hand,
                life: 20,
            },
            FramePlayer {
                id: 2,
                name: "Qopponent".to_string(),
                library_count: opponent_library,
                hand_count: opponent_hand,
                life: 20,
            },
        ]
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

    /// The opponent holds 7 cards, except before the draw (library 60).
    fn frame(local_hand: i64, opponent_library: i64, cards: Vec<FrameCard>) -> Frame {
        let opponent_hand = if opponent_library == 60 { 0 } else { 7 };
        Frame {
            players: players(local_hand, opponent_hand, opponent_library),
            cards,
        }
    }

    fn hand(cards: &[(i64, u32)]) -> Vec<FrameCard> {
        cards
            .iter()
            .map(|&(id, catalog_id)| card(id, catalog_id, 1, "Hand"))
            .collect()
    }

    fn name_of(id: u32) -> String {
        format!("Card {id}")
    }

    fn no_oracle(_: u32) -> Option<String> {
        None
    }

    fn input<'a>(frames: &'a [Frame], on_play: Option<bool>) -> GameCardsInput<'a> {
        GameCardsInput {
            frames,
            local_name: "Qlocal",
            on_play,
            local_deck: &[],
            game_one_deck: &[],
            recorded_opponent_cards: &[],
            reveal_log_cards: &[],
            has_log: false,
            local_log_cards: &[],
            opponent_log_cards: &[],
            local_pregame: &[],
            opponent_pregame: &[],
            log_starting_hands: (None, None),
            name_of: &name_of,
            oracle_of: &no_oracle,
        }
    }

    /// A mulligan (hand of 1xx replaced by 2xx), one bottomed card (207),
    /// with `draw` the turn 1 draw (300, a second copy of 2001), and a land
    /// played.
    fn mulligan_frames(draw: bool) -> Vec<Frame> {
        let first: Vec<(i64, u32)> = (1..=7).map(|i| (100 + i, 1000 + i as u32)).collect();
        let second: Vec<(i64, u32)> = (1..=7).map(|i| (200 + i, 2000 + i as u32)).collect();
        let bottomed: Vec<(i64, u32)> = second
            .iter()
            .copied()
            .filter(|(id, _)| *id != 207)
            .collect();
        let mut held = bottomed.clone();
        if draw {
            held.push((300, 2001));
        }
        let mut played: Vec<FrameCard> = hand(&held[1..]);
        played.push(card(201, 2001, 1, "Battlefield"));
        played.push(card(900, 9001, 2, "Battlefield"));
        let mut frames = vec![
            frame(0, 60, Vec::new()),
            frame(7, 53, hand(&first)),
            frame(7, 53, hand(&second)),
            frame(6, 54, hand(&bottomed)),
        ];
        if draw {
            frames.push(frame(7, 54, hand(&held)));
        }
        frames.push(frame(6, 54, played));
        frames
    }

    /// S6-2: frames without the local player's seat count no zones, so the
    /// local rows do not claim zone data.
    #[test]
    fn frames_without_the_local_seat_give_no_zone_data() {
        let frames = vec![frame(7, 53, hand(&[(1, 1001)]))];
        let deck = [DeckCard {
            catalog_id: 1001,
            quantity: 4,
            sideboard: false,
        }];
        let with_seat = game_card_rows(&GameCardsInput {
            local_deck: &deck,
            ..input(&frames, Some(true))
        });
        assert!(with_seat.row(Side::Local, 1001).expect("row").has_zone_data);
        let without_seat = game_card_rows(&GameCardsInput {
            local_name: "Qnobody",
            local_deck: &deck,
            ..input(&frames, Some(true))
        });
        let row = without_seat.row(Side::Local, 1001).expect("row");
        assert_eq!(row.main_qty, 4);
        assert!(!row.has_zone_data);
    }

    #[test]
    fn huge_deck_quantities_saturate_instead_of_overflowing() {
        let frames = vec![frame(7, 53, hand(&[(1, 1001)]))];
        let row_of = |sideboard| DeckCard {
            catalog_id: 1001,
            quantity: u32::MAX,
            sideboard,
        };
        let deck = [row_of(false), row_of(false), row_of(true)];
        let cards = game_card_rows(&GameCardsInput {
            local_deck: &deck,
            game_one_deck: &[row_of(false)],
            recorded_opponent_cards: &[(1001, u32::MAX), (1001, u32::MAX)],
            ..input(&frames, Some(true))
        });
        let row = cards.row(Side::Local, 1001).expect("row");
        assert_eq!((row.main_qty, row.side_qty), (u32::MAX, u32::MAX));
    }

    #[test]
    fn starting_hand_is_opening_hand_minus_bottomed() {
        let frames = mulligan_frames(false);
        let cards = game_card_rows(&input(&frames, Some(true)));

        assert_eq!(cards.opening_hand_size, Some(7));
        assert_eq!(cards.starting_hand_size, Some(6));
        for row in &cards.cards {
            assert_eq!(
                row.starting_hand,
                row.opening_hand - row.bottomed,
                "{row:?}"
            );
        }
        let bottomed = cards.row(Side::Local, 2007).expect("the bottomed card");
        assert_eq!((bottomed.opening_hand, bottomed.bottomed), (1, 1));
        assert_eq!(bottomed.starting_hand, 0);
        let played = cards.row(Side::Local, 2001).expect("the land");
        assert_eq!((played.opening_hand, played.first_draw), (1, 0));
        assert_eq!(
            cards.mulligans,
            (1..=7).map(|i| (1, 1000 + i, 1)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cards_in_a_hand_sent_back_are_not_seen() {
        let frames = mulligan_frames(false);
        let deck: Vec<DeckCard> = (1..=7)
            .flat_map(|i| [1000 + i, 2000 + i])
            .map(|catalog_id| DeckCard {
                catalog_id,
                quantity: 4,
                sideboard: false,
            })
            .collect();
        let cards = game_card_rows(&GameCardsInput {
            local_deck: &deck,
            ..input(&frames, Some(true))
        });

        let sent_back = cards.row(Side::Local, 1001).expect("a deck card");
        assert_eq!((sent_back.seen, sent_back.hand_seen), (0, 0));
        let kept = cards.row(Side::Local, 2002).expect("a deck card");
        assert_eq!((kept.seen, kept.hand_seen), (1, 1));
        let bottomed = cards.row(Side::Local, 2007).expect("the bottomed card");
        assert_eq!((bottomed.seen, bottomed.hand_seen), (1, 1));
    }

    #[test]
    fn first_draw_marks_the_turn_one_draw_on_the_draw() {
        let frames = mulligan_frames(true);
        for on_play in [Some(false), None] {
            let cards = game_card_rows(&input(&frames, on_play));
            let drawn = cards.row(Side::Local, 2001).expect("the drawn card");
            assert_eq!((drawn.opening_hand, drawn.first_draw), (1, 1));
            assert_eq!(cards.opening_hand_size, Some(7));
            assert_eq!(cards.starting_hand_size, Some(6));
            let firsts: u32 = cards.cards.iter().map(|row| row.first_draw).sum();
            assert_eq!(firsts, 1);
        }
        // On the play an eighth card is not a turn 1 draw: it stays in the
        // opening hand, as 0.x showed it.
        let cards = game_card_rows(&input(&frames, Some(true)));
        let twice = cards.row(Side::Local, 2001).expect("two copies");
        assert_eq!((twice.opening_hand, twice.first_draw), (2, 0));
        assert_eq!(cards.opening_hand_size, Some(8));
    }

    #[test]
    fn opponent_hand_sizes_come_from_the_library_counts() {
        let frames = mulligan_frames(false);
        let cards = game_card_rows(&input(&frames, Some(true)));
        // The opponent's library went from 53 to 54: one card bottomed.
        assert_eq!(cards.opp_opening_hand_size, Some(7));
        assert_eq!(cards.opp_starting_hand_size, Some(6));
        let seen = cards.row(Side::Opponent, 9001).expect("seen");
        assert_eq!(seen.seen, 1);
    }

    #[test]
    fn without_a_hand_the_logs_starting_sizes_are_used() {
        let mut game = input(&[], Some(true));
        game.log_starting_hands = (Some(6), Some(7));
        let cards = game_card_rows(&game);
        assert_eq!(cards.opening_hand_size, Some(6));
        assert_eq!(cards.starting_hand_size, Some(6));
        assert_eq!(cards.opp_opening_hand_size, Some(7));
        assert_eq!(cards.opp_starting_hand_size, Some(7));
        assert!(cards.cards.is_empty());
    }

    fn deck(rows: &[(u32, u32, bool)]) -> Vec<DeckCard> {
        rows.iter()
            .map(|&(catalog_id, quantity, sideboard)| DeckCard {
                catalog_id,
                quantity,
                sideboard,
            })
            .collect()
    }

    #[test]
    fn deck_counts_and_sideboarding_against_game_one() {
        let game_one = deck(&[(1, 4, false), (2, 2, false), (3, 3, true)]);
        let game_two = deck(&[(1, 4, false), (3, 2, false), (3, 1, true), (2, 2, true)]);
        let mut game = input(&[], Some(true));
        game.local_deck = &game_two;
        game.game_one_deck = &game_one;
        let cards = game_card_rows(&game);
        let row = |id| cards.row(Side::Local, id).expect("row").clone();
        assert_eq!((row(1).main_qty, row(1).side_qty), (4, 0));
        assert_eq!(
            (row(2).main_qty, row(2).side_qty, row(2).sided_out),
            (0, 2, 2)
        );
        assert_eq!(
            (row(3).main_qty, row(3).side_qty, row(3).sided_in),
            (2, 1, 2)
        );
    }

    fn log_card(mtgo_id: u32, counts: &[(Counter, u32)], cast_turn: Option<u32>) -> GameLogCard {
        let mut card = GameLogCard {
            mtgo_id,
            name: format!("Q{mtgo_id}"),
            cast_turn,
            counts: [0; crate::log_cards::COUNTER_COUNT],
            copies_seen: 0,
        };
        for (counter, count) in counts {
            card.counts[Counter::ALL
                .iter()
                .position(|c| c == counter)
                .expect("counter")] = *count;
        }
        card
    }

    #[test]
    fn log_counters_and_alternative_casts_fill_their_columns() {
        let local = vec![log_card(
            10,
            &[
                (Counter::Cast, 2),
                (Counter::Kicked, 1),
                (Counter::Activated, 3),
            ],
            Some(2),
        )];
        let opponent = vec![log_card(20, &[(Counter::Played, 1)], None)];
        let pregame = vec![PregameAction {
            mtgo_id: 20,
            kind: PregameKind::Played,
        }];
        let mut game = input(&[], Some(true));
        game.has_log = true;
        game.local_log_cards = &local;
        game.opponent_log_cards = &opponent;
        game.opponent_pregame = &pregame;
        let cards = game_card_rows(&game);
        let mine = cards.row(Side::Local, 10).expect("local");
        assert_eq!(
            (mine.cast, mine.played, mine.activated, mine.cast_turn),
            (2, Some(0), Some(3), Some(2))
        );
        let theirs = cards.row(Side::Opponent, 20).expect("opponent");
        assert_eq!((theirs.played, theirs.pregame_played), (Some(1), 1));
        assert_eq!(
            cards.casts,
            vec![GameCardCast {
                side: Side::Local,
                catalog_id: 10,
                method: "kicked".to_string(),
                count: 1,
            }]
        );
    }

    #[test]
    fn reveals_merge_recorded_frame_and_log_only_cards() {
        let last = Frame {
            players: players(0, 0, 40),
            cards: vec![card(1, 30, 2, "Battlefield"), card(2, 30, 2, "Graveyard")],
        };
        let frames = vec![last];
        let recorded = vec![(30, 1), (31, 1)];
        let logged = vec![LogCard {
            catalog_id: 32,
            name: "Qlogged".to_string(),
        }];
        let mut game = input(&frames, Some(true));
        game.recorded_opponent_cards = &recorded;
        game.reveal_log_cards = &logged;
        let cards = game_card_rows(&game);
        let seen: Vec<(u32, u32)> = cards
            .cards
            .iter()
            .filter(|row| row.side == Side::Opponent)
            .map(|row| (row.catalog_id, row.seen))
            .collect();
        assert_eq!(seen, vec![(30, 2), (31, 1), (32, 1)]);
    }
}
