//! The game log over real (anonymised) MTGO files: the 0.x `.dat` fixtures
//! (`fixtures/gamelogs`, anonymised with `anonymise_dat`) and the
//! MetaMessage fallback over the fixture `mtgo.log` files. Expected values
//! come from 0.x's tests and from an independent Python read of the files.

use chrono::{DateTime, Datelike};
use fixture_tools::{load_fixture, load_game_log, prepared_events};
use game_detail::{
    Counter, GameLog, GameLogEntry, MetaEvent, extract_log_cards, game_log_entries,
    meta_message_entries, parse_game_log, split_into_games,
};
use mtgo_protocol::EventKind;

const ALL: [&str; 8] = [
    "clean_2_0_win",
    "clean_2_1_loss",
    "clean_2_1_win",
    "concede_2_0",
    "disconnect_game1",
    "instant_concede",
    "large_2_0_win",
    "multi_game_partial_on_play",
];

fn parsed(name: &str) -> GameLog {
    parse_game_log(&load_game_log(name), chrono_tz::UTC).expect("game log parses")
}

// Ported from 0.x ParseGameLogBinaryTest.

#[test]
fn parses_a_clean_2_0_win_file() {
    let log = parsed("clean_2_0_win");
    assert_eq!(log.match_uuid, "6a0f564b-f27a-42c0-acea-464f7929342b");
    assert_eq!(log.game_uuid, "6a0f564b-f27a-42c0-acea-464f7929342b");
    assert_eq!(log.version, 1);
    assert_eq!(log.kind, 4);
    assert_eq!(log.entries.len(), 253);
}

#[test]
fn parses_a_large_multi_game_file() {
    assert_eq!(parsed("large_2_0_win").entries.len(), 600);
}

#[test]
fn parses_an_instant_concede_file() {
    assert_eq!(parsed("instant_concede").entries.len(), 8);
}

#[test]
fn every_fixture_has_the_original_entry_count() {
    // Counts of the original 0.x files: anonymising changed no record.
    let expected = [253, 250, 329, 140, 45, 8, 600, 194];
    for (name, count) in ALL.iter().zip(expected) {
        let raw = load_game_log(name);
        let log = parse_game_log(&raw, chrono_tz::UTC).expect("parses");
        assert_eq!(log.entries.len(), count, "{name}");
    }
}

#[test]
fn entries_have_a_timestamp_and_a_message() {
    let log = parsed("instant_concede");
    assert!(log.entries[0].message.contains("rolled a"));
}

#[test]
fn produces_valid_timestamps() {
    let log = parsed("clean_2_0_win");
    let first = DateTime::from_timestamp_millis(log.entries[0].timestamp_ms).expect("instant");
    assert!((2025..=2027).contains(&first.year()));
    // Independent read: the first record's ticks are 2025-12-17 13:42:33.
    assert_eq!(first.to_rfc3339(), "2025-12-17T13:42:33+00:00");
}

#[test]
fn a_file_cut_in_half_reads_the_whole_records_before_the_cut() {
    let raw = load_game_log("clean_2_0_win");
    let full = parse_game_log(&raw, chrono_tz::UTC).expect("parses");
    let half = parse_game_log(&raw[..raw.len() / 2], chrono_tz::UTC).expect("parses");
    assert!(half.entries.len() < full.entries.len());
    assert!(!half.entries.is_empty());
    assert_eq!(half.entries[..], full.entries[..half.entries.len()]);
}

#[test]
fn handles_messages_longer_than_127_bytes_with_varint_length() {
    let log = parsed("clean_2_1_win");
    assert_eq!(log.entries.len(), 329);
    let long: Vec<&GameLogEntry> = log
        .entries
        .iter()
        .filter(|e| e.message.len() > 127)
        .collect();
    assert!(!long.is_empty());
    for entry in long {
        assert!(
            entry.message.ends_with(['.', ')', ']', '!']),
            "{}",
            entry.message
        );
    }
}

#[test]
fn converts_local_wall_clock_ticks_to_utc_with_the_timezone_given() {
    let raw = load_game_log("clean_2_0_win");
    let utc = parse_game_log(&raw, chrono_tz::UTC).expect("parses");
    let la = parse_game_log(&raw, chrono_tz::America::Los_Angeles).expect("parses");
    let hours = (la.entries[0].timestamp_ms - utc.entries[0].timestamp_ms) / 3_600_000;
    // December: Los Angeles is UTC-8.
    assert_eq!(hours, 8);
}

// Ported from 0.x ExtractCardsFromGameLogTest (the fixture cases).

#[test]
fn extracts_cards_for_each_player_from_game_log_entries() {
    let result = extract_log_cards(&parsed("clean_2_0_win").entries);
    assert!(!result.players.is_empty());
    let first = &result.match_cards(&result.players[0])[0];
    assert!(first.mtgo_id > 0);
    assert!(!first.name.is_empty());
}

#[test]
fn returns_players_for_instant_concede_games() {
    let result = extract_log_cards(&parsed("instant_concede").entries);
    assert_eq!(result.players.len(), 2);
    assert_eq!(result.games.len(), 1);
    assert_eq!(result.games[0].meta.turn_count, None);
}

#[test]
fn deduplicates_cards_by_mtgo_id_per_player() {
    for name in ALL {
        let result = extract_log_cards(&parsed(name).entries);
        for player in &result.players {
            assert_unique(result.match_cards(player), name);
            for index in 0..result.games.len() {
                assert_unique(result.game_cards(index, player), name);
            }
        }
    }
}

fn assert_unique(cards: &[game_detail::GameLogCard], name: &str) {
    let mut ids: Vec<u32> = cards.iter().map(|c| c.mtgo_id).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before, "{name}");
}

#[test]
fn returns_per_game_cards_split_by_game_boundaries() {
    let result = extract_log_cards(&parsed("clean_2_1_win").entries);
    assert_eq!(result.games.len(), 3);
    for game in &result.games {
        assert_eq!(game.cards.len(), 2);
    }
}

#[test]
fn counts_casts_and_plays() {
    let result = extract_log_cards(&parsed("clean_2_0_win").entries);
    let cards = result.match_cards(&result.players[0]);
    assert!(cards.iter().any(|c| c.count(Counter::Cast) > 0));
    assert!(cards.iter().any(|c| c.count(Counter::Played) > 0));
}

// v1: game facts cross-checked with an independent read of each file.

/// One fixture's games as an independent read sees them.
struct Expected {
    name: &'static str,
    /// Entries per game.
    sizes: &'static [usize],
    /// Highest turn marker per game.
    turns: &'static [Option<u32>],
    /// Mulligans per game, in player order.
    mulligans: &'static [[u32; 2]],
}

#[test]
fn games_turns_and_mulligans_match_an_independent_read() {
    let expected = [
        Expected {
            name: "clean_2_0_win",
            sizes: &[108, 145],
            turns: &[Some(6), Some(9)],
            mulligans: &[[2, 0], [1, 1]],
        },
        Expected {
            name: "clean_2_1_loss",
            sizes: &[91, 61, 98],
            turns: &[Some(8), Some(6), Some(7)],
            mulligans: &[[0, 0], [0, 0], [1, 0]],
        },
        Expected {
            name: "clean_2_1_win",
            sizes: &[178, 37, 114],
            turns: &[Some(7), Some(3), Some(7)],
            mulligans: &[[0, 0], [1, 0], [1, 0]],
        },
        Expected {
            name: "concede_2_0",
            sizes: &[40, 100],
            turns: &[Some(4), Some(11)],
            mulligans: &[[0, 0], [1, 1]],
        },
        Expected {
            name: "disconnect_game1",
            sizes: &[45],
            turns: &[Some(3)],
            mulligans: &[[0, 0]],
        },
        Expected {
            name: "instant_concede",
            sizes: &[8],
            turns: &[None],
            mulligans: &[[0, 0]],
        },
        Expected {
            name: "large_2_0_win",
            sizes: &[314, 286],
            turns: &[Some(16), Some(16)],
            mulligans: &[[0, 0], [2, 0]],
        },
        Expected {
            name: "multi_game_partial_on_play",
            sizes: &[123, 65, 6],
            turns: &[Some(7), Some(5), None],
            mulligans: &[[0, 1], [0, 0], [0, 0]],
        },
    ];
    for want in expected {
        let name = want.name;
        let entries = parsed(name).entries;
        let sizes: Vec<usize> = split_into_games(&entries).iter().map(|g| g.len()).collect();
        assert_eq!(sizes, want.sizes, "{name}");

        let result = extract_log_cards(&entries);
        let turns: Vec<Option<u32>> = result.games.iter().map(|g| g.meta.turn_count).collect();
        assert_eq!(turns, want.turns, "{name}");
        let mut mulligans: Vec<[u32; 2]> = Vec::new();
        for game in &result.games {
            let players: Vec<&str> = game
                .meta
                .mulligans
                .iter()
                .map(|m| m.player.as_str())
                .collect();
            assert_eq!(players, result.players, "{name}");
            mulligans.push([game.meta.mulligans[0].value, game.meta.mulligans[1].value]);
        }
        assert_eq!(mulligans, want.mulligans, "{name}");
    }
}

#[test]
fn a_games_display_lines_are_its_window_cleaned() {
    let entries = parsed("clean_2_1_win").entries;
    let games = split_into_games(&entries);
    let second = games[1];
    let started = second.first().expect("entry").timestamp_ms;
    let ended = second.last().expect("entry").timestamp_ms;
    let lines = game_log_entries(&entries, &[], Some(started), Some(ended));
    assert!(lines.len() >= second.len());
    assert!(lines.len() < entries.len());
    for line in &lines {
        assert!(!line.message.contains("@P"), "{}", line.message);
        assert!(!line.message.contains("@["), "{}", line.message);
        assert!(line.timestamp_ms >= started - 5000 && line.timestamp_ms < ended + 6000);
    }
}

// The MetaMessage fallback over the fixture mtgo.log files.

fn meta_events(name: &str) -> (String, Vec<MetaEvent>) {
    let fixture = load_fixture(name);
    let events = prepared_events(&fixture)
        .into_iter()
        .filter(|event| event.kind == EventKind::GameManagementJson)
        .map(|event| MetaEvent {
            at_ms: event.at_ms,
            text: event.text,
        })
        .collect();
    (fixture.meta.local_username.clone(), events)
}

#[test]
fn meta_messages_rebuild_a_game_log_with_the_local_player() {
    for name in ["league_join_drop", "draft_hobbit"] {
        let (local, events) = meta_events(name);
        let entries = meta_message_entries(&events);
        assert!(!entries.is_empty(), "{name}");
        assert!(
            entries
                .windows(2)
                .all(|w| w[0].timestamp_ms <= w[1].timestamp_ms),
            "{name}"
        );
        let result = extract_log_cards(&entries);
        assert!(result.players.contains(&local), "{name}");
        assert!(!result.games.is_empty(), "{name}");
    }
}
