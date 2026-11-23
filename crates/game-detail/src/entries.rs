//! One game's log lines for display, port of 0.x `GetGameLogEntries`: the
//! match's game log (the `.dat` file, else the MetaMessage fallback) cut to
//! the game's window and cleaned of MTGO's markup. The caller formats the
//! time in the viewer's timezone.

use std::sync::LazyLock;

use regex::Regex;

use crate::gamelog::GameLogEntry;

/// Seconds of slack around a game's start and end.
const WINDOW_SLACK_SECONDS: i64 = 5;

static CARD_MARKUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@\[([^@]+)@:[^\]]+@\]").expect("valid regex"));

/// The entries of one game: from `dat_entries` when it has any, else from
/// `meta_entries`, kept when their second falls between the game's start
/// minus 5 seconds and its end plus 5 seconds (whole seconds, as 0.x
/// compared them), with messages cleaned. Empty when the game has no start
/// or no end.
pub fn game_log_entries(
    dat_entries: &[GameLogEntry],
    meta_entries: &[GameLogEntry],
    game_started_ms: Option<i64>,
    game_ended_ms: Option<i64>,
) -> Vec<GameLogEntry> {
    let (Some(started_ms), Some(ended_ms)) = (game_started_ms, game_ended_ms) else {
        return Vec::new();
    };
    let source = if dat_entries.is_empty() {
        meta_entries
    } else {
        dat_entries
    };
    let first_second = seconds(started_ms) - WINDOW_SLACK_SECONDS;
    let last_second = seconds(ended_ms) + WINDOW_SLACK_SECONDS;

    let mut out = Vec::new();
    for entry in source {
        let second = seconds(entry.timestamp_ms);
        if second >= first_second && second <= last_second {
            out.push(GameLogEntry {
                timestamp_ms: entry.timestamp_ms,
                message: clean_message(&entry.message),
            });
        }
    }
    out
}

/// A log message without MTGO's markup: the `@P` player markers go, and a
/// card reference `@[Name@:123,45:@]` becomes its name.
pub fn clean_message(message: &str) -> String {
    let without_prefix = message
        .strip_prefix("@P@P")
        .or_else(|| message.strip_prefix("@P"))
        .unwrap_or(message);
    let without_markers = without_prefix.replace("@P", "");
    CARD_MARKUP.replace_all(&without_markers, "$1").into_owned()
}

/// Unix seconds of a millisecond instant, rounded down.
fn seconds(ms: i64) -> i64 {
    ms.div_euclid(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-05-26 10:00:00 UTC.
    const TEN_AM: i64 = 1_779_789_600_000;

    fn at(seconds: i64) -> i64 {
        TEN_AM + seconds * 1000
    }

    fn entry(timestamp_ms: i64, message: &str) -> GameLogEntry {
        GameLogEntry {
            timestamp_ms,
            message: message.to_string(),
        }
    }

    fn messages(entries: &[GameLogEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.message.as_str()).collect()
    }

    #[test]
    fn returns_entries_within_the_game_time_window() {
        let meta = vec![
            entry(at(1), "@PPlayerA rolled a 1."),
            entry(at(20), "@PPlayerA rolled a 2."),
            entry(at(30), "@PPlayerA rolled a 3."),
            entry(at(60), "@PPlayerA rolled a 4."),
        ];
        let entries = game_log_entries(&[], &meta, Some(at(15)), Some(at(35)));
        assert_eq!(
            messages(&entries),
            vec!["PlayerA rolled a 2.", "PlayerA rolled a 3."]
        );
    }

    #[test]
    fn the_window_has_five_seconds_of_slack_each_side() {
        let meta = vec![
            entry(at(9), "@PPlayerA rolled a 1."),
            entry(at(10), "@PPlayerA rolled a 2."),
            entry(at(40), "@PPlayerA rolled a 3."),
            entry(at(41), "@PPlayerA rolled a 4."),
        ];
        let entries = game_log_entries(&[], &meta, Some(at(15)), Some(at(35)));
        assert_eq!(
            messages(&entries),
            vec!["PlayerA rolled a 2.", "PlayerA rolled a 3."]
        );
    }

    #[test]
    fn the_window_compares_whole_seconds() {
        let meta = vec![entry(at(40) + 999, "@PPlayerA rolled a 3.")];
        let entries = game_log_entries(&[], &meta, Some(at(15)), Some(at(35) + 500));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn reads_the_decoded_game_log_once_the_log_events_are_gone() {
        let dat = vec![
            entry(at(-2), "@P@PPlayerA joined the game."),
            entry(at(12 * 60 + 4), "@PPlayerA casts Karn Liberated."),
            entry(at(29 * 60 + 50), "@PPlayerA wins the game."),
            entry(at(64 * 60), "@PPlayerA wins the game."),
        ];
        let entries = game_log_entries(&dat, &[], Some(at(0)), Some(at(30 * 60)));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].message, "PlayerA casts Karn Liberated.");
        assert_eq!(entries[1].timestamp_ms, at(12 * 60 + 4));
    }

    #[test]
    fn prefers_the_decoded_game_log_over_meta_messages() {
        let meta = vec![entry(at(10), "@PPlayerA rolled a 2.")];
        let dat = vec![
            entry(at(10), "@PPlayerA rolled a 2."),
            entry(at(12 * 60 + 4), "@PPlayerA casts Karn Liberated."),
        ];
        let entries = game_log_entries(&dat, &meta, Some(at(0)), Some(at(30 * 60)));
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn returns_nothing_for_an_imported_match_with_no_log() {
        assert!(game_log_entries(&[], &[], Some(at(0)), Some(at(30 * 60))).is_empty());
    }

    #[test]
    fn strips_the_at_p_prefix_from_messages() {
        let meta = vec![entry(at(10), "@PPlayerA rolled a 2.")];
        let entries = game_log_entries(&[], &meta, Some(at(0)), Some(at(30 * 60)));
        assert_eq!(entries[0].message, "PlayerA rolled a 2.");
    }

    #[test]
    fn returns_nothing_when_the_game_has_no_start_or_end() {
        let meta = vec![entry(at(10), "@PPlayerA rolled a 2.")];
        assert!(game_log_entries(&[], &meta, None, None).is_empty());
        assert!(game_log_entries(&[], &meta, Some(at(0)), None).is_empty());
        assert!(game_log_entries(&[], &meta, None, Some(at(30))).is_empty());
    }

    #[test]
    fn cleans_markers_and_card_references() {
        assert_eq!(
            clean_message("@P@PPlayerA joined the game."),
            "PlayerA joined the game."
        );
        assert_eq!(
            clean_message("@PPlayerA casts @[Guide of Souls@:251350,424:@] targeting @PPlayerB."),
            "PlayerA casts Guide of Souls targeting PlayerB."
        );
        assert_eq!(
            clean_message(
                "@PPlayerA reveals 2 cards with @[Seer@:1,2:@]'s ability: @[Bolt@:3,4:@] and @[Fire // Ice@:5,6:@]."
            ),
            "PlayerA reveals 2 cards with Seer's ability: Bolt and Fire // Ice."
        );
        assert_eq!(clean_message("@PTurn 3: PlayerA"), "Turn 3: PlayerA");
        assert_eq!(clean_message("Match Tied 1-1"), "Match Tied 1-1");
    }
}
