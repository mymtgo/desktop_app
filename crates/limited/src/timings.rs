//! How long picks took (0.x `ComputePickTimings`, and the per-draft pick
//! times of `BuildLimitedIndex::pickTimings`).

use crate::draft::{Draft, Pick};

/// A pick with this many reservations or more was an indecisive one.
pub const INDECISIVE_RESERVATIONS: usize = 2;

const SECOND_MS: i64 = 1000;

/// One pick's timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickTiming {
    /// Whole seconds from shown to picked; `None` without both times.
    pub elapsed_seconds: Option<i64>,
    /// Whole seconds from picked to the deadline, negative when late.
    pub margin_seconds: Option<i64>,
    pub reservation_count: usize,
    pub indecisive: bool,
}

/// A draft's timing summary for the draft review header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingSummary {
    pub avg_seconds: Option<i64>,
    pub avg_margin_seconds: Option<i64>,
    pub indecisive_count: usize,
    /// The pack with the lowest average pick time.
    pub fastest_pack: Option<u32>,
    /// The pack with the highest average pick time.
    pub slowest_pack: Option<u32>,
}

/// One draft's pick times for the Limited index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPickStats {
    /// Mean seconds from shown to picked, rounded.
    pub avg_seconds: Option<i64>,
    /// Picked picks with two or more reservations.
    pub indecisive: u32,
    /// Picks with a picked time.
    pub picks: u32,
}

/// One pick's elapsed time, margin and indecision (0.x `forPick`). Seconds
/// are truncated towards zero, as 0.x's `(int)` cast.
pub fn pick_timing(pick: &Pick) -> PickTiming {
    let elapsed_seconds = match (pick.shown_at_ms, pick.picked_at_ms) {
        (Some(shown), Some(picked)) => Some((picked - shown).abs() / SECOND_MS),
        _ => None,
    };
    let margin_seconds = match (pick.picked_at_ms, pick.deadline_at_ms) {
        (Some(picked), Some(deadline)) => Some((deadline - picked) / SECOND_MS),
        _ => None,
    };
    let reservation_count = pick.reservations.len();

    PickTiming {
        elapsed_seconds,
        margin_seconds,
        reservation_count,
        indecisive: reservation_count >= INDECISIVE_RESERVATIONS,
    }
}

/// The draft's average pick time and margin, indecisive picks and fastest and
/// slowest pack (0.x `summary`). `picks` in ordinal order.
pub fn timing_summary(picks: &[Pick]) -> TimingSummary {
    let mut elapsed: Vec<i64> = Vec::new();
    let mut margins: Vec<i64> = Vec::new();
    // (pack, elapsed seconds of its timed picks), packs in first-seen order.
    let mut by_pack: Vec<(u32, Vec<i64>)> = Vec::new();
    let mut indecisive_count = 0;

    for pick in picks {
        let timing = pick_timing(pick);
        if timing.indecisive {
            indecisive_count += 1;
        }
        if let Some(margin) = timing.margin_seconds {
            margins.push(margin);
        }
        if let Some(seconds) = timing.elapsed_seconds {
            elapsed.push(seconds);
            match by_pack
                .iter_mut()
                .find(|(pack, _)| *pack == pick.pack_number)
            {
                Some((_, times)) => times.push(seconds),
                None => by_pack.push((pick.pack_number, vec![seconds])),
            }
        }
    }

    let mut pack_averages: Vec<(u32, f64)> = by_pack
        .iter()
        .map(|(pack, times)| (*pack, mean(times)))
        .collect();
    // Stable, so packs with equal averages keep first-seen order.
    pack_averages.sort_by(|a, b| a.1.total_cmp(&b.1));

    TimingSummary {
        avg_seconds: rounded_mean(&elapsed),
        avg_margin_seconds: rounded_mean(&margins),
        indecisive_count,
        fastest_pack: pack_averages.first().map(|(pack, _)| *pack),
        slowest_pack: pack_averages.last().map(|(pack, _)| *pack),
    }
}

/// A draft's pick time and indecision over its picked picks (0.x
/// `BuildLimitedIndex::pickTimings`), or `None` when no pick has a picked
/// time (0.x has no entry for such a draft).
pub fn draft_pick_stats(draft: &Draft) -> Option<DraftPickStats> {
    let picked: Vec<&Pick> = draft
        .picks
        .iter()
        .filter(|pick| pick.picked_at_ms.is_some())
        .collect();
    if picked.is_empty() {
        return None;
    }

    // Fractional seconds here, as 0.x averages Carbon's float differences.
    let seconds: Vec<f64> = picked
        .iter()
        .filter_map(|pick| match (pick.shown_at_ms, pick.picked_at_ms) {
            (Some(shown), Some(at)) => Some((at - shown).abs() as f64 / SECOND_MS as f64),
            _ => None,
        })
        .collect();
    let avg_seconds = if seconds.is_empty() {
        None
    } else {
        Some((seconds.iter().sum::<f64>() / seconds.len() as f64).round() as i64)
    };
    let indecisive = picked
        .iter()
        .filter(|pick| pick.reservations.len() >= INDECISIVE_RESERVATIONS)
        .count();

    Some(DraftPickStats {
        avg_seconds,
        indecisive: indecisive as u32,
        picks: picked.len() as u32,
    })
}

fn mean(values: &[i64]) -> f64 {
    values.iter().sum::<i64>() as f64 / values.len() as f64
}

/// The mean rounded half away from zero (PHP `round`), `None` when empty.
fn rounded_mean(values: &[i64]) -> Option<i64> {
    if values.is_empty() {
        None
    } else {
        Some(mean(values).round() as i64)
    }
}
