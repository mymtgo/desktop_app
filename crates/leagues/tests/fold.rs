//! The league run fold, as fold inputs and expected runs. Ports 0.x
//! `AssignLeagueTest`, `ProcessLeagueEventsTest`, `CompleteLeagueTest` and the
//! constructed league completion case of `MtgoMatchObserverTest`, plus the
//! Plan I review focus cases (re-entry, drops, late deck links) and order
//! independence.

use chrono::NaiveDateTime;
use chrono_tz::Tz;
use leagues::{ImportedRunSeed, LeagueInput, LeagueMatch, OrderedInput, Run, RunState, fold_runs};
use mtgo_match::{MatchState, Outcome};

const TOKEN: &str = "league-token-123";
const UTC: Tz = chrono_tz::UTC;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;

/// UTC milliseconds of `YYYY-MM-DDTHH:MM:SS`.
fn utc(text: &str) -> i64 {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .unwrap_or_else(|error| panic!("{text}: {error}"))
        .and_utc()
        .timestamp_millis()
}

fn base() -> i64 {
    utc("2026-10-07T12:00:00")
}

/// A complete, won constructed league match on `TOKEN`, 20 minutes long.
fn played(token: &str, started_at: i64, deck_version_id: Option<i64>) -> LeagueMatch {
    LeagueMatch {
        token: token.to_string(),
        league_token: TOKEN.to_string(),
        started_at,
        ended_at: Some(started_at + 20 * MINUTE),
        state: MatchState::Complete,
        outcome: Outcome::Win,
        format: Some("CSTANDARD".to_string()),
        game_structure: Some("Constructed".to_string()),
        deck_version_id,
        games: Vec::new(),
    }
}

fn on_token(mut played: LeagueMatch, league_token: &str) -> LeagueMatch {
    played.league_token = league_token.to_string();
    played
}

fn in_state(mut played: LeagueMatch, state: MatchState) -> LeagueMatch {
    played.state = state;
    if state != MatchState::Complete {
        played.ended_at = None;
    }
    played
}

/// Inputs in log order: every push takes the next byte offset of one file.
#[derive(Default)]
struct Log {
    items: Vec<OrderedInput>,
}

impl Log {
    fn push(&mut self, order_ms: i64, input: LeagueInput) {
        let offset = self.items.len() as u64 * 100;
        self.items.push((order_ms, 1, offset, input));
    }

    fn game(&mut self, played: LeagueMatch) {
        self.push(played.started_at, LeagueInput::Match(played));
    }

    fn joined(&mut self, token: &str, event_id: i64, at_ms: i64) {
        self.push(
            at_ms,
            LeagueInput::Joined {
                token: token.to_string(),
                event_id,
                at_ms,
                rounds: None,
            },
        );
    }

    fn dropped(&mut self, at_ms: i64) {
        self.push(at_ms, LeagueInput::Dropped { at_ms });
    }

    /// A user record, not a log line: file 0, offset 0.
    fn manual_drop(&mut self, run_key: &str, at_ms: i64) {
        self.items.push((
            at_ms,
            0,
            0,
            LeagueInput::ManualDrop {
                run_key: run_key.to_string(),
                at_ms,
            },
        ));
    }

    fn fold(&self) -> Vec<Run> {
        fold_runs(self.items.clone(), UTC)
    }
}

fn tokens(run: &Run) -> Vec<&str> {
    run.match_tokens.iter().map(String::as_str).collect()
}

/// Five complete matches on `TOKEN` with one deck version, 30 minutes apart.
fn full_run(log: &mut Log, prefix: &str, from: i64, deck_version_id: Option<i64>) {
    for index in 0..5 {
        log.game(played(
            &format!("{prefix}{index}"),
            from + index * 30 * MINUTE,
            deck_version_id,
        ));
    }
}

// 0.x AssignLeagueTest: real league assignment

#[test]
fn creates_a_run_for_a_new_token_and_deck_version() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].token, TOKEN);
    assert_eq!(runs[0].deck_version_id, Some(7));
    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[0].started_at, base());
    assert_eq!(runs[0].format.as_deref(), Some("CSTANDARD"));
    assert_eq!(runs[0].key, format!("{TOKEN}:m1"));
    assert_eq!(tokens(&runs[0]), ["m1"]);
    assert_eq!(runs[0].event_id, None);
    assert_eq!(runs[0].joined_at, None);
}

#[test]
fn reuses_the_run_for_the_same_token_and_deck_version() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.game(played("m2", base() + HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(tokens(&runs[0]), ["m1", "m2"]);
}

#[test]
fn splits_runs_for_the_same_token_and_a_different_deck_version() {
    // The app-not-watching re-entry: run A (v1) dropped and run B (v2)
    // joined while the app was closed. No join event, so the deck splits.
    let mut log = Log::default();
    log.game(played("m1", base(), Some(1)));
    log.game(played("m2", base() + HOUR, Some(2)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(tokens(&runs[0]), ["m1"]);
    assert_eq!(tokens(&runs[1]), ["m2"]);
    assert_eq!(runs[0].token, TOKEN);
    assert_eq!(runs[1].token, TOKEN);
}

#[test]
fn marks_the_previous_run_partial_when_a_new_run_starts_on_the_same_token() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(1)));
    log.game(played("m2", base() + HOUR, Some(2)));

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(runs[1].state, RunState::Active);
}

#[test]
fn the_first_match_after_a_join_takes_the_joins_event_id_and_time() {
    // 0.x: "attaches first match to a ProcessLeagueEvents-created league".
    // v1 never creates a run from a join; the run minted by the first match
    // reads the latest join with its token.
    let mut log = Log::default();
    log.joined(TOKEN, 10397, base() - 5 * MINUTE);
    log.game(played("m1", base(), Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].event_id, Some(10397));
    assert_eq!(runs[0].joined_at, Some(base() - 5 * MINUTE));
}

#[test]
fn a_run_without_a_deck_version_adopts_the_next_matchs() {
    // 0.x: "backfills deck_version_id on a league with null deck_version_id".
    let mut log = Log::default();
    log.game(played("m1", base(), None));
    log.game(played("m2", base() + HOUR, Some(7)));
    log.game(played("m3", base() + 2 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].deck_version_id, Some(7));
    assert_eq!(tokens(&runs[0]), ["m1", "m2", "m3"]);
}

#[test]
fn an_adopted_deck_version_then_splits_a_different_one() {
    let mut log = Log::default();
    log.game(played("m1", base(), None));
    log.game(played("m2", base() + HOUR, Some(7)));
    log.game(played("m3", base() + 2 * HOUR, Some(8)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(tokens(&runs[0]), ["m1", "m2"]);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(tokens(&runs[1]), ["m3"]);
}

#[test]
fn matches_without_a_deck_version_share_a_run_on_the_token_alone() {
    let mut log = Log::default();
    log.game(played("m1", base(), None));
    log.game(played("m2", base() + HOUR, None));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].deck_version_id, None);
    assert_eq!(tokens(&runs[0]), ["m1", "m2"]);
}

#[test]
fn a_match_without_a_deck_version_joins_the_versioned_run_on_its_token() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.game(played("m2", base() + HOUR, None));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].deck_version_id, Some(7));
    assert_eq!(tokens(&runs[0]), ["m1", "m2"]);
}

#[test]
fn folding_is_idempotent_and_a_repeated_match_is_assigned_once() {
    // 0.x: "is idempotent". A match is only ever assigned once.
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.game(played("m1", base(), Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(tokens(&runs[0]), ["m1"]);
    assert_eq!(log.fold(), runs);
}

// 0.x AssignLeagueTest: re-entry after completion

#[test]
fn re_entering_with_the_same_deck_after_five_matches_starts_a_new_run() {
    let mut log = Log::default();
    full_run(&mut log, "a", base(), Some(7));
    log.game(played("b0", base() + 4 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(tokens(&runs[0]), ["a0", "a1", "a2", "a3", "a4"]);
    assert_eq!(runs[1].state, RunState::Active);
    assert_eq!(runs[1].token, TOKEN);
    assert_eq!(runs[1].deck_version_id, Some(7));
    assert_eq!(tokens(&runs[1]), ["b0"]);
}

#[test]
fn a_sixth_match_never_joins_an_active_run_at_the_five_match_cap() {
    // The 5th match never completed (abandoned), so the run is still Active
    // with 5 matches. The next match closes it as Complete and starts anew.
    let mut log = Log::default();
    for index in 0..4 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }
    let fifth = in_state(
        played("a4", base() + 4 * HOUR, Some(7)),
        MatchState::Abandoned,
    );
    log.game(LeagueMatch {
        ended_at: Some(base() + 4 * HOUR + 10 * MINUTE),
        ..fifth
    });
    log.game(played("b0", base() + 6 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].match_tokens.len(), 5);
    // The last match's end (0.x stamped the wall clock at that moment).
    assert_eq!(runs[0].completed_at, Some(base() + 4 * HOUR + 10 * MINUTE));
    assert_eq!(tokens(&runs[1]), ["b0"]);
    assert_eq!(runs[1].state, RunState::Active);
}

#[test]
fn a_capped_run_whose_last_match_never_ended_completes_when_the_next_match_starts() {
    let mut log = Log::default();
    for index in 0..4 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }
    log.game(in_state(
        played("a4", base() + 4 * HOUR, Some(7)),
        MatchState::InProgress,
    ));
    log.game(played("b0", base() + 6 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at, Some(base() + 6 * HOUR));
}

#[test]
fn reuses_an_active_run_with_fewer_than_five_matches() {
    let mut log = Log::default();
    for index in 0..4 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].match_tokens.len(), 4);
    assert_eq!(runs[0].state, RunState::Active);
}

#[test]
fn a_run_with_an_event_id_still_takes_matches_on_its_token_and_deck() {
    // 0.x: "falls back to composite key when event_id not in gameMeta".
    let mut log = Log::default();
    log.joined(TOKEN, 10397, base() - MINUTE);
    log.game(played("m1", base(), Some(7)));
    log.game(played("m2", base() + HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].event_id, Some(10397));
    assert_eq!(tokens(&runs[0]), ["m1", "m2"]);
}

#[test]
fn format_codes_do_not_split_a_run() {
    // 0.x: "attaches match ... when format codes diverge" (panel view gives
    // Modern, match logs CMODERN). The token alone picks the run.
    let mut log = Log::default();
    let mut first = played("m1", base(), Some(7));
    first.format = Some("Modern".to_string());
    log.game(first);
    let mut second = played("m2", base() + HOUR, Some(7));
    second.format = Some("CMODERN".to_string());
    log.game(second);

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].format.as_deref(), Some("Modern"));
}

#[test]
fn a_match_without_a_league_token_is_left_out() {
    let mut log = Log::default();
    log.game(on_token(played("m1", base(), Some(7)), ""));

    assert!(log.fold().is_empty());
}

#[test]
fn a_constructed_run_splits_on_a_deck_version_change() {
    // 0.x: "mints a new constructed league on a deck-version change".
    let mut log = Log::default();
    log.game(played("m1", base(), Some(1)));
    log.game(played("m2", base() + HOUR, Some(2)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_ne!(runs[0].key, runs[1].key);
}

#[test]
fn limited_matches_are_left_out() {
    let mut log = Log::default();
    let mut draft = played("d1", base(), Some(7));
    draft.format = Some("DHOBHOBHOB".to_string());
    log.game(draft);
    let mut sealed = played("s1", base() + HOUR, Some(7));
    sealed.format = Some("S6FRA".to_string());
    log.game(sealed);

    assert!(log.fold().is_empty());
}

// 0.x ProcessLeagueEventsTest: panel-view backfill never creates a run

#[test]
fn a_join_alone_creates_no_run() {
    let mut log = Log::default();
    log.joined(TOKEN, 10397, base());

    assert!(log.fold().is_empty());
}

#[test]
fn a_join_backfills_event_id_and_joined_at_on_the_active_run() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.joined(TOKEN, 10397, base() + 30 * MINUTE);

    let runs = log.fold();

    assert_eq!(runs[0].event_id, Some(10397));
    assert_eq!(runs[0].joined_at, Some(base() + 30 * MINUTE));
}

#[test]
fn a_join_never_overwrites_an_event_id() {
    let mut log = Log::default();
    log.joined(TOKEN, 99999, base() - MINUTE);
    log.game(played("m1", base(), Some(7)));
    log.joined(TOKEN, 10397, base() + 30 * MINUTE);

    let runs = log.fold();

    assert_eq!(runs[0].event_id, Some(99999));
    assert_eq!(runs[0].joined_at, Some(base() - MINUTE));
}

#[test]
fn a_join_leaves_partial_and_complete_runs_alone() {
    let mut log = Log::default();
    log.game(played("p1", base(), Some(1)));
    log.game(played("q1", base() + HOUR, Some(2)));
    log.joined(TOKEN, 10397, base() + 2 * HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(runs[0].event_id, None);
    assert_eq!(runs[1].event_id, Some(10397));

    let mut complete = Log::default();
    full_run(&mut complete, "a", base(), Some(7));
    complete.joined(TOKEN, 10397, base() + 5 * HOUR);

    let runs = complete.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].event_id, None);
    assert_eq!(runs[0].joined_at, None);
}

#[test]
fn a_join_only_backfills_runs_with_its_token() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.joined("other-token", 10397, base() + MINUTE);

    assert_eq!(log.fold()[0].event_id, None);
}

// 0.x ProcessLeagueEventsTest: drop attribution

#[test]
fn a_drop_after_a_panel_view_drops_the_viewed_run() {
    let mut log = Log::default();
    log.joined(TOKEN, 10397, base() - MINUTE);
    log.game(played("m1", base(), Some(7)));
    log.joined(TOKEN, 10397, base() + HOUR - 5 * SECOND);
    log.dropped(base() + HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() + HOUR));
}

#[test]
fn a_drop_follows_the_most_recent_panel_view_not_the_newest_run() {
    // Pioneer joined first, Modern later; the last panel viewed is Pioneer.
    let mut log = Log::default();
    log.joined("pioneer-token", 11111, base() - 2 * HOUR - MINUTE);
    log.game(on_token(
        played("p1", base() - 2 * HOUR, Some(1)),
        "pioneer-token",
    ));
    log.joined("modern-token", 22222, base() - 10 * MINUTE - MINUTE);
    log.game(on_token(
        played("n1", base() - 10 * MINUTE, Some(2)),
        "modern-token",
    ));
    log.joined("modern-token", 22222, base() - 30 * SECOND);
    log.joined("pioneer-token", 11111, base() - 5 * SECOND);
    log.dropped(base());

    let runs = log.fold();

    assert_eq!(runs[0].token, "pioneer-token");
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[1].token, "modern-token");
    assert_eq!(runs[1].state, RunState::Active);
}

#[test]
fn a_drop_falls_back_to_the_panel_views_token() {
    // The run holds the first entry's event id; the last panel view shows a
    // newer one that no run carries yet, so the token finds the run.
    let mut log = Log::default();
    log.joined(TOKEN, 10397, base() - MINUTE);
    log.game(played("m1", base(), Some(7)));
    log.joined(TOKEN, 10500, base() + HOUR - 5 * SECOND);
    log.dropped(base() + HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].event_id, Some(10397));
    assert_eq!(runs[0].state, RunState::Dropped);
}

#[test]
fn a_drop_without_a_preceding_panel_view_does_nothing() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.dropped(base() + HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[0].dropped_at, None);
}

#[test]
fn a_panel_view_after_the_drop_does_not_count() {
    let mut log = Log::default();
    log.game(played("m1", base(), Some(7)));
    log.dropped(base() + HOUR);
    log.joined(TOKEN, 10397, base() + HOUR + SECOND);

    assert_eq!(log.fold()[0].state, RunState::Active);
}

#[test]
fn a_drop_hits_the_newest_active_run_sharing_the_event_id() {
    // 0.x: "attributes drop to the most recently started Active league when
    // two share an event_id".
    let mut log = Log::default();
    log.joined("token-a", 10397, base() - HOUR - MINUTE);
    log.game(on_token(played("a1", base() - HOUR, Some(1)), "token-a"));
    log.joined("token-b", 10397, base() - 10 * MINUTE - MINUTE);
    log.game(on_token(
        played("b1", base() - 10 * MINUTE, Some(2)),
        "token-b",
    ));
    log.joined("token-b", 10397, base() - 5 * SECOND);
    log.dropped(base());

    let runs = log.fold();

    assert_eq!(runs[0].event_id, Some(10397));
    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[1].event_id, Some(10397));
    assert_eq!(runs[1].state, RunState::Dropped);
}

// 0.x CompleteLeagueTest and MtgoMatchObserverTest

#[test]
fn five_complete_matches_complete_the_run_at_the_fifth_matchs_end() {
    let mut log = Log::default();
    full_run(&mut log, "a", base(), Some(7));

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(
        runs[0].completed_at,
        Some(base() + 4 * 30 * MINUTE + 20 * MINUTE)
    );
}

#[test]
fn completion_is_stamped_once() {
    let mut log = Log::default();
    full_run(&mut log, "a", base(), Some(7));
    log.game(played("b0", base() + 5 * HOUR, Some(7)));
    log.joined(TOKEN, 1, base() + 6 * HOUR);
    log.dropped(base() + 6 * HOUR + SECOND);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(
        runs[0].completed_at,
        Some(base() + 4 * 30 * MINUTE + 20 * MINUTE)
    );
    assert_eq!(runs[0].dropped_at, None);
}

#[test]
fn a_constructed_run_stays_active_after_three_matches() {
    let mut log = Log::default();
    for index in 0..3 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[0].completed_at, None);
}

#[test]
fn a_fifth_match_still_in_progress_leaves_the_run_active() {
    let mut log = Log::default();
    for index in 0..4 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }
    log.game(in_state(
        played("a4", base() + 4 * HOUR, Some(7)),
        MatchState::InProgress,
    ));

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[0].match_tokens.len(), 5);
}

#[test]
fn a_run_with_an_abandoned_match_completes_at_the_cap() {
    // Six matches: one abandoned along the way. The run hits the five match
    // cap first, so the sixth match closes it (0.x step 2.5).
    let mut log = Log::default();
    log.game(played("a0", base(), Some(7)));
    log.game(in_state(
        played("a1", base() + HOUR, Some(7)),
        MatchState::Abandoned,
    ));
    for index in 2..5 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Active);

    log.game(played("b0", base() + 6 * HOUR, Some(7)));
    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at, Some(base() + 4 * HOUR + 20 * MINUTE));
    assert_eq!(tokens(&runs[1]), ["b0"]);
}

// Names and keys (spec section 4)

#[test]
fn the_name_is_the_game_structure_and_the_local_start_with_minutes() {
    let mut log = Log::default();
    let mut first = played("m1", utc("2026-10-07T12:05:00"), Some(7));
    first.game_structure = Some("Modern".to_string());
    log.game(first);
    let london: Tz = chrono_tz::Europe::London;

    let runs = fold_runs(log.items.clone(), london);

    // 12:05 UTC is 13:05 in London summer time.
    assert_eq!(runs[0].name, "Modern League 07-10-2026 01:05pm");
}

#[test]
fn the_name_without_a_game_structure_is_trimmed() {
    let mut log = Log::default();
    let mut first = played("m1", utc("2026-01-02T23:59:00"), Some(7));
    first.game_structure = None;
    log.game(first);

    assert_eq!(log.fold()[0].name, "League 02-01-2026 11:59pm");
}

// Review focus 1: re-entry with the same token

#[test]
fn re_entry_after_a_drop_starts_a_new_run_and_leaves_the_dropped_one() {
    let mut log = Log::default();
    log.joined(TOKEN, 100, base() - MINUTE);
    log.game(played("a0", base(), Some(7)));
    log.game(played("a1", base() + HOUR, Some(7)));
    log.joined(TOKEN, 100, base() + 2 * HOUR);
    log.dropped(base() + 2 * HOUR + SECOND);
    log.joined(TOKEN, 200, base() + 3 * HOUR);
    log.game(played("b0", base() + 3 * HOUR + MINUTE, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].event_id, Some(100));
    assert_eq!(tokens(&runs[0]), ["a0", "a1"]);
    assert_eq!(runs[1].state, RunState::Active);
    assert_eq!(runs[1].event_id, Some(200));
    assert_eq!(runs[1].joined_at, Some(base() + 3 * HOUR));
    assert_eq!(tokens(&runs[1]), ["b0"]);
}

#[test]
fn re_entry_after_a_complete_run_reads_the_new_join() {
    let mut log = Log::default();
    log.joined(TOKEN, 100, base() - MINUTE);
    full_run(&mut log, "a", base(), Some(7));
    log.joined(TOKEN, 200, base() + 4 * HOUR);
    log.game(played("b0", base() + 4 * HOUR + MINUTE, Some(7)));

    let runs = log.fold();

    assert_eq!(runs[0].event_id, Some(100));
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[1].event_id, Some(200));
    assert_eq!(runs[1].key, format!("{TOKEN}:b0"));
}

#[test]
fn a_deck_change_mid_run_splits_and_only_older_active_runs_go_partial() {
    let mut log = Log::default();
    log.game(played("a0", base(), Some(1)));
    log.game(played("a1", base() + HOUR, Some(1)));
    log.game(on_token(
        played("x0", base() + 90 * MINUTE, Some(9)),
        "other",
    ));
    log.game(played("b0", base() + 2 * HOUR, Some(2)));
    log.game(played("b1", base() + 3 * HOUR, Some(2)));

    let runs = log.fold();

    assert_eq!(runs.len(), 3);
    assert_eq!(tokens(&runs[0]), ["a0", "a1"]);
    assert_eq!(runs[0].state, RunState::Partial);
    // A run on another token is untouched.
    assert_eq!(runs[1].token, "other");
    assert_eq!(runs[1].state, RunState::Active);
    assert_eq!(tokens(&runs[2]), ["b0", "b1"]);
    assert_eq!(runs[2].state, RunState::Active);
}

#[test]
fn a_partial_run_takes_no_more_matches_even_with_its_deck() {
    let mut log = Log::default();
    log.game(played("a0", base(), Some(1)));
    log.game(played("b0", base() + HOUR, Some(2)));
    log.game(played("c0", base() + 2 * HOUR, Some(1)));

    let runs = log.fold();

    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(runs[1].state, RunState::Partial);
    assert_eq!(tokens(&runs[2]), ["c0"]);
}

// Review focus 2: drops

#[test]
fn a_drop_before_any_match_does_nothing() {
    let mut log = Log::default();
    log.joined(TOKEN, 100, base() - 2 * MINUTE);
    log.dropped(base() - MINUTE);
    log.game(played("a0", base(), Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[0].dropped_at, None);
}

#[test]
fn a_manual_drop_drops_an_active_run() {
    let mut log = Log::default();
    log.game(played("a0", base(), Some(7)));
    log.manual_drop(&format!("{TOKEN}:a0"), base() + HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() + HOUR));
}

#[test]
fn a_manual_drop_never_undoes_a_complete_run() {
    let mut log = Log::default();
    full_run(&mut log, "a", base(), Some(7));
    log.manual_drop(&format!("{TOKEN}:a0"), base() + 5 * HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].dropped_at, None);
}

#[test]
fn a_manual_drop_on_a_run_the_log_later_continues_splits_like_0x() {
    // 0.x: the dropped league is no longer Active, so later matches on the
    // token mint a new run, and the observer never completes a dropped run.
    let mut log = Log::default();
    for index in 0..3 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }
    log.manual_drop(&format!("{TOKEN}:a0"), base() + 3 * HOUR);
    log.game(played("a3", base() + 4 * HOUR, Some(7)));
    log.game(played("a4", base() + 5 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(tokens(&runs[0]), ["a0", "a1", "a2"]);
    assert_eq!(runs[1].state, RunState::Active);
    assert_eq!(tokens(&runs[1]), ["a3", "a4"]);
}

#[test]
fn a_manual_drop_stamped_before_its_run_started_still_applies() {
    // The user record uses the wall clock; a clock a little behind the log
    // must not lose the drop.
    let mut log = Log::default();
    log.game(played("a0", base(), Some(7)));
    log.manual_drop(&format!("{TOKEN}:a0"), base() - MINUTE);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() - MINUTE));
}

#[test]
fn a_manual_drop_for_an_unknown_run_is_ignored() {
    let mut log = Log::default();
    log.game(played("a0", base(), Some(7)));
    log.manual_drop("no-such-run", base() + HOUR);

    assert_eq!(log.fold()[0].state, RunState::Active);
}

// Review focus 3: late deck links

#[test]
fn runs_follow_the_current_deck_links() {
    let unlinked = |log: &mut Log| {
        log.game(played("a0", base(), None));
        log.game(played("a1", base() + HOUR, None));
        log.game(played("a2", base() + 2 * HOUR, None));
    };
    let mut before = Log::default();
    unlinked(&mut before);
    assert_eq!(before.fold().len(), 1);

    // The deck links arrive later: a2 was played with another version.
    let mut after = Log::default();
    after.game(played("a0", base(), Some(1)));
    after.game(played("a1", base() + HOUR, Some(1)));
    after.game(played("a2", base() + 2 * HOUR, Some(2)));

    let runs = after.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(tokens(&runs[0]), ["a0", "a1"]);
    assert_eq!(tokens(&runs[1]), ["a2"]);
}

// Order independence (live equals rebuild)

fn busy_log() -> Log {
    let mut log = Log::default();
    log.joined(TOKEN, 100, base() - MINUTE);
    full_run(&mut log, "a", base(), Some(7));
    log.joined(TOKEN, 200, base() + 4 * HOUR);
    log.game(played("b0", base() + 4 * HOUR + MINUTE, Some(7)));
    log.game(played("b1", base() + 5 * HOUR, Some(8)));
    log.joined("other", 300, base() + 5 * HOUR + MINUTE);
    log.game(on_token(
        played("o0", base() + 5 * HOUR + 2 * MINUTE, Some(9)),
        "other",
    ));
    log.joined("other", 300, base() + 6 * HOUR);
    log.dropped(base() + 6 * HOUR + SECOND);
    log.manual_drop(&format!("{TOKEN}:b1"), base() + 7 * HOUR);
    log
}

#[test]
fn shuffled_inputs_give_the_same_runs() {
    let log = busy_log();
    let expected = log.fold();
    assert_eq!(expected.len(), 4);

    let mut reversed = log.items.clone();
    reversed.reverse();
    assert_eq!(fold_runs(reversed, UTC), expected);

    for step in [3usize, 5, 7] {
        // A fixed interleaving: every step-th item first, then the rest.
        let mut shuffled = Vec::new();
        let mut rest = Vec::new();
        for (index, item) in log.items.iter().enumerate() {
            if index % step == 0 {
                shuffled.push(item.clone());
            } else {
                rest.push(item.clone());
            }
        }
        shuffled.extend(rest);
        assert_eq!(fold_runs(shuffled, UTC), expected, "step {step}");
    }
}

#[test]
fn the_busy_log_folds_as_expected() {
    let runs = busy_log().fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[1].event_id, Some(200));
    assert_eq!(tokens(&runs[1]), ["b0"]);
    assert_eq!(runs[1].state, RunState::Partial);
    assert_eq!(tokens(&runs[2]), ["b1"]);
    assert_eq!(runs[2].state, RunState::Dropped);
    assert_eq!(runs[2].dropped_at, Some(base() + 7 * HOUR));
    assert_eq!(runs[3].token, "other");
    assert_eq!(runs[3].event_id, Some(300));
    assert_eq!(runs[3].state, RunState::Dropped);
}

// Review I1: a run completes when its fifth complete match ends, so a drop
// during that match wins (0.x completes in the observer, at the match's end).

/// Four complete matches, then a fifth that starts at `base() + 4h` and ends
/// 20 minutes later. A join precedes the run so log drops can find it.
fn four_and_a_fifth() -> Log {
    let mut log = Log::default();
    log.joined(TOKEN, 100, base() - MINUTE);
    for index in 0..4 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }
    log.game(played("a4", base() + 4 * HOUR, Some(7)));
    log
}

fn fifth_ends() -> i64 {
    base() + 4 * HOUR + 20 * MINUTE
}

fn brick_numbers(run: &Run, log: &Log) -> (leagues::Classification, Option<f64>) {
    let matches = log
        .items
        .iter()
        .filter_map(|(_, _, _, input)| match input {
            LeagueInput::Match(played) if run.match_tokens.contains(&played.token) => {
                Some(leagues::RunMatch {
                    token: played.token.clone(),
                    state: played.state,
                    outcome: if played.token == "a0" {
                        Outcome::Loss
                    } else {
                        played.outcome
                    },
                    started_at: played.started_at,
                    ended_at: played.ended_at,
                    deck_version_id: played.deck_version_id,
                    deck_id: Some(1),
                    games: Vec::new(),
                    opponent_archetype: None,
                })
            }
            _ => None,
        })
        .collect();
    let input = leagues::RunInput {
        course_record: None,
        state: run.state,
        manual: false,
        uncapped: false,
        format: run.format.clone(),
        started_at: run.started_at,
        matches,
    };
    let summary = leagues::summarize_run(&input, None, UTC);
    (summary.classification, summary.tix_delta)
}

#[test]
fn a_log_drop_during_the_fifth_match_drops_the_run() {
    let mut log = four_and_a_fifth();
    log.joined(TOKEN, 100, base() + 4 * HOUR + 5 * MINUTE);
    log.dropped(base() + 4 * HOUR + 10 * MINUTE);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() + 4 * HOUR + 10 * MINUTE));
    assert_eq!(runs[0].completed_at, None);
    // A brick, paid as a dropped run: 4-1 stays 4-1 (12.70).
    let (classification, tix) = brick_numbers(&runs[0], &log);
    assert_eq!(classification, leagues::Classification::Brick);
    assert_eq!(tix, Some(12.70));
}

#[test]
fn a_manual_drop_during_the_fifth_match_drops_the_run() {
    let mut log = four_and_a_fifth();
    log.manual_drop(&format!("{TOKEN}:a0"), base() + 4 * HOUR + 10 * MINUTE);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() + 4 * HOUR + 10 * MINUTE));
    assert_eq!(runs[0].completed_at, None);
}

#[test]
fn a_drop_after_the_fifth_match_ends_leaves_the_run_complete() {
    let mut log = four_and_a_fifth();
    log.joined(TOKEN, 100, fifth_ends() + MINUTE);
    log.dropped(fifth_ends() + 2 * MINUTE);
    log.manual_drop(&format!("{TOKEN}:a0"), fifth_ends() + 3 * MINUTE);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at, Some(fifth_ends()));
    assert_eq!(runs[0].dropped_at, None);
}

#[test]
fn a_drop_in_the_same_millisecond_as_the_fifth_end_wins() {
    let mut log = four_and_a_fifth();
    log.joined(TOKEN, 100, base() + 4 * HOUR + 5 * MINUTE);
    log.dropped(fifth_ends());

    let runs = log.fold();
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(fifth_ends()));

    let mut manual = four_and_a_fifth();
    manual.manual_drop(&format!("{TOKEN}:a0"), fifth_ends());
    assert_eq!(manual.fold()[0].state, RunState::Dropped);
}

#[test]
fn the_fifth_completion_counts_matches_in_the_order_they_end() {
    // Five matches complete; the run completes at the latest end among them
    // when that is the fifth to end, not at the fifth to start.
    let mut log = Log::default();
    for index in 0..4 {
        log.game(played(&format!("a{index}"), base() + index * HOUR, Some(7)));
    }
    let mut long = played("a4", base() + 4 * HOUR, Some(7));
    long.ended_at = Some(base() + 6 * HOUR);
    log.game(long);

    let runs = log.fold();

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at, Some(base() + 6 * HOUR));
}

#[test]
fn a_drop_during_the_fifth_match_is_order_independent() {
    let mut log = four_and_a_fifth();
    log.joined(TOKEN, 100, base() + 4 * HOUR + 5 * MINUTE);
    log.dropped(base() + 4 * HOUR + 10 * MINUTE);
    let expected = log.fold();

    let mut reversed = log.items.clone();
    reversed.reverse();
    assert_eq!(fold_runs(reversed, UTC), expected);
}

// Review M2: a match is placed by its `started_at`, whatever `order_ms` the
// caller passed, so log order and "newest run" always agree.

#[test]
fn a_match_is_ordered_by_its_start_not_the_callers_order_ms() {
    let mut log = Log::default();
    log.items.push((
        base() + 10 * HOUR,
        1,
        0,
        LeagueInput::Match(played("m1", base(), Some(1))),
    ));
    log.items.push((
        base(),
        1,
        100,
        LeagueInput::Match(played("m2", base() + HOUR, Some(2))),
    ));

    let runs = log.fold();

    assert_eq!(tokens(&runs[0]), ["m1"]);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(tokens(&runs[1]), ["m2"]);
    assert_eq!(runs[1].state, RunState::Active);
}

// Final review Important 1: a run imported from 0.x is a fold seed, so a run
// that spans both apps appears once and continues when play continues in v1.

const IMPORTED_KEY: &str = "0x:31";

/// A 0.x run on `TOKEN` with deck version 7, started at `base()`, holding
/// `members`.
fn seed(state: RunState, members: Vec<LeagueMatch>) -> ImportedRunSeed {
    ImportedRunSeed {
        key: IMPORTED_KEY.to_string(),
        token: TOKEN.to_string(),
        event_id: Some(500),
        joined_at: Some(base() - MINUTE),
        name: "Qimported League".to_string(),
        format: Some("CSTANDARD".to_string()),
        deck_version_id: Some(7),
        started_at: base(),
        state,
        dropped_at: None,
        completed_at: None,
        matches: members,
    }
}

/// `count` complete matches with tokens `{prefix}{n}`, an hour apart from
/// `from`.
fn hourly(prefix: &str, from: i64, count: i64, deck_version_id: Option<i64>) -> Vec<LeagueMatch> {
    (0..count)
        .map(|index| {
            played(
                &format!("{prefix}{index}"),
                from + index * HOUR,
                deck_version_id,
            )
        })
        .collect()
}

impl Log {
    fn imported(&mut self, seed: ImportedRunSeed) {
        self.push(seed.started_at, LeagueInput::ImportedRun(seed));
    }
}

#[test]
fn an_imported_run_starts_with_exactly_its_0x_state_and_members() {
    let mut finished = seed(RunState::Complete, hourly("x", base(), 5, Some(7)));
    finished.completed_at = Some(base() + 4 * HOUR + 20 * MINUTE);
    let mut log = Log::default();
    log.imported(finished);

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.key, IMPORTED_KEY);
    assert_eq!(run.token, TOKEN);
    assert_eq!(run.event_id, Some(500));
    assert_eq!(run.joined_at, Some(base() - MINUTE));
    assert_eq!(run.name, "Qimported League");
    assert_eq!(run.format.as_deref(), Some("CSTANDARD"));
    assert_eq!(run.deck_version_id, Some(7));
    assert_eq!(run.started_at, base());
    assert_eq!(run.state, RunState::Complete);
    assert_eq!(run.completed_at, Some(base() + 4 * HOUR + 20 * MINUTE));
    assert_eq!(run.dropped_at, None);
    assert_eq!(tokens(run), ["x0", "x1", "x2", "x3", "x4"]);
}

#[test]
fn an_active_imported_run_takes_the_next_live_matches_and_completes_at_the_fifth_end() {
    let mut log = Log::default();
    log.imported(seed(RunState::Active, hourly("x", base(), 2, Some(7))));
    for live in hourly("v", base() + 2 * HOUR, 3, Some(7)) {
        log.game(live);
    }

    let runs = log.fold();

    assert_eq!(runs.len(), 1, "one run, not two halves");
    assert_eq!(runs[0].key, IMPORTED_KEY);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at, Some(base() + 4 * HOUR + 20 * MINUTE));
    assert_eq!(tokens(&runs[0]), ["x0", "x1", "v0", "v1", "v2"]);
}

#[test]
fn an_active_imported_run_without_a_deck_version_adopts_the_next_matchs() {
    let mut unlinked = seed(RunState::Active, hourly("x", base(), 1, None));
    unlinked.deck_version_id = None;
    let mut log = Log::default();
    log.imported(unlinked);
    log.game(played("v0", base() + HOUR, Some(8)));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].deck_version_id, Some(8));
}

#[test]
fn a_complete_imported_run_leaves_later_matches_on_its_token_to_a_new_run() {
    let mut log = Log::default();
    log.imported(seed(RunState::Complete, hourly("x", base(), 5, Some(7))));
    log.game(played("v0", base() + 6 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].key, IMPORTED_KEY);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(tokens(&runs[0]).len(), 5);
    assert_eq!(runs[1].key, format!("{TOKEN}:v0"));
    assert_eq!(runs[1].state, RunState::Active);
    assert_eq!(tokens(&runs[1]), ["v0"]);
}

#[test]
fn dropped_and_partial_imported_runs_stay_as_they_are() {
    for state in [RunState::Dropped, RunState::Partial] {
        let mut stopped = seed(state, hourly("x", base(), 2, Some(7)));
        stopped.dropped_at = (state == RunState::Dropped).then_some(base() + 2 * HOUR);
        let mut log = Log::default();
        log.imported(stopped);
        log.game(played("v0", base() + 3 * HOUR, Some(7)));
        log.joined(TOKEN, 600, base() + 4 * HOUR);
        log.dropped(base() + 4 * HOUR + SECOND);

        let runs = log.fold();

        assert_eq!(runs.len(), 2, "{state:?}");
        assert_eq!(runs[0].state, state);
        assert_eq!(tokens(&runs[0]), ["x0", "x1"]);
        assert_eq!(
            runs[0].dropped_at,
            (state == RunState::Dropped).then_some(base() + 2 * HOUR)
        );
        assert_eq!(tokens(&runs[1]), ["v0"]);
        assert_eq!(
            runs[1].state,
            RunState::Dropped,
            "the drop is the new run's"
        );
    }
}

#[test]
fn a_new_run_on_the_token_makes_an_active_imported_run_partial() {
    let mut log = Log::default();
    log.imported(seed(RunState::Active, hourly("x", base(), 2, Some(7))));
    log.game(played("v0", base() + 3 * HOUR, Some(8)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(runs[1].state, RunState::Active);
}

#[test]
fn an_imported_run_makes_older_active_runs_on_its_token_partial() {
    // Another deck: not a part of the seed's run (issue #143).
    let mut log = Log::default();
    log.game(played("a0", base() - 2 * HOUR, Some(6)));
    log.imported(seed(RunState::Active, hourly("x", base(), 1, Some(7))));

    let runs = log.fold();

    assert_eq!(runs[0].key, format!("{TOKEN}:a0"));
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(runs[1].key, IMPORTED_KEY);
    assert_eq!(runs[1].state, RunState::Active);
}

#[test]
fn a_join_fills_an_active_imported_runs_missing_event_id() {
    let mut unjoined = seed(RunState::Active, hourly("x", base(), 1, Some(7)));
    unjoined.event_id = None;
    unjoined.joined_at = None;
    let mut log = Log::default();
    log.imported(unjoined);
    log.joined(TOKEN, 700, base() + HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].event_id, Some(700));
    assert_eq!(runs[0].joined_at, Some(base() + HOUR));
}

#[test]
fn a_log_drop_drops_an_active_imported_run() {
    let mut log = Log::default();
    log.imported(seed(RunState::Active, hourly("x", base(), 2, Some(7))));
    log.joined(TOKEN, 500, base() + 3 * HOUR);
    log.dropped(base() + 3 * HOUR + SECOND);
    log.game(played("v0", base() + 4 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].key, IMPORTED_KEY);
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() + 3 * HOUR + SECOND));
    assert_eq!(tokens(&runs[1]), ["v0"]);
}

#[test]
fn a_manual_drop_drops_an_imported_run_by_its_key_but_never_a_complete_one() {
    let mut log = Log::default();
    log.imported(seed(RunState::Active, hourly("x", base(), 2, Some(7))));
    log.manual_drop(IMPORTED_KEY, base() + 3 * HOUR);
    let runs = log.fold();
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() + 3 * HOUR));

    let mut log = Log::default();
    log.imported(seed(RunState::Complete, hourly("x", base(), 5, Some(7))));
    log.manual_drop(IMPORTED_KEY, base() + 6 * HOUR);
    let runs = log.fold();
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].dropped_at, None);
}

#[test]
fn an_active_imported_run_at_the_cap_closes_and_the_next_match_mints() {
    let mut log = Log::default();
    let mut members = hourly("x", base(), 4, Some(7));
    members.push(in_state(
        played("x4", base() + 4 * HOUR, Some(7)),
        MatchState::Abandoned,
    ));
    log.imported(seed(RunState::Active, members));
    log.game(played("v0", base() + 6 * HOUR, Some(7)));

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(tokens(&runs[1]), ["v0"]);
}

#[test]
fn re_entry_after_an_imported_run_continued_in_v1_leaves_no_active_run() {
    // The stale case from the review: the run's last matches were tracked
    // live, and the player re-entered on the same token with the same deck.
    let mut log = Log::default();
    log.imported(seed(RunState::Active, hourly("x", base(), 2, Some(7))));
    for live in hourly("v", base() + 2 * HOUR, 3, Some(7)) {
        log.game(live);
    }
    log.joined(TOKEN, 800, base() + 6 * HOUR);
    for again in hourly("w", base() + 7 * HOUR, 5, Some(7)) {
        log.game(again);
    }

    let runs = log.fold();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].key, IMPORTED_KEY);
    assert_eq!(tokens(&runs[0]), ["x0", "x1", "v0", "v1", "v2"]);
    assert_eq!(runs[1].key, format!("{TOKEN}:w0"));
    assert_eq!(runs[1].event_id, Some(800));
    assert_eq!(tokens(&runs[1]), ["w0", "w1", "w2", "w3", "w4"]);
    assert!(
        runs.iter().all(|run| run.state == RunState::Complete),
        "no Active run is left behind: {runs:?}"
    );
}

#[test]
fn imported_runs_fold_the_same_in_any_order() {
    let mut log = Log::default();
    log.imported(seed(RunState::Active, hourly("x", base(), 2, Some(7))));
    let mut other = seed(RunState::Complete, Vec::new());
    other.key = "0x:30".to_string();
    other.token = "other".to_string();
    other.started_at = base() - 3 * HOUR;
    log.imported(other);
    for live in hourly("v", base() + 2 * HOUR, 2, Some(7)) {
        log.game(live);
    }
    log.joined(TOKEN, 500, base() + 4 * HOUR);
    log.dropped(base() + 4 * HOUR + SECOND);
    log.game(played("w0", base() + 5 * HOUR, Some(7)));
    log.manual_drop(IMPORTED_KEY, base() + 6 * HOUR);
    let expected = log.fold();
    assert_eq!(expected.len(), 3);
    assert_eq!(expected[1].key, IMPORTED_KEY);
    assert_eq!(expected[1].state, RunState::Dropped);
    assert_eq!(tokens(&expected[1]), ["x0", "x1", "v0", "v1"]);

    let mut reversed = log.items.clone();
    reversed.reverse();
    assert_eq!(fold_runs(reversed, UTC), expected);
    for step in [2usize, 3, 5] {
        let mut shuffled = Vec::new();
        let mut rest = Vec::new();
        for (index, item) in log.items.iter().enumerate() {
            if index % step == 0 {
                shuffled.push(item.clone());
            } else {
                rest.push(item.clone());
            }
        }
        shuffled.extend(rest);
        assert_eq!(fold_runs(shuffled, UTC), expected, "step {step}");
    }
}

#[test]
fn a_seed_without_an_event_id_takes_the_latest_earlier_panel_view() {
    let mut log = Log::default();
    log.joined(TOKEN, 400, base() - 2 * HOUR);
    log.joined(TOKEN, 450, base() - HOUR);
    log.joined(TOKEN, 999, base() + HOUR);
    let mut no_event = seed(RunState::Active, hourly("x", base(), 1, Some(7)));
    no_event.event_id = None;
    no_event.joined_at = None;
    log.imported(no_event);

    let runs = log.fold();

    assert_eq!(runs[0].key, IMPORTED_KEY);
    assert_eq!(runs[0].event_id, Some(450));
    assert_eq!(runs[0].joined_at, Some(base() - HOUR));
}

#[test]
fn seeds_starting_together_order_by_numeric_id() {
    let mut log = Log::default();
    let mut nine = seed(RunState::Active, Vec::new());
    nine.key = "0x:9".to_string();
    let mut ten = seed(RunState::Active, Vec::new());
    ten.key = "0x:10".to_string();
    // Both sit at one position, as the store places seeds.
    log.items
        .push((nine.started_at, 0, 0, LeagueInput::ImportedRun(ten)));
    log.items
        .push((nine.started_at, 0, 0, LeagueInput::ImportedRun(nine)));

    let runs = log.fold();

    let state_of = |key: &str| runs.iter().find(|run| run.key == key).expect("run").state;
    assert_eq!(state_of("0x:9"), RunState::Partial);
    assert_eq!(state_of("0x:10"), RunState::Active);
}

#[test]
fn a_manual_drop_on_an_already_dropped_run_keeps_the_earlier_time() {
    let mut dropped = seed(RunState::Dropped, hourly("x", base(), 2, Some(7)));
    dropped.dropped_at = Some(base() + 2 * HOUR);
    let mut log = Log::default();
    log.imported(dropped);
    log.manual_drop(IMPORTED_KEY, base() + 9 * HOUR);

    let runs = log.fold();

    assert_eq!(runs[0].dropped_at, Some(base() + 2 * HOUR));
}

// Issue #143: 0.x was not running for the first matches of a run v1 saw
// whole, so 0.x's run starts later and holds fewer matches. The seed takes
// the earlier Active run's matches (its event id the seed's, or unknown)
// instead of leaving them a Partial run beside an Active seed.
#[test]
fn an_imported_run_that_saw_fewer_matches_takes_the_earlier_ones_of_its_entry() {
    let mut log = Log::default();
    log.joined(TOKEN, 500, base() - 3 * HOUR);
    for live in hourly("v", base() - 2 * HOUR, 2, Some(7)) {
        log.game(live);
    }
    log.imported(seed(RunState::Active, hourly("x", base(), 3, Some(7))));

    let runs = log.fold();

    assert_eq!(runs.len(), 1, "one run, not a Partial half and a seed");
    let run = &runs[0];
    assert_eq!(run.key, IMPORTED_KEY);
    assert_eq!(tokens(run), ["v0", "v1", "x0", "x1", "x2"]);
    assert_eq!(run.started_at, base() - 2 * HOUR);
    assert_eq!(run.state, RunState::Complete);
    assert_eq!(run.completed_at, Some(base() + 2 * HOUR + 20 * MINUTE));
    assert_eq!(run.event_id, Some(500));

    let mut reversed = log.items.clone();
    reversed.reverse();
    assert_eq!(fold_runs(reversed, UTC), runs, "any arrival order");
}

#[test]
fn an_imported_run_leaves_an_earlier_run_of_another_entry_or_too_many_matches() {
    // Another entry: another event id.
    let mut log = Log::default();
    log.joined(TOKEN, 499, base() - 3 * HOUR);
    for live in hourly("v", base() - 2 * HOUR, 2, Some(7)) {
        log.game(live);
    }
    log.imported(seed(RunState::Active, hourly("x", base(), 3, Some(7))));
    let runs = log.fold();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(tokens(&runs[1]), ["x0", "x1", "x2"]);

    // The same entry, but together more than five matches.
    let mut log = Log::default();
    log.joined(TOKEN, 500, base() - 4 * HOUR);
    for live in hourly("v", base() - 3 * HOUR, 3, Some(7)) {
        log.game(live);
    }
    log.imported(seed(RunState::Active, hourly("x", base(), 3, Some(7))));
    let runs = log.fold();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(tokens(&runs[1]), ["x0", "x1", "x2"]);
}

// Review M3: a derived run with no panel view (no event id) is taken in
// too, still only while Active and within five matches.
#[test]
fn an_imported_run_takes_an_earlier_run_without_an_event_id() {
    let mut log = Log::default();
    for live in hourly("v", base() - 2 * HOUR, 2, Some(7)) {
        log.game(live);
    }
    log.imported(seed(RunState::Active, hourly("x", base(), 3, Some(7))));

    let runs = log.fold();

    assert_eq!(runs.len(), 1);
    assert_eq!(tokens(&runs[0]), ["v0", "v1", "x0", "x1", "x2"]);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].event_id, Some(500), "the seed's");
}

// Review leftover (#143): without an event id the five match limit still
// holds. Three earlier matches on the same deck and the seed's three would
// make six, so the earlier run stays its own (Partial) run.
#[test]
fn an_imported_run_leaves_an_earlier_run_without_an_event_id_past_five_matches() {
    let mut log = Log::default();
    for live in hourly("v", base() - 3 * HOUR, 3, Some(7)) {
        log.game(live);
    }
    log.imported(seed(RunState::Active, hourly("x", base(), 3, Some(7))));

    let runs = log.fold();

    assert_eq!(runs.len(), 2, "not merged");
    assert_eq!(tokens(&runs[0]), ["v0", "v1", "v2"]);
    assert_eq!(runs[0].state, RunState::Partial);
    assert_eq!(runs[0].event_id, None);
    assert_eq!(runs[1].key, IMPORTED_KEY);
    assert_eq!(tokens(&runs[1]), ["x0", "x1", "x2"]);
    assert_eq!(runs[1].event_id, Some(500));

    let mut reversed = log.items.clone();
    reversed.reverse();
    assert_eq!(fold_runs(reversed, UTC), runs, "any arrival order");
}
