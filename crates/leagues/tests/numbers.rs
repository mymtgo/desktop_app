//! League and tournament numbers. Ports 0.x `FormatLeagueRunsTest`,
//! `GetLeagueKpisTest`, `LeagueEvTableTest` (unit and feature),
//! `LeagueFinishOddsTest`, `GetLeagueResultDistributionTest`,
//! `GetLeagueInProgressTest`, `FormatTournamentRunsTest` and the sidebar
//! trophy rule of `GetDeckViewSharedProps`.

use chrono::NaiveDateTime;
use chrono_tz::Tz;
use decks::stats::Record;
use leagues::{
    Classification, DeckVersionDate, GameResult, Mark, OddsSource, RunInput, RunMatch, RunState,
    TimeOfDay, TopMatchup, TournamentInput, TournamentMatch, WinLoss, break_even_wins,
    complete_wins, entry_cost, ev_table, finish_odds, in_progress_index, latest_finished_index,
    league_in_progress, league_kpis, net_tix, net_tix_for_run, on_leagues_page,
    passes_state_filter, result_distribution, shown_record, summarize_run, tournament_name,
    tournament_run, trophy_count, version_label, win_probability,
};
use mtgo_match::{MatchState, Outcome};

const UTC: Tz = chrono_tz::UTC;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DECK: i64 = 1;
const VERSION: i64 = 10;

/// UTC milliseconds of `YYYY-MM-DD HH:MM:SS`.
fn utc(text: &str) -> i64 {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .unwrap_or_else(|error| panic!("{text}: {error}"))
        .and_utc()
        .timestamp_millis()
}

fn base() -> i64 {
    utc("2026-04-01 10:00:00")
}

/// A complete match of `DECK` / `VERSION`, 10 minutes long.
fn played(index: i64, outcome: Outcome) -> RunMatch {
    let started_at = base() + index * 10 * MINUTE;
    RunMatch {
        token: format!("m{index}"),
        state: MatchState::Complete,
        outcome,
        started_at,
        ended_at: Some(started_at + 10 * MINUTE),
        deck_version_id: Some(VERSION),
        deck_id: Some(DECK),
        games: Vec::new(),
        opponent_archetype: None,
    }
}

fn run_with(state: RunState, outcomes: &[Outcome]) -> RunInput {
    RunInput {
        course_record: None,
        state,
        manual: false,
        uncapped: false,
        format: Some("Modern".to_string()),
        started_at: base(),
        matches: outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| played(index as i64, *outcome))
            .collect(),
    }
}

/// A run of `wins` wins then `losses` losses (0.x `seedLeagueRun`).
fn seeded(state: RunState, wins: usize, losses: usize) -> RunInput {
    let mut outcomes = vec![Outcome::Win; wins];
    outcomes.extend(vec![Outcome::Loss; losses]);
    run_with(state, &outcomes)
}

fn game(result: mtgo_match::GameResult, on_play: Option<bool>) -> GameResult {
    GameResult {
        result,
        on_play,
        started_at: None,
        ended_at: None,
    }
}

// 0.x FormatLeagueRunsTest

#[test]
fn a_five_and_oh_complete_run_is_a_trophy() {
    let summary = summarize_run(&run_with(RunState::Complete, &[Outcome::Win; 5]), None, UTC);

    assert_eq!(summary.classification, Classification::Trophy);
    assert_eq!(summary.live_round, None);
}

#[test]
fn a_four_and_one_complete_run_is_cash() {
    let run = run_with(
        RunState::Complete,
        &[
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Loss,
        ],
    );

    assert_eq!(
        summarize_run(&run, None, UTC).classification,
        Classification::Cash
    );
}

#[test]
fn a_three_and_two_complete_run_is_a_finish() {
    let run = run_with(
        RunState::Complete,
        &[
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Loss,
            Outcome::Loss,
        ],
    );

    assert_eq!(
        summarize_run(&run, None, UTC).classification,
        Classification::Finish
    );
}

#[test]
fn a_dropped_run_is_a_brick() {
    let run = run_with(
        RunState::Dropped,
        &[Outcome::Loss, Outcome::Win, Outcome::Loss, Outcome::Loss],
    );

    assert_eq!(
        summarize_run(&run, None, UTC).classification,
        Classification::Brick
    );
}

#[test]
fn an_active_run_is_live_on_the_next_round() {
    let run = run_with(
        RunState::Active,
        &[Outcome::Win, Outcome::Win, Outcome::Loss],
    );

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.classification, Classification::Live);
    assert_eq!(summary.live_round, Some(4));
}

#[test]
fn an_active_run_pads_its_results_to_five_rounds() {
    let run = run_with(
        RunState::Active,
        &[Outcome::Win, Outcome::Win, Outcome::Loss],
    );

    assert_eq!(
        summarize_run(&run, None, UTC).results,
        [Some(Mark::W), Some(Mark::W), Some(Mark::L), None, None]
    );

    // Only an Active run is padded.
    let dropped = run_with(RunState::Dropped, &[Outcome::Win]);
    assert_eq!(summarize_run(&dropped, None, UTC).results, [Some(Mark::W)]);
}

#[test]
fn a_manual_run_without_matches_is_empty() {
    let run = RunInput {
        course_record: None,
        manual: true,
        uncapped: false,
        ..run_with(RunState::Complete, &[])
    };

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.classification, Classification::Empty);
    assert_eq!(summary.live_round, None);
    assert_eq!(summary.tix_delta, None);

    // A derived run without counted matches is a finish, not empty.
    let derived = run_with(RunState::Complete, &[]);
    assert_eq!(
        summarize_run(&derived, None, UTC).classification,
        Classification::Finish
    );
}

#[test]
fn only_complete_matches_count() {
    let mut run = run_with(RunState::Active, &[Outcome::Win, Outcome::Win]);
    let mut live = played(2, Outcome::Unknown);
    live.state = MatchState::InProgress;
    live.ended_at = None;
    run.matches.push(live);

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.matches.len(), 2);
    assert_eq!(summary.live_round, Some(3));
}

/// #390: a run's complete matches count whatever their deck (0.x counted
/// only deck-linked ones, so a run of deckless matches showed 0-0 while the
/// KPIs counted its matches).
#[test]
fn complete_matches_with_no_deck_count_and_agree_with_the_kpis() {
    let mut run = run_with(
        RunState::Complete,
        &[
            Outcome::Win,
            Outcome::Win,
            Outcome::Loss,
            Outcome::Win,
            Outcome::Win,
        ],
    );
    for played in &mut run.matches {
        played.deck_version_id = None;
        played.deck_id = None;
    }

    let summary = summarize_run(&run, None, UTC);

    let tokens: Vec<&str> = summary
        .matches
        .iter()
        .map(|row| row.token.as_str())
        .collect();
    assert_eq!(tokens, ["m0", "m1", "m2", "m3", "m4"]);
    assert_eq!(
        summary.results,
        [
            Some(Mark::W),
            Some(Mark::W),
            Some(Mark::L),
            Some(Mark::W),
            Some(Mark::W)
        ]
    );
    assert_eq!(summary.classification, Classification::Cash);
    let wins = summary
        .results
        .iter()
        .filter(|mark| **mark == Some(Mark::W))
        .count() as u32;
    assert_eq!(wins, complete_wins(&run));
    let kpis = league_kpis(std::slice::from_ref(&run));
    assert_eq!(kpis.avg_finish, Some(f64::from(wins)));
    assert_eq!(kpis.cash_rate, Some(100.0));
    // A deck page still counts only that deck's matches.
    assert!(summarize_run(&run, Some(DECK), UTC).matches.is_empty());
}

#[test]
fn a_deck_filter_keeps_only_that_decks_matches() {
    let mut run = run_with(RunState::Complete, &[Outcome::Win, Outcome::Win]);
    run.matches[1].deck_id = Some(2);

    let summary = summarize_run(&run, Some(DECK), UTC);

    assert_eq!(summary.matches.len(), 1);
    assert_eq!(summary.matches[0].token, "m0");
}

#[test]
fn a_draw_shows_as_a_loss_mark_but_is_not_a_loss_for_tix() {
    let run = run_with(
        RunState::Complete,
        &[
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Draw,
            Outcome::Loss,
        ],
    );

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.results[3], Some(Mark::L));
    // 3 wins and 1 loss is no finish on the prize table.
    assert_eq!(summary.tix_delta, None);
}

#[test]
fn computes_the_average_match_duration_in_seconds() {
    let mut first = played(0, Outcome::Win);
    first.started_at = utc("2026-04-01 10:00:00");
    first.ended_at = Some(utc("2026-04-01 10:10:00"));
    let mut second = played(1, Outcome::Loss);
    second.started_at = utc("2026-04-01 11:00:00");
    second.ended_at = Some(utc("2026-04-01 11:20:00"));
    let run = RunInput {
        course_record: None,
        matches: vec![first, second],
        ..run_with(RunState::Complete, &[])
    };

    assert_eq!(summarize_run(&run, None, UTC).avg_match_seconds, Some(900));
}

#[test]
fn the_average_duration_rounds_half_away_from_zero() {
    let mut first = played(0, Outcome::Win);
    first.ended_at = Some(first.started_at + 1_000);
    let mut second = played(1, Outcome::Win);
    second.ended_at = Some(second.started_at + 2_000);
    let run = RunInput {
        course_record: None,
        matches: vec![first, second],
        ..run_with(RunState::Complete, &[])
    };

    assert_eq!(summarize_run(&run, None, UTC).avg_match_seconds, Some(2));
}

#[test]
fn no_average_duration_without_an_end() {
    let mut only = played(0, Outcome::Win);
    only.ended_at = None;
    let run = RunInput {
        course_record: None,
        matches: vec![only],
        ..run_with(RunState::Active, &[])
    };

    assert_eq!(summarize_run(&run, None, UTC).avg_match_seconds, None);
}

#[test]
fn the_time_of_day_is_the_most_common_bucket() {
    let mut matches = Vec::new();
    for (index, time) in ["18:00:00", "19:30:00", "14:00:00"].iter().enumerate() {
        let mut one = played(index as i64, Outcome::Win);
        one.started_at = utc(&format!("2026-04-01 {time}"));
        one.ended_at = Some(one.started_at);
        matches.push(one);
    }
    let run = RunInput {
        course_record: None,
        matches,
        ..run_with(RunState::Complete, &[])
    };

    assert_eq!(
        summarize_run(&run, None, UTC).time_of_day,
        Some(TimeOfDay::Evening)
    );
}

#[test]
fn the_time_of_day_uses_local_time_and_ties_go_to_the_first_seen() {
    let mut morning = played(0, Outcome::Win);
    morning.started_at = utc("2026-04-01 05:30:00");
    let mut night = played(1, Outcome::Win);
    night.started_at = utc("2026-04-01 23:30:00");
    let run = RunInput {
        course_record: None,
        matches: vec![morning, night],
        ..run_with(RunState::Complete, &[])
    };

    // 05:30 UTC is night in UTC, 07:30 in Berlin summer time (morning);
    // 23:30 UTC is 01:30 in Berlin (night).
    assert_eq!(
        summarize_run(&run, None, UTC).time_of_day,
        Some(TimeOfDay::Night)
    );
    let berlin: Tz = chrono_tz::Europe::Berlin;
    assert_eq!(
        summarize_run(&run, None, berlin).time_of_day,
        Some(TimeOfDay::Morning)
    );
    assert_eq!(TimeOfDay::Afternoon.as_str(), "afternoon");
}

#[test]
fn aggregates_game_wins_and_the_play_draw_record() {
    let mut only = played(0, Outcome::Win);
    only.games = vec![
        game(mtgo_match::GameResult::Win, Some(true)),
        game(mtgo_match::GameResult::Loss, Some(false)),
        game(mtgo_match::GameResult::Win, Some(true)),
        // No result, no count.
        game(mtgo_match::GameResult::Unknown, Some(true)),
    ];
    let run = RunInput {
        course_record: None,
        matches: vec![only],
        ..run_with(RunState::Complete, &[])
    };

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.game_wins, 2);
    assert_eq!(summary.game_losses, 1);
    assert_eq!(summary.on_play, WinLoss { wins: 2, losses: 0 });
    assert_eq!(summary.on_draw, WinLoss { wins: 0, losses: 1 });
    assert_eq!(summary.matches[0].games.len(), 3);
    assert_eq!(summary.matches[0].games[1].result, Mark::L);
    assert_eq!(summary.matches[0].games[1].on_play, Some(false));
}

#[test]
fn the_tix_delta_reads_the_prize_table_for_a_complete_run() {
    let run = run_with(
        RunState::Complete,
        &[
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Loss,
        ],
    );

    assert_eq!(summarize_run(&run, None, UTC).tix_delta, Some(12.70));
}

#[test]
fn a_live_run_has_no_tix_delta() {
    let run = run_with(RunState::Active, &[Outcome::Win, Outcome::Win]);

    assert_eq!(summarize_run(&run, None, UTC).tix_delta, None);
}

#[test]
fn a_brick_is_paid_at_its_padded_score() {
    // 1 win, 3 losses is padded to 1-4: -10.00.
    let run = run_with(
        RunState::Dropped,
        &[Outcome::Loss, Outcome::Win, Outcome::Loss, Outcome::Loss],
    );

    assert_eq!(summarize_run(&run, None, UTC).tix_delta, Some(-10.0));
}

#[test]
fn the_tix_delta_reads_a_raw_format_code() {
    let run = RunInput {
        course_record: None,
        format: Some("CMODERN".to_string()),
        ..run_with(
            RunState::Complete,
            &[
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Loss,
                Outcome::Loss,
            ],
        )
    };

    assert_eq!(summarize_run(&run, None, UTC).tix_delta, Some(2.62));
}

#[test]
fn exposes_each_matchs_duration() {
    let mut only = played(0, Outcome::Win);
    only.ended_at = Some(only.started_at + 14 * MINUTE);
    let run = RunInput {
        course_record: None,
        matches: vec![only],
        ..run_with(RunState::Complete, &[])
    };

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.matches[0].duration_seconds, Some(14 * 60));
    assert_eq!(summary.matches[0].result, Mark::W);
    assert_eq!(summary.matches[0].started_at, base());
}

#[test]
fn matches_are_listed_in_start_order() {
    let mut run = run_with(RunState::Complete, &[Outcome::Win, Outcome::Loss]);
    run.matches.reverse();

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.matches[0].token, "m0");
    assert_eq!(summary.results, [Some(Mark::W), Some(Mark::L)]);
}

#[test]
fn the_version_label_counts_versions_up_to_the_runs() {
    let versions = [100, 200, 300];

    assert_eq!(version_label(200, &versions), "v2");
    assert_eq!(version_label(300, &versions), "v3");
    // Versions saved in the same instant both count.
    assert_eq!(version_label(200, &[100, 200, 200]), "v3");
}

/// `run` with its matches' opponent archetypes, in match order.
fn facing(mut run: RunInput, archetypes: &[Option<&str>]) -> RunInput {
    for (played, archetype) in run.matches.iter_mut().zip(archetypes) {
        played.opponent_archetype = archetype.map(str::to_string);
    }
    run
}

#[test]
fn computes_top_opponent_archetype_and_top_matchups_list() {
    // 0.x "computes top opponent archetype and top matchups list": most
    // faced first, then the better record; three at most; matches with no
    // archetype left out.
    let run = facing(
        run_with(
            RunState::Complete,
            &[
                Outcome::Win,
                Outcome::Loss,
                Outcome::Win,
                Outcome::Loss,
                Outcome::Win,
            ],
        ),
        &[
            Some("Qyawgmoth"),
            Some("Qyawgmoth"),
            Some("Qburn"),
            Some("Qhammer"),
            None,
        ],
    );

    let summary = summarize_run(&run, None, UTC);

    assert_eq!(summary.top_opponent_archetype.as_deref(), Some("Qyawgmoth"));
    assert_eq!(
        summary.top_matchups,
        vec![
            TopMatchup {
                archetype: "Qyawgmoth".to_string(),
                wins: 1,
                losses: 1,
                count: 2,
            },
            TopMatchup {
                archetype: "Qburn".to_string(),
                wins: 1,
                losses: 0,
                count: 1,
            },
            TopMatchup {
                archetype: "Qhammer".to_string(),
                wins: 0,
                losses: 1,
                count: 1,
            },
        ]
    );
    assert_eq!(
        summary.matches[0].opponent_archetype.as_deref(),
        Some("Qyawgmoth")
    );
    assert_eq!(summary.matches[4].opponent_archetype, None);
}

#[test]
fn a_runs_top_matchups_count_a_draw_as_a_loss_and_the_kpi_does_not() {
    // 0.x FormatLeagueRuns::computeStats counts `won === false` as a loss;
    // GetLeagueKpis counts `outcome = 'loss'` only.
    let run = facing(
        run_with(RunState::Complete, &[Outcome::Draw, Outcome::Win]),
        &[Some("Qburn"), Some("Qburn")],
    );
    let summary = summarize_run(&run, None, UTC);
    assert_eq!(
        (summary.top_matchups[0].wins, summary.top_matchups[0].losses),
        (1, 1)
    );
    let kpi = league_kpis(&[run]).top_matchup.expect("top matchup");
    assert_eq!((kpi.wins, kpi.losses, kpi.count), (1, 0, 2));
}

#[test]
fn top_matchups_keep_four_archetypes_to_three_and_count_only_counted_matches() {
    let mut run = facing(
        run_with(
            RunState::Complete,
            &[
                Outcome::Loss,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
            ],
        ),
        &[Some("Qa"), Some("Qb"), Some("Qc"), Some("Qd"), Some("Qd")],
    );
    // Not complete: not counted.
    run.matches[4].state = MatchState::Ended;

    let summary = summarize_run(&run, None, UTC);
    let names: Vec<&str> = summary
        .top_matchups
        .iter()
        .map(|matchup| matchup.archetype.as_str())
        .collect();
    // All once now: the wins first, ties in the order first faced.
    assert_eq!(names, ["Qb", "Qc", "Qd"]);
    let none = summarize_run(&run_with(RunState::Complete, &[Outcome::Win]), None, UTC);
    assert_eq!(none.top_opponent_archetype, None);
    assert!(none.top_matchups.is_empty());
}

// 0.x GetLeagueKpisTest

#[test]
fn kpis_count_runs_trophies_cash_and_average_finish() {
    let runs = vec![
        seeded(RunState::Complete, 5, 0),
        seeded(RunState::Complete, 4, 1),
        seeded(RunState::Complete, 3, 2),
        seeded(RunState::Active, 1, 0),
    ];

    let kpis = league_kpis(&runs);

    assert_eq!(kpis.total, 4);
    assert_eq!(kpis.completed, 3);
    assert_eq!(kpis.live, 1);
    assert_eq!(kpis.trophies, 1);
    assert_eq!(kpis.trophy_rate, Some(33.0));
    assert_eq!(kpis.cash_rate, Some(67.0));
    assert_eq!(kpis.avg_finish, Some(4.0));
}

#[test]
fn kpis_count_dropped_runs_as_completed() {
    let runs = vec![
        seeded(RunState::Complete, 5, 0),
        seeded(RunState::Dropped, 2, 2),
    ];

    let kpis = league_kpis(&runs);

    assert_eq!(kpis.completed, 2);
    assert_eq!(kpis.trophies, 1);
    assert_eq!(kpis.trophy_rate, Some(50.0));
    assert_eq!(kpis.avg_finish, Some(3.5));
}

#[test]
fn kpis_have_no_rates_without_completed_runs() {
    let kpis = league_kpis(&[seeded(RunState::Active, 1, 0)]);

    assert_eq!(kpis.trophy_rate, None);
    assert_eq!(kpis.cash_rate, None);
    assert_eq!(kpis.avg_finish, None);
    assert_eq!(kpis.dropped, 0);
    assert_eq!(kpis.drop_rate, None);
}

#[test]
fn returns_top_matchup_with_record_and_play_count() {
    // 0.x GetLeagueKpisTest "returns top matchup with record and play
    // count": over every complete match of the runs.
    let runs = vec![
        facing(
            seeded(RunState::Complete, 3, 0),
            &[Some("Qyawgmoth"), Some("Qyawgmoth"), Some("Qyawgmoth")],
        ),
        facing(seeded(RunState::Active, 0, 1), &[Some("Qburn")]),
    ];

    let kpis = league_kpis(&runs);

    assert_eq!(
        kpis.top_matchup,
        Some(TopMatchup {
            archetype: "Qyawgmoth".to_string(),
            wins: 3,
            losses: 0,
            count: 3,
        })
    );
    assert_eq!(
        league_kpis(&[seeded(RunState::Complete, 5, 0)]).top_matchup,
        None
    );
}

#[test]
fn kpis_count_drops_and_the_drop_rate() {
    let runs = vec![
        seeded(RunState::Complete, 5, 0),
        seeded(RunState::Complete, 3, 2),
        seeded(RunState::Dropped, 1, 2),
        seeded(RunState::Active, 1, 0),
    ];

    let kpis = league_kpis(&runs);

    assert_eq!(kpis.dropped, 1);
    assert_eq!(kpis.drop_rate, Some(33.0));
}

#[test]
fn kpis_count_distinct_decks() {
    let mut other = seeded(RunState::Complete, 4, 1);
    for one in &mut other.matches {
        one.deck_id = Some(2);
        one.deck_version_id = Some(20);
    }
    let runs = vec![seeded(RunState::Complete, 5, 0), other];

    assert_eq!(league_kpis(&runs).decks, 2);
}

#[test]
fn kpis_of_nothing_are_empty() {
    let kpis = league_kpis(&[]);

    assert_eq!(kpis.total, 0);
    assert_eq!(kpis.completed, 0);
    assert_eq!(kpis.decks, 0);
    assert_eq!(kpis.trophy_rate, None);
}

#[test]
fn kpis_leave_partial_runs_out_of_completed() {
    let kpis = league_kpis(&[seeded(RunState::Partial, 3, 0)]);

    assert_eq!(kpis.total, 1);
    assert_eq!(kpis.completed, 0);
    assert_eq!(kpis.live, 0);
}

// 0.x LeagueEvTableTest (feature)

#[test]
fn net_tix_per_modern_finish() {
    assert_eq!(net_tix("Modern", 5, 0), Some(29.02));
    assert_eq!(net_tix("Modern", 4, 1), Some(12.70));
    assert_eq!(net_tix("Modern", 3, 2), Some(2.62));
    assert_eq!(net_tix("Modern", 2, 3), Some(-5.00));
    assert_eq!(net_tix("Modern", 1, 4), Some(-10.00));
    assert_eq!(net_tix("Modern", 0, 5), Some(-10.00));
}

#[test]
fn net_tix_is_none_for_an_unknown_format_or_score() {
    assert_eq!(net_tix("Frontier", 5, 0), None);
    assert_eq!(net_tix("Modern", 6, 0), None);
}

#[test]
fn net_tix_matches_the_format_in_any_case() {
    assert_eq!(net_tix("modern", 5, 0), Some(29.02));
    assert_eq!(net_tix("MODERN", 5, 0), Some(29.02));
    for format in ["Pioneer", "Legacy", "Standard", "Vintage", "Pauper"] {
        assert_eq!(net_tix(format, 5, 0), Some(29.02), "{format}");
    }
}

#[test]
fn the_ev_table_lists_every_finish() {
    assert_eq!(
        ev_table(),
        [
            (5, 0, 29.02),
            (4, 1, 12.70),
            (3, 2, 2.62),
            (2, 3, -5.00),
            (1, 4, -10.00),
            (0, 5, -10.00),
        ]
    );
}

// 0.x LeagueEvTableTest (unit)

#[test]
fn a_completed_run_reads_the_prize_table() {
    assert_eq!(
        net_tix_for_run("modern", RunState::Complete, 3, 2),
        Some(2.62)
    );
}

#[test]
fn a_dropped_run_pays_unplayed_rounds_as_losses() {
    assert_eq!(
        net_tix_for_run("modern", RunState::Dropped, 1, 2),
        Some(-10.0)
    );
}

#[test]
fn a_run_still_being_played_has_no_figure() {
    assert_eq!(net_tix_for_run("modern", RunState::Active, 1, 1), None);
    assert_eq!(net_tix_for_run("modern", RunState::Partial, 3, 2), None);
}

#[test]
fn a_completed_run_without_matches_has_no_figure() {
    assert_eq!(net_tix_for_run("modern", RunState::Complete, 0, 0), None);
}

#[test]
fn an_unpriced_format_has_no_figure() {
    assert_eq!(net_tix_for_run("limited", RunState::Complete, 3, 0), None);
}

#[test]
fn the_entry_costs_ten_tix_in_priced_formats() {
    assert_eq!(entry_cost("modern"), Some(10.0));
    assert_eq!(entry_cost("limited"), None);
}

#[test]
fn break_even_sits_between_the_last_brick_and_the_first_prize() {
    // 2-3 nets -5.00 and 3-2 nets +2.62.
    assert_eq!(break_even_wins("modern", 5), Some(2.7));
    assert_eq!(break_even_wins("limited", 3), None);
}

// 0.x LeagueFinishOddsTest

#[test]
fn the_prize_threshold_is_the_lowest_paying_finish() {
    let odds = finish_odds("modern", 1, 1, 5, 0.5);

    assert_eq!(odds.prize_wins, Some(3));
    assert_eq!(odds.wins_for_prize, Some(2));
    assert_eq!(odds.rounds_left, 3);
}

#[test]
fn two_wins_in_three_rounds_at_a_coin_flip_is_even() {
    assert_eq!(
        finish_odds("modern", 1, 1, 5, 0.5).chance_of_prize,
        Some(50)
    );
}

#[test]
fn a_finished_run_is_settled() {
    let odds = finish_odds("modern", 3, 2, 5, 0.5);

    assert_eq!(odds.rounds_left, 0);
    assert_eq!(odds.wins_for_prize, Some(0));
    assert_eq!(odds.chance_of_prize, Some(100));
    assert_eq!(odds.expected_tix, Some(2.62));
}

#[test]
fn expected_tix_weights_every_reachable_finish() {
    // Half the time 5-0 at 29.02, half the time 4-1 at 12.70.
    assert_eq!(
        finish_odds("modern", 4, 0, 5, 0.5).expected_tix,
        Some(20.86)
    );
}

#[test]
fn an_unpriced_format_has_no_odds() {
    let odds = finish_odds("limited", 1, 1, 3, 0.5);

    assert_eq!(odds.rounds_left, 1);
    assert_eq!(odds.expected_tix, None);
    assert_eq!(odds.prize_wins, None);
    assert_eq!(odds.wins_for_prize, None);
    assert_eq!(odds.chance_of_prize, None);
}

// 0.x GetLeagueInProgressTest

#[test]
fn the_win_probability_comes_from_the_league_record_with_ten_matches() {
    // 10-4 in leagues: 71%.
    let (probability, source) = win_probability(
        &Record::from_counts(10, 4, 0),
        &Record::from_counts(30, 30, 0),
    );

    assert_eq!(source, OddsSource::League);
    assert_eq!(probability, 0.71);
    assert_eq!(source.as_str(), "league");
}

#[test]
fn the_win_probability_falls_back_to_the_decks_record() {
    let (probability, source) = win_probability(
        &Record::from_counts(1, 1, 0),
        &Record::from_counts(11, 11, 0),
    );

    assert_eq!(source, OddsSource::Deck);
    assert_eq!(probability, 0.5);
}

#[test]
fn the_win_probability_is_a_coin_flip_without_a_record() {
    let (probability, source) = win_probability(&Record::default(), &Record::default());

    assert_eq!(source, OddsSource::None);
    assert_eq!(probability, 0.5);
    assert_eq!(source.as_str(), "none");
}

#[test]
fn nothing_is_in_progress_without_an_active_run() {
    let runs = vec![seeded(RunState::Complete, 3, 2)];

    assert_eq!(in_progress_index(&runs, DECK, None), None);
}

#[test]
fn reports_the_run_under_way_with_the_rounds_it_has_left() {
    let runs = vec![
        seeded(RunState::Complete, 3, 2),
        seeded(RunState::Active, 1, 1),
    ];

    let index = in_progress_index(&runs, DECK, None);
    assert_eq!(index, Some(1));

    let progress = league_in_progress(
        &runs[1],
        &Record::from_counts(1, 1, 0),
        &Record::from_counts(1, 1, 0),
    );

    assert_eq!(progress.wins, 1);
    assert_eq!(progress.losses, 1);
    assert_eq!(progress.odds.rounds_left, 3);
    assert_eq!(progress.odds.wins_for_prize, Some(2));
}

#[test]
fn predicts_the_run_from_the_decks_league_record() {
    let run = seeded(RunState::Active, 1, 1);

    let progress = league_in_progress(
        &run,
        &Record::from_counts(10, 4, 0),
        &Record::from_counts(10, 4, 0),
    );

    assert_eq!(progress.odds_source, OddsSource::League);
    assert_eq!(progress.win_probability, 71);
}

#[test]
fn falls_back_to_the_decks_overall_record_when_it_has_barely_played_leagues() {
    let run = seeded(RunState::Active, 1, 1);

    let progress = league_in_progress(
        &run,
        &Record::from_counts(1, 1, 0),
        &Record::from_counts(11, 11, 0),
    );

    assert_eq!(progress.odds_source, OddsSource::Deck);
    assert_eq!(progress.win_probability, 50);
    assert_eq!(progress.odds.chance_of_prize, Some(50));
}

#[test]
fn the_run_in_progress_is_the_newest_active_run_with_the_decks_matches() {
    let mut older = seeded(RunState::Active, 1, 0);
    older.started_at = base() - HOUR;
    let mut newer = seeded(RunState::Active, 2, 0);
    newer.started_at = base();
    let mut other_deck = seeded(RunState::Active, 1, 0);
    other_deck.started_at = base() + HOUR;
    for one in &mut other_deck.matches {
        one.deck_id = Some(2);
        one.deck_version_id = Some(20);
    }
    // A match still being played counts for picking the run.
    let mut starting = seeded(RunState::Active, 0, 0);
    starting.started_at = base() + 2 * HOUR;
    let mut first = played(0, Outcome::Unknown);
    first.state = MatchState::InProgress;
    first.deck_version_id = Some(11);
    starting.matches.push(first);
    let runs = vec![older, newer, other_deck, starting];

    assert_eq!(in_progress_index(&runs, DECK, None), Some(3));
    assert_eq!(in_progress_index(&runs, DECK, Some(VERSION)), Some(1));
    assert_eq!(in_progress_index(&runs, 2, None), Some(2));
}

#[test]
fn the_latest_finished_run_is_the_newest_complete_or_dropped_run_with_the_decks_matches() {
    let mut complete = seeded(RunState::Complete, 3, 2);
    complete.started_at = base() - 2 * HOUR;
    let mut dropped = seeded(RunState::Dropped, 1, 2);
    dropped.started_at = base() - HOUR;
    let mut partial = seeded(RunState::Partial, 1, 0);
    partial.started_at = base();
    let mut active = seeded(RunState::Active, 1, 0);
    active.started_at = base() + HOUR;
    let runs = vec![complete, dropped, partial, active];

    assert_eq!(latest_finished_index(&runs, DECK, None), Some(1));
    assert_eq!(latest_finished_index(&runs, DECK, Some(99)), None);
    assert_eq!(latest_finished_index(&runs, 2, None), None);
}

// 0.x GetLeagueResultDistributionTest

#[test]
fn the_distribution_is_empty_without_runs() {
    let distribution = result_distribution(&[], DECK, None);

    assert_eq!(
        distribution.buckets,
        [
            ("5-0".to_string(), 0),
            ("4-1".to_string(), 0),
            ("3-2".to_string(), 0),
            ("2-3".to_string(), 0),
            ("1-4".to_string(), 0),
            ("0-5".to_string(), 0),
        ]
    );
    assert_eq!(distribution.dropped, 0);
}

#[test]
fn dropped_runs_count_apart_from_finishes() {
    let runs = vec![
        seeded(RunState::Complete, 4, 1),
        seeded(RunState::Dropped, 1, 2),
        seeded(RunState::Dropped, 0, 1),
    ];

    let distribution = result_distribution(&runs, DECK, None);

    assert_eq!(distribution.buckets[1], ("4-1".to_string(), 1));
    assert_eq!(distribution.buckets[4], ("1-4".to_string(), 0));
    assert_eq!(distribution.dropped, 2);
}

#[test]
fn partial_runs_are_not_drops() {
    let distribution = result_distribution(&[seeded(RunState::Partial, 1, 1)], DECK, None);

    assert_eq!(distribution.dropped, 0);
}

#[test]
fn the_distribution_counts_only_the_decks_runs_and_records() {
    let mut mixed = seeded(RunState::Complete, 4, 1);
    // One win played with another deck: the deck's own record is 3-1, no bucket.
    mixed.matches[0].deck_id = Some(2);
    let mut elsewhere = seeded(RunState::Complete, 5, 0);
    for one in &mut elsewhere.matches {
        one.deck_id = Some(2);
    }
    let runs = vec![mixed, elsewhere, seeded(RunState::Complete, 3, 2)];

    let distribution = result_distribution(&runs, DECK, None);

    assert_eq!(distribution.buckets[0].1, 0);
    assert_eq!(distribution.buckets[1].1, 0);
    assert_eq!(distribution.buckets[2].1, 1);
}

#[test]
fn a_version_filter_picks_runs_but_records_count_the_whole_deck() {
    let mut run = seeded(RunState::Complete, 4, 1);
    run.matches[0].deck_version_id = Some(11);
    let runs = vec![run];

    assert_eq!(result_distribution(&runs, DECK, Some(11)).buckets[1].1, 1);
    assert_eq!(result_distribution(&runs, DECK, Some(99)).buckets[1].1, 0);
}

// 0.x GetDeckViewSharedProps: sidebar trophies

#[test]
fn a_trophy_is_a_complete_run_where_every_deck_match_won() {
    let runs = vec![
        seeded(RunState::Complete, 5, 0),
        seeded(RunState::Complete, 4, 1),
        // The 0.x rule has no five-match check.
        seeded(RunState::Complete, 3, 0),
        // Only Complete runs.
        seeded(RunState::Dropped, 2, 0),
        seeded(RunState::Active, 2, 0),
        // No matches of the deck, no trophy.
        seeded(RunState::Complete, 0, 0),
    ];

    assert_eq!(trophy_count(&runs, DECK), 2);
}

#[test]
fn a_trophy_reads_only_the_decks_complete_matches() {
    let mut run = seeded(RunState::Complete, 4, 1);
    run.matches[4].deck_id = Some(2);
    let mut live = played(5, Outcome::Loss);
    live.state = MatchState::InProgress;
    run.matches.push(live);

    assert_eq!(trophy_count(&[run], DECK), 1);
}

// Leagues page base and state chips (0.x IndexController::buildQuery)

#[test]
fn the_page_lists_manual_runs_and_runs_with_a_complete_match() {
    assert!(on_leagues_page(&seeded(RunState::Active, 1, 0)));

    let mut starting = seeded(RunState::Active, 0, 0);
    let mut first = played(0, Outcome::Unknown);
    first.state = MatchState::InProgress;
    starting.matches.push(first);
    assert!(!on_leagues_page(&starting));

    let manual = RunInput {
        course_record: None,
        manual: true,
        uncapped: false,
        ..seeded(RunState::Complete, 0, 0)
    };
    assert!(on_leagues_page(&manual));
}

#[test]
fn state_chips_filter_like_0x() {
    let trophy = seeded(RunState::Complete, 5, 0);
    let cash = seeded(RunState::Complete, 4, 1);
    let finish = seeded(RunState::Complete, 2, 3);
    let live = seeded(RunState::Active, 1, 0);
    let brick = seeded(RunState::Dropped, 1, 1);

    assert!(passes_state_filter("all", &partial_run()));
    assert!(passes_state_filter("live", &live));
    assert!(!passes_state_filter("live", &trophy));
    assert!(passes_state_filter("trophies", &trophy));
    assert!(!passes_state_filter("trophies", &cash));
    assert!(passes_state_filter("cash", &trophy));
    assert!(passes_state_filter("cash", &cash));
    assert!(!passes_state_filter("cash", &finish));
    assert!(passes_state_filter("finish", &finish));
    assert!(!passes_state_filter("finish", &cash));
    assert!(passes_state_filter("bricks", &brick));
    assert!(!passes_state_filter("bricks", &live));
    // An unknown chip filters nothing, as 0.x.
    assert!(passes_state_filter("other", &brick));
}

fn partial_run() -> RunInput {
    seeded(RunState::Partial, 1, 0)
}

#[test]
fn complete_wins_count_every_complete_win_of_the_run() {
    let mut run = seeded(RunState::Complete, 3, 2);
    run.matches[0].deck_id = None;
    let mut live = played(6, Outcome::Win);
    live.state = MatchState::InProgress;
    run.matches.push(live);

    assert_eq!(complete_wins(&run), 3);
}

// 0.x FormatTournamentRunsTest

fn tournament_match(index: i64, outcome: Outcome, round: Option<i64>) -> TournamentMatch {
    let started_at = base() + index * HOUR;
    TournamentMatch {
        token: format!("t{index}"),
        state: MatchState::Complete,
        outcome,
        started_at,
        ended_at: Some(started_at + 15 * MINUTE),
        deck_version_id: Some(VERSION),
        round,
        games: Vec::new(),
    }
}

fn tournament(matches: Vec<TournamentMatch>) -> TournamentInput {
    TournamentInput {
        event_id: 1234,
        format: Some("CMODERN".to_string()),
        started_at: base(),
        matches,
    }
}

fn versions() -> Vec<DeckVersionDate> {
    vec![
        DeckVersionDate {
            id: VERSION,
            modified_at: 100,
        },
        DeckVersionDate {
            id: 11,
            modified_at: 200,
        },
        DeckVersionDate {
            id: 12,
            modified_at: 300,
        },
    ]
}

#[test]
fn a_tournament_has_its_record_and_rounds() {
    let input = tournament(vec![
        tournament_match(0, Outcome::Win, Some(1)),
        tournament_match(1, Outcome::Loss, Some(2)),
    ]);

    let row = tournament_run(&input, &versions(), UTC);

    assert_eq!(row.event_id, 1234);
    assert_eq!(row.name, "Tournament 1234 (01-04-2026)");
    assert_eq!(row.format, "Modern");
    assert_eq!(row.started_at, base());
    assert_eq!(row.results, [Mark::W, Mark::L]);
    assert_eq!(row.matches.len(), 2);
    assert_eq!(row.matches[0].result, Some(Mark::W));
    assert_eq!(row.matches[0].round, Some(1));
    assert_eq!(row.matches[1].round, Some(2));
    assert_eq!(row.wins, 1);
    assert_eq!(row.losses, 1);
    assert_eq!(row.matches_count, 2);
    assert_eq!(row.in_progress_count, 0);
    assert_eq!(row.version_label.as_deref(), Some("v1"));
    assert!(row.avg_match_seconds.unwrap_or(0) > 0);
}

#[test]
fn a_tournament_without_a_format_shows_an_empty_one() {
    let input = TournamentInput {
        format: None,
        ..tournament(vec![tournament_match(0, Outcome::Win, None)])
    };

    assert_eq!(tournament_run(&input, &versions(), UTC).format, "");
}

#[test]
fn tournament_results_are_not_padded() {
    let row = tournament_run(
        &tournament(vec![tournament_match(0, Outcome::Win, None)]),
        &versions(),
        UTC,
    );

    assert_eq!(row.results, [Mark::W]);
}

#[test]
fn a_tournament_averages_its_complete_matches() {
    let mut first = tournament_match(0, Outcome::Win, None);
    first.started_at = utc("2026-04-01 10:00:00");
    first.ended_at = Some(utc("2026-04-01 10:10:00"));
    let mut second = tournament_match(1, Outcome::Loss, None);
    second.started_at = utc("2026-04-01 11:00:00");
    second.ended_at = Some(utc("2026-04-01 11:20:00"));

    let row = tournament_run(&tournament(vec![first, second]), &versions(), UTC);

    assert_eq!(row.avg_match_seconds, Some(900));
}

#[test]
fn a_tournament_aggregates_game_wins_and_the_play_draw_record() {
    let mut only = tournament_match(0, Outcome::Win, None);
    only.games = vec![
        game(mtgo_match::GameResult::Win, Some(true)),
        game(mtgo_match::GameResult::Loss, Some(false)),
        game(mtgo_match::GameResult::Win, Some(true)),
    ];

    let row = tournament_run(&tournament(vec![only]), &versions(), UTC);

    assert_eq!(row.game_wins, 2);
    assert_eq!(row.game_losses, 1);
    assert_eq!(row.on_play, WinLoss { wins: 2, losses: 0 });
    assert_eq!(row.on_draw, WinLoss { wins: 0, losses: 1 });
}

#[test]
fn a_tournament_exposes_each_matchs_duration_and_round() {
    let mut only = tournament_match(0, Outcome::Win, Some(3));
    only.ended_at = Some(only.started_at + 14 * MINUTE);

    let row = tournament_run(&tournament(vec![only]), &versions(), UTC);

    assert_eq!(row.matches[0].duration_seconds, Some(14 * 60));
    assert_eq!(row.matches[0].round, Some(3));
}

#[test]
fn a_match_in_progress_has_no_result_and_counts_apart() {
    let mut live = tournament_match(1, Outcome::Unknown, Some(2));
    live.state = MatchState::InProgress;
    live.ended_at = None;
    live.games = vec![game(mtgo_match::GameResult::Win, Some(true))];
    let mut first = tournament_match(0, Outcome::Win, Some(1));
    first.games = vec![game(mtgo_match::GameResult::Loss, Some(false))];

    let row = tournament_run(&tournament(vec![first, live]), &versions(), UTC);

    assert_eq!(row.matches[1].result, None);
    assert_eq!(row.matches[1].state, MatchState::InProgress);
    assert_eq!(row.matches[1].games.len(), 1);
    assert_eq!(row.results, [Mark::W]);
    assert_eq!(row.in_progress_count, 1);
    assert_eq!(row.matches_count, 2);
    // Game numbers count complete matches only.
    assert_eq!(row.game_wins, 0);
    assert_eq!(row.game_losses, 1);
}

#[test]
fn a_tournament_version_label_spans_the_versions_played() {
    let mut first = tournament_match(0, Outcome::Win, None);
    first.deck_version_id = Some(12);
    let second = tournament_match(1, Outcome::Win, None);
    let mut third = tournament_match(2, Outcome::Win, None);
    third.deck_version_id = None;

    let row = tournament_run(&tournament(vec![first, second, third]), &versions(), UTC);

    assert_eq!(row.version_label.as_deref(), Some("v1\u{2013}v3"));

    let unknown = TournamentMatch {
        deck_version_id: Some(99),
        ..tournament_match(0, Outcome::Win, None)
    };
    let row = tournament_run(&tournament(vec![unknown]), &versions(), UTC);
    assert_eq!(row.version_label, None);
}

#[test]
fn a_tournament_name_uses_the_local_day() {
    let london: Tz = chrono_tz::Europe::London;

    // 23:30 UTC on 30 June is 00:30 on 1 July in London.
    assert_eq!(
        tournament_name(55, utc("2026-06-30 23:30:00"), london),
        "Tournament 55 (01-07-2026)"
    );
}

// Cross-device sync spec section 8: an Arena course's stated record counts
// matches played where MyMTGO never saw them.

#[test]
fn a_course_record_counts_in_the_finish_and_its_tix() {
    let mut run = seeded(RunState::Complete, 2, 0);
    run.course_record = Some((5, 0));
    let summary = summarize_run(&run, None, UTC);
    let five_oh = summarize_run(&seeded(RunState::Complete, 5, 0), None, UTC);
    assert_eq!(summary.classification, Classification::Trophy);
    assert_eq!(summary.tix_delta, five_oh.tix_delta);
    assert_eq!(summary.results.len(), 2, "the tracked matches only");
    assert_eq!(summary.matches.len(), 2);
}

#[test]
fn a_course_record_moves_the_live_round() {
    let mut run = seeded(RunState::Active, 1, 0);
    run.course_record = Some((3, 1));
    assert_eq!(summarize_run(&run, None, UTC).live_round, Some(5));
    run.course_record = None;
    assert_eq!(summarize_run(&run, None, UTC).live_round, Some(2));
}

#[test]
fn the_shown_record_never_drops_below_the_tracked_one() {
    assert_eq!(shown_record(None, (2, 1)), (2, 1));
    assert_eq!(shown_record(Some((5, 1)), (2, 1)), (5, 1));
    // A wrong merge put more tracked matches in the run than the course
    // counted: they show until the player takes them out.
    assert_eq!(shown_record(Some((1, 0)), (3, 2)), (3, 2));
}
