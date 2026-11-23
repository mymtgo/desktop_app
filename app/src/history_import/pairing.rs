//! Pairing history records with game log files, and the confidence that a
//! paired match was played with the chosen deck (0.x
//! `MatchAndScoreJob::findMatchingLog` and `ScoreMatchConfidence`). Pure: the
//! scan reads the files and the database and passes the facts in.

use std::collections::HashSet;
use std::path::PathBuf;

use mtgo_history::HistoryRecord;

/// A log pairs with a record when its first entry is at most this far from
/// the record's start time, either way (inclusive).
pub const PAIR_WINDOW_MS: i64 = 5 * 60 * 1000;

/// A game log file that can be paired: one with at least one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateLog {
    pub path: PathBuf,
    /// The match token in its name.
    pub token: String,
    /// Its first entry, UTC ms.
    pub first_entry_ms: i64,
    /// The players its join and roll lines name, in first-seen order.
    pub players: Vec<String>,
}

/// The deck version's cards as oracle ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeckOracles {
    /// The version lists at least one card. 0.x scored nothing for an empty
    /// deck, but a deck whose cards have no known oracle id scores 0.
    pub has_cards: bool,
    pub oracle_ids: HashSet<String>,
}

/// Whether a log's player is the record's opponent: the whole name, any
/// ASCII case (0.x matched with SQLite `LIKE`, which ignores ASCII case).
pub fn same_player(player: &str, opponent: &str) -> bool {
    player.eq_ignore_ascii_case(opponent)
}

/// The record's opponent for pairing: `Opponents[0]`, `None` when there is
/// none or it is blank.
pub fn record_opponent(record: &HistoryRecord) -> Option<&str> {
    record
        .opponents
        .first()
        .map(String::as_str)
        .filter(|name| !name.is_empty())
}

/// For each record, the index in `logs` of the game log it pairs with.
///
/// A candidate is a log whose token no stored match has (`linked`), whose
/// first entry is within `PAIR_WINDOW_MS` of the record's start, and whose
/// players include the record's opponent. The earliest first entry wins,
/// then the smallest path. A token pairs with at most one record: records
/// are paired in order and a used token is skipped (copies of a file share
/// a token).
pub fn pair_records(
    records: &[HistoryRecord],
    logs: &[CandidateLog],
    linked: &HashSet<String>,
) -> Vec<Option<usize>> {
    // Candidates in pairing order: earliest first entry, then smallest path.
    let mut order: Vec<usize> = (0..logs.len()).collect();
    order.sort_by(|&a, &b| {
        (logs[a].first_entry_ms, &logs[a].path).cmp(&(logs[b].first_entry_ms, &logs[b].path))
    });
    let mut used: HashSet<&str> = HashSet::new();
    let mut pairs = Vec::new();
    for record in records {
        let (Some(opponent), Some(start_ms)) = (record_opponent(record), record.start_time_ms)
        else {
            pairs.push(None);
            continue;
        };
        // The first candidate not before the window opens.
        let from = order.partition_point(|&index| {
            logs[index].first_entry_ms < start_ms.saturating_sub(PAIR_WINDOW_MS)
        });
        let mut found = None;
        for &index in &order[from..] {
            let log = &logs[index];
            if log.first_entry_ms > start_ms.saturating_add(PAIR_WINDOW_MS) {
                break;
            }
            let free = !linked.contains(&log.token) && !used.contains(log.token.as_str());
            let has_opponent = log
                .players
                .iter()
                .any(|player| same_player(player, opponent));
            if free && has_opponent {
                found = Some(index);
                break;
            }
        }
        if let Some(index) = found {
            used.insert(logs[index].token.as_str());
        }
        pairs.push(found);
    }
    pairs
}

/// The local player of a paired log: the first player that is not the
/// opponent. A log naming only the opponent names no local player (#180):
/// 0.x fell back to the first player there, storing the opponent as the
/// local account. The import then uses its fallback account and reads no
/// games from the log.
pub fn local_player(players: &[String], opponent: &str) -> Option<String> {
    players
        .iter()
        .find(|player| !same_player(player, opponent))
        .cloned()
}

/// 0.x `ScoreMatchConfidence`: the share of the log's oracle ids that the
/// deck has, rounded to 2 places. `None` when the log resolved no oracle id
/// or the deck has no cards.
pub fn confidence(log_oracle_ids: &HashSet<String>, deck: &DeckOracles) -> Option<f64> {
    if log_oracle_ids.is_empty() || !deck.has_cards {
        return None;
    }
    let overlap = log_oracle_ids
        .iter()
        .filter(|oracle_id| deck.oracle_ids.contains(*oracle_id))
        .count();
    Some(rounded_ratio(overlap, log_oracle_ids.len()))
}

/// `part / whole` rounded half up to 2 places, computed exactly in whole
/// numbers (PHP's `round` gives 0.29 for 57/200, where `(x * 100).round()`
/// on the float would give 0.28).
pub fn rounded_ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    // round(part / whole * 100) = floor((200 * part + whole) / (2 * whole)).
    let hundredths = (200 * part + whole) / (2 * whole);
    hundredths as f64 / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_767_225_600_000;

    fn record(id: u64, start_ms: i64, opponent: &str) -> HistoryRecord {
        HistoryRecord {
            id,
            start_time_ms: Some(start_ms),
            opponents: vec![opponent.to_string()],
            match_winners: Vec::new(),
            match_losers: Vec::new(),
            game_wins: 2,
            game_losses: 0,
            game_wins_to_win: Some(2),
            game_ids: Vec::new(),
            round: 0,
            description: String::new(),
            format: Some("CMODERN".to_string()),
        }
    }

    fn log(path: &str, token: &str, first_ms: i64, players: &[&str]) -> CandidateLog {
        CandidateLog {
            path: PathBuf::from(path),
            token: token.to_string(),
            first_entry_ms: first_ms,
            players: players.iter().map(|player| player.to_string()).collect(),
        }
    }

    fn pairs(records: &[HistoryRecord], logs: &[CandidateLog]) -> Vec<Option<usize>> {
        pair_records(records, logs, &HashSet::new())
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn the_window_is_five_minutes_either_way_inclusive() {
        let records = [record(1, START, "Qopp")];
        for (offset, paired) in [
            (-PAIR_WINDOW_MS - 1000, false),
            (-PAIR_WINDOW_MS, true),
            (0, true),
            (PAIR_WINDOW_MS, true),
            (PAIR_WINDOW_MS + 1000, false),
        ] {
            let logs = [log("a", "t", START + offset, &["Qlocal", "Qopp"])];
            let expected = if paired { Some(0) } else { None };
            assert_eq!(pairs(&records, &logs), vec![expected], "offset {offset}");
        }
    }

    #[test]
    fn the_opponent_matches_in_any_ascii_case_but_only_as_a_whole_name() {
        let logs = [log("a", "t", START, &["Qlocal", "QOpp_x"])];
        assert_eq!(pairs(&[record(1, START, "qopp_X")], &logs), vec![Some(0)]);
        // 0.x's LIKE took `_` and `%` as wildcards; v1 does not.
        assert_eq!(pairs(&[record(1, START, "QOpp%")], &logs), vec![None]);
        assert_eq!(pairs(&[record(1, START, "QOpp__")], &logs), vec![None]);
        assert_eq!(pairs(&[record(1, START, "QOpp_")], &logs), vec![None]);
        assert_eq!(pairs(&[record(1, START, "Opp_x")], &logs), vec![None]);
        assert!(same_player("Qöpp", "Qöpp"));
        assert!(!same_player("QÖPP", "qöpp"), "only ASCII letters fold");
    }

    #[test]
    fn a_record_without_an_opponent_or_a_start_time_pairs_with_nothing() {
        let logs = [log("a", "t", START, &["Qlocal", ""])];
        let mut no_opponent = record(1, START, "");
        assert_eq!(pairs(std::slice::from_ref(&no_opponent), &logs), vec![None]);
        no_opponent.opponents.clear();
        assert_eq!(record_opponent(&no_opponent), None);
        assert_eq!(pairs(&[no_opponent], &logs), vec![None]);

        let logs = [log("a", "t", START, &["Qlocal", "Qopp"])];
        let mut no_time = record(2, START, "Qopp");
        no_time.start_time_ms = None;
        assert_eq!(pairs(&[no_time], &logs), vec![None]);
    }

    #[test]
    fn only_the_first_opponent_is_looked_for() {
        let mut two = record(1, START, "Qfirst");
        two.opponents.push("Qsecond".to_string());
        let logs = [log("a", "t", START, &["Qlocal", "Qsecond"])];
        assert_eq!(pairs(&[two], &logs), vec![None]);
    }

    #[test]
    fn the_earliest_first_entry_wins_then_the_smallest_path() {
        let records = [record(1, START, "Qopp")];
        let logs = [
            log("c", "t3", START + 60_000, &["Qopp"]),
            log("b", "t2", START - 60_000, &["Qopp"]),
            log("a", "t1", START - 60_000, &["Qopp"]),
        ];
        assert_eq!(pairs(&records, &logs), vec![Some(2)]);
    }

    #[test]
    fn a_log_already_linked_to_a_match_is_skipped() {
        let records = [record(1, START, "Qopp")];
        let logs = [
            log("a", "linked", START, &["Qopp"]),
            log("b", "free", START + 1000, &["Qopp"]),
        ];
        assert_eq!(
            pair_records(&records, &logs, &set(&["linked"])),
            vec![Some(1)]
        );
        assert_eq!(
            pair_records(&records, &logs, &set(&["linked", "free"])),
            vec![None]
        );
    }

    #[test]
    fn a_log_pairs_with_one_record_at_most() {
        // 0.x could give both records the same log; v1 gives it to the first.
        let records = [record(1, START, "Qopp"), record(2, START + 30_000, "Qopp")];
        let logs = [log("a", "t1", START, &["Qopp"])];
        assert_eq!(pairs(&records, &logs), vec![Some(0), None]);

        let logs = [
            log("a", "t1", START, &["Qopp"]),
            log("b", "t2", START + 30_000, &["Qopp"]),
        ];
        assert_eq!(pairs(&records, &logs), vec![Some(0), Some(1)]);

        // Two copies of one file share a token: the second copy is used too.
        let copies = [
            log("a", "t1", START, &["Qopp"]),
            log("b", "t1", START, &["Qopp"]),
        ];
        assert_eq!(pairs(&records, &copies), vec![Some(0), None]);
    }

    #[test]
    fn the_local_player_is_the_first_that_is_not_the_opponent() {
        let players =
            |names: &[&str]| -> Vec<String> { names.iter().map(|name| name.to_string()).collect() };
        assert_eq!(
            local_player(&players(&["Qopp", "Qlocal"]), "Qopp").as_deref(),
            Some("Qlocal")
        );
        assert_eq!(
            local_player(&players(&["qopp", "Qlocal"]), "Qopp").as_deref(),
            Some("Qlocal"),
            "the opponent is matched as in pairing"
        );
        assert_eq!(
            local_player(&players(&["Qopp"]), "Qopp"),
            None,
            "a log naming only the opponent names no local player (#180)"
        );
        assert_eq!(
            local_player(&players(&["QOPP", "qopp"]), "Qopp"),
            None,
            "in any case"
        );
        assert_eq!(local_player(&[], "Qopp"), None);
    }

    #[test]
    fn confidence_is_the_share_of_log_cards_in_the_deck_rounded() {
        let deck = DeckOracles {
            has_cards: true,
            oracle_ids: set(&["oracle-a", "oracle-b"]),
        };
        assert_eq!(
            confidence(&set(&["oracle-a", "oracle-b", "oracle-c"]), &deck),
            Some(0.67)
        );
        assert_eq!(
            confidence(&set(&["oracle-a", "oracle-b"]), &deck),
            Some(1.0)
        );
        assert_eq!(confidence(&set(&["oracle-z"]), &deck), Some(0.0));
    }

    #[test]
    fn confidence_is_null_without_log_oracle_ids_or_deck_cards() {
        let deck = DeckOracles {
            has_cards: true,
            oracle_ids: set(&["oracle-a"]),
        };
        assert_eq!(confidence(&HashSet::new(), &deck), None);
        assert_eq!(
            confidence(&set(&["oracle-a"]), &DeckOracles::default()),
            None
        );
        // Cards without known oracle ids: 0.x still scored them, as 0.
        let unknown_cards = DeckOracles {
            has_cards: true,
            oracle_ids: HashSet::new(),
        };
        assert_eq!(confidence(&set(&["oracle-a"]), &unknown_cards), Some(0.0));
    }

    #[test]
    fn rounding_is_half_up_on_the_exact_ratio() {
        assert_eq!(rounded_ratio(2, 3), 0.67);
        assert_eq!(rounded_ratio(1, 3), 0.33);
        assert_eq!(rounded_ratio(1, 8), 0.13);
        assert_eq!(rounded_ratio(57, 200), 0.29);
        assert_eq!(rounded_ratio(1, 40), 0.03);
        assert_eq!(rounded_ratio(0, 7), 0.0);
        assert_eq!(rounded_ratio(7, 7), 1.0);
    }
}
