//! Which colours looked open during a draft (0.x `ComputeDraftSignals`).

use std::collections::BTreeMap;

use crate::cards::{COLORS, CardFactsMap, colour_letters};
use crate::wheel::SeenFacts;

/// One colour's signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSignal {
    pub color: char,
    /// Distinct cards of this colour that wheeled.
    pub wheeled: u32,
    /// Distinct cards of this colour seen in two or more packs.
    pub seen_twice: u32,
    pub score: u32,
    /// Score relative to the top colour (1.0 for the leader), to 2 decimals.
    pub share: f64,
}

/// Per colour, how many distinct cards wheeled or were seen twice or more,
/// ranked by score (ties keep WUBRG order). Multicolour cards count once per
/// colour; unknown cards count for no colour.
pub fn draft_signals(seen: &BTreeMap<u32, SeenFacts>, cards: &CardFactsMap) -> Vec<ColorSignal> {
    let mut signals: Vec<ColorSignal> = COLORS
        .iter()
        .map(|&color| ColorSignal {
            color,
            wheeled: 0,
            seen_twice: 0,
            score: 0,
            share: 0.0,
        })
        .collect();

    for (&catalog_id, facts) in seen {
        for letter in colour_letters(cards, catalog_id) {
            let Some(signal) = signals.iter_mut().find(|signal| signal.color == letter) else {
                continue;
            };
            if facts.wheeled {
                signal.wheeled += 1;
            }
            if facts.seen_count >= 2 {
                signal.seen_twice += 1;
            }
        }
    }

    for signal in &mut signals {
        signal.score = signal.wheeled + signal.seen_twice;
    }
    // Stable, so equal scores keep WUBRG order (PHP 8 sorts are stable).
    signals.sort_by_key(|signal| std::cmp::Reverse(signal.score));

    let top = signals.first().map_or(1, |signal| signal.score.max(1));
    for signal in &mut signals {
        let share = f64::from(signal.score) / f64::from(top);
        signal.share = decks::php_round(share, 2);
    }
    signals
}
