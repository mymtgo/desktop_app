//! One Arena game's per-game rows, with the meanings MTGO's rows have
//! (`game_detail::GameCards`, schema `game_cards`): the deck played, the
//! sideboarding against game 1, the opening hand, the bottomed cards and the
//! starting hand, the turn 1 draw, the hands sent back, the cards seen and
//! the zone counters, the casts and land plays, and the opponent's cards
//! that were public; plus the `games` columns Arena's messages tell.

use std::collections::{BTreeMap, BTreeSet};

use game_detail::{GameCardRow, GameCards, Side};
use serde::Deserialize;

use crate::{
    CARD, CardKey, Fold, GameInput, GameState, Source, Transfer, catalog_id, fold_game, front_face,
};

/// The size of a full opening hand (the London mulligan always draws 7).
const FULL_HAND: u32 = 7;

const HAND: &str = "ZoneType_Hand";
const LIBRARY: &str = "ZoneType_Library";
const GRAVEYARD: &str = "ZoneType_Graveyard";
const EXILE: &str = "ZoneType_Exile";
const BATTLEFIELD: &str = "ZoneType_Battlefield";
const STACK: &str = "ZoneType_Stack";
const LIMBO: &str = "ZoneType_Limbo";
const REVEALED: &str = "ZoneType_Revealed";
const COMMAND: &str = "ZoneType_Command";

/// A card of the opponent's hand shown to the local player: a copy in the
/// opponent's Revealed zone, not the card itself.
const REVEALED_CARD: &str = "GameObjectType_RevealedCard";

/// The zones where the local player sees their own cards (as MTGO's `seen`).
const SEEN_ZONES: [&str; 5] = [HAND, BATTLEFIELD, GRAVEYARD, EXILE, STACK];

/// The `games` columns one game's messages tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameColumns {
    pub on_play: Option<bool>,
    pub local_mulligans: Option<u32>,
    pub opp_mulligans: Option<u32>,
    pub turn_count: Option<u32>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// Each seat's commanders as catalog ids (`ARENA_ID_OFFSET + grpId`), in the
/// command zone's order. Empty for a game with no command zone (not a
/// commander format).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commanders {
    pub local: Vec<u32>,
    pub opponent: Vec<u32>,
}

/// Everything stored for one game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameSummary {
    pub columns: GameColumns,
    pub cards: GameCards,
    pub commanders: Commanders,
}

/// One game's rows from its fold and its messages (in log order).
/// `first_game_deck`: game 1's local main deck (catalog id -> quantity) for
/// the sideboarding counts; None for game 1.
///
/// - The deck: the game's last `SubmitDeckResp`, else `SubmitDeckReq`, else
///   the `ConnectResp` deck.
/// - The hands: the local hand at each `MulliganResp`; the hands sent back
///   go to `mulligans`, the kept one is the opening hand. Bottomed cards are
///   opening hand cards moved from the hand to the library after the keep,
///   up to the state that starts turn 1.
/// - `on_play`: turn 1 is the local player's. The turn 1 draw: on the draw,
///   the local player's first draw in their first turn.
/// - Seen and the zone counters: distinct cards (`CardKey`) per zone kind,
///   from the keep on (the hands sent back are not seen in the game, the
///   owner's ruling), capped at the copies played as MTGO caps them. Discards: `Discard` transfers.
/// - Casts and land plays: `CastSpell` and `PlayLand` transfers, both sides.
/// - The opponent's cards: every card of theirs that was public, once per
///   card, and the hand cards they revealed.
pub fn summarise_game(
    fold: &Fold,
    messages: &[GameInput],
    first_game_deck: Option<&BTreeMap<u32, u32>>,
) -> GameSummary {
    // A fold that never saw the game start holds another game's tail.
    let (states, transfers): (&[GameState], &[Transfer]) = if fold.started {
        (&fold.states, &fold.transfers)
    } else {
        (&[], &[])
    };
    let read = read_messages(messages);
    let local = fold.local_seat;
    let opponent = local.and_then(|seat| opponent_seat(states, seat));
    let faces = Faces {
        deck: read.deck.as_ref(),
    };
    let hands = match local {
        Some(seat) => hands(fold, messages, &read.decisions, seat, &faces),
        None => Hands::default(),
    };
    let on_play = on_play(states, local);
    let opp_mulligans = opponent.and_then(|seat| mulligans_of(states, seat));

    let mut rows: BTreeMap<(Side, u32), GameCardRow> = BTreeMap::new();
    let mut cards = GameCards::default();
    if let Some(deck) = &read.deck {
        add_deck(&mut rows, deck, first_game_deck);
    }
    if let (Some(seat), Some(keep)) = (local, &hands.keep) {
        let bottomed = add_opening_hand(&mut rows, states, transfers, keep, seat);
        cards.opening_hand_size = Some(keep.cards.len() as u32);
        cards.starting_hand_size = Some(keep.cards.len() as u32 - bottomed);
        cards.opp_opening_hand_size = Some(FULL_HAND);
        cards.opp_starting_hand_size = Some(FULL_HAND.saturating_sub(opp_mulligans.unwrap_or(0)));
    }
    for (index, hand) in hands.sent_back.iter().enumerate() {
        for (catalog_id, quantity) in hand {
            cards
                .mulligans
                .push((index as u32 + 1, *catalog_id, *quantity));
        }
    }
    if let Some(seat) = local {
        if on_play != Some(true) {
            add_first_draw(&mut rows, states, transfers, seat, &faces);
        }
        // The hands sent back are not seen in the game: count from the keep.
        let from = hands
            .keep
            .as_ref()
            .map(|keep| keep.state_index)
            .or_else(|| states.iter().position(|state| state.turn >= 1));
        if let Some(from) = from {
            let deck_known = read.deck.is_some();
            add_local_zones(
                &mut rows,
                &states[from..],
                &fold.identity,
                seat,
                &faces,
                deck_known,
            );
            add_discards(&mut rows, transfers, seat, &faces, deck_known);
        }
    }
    let played = add_counters(&mut rows, states, transfers, local, opponent, &faces);
    if let Some(seat) = opponent {
        add_opponent_cards(&mut rows, states, transfers, &fold.identity, seat);
    }
    for row in rows.values_mut() {
        if fold.started {
            let count = played.get(&(row.side, row.catalog_id)).copied();
            row.played = Some(count.unwrap_or(0));
        }
        row.has_zone_data = fold.started && local.is_some() && row.side == Side::Local;
    }
    cards.cards = rows.into_values().collect();

    let columns = GameColumns {
        on_play,
        local_mulligans: hands.decisions,
        opp_mulligans,
        turn_count: states
            .last()
            .map(|state| state.turn)
            .filter(|turn| *turn > 0),
        started_at: read.first_full_at.or(messages.first().map(|m| m.logged_at)),
        ended_at: messages.last().map(|m| m.logged_at),
    };
    let commanders = match local {
        Some(seat) => command_zone_commanders(states, seat),
        None => Commanders::default(),
    };
    GameSummary {
        columns,
        cards,
        commanders,
    }
}

/// The commanders of both seats: the cards in the command zones of the first
/// state that has any. A card that enters the command zone later (an emblem
/// source, a Companion-style effect) is not a commander. The seat is the
/// card's owner, against the local seat.
fn command_zone_commanders(states: &[GameState], local: u32) -> Commanders {
    let mut commanders = Commanders::default();
    for state in states {
        for zone in state.zones.values().filter(|zone| zone.kind == COMMAND) {
            for id in &zone.object_ids {
                let Some(object) = state.objects.get(id) else {
                    continue;
                };
                if object.kind != CARD {
                    continue;
                }
                let Some(catalog) = catalog_id(object.card_grp_id()) else {
                    continue;
                };
                if object.owner_seat == local {
                    commanders.local.push(catalog);
                } else {
                    commanders.opponent.push(catalog);
                }
            }
        }
        if !commanders.local.is_empty() || !commanders.opponent.is_empty() {
            break;
        }
    }
    commanders
}

/// The local main deck of a summarised game (catalog id -> quantity).
pub fn local_main_deck(summary: &GameSummary) -> BTreeMap<u32, u32> {
    let mut deck = BTreeMap::new();
    for row in &summary.cards.cards {
        if row.side == Side::Local && row.main_qty > 0 {
            deck.insert(row.catalog_id, row.main_qty);
        }
    }
    deck
}

/// A game's local deck by grpId, (main, sideboard): the deck the summary
/// reads (`summarise_game`). `None` when no message carries one.
pub(crate) fn game_deck(
    messages: &[GameInput],
) -> Option<(BTreeMap<u32, u32>, BTreeMap<u32, u32>)> {
    let deck = read_messages(messages).deck?;
    // A commander is listed as a sideboard card, the way the store keeps it.
    let mut side = by_grp_id(&deck.side);
    for (grp_id, count) in by_grp_id(&deck.commanders) {
        *side.entry(grp_id).or_insert(0) += count;
    }
    Some((by_grp_id(&deck.main), side))
}

/// Quantities by catalog id as quantities by grpId.
fn by_grp_id(by_catalog_id: &BTreeMap<u32, u32>) -> BTreeMap<u32, u32> {
    by_catalog_id
        .iter()
        .filter_map(|(catalog_id, count)| {
            cards::grp_id_of(i64::from(*catalog_id)).map(|grp| (grp.0, *count))
        })
        .collect()
}

/// The parts of a GRE or client message the summary reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    connect_resp: Option<RawConnect>,
    submit_deck_req: Option<RawSubmitDeck>,
    submit_deck_resp: Option<RawSubmitDeck>,
    mulligan_resp: Option<RawMulliganResp>,
    game_state_message: Option<RawStateKind>,
    #[serde(rename = "type")]
    message_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConnect {
    deck_message: Option<RawDeck>,
}

#[derive(Debug, Deserialize)]
struct RawSubmitDeck {
    deck: Option<RawDeck>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDeck {
    #[serde(default)]
    deck_cards: Vec<u32>,
    #[serde(default)]
    sideboard_cards: Vec<u32>,
    #[serde(default)]
    commander_cards: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct RawMulliganResp {
    #[serde(default)]
    decision: String,
}

#[derive(Debug, Deserialize)]
struct RawStateKind {
    #[serde(rename = "type", default)]
    kind: String,
}

/// A deck by catalog id: main and sideboard quantities.
#[derive(Debug, Default)]
struct Deck {
    main: BTreeMap<u32, u32>,
    side: BTreeMap<u32, u32>,
    /// The command zone cards of a commander format deck (not in `side`).
    commanders: BTreeMap<u32, u32>,
}

impl Deck {
    fn from_raw(raw: &RawDeck) -> Deck {
        Deck {
            main: tally(&raw.deck_cards),
            side: tally(&raw.sideboard_cards),
            commanders: tally(&raw.commander_cards),
        }
    }
}

/// Quantities per catalog id of a grpId list.
fn tally(grp_ids: &[u32]) -> BTreeMap<u32, u32> {
    let mut counts = BTreeMap::new();
    for grp_id in grp_ids {
        if let Some(catalog_id) = catalog_id(*grp_id) {
            *counts.entry(catalog_id).or_insert(0) += 1;
        }
    }
    counts
}

/// What the summary reads from the messages themselves.
#[derive(Debug, Default)]
struct ReadMessages {
    deck: Option<Deck>,
    /// The local mulligan decisions: message index and whether it kept.
    decisions: Vec<(usize, bool)>,
    /// When the game's first full state was logged.
    first_full_at: Option<i64>,
}

/// The deck messages after the game's intermission are the next game's
/// (stored as this game's by a restart while sideboarding, before Arena plan
/// 3's final fix) and are not read. An intermission before the game's first
/// full state is the previous game's (Arena can send it twice) and ends
/// nothing.
fn read_messages(messages: &[GameInput]) -> ReadMessages {
    let mut read = ReadMessages::default();
    let mut submitted: Option<Deck> = None;
    let mut offered: Option<Deck> = None;
    let mut connected: Option<Deck> = None;
    let mut ended = false;
    for (index, input) in messages.iter().enumerate() {
        let Ok(message) = serde_json::from_str::<RawMessage>(&input.json) else {
            continue;
        };
        match input.source {
            Source::Client => {
                if let Some(deck) = message.submit_deck_resp.and_then(|resp| resp.deck)
                    && !ended
                {
                    submitted = Some(Deck::from_raw(&deck));
                }
                if let Some(resp) = message.mulligan_resp {
                    match resp.decision.as_str() {
                        "MulliganOption_AcceptHand" => read.decisions.push((index, true)),
                        "MulliganOption_Mulligan" => read.decisions.push((index, false)),
                        _ => {}
                    }
                }
            }
            Source::Gre => {
                if message.message_type.as_deref() == Some(arena_protocol::INTERMISSION_TYPE)
                    && read.first_full_at.is_some()
                {
                    ended = true;
                }
                if let Some(deck) = message.submit_deck_req.and_then(|req| req.deck)
                    && !ended
                {
                    offered = Some(Deck::from_raw(&deck));
                }
                let connect_deck = message.connect_resp.and_then(|resp| resp.deck_message);
                if let (None, Some(deck)) = (&connected, connect_deck) {
                    connected = Some(Deck::from_raw(&deck));
                }
                let full = message
                    .game_state_message
                    .is_some_and(|state| state.kind == "GameStateType_Full");
                if full && read.first_full_at.is_none() {
                    read.first_full_at = Some(input.logged_at);
                }
            }
        }
    }
    read.deck = submitted.or(offered).or(connected);
    read
}

/// Which face of a local two-faced card is the card: the one the game's
/// deck lists, else the front face (`front_face`). The opponent's cards
/// always take the front face.
struct Faces<'a> {
    deck: Option<&'a Deck>,
}

impl Faces<'_> {
    /// The catalog id of a local card shown as `grp_id` with other face
    /// `other_grp_id` (0 for none).
    fn local(&self, grp_id: u32, other_grp_id: u32) -> Option<u32> {
        if other_grp_id != 0
            && let Some(deck) = self.deck
        {
            for face in [grp_id, other_grp_id] {
                if let Some(id) = catalog_id(face)
                    && (deck.main.contains_key(&id) || deck.side.contains_key(&id))
                {
                    return Some(id);
                }
            }
        }
        catalog_id(front_face(grp_id, other_grp_id))
    }

    fn local_transfer(&self, transfer: &Transfer) -> Option<u32> {
        self.local(transfer.grp_id, transfer.other_face_grp_id)
    }
}

/// The local hand at the mulligan decisions.
#[derive(Debug, Default)]
struct Hands {
    /// The number of hands sent back, once a decision was made.
    decisions: Option<u32>,
    /// Each hand sent back: catalog id -> quantity.
    sent_back: Vec<BTreeMap<u32, u32>>,
    keep: Option<Keep>,
}

/// The hand kept.
#[derive(Debug)]
struct Keep {
    /// The index in `Fold::states` of the state the hand was kept in.
    state_index: usize,
    /// The hand's cards and their catalog ids.
    cards: Vec<(CardKey, u32)>,
}

/// Each decision's hand is the local hand in the last state before it: the
/// game folded up to the decision.
fn hands(
    fold: &Fold,
    messages: &[GameInput],
    decisions: &[(usize, bool)],
    local: u32,
    faces: &Faces,
) -> Hands {
    let mut hands = Hands::default();
    for (index, kept) in decisions {
        let before = fold_game(&messages[..*index]);
        let Some(state) = before.states.last() else {
            continue;
        };
        let cards = hand_cards(state, &fold.identity, local, faces);
        if *kept {
            hands.decisions = Some(hands.decisions.unwrap_or(0));
            if !cards.is_empty() {
                hands.keep = Some(Keep {
                    state_index: before.states.len() - 1,
                    cards,
                });
            }
            break;
        }
        hands.decisions = Some(hands.decisions.unwrap_or(0) + 1);
        let mut counts = BTreeMap::new();
        for (_, catalog_id) in cards {
            *counts.entry(catalog_id).or_insert(0) += 1;
        }
        hands.sent_back.push(counts);
    }
    hands
}

/// The cards in a seat's hand.
fn hand_cards(
    state: &GameState,
    identity: &crate::Identity,
    seat: u32,
    faces: &Faces,
) -> Vec<(CardKey, u32)> {
    let mut cards = Vec::new();
    for zone in state.zones.values() {
        if zone.kind != HAND || zone.owner_seat != Some(seat) {
            continue;
        }
        for id in &zone.object_ids {
            let Some(object) = state.objects.get(id) else {
                continue;
            };
            if object.kind != CARD {
                continue;
            }
            if let Some(catalog_id) = faces.local(object.grp_id, object.otherside_grp_id) {
                cards.push((identity.key(*id), catalog_id));
            }
        }
    }
    cards
}

/// The other seat of the game.
fn opponent_seat(states: &[GameState], local: u32) -> Option<u32> {
    for state in states {
        for seat in state.players.keys() {
            if *seat != local {
                return Some(*seat);
            }
        }
        for zone in state.zones.values() {
            if let Some(seat) = zone.owner_seat
                && seat != local
            {
                return Some(seat);
            }
        }
    }
    None
}

/// Whether turn 1 was the local player's.
fn on_play(states: &[GameState], local: Option<u32>) -> Option<bool> {
    let local = local?;
    let first = states.iter().find(|state| state.turn == 1)?;
    first.active_seat.map(|seat| seat == local)
}

/// A seat's mulligans: the most any state states, or 0 once turn 1 began.
fn mulligans_of(states: &[GameState], seat: u32) -> Option<u32> {
    let stated = states
        .iter()
        .filter_map(|state| state.mulligans.get(&seat).copied())
        .max();
    let began = states.iter().any(|state| state.turn >= 1);
    match stated {
        Some(count) => Some(count),
        None if began => Some(0),
        None => None,
    }
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

/// The deck as played and the copies changed against game 1's main deck.
fn add_deck(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    deck: &Deck,
    first_game_deck: Option<&BTreeMap<u32, u32>>,
) {
    for (catalog_id, quantity) in &deck.main {
        row(rows, Side::Local, *catalog_id).main_qty = *quantity;
    }
    for (catalog_id, quantity) in &deck.side {
        row(rows, Side::Local, *catalog_id).side_qty = *quantity;
    }
    let Some(first) = first_game_deck else {
        return;
    };
    if first.is_empty() || deck.main.is_empty() {
        return;
    }
    let mut ids: BTreeSet<u32> = first.keys().copied().collect();
    ids.extend(deck.main.keys());
    for catalog_id in ids {
        let before = first.get(&catalog_id).copied().unwrap_or(0);
        let now = deck.main.get(&catalog_id).copied().unwrap_or(0);
        if now > before {
            row(rows, Side::Local, catalog_id).sided_in = now - before;
        } else if before > now {
            row(rows, Side::Local, catalog_id).sided_out = before - now;
        }
    }
}

/// The opening hand and the bottomed cards: opening hand cards moved from
/// the hand to the library after the keep, up to the state that starts turn
/// 1 (the game starts once both players have bottomed). Returns how many
/// were bottomed.
fn add_opening_hand(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    states: &[GameState],
    transfers: &[Transfer],
    keep: &Keep,
    local: u32,
) -> u32 {
    let last = states
        .iter()
        .enumerate()
        .skip(keep.state_index)
        .find(|(_, state)| state.turn >= 1)
        .map_or(usize::MAX, |(index, _)| index);
    let mut bottomed: BTreeSet<CardKey> = BTreeSet::new();
    for transfer in transfers {
        if transfer.state_index > keep.state_index
            && transfer.state_index <= last
            && transfer.owner_seat == local
            && transfer.from_zone == HAND
            && transfer.to_zone == LIBRARY
        {
            bottomed.insert(transfer.card);
        }
    }
    let mut count = 0;
    for (key, catalog_id) in &keep.cards {
        let entry = row(rows, Side::Local, *catalog_id);
        entry.opening_hand += 1;
        if bottomed.contains(key) {
            entry.bottomed += 1;
            count += 1;
        }
        entry.starting_hand = entry.opening_hand - entry.bottomed;
    }
    count
}

/// On the draw: the card the local player drew first in their first turn.
fn add_first_draw(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    states: &[GameState],
    transfers: &[Transfer],
    local: u32,
    faces: &Faces,
) {
    let is_local_turn = |state: &GameState| state.turn >= 1 && state.active_seat == Some(local);
    let Some(first_turn) = states.iter().find(|s| is_local_turn(s)).map(|s| s.turn) else {
        return;
    };
    let drawn = transfers.iter().find(|transfer| {
        let state = &states[transfer.state_index];
        transfer.category == "Draw"
            && transfer.kind == CARD
            && transfer.owner_seat == local
            && is_local_turn(state)
            && state.turn == first_turn
    });
    if let Some(catalog_id) = drawn.and_then(|transfer| faces.local_transfer(transfer)) {
        row(rows, Side::Local, catalog_id).first_draw += 1;
    }
}

/// Distinct cards per catalog id.
type CardSets = BTreeMap<u32, BTreeSet<CardKey>>;

/// The local cards seen (`seen`) and the zone counters over these states:
/// distinct cards per zone kind, capped at the copies played when the deck
/// is known. A card no row holds gets one only with a count left.
fn add_local_zones(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    states: &[GameState],
    identity: &crate::Identity,
    local: u32,
    faces: &Faces,
    deck_known: bool,
) {
    let mut seen: CardSets = BTreeMap::new();
    let mut by_zone: BTreeMap<&str, CardSets> = BTreeMap::new();
    for state in states {
        for zone in state.zones.values() {
            let Some(kind) = SEEN_ZONES.iter().find(|kind| **kind == zone.kind) else {
                continue;
            };
            for id in &zone.object_ids {
                let Some(object) = state.objects.get(id) else {
                    continue;
                };
                if object.kind != CARD || object.owner_seat != local {
                    continue;
                }
                let Some(catalog_id) = faces.local(object.grp_id, object.otherside_grp_id) else {
                    continue;
                };
                let key = identity.key(*id);
                seen.entry(catalog_id).or_default().insert(key);
                let zone_sets = by_zone.entry(kind).or_default();
                zone_sets.entry(catalog_id).or_default().insert(key);
            }
        }
    }
    let count = |zone: &str, catalog_id: u32| -> u32 {
        by_zone
            .get(zone)
            .and_then(|sets| sets.get(&catalog_id))
            .map_or(0, |keys| keys.len() as u32)
    };
    for (catalog_id, keys) in &seen {
        let cap = |count: u32| capped(rows, *catalog_id, deck_known, count);
        let seen_count = cap(keys.len() as u32);
        let hand = cap(count(HAND, *catalog_id));
        let graveyard = cap(count(GRAVEYARD, *catalog_id));
        let exile = cap(count(EXILE, *catalog_id));
        let battlefield = cap(count(BATTLEFIELD, *catalog_id));
        let exists = rows.contains_key(&(Side::Local, *catalog_id));
        if !exists && seen_count == 0 {
            continue;
        }
        let entry = row(rows, Side::Local, *catalog_id);
        entry.seen = seen_count;
        entry.hand_seen = hand;
        entry.graveyard_seen = graveyard;
        entry.exile_seen = exile;
        entry.battlefield_seen = battlefield;
    }
}

/// A count capped at the copies of a local card played, when the deck is
/// known (as MTGO caps `seen` and the zone counters).
fn capped(
    rows: &BTreeMap<(Side, u32), GameCardRow>,
    catalog_id: u32,
    deck_known: bool,
    count: u32,
) -> u32 {
    if !deck_known {
        return count;
    }
    let copies = rows
        .get(&(Side::Local, catalog_id))
        .map_or(0, |row| row.main_qty.saturating_add(row.side_qty));
    count.min(copies)
}

/// The local cards discarded (`Discard` transfers), capped like the zones.
fn add_discards(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    transfers: &[Transfer],
    local: u32,
    faces: &Faces,
    deck_known: bool,
) {
    let mut discarded: CardSets = BTreeMap::new();
    for transfer in transfers {
        if transfer.category != "Discard" || transfer.kind != CARD || transfer.owner_seat != local {
            continue;
        }
        if let Some(catalog_id) = faces.local_transfer(transfer) {
            discarded
                .entry(catalog_id)
                .or_default()
                .insert(transfer.card);
        }
    }
    for (catalog_id, keys) in discarded {
        let count = capped(rows, catalog_id, deck_known, keys.len() as u32);
        if count > 0 {
            row(rows, Side::Local, catalog_id).discarded = count;
        }
    }
}

/// Casts (and the first cast's turn) on the rows; returns the land plays
/// per row.
fn add_counters(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    states: &[GameState],
    transfers: &[Transfer],
    local: Option<u32>,
    opponent: Option<u32>,
    faces: &Faces,
) -> BTreeMap<(Side, u32), u32> {
    let mut played = BTreeMap::new();
    for transfer in transfers {
        if transfer.kind != CARD {
            continue;
        }
        let side = if Some(transfer.owner_seat) == local {
            Side::Local
        } else if Some(transfer.owner_seat) == opponent {
            Side::Opponent
        } else {
            continue;
        };
        let catalog_id = match side {
            Side::Local => faces.local_transfer(transfer),
            Side::Opponent => catalog_id(transfer.grp_id),
        };
        let Some(catalog_id) = catalog_id else {
            continue;
        };
        match transfer.category.as_str() {
            "CastSpell" => {
                let turn = states[transfer.state_index].turn;
                let entry = row(rows, side, catalog_id);
                entry.cast += 1;
                if entry.cast_turn.is_none() && turn > 0 {
                    entry.cast_turn = Some(turn);
                }
            }
            "PlayLand" => {
                row(rows, side, catalog_id);
                *played.entry((side, catalog_id)).or_insert(0) += 1;
            }
            _ => {}
        }
    }
    played
}

/// The opponent's cards seen: per catalog id, every card of theirs that was
/// public in a zone (Limbo aside: it holds old ids of the same cards), plus
/// each hand card they revealed that never showed as a public card. A
/// revealed card is matched to the card that leaves their hand as its reveal
/// ends.
fn add_opponent_cards(
    rows: &mut BTreeMap<(Side, u32), GameCardRow>,
    states: &[GameState],
    transfers: &[Transfer],
    identity: &crate::Identity,
    opponent: u32,
) {
    let mut cards: CardSets = BTreeMap::new();
    // Revealed copies by instance id: catalog id and the card matched.
    let mut revealed: BTreeMap<u32, (u32, Option<CardKey>)> = BTreeMap::new();
    let mut showing: BTreeSet<u32> = BTreeSet::new();
    for (index, state) in states.iter().enumerate() {
        let mut now: BTreeSet<u32> = BTreeSet::new();
        for zone in state.zones.values() {
            if zone.kind == LIMBO {
                continue;
            }
            for id in &zone.object_ids {
                let Some(object) = state.objects.get(id) else {
                    continue;
                };
                if object.owner_seat != opponent {
                    continue;
                }
                let Some(catalog_id) = catalog_id(object.card_grp_id()) else {
                    continue;
                };
                if object.kind == CARD && object.public {
                    cards
                        .entry(catalog_id)
                        .or_default()
                        .insert(identity.key(*id));
                } else if object.kind == REVEALED_CARD && zone.kind == REVEALED {
                    now.insert(*id);
                    revealed.entry(*id).or_insert((catalog_id, None));
                }
            }
        }
        for id in showing.difference(&now) {
            match_reveal(&mut revealed, *id, transfers, index, opponent);
        }
        showing = now;
    }
    // A card known only by its reveal counts once on its own.
    let mut only_revealed: BTreeMap<u32, u32> = BTreeMap::new();
    for (catalog_id, card) in revealed.into_values() {
        match card {
            Some(key) => {
                cards.entry(catalog_id).or_default().insert(key);
            }
            None => *only_revealed.entry(catalog_id).or_insert(0) += 1,
        }
    }
    let mut ids: BTreeSet<u32> = cards.keys().copied().collect();
    ids.extend(only_revealed.keys());
    for catalog_id in ids {
        let public = cards.get(&catalog_id).map_or(0, |keys| keys.len() as u32);
        let shown = only_revealed.get(&catalog_id).copied().unwrap_or(0);
        row(rows, Side::Opponent, catalog_id).seen = public + shown;
    }
}

/// Match an ended reveal to the opponent's card of the same catalog id that
/// left their hand in that state.
fn match_reveal(
    revealed: &mut BTreeMap<u32, (u32, Option<CardKey>)>,
    id: u32,
    transfers: &[Transfer],
    state_index: usize,
    opponent: u32,
) {
    let Some((catalog_id, None)) = revealed.get(&id).copied() else {
        return;
    };
    let taken: Vec<CardKey> = revealed.values().filter_map(|(_, card)| *card).collect();
    let card = transfers.iter().find(|transfer| {
        transfer.state_index == state_index
            && transfer.owner_seat == opponent
            && transfer.from_zone == HAND
            && crate::catalog_id(transfer.grp_id) == Some(catalog_id)
            && !taken.contains(&transfer.card)
    });
    if let Some(transfer) = card {
        revealed.insert(id, (catalog_id, Some(transfer.card)));
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use arena_match::{ArenaInput, ArenaProjector};
    use arena_protocol::{decode_all, split_complete};
    use cards::ARENA_ID_OFFSET;
    use game_detail::{GameCardRow, Side};

    use super::*;
    use crate::test_support::{fixture_game, fixture_match_game};
    use crate::{Source, fold_game};

    fn gre(json: &str) -> GameInput {
        GameInput {
            source: Source::Gre,
            logged_at: 1000,
            json: json.to_string(),
        }
    }

    /// One fixture game's summary, with game 1's deck for later games.
    fn summarise_fixture(name: &str, match_index: usize, game: u32) -> GameSummary {
        let first_deck = if game > 1 {
            Some(local_main_deck(&summarise_fixture(name, match_index, 1)))
        } else {
            None
        };
        let inputs = fixture_match_game(name, match_index, game).inputs;
        summarise_game(&fold_game(&inputs), &inputs, first_deck.as_ref())
    }

    /// The deck a game's messages submitted: the last `SubmitDeckResp`, else
    /// `SubmitDeckReq`, else `ConnectResp` (main, sideboard grpIds), read
    /// straight from the JSON.
    fn raw_deck(inputs: &[GameInput]) -> (Vec<u32>, Vec<u32>) {
        let mut found: [Option<(Vec<u32>, Vec<u32>)>; 3] = [None, None, None];
        for input in inputs {
            let value: serde_json::Value = serde_json::from_str(&input.json).unwrap();
            for (slot, pointer) in [
                (0, "/submitDeckResp/deck"),
                (1, "/submitDeckReq/deck"),
                (2, "/connectResp/deckMessage"),
            ] {
                if let Some(deck) = value.pointer(pointer) {
                    let ids = |key: &str| -> Vec<u32> {
                        deck[key]
                            .as_array()
                            .map(|list| list.iter().map(|n| n.as_u64().unwrap() as u32).collect())
                            .unwrap_or_default()
                    };
                    found[slot] = Some((ids("deckCards"), ids("sideboardCards")));
                }
            }
        }
        found
            .into_iter()
            .flatten()
            .next()
            .expect("the game has a deck")
    }

    fn local_rows(summary: &GameSummary) -> Vec<&GameCardRow> {
        summary
            .cards
            .cards
            .iter()
            .filter(|row| row.side == Side::Local)
            .collect()
    }

    fn sum(rows: &[&GameCardRow], field: fn(&GameCardRow) -> u32) -> u32 {
        rows.iter().map(|row| field(row)).sum()
    }

    #[test]
    fn local_deck_sums_to_submitted_deck() {
        for game in 1..=2 {
            let inputs = fixture_game("bo3-three-games", game).inputs;
            let (main, side) = raw_deck(&inputs);
            let summary = summarise_fixture("bo3-three-games", 0, game);
            let rows = local_rows(&summary);
            assert_eq!(
                sum(&rows, |r| r.main_qty) as usize,
                main.len(),
                "game {game}"
            );
            assert_eq!(
                sum(&rows, |r| r.side_qty) as usize,
                side.len(),
                "game {game}"
            );
            assert!(main.len() >= 60);
            for row in &summary.cards.cards {
                assert!(row.catalog_id >= ARENA_ID_OFFSET, "{row:?}");
            }
        }
    }

    /// Every game of both fixtures.
    fn every_fixture_game() -> Vec<(String, GameSummary)> {
        let mut games = Vec::new();
        for (name, match_index, count) in [
            ("bo3-three-games", 0, 3),
            ("bo3-loss-and-match-concede", 0, 3),
            ("bo3-loss-and-match-concede", 1, 2),
        ] {
            for game in 1..=count {
                let label = format!("{name} match {match_index} game {game}");
                games.push((label, summarise_fixture(name, match_index, game)));
            }
        }
        games
    }

    #[test]
    fn starting_hand_is_opening_minus_bottomed() {
        for (label, summary) in every_fixture_game() {
            let rows = local_rows(&summary);
            for row in &summary.cards.cards {
                assert_eq!(
                    row.starting_hand,
                    row.opening_hand - row.bottomed,
                    "{label}: {row:?}"
                );
            }
            let cards = &summary.cards;
            if let Some(opening) = cards.opening_hand_size {
                assert_eq!(opening, sum(&rows, |r| r.opening_hand), "{label}");
                assert_eq!(
                    cards.starting_hand_size,
                    Some(sum(&rows, |r| r.starting_hand)),
                    "{label}"
                );
            }
        }
    }

    /// A game where seat 2's card sits in its hand, then is played.
    const PRIVATE: &str = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[500]},
          {"zoneId":30,"type":"ZoneType_Limbo","objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":500,"grpId":7000,"type":"GameObjectType_Card","zoneId":35,"visibility":"Visibility_Private","ownerSeatId":2}]}}"#;

    const PLAYED: &str = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":30,"type":"ZoneType_Limbo","objectInstanceIds":[500]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[501]}],
        "gameObjects":[
          {"instanceId":500,"grpId":7000,"type":"GameObjectType_Card","zoneId":30,"visibility":"Visibility_Public","ownerSeatId":2},
          {"instanceId":501,"grpId":7000,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2}],
        "annotations":[
          {"id":7,"affectedIds":[500],"type":["AnnotationType_ObjectIdChanged"],
           "details":[{"key":"orig_id","valueInt32":[500]},{"key":"new_id","valueInt32":[501]}]},
          {"id":8,"affectedIds":[501],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","valueInt32":[35]},{"key":"zone_dest","valueInt32":[28]},
                      {"key":"category","valueString":["PlayLand"]}]}]}}"#;

    #[test]
    fn opponent_side_has_only_public_cards() {
        let catalog_id = ARENA_ID_OFFSET + 7000;
        let hidden = [gre(PRIVATE)];
        let summary = summarise_game(&fold_game(&hidden), &hidden, None);
        assert_eq!(summary.cards.row(Side::Opponent, catalog_id), None);

        let shown = [gre(PRIVATE), gre(PLAYED)];
        let summary = summarise_game(&fold_game(&shown), &shown, None);
        let row = summary
            .cards
            .row(Side::Opponent, catalog_id)
            .expect("the played card");
        assert_eq!(row.seen, 1, "its old id in Limbo is the same card");
        assert_eq!(row.played, Some(1));
        assert_eq!(summary.cards.row(Side::Local, catalog_id), None);
    }

    #[test]
    fn local_two_faced_card_takes_the_face_its_deck_lists() {
        // The deck lists the larger id; the opponent's copy takes the smaller.
        let connect = r#"{"type":"GREMessageType_ConnectResp","systemSeatIds":[1],
          "connectResp":{"deckMessage":{"deckCards":[73477],"sideboardCards":[]}}}"#;
        let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
          "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
            "turnInfo":{"turnNumber":1,"activePlayer":1},
            "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
            "zones":[
              {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[300]},
              {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[400]}],
            "gameObjects":[
              {"instanceId":300,"grpId":73476,"othersideGrpId":73477,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1},
              {"instanceId":400,"grpId":73477,"othersideGrpId":73476,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2}]}}"#;
        let inputs = [gre(connect), gre(full)];
        let summary = summarise_game(&fold_game(&inputs), &inputs, None);
        let local = summary
            .cards
            .row(Side::Local, ARENA_ID_OFFSET + 73477)
            .expect("the deck's face");
        assert_eq!((local.main_qty, local.seen, local.hand_seen), (1, 1, 1));
        assert_eq!(
            summary.cards.row(Side::Local, ARENA_ID_OFFSET + 73476),
            None
        );
        let opponent = summary
            .cards
            .row(Side::Opponent, ARENA_ID_OFFSET + 73476)
            .expect("the front face");
        assert_eq!(opponent.seen, 1);
    }

    #[test]
    fn mulliganed_away_cards_are_not_seen() {
        // Three-games game 1: 69396 and 79448 were only in the hand sent back.
        let summary = summarise_fixture("bo3-three-games", 0, 1);
        assert!(
            summary
                .cards
                .mulligans
                .iter()
                .any(|m| m.1 == ARENA_ID_OFFSET + 69396)
        );
        for grp_id in [69396, 79448] {
            let row = summary
                .cards
                .row(Side::Local, ARENA_ID_OFFSET + grp_id)
                .expect("a deck card");
            assert_eq!((row.seen, row.hand_seen), (0, 0), "{grp_id}");
        }
    }

    #[test]
    fn tokens_are_not_cards() {
        let tokens = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
          "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
            "turnInfo":{"turnNumber":1,"activePlayer":1},
            "zones":[{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[600,601,602]}],
            "gameObjects":[
              {"instanceId":600,"grpId":8000,"type":"GameObjectType_Token","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1},
              {"instanceId":601,"grpId":8001,"type":"GameObjectType_Token","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2},
              {"instanceId":602,"grpId":8002,"type":"GameObjectType_Ability","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2}]}}"#;
        let inputs = [gre(tokens)];
        let summary = summarise_game(&fold_game(&inputs), &inputs, None);
        assert_eq!(summary.cards.cards, Vec::new());
    }

    #[test]
    fn game_two_sided_in_and_out() {
        let game_one = fixture_game("bo3-three-games", 1).inputs;
        let game_two = fixture_game("bo3-three-games", 2).inputs;
        let tally = |ids: Vec<u32>| {
            let mut counts: BTreeMap<u32, i64> = BTreeMap::new();
            for id in ids {
                *counts.entry(id).or_insert(0) += 1;
            }
            counts
        };
        let before = tally(raw_deck(&game_one).0);
        let after = tally(raw_deck(&game_two).0);
        let mut ids: Vec<u32> = before.keys().chain(after.keys()).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        let (mut exact_in, mut exact_out) = (0, 0);
        for id in ids {
            let change =
                after.get(&id).copied().unwrap_or(0) - before.get(&id).copied().unwrap_or(0);
            if change > 0 {
                exact_in += change;
            } else {
                exact_out -= change;
            }
        }
        assert!(exact_in > 0, "game 2 was sideboarded");

        let summary = summarise_fixture("bo3-three-games", 0, 2);
        let rows = local_rows(&summary);
        assert_eq!(i64::from(sum(&rows, |r| r.sided_in)), exact_in);
        assert_eq!(i64::from(sum(&rows, |r| r.sided_out)), exact_out);
        let game_one_summary = summarise_fixture("bo3-three-games", 0, 1);
        let rows = local_rows(&game_one_summary);
        assert_eq!(sum(&rows, |r| r.sided_in) + sum(&rows, |r| r.sided_out), 0);
    }

    /// The app restarted while sideboarding (before Arena plan 3's final
    /// fix) and stored game 2's deck messages as game 1's: game 1 keeps the
    /// deck it was played with.
    #[test]
    fn deck_messages_after_the_intermission_are_not_the_games_deck() {
        let game_one = fixture_game("bo3-three-games", 1).inputs;
        let game_two = fixture_game("bo3-three-games", 2).inputs;
        let last_deck = game_two
            .iter()
            .rposition(|input| input.json.contains("submitDeckResp"))
            .expect("game 2 submits its deck");
        let mut stored_as_one = game_one.clone();
        stored_as_one.extend(game_two[..=last_deck].iter().cloned());
        assert_ne!(raw_deck(&stored_as_one), raw_deck(&game_one));

        let expected = summarise_game(&fold_game(&game_one), &game_one, None);
        let summary = summarise_game(&fold_game(&stored_as_one), &stored_as_one, None);
        assert_eq!(local_main_deck(&summary), local_main_deck(&expected));
        assert_eq!(summary.cards, expected.cards);
    }

    /// The projector's `on_play` per game of the fixture's first match.
    fn projector_on_play(name: &str) -> Vec<Option<bool>> {
        let fixture = fixture_tools::arena_fixture(name);
        let mut projector = ArenaProjector::new();
        let mut tokens: Vec<String> = Vec::new();
        for entry in split_complete(&fixture.bytes) {
            for (seq, event) in decode_all(&entry).into_iter().enumerate() {
                let changes = projector.apply(&ArenaInput {
                    file_id: 1,
                    offset: entry.offset,
                    seq: seq as u32,
                    at_ms: 0,
                    event,
                });
                for token in changes.matches {
                    if !tokens.contains(&token) {
                        tokens.push(token);
                    }
                }
            }
        }
        let view = projector.view(&tokens[0]).expect("the first match");
        view.games.iter().map(|game| game.on_play).collect()
    }

    #[test]
    fn on_play_matches_projector() {
        let projected = projector_on_play("bo3-three-games");
        assert_eq!(projected.len(), 3);
        for (index, on_play) in projected.into_iter().enumerate() {
            let summary = summarise_fixture("bo3-three-games", 0, index as u32 + 1);
            assert!(on_play.is_some());
            assert_eq!(summary.columns.on_play, on_play, "game {}", index + 1);
        }
    }

    #[test]
    fn game_conceded_at_the_mulligan_has_a_deck_and_no_casts() {
        let summary = summarise_fixture("bo3-loss-and-match-concede", 1, 1);
        let rows = local_rows(&summary);
        assert_eq!(sum(&rows, |r| r.main_qty), 60);
        assert_eq!(
            summary.columns.local_mulligans, None,
            "no decision was sent"
        );
        assert_eq!(summary.columns.turn_count, None);
        assert_eq!(summary.cards.opening_hand_size, None);
        assert!(summary.cards.mulligans.is_empty());
        for row in &summary.cards.cards {
            assert_eq!((row.cast, row.opening_hand, row.seen), (0, 0, 0), "{row:?}");
        }
    }

    #[test]
    fn a_game_without_messages_is_empty() {
        let summary = summarise_game(&fold_game(&[]), &[], None);
        assert_eq!(summary, GameSummary::default());
    }

    /// A summary as text for a snapshot: the columns, the hand sizes, one
    /// line per row with its non-zero counts (by grpId, as the log shows
    /// them), then the hands sent back.
    fn render(summary: &GameSummary) -> String {
        let mut out = String::new();
        let cards = &summary.cards;
        writeln!(out, "{:?}", summary.columns).unwrap();
        writeln!(
            out,
            "hand sizes: opening {:?} starting {:?} opponent opening {:?} starting {:?}",
            cards.opening_hand_size,
            cards.starting_hand_size,
            cards.opp_opening_hand_size,
            cards.opp_starting_hand_size
        )
        .unwrap();
        for row in &cards.cards {
            let mut fields: Vec<String> = Vec::new();
            for (name, value) in [
                ("main", row.main_qty),
                ("side", row.side_qty),
                ("in", row.sided_in),
                ("out", row.sided_out),
                ("opening", row.opening_hand),
                ("bottomed", row.bottomed),
                ("starting", row.starting_hand),
                ("first_draw", row.first_draw),
                ("seen", row.seen),
                ("cast", row.cast),
                ("discarded", row.discarded),
                ("hand", row.hand_seen),
                ("graveyard", row.graveyard_seen),
                ("exile", row.exile_seen),
                ("battlefield", row.battlefield_seen),
            ] {
                if value > 0 {
                    fields.push(format!("{name}={value}"));
                }
            }
            match row.played {
                Some(0) => {}
                Some(played) => fields.push(format!("played={played}")),
                None => fields.push("played=unknown".to_string()),
            }
            if let Some(turn) = row.cast_turn {
                fields.push(format!("cast_turn={turn}"));
            }
            if row.activated.is_some() {
                fields.push(format!("activated={:?}", row.activated));
            }
            if row.has_zone_data {
                fields.push("zones".to_string());
            }
            writeln!(
                out,
                "{} {} {}",
                row.side.as_str(),
                row.catalog_id - ARENA_ID_OFFSET,
                fields.join(" ")
            )
            .unwrap();
        }
        for (hand, catalog_id, quantity) in &cards.mulligans {
            writeln!(
                out,
                "mulligan hand {hand}: {} x{quantity}",
                catalog_id - ARENA_ID_OFFSET
            )
            .unwrap();
        }
        writeln!(out, "casts by method: {}", cards.casts.len()).unwrap();
        out
    }

    #[test]
    fn fixture_three_games_summary() {
        let mut out = String::new();
        for game in 1..=3 {
            writeln!(out, "== game {game}").unwrap();
            out.push_str(&render(&summarise_fixture("bo3-three-games", 0, game)));
        }
        insta::assert_snapshot!(out);
    }

    #[test]
    fn fixture_concede_summary() {
        let mut out = String::new();
        for (match_index, count) in [(0, 3), (1, 2)] {
            for game in 1..=count {
                writeln!(out, "== match {} game {game}", match_index + 1).unwrap();
                let summary = summarise_fixture("bo3-loss-and-match-concede", match_index, game);
                out.push_str(&render(&summary));
            }
        }
        insta::assert_snapshot!(out);
    }
}
