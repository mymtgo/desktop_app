//! The home dashboard (dashboard redesign spec,
//! `docs/superpowers/specs/2026-10-13-dashboard-redesign-design.md`): one
//! fixed page of cards for a timeframe and a format, read by one command
//! (`home_dashboard`), and Most picked's set switch (`dashboard_picks`).
//!
//! Each card read is isolated: one that fails is logged and its card is
//! hidden, and the page still renders. The league gathers (the constructed
//! runs, and the limited runs in All and Limited modes) run once per call.

use std::collections::HashMap;

use chrono_tz::Tz;
use decks::stats::{DayDeck, DeckMatch, PlayedMatch, Range, Record, daily_decks, daily_results};
use leagues::{LeagueKind, RunState};
use serde::Serialize;
use store::dashboard::{
    self as numbers, FormatFilter, LastSession, RecentDeck, RecentSession, WindowMatch,
};
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready, storage};
use crate::decks::{GameRecordView, RecordView, game_record_view, record_view, timeframe_ranges};
use crate::leagues::{RunCardView, RunStateView, accounts_or_all, state_view};
use crate::limited::{LimitedPicksView, PickSetOptionView};
use crate::logging::Timer;
use crate::platform::{PageScope, page_scope};
use crate::rank::{LadderView, RankLineView, rank_line_view};
use crate::summary::{MatchSummary, from_row};

// ---- Shared card views ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct StreakView {
    /// `3W` or `2L`; `None` without one.
    pub current: Option<String>,
    pub best_win: u32,
    pub best_loss: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlayDrawSplitView {
    pub otp_win_rate: u32,
    pub otd_win_rate: u32,
}

/// One local day of Recent sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentSessionView {
    /// `YYYY-MM-DD` in the local timezone.
    pub day: String,
    /// Whole local days before today; 0 is today. The page words it (Today,
    /// Yesterday, a weekday or a date).
    pub days_ago: u32,
    pub record: RecordView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionMatchView {
    pub token: String,
    /// `win`, `loss`, `draw` or `unknown`.
    pub outcome: String,
    /// The opponent archetype's name, `Unknown` without one.
    pub opponent_archetype: String,
    pub games_won: u32,
    pub games_lost: u32,
}

/// Last session (0.x `GetLastSession`, spec 4.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LastSessionView {
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub ended_at_ms: i64,
    /// `Oct 7, 2026`.
    pub started_label: String,
    /// Oldest first.
    pub matches: Vec<SessionMatchView>,
    /// `2-1`.
    pub record: String,
    /// `~1h 50m` or `~30m`.
    pub duration: String,
}

fn recent_session_view(session: &RecentSession) -> RecentSessionView {
    RecentSessionView {
        day: session.day.clone(),
        days_ago: session.days_ago,
        record: record_view(&session.record),
    }
}

fn last_session_view(session: LastSession) -> LastSessionView {
    let mut matches = Vec::new();
    for played in session.matches {
        matches.push(SessionMatchView {
            token: played.token,
            outcome: played.outcome,
            opponent_archetype: played.opponent_archetype,
            games_won: played.games_won,
            games_lost: played.games_lost,
        });
    }
    LastSessionView {
        started_at_ms: session.started_at_ms,
        ended_at_ms: session.ended_at_ms,
        started_label: session.started_label,
        matches,
        record: session.record,
        duration: session.duration,
    }
}

// ---- Home dashboard (dashboard redesign spec) ------------------------------------

/// One league run as the home dashboard's cards read it, constructed or
/// limited (spec 4.2, 4.6, 4.7): built by `leagues::dashboard_runs` and
/// `limited::dashboard_runs` from their gathers.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardLeagueRun {
    pub key: String,
    pub kind: LeagueKind,
    /// `LeagueKind::round_count`: constructed 5, draft 3, sealed 5.
    pub rounds: u32,
    pub wins: u32,
    pub losses: u32,
    /// The fold's state: the league card's "run in progress" rule reads it
    /// (league self-management spec 4.3).
    pub state: RunState,
    /// What the run's card shows: the player's status when set, else the
    /// fold's state. The League results bars read it.
    pub shown_state: RunState,
    pub name: String,
    /// The deck's name; for a limited run the set's name and colours.
    pub subject: String,
    pub started_at: i64,
    /// The latest match start, else the run's start.
    pub last_played_at: i64,
    /// The format code (`CMODERN`): the format switch filters by it.
    pub format: Option<String>,
    pub deck_id: Option<i64>,
    pub set_code: Option<String>,
    pub set_name: Option<String>,
    /// WUBRG letters: the deck's identity, or a limited run's deck colours;
    /// `None` when unknown.
    pub colors: Option<Vec<String>>,
    /// A limited run's colour combination name (`Dimir`).
    pub colour_name: Option<String>,
    pub cover_art: Option<String>,
    pub local_cover_art: Option<String>,
    pub game_wins: u32,
    pub game_losses: u32,
    /// A draft's first pick by name; `Sealed` for a sealed run; `None` for a
    /// constructed run or one the dashboard shows without art.
    pub first_pick: Option<String>,
    /// The tokens of the run's matches.
    pub match_tokens: Vec<String>,
}

/// Recent matches, Top matchups and Your colours show at most this many rows.
pub const TOP_ROWS: usize = 5;
/// Recent decks or drafts show this many (spec 4.7).
pub const RECENT_ENTRIES: usize = 2;
/// The Sets row of matches no set was found for.
const OTHER_SET: &str = "Other";

/// One option of the format switch (after Constructed and Limited).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FormatSwitchOptionView {
    /// `modern`: the switch's value and `?format=`.
    pub key: String,
    pub label: String,
}

/// What one of the player's decks did on a chart day (the Results chart's
/// tooltip): grouped by the deck's archetype, else by the deck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DayDeckView {
    /// Names the group (`archetype:<id>` or `deck:<id>`), so a chart that
    /// buckets several days can sum the same group.
    pub key: String,
    pub name: String,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
}

/// One day of the home Results chart: the deck dashboard's `DayView` fields
/// and the decks played that day, most matches first. The deck dashboard
/// keeps `DayView`, since its chart is one deck and has nothing to list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeDayView {
    pub day: String,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    /// `None` on a day without matches.
    pub win_rate: Option<u32>,
    /// The win rate trend after the day's last match, kept on days without
    /// one.
    pub trend: Option<u32>,
    pub decks: Vec<DayDeckView>,
}

fn day_deck_view(deck: &DayDeck) -> DayDeckView {
    DayDeckView {
        key: deck.key.clone(),
        name: deck.name.clone(),
        wins: deck.wins,
        losses: deck.losses,
        draws: deck.draws,
    }
}

/// Results (spec 4.1): the Overview numbers, the daily chart and Recent
/// sessions.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ResultsView {
    /// Complete matches in the window (with games or not); 0 shows "No
    /// matches in this timeframe".
    pub matches: u32,
    pub record: RecordView,
    pub games: GameRecordView,
    pub streak: StreakView,
    /// Against the previous window; 0 for all time.
    pub match_win_rate_delta: i32,
    pub game_win_rate_delta: i32,
    pub play_draw: PlayDrawSplitView,
    /// The chart's days: the deck dashboard's, each with the decks played.
    pub daily: Vec<HomeDayView>,
    /// The last 7 local days with a match, newest first: follows the
    /// format, not the timeframe.
    pub recent_sessions: Vec<RecentSessionView>,
}

/// How a League results bar is drawn (spec 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RunBarToneView {
    Trophy,
    Winning,
    Losing,
    Dropped,
    Partial,
    Active,
}

/// A league's kind (`leagues::LeagueKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum LeagueKindView {
    Constructed,
    Draft,
    Sealed,
}

impl From<LeagueKind> for LeagueKindView {
    fn from(kind: LeagueKind) -> Self {
        match kind {
            LeagueKind::Constructed => LeagueKindView::Constructed,
            LeagueKind::Draft => LeagueKindView::Draft,
            LeagueKind::Sealed => LeagueKindView::Sealed,
        }
    }
}

/// One League results bar.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueRunBarView {
    pub key: String,
    pub kind: LeagueKindView,
    pub rounds: u32,
    pub wins: u32,
    pub losses: u32,
    pub state: RunStateView,
    pub name: String,
    pub subject: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    /// Share of the bar area, 0 to 1.
    pub height: f64,
    pub tone: RunBarToneView,
    /// A constructed run's deck (the bar opens its Leagues tab).
    #[specta(type = Option<specta_typescript::Number>)]
    pub deck_id: Option<i64>,
}

/// League results (spec 4.2).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueRunsView {
    /// Runs started in the window, oldest first.
    pub runs: Vec<LeagueRunBarView>,
    pub trophies: u32,
    /// The average wins of the Complete runs, one decimal.
    pub avg_wins: Option<f64>,
    /// The axis's win count: outside All mode, when every run has the same
    /// rounds.
    pub axis_rounds: Option<u32>,
}

/// One Top matchups row (spec 4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TopMatchupRowView {
    #[specta(type = specta_typescript::Number)]
    pub archetype_id: i64,
    pub name: String,
    /// Matches against it, in percent of the window's matches with an
    /// opponent archetype.
    pub share: u32,
    pub record: RecordView,
}

/// One Your colours row (spec 4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ColourShareView {
    /// WUBRG letters (`["U", "B"]`); empty for colourless.
    pub colors: Vec<String>,
    /// `Dimir`.
    pub name: String,
    pub runs: u32,
    /// Percent of the window's runs with known colours.
    pub share: u32,
    pub record: RecordView,
}

/// One Recent matches row (spec 4.5): the match list's row, with its deck
/// or its limited run.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeRecentMatchView {
    #[serde(rename = "match")]
    pub summary: MatchSummary,
    #[specta(type = Option<specta_typescript::Number>)]
    pub deck_id: Option<i64>,
    pub deck_name: Option<String>,
    /// A limited match's run, its set and its colours.
    pub run_key: Option<String>,
    pub set_code: Option<String>,
    pub colors: Option<Vec<String>>,
}

/// One Recent decks entry (spec 4.7): numbers all time.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentDeckView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub format_label: String,
    pub colors: Vec<String>,
    pub cover_art: Option<String>,
    pub local_cover_art: Option<String>,
    /// The deck's own cover image, a file name in the deck covers folder.
    pub custom_cover_art: Option<String>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_played_at_ms: Option<i64>,
    pub record: RecordView,
    pub games: GameRecordView,
    /// Win rates, 0 to 100, oldest first.
    pub sparkline: Vec<f64>,
}

/// One Recent drafts entry (spec 4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentDraftView {
    pub key: String,
    /// `draft` or `sealed`.
    pub kind: String,
    pub set_code: Option<String>,
    pub set_name: Option<String>,
    pub colors: Option<Vec<String>>,
    pub colour_name: Option<String>,
    pub cover_art: Option<String>,
    pub local_cover_art: Option<String>,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    pub wins: u32,
    pub losses: u32,
    pub game_wins: u32,
    pub game_losses: u32,
    /// The first pick's name, `Sealed` for a sealed run.
    pub first_pick: Option<String>,
}

/// A Recent card entry: a deck or a limited run.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RecentEntryView {
    Deck(RecentDeckView),
    Draft(RecentDraftView),
}

/// Most picked (spec 4.8): the newest set's picks and the sets to switch to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MostPickedView {
    /// Newest first; the UI adds All.
    pub sets: Vec<PickSetOptionView>,
    pub picks: LimitedPicksView,
}

/// One Formats or Sets row (spec 4.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRowView {
    pub key: String,
    pub label: String,
    pub matches: u32,
    /// Percent of the card's matches.
    pub share: u32,
    pub record: RecordView,
}

/// Formats or Sets (spec 4.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownView {
    /// `formats` or `sets`.
    pub kind: String,
    pub total: u32,
    pub rows: Vec<BreakdownRowView>,
}

/// The home dashboard (spec 5.1): one `Option` per card, `None` hidden.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeDashboardView {
    /// The account has a complete match.
    pub has_matches: bool,
    /// The format the cards follow: `constructed`, `limited` or a format key
    /// (an unknown or empty key reads as the format last played; `all` is
    /// still understood but nothing asks for it).
    pub format: String,
    /// The constructed formats played, after Constructed and Limited.
    pub formats: Vec<FormatSwitchOptionView>,
    pub results: Option<ResultsView>,
    pub league_results: Option<LeagueRunsView>,
    pub matchups: Option<Vec<TopMatchupRowView>>,
    pub your_colours: Option<Vec<ColourShareView>>,
    pub recent_matches: Option<Vec<HomeRecentMatchView>>,
    /// League in progress (spec 4.6): the Leagues page's run card, for a
    /// limited run too.
    pub league: Option<RunCardView>,
    pub recent: Option<Vec<RecentEntryView>>,
    pub most_picked: Option<MostPickedView>,
    pub last_session: Option<LastSessionView>,
    pub breakdown: Option<BreakdownView>,
    /// Arena rank (Arena spec 15): the constructed line, then the limited
    /// one, always both on Arena (an empty line when a ladder has no reading
    /// this season). `None` on MTGO.
    pub rank: Option<Vec<RankLineView>>,
}

/// One card's read, isolated (spec 5.1): a read that fails is logged and
/// its card is hidden; the page still renders.
fn isolated<T>(card: &str, read: impl FnOnce() -> Result<Option<T>, StoreError>) -> Option<T> {
    match read() {
        Ok(found) => found,
        Err(error) => {
            tracing::error!(%error, card, "a home dashboard card failed");
            None
        }
    }
}

/// `part` of `whole` in whole percent, halves up; 0 of nothing.
fn percent(part: u32, whole: u32) -> u32 {
    if whole == 0 {
        return 0;
    }
    ((200 * u64::from(part) + u64::from(whole)) / (2 * u64::from(whole))) as u32
}

fn in_window(at_ms: i64, range: Range) -> bool {
    at_ms >= range.from_ms && at_ms <= range.to_ms
}

/// The runs the format takes, newest first.
fn mode_runs(runs: Vec<DashboardLeagueRun>, filter: &FormatFilter) -> Vec<DashboardLeagueRun> {
    let mut kept = Vec::new();
    for run in runs {
        let constructed = run.kind == LeagueKind::Constructed;
        let wanted = match filter {
            FormatFilter::All => true,
            FormatFilter::Constructed => constructed,
            FormatFilter::Limited => !constructed,
            FormatFilter::Codes(codes) => {
                constructed && run.format.as_ref().is_some_and(|code| codes.contains(code))
            }
        };
        if wanted {
            kept.push(run);
        }
    }
    kept.sort_by_key(|run| std::cmp::Reverse(run.started_at));
    kept
}

/// Results (spec 4.1).
#[allow(clippy::too_many_arguments)]
fn results_view(
    conn: &Connection,
    scope: PageScope,
    range: Range,
    previous: Option<Range>,
    filter: &FormatFilter,
    window: &[WindowMatch],
    now_ms: i64,
    tz: Tz,
) -> Result<ResultsView, StoreError> {
    let overview = numbers::overview(conn, scope.account, scope.platform, range, previous, filter)?;
    let earlier = numbers::earlier_matches(conn, scope.account, scope.platform, range, filter)?;
    let sessions =
        numbers::recent_sessions(conn, scope.account, scope.platform, filter, now_ms, tz)?;
    let mut played = Vec::new();
    for found in window {
        played.push(PlayedMatch {
            started_at_ms: found.started_at,
            outcome: found.outcome,
        });
    }
    let mut with_decks = Vec::new();
    for found in window {
        with_decks.push(DeckMatch {
            started_at_ms: found.started_at,
            outcome: found.outcome,
            deck: found.deck.clone(),
        });
    }
    let mut decks_by_day = daily_decks(&with_decks, tz);
    let mut daily = Vec::new();
    for day in daily_results(&earlier, &played, tz) {
        let decks = decks_by_day.remove(&day.day).unwrap_or_default();
        daily.push(HomeDayView {
            decks: decks.iter().map(day_deck_view).collect(),
            day: day.day,
            wins: day.wins,
            losses: day.losses,
            draws: day.draws,
            win_rate: day.win_rate,
            trend: day.trend,
        });
    }
    Ok(ResultsView {
        matches: window.len() as u32,
        record: record_view(&overview.record),
        games: game_record_view(&overview.games),
        streak: StreakView {
            current: overview.streak.current.clone(),
            best_win: overview.streak.best_win,
            best_loss: overview.streak.best_loss,
        },
        match_win_rate_delta: overview.match_win_rate_delta,
        game_win_rate_delta: overview.game_win_rate_delta,
        play_draw: PlayDrawSplitView {
            otp_win_rate: overview.play_draw.otp_win_rate,
            otd_win_rate: overview.play_draw.otd_win_rate,
        },
        daily,
        recent_sessions: sessions.iter().map(recent_session_view).collect(),
    })
}

fn tone_view(tone: leagues::BarTone) -> RunBarToneView {
    match tone {
        leagues::BarTone::Trophy => RunBarToneView::Trophy,
        leagues::BarTone::Winning => RunBarToneView::Winning,
        leagues::BarTone::Losing => RunBarToneView::Losing,
        leagues::BarTone::Dropped => RunBarToneView::Dropped,
        leagues::BarTone::Partial => RunBarToneView::Partial,
        leagues::BarTone::Active => RunBarToneView::Active,
    }
}

/// League results (spec 4.2): `None` without a run started in the window.
fn league_runs_view(
    runs: &[DashboardLeagueRun],
    range: Range,
    filter: &FormatFilter,
) -> Option<LeagueRunsView> {
    let mut shown: Vec<&DashboardLeagueRun> = runs
        .iter()
        .filter(|run| in_window(run.started_at, range))
        .collect();
    if shown.is_empty() {
        return None;
    }
    shown.sort_by_key(|run| run.started_at);
    let mut bars = Vec::new();
    let mut inputs = Vec::new();
    for run in &shown {
        let bar = leagues::run_bar(run.shown_state, run.rounds, run.wins, run.losses);
        inputs.push(leagues::BarRun {
            state: run.shown_state,
            rounds: run.rounds,
            wins: run.wins,
        });
        bars.push(LeagueRunBarView {
            key: run.key.clone(),
            kind: run.kind.into(),
            rounds: run.rounds,
            wins: run.wins,
            losses: run.losses,
            state: state_view(run.shown_state),
            name: run.name.clone(),
            subject: run.subject.clone(),
            started_at_ms: run.started_at,
            height: bar.height,
            tone: tone_view(bar.tone),
            deck_id: run.deck_id,
        });
    }
    let summary = leagues::runs_summary(&inputs);
    let first_rounds = shown[0].rounds;
    let same_rounds = shown.iter().all(|run| run.rounds == first_rounds);
    Some(LeagueRunsView {
        runs: bars,
        trophies: summary.trophies,
        avg_wins: summary.avg_wins,
        axis_rounds: (*filter != FormatFilter::All && same_rounds).then_some(first_rounds),
    })
}

/// Top matchups (spec 4.3): `None` without archetype data.
fn matchups_view(
    conn: &Connection,
    account: Option<i64>,
    range: Range,
    filter: &FormatFilter,
) -> Result<Option<Vec<TopMatchupRowView>>, StoreError> {
    let spread = numbers::matchup_spread(conn, account, range, filter)?;
    if spread.rows.is_empty() {
        return Ok(None);
    }
    let total = spread.total;
    let mut views = Vec::new();
    for row in &spread.rows {
        views.push(TopMatchupRowView {
            archetype_id: row.archetype_id,
            name: row.name.clone(),
            share: percent(row.record.total, total),
            record: record_view(&row.record),
        });
    }
    Ok(Some(views))
}

/// Your colours (spec 4.4): the window's limited runs with known colours,
/// by colour combination, most played first, five of them.
fn your_colours_view(runs: &[DashboardLeagueRun], range: Range) -> Option<Vec<ColourShareView>> {
    // (colours, runs, wins, losses), in first-seen order.
    let mut groups: Vec<(Vec<String>, u32, u32, u32)> = Vec::new();
    let mut known = 0u32;
    for run in runs {
        if run.kind == LeagueKind::Constructed || !in_window(run.started_at, range) {
            continue;
        }
        let Some(colors) = &run.colors else {
            continue;
        };
        known += 1;
        match groups.iter_mut().find(|group| group.0 == *colors) {
            Some(group) => {
                group.1 += 1;
                group.2 += run.wins;
                group.3 += run.losses;
            }
            None => groups.push((colors.clone(), 1, run.wins, run.losses)),
        }
    }
    if groups.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for (colors, count, wins, losses) in groups {
        rows.push(ColourShareView {
            name: limited::colour_combination_name(&colors.join(",")),
            colors,
            runs: count,
            share: percent(count, known),
            record: record_view(&Record::from_counts(wins, losses, 0)),
        });
    }
    rows.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.name.cmp(&b.name)));
    rows.truncate(TOP_ROWS);
    Some(rows)
}

/// Recent matches (spec 4.5): a limited match carries its run's set and
/// colours.
fn recent_matches_view(
    conn: &Connection,
    scope: PageScope,
    range: Range,
    filter: &FormatFilter,
    runs: &[DashboardLeagueRun],
) -> Result<Option<Vec<HomeRecentMatchView>>, StoreError> {
    let mut views = Vec::new();
    for recent in numbers::recent_matches(conn, scope.account, scope.platform, range, filter)? {
        let Some(summary) = from_row(&recent.row) else {
            tracing::warn!(token = %recent.row.token, state = %recent.row.state, "skipped a match row the list cannot show");
            continue;
        };
        let run = runs.iter().find(|run| {
            run.kind != LeagueKind::Constructed && run.match_tokens.contains(&summary.token)
        });
        views.push(HomeRecentMatchView {
            deck_id: recent.deck_id,
            deck_name: recent.deck_name,
            run_key: run.map(|run| run.key.clone()),
            set_code: run.and_then(|run| run.set_code.clone()),
            colors: run.and_then(|run| run.colors.clone()),
            summary,
        });
    }
    Ok((!views.is_empty()).then_some(views))
}

/// The run League in progress shows (spec 4.6): the newest Active run, else
/// the newest other one. `runs` is newest first.
fn league_card_run(runs: &[DashboardLeagueRun]) -> Option<&DashboardLeagueRun> {
    runs.iter()
        .find(|run| run.state == RunState::Active)
        .or_else(|| runs.first())
}

/// League in progress (spec 4.6) for the chosen run, from the gathers
/// already read: the Leagues page's run card for either kind. `None` when
/// its gather was not read.
fn league_view(
    conn: &Connection,
    run: &DashboardLeagueRun,
    constructed: Option<&crate::leagues::Gathered>,
    limited: Option<&crate::limited::Gathered>,
    tz: Tz,
) -> Result<Option<RunCardView>, StoreError> {
    if run.kind == LeagueKind::Constructed {
        let Some(data) = constructed else {
            return Ok(None);
        };
        let Some(found) = data.runs.iter().find(|found| found.key == run.key) else {
            return Ok(None);
        };
        return Ok(Some(crate::leagues::card(found, None, data, tz)));
    }
    let Some(data) = limited else {
        return Ok(None);
    };
    crate::limited::run_card(conn, data, &run.key, tz)
}

fn recent_deck_view(recent: &RecentDeck) -> RecentDeckView {
    RecentDeckView {
        id: recent.deck.id,
        name: recent.deck.name.clone(),
        format_label: recent.deck.format_label.clone(),
        colors: crate::decks::color_list(recent.deck.color_identity.as_deref()),
        cover_art: recent.deck.cover_art.clone(),
        local_cover_art: recent.deck.local_cover_art.clone(),
        custom_cover_art: recent.deck.cover_file.clone(),
        last_played_at_ms: recent.deck.last_played_at,
        record: record_view(&recent.deck.record),
        games: game_record_view(&recent.games),
        sparkline: recent.sparkline.clone(),
    }
}

fn recent_draft_view(run: &DashboardLeagueRun) -> RecentDraftView {
    RecentDraftView {
        key: run.key.clone(),
        kind: run.kind.as_str().to_string(),
        set_code: run.set_code.clone(),
        set_name: run.set_name.clone(),
        colors: run.colors.clone(),
        colour_name: run.colour_name.clone(),
        cover_art: run.cover_art.clone(),
        local_cover_art: run.local_cover_art.clone(),
        started_at_ms: run.started_at,
        wins: run.wins,
        losses: run.losses,
        game_wins: run.game_wins,
        game_losses: run.game_losses,
        first_pick: run.first_pick.clone(),
    }
}

/// Recent decks, Recent drafts, or both in All mode (spec 4.7): two
/// entries, whatever the timeframe, newest played first.
fn recent_view(
    conn: &Connection,
    scope: PageScope,
    filter: &FormatFilter,
    runs: &[DashboardLeagueRun],
) -> Result<Option<Vec<RecentEntryView>>, StoreError> {
    // (last played, entry)
    let mut entries: Vec<(i64, RecentEntryView)> = Vec::new();
    if *filter != FormatFilter::Limited {
        for recent in numbers::recent_decks(conn, scope.account, scope.platform, filter)? {
            let at = recent.deck.last_played_at.unwrap_or(i64::MIN);
            entries.push((at, RecentEntryView::Deck(recent_deck_view(&recent))));
        }
    }
    if matches!(filter, FormatFilter::All | FormatFilter::Limited) {
        let mut drafts: Vec<&DashboardLeagueRun> = runs
            .iter()
            .filter(|run| run.kind != LeagueKind::Constructed)
            .collect();
        drafts.sort_by_key(|run| std::cmp::Reverse(run.started_at));
        for run in drafts.into_iter().take(RECENT_ENTRIES) {
            entries.push((
                run.last_played_at,
                RecentEntryView::Draft(recent_draft_view(run)),
            ));
        }
    }
    entries.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    entries.truncate(RECENT_ENTRIES);
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(entries.into_iter().map(|(_, entry)| entry).collect()))
}

/// Most picked (spec 4.8): `None` without a drafted set.
fn most_picked_view(
    conn: &Connection,
    data: &crate::limited::Gathered,
) -> Result<Option<MostPickedView>, StoreError> {
    let sets = crate::limited::picked_sets(conn, data)?;
    let Some(newest) = sets.first() else {
        return Ok(None);
    };
    let picks = crate::limited::most_picked(conn, data, Some(&newest.code))?;
    Ok(Some(MostPickedView { sets, picks }))
}

/// Formats, or Sets in Limited mode (spec 4.10): hidden with fewer than two
/// rows.
fn breakdown_view(
    conn: &Connection,
    filter: &FormatFilter,
    window: &[WindowMatch],
    runs: &[DashboardLeagueRun],
) -> Result<Option<BreakdownView>, StoreError> {
    let limited_mode = *filter == FormatFilter::Limited;
    let mut rows = Vec::new();
    if limited_mode {
        let mut run_sets: HashMap<String, String> = HashMap::new();
        for run in runs {
            if let Some(code) = &run.set_code {
                for token in &run.match_tokens {
                    run_sets.insert(token.clone(), code.clone());
                }
            }
        }
        let mut matches = Vec::new();
        for played in window {
            matches.push(limited::SetMatch {
                token: played.token.clone(),
                format: played.format.clone(),
                outcome: played.outcome,
            });
        }
        for set in limited::matches_by_set(&matches, &run_sets) {
            let label = match &set.set_code {
                Some(code) => match runs
                    .iter()
                    .find(|run| run.set_code.as_ref() == Some(code) && run.set_name.is_some())
                {
                    Some(run) => run.set_name.clone().unwrap_or_default(),
                    None => store::limited::set_name(conn, code)?.unwrap_or_else(|| code.clone()),
                },
                None => OTHER_SET.to_string(),
            };
            rows.push(BreakdownRowView {
                key: set.set_code.clone().unwrap_or_else(|| "other".to_string()),
                label,
                matches: set.matches,
                share: 0,
                record: record_view(&Record::from_total(set.wins, set.losses, set.matches)),
            });
        }
    } else {
        let mut matches = Vec::new();
        for played in window {
            matches.push(decks::FormatMatch {
                format: played.format.clone(),
                outcome: played.outcome,
            });
        }
        for share in decks::format_breakdown(&matches) {
            rows.push(BreakdownRowView {
                key: share.label.to_lowercase(),
                label: share.label.clone(),
                matches: share.record.total,
                share: 0,
                record: record_view(&share.record),
            });
        }
    }
    if rows.len() < 2 {
        return Ok(None);
    }
    let total: u32 = rows.iter().map(|row| row.matches).sum();
    for row in &mut rows {
        row.share = percent(row.matches, total);
    }
    let kind = if limited_mode { "sets" } else { "formats" };
    Ok(Some(BreakdownView {
        kind: kind.to_string(),
        total,
        rows,
    }))
}

/// The Arena rank cards: the constructed line, then the limited one. A
/// ladder with no reading this season is an empty line (no points, no
/// current rank), so its card still shows.
fn rank_lines(conn: &Connection, account: i64) -> Result<Option<Vec<RankLineView>>, StoreError> {
    let mut lines = Vec::new();
    for ladder in [LadderView::Constructed, LadderView::Limited] {
        let line = match rank_line_view(conn, account, ladder, None)? {
            Some(line) => line,
            None => empty_rank_line(ladder),
        };
        lines.push(line);
    }
    Ok(Some(lines))
}

/// A ladder's line with no reading this season.
fn empty_rank_line(ladder: LadderView) -> RankLineView {
    RankLineView {
        ladder,
        season: None,
        season_start_ms: None,
        season_end_ms: None,
        current: None,
        points: Vec::new(),
        gaps: Vec::new(),
        matches: Vec::new(),
    }
}

/// The home dashboard for `timeframe` (the deck pages' keys) and `format`
/// (`constructed`, `limited`, a format key, or empty for the format last
/// played; `all` is still understood), spec section 5.1.
/// Page-level reads (the account, `has_matches`, the format options) fail
/// the command; each card read is isolated.
pub fn home_dashboard_view(
    conn: &Connection,
    timeframe: &str,
    format: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<HomeDashboardView, CommandError> {
    let scope = page_scope(conn).map_err(storage)?;
    let account = scope.account;
    let on_mtgo = scope.platform == store::Platform::Mtgo;
    let has_matches = numbers::has_matches(conn, account, scope.platform).map_err(storage)?;
    let options = numbers::format_options(conn, account, scope.platform).map_err(storage)?;
    // An empty or unknown key is the format last played (a constructed
    // format, `limited`, or `constructed`: always one the switch offers).
    let (format, filter) = match numbers::resolve_format(format, &options) {
        Some(found) => found,
        None => {
            let last =
                numbers::last_played_format(conn, account, scope.platform).map_err(storage)?;
            numbers::resolve_format(&last, &options).unwrap_or((
                "constructed".to_string(),
                numbers::FormatFilter::Constructed,
            ))
        }
    };
    let (range, previous) = timeframe_ranges(timeframe, now_ms, tz);
    let limited_mode = filter == FormatFilter::Limited;
    let reads_limited = matches!(filter, FormatFilter::All | FormatFilter::Limited);

    // The gathers run once per call (spec 5.1); the limited one only in All
    // and Limited modes.
    let constructed_data = if limited_mode || !on_mtgo {
        None
    } else {
        isolated("constructed league runs", || {
            Ok(Some(crate::leagues::gather(
                conn,
                &accounts_or_all(conn, store::Platform::Mtgo, account)?,
            )?))
        })
    };
    let constructed = match &constructed_data {
        Some(data) => Some(crate::leagues::dashboard_runs(data, tz)),
        None if limited_mode || !on_mtgo => Some(Vec::new()),
        None => None,
    };
    let limited_data = if reads_limited && on_mtgo {
        isolated("limited runs", || Ok(Some(crate::limited::gather(conn)?)))
    } else {
        None
    };
    let limited = match &limited_data {
        Some(data) => isolated("limited league runs", || {
            Ok(Some(crate::limited::dashboard_runs(conn, data, tz)?))
        }),
        None if reads_limited && on_mtgo => None,
        None => Some(Vec::new()),
    };
    let runs = match (constructed, limited) {
        (Some(mut every), Some(more)) => {
            every.extend(more);
            Some(mode_runs(every, &filter))
        }
        _ => None,
    };
    let known_runs: &[DashboardLeagueRun] = runs.as_deref().unwrap_or(&[]);

    // Window cards hide without a match in the window; Results stays (spec 3).
    let window = isolated("window matches", || {
        Ok(Some(numbers::window_matches(
            conn,
            account,
            scope.platform,
            range,
            &filter,
        )?))
    });
    let window_open = window.as_ref().is_some_and(|found| !found.is_empty());
    let results = match &window {
        Some(found) => isolated("results", || {
            Ok(Some(results_view(
                conn, scope, range, previous, &filter, found, now_ms, tz,
            )?))
        }),
        None => None,
    };
    let league_results = match &runs {
        Some(runs) if window_open && on_mtgo => league_runs_view(runs, range, &filter),
        _ => None,
    };
    let matchups = if window_open && !limited_mode {
        isolated("top matchups", || {
            matchups_view(conn, account, range, &filter)
        })
    } else {
        None
    };
    let your_colours = match &runs {
        Some(runs) if window_open && limited_mode => your_colours_view(runs, range),
        _ => None,
    };
    let recent_matches = if window_open {
        isolated("recent matches", || {
            recent_matches_view(conn, scope, range, &filter, known_runs)
        })
    } else {
        None
    };
    let breakdown = match &window {
        Some(found) if window_open => isolated("formats", || {
            breakdown_view(conn, &filter, found, known_runs)
        }),
        _ => None,
    };

    // Standing cards ignore the timeframe (spec 3).
    let league = match runs.as_deref().and_then(league_card_run) {
        Some(run) => isolated("league in progress", || {
            league_view(
                conn,
                run,
                constructed_data.as_ref(),
                limited_data.as_ref(),
                tz,
            )
        }),
        None => None,
    };
    let recent = isolated("recent", || recent_view(conn, scope, &filter, known_runs));
    let most_picked = match &limited_data {
        Some(data) if limited_mode => isolated("most picked", || most_picked_view(conn, data)),
        _ => None,
    };
    let last_session = isolated("last session", || {
        Ok(numbers::last_session(conn, account, &filter, tz)?.map(last_session_view))
    });

    // Arena rank (Arena spec 15) is a standing card too.
    let rank = match (scope.platform, account) {
        (store::Platform::Arena, Some(account)) => isolated("rank", || rank_lines(conn, account)),
        _ => None,
    };

    let mut formats = Vec::new();
    for option in options {
        formats.push(FormatSwitchOptionView {
            key: option.key,
            label: option.label,
        });
    }
    Ok(HomeDashboardView {
        has_matches,
        format,
        formats,
        results,
        league_results,
        matchups,
        your_colours,
        recent_matches,
        league,
        recent,
        most_picked,
        last_session,
        breakdown,
        rank,
    })
}

/// The home dashboard (spec 5.1). Async on the worker pool: it may read
/// every limited run, which must not block the window.
#[tauri::command]
#[specta::specta]
pub async fn home_dashboard(
    app: AppHandle,
    timeframe: String,
    format: String,
) -> Result<HomeDashboardView, CommandError> {
    let _timer = Timer::start("home_dashboard");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        home_dashboard_view(
            &lock(&state.reader),
            &timeframe,
            &format,
            crate::wall_clock_ms(),
            state.timezone,
        )
    })
    .await
}

/// Most picked's set switch (spec 4.8): `set_code` `None` for every set.
#[tauri::command]
#[specta::specta]
pub async fn dashboard_picks(
    app: AppHandle,
    set_code: Option<String>,
) -> Result<LimitedPicksView, CommandError> {
    let _timer = Timer::start("dashboard_picks");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let conn = lock(&state.reader);
        let data = crate::limited::gather(&conn).map_err(storage)?;
        crate::limited::most_picked(&conn, &data, set_code.as_deref()).map_err(storage)
    })
    .await
}

#[cfg(test)]
mod home_tests;
