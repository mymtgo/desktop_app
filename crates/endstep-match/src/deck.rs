//! The deck the user played: from `GET /api/decks/<id>` when it was
//! captured, else from the match body's card names.

use std::collections::BTreeMap;

use endstep_protocol::{DeckBody, DeckLine, EndstepCardKey, MatchBody};

use crate::EndstepDeck;

/// The deck from its deck body. A line without a printing takes the one in
/// the match body's `myDeckArt`.
pub(crate) fn deck_from_body(deck: &DeckBody, body: &MatchBody) -> EndstepDeck {
    let format = deck
        .format_id
        .clone()
        .or_else(|| body.format_id.clone())
        .unwrap_or_default();
    EndstepDeck {
        key: format!("endstep:{}", deck.id),
        name: deck.name.clone(),
        format,
        main: body_lines(&deck.main, &body.my_deck_art),
        side: body_lines(&deck.side, &body.my_deck_art),
    }
}

/// The deck from `myDeck` and `mySideboard` (one name per copy), with
/// printings from `myDeckArt`.
pub(crate) fn deck_from_names(body: &MatchBody) -> EndstepDeck {
    EndstepDeck {
        key: crate::match_deck_key(&body.id),
        name: "Endstep deck".to_string(),
        format: body.format_id.clone().unwrap_or_default(),
        main: name_lines(&body.my_deck, &body.my_deck_art),
        side: name_lines(&body.my_sideboard, &body.my_deck_art),
    }
}

fn body_lines(lines: &[DeckLine], art: &[(String, String, String)]) -> Vec<(EndstepCardKey, u32)> {
    let mut counts: BTreeMap<EndstepCardKey, u32> = BTreeMap::new();
    for line in lines {
        // An empty set code or collector number counts as missing.
        let set_code = line.set_code.as_ref().filter(|text| !text.is_empty());
        let number = line
            .collector_number
            .as_ref()
            .filter(|text| !text.is_empty());
        let key = match (set_code, number) {
            (Some(set_code), Some(number)) => EndstepCardKey {
                name: line.name.clone(),
                set_code: set_code.to_lowercase(),
                collector_number: number.clone(),
            },
            _ => key_from_art(&line.name, art),
        };
        let count = counts.entry(key).or_insert(0);
        *count = count.saturating_add(line.quantity);
    }
    counts.into_iter().collect()
}

fn name_lines(names: &[String], art: &[(String, String, String)]) -> Vec<(EndstepCardKey, u32)> {
    let mut counts: BTreeMap<EndstepCardKey, u32> = BTreeMap::new();
    for name in names {
        let count = counts.entry(key_from_art(name, art)).or_insert(0);
        *count = count.saturating_add(1);
    }
    counts.into_iter().collect()
}

/// The card's printing from `myDeckArt` (keyed by lower-case name); empty
/// set code and collector number when it has none.
fn key_from_art(name: &str, art: &[(String, String, String)]) -> EndstepCardKey {
    let lower = name.to_lowercase();
    for (art_name, set_code, number) in art {
        if *art_name == lower {
            return EndstepCardKey {
                name: name.to_string(),
                set_code: set_code.to_lowercase(),
                collector_number: number.clone(),
            };
        }
    }
    EndstepCardKey {
        name: name.to_string(),
        set_code: String::new(),
        collector_number: String::new(),
    }
}
