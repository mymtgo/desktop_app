//! Run numbers (leagues spec section 7).

use std::borrow::Borrow;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use decks::display_format;
use decks::php_round;
use decks::stats::Record;
use mtgo_match::{MatchState, Outcome};

use crate::ev::net_tix_for_run;
use crate::fold::{CONSTRUCTED_ROUNDS, GameResult, RunState};
use crate::odds::{FinishOdds, OddsSource, finish_odds, win_probability};

/// A match of a run, as the numbers read it. Derived, manual and imported
/// runs all give their matches in this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMatch {
    /// The match token (the app joins opponent names and notes by it).
    pub token: String,
    pub state: MatchState,
    pub outcome: Outcome,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub deck_version_id: Option<i64>,
    /// The deck of `deck_version_id`.
    pub deck_id: Option<i64>,
    /// Games in play order.
    pub games: Vec<GameResult>,
    /// The opponent seat's archetype name, when the match has one.
    pub opponent_archetype: Option<String>,
}

/// A league run for the numbers: a derived run (`fold::Run` with its
/// matches), a manual league (always Complete) or an imported 0.x run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInput {
    pub state: RunState,
    /// A user-created league.
    pub manual: bool,
    /// A run with no known round count (an Arena run): no padding slots and
    /// no Trophy, Cash or Brick, only Live or Finish.
    pub uncapped: bool,
    /// The format code (`CMODERN`) or label (`Modern`).
    pub format: Option<String>,
    pub started_at: i64,
    /// Every match of the run, in any state and of any deck.
    pub matches: Vec<RunMatch>,
    /// The run's record as its course stated it (an Arena course run:
    /// `leagues.course_wins`, `course_losses`), which counts matches played
    /// where MyMTGO never saw them. `None` for every other run.
    pub course_record: Option<(u32, u32)>,
}

/// The record a run shows: its tracked wins and losses, each raised to the
/// course's stated count when that is higher (matches played elsewhere).
/// The tracked list itself is never padded: the player corrects a wrong
/// merge by editing the tracked matches, and the course record shows again.
pub fn shown_record(course_record: Option<(u32, u32)>, tracked: (u32, u32)) -> (u32, u32) {
    match course_record {
        Some((wins, losses)) => (wins.max(tracked.0), losses.max(tracked.1)),
        None => tracked,
    }
}

/// A won (`W`) or lost (`L`) match or game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    W,
    L,
}

impl Mark {
    /// `W` or `L`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mark::W => "W",
            Mark::L => "L",
        }
    }
}

/// How a run card reads (0.x `classification`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Empty,
    Live,
    Brick,
    Trophy,
    Cash,
    Finish,
}

impl Classification {
    /// 0.x's labels: `EMPTY`, `LIVE`, `BRICK`, `TROPHY`, `CASH`, `FINISH`.
    pub fn as_str(self) -> &'static str {
        match self {
            Classification::Empty => "EMPTY",
            Classification::Live => "LIVE",
            Classification::Brick => "BRICK",
            Classification::Trophy => "TROPHY",
            Classification::Cash => "CASH",
            Classification::Finish => "FINISH",
        }
    }
}

/// When a run was mostly played, in local time: morning 6 to 12, afternoon
/// 12 to 17, evening 17 to 22, night otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeOfDay {
    Morning,
    Afternoon,
    Evening,
    Night,
}

impl TimeOfDay {
    /// `morning`, `afternoon`, `evening` or `night`.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeOfDay::Morning => "morning",
            TimeOfDay::Afternoon => "afternoon",
            TimeOfDay::Evening => "evening",
            TimeOfDay::Night => "night",
        }
    }
}

/// Games won and lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WinLoss {
    pub wins: u32,
    pub losses: u32,
}

/// One game with a result, and whether the player was on the play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameMark {
    pub result: Mark,
    pub on_play: Option<bool>,
}

/// One counted match of a run card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMatchRow {
    pub token: String,
    /// W when won, L otherwise (a draw shows L, as in 0.x).
    pub result: Mark,
    /// Games with a result, in play order.
    pub games: Vec<GameMark>,
    pub started_at: i64,
    /// `None` when the match never ended.
    pub duration_seconds: Option<i64>,
    /// The opponent seat's archetype name.
    pub opponent_archetype: Option<String>,
}

/// One opponent archetype faced in some matches, with the record against
/// it (0.x `topMatchups` and the KPI `topMatchup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopMatchup {
    pub archetype: String,
    pub wins: u32,
    pub losses: u32,
    /// Matches against it (draws included).
    pub count: u32,
}

/// A run card's numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub classification: Classification,
    /// The round being played, for a LIVE run.
    pub live_round: Option<u32>,
    /// One mark per counted match; an Active run is padded with `None` to 5.
    pub results: Vec<Option<Mark>>,
    pub matches: Vec<RunMatchRow>,
    pub avg_match_seconds: Option<i64>,
    pub time_of_day: Option<TimeOfDay>,
    pub game_wins: u32,
    pub game_losses: u32,
    pub on_play: WinLoss,
    pub on_draw: WinLoss,
    /// Net tix of the finish (`ev::net_tix_for_run`).
    pub tix_delta: Option<f64>,
    /// The most faced opponent archetype (0.x `topOpponentArchetype`).
    pub top_opponent_archetype: Option<String>,
    /// The three most faced opponent archetypes (0.x `topMatchups`).
    pub top_matchups: Vec<TopMatchup>,
}

/// The Leagues page KPI strip. Rates are whole percentages of `completed`,
/// `None` without completed runs.
#[derive(Debug, Clone, PartialEq)]
pub struct LeagueKpis {
    /// Every run.
    pub total: u32,
    /// Complete and Dropped runs.
    pub completed: u32,
    /// Active runs.
    pub live: u32,
    /// Distinct decks played.
    pub decks: u32,
    /// Completed runs with 5 wins.
    pub trophies: u32,
    pub trophy_rate: Option<f64>,
    /// Completed runs with 4 or more wins.
    pub cash_rate: Option<f64>,
    pub dropped: u32,
    pub drop_rate: Option<f64>,
    /// Average wins of completed runs, to one place.
    pub avg_finish: Option<f64>,
    /// The most faced opponent archetype over every complete match of the
    /// runs (0.x `topMatchup`).
    pub top_matchup: Option<TopMatchup>,
}

/// The deck dashboard's "League Finishes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultDistribution {
    /// `("5-0", runs)` down to `("0-5", runs)`.
    pub buckets: Vec<(String, u32)>,
    /// Dropped runs.
    pub dropped: u32,
}

/// A run in progress with where it is likely to finish.
#[derive(Debug, Clone, PartialEq)]
pub struct InProgress {
    pub wins: u32,
    pub losses: u32,
    /// The per-round win probability used, as a whole percentage.
    pub win_probability: u32,
    pub odds_source: OddsSource,
    pub odds: FinishOdds,
}

/// One league run's numbers (0.x `FormatLeagueRuns::formatRun`). The counted
/// matches are the run's complete matches, whatever their deck, and only
/// `deck_id`'s when given (the deck pages), in start order. 0.x joined
/// `deck_versions`, so a match with no deck link did not count; v1 counts
/// it, as the KPIs do (#390, parity allowlist).
pub fn summarize_run(run: &RunInput, deck_id: Option<i64>, tz: Tz) -> RunSummary {
    let counted = counted_matches(run, deck_id);
    summarize(run, counted, CONSTRUCTED_ROUNDS, tz)
}

/// A draft or sealed run's numbers (0.x `FormatLeagueRuns::formatRun` for a
/// limited league): classified and padded against the run's `rounds`
/// (`Run::round_count`; a draft trophy is 3-0). Every complete match counts: 0.x counted the
/// matches linked to its synthetic limited deck, which v1 does not make
/// (limited spec ruling 3).
pub fn summarize_limited_run(run: &RunInput, rounds: usize, tz: Tz) -> RunSummary {
    let mut counted: Vec<&RunMatch> = run
        .matches
        .iter()
        .filter(|played| played.state == MatchState::Complete)
        .collect();
    counted.sort_by_key(|played| played.started_at);
    summarize(run, counted, rounds, tz)
}

/// The numbers of `counted` (in start order) for a run of `rounds` rounds.
fn summarize(run: &RunInput, counted: Vec<&RunMatch>, rounds: usize, tz: Tz) -> RunSummary {
    let tracked = (
        count_outcome(&counted, Outcome::Win),
        count_outcome(&counted, Outcome::Loss),
    );
    let (wins, losses) = shown_record(run.course_record, tracked);
    let (classification, live_round) = classify(run, &counted, rounds, (wins, losses));

    let mut results: Vec<Option<Mark>> = counted
        .iter()
        .map(|played| Some(match_mark(played)))
        .collect();
    if run.state == RunState::Active && !run.uncapped {
        while results.len() < rounds {
            results.push(None);
        }
    }

    let matches = counted
        .iter()
        .map(|played| RunMatchRow {
            token: played.token.clone(),
            result: match_mark(played),
            games: game_marks(&played.games),
            started_at: played.started_at,
            duration_seconds: duration_seconds(played.started_at, played.ended_at),
            opponent_archetype: played.opponent_archetype.clone(),
        })
        .collect();

    let spans: Vec<(i64, Option<i64>)> = counted
        .iter()
        .map(|played| (played.started_at, played.ended_at))
        .collect();
    let starts: Vec<i64> = counted.iter().map(|played| played.started_at).collect();
    let games: Vec<&GameResult> = counted.iter().flat_map(|played| &played.games).collect();
    let stats = game_stats(&games);

    let format = display_format(run.format.as_deref().unwrap_or(""));
    let mut top_matchups = matchups(&counted, true);
    top_matchups.truncate(3);

    RunSummary {
        classification,
        live_round,
        results,
        matches,
        avg_match_seconds: avg_match_seconds(&spans),
        time_of_day: time_of_day(&starts, tz),
        game_wins: stats.game_wins,
        game_losses: stats.game_losses,
        on_play: stats.on_play,
        on_draw: stats.on_draw,
        tix_delta: net_tix_for_run(&format, run.state, wins, losses),
        top_opponent_archetype: top_matchups.first().map(|top| top.archetype.clone()),
        top_matchups,
    }
}

/// The opponent archetypes of some matches with the record against each,
/// most faced first, then the better win rate (0.x sorts by the key
/// `count * 1000 + wins / count`); equal ones keep the order first faced.
/// Matches with no archetype are left out. With `draws_lose`, anything but
/// a win is a loss (0.x `FormatLeagueRuns::computeStats`); without it only a
/// loss is (0.x `GetLeagueKpis`).
pub(crate) fn matchups(matches: &[&RunMatch], draws_lose: bool) -> Vec<TopMatchup> {
    let mut found: Vec<TopMatchup> = Vec::new();
    for played in matches {
        let Some(name) = &played.opponent_archetype else {
            continue;
        };
        let index = match found.iter().position(|seen| &seen.archetype == name) {
            Some(index) => index,
            None => {
                found.push(TopMatchup {
                    archetype: name.clone(),
                    wins: 0,
                    losses: 0,
                    count: 0,
                });
                found.len() - 1
            }
        };
        let entry = &mut found[index];
        entry.count += 1;
        match played.outcome {
            Outcome::Win => entry.wins += 1,
            Outcome::Loss => entry.losses += 1,
            _ if draws_lose => entry.losses += 1,
            _ => {}
        }
    }
    let key = |matchup: &TopMatchup| {
        f64::from(matchup.count) * 1000.0 + f64::from(matchup.wins) / f64::from(matchup.count)
    };
    // A stable sort keeps the first-faced order of equal ones.
    found.sort_by(|a, b| key(b).total_cmp(&key(a)));
    found
}

/// The run's complete matches (of `deck_id` when given), in start order.
fn counted_matches(run: &RunInput, deck_id: Option<i64>) -> Vec<&RunMatch> {
    let mut counted: Vec<&RunMatch> = run
        .matches
        .iter()
        .filter(|played| played.state == MatchState::Complete)
        .filter(|played| deck_id.is_none() || played.deck_id == deck_id)
        .collect();
    counted.sort_by_key(|played| played.started_at);
    counted
}

/// 0.x `classifyRun`: EMPTY (manual, nothing counted), LIVE (Active, on the
/// next round), BRICK (Dropped), TROPHY (a win every round), CASH (one
/// loss), else FINISH. An uncapped run is LIVE or FINISH.
/// A run's classification and live round from its shown record (`wins`,
/// `losses`: the tracked ones unless a course record is higher).
fn classify(
    run: &RunInput,
    counted: &[&RunMatch],
    rounds: usize,
    (wins, losses): (u32, u32),
) -> (Classification, Option<u32>) {
    if counted.is_empty() && run.manual {
        return (Classification::Empty, None);
    }
    // Draws are counted matches that are neither: the round count keeps
    // them, as it always did.
    let draws = counted.len() as u32
        - count_outcome(counted, Outcome::Win)
        - count_outcome(counted, Outcome::Loss);
    let played = wins + losses + draws;
    let wins = wins as usize;
    match run.state {
        RunState::Active => (Classification::Live, Some(played + 1)),
        _ if run.uncapped => (Classification::Finish, None),
        RunState::Dropped => (Classification::Brick, None),
        _ if wins >= rounds => (Classification::Trophy, None),
        _ if wins + 1 == rounds => (Classification::Cash, None),
        _ => (Classification::Finish, None),
    }
}

/// A match shows W when won and L otherwise (a draw shows L, as in 0.x).
fn match_mark(played: &RunMatch) -> Mark {
    if played.outcome == Outcome::Win {
        Mark::W
    } else {
        Mark::L
    }
}

fn count_outcome(matches: &[&RunMatch], outcome: Outcome) -> u32 {
    matches
        .iter()
        .filter(|played| played.outcome == outcome)
        .count() as u32
}

/// Games with a result (won or lost), in play order.
pub(crate) fn game_marks(games: &[GameResult]) -> Vec<GameMark> {
    games
        .iter()
        .filter_map(|game| {
            let result = match game.result {
                mtgo_match::GameResult::Win => Mark::W,
                mtgo_match::GameResult::Loss => Mark::L,
                mtgo_match::GameResult::Unknown => return None,
            };
            Some(GameMark {
                result,
                on_play: game.on_play,
            })
        })
        .collect()
}

/// Whole seconds between start and end (0.x `(int) abs(diffInSeconds)`),
/// `None` when the match never ended.
pub(crate) fn duration_seconds(started_at: i64, ended_at: Option<i64>) -> Option<i64> {
    ended_at.map(|ended_at| (ended_at - started_at).abs() / 1_000)
}

/// The rounded average duration of the matches that ended.
pub(crate) fn avg_match_seconds(spans: &[(i64, Option<i64>)]) -> Option<i64> {
    let durations: Vec<i64> = spans
        .iter()
        .filter_map(|(started_at, ended_at)| duration_seconds(*started_at, *ended_at))
        .collect();
    if durations.is_empty() {
        return None;
    }
    let total: i64 = durations.iter().sum();
    let average = total as f64 / durations.len() as f64;
    Some(average.round() as i64)
}

/// The most common local time of day of the starts; a tie goes to the
/// bucket seen first.
fn time_of_day(starts: &[i64], tz: Tz) -> Option<TimeOfDay> {
    // (bucket, count) in first-seen order.
    let mut counts: Vec<(TimeOfDay, u32)> = Vec::new();
    for started_at in starts {
        let bucket = bucket_of(local_hour(*started_at, tz));
        match counts.iter_mut().find(|(seen, _)| *seen == bucket) {
            Some((_, count)) => *count += 1,
            None => counts.push((bucket, 1)),
        }
    }
    let mut best: Option<(TimeOfDay, u32)> = None;
    for (bucket, count) in counts {
        if best.is_none_or(|(_, best_count)| count > best_count) {
            best = Some((bucket, count));
        }
    }
    best.map(|(bucket, _)| bucket)
}

fn bucket_of(hour: u32) -> TimeOfDay {
    match hour {
        6..=11 => TimeOfDay::Morning,
        12..=16 => TimeOfDay::Afternoon,
        17..=21 => TimeOfDay::Evening,
        _ => TimeOfDay::Night,
    }
}

fn local_hour(ms: i64, tz: Tz) -> u32 {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .with_timezone(&tz)
        .hour()
}

/// Game wins and losses and the play/draw record of some games.
pub(crate) struct GameStats {
    pub game_wins: u32,
    pub game_losses: u32,
    pub on_play: WinLoss,
    pub on_draw: WinLoss,
}

pub(crate) fn game_stats(games: &[&GameResult]) -> GameStats {
    let mut stats = GameStats {
        game_wins: 0,
        game_losses: 0,
        on_play: WinLoss::default(),
        on_draw: WinLoss::default(),
    };
    for game in games {
        let won = match game.result {
            mtgo_match::GameResult::Win => true,
            mtgo_match::GameResult::Loss => false,
            mtgo_match::GameResult::Unknown => continue,
        };
        let side = match game.on_play {
            Some(true) => Some(&mut stats.on_play),
            Some(false) => Some(&mut stats.on_draw),
            None => None,
        };
        if won {
            stats.game_wins += 1;
            if let Some(side) = side {
                side.wins += 1;
            }
        } else {
            stats.game_losses += 1;
            if let Some(side) = side {
                side.losses += 1;
            }
        }
    }
    stats
}

/// `vN`: how many of the deck's versions were saved at or before the run's
/// version (0.x `FormatLeagueRuns`).
pub fn version_label(version_modified_at: i64, deck_versions_modified_at: &[i64]) -> String {
    let index = deck_versions_modified_at
        .iter()
        .filter(|modified_at| **modified_at <= version_modified_at)
        .count();
    format!("v{index}")
}

/// The KPI strip over some runs (0.x `GetLeagueKpis`). Wins count every complete match of
/// a run; completed runs are Complete and Dropped (Partial runs only count
/// towards the total); decks are the distinct decks of every linked match.
pub fn league_kpis(runs: &[RunInput]) -> LeagueKpis {
    let mut completed = 0u32;
    let mut dropped = 0u32;
    let mut live = 0u32;
    let mut trophies = 0u32;
    let mut cash = 0u32;
    let mut completed_wins: Vec<u32> = Vec::new();
    let mut decks: Vec<i64> = Vec::new();
    let mut complete: Vec<&RunMatch> = Vec::new();
    for run in runs {
        complete.extend(
            run.matches
                .iter()
                .filter(|played| played.state == MatchState::Complete),
        );
        let wins = complete_wins(run);
        match run.state {
            RunState::Complete | RunState::Dropped => {
                completed += 1;
                if run.state == RunState::Dropped {
                    dropped += 1;
                }
                completed_wins.push(wins);
                if wins as usize >= CONSTRUCTED_ROUNDS {
                    trophies += 1;
                }
                if wins >= 4 {
                    cash += 1;
                }
            }
            RunState::Active => live += 1,
            RunState::Partial => {}
        }
        for played in &run.matches {
            if let Some(deck) = played.deck_id
                && played.deck_version_id.is_some()
                && !decks.contains(&deck)
            {
                decks.push(deck);
            }
        }
    }

    let rate = |count: u32| -> Option<f64> {
        (completed > 0).then(|| php_round(f64::from(count) / f64::from(completed) * 100.0, 0))
    };
    let avg_finish = (completed > 0).then(|| {
        let total: u32 = completed_wins.iter().sum();
        php_round(f64::from(total) / f64::from(completed), 1)
    });

    LeagueKpis {
        total: runs.len() as u32,
        completed,
        live,
        decks: decks.len() as u32,
        trophies,
        trophy_rate: rate(trophies),
        cash_rate: rate(cash),
        dropped,
        drop_rate: rate(dropped),
        avg_finish,
        top_matchup: matchups(&complete, false).into_iter().next(),
    }
}

/// The finish buckets a run's record can land in, best first.
const BUCKETS: [&str; 6] = ["5-0", "4-1", "3-2", "2-3", "1-4", "0-5"];

/// The deck's "League Finishes" (0.x `GetLeagueResultDistribution`): of the
/// Complete and Dropped runs holding one of the deck's complete matches (of
/// `version_id` when given), Dropped runs are counted apart and each
/// Complete run lands in the bucket of the deck's complete wins and losses
/// in it, if the table has one. Partial runs are not drops.
pub fn result_distribution(
    runs: &[RunInput],
    deck_id: i64,
    version_id: Option<i64>,
) -> ResultDistribution {
    let mut buckets: Vec<(String, u32)> = BUCKETS
        .iter()
        .map(|bucket| (bucket.to_string(), 0))
        .collect();
    let mut dropped = 0u32;

    for run in runs {
        if !matches!(run.state, RunState::Complete | RunState::Dropped)
            || !has_complete_deck_match(run, deck_id, version_id)
        {
            continue;
        }
        if run.state == RunState::Dropped {
            dropped += 1;
            continue;
        }
        let deck_matches: Vec<&RunMatch> = run
            .matches
            .iter()
            .filter(|played| {
                played.state == MatchState::Complete && played.deck_id == Some(deck_id)
            })
            .collect();
        let key = format!(
            "{}-{}",
            count_outcome(&deck_matches, Outcome::Win),
            count_outcome(&deck_matches, Outcome::Loss)
        );
        if let Some(bucket) = buckets.iter_mut().find(|(name, _)| *name == key) {
            bucket.1 += 1;
        }
    }

    ResultDistribution { buckets, dropped }
}

/// The deck's sidebar trophies (0.x `GetDeckViewSharedProps`): Complete runs
/// in which every one of the deck's complete matches is a win. There is no
/// five-match check.
pub fn trophy_count(runs: &[RunInput], deck_id: i64) -> u32 {
    let mut trophies = 0;
    for run in runs {
        if run.state != RunState::Complete {
            continue;
        }
        let deck_matches: Vec<&RunMatch> = run
            .matches
            .iter()
            .filter(|played| {
                played.state == MatchState::Complete && played.deck_id == Some(deck_id)
            })
            .collect();
        if !deck_matches.is_empty()
            && deck_matches
                .iter()
                .all(|played| played.outcome == Outcome::Win)
        {
            trophies += 1;
        }
    }
    trophies
}

/// Whether the Leagues page lists the run: manual runs, and runs with at
/// least one complete match (0.x `IndexController::buildQuery`).
pub fn on_leagues_page(run: &RunInput) -> bool {
    run.manual
        || run
            .matches
            .iter()
            .any(|played| played.state == MatchState::Complete)
}

/// The Leagues page state chips (0.x `buildQuery`): `live` (Active),
/// `trophies` (Complete with exactly 5 complete wins), `cash` (Complete with
/// 4 or more), `finish` (Complete with fewer than 4), `bricks` (Dropped).
/// `all` and anything else filter nothing.
pub fn passes_state_filter(filter: &str, run: &RunInput) -> bool {
    let wins = complete_wins(run) as usize;
    let complete = run.state == RunState::Complete;
    match filter {
        "live" => run.state == RunState::Active,
        "trophies" => complete && wins == CONSTRUCTED_ROUNDS,
        "cash" => complete && wins >= CONSTRUCTED_ROUNDS - 1,
        "finish" => complete && wins < CONSTRUCTED_ROUNDS - 1,
        "bricks" => run.state == RunState::Dropped,
        _ => true,
    }
}

/// Wins among the run's complete matches, any deck (0.x `wins_in_run`, the
/// best and worst sorts).
pub fn complete_wins(run: &RunInput) -> u32 {
    run.matches
        .iter()
        .filter(|played| played.state == MatchState::Complete && played.outcome == Outcome::Win)
        .count() as u32
}

/// The deck's run in progress (0.x `GetLeagueInProgress`): the newest
/// Active run holding a match of the deck (of `version_id` when given), in
/// any state.
pub fn in_progress_index(
    runs: &[RunInput],
    deck_id: i64,
    version_id: Option<i64>,
) -> Option<usize> {
    newest_index(runs, |run| {
        run.state == RunState::Active
            && run
                .matches
                .iter()
                .any(|played| is_deck_match(played, deck_id, version_id))
    })
}

/// The deck's latest finished run (0.x `GetLatestLeague`): the newest
/// Complete or Dropped run holding one of the deck's complete matches (of
/// `version_id` when given).
pub fn latest_finished_index<T: Borrow<RunInput>>(
    runs: &[T],
    deck_id: i64,
    version_id: Option<i64>,
) -> Option<usize> {
    newest_index(runs, |run| {
        matches!(run.state, RunState::Complete | RunState::Dropped)
            && has_complete_deck_match(run, deck_id, version_id)
    })
}

/// The run in progress with its finish odds (0.x `GetLeagueInProgress`).
/// `league_record` is the deck's complete matches that belong to a run,
/// `overall_record` all its complete matches (both of the version when the
/// page filters one). Wins and losses are the run's W and L marks (a draw
/// marks L).
pub fn league_in_progress(
    run: &RunInput,
    league_record: &Record,
    overall_record: &Record,
) -> InProgress {
    let counted = counted_matches(run, None);
    let wins = counted
        .iter()
        .filter(|played| match_mark(played) == Mark::W)
        .count() as u32;
    let losses = counted.len() as u32 - wins;
    let (probability, odds_source) = win_probability(league_record, overall_record);
    let format = display_format(run.format.as_deref().unwrap_or(""));

    InProgress {
        wins,
        losses,
        win_probability: (probability * 100.0).round() as u32,
        odds_source,
        odds: finish_odds(
            &format,
            wins,
            losses,
            CONSTRUCTED_ROUNDS as u32,
            probability,
        ),
    }
}

fn is_deck_match(played: &RunMatch, deck_id: i64, version_id: Option<i64>) -> bool {
    played.deck_id == Some(deck_id)
        && (version_id.is_none() || played.deck_version_id == version_id)
}

fn has_complete_deck_match(run: &RunInput, deck_id: i64, version_id: Option<i64>) -> bool {
    run.matches.iter().any(|played| {
        played.state == MatchState::Complete && is_deck_match(played, deck_id, version_id)
    })
}

/// The index of the newest run (by `started_at`) that `wanted` accepts; on a
/// tie the later one in the list.
fn newest_index<T: Borrow<RunInput>>(
    runs: &[T],
    wanted: impl Fn(&RunInput) -> bool,
) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, run) in runs.iter().enumerate() {
        let run: &RunInput = run.borrow();
        if !wanted(run) {
            continue;
        }
        if best.is_none_or(|best| run.started_at >= runs[best].borrow().started_at) {
            best = Some(index);
        }
    }
    best
}
