//! The imported postboard rule (issue #6 comment, decided; cards spec
//! section 8 and ruling 7): the deck of a history game (a match imported
//! from MTGO's match history: a v1 history import, or a 0.x copy with 0.x's
//! `matches.imported` flag, issue #313) whose match has a deck version and
//! whose game has a game log.
//!
//! 0.x used the registered maindeck for every imported game, so a sideboard
//! card never got a row and a postboard game counted as if nothing was
//! sided. v1 knows the full registered deck, so a sideboard card the log
//! shows in a postboard game was brought in:
//! - game 1: the registered maindeck (and sideboard), nothing sided;
//! - a postboard game starts from the registered maindeck at full quantity
//!   (what came out is unknowable, so a sided-out card stays in);
//! - a sideboard-only card the log shows is sided in for the copies seen
//!   (capped at main plus side), with its log counters and casts;
//! - a card in both main and side is sided in only for the copies seen
//!   beyond the maindeck count;
//! - a sideboard card not seen keeps a zero-quantity row;
//! - every local row of a postboard game is `inferred`.
//!
//! Only the deck and sideboarding columns change (and a sided-in card's
//! counters, which 0.x threw away); the rest of the rows stay as imported.
//! Cards are matched by catalog id, else by oracle id: the log may show
//! another printing of a registered card (a variant with its own catalog
//! id, as warp casts are logged). An unregistered printing's row folds into
//! the registered printing of its oracle (counters, cast turn and casts)
//! and is dropped, as 0.x counted every printing the player touched under
//! the deck card's oracle. Pure.

use std::collections::{BTreeMap, BTreeSet};

use decks::DeckCard;

use crate::game_cards::{GameCardCast, GameCardRow, GameCards, Side};
use crate::log_cards::{Counter, GameLogCard};

/// Apply the rule to one game's rows. `game_number` is the game's number in
/// the match (1 is game 1), `registered` the match's deck version (main and
/// side rows), `log_cards` the local player's cards in this game's log,
/// `oracle_of` a printing's oracle id (`None` without card data). Call it
/// only for a game that has a game log.
pub fn apply_imported_rule(
    game_number: u32,
    registered: &[DeckCard],
    log_cards: &[GameLogCard],
    oracle_of: &dyn Fn(u32) -> Option<String>,
    rows: &mut GameCards,
) {
    let mut main: BTreeMap<u32, u32> = BTreeMap::new();
    let mut side: BTreeMap<u32, u32> = BTreeMap::new();
    for card in registered {
        let list = if card.sideboard { &mut side } else { &mut main };
        let total = list.entry(card.catalog_id).or_insert(0);
        *total = total.saturating_add(card.quantity);
    }
    let postboard = game_number > 1;

    // The deck columns come from the registered deck alone.
    for row in rows.cards.iter_mut().filter(|row| row.side == Side::Local) {
        row.main_qty = 0;
        row.side_qty = 0;
        row.sided_in = 0;
        row.sided_out = 0;
    }
    for (&catalog_id, &quantity) in &main {
        local_row(rows, catalog_id).main_qty = quantity;
    }
    // Log printings a registered printing took: variant id to that printing.
    let mut claimed: BTreeMap<u32, u32> = BTreeMap::new();
    // Variants whose log counters a sided-in card already took.
    let mut consumed: BTreeSet<u32> = BTreeSet::new();
    for (&catalog_id, &quantity) in &side {
        let in_main = main.get(&catalog_id).copied().unwrap_or(0);
        let (log_card, exact) = match log_cards.iter().find(|card| card.mtgo_id == catalog_id) {
            Some(card) => (Some(card), true),
            None => {
                let found =
                    other_printing(catalog_id, log_cards, &main, &side, &claimed, oracle_of);
                if let Some(card) = found {
                    claimed.insert(card.mtgo_id, catalog_id);
                }
                (found, false)
            }
        };
        let copies = log_card
            .map_or(0, |card| card.copies_seen)
            .min(in_main + quantity);
        let sided = if postboard {
            copies.saturating_sub(in_main)
        } else {
            0
        };
        let row = local_row(rows, catalog_id);
        row.main_qty = in_main + sided;
        row.side_qty = quantity - sided;
        row.sided_in = sided;
        if sided > 0
            && let Some(card) = log_card
        {
            if exact {
                // The log's counters for this card and its unclaimed
                // variants: the variant rows are then dropped, not folded.
                let mut taken = vec![card];
                while let Some(variant) =
                    other_printing(catalog_id, log_cards, &main, &side, &claimed, oracle_of)
                {
                    claimed.insert(variant.mtgo_id, catalog_id);
                    consumed.insert(variant.mtgo_id);
                    taken.push(variant);
                }
                take_log_counters(rows, catalog_id, &taken, copies);
            } else {
                // The variant's row folds in below: only the copies seen.
                let row = local_row(rows, catalog_id);
                row.seen = row.seen.max(copies.min(row.main_qty));
            }
        }
    }
    fold_variants(rows, &main, &side, &claimed, &consumed, oracle_of);

    for row in rows.cards.iter_mut().filter(|row| row.side == Side::Local) {
        row.inferred = postboard;
    }
    rows.cards.sort_by_key(|row| (row.side, row.catalog_id));
    rows.casts
        .sort_by(|a, b| (a.side, a.catalog_id, &a.method).cmp(&(b.side, b.catalog_id, &b.method)));
}

/// The log card of another printing of `catalog_id` (same oracle id; MTGO
/// gives some variants their own catalog id), when the card data knows
/// both, that printing is not registered itself and no other registered
/// printing claimed it (`claimed`, #314 row 8).
fn other_printing<'a>(
    catalog_id: u32,
    log_cards: &'a [GameLogCard],
    main: &BTreeMap<u32, u32>,
    side: &BTreeMap<u32, u32>,
    claimed: &BTreeMap<u32, u32>,
    oracle_of: &dyn Fn(u32) -> Option<String>,
) -> Option<&'a GameLogCard> {
    let oracle = oracle_of(catalog_id)?;
    log_cards.iter().find(|card| {
        !main.contains_key(&card.mtgo_id)
            && !side.contains_key(&card.mtgo_id)
            && !claimed.contains_key(&card.mtgo_id)
            && oracle_of(card.mtgo_id).as_deref() == Some(oracle.as_str())
    })
}

/// The local row of a card, made when missing. A new row counts 0 plays and
/// activations, not unknown: the rule only runs for a game with a log.
fn local_row(rows: &mut GameCards, catalog_id: u32) -> &mut GameCardRow {
    let index = match rows
        .cards
        .iter()
        .position(|row| row.side == Side::Local && row.catalog_id == catalog_id)
    {
        Some(index) => index,
        None => {
            rows.cards.push(GameCardRow {
                side: Side::Local,
                catalog_id,
                played: Some(0),
                activated: Some(0),
                ..GameCardRow::default()
            });
            rows.cards.len() - 1
        }
    };
    &mut rows.cards[index]
}

/// Fold each unregistered local row whose oracle is a registered card's
/// into that card's row and drop it (review I1): the printing that claimed
/// its log card, else the lowest registered catalog id of the oracle. A
/// claimed variant's copies are already the card's `seen`, so `seen` takes
/// the larger; a consumed variant's counters came from the log, so its row
/// and casts are only dropped.
fn fold_variants(
    rows: &mut GameCards,
    main: &BTreeMap<u32, u32>,
    side: &BTreeMap<u32, u32>,
    claimed: &BTreeMap<u32, u32>,
    consumed: &BTreeSet<u32>,
    oracle_of: &dyn Fn(u32) -> Option<String>,
) {
    let mut registered_by_oracle: BTreeMap<String, u32> = BTreeMap::new();
    for &catalog_id in main.keys().chain(side.keys()) {
        if let Some(oracle) = oracle_of(catalog_id) {
            let lowest = registered_by_oracle.entry(oracle).or_insert(catalog_id);
            *lowest = (*lowest).min(catalog_id);
        }
    }
    let mut variants: Vec<(u32, u32)> = Vec::new();
    for row in rows.cards.iter().filter(|row| row.side == Side::Local) {
        let id = row.catalog_id;
        if main.contains_key(&id) || side.contains_key(&id) {
            continue;
        }
        let target = match claimed.get(&id) {
            Some(&target) => Some(target),
            None => oracle_of(id).and_then(|oracle| registered_by_oracle.get(&oracle).copied()),
        };
        if let Some(target) = target {
            variants.push((id, target));
        }
    }
    for (variant, target) in variants {
        let Some(index) = rows
            .cards
            .iter()
            .position(|row| row.side == Side::Local && row.catalog_id == variant)
        else {
            continue;
        };
        let from = rows.cards.remove(index);
        let moved: Vec<GameCardCast> = rows
            .casts
            .iter()
            .filter(|cast| cast.side == Side::Local && cast.catalog_id == variant)
            .cloned()
            .collect();
        rows.casts
            .retain(|cast| !(cast.side == Side::Local && cast.catalog_id == variant));
        if consumed.contains(&variant) {
            continue;
        }
        let into = local_row(rows, target);
        add_counters(into, &from, claimed.contains_key(&variant));
        for cast in moved {
            add_cast(rows, target, &cast.method, cast.count);
        }
    }
}

/// Add a variant row's counters to its card's row (as the card stats
/// shipping merges printings: counts added, the earliest cast turn), with
/// `seen` and the hand counts capped at the card's quantity.
/// `seen_counted`: the card's `seen` already counts the variant's copies.
fn add_counters(into: &mut GameCardRow, from: &GameCardRow, seen_counted: bool) {
    into.opening_hand += from.opening_hand;
    into.bottomed += from.bottomed;
    into.starting_hand += from.starting_hand;
    into.first_draw += from.first_draw;
    into.seen = if seen_counted {
        into.seen.max(from.seen)
    } else {
        into.seen + from.seen
    };
    into.pregame_revealed += from.pregame_revealed;
    into.pregame_played += from.pregame_played;
    into.cast += from.cast;
    into.played = add_optional(into.played, from.played);
    into.activated = add_optional(into.activated, from.activated);
    into.cast_turn = earliest(into.cast_turn, from.cast_turn);
    into.discarded += from.discarded;
    into.hand_seen += from.hand_seen;
    into.graveyard_seen += from.graveyard_seen;
    into.exile_seen += from.exile_seen;
    into.battlefield_seen += from.battlefield_seen;
    into.has_zone_data |= from.has_zone_data;
    // 0.x caps seen and kept at the card's quantity (review N2).
    let quantity = into.main_qty.saturating_add(into.side_qty);
    if quantity > 0 {
        into.seen = into.seen.min(quantity);
        into.opening_hand = into.opening_hand.min(quantity);
        into.starting_hand = into.starting_hand.min(quantity);
    }
}

fn add_optional(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    }
}

fn earliest(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Add `count` casts by `method` to a local card's casts.
fn add_cast(rows: &mut GameCards, catalog_id: u32, method: &str, count: u32) {
    let found = rows.casts.iter_mut().find(|cast| {
        cast.side == Side::Local && cast.catalog_id == catalog_id && cast.method == method
    });
    match found {
        Some(cast) => cast.count += count,
        None => rows.casts.push(GameCardCast {
            side: Side::Local,
            catalog_id,
            method: method.to_string(),
            count,
        }),
    }
}

/// A sided-in card's counters and casts from the log (0.x threw them away):
/// the log cards `taken` (the card and its variants) added together, and
/// its copies seen (at most the copies played).
fn take_log_counters(rows: &mut GameCards, catalog_id: u32, taken: &[&GameLogCard], copies: u32) {
    let total = |counter: Counter| taken.iter().map(|card| card.count(counter)).sum::<u32>();
    let mut cast_turn = None;
    for card in taken {
        cast_turn = earliest(cast_turn, card.cast_turn);
    }
    let row = local_row(rows, catalog_id);
    row.cast = total(Counter::Cast);
    row.played = Some(total(Counter::Played));
    row.activated = Some(total(Counter::Activated));
    row.cast_turn = cast_turn;
    row.seen = row.seen.max(copies.min(row.main_qty));
    rows.casts
        .retain(|cast| !(cast.side == Side::Local && cast.catalog_id == catalog_id));
    for counter in Counter::ALL {
        if matches!(
            counter,
            Counter::Cast | Counter::Played | Counter::Activated
        ) {
            continue;
        }
        let count = total(counter);
        if count > 0 {
            rows.casts.push(GameCardCast {
                side: Side::Local,
                catalog_id,
                method: counter.field_name().to_string(),
                count,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamelog::GameLogEntry;
    use crate::log_cards::{COUNTER_COUNT, extract_log_cards};

    /// 4 main 1001, 2 main 1002 + 1 side 1002, 3 side 1003, 2 side 1004.
    fn registered() -> Vec<DeckCard> {
        let card = |catalog_id, quantity, sideboard| DeckCard {
            catalog_id,
            quantity,
            sideboard,
        };
        vec![
            card(1001, 4, false),
            card(1002, 2, false),
            card(1002, 1, true),
            card(1003, 3, true),
            card(1004, 2, true),
        ]
    }

    fn log_card(mtgo_id: u32, copies_seen: u32, cast: u32) -> GameLogCard {
        let mut counts = [0; COUNTER_COUNT];
        counts[Counter::ALL
            .iter()
            .position(|c| *c == Counter::Cast)
            .expect("cast")] = cast;
        GameLogCard {
            mtgo_id,
            name: format!("Q{mtgo_id}"),
            cast_turn: (cast > 0).then_some(2),
            counts,
            copies_seen,
        }
    }

    /// The rows as a 0.x lay-over leaves them: only the cards seen, at the
    /// copies seen (0.x's seen-only `deck_json`).
    fn imported_rows() -> GameCards {
        let row = |catalog_id, main_qty, seen, cast| GameCardRow {
            side: Side::Local,
            catalog_id,
            main_qty,
            seen,
            cast,
            played: Some(0),
            activated: Some(0),
            ..GameCardRow::default()
        };
        GameCards {
            cards: vec![
                row(1001, 1, 1, 1),
                row(1003, 0, 0, 1),
                GameCardRow {
                    side: Side::Opponent,
                    catalog_id: 9001,
                    seen: 1,
                    ..GameCardRow::default()
                },
            ],
            ..GameCards::default()
        }
    }

    fn no_oracle(_: u32) -> Option<String> {
        None
    }

    fn local(rows: &GameCards, catalog_id: u32) -> GameCardRow {
        rows.row(Side::Local, catalog_id)
            .expect("a local row")
            .clone()
    }

    #[test]
    fn game_one_keeps_the_registered_maindeck() {
        let mut rows = imported_rows();
        let log = vec![log_card(1001, 1, 1), log_card(1003, 1, 1)];
        apply_imported_rule(1, &registered(), &log, &no_oracle, &mut rows);
        let deck: Vec<(u32, u32, u32, u32, bool)> = rows
            .cards
            .iter()
            .filter(|row| row.side == Side::Local)
            .map(|row| {
                (
                    row.catalog_id,
                    row.main_qty,
                    row.side_qty,
                    row.sided_in,
                    row.inferred,
                )
            })
            .collect();
        assert_eq!(
            deck,
            vec![
                (1001, 4, 0, 0, false),
                (1002, 2, 1, 0, false),
                (1003, 0, 3, 0, false),
                (1004, 0, 2, 0, false),
            ]
        );
    }

    #[test]
    fn a_sideboard_only_card_seen_postboard_is_sided_in_with_its_counters() {
        let mut rows = imported_rows();
        let log = vec![log_card(1001, 1, 1), log_card(1003, 2, 3)];
        apply_imported_rule(2, &registered(), &log, &no_oracle, &mut rows);
        let sided = local(&rows, 1003);
        assert_eq!((sided.main_qty, sided.side_qty, sided.sided_in), (2, 1, 2));
        assert_eq!((sided.cast, sided.cast_turn), (3, Some(2)));
        assert_eq!(sided.seen, 2);
    }

    #[test]
    fn a_sided_in_card_takes_its_alternative_casts_from_the_log() {
        let mut rows = imported_rows();
        let mut card = log_card(1004, 1, 1);
        card.counts[Counter::ALL
            .iter()
            .position(|c| *c == Counter::Kicked)
            .expect("kicked")] = 1;
        apply_imported_rule(2, &registered(), &[card], &no_oracle, &mut rows);
        assert_eq!(
            rows.casts,
            vec![GameCardCast {
                side: Side::Local,
                catalog_id: 1004,
                method: "kicked".to_string(),
                count: 1,
            }]
        );
    }

    #[test]
    fn a_main_and_side_card_is_sided_in_only_beyond_the_maindeck_count() {
        // 2 main + 1 side: two copies seen fit the maindeck.
        let mut rows = imported_rows();
        apply_imported_rule(
            2,
            &registered(),
            &[log_card(1002, 2, 0)],
            &no_oracle,
            &mut rows,
        );
        let card = local(&rows, 1002);
        assert_eq!((card.main_qty, card.side_qty, card.sided_in), (2, 1, 0));

        // Three copies: the third came in.
        let mut rows = imported_rows();
        apply_imported_rule(
            2,
            &registered(),
            &[log_card(1002, 3, 0)],
            &no_oracle,
            &mut rows,
        );
        let card = local(&rows, 1002);
        assert_eq!((card.main_qty, card.side_qty, card.sided_in), (3, 0, 1));
    }

    #[test]
    fn an_unseen_sideboard_card_stays_a_zero_row() {
        let mut rows = imported_rows();
        apply_imported_rule(
            2,
            &registered(),
            &[log_card(1003, 1, 1)],
            &no_oracle,
            &mut rows,
        );
        let unseen = local(&rows, 1004);
        assert_eq!(
            (unseen.main_qty, unseen.side_qty, unseen.sided_in),
            (0, 2, 0)
        );
    }

    #[test]
    fn sided_out_cards_stay_in_at_full_quantity() {
        // 1001 is never seen in game 2: maybe sided out, but unknowable.
        let mut rows = imported_rows();
        apply_imported_rule(
            2,
            &registered(),
            &[log_card(1003, 3, 1)],
            &no_oracle,
            &mut rows,
        );
        let main = local(&rows, 1001);
        assert_eq!((main.main_qty, main.sided_out), (4, 0));
        assert!(rows.cards.iter().all(|row| row.sided_out == 0));
    }

    #[test]
    fn copies_seen_counts_distinct_instances_capped_at_main_plus_side() {
        let messages = [
            "@P@PQlocal joined the game.",
            "@P@PQopponent joined the game.",
            "@PTurn 1: Qlocal",
            "@PQlocal casts @[Qside@:2006,301:@].",
            "@PQlocal casts @[Qside@:2006,302:@].",
            "@PQlocal casts @[Qside@:2006,301:@].",
            "@PQlocal casts @[Qside@:2006,303:@].",
            "@PQlocal casts @[Qside@:2006,304:@].",
            "@PQlocal wins the game.",
        ];
        let entries: Vec<GameLogEntry> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| GameLogEntry {
                timestamp_ms: index as i64,
                message: message.to_string(),
            })
            .collect();
        let log = extract_log_cards(&entries);
        let cards = log.game_cards(0, "Qlocal");
        assert_eq!(cards[0].mtgo_id, 1003);
        assert_eq!(cards[0].copies_seen, 4, "301 twice counts once");

        // 1003 is a three-of in the sideboard: four copies cannot be.
        let mut rows = imported_rows();
        apply_imported_rule(2, &registered(), cards, &no_oracle, &mut rows);
        let sided = local(&rows, 1003);
        assert_eq!((sided.main_qty, sided.side_qty, sided.sided_in), (3, 0, 3));
    }

    #[test]
    fn every_local_row_of_an_imported_postboard_game_is_inferred() {
        let mut rows = imported_rows();
        apply_imported_rule(
            3,
            &registered(),
            &[log_card(1003, 1, 1)],
            &no_oracle,
            &mut rows,
        );
        for row in &rows.cards {
            assert_eq!(row.inferred, row.side == Side::Local, "{row:?}");
        }
        let ids: Vec<u32> = rows
            .cards
            .iter()
            .filter(|row| row.side == Side::Local)
            .map(|row| row.catalog_id)
            .collect();
        assert_eq!(ids, vec![1001, 1002, 1003, 1004], "sorted, one per card");
    }

    #[test]
    fn rows_the_rule_makes_have_zero_log_counters_not_unknown() {
        // The rule runs only for a game with a log: NULL would say "no log".
        let mut rows = imported_rows();
        apply_imported_rule(2, &registered(), &[], &no_oracle, &mut rows);
        for id in [1002, 1004] {
            let made = local(&rows, id);
            assert_eq!((made.played, made.activated), (Some(0), Some(0)), "{id}");
        }
    }

    // S7-2: a sideboard card the log shows only as another printing of
    // the card (a variant with its own catalog id): it is matched by oracle id,
    // and the variant's row folds into it (0.x counted every printing the
    // player touched under the deck card's oracle).
    #[test]
    fn a_sideboard_card_seen_as_another_printing_is_sided_in() {
        let oracle_of = |catalog_id: u32| match catalog_id {
            1003 | 1903 => Some("o-3".to_string()),
            1001 => Some("o-1".to_string()),
            _ => None,
        };
        let mut rows = imported_rows();
        rows.cards.push(GameCardRow {
            side: Side::Local,
            catalog_id: 1903,
            cast: 3,
            played: Some(0),
            activated: Some(0),
            ..GameCardRow::default()
        });
        let log = vec![log_card(1001, 1, 1), log_card(1903, 2, 3)];
        apply_imported_rule(2, &registered(), &log, &oracle_of, &mut rows);
        let sided = local(&rows, 1003);
        assert_eq!((sided.main_qty, sided.side_qty, sided.sided_in), (2, 1, 2));
        assert_eq!(sided.seen, 2, "the copies seen, not added again");
        assert_eq!(sided.cast, 4, "its own counters plus the variant's");
        assert!(
            rows.row(Side::Local, 1903).is_none(),
            "folded, counted once"
        );
        // Without card data, nothing matches another printing.
        let mut rows = imported_rows();
        apply_imported_rule(2, &registered(), &log, &no_oracle, &mut rows);
        assert_eq!(local(&rows, 1003).sided_in, 0);
    }

    // Review I1: MTGO logs some casts under a variant's catalog id (warp).
    // The variant row of a maindeck card folds into the registered printing
    // with its casts, so card stats count them as 0.x did.
    #[test]
    fn a_maindeck_card_variant_row_folds_into_the_registered_printing() {
        let oracle_of = |catalog_id: u32| match catalog_id {
            1001 | 1901 => Some("o-1".to_string()),
            _ => None,
        };
        let mut rows = imported_rows();
        rows.cards.push(GameCardRow {
            side: Side::Local,
            catalog_id: 1901,
            seen: 1,
            cast: 2,
            cast_turn: Some(3),
            played: Some(0),
            activated: Some(1),
            ..GameCardRow::default()
        });
        rows.casts.push(GameCardCast {
            side: Side::Local,
            catalog_id: 1901,
            method: "warp".to_string(),
            count: 2,
        });
        for game in [1, 2] {
            let mut folded = rows.clone();
            apply_imported_rule(game, &registered(), &[], &oracle_of, &mut folded);
            assert!(folded.row(Side::Local, 1901).is_none(), "game {game}");
            let card = local(&folded, 1001);
            assert_eq!((card.main_qty, card.seen, card.cast), (4, 2, 3));
            assert_eq!((card.cast_turn, card.activated), (Some(3), Some(1)));
            assert_eq!(
                folded.casts,
                vec![GameCardCast {
                    side: Side::Local,
                    catalog_id: 1001,
                    method: "warp".to_string(),
                    count: 2,
                }]
            );
            let mut twice = folded.clone();
            apply_imported_rule(game, &registered(), &[], &oracle_of, &mut twice);
            assert_eq!(twice, folded, "applied twice equals once");
        }
    }

    // Review N2: a folded `seen` and hand count never pass the card's
    // quantity (0.x caps them per card).
    #[test]
    fn a_folded_seen_and_hand_are_capped_at_the_quantity() {
        let oracle_of = |catalog_id: u32| match catalog_id {
            1001 | 1901 => Some("o-1".to_string()),
            _ => None,
        };
        let mut rows = imported_rows();
        let main = rows
            .cards
            .iter_mut()
            .find(|row| row.catalog_id == 1001)
            .expect("main");
        main.seen = 3;
        main.opening_hand = 3;
        main.starting_hand = 3;
        rows.cards.push(GameCardRow {
            side: Side::Local,
            catalog_id: 1901,
            seen: 3,
            opening_hand: 2,
            starting_hand: 2,
            ..GameCardRow::default()
        });
        apply_imported_rule(2, &registered(), &[], &oracle_of, &mut rows);
        let card = local(&rows, 1001);
        assert_eq!(
            (
                card.main_qty,
                card.seen,
                card.opening_hand,
                card.starting_hand
            ),
            (4, 4, 4, 4)
        );
    }

    // A sided-in card the log shows under its own id and as a variant: its
    // counters are the log's for both printings, and the variant row goes.
    #[test]
    fn a_sided_in_card_takes_its_variants_log_counters_once() {
        let oracle_of = |catalog_id: u32| match catalog_id {
            1003 | 1903 => Some("o-3".to_string()),
            _ => None,
        };
        let mut rows = imported_rows();
        rows.cards.push(GameCardRow {
            side: Side::Local,
            catalog_id: 1903,
            cast: 2,
            played: Some(0),
            activated: Some(0),
            ..GameCardRow::default()
        });
        let log = vec![log_card(1003, 2, 1), log_card(1903, 1, 2)];
        apply_imported_rule(2, &registered(), &log, &oracle_of, &mut rows);
        assert!(rows.row(Side::Local, 1903).is_none());
        let sided = local(&rows, 1003);
        assert_eq!((sided.sided_in, sided.cast), (2, 3));
        let mut twice = rows.clone();
        apply_imported_rule(2, &registered(), &log, &oracle_of, &mut twice);
        assert_eq!(twice, rows);
    }

    // #314 row 8: two registered printings of one card never both claim
    // the same log printing; the first (lowest catalog id) takes it.
    #[test]
    fn a_log_printing_is_claimed_once() {
        let oracle_of = |catalog_id: u32| match catalog_id {
            1003 | 1004 | 1903 => Some("o-3".to_string()),
            _ => None,
        };
        let mut rows = imported_rows();
        let log = vec![log_card(1903, 2, 1)];
        apply_imported_rule(2, &registered(), &log, &oracle_of, &mut rows);
        let first = local(&rows, 1003);
        assert_eq!((first.main_qty, first.side_qty, first.sided_in), (2, 1, 2));
        let second = local(&rows, 1004);
        assert_eq!(
            (second.main_qty, second.side_qty, second.sided_in),
            (0, 2, 0)
        );
    }

    #[test]
    fn the_rule_applied_twice_equals_once() {
        let mut once = imported_rows();
        let log = vec![log_card(1002, 3, 1), log_card(1003, 2, 1)];
        apply_imported_rule(2, &registered(), &log, &no_oracle, &mut once);
        let mut twice = once.clone();
        apply_imported_rule(2, &registered(), &log, &no_oracle, &mut twice);
        assert_eq!(once, twice);
    }
}
