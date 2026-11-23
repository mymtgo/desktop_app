//! League runs as the home dashboard's bars (dashboard redesign spec
//! section 4.2): a bar's height is the share of a full run won, so a 5-0
//! constructed run, a 3-0 draft and a 6-0 sealed run are all full height.

use crate::fold::RunState;

/// The height of a Dropped run's bar: small and fixed, whatever its record.
pub const DROPPED_BAR_HEIGHT: f64 = 0.1;

/// How a bar is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarTone {
    /// A Complete run that won every round (gold).
    Trophy,
    /// A Complete run with more wins than losses (green).
    Winning,
    /// Any other Complete run (red).
    Losing,
    /// The player dropped it (grey, small).
    Dropped,
    /// The app does not know how it ended (grey, hatched).
    Partial,
    /// Still being played (outlined).
    Active,
}

/// One bar: its height (0 to 1 of the bar area) and its tone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunBar {
    pub height: f64,
    pub tone: BarTone,
}

/// The bar of a run with `wins` and `losses` of `rounds`
/// (`LeagueKind::round_count`): height `wins / rounds`, at most 1.
pub fn run_bar(state: RunState, rounds: u32, wins: u32, losses: u32) -> RunBar {
    let height = if rounds == 0 {
        0.0
    } else {
        (f64::from(wins) / f64::from(rounds)).min(1.0)
    };
    let tone = match state {
        RunState::Active => BarTone::Active,
        RunState::Partial => BarTone::Partial,
        RunState::Dropped => {
            return RunBar {
                height: DROPPED_BAR_HEIGHT,
                tone: BarTone::Dropped,
            };
        }
        RunState::Complete if rounds > 0 && wins >= rounds => BarTone::Trophy,
        RunState::Complete if wins > losses => BarTone::Winning,
        RunState::Complete => BarTone::Losing,
    };
    RunBar { height, tone }
}

/// What `runs_summary` reads of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarRun {
    pub state: RunState,
    pub rounds: u32,
    pub wins: u32,
}

/// The League results header's numbers (`14 leagues · 1 trophy · 2.4 avg
/// wins`; the UI counts the runs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunsSummary {
    /// Complete runs that won every round.
    pub trophies: u32,
    /// The average wins of the Complete runs, to one decimal; `None` without
    /// one.
    pub avg_wins: Option<f64>,
}

pub fn runs_summary(runs: &[BarRun]) -> RunsSummary {
    let mut trophies = 0;
    let mut complete = 0u32;
    let mut wins = 0u32;
    for run in runs {
        if run.state != RunState::Complete {
            continue;
        }
        complete += 1;
        wins += run.wins;
        if run.rounds > 0 && run.wins >= run.rounds {
            trophies += 1;
        }
    }
    let avg_wins =
        (complete > 0).then(|| decks::php_round(f64::from(wins) / f64::from(complete), 1));
    RunsSummary { trophies, avg_wins }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_run_is_full_height_whatever_its_kind() {
        for rounds in [5, 3] {
            assert_eq!(
                run_bar(RunState::Complete, rounds, rounds, 0),
                RunBar {
                    height: 1.0,
                    tone: BarTone::Trophy
                },
                "{rounds}"
            );
        }
    }

    #[test]
    fn complete_runs_are_coloured_by_their_finish() {
        assert_eq!(
            run_bar(RunState::Complete, 5, 4, 1),
            RunBar {
                height: 0.8,
                tone: BarTone::Winning
            }
        );
        assert_eq!(run_bar(RunState::Complete, 3, 1, 2).tone, BarTone::Losing);
        assert_eq!(
            run_bar(RunState::Complete, 5, 3, 3).tone,
            BarTone::Losing,
            "an even finish is not a winning one"
        );
    }

    #[test]
    fn dropped_partial_and_active_runs_have_their_own_tones() {
        assert_eq!(
            run_bar(RunState::Dropped, 5, 4, 0),
            RunBar {
                height: DROPPED_BAR_HEIGHT,
                tone: BarTone::Dropped
            }
        );
        assert_eq!(
            run_bar(RunState::Partial, 3, 1, 0),
            RunBar {
                height: 1.0 / 3.0,
                tone: BarTone::Partial
            }
        );
        assert_eq!(
            run_bar(RunState::Active, 5, 2, 1),
            RunBar {
                height: 0.4,
                tone: BarTone::Active
            }
        );
    }

    #[test]
    fn odd_records_never_overflow_or_divide_by_zero() {
        assert_eq!(run_bar(RunState::Complete, 5, 7, 0).height, 1.0);
        assert_eq!(
            run_bar(RunState::Complete, 0, 0, 0),
            RunBar {
                height: 0.0,
                tone: BarTone::Losing
            }
        );
    }

    #[test]
    fn the_summary_counts_trophies_and_averages_complete_runs() {
        let runs = [
            BarRun {
                state: RunState::Complete,
                rounds: 5,
                wins: 5,
            },
            BarRun {
                state: RunState::Complete,
                rounds: 3,
                wins: 1,
            },
            BarRun {
                state: RunState::Complete,
                rounds: 5,
                wins: 2,
            },
            BarRun {
                state: RunState::Active,
                rounds: 5,
                wins: 4,
            },
            BarRun {
                state: RunState::Dropped,
                rounds: 5,
                wins: 0,
            },
        ];
        assert_eq!(
            runs_summary(&runs),
            RunsSummary {
                trophies: 1,
                avg_wins: Some(2.7)
            }
        );
        assert_eq!(
            runs_summary(&[]),
            RunsSummary {
                trophies: 0,
                avg_wins: None
            }
        );
    }
}
