//! MTGO's own verdict on a match, from a history record's `MatchWinners`
//! and `MatchLosers`. The live log cannot always say who won (a player who
//! leaves at a level score logs only "Match Tied"), but the history names
//! the winner, so it is the check on a derived outcome.

use crate::HistoryRecord;

/// Who MTGO says won the match, for one player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Win,
    Loss,
}

/// MTGO's verdict for `local`: a win when `MatchWinners` names them, a loss
/// when `MatchLosers` does. Names compare in any ASCII case, as the history
/// pairing does. `None` when neither list names them (a draw has no winner),
/// or both do (a broken record).
pub fn local_verdict(record: &HistoryRecord, local: &str) -> Option<Verdict> {
    let names = |list: &[String]| list.iter().any(|name| name.eq_ignore_ascii_case(local));
    match (names(&record.match_winners), names(&record.match_losers)) {
        (true, false) => Some(Verdict::Win),
        (false, true) => Some(Verdict::Loss),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(winners: &[&str], losers: &[&str]) -> HistoryRecord {
        HistoryRecord {
            id: 1,
            start_time_ms: None,
            opponents: vec!["Qopponent".to_string()],
            match_winners: winners.iter().map(|name| name.to_string()).collect(),
            match_losers: losers.iter().map(|name| name.to_string()).collect(),
            game_wins: 1,
            game_losses: 1,
            game_wins_to_win: Some(2),
            game_ids: vec![10, 11],
            round: 0,
            description: String::new(),
            format: None,
        }
    }

    #[test]
    fn a_level_score_with_a_winner_is_the_winner_s() {
        let left = record(&["Qlocal"], &["Qopponent"]);
        assert_eq!(local_verdict(&left, "Qlocal"), Some(Verdict::Win));
        assert_eq!(local_verdict(&left, "Qopponent"), Some(Verdict::Loss));
    }

    #[test]
    fn names_compare_in_any_ascii_case() {
        let won = record(&["QLocal"], &["Qopponent"]);
        assert_eq!(local_verdict(&won, "qlocal"), Some(Verdict::Win));
    }

    #[test]
    fn no_verdict_without_a_winner_or_with_a_broken_record() {
        assert_eq!(local_verdict(&record(&[], &[]), "Qlocal"), None);
        assert_eq!(local_verdict(&record(&["Qother"], &[]), "Qlocal"), None);
        assert_eq!(
            local_verdict(&record(&["Qlocal"], &["Qlocal"]), "Qlocal"),
            None
        );
    }
}
