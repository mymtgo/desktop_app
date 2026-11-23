//! The constructed league prize table (0.x `LeagueEvTable`).
//!
//! Formats are format labels (`Modern`, any case), as 0.x passes
//! `MtgoFormat::display($league->format)`; a raw code (`CMODERN`) is not
//! priced, so callers pass `decks::display_format(code)`.

use crate::fold::RunState;

/// Net tix per finish of a 5-round league with a 10 tix entry, the same in
/// every priced format.
const SCORE_EV: [(u32, u32, f64); 6] = [
    (5, 0, 29.02),
    (4, 1, 12.70),
    (3, 2, 2.62),
    (2, 3, -5.00),
    (1, 4, -10.00),
    (0, 5, -10.00),
];

const SUPPORTED_FORMATS: [&str; 6] = [
    "modern", "pioneer", "legacy", "standard", "vintage", "pauper",
];

/// Every constructed league costs the same to enter.
const ENTRY_COST: f64 = 10.0;

/// The prize table as `(wins, losses, net tix)`, best finish first.
pub fn ev_table() -> Vec<(u32, u32, f64)> {
    SCORE_EV.to_vec()
}

fn is_priced(format: &str) -> bool {
    let key = format.to_lowercase();
    SUPPORTED_FORMATS.contains(&key.as_str())
}

/// What one league entry costs, `None` for a format the table does not
/// price.
pub fn entry_cost(format: &str) -> Option<f64> {
    is_priced(format).then_some(ENTRY_COST)
}

/// The average wins a run needs before it stops costing money: between the
/// last losing finish and the first paying one, interpolated across the gap
/// and rounded to one place. `None` for an unpriced format.
pub fn break_even_wins(format: &str, rounds: u32) -> Option<f64> {
    for wins in 0..=rounds {
        let net = net_tix(format, wins, rounds - wins)?;
        if net <= 0.0 {
            continue;
        }
        if wins == 0 {
            return Some(0.0);
        }
        let below = net_tix(format, wins - 1, rounds - wins + 1).unwrap_or(0.0);
        let crossing = f64::from(wins - 1) + below.abs() / (below.abs() + net);
        return Some(decks::php_round(crossing, 1));
    }
    None
}

/// Net tix for a run, reading its state as well as its record. A dropped
/// run is paid as though every unplayed round was a loss; a run in progress
/// (or Partial) has no figure, nor does a complete run without matches (an
/// empty manual entry, not a 0-0 finish).
pub fn net_tix_for_run(format: &str, state: RunState, wins: u32, losses: u32) -> Option<f64> {
    match state {
        RunState::Dropped => net_tix(format, wins, losses.max(5u32.saturating_sub(wins))),
        RunState::Complete if wins + losses > 0 => net_tix(format, wins, losses),
        _ => None,
    }
}

/// Net tix for a finish, `None` for an unpriced format or a score the table
/// does not hold.
pub fn net_tix(format: &str, wins: u32, losses: u32) -> Option<f64> {
    if !is_priced(format) {
        return None;
    }
    SCORE_EV
        .iter()
        .find(|(w, l, _)| *w == wins && *l == losses)
        .map(|(_, _, net)| *net)
}
