//! A full state in the record's terms: the cards it shows (`Shown`) and each
//! seat's numbers.

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{CardType, Zone};
use endstep_protocol::{GameState, PlayerState, StateCard, Zone as StateZone};

use super::MAX_OBJECTS;
use super::board::SEATS;
use super::objects::Permanent;

/// The most cards read into a hidden hand from one `handSize` (hostile
/// input: the number sizes a loop).
const MAX_HAND: u32 = 100;

/// One card as a full state shows it, in the record's terms. What a
/// permanent has is empty off the battlefield, as it is for a reader.
pub(super) struct Shown {
    pub(super) forge_id: u64,
    pub(super) name: String,
    pub(super) owner: u32,
    pub(super) controller: u32,
    pub(super) zone: Zone,
    pub(super) token: bool,
    pub(super) permanent: Permanent,
    pub(super) counters: BTreeMap<String, u32>,
    pub(super) attached_forge: Option<u64>,
}

pub(super) fn player_of(state: &GameState, seat: u32) -> Option<&PlayerState> {
    state
        .player_states
        .iter()
        .find(|player| player.seat == seat)
}

/// A seat's `handSize`, capped.
pub(super) fn hand_size(state: &GameState, seat: u32) -> Option<u32> {
    player_of(state, seat)
        .and_then(|player| player.hand_size)
        .map(|size| size.min(MAX_HAND))
}

/// The cards of a state the record tracks, each Forge id once: cards with
/// types (a Forge effect object has none) owned by seat 0 or 1, at most
/// `MAX_OBJECTS` of them, counted after those filters (hostile input: one
/// state must not make the diff scan without bound, and padding the record
/// skips must not push real cards out). Stack cards come last, the bottom of the stack
/// first, so they are put on the stack in the order they were cast.
pub(super) fn shown_cards(state: &GameState) -> Vec<Shown> {
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut cards = Vec::new();
    let mut stack = Vec::new();
    for card in &state.cards {
        if cards.len() + stack.len() >= MAX_OBJECTS {
            break;
        }
        let Some(owner) = card.owner_seat.filter(|seat| SEATS.contains(seat)) else {
            continue;
        };
        if !card.has_types || !seen.insert(card.id) {
            continue;
        }
        let shown = shown_card(card, owner);
        if shown.zone == Zone::Stack {
            stack.push(shown);
        } else {
            cards.push(shown);
        }
    }
    stack.reverse();
    cards.extend(stack);
    cards
}

fn shown_card(card: &StateCard, owner: u32) -> Shown {
    let zone = zone_of(card.zone);
    let on_battlefield = zone == Zone::Battlefield;
    let types = if on_battlefield {
        card_types(&card.types)
    } else {
        Vec::new()
    };
    // Forge sends 0/0 on every card that is not a creature.
    let pt = types
        .contains(&CardType::Creature)
        .then(|| (card.power.unwrap_or(0), card.toughness.unwrap_or(0)));
    let counters = if on_battlefield {
        named_counters(&card.counters)
    } else {
        BTreeMap::new()
    };
    Shown {
        forge_id: card.id,
        name: card.name.clone(),
        owner,
        controller: card
            .controller_seat
            .filter(|seat| SEATS.contains(seat))
            .unwrap_or(owner),
        zone,
        token: card.is_token,
        permanent: Permanent {
            tapped: on_battlefield && card.tapped,
            types,
            pt,
        },
        counters,
        attached_forge: card.attached_to.filter(|_| on_battlefield),
    }
}

fn zone_of(zone: StateZone) -> Zone {
    match zone {
        StateZone::Hand => Zone::Hand,
        StateZone::Battlefield => Zone::Battlefield,
        StateZone::Graveyard => Zone::Graveyard,
        StateZone::Exile => Zone::Exile,
        StateZone::Stack => Zone::Stack,
        StateZone::Command => Zone::Command,
        StateZone::Library => Zone::Library,
    }
}

/// A permanent's card types, in the standard's order. Forge's other type
/// words (`Instant`, `Sorcery`) are not permanent types.
fn card_types(types: &[String]) -> Vec<CardType> {
    let known = [
        ("Artifact", CardType::Artifact),
        ("Battle", CardType::Battle),
        ("Creature", CardType::Creature),
        ("Enchantment", CardType::Enchantment),
        ("Kindred", CardType::Kindred),
        ("Land", CardType::Land),
        ("Planeswalker", CardType::Planeswalker),
    ];
    let mut out = Vec::new();
    for (word, card_type) in known {
        if types.iter().any(|name| name == word) {
            out.push(card_type);
        }
    }
    out
}

/// Counters above zero by the standard's name: lower case, with Forge's
/// `P1P1` and `M1M1` as `+1/+1` and `-1/-1`. No capture has shown a counter
/// yet, so the other names are Forge's own words in lower case.
fn named_counters(counters: &[(String, u32)]) -> BTreeMap<String, u32> {
    let mut out: BTreeMap<String, u32> = BTreeMap::new();
    for (name, count) in counters {
        if *count == 0 {
            continue;
        }
        let name = match name.to_uppercase().as_str() {
            "P1P1" => "+1/+1".to_string(),
            "M1M1" => "-1/-1".to_string(),
            other => other.to_lowercase().replace('_', " "),
        };
        let total = out.entry(name).or_insert(0);
        *total = total.saturating_add(*count);
    }
    out
}
