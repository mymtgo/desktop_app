use super::*;
use crate::history_import::scan::ScanOutcome;
use fixture_tools::{TestDir, load_game_log};
use game_detail::{extract_game_results, extract_log_cards, parse_game_log};

/// The local player of the clean 2-1 win fixture.
const LOCAL: &str = "Qbxxxxxxxx";

fn entries(name: &str) -> Vec<GameLogEntry> {
    parse_game_log(&load_game_log(name), chrono_tz::UTC)
        .expect("game log parses")
        .entries
}

/// A scanned 2-1 win against Qopponent in round 1, unpaired.
fn row(history_id: u64) -> HistoryScanMatch {
    HistoryScanMatch {
        history_id,
        started_at_ms: Some(1_767_000_000_000),
        opponent: "Qopponent".to_string(),
        format: "CMODERN".to_string(),
        format_display: "Modern".to_string(),
        games_won: 2,
        games_lost: 1,
        outcome: ScanOutcome::Win,
        game_log_token: None,
        game_log_path: None,
        confidence: None,
        round: 1,
        description: "Q league".to_string(),
        game_ids: vec![11, 12, 13],
        local_player: None,
    }
}

#[test]
fn a_paired_log_gives_each_game_s_result_side_and_history_game_id() {
    let entries = entries("clean_2_1_win");
    let games = games_from_log(&entries, LOCAL, &[11, 12]);
    let results = extract_game_results(&entries, LOCAL);
    let cards = extract_log_cards(&entries);
    assert_eq!(games.len(), 3);
    let shown: Vec<(u32, Option<i64>, &str)> = games
        .iter()
        .map(|game| (game.game_number, game.mtgo_game_id, game.result.as_str()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (1, Some(11), "win"),
            (2, Some(12), "loss"),
            (3, None, "win")
        ],
        "game ids by index; a game past the history's list has none"
    );
    for (game, read) in games.iter().zip(&results.games) {
        assert_eq!(game.started_at_ms, read.started_at_ms);
        assert_eq!(game.ended_at_ms, read.ended_at_ms);
        assert_eq!(
            game.on_play,
            read.on_play.as_ref().map(|player| player == LOCAL)
        );
        assert_ne!(game.end_reason, "unknown", "each game was decided");
        let meta = &cards.games[read.game_index].meta;
        assert_eq!(game.turn_count, meta.turn_count.map(i64::from));
        assert_eq!(
            game.local_mulligans,
            Some(value_of(&meta.mulligans, LOCAL).unwrap_or(0))
        );
        assert_eq!(game.local_die_roll, value_of(&meta.dice_rolls, LOCAL));
    }
    assert!(
        games.iter().any(|game| game.on_play == Some(true)),
        "the local player was on the play in some game"
    );
}

#[test]
fn a_game_without_a_winner_is_unknown_and_concedes_say_so() {
    let lines = |last: &str| -> Vec<GameLogEntry> {
        [
            "@PQlocal rolled a 5.",
            "@PQopp rolled a 1.",
            "@P@PQlocal joined the game.",
            "@P@PQopp joined the game.",
            last,
        ]
        .iter()
        .enumerate()
        .map(|(index, message)| GameLogEntry {
            timestamp_ms: 1_000 * index as i64,
            message: message.to_string(),
        })
        .collect()
    };
    let games = games_from_log(
        &lines("@PQopp has lost connection to the game."),
        "Qlocal",
        &[77],
    );
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].result, "unknown");
    assert_eq!(games[0].end_reason, "unknown");
    assert_eq!(games[0].mtgo_game_id, Some(77));
    assert_eq!(games[0].local_die_roll, Some(5));
    assert_eq!(games[0].opp_die_roll, Some(1));
    assert_eq!(games[0].on_play, None, "the log never says");

    let games = games_from_log(&lines("@PQopp has conceded from the game."), "Qlocal", &[]);
    assert_eq!(games.len(), 1);
    assert_eq!(
        (games[0].result.as_str(), games[0].end_reason.as_str()),
        ("win", "concede")
    );
    assert_eq!(games[0].mtgo_game_id, None);

    let entries = entries("concede_2_0");
    let games = games_from_log(&entries, LOCAL, &[]);
    assert!(games.iter().all(|game| game.result == "win"));
    // Scored for a player who is not in the log: nobody won as them.
    let games = games_from_log(&entries, "Qnobody", &[]);
    assert!(games.iter().all(|game| game.result == "loss"));
    assert!(games_from_log(&[], LOCAL, &[1]).is_empty());
}

#[test]
fn an_unpaired_row_is_stored_gameless_under_the_fallback_account() {
    let found = history_match(&row(501), 7, Some("Qlocal"), chrono_tz::UTC).expect("a match");
    assert_eq!(
        found,
        HistoryMatch {
            token: None,
            mtgo_match_id: 501,
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: "League".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 1_767_000_000_000,
            ended_at_ms: None,
            games_won: 2,
            games_lost: 1,
            games: Vec::new(),
            deck_version_id: 7,
            log: Vec::new(),
        }
    );

    let mut casual = row(502);
    casual.round = 0;
    casual.opponent = "Unknown".to_string();
    casual.format = String::new();
    casual.outcome = ScanOutcome::Draw;
    let found = history_match(&casual, 7, Some("Qlocal"), chrono_tz::UTC).expect("a match");
    assert_eq!(found.match_type, "Constructed");
    assert_eq!(
        found.opponent, None,
        "the scan's placeholder is no opponent"
    );
    assert_eq!(found.format, None);
    assert_eq!(found.outcome, "draw");

    assert_eq!(
        history_match(&row(503), 7, None, chrono_tz::UTC),
        None,
        "no account to store it under"
    );
    let mut timeless = row(504);
    timeless.started_at_ms = None;
    assert_eq!(
        history_match(&timeless, 7, Some("Qlocal"), chrono_tz::UTC),
        None,
        "no start time"
    );
}

#[test]
fn a_paired_row_takes_the_log_token_player_and_games() {
    let dir = TestDir::new("history-import-paired-row");
    let path = dir.write("mtgo/Match_GameLog_q.dat", &load_game_log("clean_2_1_win"));
    let mut paired = row(505);
    paired.game_log_token = Some("q-log-token".to_string());
    paired.game_log_path = Some(path.to_string_lossy().into_owned());
    paired.local_player = Some(LOCAL.to_string());
    let found = history_match(&paired, 7, Some("Qlocal"), chrono_tz::UTC).expect("a match");
    assert_eq!(found.token.as_deref(), Some("q-log-token"));
    assert_eq!(found.local_username, LOCAL, "the log's local player");
    assert_eq!(found.games.len(), 3);
    assert_eq!(found.ended_at_ms, found.games[2].ended_at_ms);
    assert!(found.ended_at_ms.is_some());

    // The file is gone: the token stays, the games are lost.
    std::fs::remove_file(&path).expect("remove");
    let found = history_match(&paired, 7, Some("Qlocal"), chrono_tz::UTC).expect("a match");
    assert_eq!(found.token.as_deref(), Some("q-log-token"));
    assert!(found.games.is_empty());
    assert_eq!(found.ended_at_ms, None);
    assert!(read_log(&path, chrono_tz::UTC).is_empty());
}
