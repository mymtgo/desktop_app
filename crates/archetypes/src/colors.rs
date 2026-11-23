//! Colour identity (0.x `App\Support\ColorIdentity`, `ResolveCardsFromDek`,
//! the API `DeckColorNamer`).
//!
//! The canonical form is upper-case letters joined with commas ("U,B"), in
//! WUBRG order with colourless last, each letter once. `normalize_colors`
//! lives in `cards` and is re-exported here.

use crate::{ObservedCard, name_key, name_keys};

pub use cards::colors::normalize_colors;

const ORDER: [char; 6] = cards::colors::COLOR_ORDER;

/// The colour identity of a card list: the colour identities of its
/// non-land cards, in WUBRG order (0.x `ResolveCardsFromDek`). A card whose
/// type contains "Land" never counts. Every copy counts, so a single splash
/// card colours the list; `deck_identity` is the server's deck reading.
pub fn colors_of(cards: &[ObservedCard]) -> Option<String> {
    let mut present = [false; 6];
    for card in cards {
        let card_type = card.card_type.as_deref().unwrap_or("");
        if card_type.contains("Land") {
            continue;
        }
        let Some(identity) = card.color_identity.as_deref() else {
            continue;
        };
        for ch in identity.to_uppercase().chars() {
            // Only the five colours: colourless is no colour of a deck.
            if let Some(index) = ORDER[..5].iter().position(|c| *c == ch) {
                present[index] = true;
            }
        }
    }
    join_present(&present)
}

/// Copies of a colour a full list must play before it is one of the deck's
/// colours rather than a splash (the API `DeckColorNamer::MIN_COPIES`).
const MIN_COPIES: u32 = 4;

/// Non-land main copies below which a list is a partial observation
/// (the API `DeckColorNamer::SMALL_SAMPLE_COPIES`): every card seen counts,
/// lands included, from one copy.
const SMALL_SAMPLE_COPIES: u32 = 20;

/// Lands that make a deck five-colour at `MIN_COPIES` main copies (the API
/// `config('mtgoformatdata.rainbow_lands')`), as name keys.
const RAINBOW_LANDS: [&str; 7] = [
    "ancient ziggurat",
    "sliver hive",
    "ally encampment",
    "primal beyond",
    "glimmervoid",
    "spire of industry",
    "cavern of souls",
];

/// A deck's colours from its main deck, as the server names archetypes
/// (the API `DeckColorNamer::identity`), in the canonical form; `None` for
/// a colourless deck.
///
/// - `MIN_COPIES` rainbow lands make it five-colour.
/// - A full list (`SMALL_SAMPLE_COPIES` non-land copies or more) counts its
///   non-land cards, a colour from `MIN_COPIES` copies. A smaller sample
///   counts every card, lands included, a colour from one copy.
/// - A card's colours are those of its mana cost (`castable_colours`), else
///   of its colour identity when it has no cost.
/// - `anchors` are the name keys of the cards the matched rule requires in
///   the main deck: their colours always count, when the card has a cost.
pub fn deck_identity(cards: &[ObservedCard], anchors: &[String]) -> Option<String> {
    let main: Vec<&ObservedCard> = cards.iter().filter(|card| !card.sideboard).collect();

    let mut rainbow: u32 = 0;
    for card in &main {
        if card
            .name
            .as_deref()
            .is_some_and(|name| RAINBOW_LANDS.contains(&name_key(name).as_str()))
        {
            rainbow = rainbow.saturating_add(card.quantity);
        }
    }
    if rainbow >= MIN_COPIES {
        return Some("W,U,B,R,G".to_string());
    }

    let spells: Vec<&ObservedCard> = main.iter().copied().filter(|card| !is_land(card)).collect();
    // Quantities saturate: a crafted list must not overflow the count.
    let spell_copies: u32 = spells
        .iter()
        .fold(0u32, |total, card| total.saturating_add(card.quantity));
    let small_sample = spell_copies < SMALL_SAMPLE_COPIES;
    let (countable, min_copies) = if small_sample {
        (main, 1)
    } else {
        (spells, MIN_COPIES)
    };

    let mut counts = [0u32; 5];
    let mut anchored = [false; 5];
    for card in countable {
        let is_anchor = has_cost(card) && is_anchored(card, anchors);
        for index in castable_colours(card) {
            counts[index] = counts[index].saturating_add(card.quantity);
            if is_anchor {
                anchored[index] = true;
            }
        }
    }

    let mut present = [false; 6];
    for index in 0..5 {
        present[index] = anchored[index] || counts[index] >= min_copies;
    }
    join_present(&present)
}

/// The type names a land (the front face's type, as the API stores it).
fn is_land(card: &ObservedCard) -> bool {
    card.card_type
        .as_deref()
        .is_some_and(|card_type| card_type.contains("Land"))
}

fn has_cost(card: &ObservedCard) -> bool {
    card.mana_cost
        .as_deref()
        .is_some_and(|cost| !cost.is_empty())
}

/// Whether the card's full name or front face is one of `anchors`.
fn is_anchored(card: &ObservedCard, anchors: &[String]) -> bool {
    let Some(name) = &card.name else {
        return false;
    };
    name_keys(name).iter().any(|key| anchors.contains(key))
}

/// The WUBRG indexes the deck pays when casting the card, each once
/// (the API `DeckColorNamer::castableColors`): the coloured symbols of its
/// cost, hybrid symbols both halves, Phyrexian ones (payable with life)
/// none. A cost with no coloured symbol (`{2}`) gives none; a card with no
/// cost gives its colour identity.
fn castable_colours(card: &ObservedCard) -> Vec<usize> {
    let mut present = [false; 5];
    if has_cost(card) {
        let cost = card.mana_cost.as_deref().unwrap_or("");
        for symbol in cost.split('{').skip(1) {
            let Some((inside, _)) = symbol.split_once('}') else {
                continue;
            };
            let parts: Vec<String> = inside.split('/').map(str::to_uppercase).collect();
            if parts.iter().any(|part| part == "P") {
                continue;
            }
            for part in &parts {
                if let Some(index) = colour_index(part) {
                    present[index] = true;
                }
            }
        }
    } else if let Some(identity) = card.color_identity.as_deref() {
        for ch in identity.to_uppercase().chars() {
            if let Some(index) = ORDER[..5].iter().position(|c| *c == ch) {
                present[index] = true;
            }
        }
    }
    (0..5).filter(|index| present[*index]).collect()
}

/// The WUBRG index of a one-letter symbol part.
fn colour_index(part: &str) -> Option<usize> {
    let mut chars = part.chars();
    let letter = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    ORDER[..5].iter().position(|c| *c == letter)
}

fn join_present(present: &[bool; 6]) -> Option<String> {
    let mut letters = Vec::new();
    for (index, is_present) in present.iter().enumerate() {
        if *is_present {
            letters.push(ORDER[index].to_string());
        }
    }
    if letters.is_empty() {
        None
    } else {
        Some(letters.join(","))
    }
}
