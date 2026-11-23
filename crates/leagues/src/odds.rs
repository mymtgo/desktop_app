//! Finish odds for a run in progress (0.x `LeagueFinishOdds` and
//! `GetLeagueInProgress::winProbability`).

use decks::php_round;
use decks::stats::Record;

use crate::ev::net_tix;

/// Where a run in progress is likely to end.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishOdds {
    pub rounds_left: u32,
    /// The fewest wins that pay (net tix above zero); `None` for an unpriced
    /// format, as are the fields below.
    pub prize_wins: Option<u32>,
    /// Wins still needed to reach `prize_wins`.
    pub wins_for_prize: Option<u32>,
    /// Chance of reaching `prize_wins`, a whole percentage.
    pub chance_of_prize: Option<u32>,
    /// Expected net tix over every reachable finish, to 2 places.
    pub expected_tix: Option<f64>,
}

/// Which record gave the win probability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OddsSource {
    /// The deck's league matches (at least `MINIMUM_LEAGUE_MATCHES`).
    League,
    /// The deck's whole record.
    Deck,
    /// No record: a coin flip.
    None,
}

impl OddsSource {
    /// `league`, `deck` or `none` (0.x `oddsSource`).
    pub fn as_str(self) -> &'static str {
        match self {
            OddsSource::League => "league",
            OddsSource::Deck => "deck",
            OddsSource::None => "none",
        }
    }
}

/// League matches a deck needs before its league record predicts its runs.
pub const MINIMUM_LEAGUE_MATCHES: u32 = 10;

/// The finish odds of a run at `wins`-`losses` out of `rounds`, each round
/// won with `win_probability` (a plain binomial: a league is always played
/// out). `format` is a format label, as for `net_tix`.
pub fn finish_odds(
    format: &str,
    wins: u32,
    losses: u32,
    rounds: u32,
    win_probability: f64,
) -> FinishOdds {
    let rounds_left = rounds.saturating_sub(wins + losses);
    let Some(prize_wins) = prize_threshold(format, rounds) else {
        return FinishOdds {
            rounds_left,
            prize_wins: None,
            wins_for_prize: None,
            chance_of_prize: None,
            expected_tix: None,
        };
    };

    let mut chance = 0.0;
    let mut expected = 0.0;
    for extra_wins in 0..=rounds_left {
        let probability = binomial(rounds_left, extra_wins)
            * win_probability.powi(extra_wins as i32)
            * (1.0 - win_probability).powi((rounds_left - extra_wins) as i32);
        let final_wins = wins + extra_wins;
        let final_losses = losses + rounds_left - extra_wins;
        if final_wins >= prize_wins {
            chance += probability;
        }
        expected += probability * net_tix(format, final_wins, final_losses).unwrap_or(0.0);
    }

    FinishOdds {
        rounds_left,
        prize_wins: Some(prize_wins),
        wins_for_prize: Some(prize_wins.saturating_sub(wins)),
        chance_of_prize: Some((chance * 100.0).round() as u32),
        expected_tix: Some(php_round(expected, 2)),
    }
}

/// The per-round win probability for a deck's run, from the deck's league
/// record when it has at least `MINIMUM_LEAGUE_MATCHES` league matches, else
/// its whole record, else a coin flip. Rates are 0.x's whole percentages
/// (`MatchRecord::winrate`), so 10-4 gives 0.71.
pub fn win_probability(league: &Record, overall: &Record) -> (f64, OddsSource) {
    if league.total >= MINIMUM_LEAGUE_MATCHES {
        return (f64::from(league.win_rate) / 100.0, OddsSource::League);
    }
    if overall.total == 0 {
        return (0.5, OddsSource::None);
    }
    (f64::from(overall.win_rate) / 100.0, OddsSource::Deck)
}

/// The fewest wins with positive net tix, `None` when the table has nothing
/// for this format.
fn prize_threshold(format: &str, rounds: u32) -> Option<u32> {
    for wins in 0..=rounds {
        let net = net_tix(format, wins, rounds - wins)?;
        if net > 0.0 {
            return Some(wins);
        }
    }
    None
}

/// `n` choose `k`, as a float.
fn binomial(n: u32, k: u32) -> f64 {
    let mut result = 1.0;
    for i in 1..=k {
        result = result * f64::from(n - k + i) / f64::from(i);
    }
    result
}
