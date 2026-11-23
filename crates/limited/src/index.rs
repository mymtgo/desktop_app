//! The Limited index KPI strip (0.x `BuildLimitedIndex::kpis`).

use crate::timings::DraftPickStats;

/// One limited run on the index, after the set, kind and timeframe filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRun {
    pub set_code: Option<String>,
    /// The run's state is Complete.
    pub complete: bool,
    /// Won completed matches.
    pub wins: u32,
    /// Lost completed matches.
    pub losses: u32,
    /// Completed matches.
    pub matches: u32,
    pub has_draft: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LimitedIndexKpis {
    /// Runs plus unlinked drafts.
    pub events: u32,
    /// Runs with a draft plus unlinked drafts.
    pub drafts: u32,
    pub unlinked: u32,
    pub match_win_pct: Option<i64>,
    pub match_wins: u32,
    pub match_losses: u32,
    /// Mean wins per completed run with matches, to one decimal.
    pub avg_wins: Option<f64>,
    pub avg_losses: Option<f64>,
    pub completed_runs: u32,
    /// The most common set code among the runs (ties go to the first run).
    pub most_drafted_set: Option<String>,
    pub most_drafted_count: u32,
    /// Mean of the per-draft average pick times.
    pub avg_pick_seconds: Option<i64>,
    /// Picks with two or more reservations, as a share of picked picks.
    pub indecision_pct: Option<i64>,
}

/// The KPI strip over the filtered runs (newest first), the unlinked drafts
/// count, and the pick stats of every draft shown (`draft_pick_stats` of the
/// runs' drafts and the unlinked drafts).
pub fn limited_index_kpis(
    runs: &[IndexRun],
    unlinked: u32,
    pick_stats: &[DraftPickStats],
) -> LimitedIndexKpis {
    let match_wins: u32 = runs.iter().map(|run| run.wins).sum();
    let match_losses: u32 = runs.iter().map(|run| run.losses).sum();
    let decided = match_wins + match_losses;

    let completed: Vec<&IndexRun> = runs
        .iter()
        .filter(|run| run.complete && run.matches > 0)
        .collect();
    let average = |count: fn(&IndexRun) -> u32| -> Option<f64> {
        if completed.is_empty() {
            return None;
        }
        let total: u32 = completed.iter().map(|run| count(run)).sum();
        Some(round_to_tenth(f64::from(total) / completed.len() as f64))
    };

    let (most_drafted_set, most_drafted_count) = most_common_set(runs);

    let pick_total: u32 = pick_stats.iter().map(|stats| stats.picks).sum();
    let indecisive_total: u32 = pick_stats.iter().map(|stats| stats.indecisive).sum();
    let averages: Vec<i64> = pick_stats
        .iter()
        .filter_map(|stats| stats.avg_seconds)
        .collect();

    LimitedIndexKpis {
        events: runs.len() as u32 + unlinked,
        drafts: runs.iter().filter(|run| run.has_draft).count() as u32 + unlinked,
        unlinked,
        match_win_pct: percent(match_wins, decided),
        match_wins,
        match_losses,
        avg_wins: average(|run| run.wins),
        avg_losses: average(|run| run.losses),
        completed_runs: completed.len() as u32,
        most_drafted_set,
        most_drafted_count,
        avg_pick_seconds: if averages.is_empty() {
            None
        } else {
            let mean = averages.iter().sum::<i64>() as f64 / averages.len() as f64;
            Some(mean.round() as i64)
        },
        indecision_pct: percent(indecisive_total, pick_total),
    }
}

/// The set code most runs have, and how many; a tie goes to the set seen
/// first (0.x's stable `countBy()->sortDesc()`).
fn most_common_set(runs: &[IndexRun]) -> (Option<String>, u32) {
    let mut counts: Vec<(&str, u32)> = Vec::new();
    for set_code in runs.iter().filter_map(|run| run.set_code.as_deref()) {
        match counts.iter_mut().find(|(code, _)| *code == set_code) {
            Some((_, count)) => *count += 1,
            None => counts.push((set_code, 1)),
        }
    }

    let mut best: Option<(&str, u32)> = None;
    for (code, count) in counts {
        if best.is_none_or(|(_, top)| count > top) {
            best = Some((code, count));
        }
    }
    match best {
        Some((code, count)) => (Some(code.to_string()), count),
        None => (None, 0),
    }
}

/// 0.x `BuildLimitedIndex`: `round($part / $whole * 100)` (the `SampleRate`
/// order), `None` when `whole` is 0.
fn percent(part: u32, whole: u32) -> Option<i64> {
    decks::stats::sample_rate_percent(part, whole).map(i64::from)
}

fn round_to_tenth(value: f64) -> f64 {
    decks::php_round(value, 1)
}
