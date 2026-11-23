//! Game results over the anonymised 0.x `.dat` fixtures, ported from 0.x
//! `ExtractGameResultsTest`. Each file's local account has its own fake
//! name (the anonymiser mapped names per file).

use fixture_tools::load_game_log;
use game_detail::{LogEndReason, LogGameResults, extract_game_results, parse_game_log};

fn results(name: &str, local: &str) -> LogGameResults {
    let log = parse_game_log(&load_game_log(name), chrono_tz::UTC).expect("game log parses");
    extract_game_results(&log.entries, local)
}

fn winners(results: &LogGameResults) -> Vec<Option<&str>> {
    results
        .games
        .iter()
        .map(|game| game.winner.as_deref())
        .collect()
}

const CLEAN_LOCAL: &str = "Qbxxxxxxxx";

#[test]
fn extracts_a_clean_2_0_win() {
    let results = results("clean_2_0_win", CLEAN_LOCAL);
    assert_eq!(
        winners(&results),
        vec![Some(CLEAN_LOCAL), Some(CLEAN_LOCAL)]
    );
    assert_eq!(results.games[0].end_reason, LogEndReason::Win);
    assert_eq!(results.match_score, Some((2, 0)));
}

#[test]
fn extracts_a_2_1_win() {
    let results = results("clean_2_1_win", "Qbxxxxxxxx");
    assert_eq!(results.games.len(), 3);
    assert_eq!(results.games[0].winner.as_deref(), Some("Qbxxxxxxxx"));
    assert_ne!(results.games[1].winner.as_deref(), Some("Qbxxxxxxxx"));
    assert_eq!(results.games[2].winner.as_deref(), Some("Qbxxxxxxxx"));
    assert_eq!(results.match_score, Some((2, 1)));
}

#[test]
fn extracts_a_2_1_loss() {
    let results = results("clean_2_1_loss", "Qbxxxxxxxx");
    assert_eq!(results.games.len(), 3);
    assert_eq!(results.match_score, Some((1, 2)));
}

#[test]
fn extracts_results_with_concedes() {
    let results = results("concede_2_0", "Qbxxxxxxxx");
    assert_eq!(
        winners(&results),
        vec![Some("Qbxxxxxxxx"), Some("Qbxxxxxxxx")]
    );
    for game in &results.games {
        assert!(matches!(
            game.end_reason,
            LogEndReason::Win | LogEndReason::Concede
        ));
    }
}

#[test]
fn extracts_results_with_disconnect() {
    let results = results("disconnect_game1", "Qaxxxxxxxx");
    assert_eq!(winners(&results), vec![Some("Qaxxxxxxxx")]);
}

#[test]
fn extracts_instant_concede() {
    let results = results("instant_concede", "Qbxxxxxxxx");
    assert_eq!(winners(&results), vec![Some("Qbxxxxxxxx")]);
    assert!(matches!(
        results.games[0].end_reason,
        LogEndReason::Win | LogEndReason::Concede
    ));
}

#[test]
fn extracts_on_play_name_per_game() {
    let results = results("clean_2_0_win", CLEAN_LOCAL);
    for game in &results.games {
        let on_play = game.on_play.as_ref().expect("on play");
        assert!(results.players.contains(on_play));
    }
}

#[test]
fn extracts_starting_hand_sizes() {
    let results = results("clean_2_0_win", CLEAN_LOCAL);
    assert!(!results.starting_hands.is_empty());
    for hand in &results.starting_hands {
        assert!((1..=7).contains(&hand.value), "{hand:?}");
    }
}

#[test]
fn extracts_player_names() {
    let results = results("clean_2_0_win", CLEAN_LOCAL);
    assert_eq!(results.players.len(), 2);
    assert!(results.players.iter().any(|player| player == CLEAN_LOCAL));
}

#[test]
fn provides_per_game_timestamps() {
    let results = results("clean_2_0_win", CLEAN_LOCAL);
    for game in &results.games {
        assert!(game.started_at_ms.is_some());
        assert!(game.ended_at_ms >= game.started_at_ms);
    }
}

#[test]
fn handles_large_multi_game_files() {
    let results = results("large_2_0_win", "Qbxxxxxxxx");
    assert_eq!(results.games.len(), 2);
    assert_eq!(results.match_score, Some((2, 0)));
}

#[test]
fn returns_player_names_without_the_at_p_prefix() {
    let results = results("clean_2_0_win", CLEAN_LOCAL);
    for player in &results.players {
        assert!(!player.starts_with('@'));
    }
    for game in &results.games {
        if let Some(winner) = &game.winner {
            assert!(!winner.starts_with('@'));
        }
    }
}

#[test]
fn preserves_a_missing_on_play_per_game() {
    let results = results("multi_game_partial_on_play", "Qaxxxxxxxx");
    let on_play: Vec<Option<&str>> = results
        .games
        .iter()
        .map(|game| game.on_play.as_deref())
        .collect();
    assert_eq!(on_play, vec![Some("Qaxxxxxxxx"), Some("Qaxxxxxxxx"), None]);
    let indexes: Vec<usize> = results.games.iter().map(|game| game.game_index).collect();
    assert_eq!(indexes, vec![0, 1, 2]);
}
