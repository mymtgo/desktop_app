//! Per-game results read from a match's game log, port of 0.x
//! `ExtractGameResults::run`: who won each game and how, who was on the
//! play, starting hand sizes, each game's first and last entry times, a
//! deciding game forfeited between games, and the match score. The history
//! import builds an imported match's games from this (Plan M).
//!
//! Games are split with `split_into_games` and players found with
//! `detect_players` (the parts `ExtractCardsFromGameLog` already uses).

use std::sync::LazyLock;

use mtgo_protocol::entries::{
    BEGINS, CHOOSES, CONCEDED, JOINED, MATCH_SCORE, MATCH_TIED, ROLLED, WINS_GAME, hand_size,
    player_re,
};
use regex::Regex;

use crate::gamelog::GameLogEntry;
use crate::log_cards::{PlayerValue, detect_players, split_into_games};

static ANY_GAME_WIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"wins the game").expect("valid regex"));
static MATCH_WIN_LINE: LazyLock<Regex> =
    LazyLock::new(|| player_re(r"^@P{P} wins the match [0-9]+-[0-9]+"));

/// How a game ended, as the log tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEndReason {
    Win,
    Concede,
    /// No win or concede line (a disconnect is not a result).
    Unknown,
}

/// One game read from the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogGameResult {
    /// 0-based, in log order.
    pub game_index: usize,
    pub winner: Option<String>,
    pub loser: Option<String>,
    pub end_reason: LogEndReason,
    /// The player on the play, `None` when the log never says.
    pub on_play: Option<String>,
    /// Each player's starting hand size, in first-seen order.
    pub starting_hands: Vec<PlayerValue>,
    /// The game's first entry, UTC ms.
    pub started_at_ms: Option<i64>,
    /// The game's last entry, UTC ms.
    pub ended_at_ms: Option<i64>,
}

/// What the whole log says about the match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogGameResults {
    pub games: Vec<LogGameResult>,
    pub players: Vec<String>,
    /// (local wins, opponent wins) from the last "leads the match",
    /// "wins the match" or "Match Tied" line.
    pub match_score: Option<(u32, u32)>,
    /// A "wins the match" line exists ("leads the match" is not final).
    pub match_decided: bool,
    /// Every game's starting hands, game by game.
    pub starting_hands: Vec<PlayerValue>,
}

/// Read every game's result from `entries`, scoring for `local_player`.
pub fn extract_game_results(entries: &[GameLogEntry], local_player: &str) -> LogGameResults {
    let players = detect_players(entries);
    let mut games = Vec::new();
    let mut starting_hands = Vec::new();
    for (index, game_entries) in split_into_games(entries).into_iter().enumerate() {
        let game = analyse_game(game_entries, index, &players);
        starting_hands.extend(game.starting_hands.iter().cloned());
        games.push(game);
    }
    if let Some(forfeit) = trailing_forfeit(entries, &games, &players, local_player) {
        games.push(forfeit);
    }
    LogGameResults {
        match_score: match_score(entries, local_player),
        match_decided: entries
            .iter()
            .any(|entry| MATCH_WIN_LINE.is_match(&entry.message)),
        games,
        players,
        starting_hands,
    }
}

/// The first player that is not `player` (0.x `otherPlayer`).
fn other_player(player: &str, players: &[String]) -> Option<String> {
    players.iter().find(|other| *other != player).cloned()
}

/// Set `player`'s value, keeping the order players were first seen in.
fn set_value(values: &mut Vec<PlayerValue>, player: &str, value: u32) {
    match values.iter_mut().find(|kept| kept.player == player) {
        Some(kept) => kept.value = value,
        None => values.push(PlayerValue {
            player: player.to_string(),
            value,
        }),
    }
}

/// One game's facts (0.x `analyzeGame`). A win line decides the game; a
/// concede decides it only when no win line came first; a lost connection
/// never does.
fn analyse_game(entries: &[GameLogEntry], index: usize, players: &[String]) -> LogGameResult {
    let mut game = LogGameResult {
        game_index: index,
        winner: None,
        loser: None,
        end_reason: LogEndReason::Unknown,
        on_play: None,
        starting_hands: Vec::new(),
        started_at_ms: entries.first().map(|entry| entry.timestamp_ms),
        ended_at_ms: entries.last().map(|entry| entry.timestamp_ms),
    };
    for entry in entries {
        let message = &entry.message;
        if let Some(caps) = CHOOSES.captures(message) {
            game.on_play = if &caps[2] == "first" {
                Some(caps[1].to_string())
            } else {
                other_player(&caps[1], players)
            };
        }
        if let Some(caps) = BEGINS.captures(message)
            && let Some(size) = hand_size(&caps[2])
        {
            set_value(&mut game.starting_hands, &caps[1], size);
        }
        if let Some(caps) = WINS_GAME.captures(message) {
            game.winner = Some(caps[1].to_string());
            game.loser = other_player(&caps[1], players);
            game.end_reason = LogEndReason::Win;
        }
        if let Some(caps) = CONCEDED.captures(message)
            && game.winner.is_none()
        {
            game.loser = Some(caps[1].to_string());
            game.winner = other_player(&caps[1], players);
            game.end_reason = LogEndReason::Concede;
        }
    }
    game
}

/// A deciding game forfeited between games (0.x `detectTrailingForfeit`):
/// at a level score, a concede after the last win line with no new game
/// started is a lost game for the player who conceded. A lost connection
/// is not (the player may reconnect).
fn trailing_forfeit(
    entries: &[GameLogEntry],
    games: &[LogGameResult],
    players: &[String],
    local_player: &str,
) -> Option<LogGameResult> {
    games.last()?.winner.as_ref()?;
    let mut wins = 0;
    let mut losses = 0;
    for game in games {
        match &game.winner {
            Some(winner) if winner == local_player => wins += 1,
            Some(_) => losses += 1,
            None => {}
        }
    }
    if wins == 0 || wins != losses {
        return None;
    }
    let last_win = entries
        .iter()
        .rposition(|entry| ANY_GAME_WIN.is_match(&entry.message))?;
    let mut forfeit: Option<(String, i64)> = None;
    for entry in &entries[last_win + 1..] {
        let message = &entry.message;
        if ROLLED.is_match(message) || JOINED.is_match(message) {
            // A new game started: splitting already has it.
            return None;
        }
        if let Some(caps) = CONCEDED.captures(message) {
            forfeit = Some((caps[1].to_string(), entry.timestamp_ms));
        }
    }
    let (leaver, at_ms) = forfeit?;
    Some(LogGameResult {
        game_index: games.len(),
        winner: other_player(&leaver, players),
        loser: Some(leaver),
        end_reason: LogEndReason::Concede,
        on_play: None,
        starting_hands: Vec::new(),
        started_at_ms: Some(at_ms),
        ended_at_ms: Some(at_ms),
    })
}

/// The last match score line as (local, opponent) (0.x `extractMatchScore`).
fn match_score(entries: &[GameLogEntry], local_player: &str) -> Option<(u32, u32)> {
    let mut score = None;
    for entry in entries {
        let message = &entry.message;
        if let Some(caps) = MATCH_SCORE.captures(message)
            && let (Ok(first), Ok(second)) = (caps[3].parse::<u32>(), caps[4].parse::<u32>())
        {
            score = if &caps[1] == local_player {
                Some((first, second))
            } else {
                Some((second, first))
            };
        }
        if let Some(caps) = MATCH_TIED.captures(message)
            && let (Ok(first), Ok(second)) = (caps[1].parse::<u32>(), caps[2].parse::<u32>())
        {
            score = Some((first, second));
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(second: i64, message: &str) -> GameLogEntry {
        GameLogEntry {
            timestamp_ms: 1_767_225_600_000 + second * 1000,
            message: message.to_string(),
        }
    }

    fn lines(list: &[(i64, &str)]) -> Vec<GameLogEntry> {
        list.iter()
            .map(|(second, message)| entry(*second, message))
            .collect()
    }

    fn winners(results: &LogGameResults) -> Vec<Option<&str>> {
        results
            .games
            .iter()
            .map(|game| game.winner.as_deref())
            .collect()
    }

    fn has_hand(game: &LogGameResult, player: &str) -> bool {
        game.starting_hands.iter().any(|hand| hand.player == player)
    }

    // Ported from 0.x ExtractGameResultsTest (synthetic cases), with fake
    // names: the local player is `Qlocal`, the opponents `Qopp` and names
    // with a hyphen, a dot and a space.

    #[test]
    fn does_not_fabricate_a_decisive_third_game_from_a_between_games_disconnect_at_1_1() {
        let entries = lines(&[
            (0, "@PQlocal rolled a 5."),
            (0, "@PQopp rolled a 1."),
            (1, "@P@PQlocal joined the game."),
            (1, "@P@PQopp joined the game."),
            (2, "@PQlocal has conceded from the game."),
            (2, "@PQopp wins the game."),
            (60, "@P@PQlocal joined the game."),
            (60, "@P@PQopp joined the game."),
            (62, "@PQopp has conceded from the game."),
            (62, "@PQlocal wins the game."),
            (120, "@PQopp has lost connection to the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(winners(&results), vec![Some("Qopp"), Some("Qlocal")]);
    }

    #[test]
    fn does_not_declare_a_game_winner_from_a_lone_disconnect() {
        let entries = lines(&[
            (0, "@PQlocal rolled a 5."),
            (0, "@PQopp rolled a 1."),
            (1, "@P@PQlocal joined the game."),
            (1, "@P@PQopp joined the game."),
            (2, "@PQopp has lost connection to the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(winners(&results), vec![None]);
        assert_eq!(results.games[0].end_reason, LogEndReason::Unknown);
    }

    #[test]
    fn does_not_split_a_single_game_into_two_when_a_player_disconnects_then_reconnects() {
        let entries = lines(&[
            (0, "@PQlocal rolled a 5."),
            (0, "@PQopp rolled a 1."),
            (1, "@P@PQlocal joined the game."),
            (1, "@P@PQopp joined the game."),
            (2, "@PQopp has lost connection to the game."),
            (30, "@PQlocal wins the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(winners(&results), vec![Some("Qlocal")]);
    }

    #[test]
    fn counts_a_local_between_games_concede_at_1_1_as_a_decisive_loss() {
        let entries = lines(&[
            (0, "@PQlocal rolled a 5."),
            (0, "@PQopp rolled a 1."),
            (1, "@P@PQlocal joined the game."),
            (1, "@P@PQopp joined the game."),
            (2, "@PQopp has conceded from the game."),
            (2, "@PQlocal wins the game."),
            (60, "@P@PQlocal joined the game."),
            (60, "@P@PQopp joined the game."),
            (62, "@PQlocal has conceded from the game."),
            (62, "@PQopp wins the game."),
            (120, "@PQlocal has conceded from the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(results.games.len(), 3);
        let forfeit = &results.games[2];
        assert_eq!(forfeit.game_index, 2);
        assert_eq!(forfeit.winner.as_deref(), Some("Qopp"));
        assert_eq!(forfeit.loser.as_deref(), Some("Qlocal"));
        assert_eq!(forfeit.end_reason, LogEndReason::Concede);
        assert_eq!(forfeit.on_play, None);
        assert!(forfeit.starting_hands.is_empty());
        let at = Some(entry(120, "").timestamp_ms);
        assert_eq!((forfeit.started_at_ms, forfeit.ended_at_ms), (at, at));
    }

    #[test]
    fn a_trailing_concede_when_a_player_is_ahead_is_not_a_game() {
        let entries = lines(&[
            (0, "@P@PQlocal joined the game."),
            (0, "@P@PQopp joined the game."),
            (2, "@PQlocal wins the game."),
            (3, "@PQopp has conceded from the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(winners(&results), vec![Some("Qlocal")]);
    }

    #[test]
    fn detects_players_with_hyphens_in_usernames() {
        let entries = lines(&[
            (0, "@PQab-Cd rolled a 5."),
            (0, "@PQlocal rolled a 3."),
            (0, "@P@PQab-Cd joined the game."),
            (0, "@P@PQlocal joined the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(results.players, vec!["Qab-Cd", "Qlocal"]);
    }

    #[test]
    fn extracts_game_results_for_players_with_hyphens() {
        let entries = lines(&[
            (0, "@P@PQab-Cd joined the game."),
            (0, "@P@PQlocal joined the game."),
            (1, "@PQab-Cd chooses to play first."),
            (2, "@PQab-Cd begins the game with seven cards in hand."),
            (2, "@PQlocal begins the game with seven cards in hand."),
            (3, "@PQlocal wins the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        let game = &results.games[0];
        assert_eq!(game.winner.as_deref(), Some("Qlocal"));
        assert_eq!(game.on_play.as_deref(), Some("Qab-Cd"));
        assert!(has_hand(game, "Qab-Cd"));
    }

    #[test]
    fn detects_players_with_dots_in_usernames() {
        let entries = lines(&[
            (0, "@PQa.bcd rolled a 5."),
            (0, "@PQopp rolled a 3."),
            (0, "@P@PQa.bcd joined the game."),
            (0, "@P@PQopp joined the game."),
        ]);
        let results = extract_game_results(&entries, "Qa.bcd");
        assert_eq!(results.players, vec!["Qa.bcd", "Qopp"]);
    }

    #[test]
    fn credits_wins_to_a_local_player_with_a_dotted_username() {
        let entries = lines(&[
            (0, "@P@PQa.bcd joined the game."),
            (0, "@P@PQopp joined the game."),
            (1, "@PQa.bcd chooses to play first."),
            (2, "@PQa.bcd begins the game with seven cards in hand."),
            (2, "@PQopp begins the game with seven cards in hand."),
            (3, "@PQa.bcd wins the game."),
            (4, "@PQa.bcd rolled a 6."),
            (4, "@P@PQopp joined the game."),
            (5, "@PQopp chooses to play first."),
            (6, "@PQa.bcd wins the game."),
            (7, "@PQa.bcd wins the match 2-0"),
        ]);
        let results = extract_game_results(&entries, "Qa.bcd");
        assert_eq!(winners(&results), vec![Some("Qa.bcd"), Some("Qa.bcd")]);
        assert_eq!(results.games[0].on_play.as_deref(), Some("Qa.bcd"));
        assert!(has_hand(&results.games[0], "Qa.bcd"));
        assert_eq!(results.match_score, Some((2, 0)));
        assert!(results.match_decided);
    }

    #[test]
    fn credits_a_concede_win_to_a_dotted_local_player() {
        let entries = lines(&[
            (0, "@P@PQa.bcd joined the game."),
            (0, "@P@PQopp joined the game."),
            (1, "@PQopp has conceded from the game."),
        ]);
        let results = extract_game_results(&entries, "Qa.bcd");
        assert_eq!(results.games[0].winner.as_deref(), Some("Qa.bcd"));
        assert_eq!(results.games[0].end_reason, LogEndReason::Concede);
    }

    #[test]
    fn detects_players_with_spaces_in_usernames() {
        let entries = lines(&[
            (0, "@PQab Cd rolled a 3."),
            (0, "@PQlocal rolled a 4."),
            (0, "@P@PQab Cd joined the game."),
            (0, "@P@PQlocal joined the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(results.players, vec!["Qab Cd", "Qlocal"]);
    }

    #[test]
    fn resolves_game_results_against_a_spaced_opponent() {
        let entries = lines(&[
            (0, "@P@PQab Cd joined the game."),
            (0, "@P@PQlocal joined the game."),
            (1, "@PQab Cd chooses to play first."),
            (2, "@PQab Cd begins the game with seven cards in hand."),
            (2, "@PQlocal begins the game with seven cards in hand."),
            (3, "@PQlocal wins the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        let game = &results.games[0];
        assert_eq!(game.winner.as_deref(), Some("Qlocal"));
        assert_eq!(game.on_play.as_deref(), Some("Qab Cd"));
        assert!(has_hand(game, "Qab Cd"));
    }

    // Extra v1 cases for lines the 0.x tests reach only through fixtures.

    #[test]
    fn playing_second_puts_the_other_player_on_the_play() {
        let entries = lines(&[
            (0, "@P@PQlocal joined the game."),
            (0, "@P@PQopp joined the game."),
            (1, "@PQopp chooses to play second."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(results.games[0].on_play.as_deref(), Some("Qlocal"));
    }

    #[test]
    fn hand_sizes_read_words_and_digits_and_skip_anything_else() {
        let entries = lines(&[
            (0, "@P@PQlocal joined the game."),
            (0, "@P@PQopp joined the game."),
            (1, "@PQlocal begins the game with Six cards in hand."),
            (1, "@PQopp begins the game with 5 cards in hand."),
            (2, "@PQlocal begins the game with one card in hand."),
            (2, "@PQopp begins the game with zero cards in hand."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(
            results.games[0].starting_hands,
            vec![
                PlayerValue {
                    player: "Qlocal".to_string(),
                    value: 1,
                },
                PlayerValue {
                    player: "Qopp".to_string(),
                    value: 5,
                },
            ]
        );
        assert_eq!(results.starting_hands, results.games[0].starting_hands);
    }

    #[test]
    fn a_hand_after_bottoming_belongs_to_the_player_not_the_bottoming_clause() {
        let entries = lines(&[
            (0, "@P@PQab Cd joined the game."),
            (0, "@P@PQopp joined the game."),
            (
                1,
                "@PQab Cd puts a card on the bottom of their library and begins the game with six cards in hand.",
            ),
            (1, "@PQopp begins the game with seven cards in hand."),
        ]);
        let results = extract_game_results(&entries, "Qab Cd");
        assert_eq!(
            results.games[0].starting_hands,
            vec![
                PlayerValue {
                    player: "Qab Cd".to_string(),
                    value: 6,
                },
                PlayerValue {
                    player: "Qopp".to_string(),
                    value: 7,
                },
            ]
        );
    }

    #[test]
    fn a_win_line_is_not_overridden_by_a_later_concede() {
        let entries = lines(&[
            (0, "@P@PQlocal joined the game."),
            (0, "@P@PQopp joined the game."),
            (1, "@PQopp wins the game."),
            (2, "@PQopp has conceded from the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        let game = &results.games[0];
        assert_eq!(game.winner.as_deref(), Some("Qopp"));
        assert_eq!(game.loser.as_deref(), Some("Qlocal"));
        assert_eq!(game.end_reason, LogEndReason::Win);
    }

    #[test]
    fn the_score_is_the_last_match_line_from_the_local_side() {
        let entries = lines(&[
            (0, "@P@PQlocal joined the game."),
            (0, "@P@PQopp joined the game."),
            (1, "@PQopp leads the match 1-0"),
            (2, "Match Tied 1-1"),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        assert_eq!(results.match_score, Some((1, 1)));
        assert!(!results.match_decided);

        let opponent_ahead = extract_game_results(&entries[..3], "Qlocal");
        assert_eq!(opponent_ahead.match_score, Some((0, 1)));
        assert_eq!(
            extract_game_results(&entries[..2], "Qlocal").match_score,
            None
        );
    }

    #[test]
    fn games_carry_their_first_and_last_entry_times() {
        let entries = lines(&[
            (0, "@P@PQlocal joined the game."),
            (0, "@P@PQopp joined the game."),
            (9, "@PQlocal wins the game."),
            (60, "@PQlocal rolled a 2."),
            (75, "@PQopp wins the game."),
        ]);
        let results = extract_game_results(&entries, "Qlocal");
        let times: Vec<(Option<i64>, Option<i64>)> = results
            .games
            .iter()
            .map(|game| (game.started_at_ms, game.ended_at_ms))
            .collect();
        let at = |second: i64| Some(entry(second, "").timestamp_ms);
        assert_eq!(times, vec![(at(0), at(9)), (at(60), at(75))]);
    }

    #[test]
    fn no_entries_give_no_games() {
        let results = extract_game_results(&[], "Qlocal");
        assert!(results.games.is_empty());
        assert!(results.players.is_empty());
        assert_eq!(results.match_score, None);
    }
}
