//! Each game's card rows: what the game's states showed of each card and
//! what its events counted, by seat, turned into `game_cards` rows once the
//! user's seat is known.
//!
//! A card's side is its owner seat against the user's seat, never whether it
//! has a printing (trap 22: many of the user's own cards have none). Cards
//! seen are distinct card ids, never one per zone entry (trap 23), and a
//! card id counts under the first name it was seen with: a double-faced card
//! shows its other face under the same id (trap 60).

use std::collections::{BTreeMap, BTreeSet};

use endstep_protocol::{EndstepCardKey, GameEvent, GameState, Zone};
use game_detail::{GameCardRow, Side};

use crate::EndstepDeck;
use crate::hands::{HandCard, Hands};
use crate::sideboard::GameDeck;

/// Distinct ids of one card name seen in each zone the counters cover.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ZoneIds {
    hand: BTreeSet<u64>,
    battlefield: BTreeSet<u64>,
    graveyard: BTreeSet<u64>,
    exile: BTreeSet<u64>,
}

impl ZoneIds {
    fn insert(&mut self, zone: Zone, id: u64) {
        match zone {
            Zone::Hand => self.hand.insert(id),
            Zone::Battlefield => self.battlefield.insert(id),
            Zone::Graveyard => self.graveyard.insert(id),
            Zone::Exile => self.exile.insert(id),
            Zone::Stack | Zone::Command | Zone::Library => false,
        };
    }
}

/// What one game's states showed of one card name for one owner seat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SeenName {
    /// Distinct ids seen in a hand, on the battlefield, in a graveyard, in
    /// exile or on the stack.
    ids: BTreeSet<u64>,
    /// The same ids, from the owner's kept hand state on (the hand sent back
    /// by a mulligan is left out).
    ids_kept: BTreeSet<u64>,
    /// Distinct ids seen in the command zone (counted for the opponent only).
    command_ids: BTreeSet<u64>,
    /// The first printing a state gave, as (set code, collector number).
    printing: Option<(String, String)>,
    /// The ids per zone over every state.
    zones_all: ZoneIds,
    /// The ids per zone over the owner's kept hand state and every state
    /// after it.
    zones_kept: ZoneIds,
}

/// One seat's event counts for one card name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EventCounts {
    cast: u32,
    played: u32,
    activated: u32,
}

/// Why an event named a card. The owner is resolved once the user's seat is
/// known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    /// A `CARD_REVEALED` list; true when Endstep marked it the user's own
    /// cards (`ownerEcho`). Its `playerIndex` is the viewer, never the owner.
    Revealed(bool),
    /// A `SPELL_CAST`, `LAND_PLAYED` or `ABILITY_ACTIVATED` by this seat.
    Cast(u32),
    /// A named `CARD_ZONE_CHANGE` out of this seat's hand. Forge sends the
    /// cast of a card cast for free (Force of Negation) only this way.
    LeftHand(u32),
}

/// What a game's states showed of one seat's zones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SeatStates {
    /// A full state came.
    any: bool,
    /// A full state came from the kept hand on: the last one before the
    /// seat's `HAND_KEPT`, or any after it.
    after_keep: bool,
    /// The latest full state was of turn 0 (or no turn): only then can a
    /// `HAND_KEPT` keep its hand.
    last_from_turn_zero: bool,
}

/// One game's tally, keyed by (seat, card name).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CardTally {
    seen: BTreeMap<(u32, String), SeenName>,
    events: BTreeMap<(u32, String), EventCounts>,
    /// The owner each state card id had, the first one read, for every state
    /// card with an owner (even one the counts leave out).
    owner_of: BTreeMap<u64, u32>,
    /// Per seat not yet kept: the counted cards of its last full state.
    pending: BTreeMap<u32, Vec<(String, Zone, u64)>>,
    /// Ids of tokens, from states and `TOKEN_CREATED` events.
    token_ids: BTreeSet<u64>,
    /// The name each card id a state counted or an event named was first
    /// seen with. A double-faced card keeps its id when it turns over, so
    /// its other face's name counts under this one.
    first_names: BTreeMap<u64, String>,
    /// Cards only an event named: (card id, name, why).
    sightings: BTreeSet<(u64, String, Source)>,
    /// Seats whose `HAND_KEPT` came.
    kept: BTreeSet<u32>,
    states: BTreeMap<u32, SeatStates>,
    /// Ids of the discarded cards, by the discarding seat and card name: a
    /// named hand to graveyard `CARD_ZONE_CHANGE` (by the zone's owner) or a
    /// `DISCARD_LOG` (by its player). Forge sends both for one discard, so a
    /// card id counts once.
    discards: BTreeMap<(u32, String), BTreeSet<u64>>,
    /// The same, for the discards after the zone owner's `HAND_KEPT`.
    discards_kept: BTreeMap<(u32, String), BTreeSet<u64>>,
}

impl CardTally {
    /// Notes the cards of one full state. Tokens, face-down cards, library
    /// cards, cards without types (Forge effect objects, trap 24) and cards
    /// without an owner are left out, and so is a sideboarding state (its
    /// zones are empty).
    pub(crate) fn read_state(&mut self, state: &GameState) {
        if state.sideboarding {
            return;
        }
        let mut seats: BTreeSet<u32> = state.player_states.iter().map(|p| p.seat).collect();
        seats.extend(state.cards.iter().filter_map(|card| card.owner_seat));
        for seat in seats {
            let states = self.states.entry(seat).or_default();
            states.any = true;
            states.last_from_turn_zero = matches!(state.turn_number, None | Some(0));
            if self.kept.contains(&seat) {
                states.after_keep = true;
            } else {
                // The last full state before the keep is the kept hand.
                self.pending.insert(seat, Vec::new());
            }
        }
        for card in &state.cards {
            if card.is_token {
                self.token_ids.insert(card.id);
            }
            if let Some(owner) = card.owner_seat {
                self.owner_of.entry(card.id).or_insert(owner);
            }
            if card.is_token || card.face_down || !card.has_types || card.zone == Zone::Library {
                continue;
            }
            let Some(owner) = card.owner_seat else {
                continue;
            };
            let name = self.first_name(card.id, &card.name);
            let kept = self.kept.contains(&owner);
            if !kept && let Some(pending) = self.pending.get_mut(&owner) {
                pending.push((name.clone(), card.zone, card.id));
            }
            let seen = self.seen.entry((owner, name)).or_default();
            if card.zone == Zone::Command {
                seen.command_ids.insert(card.id);
            } else {
                seen.ids.insert(card.id);
            }
            seen.zones_all.insert(card.zone, card.id);
            if kept {
                if card.zone != Zone::Command {
                    seen.ids_kept.insert(card.id);
                }
                seen.zones_kept.insert(card.zone, card.id);
            }
            if seen.printing.is_none()
                && !card.set_code.is_empty()
                && !card.collector_number.is_empty()
            {
                seen.printing = Some((card.set_code.to_lowercase(), card.collector_number.clone()));
            }
        }
    }

    /// The name card `id` was first seen with, `name` when it is new.
    fn first_name(&mut self, id: u64, name: &str) -> String {
        self.first_names
            .entry(id)
            .or_insert_with(|| name.to_string())
            .clone()
    }

    /// A seat kept its hand: the cards of its last full state are the opening
    /// hand and count from here on. A keep after a state of a later turn is
    /// the next game's (its sideboarding state was lost) and is ignored.
    fn keep(&mut self, seat: u32) {
        let from_turn_zero = self
            .states
            .get(&seat)
            .is_some_and(|states| states.last_from_turn_zero);
        if !from_turn_zero || !self.kept.insert(seat) {
            return;
        }
        let Some(pending) = self.pending.remove(&seat) else {
            return;
        };
        self.states.entry(seat).or_default().after_keep = true;
        for (name, zone, id) in pending {
            let seen = self.seen.entry((seat, name)).or_default();
            if zone != Zone::Command {
                seen.ids_kept.insert(id);
            }
            seen.zones_kept.insert(zone, id);
        }
    }

    /// Counts a `SPELL_CAST`, `LAND_PLAYED` or `ABILITY_ACTIVATED` event that
    /// names its card and seat, and notes the cards the events name:
    /// reveals, casts, land plays and activations (sightings), tokens, keeps
    /// and discards.
    pub(crate) fn read_event(&mut self, event: &GameEvent) {
        match event.kind.as_str() {
            "CARD_REVEALED" => {
                for (id, name) in event.card_ids.iter().zip(&event.card_names) {
                    if is_named(name) {
                        let name = self.first_name(*id, name);
                        let source = Source::Revealed(event.owner_echo);
                        self.sightings.insert((*id, name, source));
                    }
                }
            }
            "TOKEN_CREATED" => {
                if let Some(id) = event.card_id {
                    self.token_ids.insert(id);
                }
            }
            "HAND_KEPT" => {
                if let Some(seat) = event.player_index {
                    self.keep(seat);
                }
            }
            "CARD_ZONE_CHANGE" => self.read_zone_change(event),
            "DISCARD_LOG" => self.read_discard_log(event),
            _ => {}
        }
        let (Some(seat), Some(name)) = (event.player_index, &event.card_name) else {
            return;
        };
        let mut name = name.clone();
        if let Some(id) = event.card_id
            && is_named(&name)
        {
            name = self.first_name(id, &name);
            let sighting = matches!(
                event.kind.as_str(),
                "SPELL_CAST" | "LAND_PLAYED" | "ABILITY_ACTIVATED"
            );
            if sighting {
                self.sightings
                    .insert((id, name.clone(), Source::Cast(seat)));
            }
        }
        let counts = self.events.entry((seat, name)).or_default();
        match event.kind.as_str() {
            "SPELL_CAST" => counts.cast = counts.cast.saturating_add(1),
            "LAND_PLAYED" => counts.played = counts.played.saturating_add(1),
            "ABILITY_ACTIVATED" => counts.activated = counts.activated.saturating_add(1),
            _ => {}
        }
    }

    /// A named card leaving a hand is shown, and the hand's owner owns it. One
    /// that goes to a graveyard is a discard (the mulligan moves have no
    /// card).
    fn read_zone_change(&mut self, event: &GameEvent) {
        let (Some(id), Some(name), Some(owner)) =
            (event.card_id, &event.card_name, event.zone_owner)
        else {
            return;
        };
        if event.from_zone.as_deref() != Some("Hand") || !is_named(name) {
            return;
        }
        let name = self.first_name(id, name);
        self.sightings
            .insert((id, name.clone(), Source::LeftHand(owner)));
        if event.to_zone.as_deref() == Some("Graveyard") {
            self.note_discard(owner, id, name);
        }
    }

    /// A `DISCARD_LOG` names the card its player discarded: the card left
    /// that player's hand.
    fn read_discard_log(&mut self, event: &GameEvent) {
        let (Some(id), Some(name), Some(seat)) =
            (event.card_id, &event.card_name, event.player_index)
        else {
            return;
        };
        if !is_named(name) {
            return;
        }
        let name = self.first_name(id, name);
        self.sightings
            .insert((id, name.clone(), Source::LeftHand(seat)));
        self.note_discard(seat, id, name);
    }

    /// Notes card `id` discarded from `seat`'s hand, and from the kept hand
    /// on when the seat has kept.
    fn note_discard(&mut self, seat: u32, id: u64, name: String) {
        if self.kept.contains(&seat) {
            self.discards_kept
                .entry((seat, name.clone()))
                .or_default()
                .insert(id);
        }
        self.discards.entry((seat, name)).or_default().insert(id);
    }

    /// The event counts of `name` for the user's `seat` (`local`) or for
    /// every other seat.
    fn counts(&self, seat: u32, local: bool, name: &str) -> EventCounts {
        let mut total = EventCounts::default();
        for ((event_seat, event_name), counts) in &self.events {
            if event_name != name || (*event_seat == seat) != local {
                continue;
            }
            total.cast = total.cast.saturating_add(counts.cast);
            total.played = total.played.saturating_add(counts.played);
            total.activated = total.activated.saturating_add(counts.activated);
        }
        total
    }

    /// Whether the user's `seat` has zone counters: a full state came after
    /// its keep, or any state when no keep is known.
    fn has_zone_data(&self, seat: u32) -> bool {
        let Some(states) = self.states.get(&seat) else {
            return false;
        };
        if self.kept.contains(&seat) {
            states.after_keep
        } else {
            states.any
        }
    }

    /// The distinct card ids seen of each name, split into the user's cards
    /// and every other seat's: the states' ids joined by the ids only an
    /// event named. A token is never a sighting, and an event card a state
    /// counted counts once, under the state's owner and name. A user's card
    /// the states showed only in a hand sent back is not counted by them, so
    /// an event that names it later (an echo reveal) still counts it.
    fn distinct_seen(&self, seat: u32) -> DistinctSeen {
        let mut out = DistinctSeen::default();
        // An id two events name (a reveal, then a cast), or a state and an
        // event, counts once.
        let mut placed: BTreeSet<u64> = BTreeSet::new();
        for ((owner, name), seen) in &self.seen {
            if *owner == seat {
                let ids = if self.kept.contains(&seat) {
                    &seen.ids_kept
                } else {
                    &seen.ids
                };
                out.local
                    .entry(name.clone())
                    .or_default()
                    .extend(ids.iter().copied());
                placed.extend(ids.iter().copied());
            } else {
                let ids = out.opponent.entry(name.clone()).or_default();
                ids.extend(seen.ids.iter().copied());
                ids.extend(seen.command_ids.iter().copied());
                placed.extend(seen.ids.iter().copied());
                placed.extend(seen.command_ids.iter().copied());
            }
        }
        for (id, name, source) in &self.sightings {
            if self.token_ids.contains(id) || !placed.insert(*id) {
                continue;
            }
            // A state's owner of the card wins over what the event says.
            let local = match (self.owner_of.get(id), source) {
                (Some(owner), _) => *owner == seat,
                (None, Source::Revealed(owner_echo)) => *owner_echo,
                (None, Source::Cast(owner) | Source::LeftHand(owner)) => *owner == seat,
            };
            let side = if local {
                &mut out.local
            } else {
                &mut out.opponent
            };
            side.entry(name.clone()).or_default().insert(*id);
        }
        out
    }
}

/// Distinct card ids by name, for the user's cards and the others'.
#[derive(Default)]
struct DistinctSeen {
    local: BTreeMap<String, BTreeSet<u64>>,
    opponent: BTreeMap<String, BTreeSet<u64>>,
}

/// Endstep names a card the viewer cannot see "Hidden card".
fn is_named(name: &str) -> bool {
    !name.is_empty() && name != "Hidden card"
}

type CardRows = Vec<(EndstepCardKey, GameCardRow)>;
type MulliganLines = Vec<(u32, EndstepCardKey, u32)>;

/// One game's card rows and hand numbers.
pub(crate) struct BuiltCards {
    pub rows: CardRows,
    pub opening_hand_size: Option<u32>,
    pub starting_hand_size: Option<u32>,
    pub mulligans: MulliganLines,
}

/// The decks one game's rows need.
pub(crate) struct GameDecks<'a> {
    /// The deck the game was played with.
    pub played: Option<&'a GameDeck>,
    /// Game 1's deck to measure the sideboarding against (none for game 1
    /// itself).
    pub game_one: Option<&'a GameDeck>,
}

/// One game's rows, sorted by side then key, with `catalog_id` 0 (the store
/// fills it in). `seat` is the user's seat, `deck` the match deck (it gives
/// the keys) and `hands` the user's hands.
pub(crate) fn game_card_rows(
    tally: &CardTally,
    seat: u32,
    deck: Option<&EndstepDeck>,
    decks: GameDecks,
    hands: &Hands,
) -> BuiltCards {
    let distinct = tally.distinct_seen(seat);
    let (mut rows, mulligans) = local_rows(tally, &distinct, seat, deck, decks, hands);
    rows.extend(opponent_rows(tally, &distinct, seat));
    let (opening_hand_size, starting_hand_size) = if hands.has_opening() {
        let opening = count(hands.opening.len());
        // The settle's `amount` says how many were kept even when no state
        // showed which card went to the bottom; never more than the hand.
        let starting = match hands.settled_size {
            Some(amount) => amount.min(opening),
            None => opening.saturating_sub(count(hands.bottomed.len())),
        };
        (Some(opening), Some(starting))
    } else {
        (None, None)
    };
    BuiltCards {
        rows,
        opening_hand_size,
        starting_hand_size,
        mulligans,
    }
}

/// Every match deck card, plus every card of the user's seen but not in the
/// deck. A name's counters go on the match deck's first key for that name, a
/// name the match deck lacks gets a name-only key. The quantities are the
/// game's deck spread over the match deck's keys for the name (see
/// `spread_over_keys`), and the sideboarding is its main against game 1's.
fn local_rows(
    tally: &CardTally,
    distinct: &DistinctSeen,
    seat: u32,
    deck: Option<&EndstepDeck>,
    decks: GameDecks,
    hands: &Hands,
) -> (CardRows, MulliganLines) {
    let GameDecks { played, game_one } = decks;
    let mut rows: BTreeMap<EndstepCardKey, GameCardRow> = BTreeMap::new();
    let mut key_by_name: BTreeMap<String, EndstepCardKey> = BTreeMap::new();
    if let Some(deck) = deck {
        for (key, _) in deck.main.iter().chain(&deck.side) {
            rows.entry(key.clone())
                .or_insert_with(|| empty_row(Side::Local));
            key_by_name.entry(key.name.clone()).or_insert(key.clone());
        }
    }
    let deck_names = played
        .into_iter()
        .chain(game_one)
        .flat_map(|deck| deck.main.iter().chain(&deck.side));
    for (name, _) in deck_names {
        if !key_by_name.contains_key(name) {
            let key = state_key(name, None);
            rows.entry(key.clone())
                .or_insert_with(|| empty_row(Side::Local));
            key_by_name.insert(name.clone(), key);
        }
    }
    if let Some(played) = played {
        let (match_main, match_side) = match deck {
            Some(deck) => (deck.main.as_slice(), deck.side.as_slice()),
            None => (&[][..], &[][..]),
        };
        let main = spread_over_keys(match_main, &played.main, &key_by_name);
        for (key, quantity) in main {
            if let Some(row) = rows.get_mut(&key) {
                row.main_qty = quantity;
            }
        }
        let side = spread_over_keys(match_side, &played.side, &key_by_name);
        for (key, quantity) in side {
            if let Some(row) = rows.get_mut(&key) {
                row.side_qty = quantity;
            }
        }
        if let Some(game_one) = game_one {
            for (name, change) in sideboarding(&game_one.main, &played.main) {
                let row = key_by_name.get(&name).and_then(|key| rows.get_mut(key));
                if let Some(row) = row {
                    row.sided_in = change.0;
                    row.sided_out = change.1;
                }
            }
        }
    }

    // The user's cards seen outside the deck take their state printing, when
    // a state showed them.
    for (name, ids) in &distinct.local {
        if ids.is_empty() || key_by_name.contains_key(name) {
            continue;
        }
        let printing = tally
            .seen
            .get(&(seat, name.clone()))
            .and_then(|seen| seen.printing.as_ref());
        let key = state_key(name, printing);
        rows.entry(key.clone())
            .or_insert_with(|| empty_row(Side::Local));
        key_by_name.insert(name.clone(), key);
    }

    // A hand card with no row yet gets one.
    let hand_cards = hands
        .opening
        .iter()
        .chain(&hands.bottomed)
        .chain(hands.sent_back.iter().flatten());
    for (_, name) in hand_cards {
        if !key_by_name.contains_key(name) {
            let key = state_key(name, None);
            rows.entry(key.clone())
                .or_insert_with(|| empty_row(Side::Local));
            key_by_name.insert(name.clone(), key);
        }
    }

    for (name, key) in &key_by_name {
        let Some(row) = rows.get_mut(key) else {
            continue;
        };
        if let Some(ids) = distinct.local.get(name) {
            row.seen = count(ids.len());
        }
        set_counts(row, tally.counts(seat, true, name));
        // A discard is named by its events, so it counts without a state.
        let discards = if tally.kept.contains(&seat) {
            &tally.discards_kept
        } else {
            &tally.discards
        };
        if let Some(ids) = discards.get(&(seat, name.clone())) {
            row.discarded = count(ids.len());
        }
    }
    set_zone_counters(tally, seat, &key_by_name, &mut rows);

    if hands.has_opening() {
        for (name, key) in &key_by_name {
            let Some(row) = rows.get_mut(key) else {
                continue;
            };
            row.opening_hand = copies(&hands.opening, name);
            row.bottomed = copies(&hands.bottomed, name);
            row.starting_hand = row.opening_hand.saturating_sub(row.bottomed);
        }
    }
    let mulligans = mulligan_lines(&hands.sent_back, &key_by_name);
    (rows.into_iter().collect(), mulligans)
}

/// The zone counters of the user's rows: distinct ids a state showed in each
/// zone, from the opening hand on (the states after the user's keep, or every
/// state when no keep is known).
fn set_zone_counters(
    tally: &CardTally,
    seat: u32,
    key_by_name: &BTreeMap<String, EndstepCardKey>,
    rows: &mut BTreeMap<EndstepCardKey, GameCardRow>,
) {
    if !tally.has_zone_data(seat) {
        return;
    }
    let from_keep = tally.kept.contains(&seat);
    for row in rows.values_mut() {
        row.has_zone_data = true;
    }
    for (name, key) in key_by_name {
        let (Some(row), Some(seen)) = (rows.get_mut(key), tally.seen.get(&(seat, name.clone())))
        else {
            continue;
        };
        let zones = if from_keep {
            &seen.zones_kept
        } else {
            &seen.zones_all
        };
        row.hand_seen = count(zones.hand.len());
        row.battlefield_seen = count(zones.battlefield.len());
        row.graveyard_seen = count(zones.graveyard.len());
        row.exile_seen = count(zones.exile.len());
    }
}

/// Each name's (sided in, sided out): the game's main quantity against game
/// 1's, more is in and fewer is out. This is Arena's `add_deck` rule
/// (`crates/arena-game/src/summary.rs`) on name keys.
fn sideboarding(
    game_one: &[(String, u32)],
    game: &[(String, u32)],
) -> BTreeMap<String, (u32, u32)> {
    let mut changes: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut before: BTreeMap<&str, u32> = BTreeMap::new();
    let mut after: BTreeMap<&str, u32> = BTreeMap::new();
    for (name, quantity) in game_one {
        before.insert(name, *quantity);
    }
    for (name, quantity) in game {
        after.insert(name, *quantity);
    }
    for name in before.keys().chain(after.keys()) {
        let was = before.get(name).copied().unwrap_or(0);
        let now = after.get(name).copied().unwrap_or(0);
        changes.insert(
            (*name).to_string(),
            (now.saturating_sub(was), was.saturating_sub(now)),
        );
    }
    changes
}

/// A game's quantity per card name, spread over the match deck's keys: each
/// key of the name in match deck order takes up to its own match quantity, and
/// what is left goes on the name's first key (`key_by_name`). Game 1 so gets
/// the match deck's lines exactly.
fn spread_over_keys(
    match_lines: &[(EndstepCardKey, u32)],
    wanted: &[(String, u32)],
    key_by_name: &BTreeMap<String, EndstepCardKey>,
) -> BTreeMap<EndstepCardKey, u32> {
    let mut spread: BTreeMap<EndstepCardKey, u32> = BTreeMap::new();
    for (name, quantity) in wanted {
        let mut left = *quantity;
        for (key, match_quantity) in match_lines {
            if &key.name != name {
                continue;
            }
            let given = left.min(*match_quantity);
            left -= given;
            let total = spread.entry(key.clone()).or_insert(0);
            *total = total.saturating_add(given);
        }
        if left > 0
            && let Some(first) = key_by_name.get(name)
        {
            let total = spread.entry(first.clone()).or_insert(0);
            *total = total.saturating_add(left);
        }
    }
    spread
}

/// How many of `cards` have the name `name`.
fn copies(cards: &[HandCard], name: &str) -> u32 {
    count(cards.iter().filter(|(_, card)| card == name).count())
}

/// The hands sent back as (hand number from 1, card, quantity), sorted.
fn mulligan_lines(
    sent_back: &[Vec<HandCard>],
    key_by_name: &BTreeMap<String, EndstepCardKey>,
) -> MulliganLines {
    let mut lines: BTreeMap<(u32, EndstepCardKey), u32> = BTreeMap::new();
    for (index, hand) in sent_back.iter().enumerate() {
        let number = count(index).saturating_add(1);
        for (_, name) in hand {
            let key = key_by_name
                .get(name)
                .cloned()
                .unwrap_or_else(|| state_key(name, None));
            let quantity = lines.entry((number, key)).or_insert(0);
            *quantity = quantity.saturating_add(1);
        }
    }
    lines
        .into_iter()
        .map(|((number, key), quantity)| (number, key, quantity))
        .collect()
}

/// One row per card name of the other seats that a state, a reveal or a
/// cast showed. The key is the name only: the opponent's cards carry no
/// printing.
fn opponent_rows(
    tally: &CardTally,
    distinct: &DistinctSeen,
    seat: u32,
) -> Vec<(EndstepCardKey, GameCardRow)> {
    let mut rows = Vec::new();
    for (name, ids) in &distinct.opponent {
        if ids.is_empty() {
            continue;
        }
        let mut row = empty_row(Side::Opponent);
        row.seen = count(ids.len());
        set_counts(&mut row, tally.counts(seat, false, name));
        rows.push((state_key(name, None), row));
    }
    rows
}

/// A row with every counter at zero. `played` and `activated` are known
/// (zero) because every Endstep game has its events.
fn empty_row(side: Side) -> GameCardRow {
    GameCardRow {
        side,
        played: Some(0),
        activated: Some(0),
        ..GameCardRow::default()
    }
}

fn set_counts(row: &mut GameCardRow, counts: EventCounts) {
    row.cast = counts.cast;
    row.played = Some(counts.played);
    row.activated = Some(counts.activated);
}

fn state_key(name: &str, printing: Option<&(String, String)>) -> EndstepCardKey {
    let (set_code, collector_number) = match printing {
        Some((set_code, number)) => (set_code.clone(), number.clone()),
        None => (String::new(), String::new()),
    };
    EndstepCardKey {
        name: name.to_string(),
        set_code,
        collector_number,
    }
}

fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, set_code: &str) -> EndstepCardKey {
        EndstepCardKey {
            name: name.to_string(),
            set_code: set_code.to_string(),
            collector_number: "1".to_string(),
        }
    }

    fn two_printing_deck() -> EndstepDeck {
        EndstepDeck {
            key: "endstep:test".to_string(),
            name: "Test".to_string(),
            format: "Modern".to_string(),
            main: vec![
                (key("Forest", "eos"), 4),
                (key("Forest", "m21"), 3),
                (key("Bolt", "lea"), 4),
            ],
            side: vec![(key("Forest", "m21"), 1), (key("Bolt", "lea"), 2)],
        }
    }

    fn qtys(rows: &[(EndstepCardKey, GameCardRow)], name: &str, set_code: &str) -> (u32, u32) {
        let found = rows
            .iter()
            .find(|(key, row)| {
                key.name == name && key.set_code == set_code && row.side == Side::Local
            })
            .expect("row");
        (found.1.main_qty, found.1.side_qty)
    }

    #[test]
    fn game_one_keeps_two_printings_of_a_name_apart() {
        let deck = two_printing_deck();
        let game_one = GameDeck::from_match(&deck);
        let built = game_card_rows(
            &CardTally::default(),
            0,
            Some(&deck),
            GameDecks {
                played: Some(&game_one),
                game_one: None,
            },
            &Hands::default(),
        );
        assert_eq!(qtys(&built.rows, "Forest", "eos"), (4, 0));
        assert_eq!(qtys(&built.rows, "Forest", "m21"), (3, 1));
        assert_eq!(qtys(&built.rows, "Bolt", "lea"), (4, 2));
        let empty = built
            .rows
            .iter()
            .filter(|(_, row)| row.main_qty == 0 && row.side_qty == 0)
            .count();
        assert_eq!(empty, 0);
    }

    #[test]
    fn a_later_game_fills_the_decks_keys_in_order() {
        let deck = two_printing_deck();
        let game_one = GameDeck::from_match(&deck);
        let played = GameDeck {
            main: vec![("Bolt".to_string(), 4), ("Forest".to_string(), 6)],
            side: vec![("Bolt".to_string(), 2), ("Forest".to_string(), 2)],
        };
        let built = game_card_rows(
            &CardTally::default(),
            0,
            Some(&deck),
            GameDecks {
                played: Some(&played),
                game_one: Some(&game_one),
            },
            &Hands::default(),
        );
        // Main 6 fills eos (4) then m21 (2); side 2 fills m21's one side
        // slot and the extra goes on the name's first key.
        assert_eq!(qtys(&built.rows, "Forest", "eos"), (4, 1));
        assert_eq!(qtys(&built.rows, "Forest", "m21"), (2, 1));
        let first = built
            .rows
            .iter()
            .find(|(key, _)| key.name == "Forest" && key.set_code == "eos")
            .expect("row");
        assert_eq!((first.1.sided_in, first.1.sided_out), (0, 1));
    }
}
