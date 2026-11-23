//! League commands (leagues spec sections 5 to 8): the Leagues page, the deck
//! pages' Leagues and Tournaments tabs and dashboard panels, and the league
//! edits, which go through the tracker (the only writer).
//!
//! Runs are the stored constructed runs (`store::leagues::stored_runs`): runs
//! derived from the log, runs imported from 0.x (which the league sync starts
//! as 0.x left them and continues with live matches), and the manual
//! leagues, each with its listed matches. Filtering, sorting and paging
//! happen in memory (0.x `Leagues\IndexController`), and each card is 0.x
//! `FormatLeagueRuns` through the `leagues` crate's numbers.
//!
//! Whose runs: the Leagues page takes the current account's
//! (`accounts::current_account_id`; every account's when there is none),
//! a deck page its deck's account's (every account's for a deck with none).
//! Manual leagues and imported runs with no account (0.x leagues the import
//! could not match to one) are listed under every account.
//!
//! Ids from the UI are `u32` (the TypeScript export refuses `i64`
//! arguments); run keys are strings.

use std::collections::{HashMap, HashSet};

use chrono::{NaiveDate, NaiveDateTime};
use chrono_tz::Tz;
use decks::display_format;
use decks::stats::{Range, Record};
use leagues::{
    Classification, DeckVersionDate, FinishOdds, GameMark, InProgress, LeagueKind, Mark,
    OddsSource, RunInput, RunState, RunSummary, TimeOfDay, TournamentInput, TournamentRun, WinLoss,
    complete_wins, in_progress_index, latest_finished_index, league_in_progress, league_kpis,
    on_leagues_page, passes_state_filter, result_distribution, summarize_run, tournament_run,
    trophy_count, version_label,
};
use mtgo_match::{GameResult, MatchState, Outcome};
use serde::{Deserialize, Serialize};
use store::league_sync::LeagueOwner;
use store::leagues::{
    DeleteMatches, LeagueDeck, LeagueEdit, LeagueEditOutcome, LeagueMatchRow, LeagueRef,
    LeagueStatus, MoveTarget, RunFilter, StoredRun, UserState, VersionDate, account_ids,
    available_matches, league_decks, league_status, match_leagues, move_targets, notes_by_match,
    stored_runs, stored_tournaments, version_dates,
};
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager, State};

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::dashboard::DashboardLeagueRun;
use crate::decks::{color_list, not_blank, timeframe_ranges};
use crate::logging::Timer;

/// Runs per Leagues page (0.x `paginate(20)`).
pub const LEAGUES_PER_PAGE: u32 = 20;

// ---- Types the UI sends ------------------------------------------------------

/// The Leagues page sort (0.x `sort`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LeagueSort {
    /// Newest start first.
    #[default]
    Newest,
    Oldest,
    /// Most complete wins first, then newest.
    Best,
    /// Fewest complete wins first, then newest.
    Worst,
}

/// The Leagues page state chips (0.x `state`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LeagueStateChip {
    #[default]
    All,
    /// Active runs.
    Live,
    /// Complete runs with exactly 5 complete wins.
    Trophies,
    /// Complete runs with 4 or more (the "4-1+" chip).
    Cash,
    /// Complete runs with fewer than 4.
    Finish,
    /// Dropped runs.
    Bricks,
}

impl LeagueStateChip {
    /// The filter name `leagues::passes_state_filter` takes.
    pub fn as_str(self) -> &'static str {
        match self {
            LeagueStateChip::All => "all",
            LeagueStateChip::Live => "live",
            LeagueStateChip::Trophies => "trophies",
            LeagueStateChip::Cash => "cash",
            LeagueStateChip::Finish => "finish",
            LeagueStateChip::Bricks => "bricks",
        }
    }
}

/// What the Leagues page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueListQuery {
    /// A format code (`CMODERN`); `None` or blank for every format.
    pub format: Option<String>,
    pub state: LeagueStateChip,
    /// Only runs of this deck.
    pub deck_id: Option<u32>,
    /// Only runs whose deck has this archetype (0.x `archetype`).
    #[specta(optional)]
    #[serde(default)]
    pub archetype_id: Option<u32>,
    /// Part of an opponent's name or of an opponent archetype's name, any
    /// case; `None` or blank for none.
    pub search: Option<String>,
    pub sort: LeagueSort,
    /// 1-based; past the end gives the last page.
    pub page: u32,
}

// ---- Types the UI receives ---------------------------------------------------

/// Where a run comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RunSource {
    /// Derived from the log.
    Log,
    /// A league the player recorded by hand.
    Manual,
    /// A run brought over from elsewhere: the 0.x import, or a league pulled
    /// from the cloud (both are stored as imported runs; nothing in the UI
    /// calls it 0.x).
    Imported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RunStateView {
    Active,
    Complete,
    Partial,
    Dropped,
}

/// A won (`W`) or lost (`L`) match or game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum MarkView {
    W,
    L,
}

impl MarkView {
    /// A stored `matches.outcome`: W, L, or `None` for any other outcome.
    pub(crate) fn from_stored_outcome(outcome: &str) -> Option<MarkView> {
        match store::names::stored_outcome(outcome) {
            Outcome::Win => Some(MarkView::W),
            Outcome::Loss => Some(MarkView::L),
            Outcome::Draw | Outcome::Unknown => None,
        }
    }
}

/// How a run card reads (0.x `classification`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClassificationView {
    Empty,
    Live,
    Brick,
    Trophy,
    Cash,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TimeOfDayView {
    Morning,
    Afternoon,
    Evening,
    Night,
}

/// Which record gave a run's win probability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum OddsSourceView {
    League,
    Deck,
    None,
}

/// The deck a run card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RunDeckView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// Colour identity letters in order, e.g. `["U", "R"]`.
    pub colors: Vec<String>,
    /// The cover card's art crop URL.
    pub cover_art: Option<String>,
    /// Its file in v1's images folder (`card` scheme), when downloaded.
    pub local_cover_art: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameMarkView {
    pub result: MarkView,
    pub on_play: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WinLossView {
    pub wins: u32,
    pub losses: u32,
}

/// One counted match of a run card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RunMatchView {
    pub token: String,
    /// W when won, L otherwise (a draw shows L, as in 0.x).
    pub result: MarkView,
    pub opponent_name: Option<String>,
    /// The opponent seat's archetype (0.x `opponentArchetype`,
    /// `opponentArchetypeId`).
    pub opponent_archetype: Option<String>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub opponent_archetype_id: Option<i64>,
    /// Games with a result, in play order.
    pub games: Vec<GameMarkView>,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub duration_seconds: Option<i64>,
    /// The match's note.
    pub notes: Option<String>,
    /// Entered by hand (0.x `manual`: the row shows the "M" badge).
    pub manual: bool,
    /// A limited match's opponent colours, WUBRG letters comma separated
    /// (0.x `opponentColors`); only limited runs fill it.
    #[specta(optional)]
    pub opponent_colors: Option<String>,
}

/// One opponent archetype faced, with the record against it (0.x
/// `topMatchups`, `topMatchup`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TopMatchupView {
    pub archetype: String,
    pub wins: u32,
    pub losses: u32,
    /// Matches against it.
    pub count: u32,
}

pub(crate) fn top_matchup_view(matchup: &leagues::TopMatchup) -> TopMatchupView {
    TopMatchupView {
        archetype: matchup.archetype.clone(),
        wins: matchup.wins,
        losses: matchup.losses,
        count: matchup.count,
    }
}

/// A deck archetype the Leagues page can filter by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueArchetypeOption {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
}

/// One league run card (0.x `FormatLeagueRuns::formatRun`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RunCardView {
    /// The run key: notes, drops and links name the run by it.
    pub key: String,
    pub source: RunSource,
    pub name: String,
    /// The display format ("Modern"), empty when unknown.
    pub format: String,
    /// The format code (`CMODERN`).
    pub format_code: Option<String>,
    pub state: RunStateView,
    /// The status the player set (league self-management spec 4.3); `None`
    /// is Automatic. `state` shows it when set.
    pub user_state: Option<RunStateView>,
    /// The fold's state: what Automatic shows ("Automatic (Complete)").
    pub auto_state: RunStateView,
    /// A user-created league (shows the Manual badge, links and unlinks).
    pub manual: bool,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub dropped_at_ms: Option<i64>,
    /// `None` when the run has no deck.
    pub deck: Option<RunDeckView>,
    /// `vN` of the run's deck version.
    pub version_label: Option<String>,
    /// One mark per counted match; an Active run is padded with `null` to 5.
    pub results: Vec<Option<MarkView>>,
    pub matches: Vec<RunMatchView>,
    /// The run's note.
    pub notes: Option<String>,
    pub classification: ClassificationView,
    /// The round being played, for a LIVE run.
    pub live_round: Option<u32>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_match_seconds: Option<i64>,
    pub time_of_day: Option<TimeOfDayView>,
    pub game_wins: u32,
    pub game_losses: u32,
    pub on_play_record: WinLossView,
    pub on_draw_record: WinLossView,
    /// Net tix of the finish, `None` when unpriced or unfinished.
    pub tix_delta: Option<f64>,
    /// The most faced opponent archetype (0.x `topOpponentArchetype`).
    pub top_opponent_archetype: Option<String>,
    /// The three most faced opponent archetypes (0.x `topMatchups`).
    pub top_matchups: Vec<TopMatchupView>,
    /// A draft or sealed run (the home dashboard's League in progress card
    /// shows one; the Leagues page lists constructed runs only).
    pub limited: bool,
    /// A limited run's deck colours, letters in WUBRG order (`None` without
    /// a registered deck, and for a constructed run, whose deck has them).
    pub colors: Option<Vec<String>>,
}

/// The KPI strip (0.x `GetLeagueKpis`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueKpisView {
    pub total: u32,
    /// Complete and Dropped runs.
    pub completed: u32,
    pub live: u32,
    pub decks: u32,
    pub trophies: u32,
    pub trophy_rate: Option<f64>,
    pub cash_rate: Option<f64>,
    pub dropped: u32,
    pub drop_rate: Option<f64>,
    pub avg_finish: Option<f64>,
    /// The most faced opponent archetype (0.x `topMatchup`).
    pub top_matchup: Option<TopMatchupView>,
}

/// A format the Leagues page can filter by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueFormatOption {
    pub code: String,
    pub label: String,
}

/// A deck the Leagues page can filter by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueDeckOption {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
}

/// A deck a manual league can be created for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualDeckOption {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// The display format.
    pub format: String,
    pub format_code: String,
}

/// One Leagues page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueListView {
    pub runs: Vec<RunCardView>,
    /// Runs matching the filters, every page.
    pub total: u32,
    /// The page shown (a page past the end shows the last one).
    pub page: u32,
    pub per_page: u32,
    pub last_page: u32,
    /// Over every run matching the filters, not just this page.
    pub kpis: LeagueKpisView,
    /// The formats of the listed runs' complete matches, by code.
    pub format_options: Vec<LeagueFormatOption>,
    /// The decks of the listed runs, by name.
    pub deck_options: Vec<LeagueDeckOption>,
    /// The archetypes of those decks, by name (0.x `deckArchetypes`).
    pub deck_archetypes: Vec<LeagueArchetypeOption>,
    /// The account's decks that are not archived, by name.
    pub manual_deck_options: Vec<ManualDeckOption>,
}

/// A deck's Leagues tab.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckLeaguesView {
    /// Newest first; cards count only the deck's matches.
    pub runs: Vec<RunCardView>,
    pub kpis: LeagueKpisView,
}

/// One row of a tournament's rounds table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TournamentMatchView {
    pub token: String,
    /// `in_progress`, `ended`, `complete` or `abandoned`.
    pub state: String,
    /// `null` until the match is complete (a live dot).
    pub result: Option<MarkView>,
    pub opponent_name: Option<String>,
    /// The opponent seat's archetype.
    pub opponent_archetype: Option<String>,
    pub games: Vec<GameMarkView>,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub duration_seconds: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub round: Option<i64>,
    pub notes: Option<String>,
}

/// One tournament card on a deck's Tournaments tab (0.x
/// `FormatTournamentRuns`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TournamentView {
    #[specta(type = specta_typescript::Number)]
    pub event_id: i64,
    /// `Tournament {eventId} ({d-m-Y})` until names come from the API (#7).
    pub name: String,
    /// The display format, empty when unknown.
    pub format: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    /// `vN`, or `vN` en dash `vM` over several versions.
    pub version_label: Option<String>,
    /// Complete matches only.
    pub results: Vec<MarkView>,
    /// Every match of the deck, in start order, in progress ones included.
    pub matches: Vec<TournamentMatchView>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_match_seconds: Option<i64>,
    pub game_wins: u32,
    pub game_losses: u32,
    pub on_play_record: WinLossView,
    pub on_draw_record: WinLossView,
    pub matches_count: u32,
    pub in_progress_count: u32,
    pub wins: u32,
    pub losses: u32,
}

/// One "League Finishes" row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FinishBucketView {
    /// `5-0` down to `0-5`.
    pub label: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FinishesView {
    pub buckets: Vec<FinishBucketView>,
    pub dropped: u32,
}

/// Where a run in progress is likely to end (0.x `LeagueFinishOdds`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FinishOddsView {
    pub rounds_left: u32,
    pub prize_wins: Option<u32>,
    pub wins_for_prize: Option<u32>,
    /// A whole percentage.
    pub chance_of_prize: Option<u32>,
    pub expected_tix: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InProgressView {
    pub wins: u32,
    pub losses: u32,
    /// A whole percentage.
    pub win_probability: u32,
    pub odds_source: OddsSourceView,
    pub odds: FinishOddsView,
}

/// The dashboard's run card: the run in progress with its odds, or else the
/// latest finished run.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PanelRunView {
    pub run: RunCardView,
    /// Set for a run in progress (Active).
    pub in_progress: Option<InProgressView>,
}

/// A deck dashboard's league panels.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckLeaguePanelsView {
    pub finishes: FinishesView,
    pub current: Option<PanelRunView>,
}

/// A match a manual league can link (0.x `AvailableMatchesController`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AvailableMatchView {
    pub token: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub ended_at_ms: Option<i64>,
    /// `null` for a draw.
    pub result: Option<MarkView>,
    pub opponent_name: Option<String>,
    /// The opponent seat's archetype (0.x `AddMatchDialog`).
    pub opponent_archetype: Option<String>,
    /// Games won and lost, `2-1`.
    pub game_record: String,
    /// The league it sits in now (spec 5.3: "in 11 Oct league"); `None`
    /// for no league.
    pub current_league: Option<LeagueRefView>,
}

/// A league as the match pickers name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueRefView {
    pub key: String,
    pub name: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
}

/// A league "Move to league..." offers (spec 5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueTargetView {
    pub key: String,
    pub name: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    /// What the league's card shows: the player's status, else the fold's.
    pub state: String,
    pub manual: bool,
    pub match_count: u32,
}

/// Where a match is and where it can go ("Move to league...").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchLeagueTargetsView {
    /// The league it sits in now; `None` for no league.
    pub current: Option<LeagueRefView>,
    /// Newest first.
    pub targets: Vec<LeagueTargetView>,
}

/// What a delete answers (spec 5.2): the league's row id and when it was
/// removed, for Undo, and the status the player had set when the delete also recorded
/// a drop (spec 4.4), for Undo to put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeleteLeagueView {
    /// The league's row id, for Undo: a rekey keeps the row and changes
    /// its run key (sync identity spec 2.5).
    #[specta(type = specta_typescript::Number)]
    pub league_id: i64,
    #[specta(type = specta_typescript::Number)]
    pub removed_at: i64,
    pub previous_status: Option<String>,
}

pub(crate) fn league_ref_view(league: LeagueRef) -> LeagueRefView {
    LeagueRefView {
        key: league.run_key,
        name: league.name,
        started_at_ms: league.started_at,
    }
}

fn target_view(target: MoveTarget) -> LeagueTargetView {
    LeagueTargetView {
        manual: target.source == "manual",
        key: target.league.run_key,
        name: target.league.name,
        started_at_ms: target.league.started_at,
        state: target.state,
        match_count: target.match_count,
    }
}

// ---- Gathering runs ----------------------------------------------------------

/// One league run of any source, with its matches.
#[derive(Debug, Clone, PartialEq)]
pub struct GatheredRun {
    pub key: String,
    pub source: RunSource,
    pub name: String,
    pub dropped_at: Option<i64>,
    pub deck_version_id: Option<i64>,
    pub deck_id: Option<i64>,
    /// The fold's state (`StoredRun::state`). `input.state` is what the
    /// cards show: `StoredRun::shown_state()`, the player's status when set
    /// (spec 4.3), else this.
    pub fold_state: RunState,
    /// `StoredRun::user_state`.
    pub user_state: Option<UserState>,
    /// What the numbers read.
    pub input: RunInput,
    /// The run's matches as stored (opponents, formats).
    pub rows: Vec<LeagueMatchRow>,
}

/// Every run of some accounts and what their cards need.
#[derive(Debug, Clone, PartialEq)]
pub struct Gathered {
    /// Derived runs, then manual leagues, then imported runs (each by
    /// account, as `stored_runs` orders them).
    pub runs: Vec<GatheredRun>,
    pub decks: HashMap<i64, LeagueDeck>,
    pub versions: Vec<VersionDate>,
    /// League notes by run key.
    pub notes: HashMap<String, String>,
    /// Match notes by match token.
    pub notes_by_match: HashMap<String, String>,
}

/// The owners whose runs a page of `accounts` lists: those accounts, and the
/// runs with no account (0.x leagues the import could not give one).
pub(crate) fn owners_of(accounts: &[i64]) -> Vec<LeagueOwner> {
    let mut owners: Vec<LeagueOwner> = accounts
        .iter()
        .map(|account| LeagueOwner::Account(*account))
        .collect();
    owners.push(LeagueOwner::Nobody);
    owners
}

pub(crate) fn run_source(source: &str) -> RunSource {
    match source {
        "manual" => RunSource::Manual,
        "imported" => RunSource::Imported,
        _ => RunSource::Log,
    }
}

/// Every stored constructed run of `accounts` (plus the account-less manual
/// leagues and imported runs). One query per kind of row, whatever the
/// count; the log is never read.
pub fn gather(conn: &Connection, accounts: &[i64]) -> Result<Gathered, StoreError> {
    let mut decks: HashMap<i64, LeagueDeck> = HashMap::new();
    for deck in league_decks(conn)? {
        decks.insert(deck.id, deck);
    }
    let versions = version_dates(conn)?;
    let filter = RunFilter {
        owners: owners_of(accounts),
        kinds: vec![LeagueKind::Constructed],
        deck_id: None,
        run_key: None,
    };
    let mut runs = Vec::new();
    let mut notes = HashMap::new();
    for run in stored_runs(conn, &filter)? {
        if let Some(note) = &run.notes {
            notes.insert(run.key.clone(), note.clone());
        }
        runs.push(gathered_run(run));
    }
    Ok(Gathered {
        runs,
        decks,
        versions,
        notes,
        notes_by_match: notes_by_match(conn)?,
    })
}

fn gathered_run(run: StoredRun) -> GatheredRun {
    let input = run.run_input();
    GatheredRun {
        input,
        fold_state: run.state,
        user_state: run.user_state,
        source: run_source(&run.source),
        key: run.key,
        name: run.name,
        dropped_at: run.dropped_at,
        deck_version_id: run.deck_version_id,
        deck_id: run.deck_id,
        rows: run.matches,
    }
}

/// The run as the fold left it. The "current run" rules (the deck
/// dashboard's run in progress, the home dashboard) read the fold's state,
/// never the player's (spec 4.3), so marking an old run Active does not
/// make it the current one.
fn fold_input(run: &GatheredRun) -> RunInput {
    let mut input = run.input.clone();
    input.state = run.fold_state;
    input
}

/// The accounts a read takes: the one given, else every account of
/// `platform`.
pub(crate) fn accounts_or_all(
    conn: &Connection,
    platform: store::Platform,
    account: Option<i64>,
) -> Result<Vec<i64>, StoreError> {
    match account {
        Some(id) => Ok(vec![id]),
        None => account_ids(conn, platform),
    }
}

/// The deck's account (every account's for a deck with none), `None` when
/// there is no such deck.
pub(crate) fn deck_accounts(
    conn: &Connection,
    deck_id: i64,
) -> Result<Option<Vec<i64>>, StoreError> {
    match store::decks::deck_by_id(conn, deck_id)? {
        Some(deck) => Ok(Some(accounts_or_all(
            conn,
            store::Platform::Mtgo,
            deck.account_id,
        )?)),
        None => Ok(None),
    }
}

/// Whether the run holds one of the deck's complete matches (of `version`
/// when given).
fn has_complete_deck_match(run: &GatheredRun, deck_id: i64, version: Option<i64>) -> bool {
    run.input.matches.iter().any(|played| {
        played.state == MatchState::Complete
            && played.deck_id == Some(deck_id)
            && (version.is_none() || played.deck_version_id == version)
    })
}

// ---- Cards -------------------------------------------------------------------

fn deck_view(deck: &LeagueDeck) -> RunDeckView {
    RunDeckView {
        id: deck.id,
        name: deck.name.clone(),
        colors: color_list(deck.color_identity.as_deref()),
        cover_art: deck.cover_art.clone(),
        local_cover_art: deck.local_cover_art.clone(),
    }
}

pub(crate) fn mark_view(mark: Mark) -> MarkView {
    match mark {
        Mark::W => MarkView::W,
        Mark::L => MarkView::L,
    }
}

pub(crate) fn state_view(state: RunState) -> RunStateView {
    match state {
        RunState::Active => RunStateView::Active,
        RunState::Complete => RunStateView::Complete,
        RunState::Partial => RunStateView::Partial,
        RunState::Dropped => RunStateView::Dropped,
    }
}

pub(crate) fn classification_view(classification: Classification) -> ClassificationView {
    match classification {
        Classification::Empty => ClassificationView::Empty,
        Classification::Live => ClassificationView::Live,
        Classification::Brick => ClassificationView::Brick,
        Classification::Trophy => ClassificationView::Trophy,
        Classification::Cash => ClassificationView::Cash,
        Classification::Finish => ClassificationView::Finish,
    }
}

pub(crate) fn time_of_day_view(time: TimeOfDay) -> TimeOfDayView {
    match time {
        TimeOfDay::Morning => TimeOfDayView::Morning,
        TimeOfDay::Afternoon => TimeOfDayView::Afternoon,
        TimeOfDay::Evening => TimeOfDayView::Evening,
        TimeOfDay::Night => TimeOfDayView::Night,
    }
}

pub(crate) fn win_loss_view(record: WinLoss) -> WinLossView {
    WinLossView {
        wins: record.wins,
        losses: record.losses,
    }
}

pub(crate) fn game_views(games: &[GameMark]) -> Vec<GameMarkView> {
    let mut views = Vec::new();
    for game in games {
        views.push(GameMarkView {
            result: mark_view(game.result),
            on_play: game.on_play,
        });
    }
    views
}

/// The run's deck (0.x): its own deck, else the deck most of its counted
/// matches were played with (a tie goes to the first played).
fn run_deck(run: &GatheredRun, summary: &RunSummary, data: &Gathered) -> Option<RunDeckView> {
    if let Some(deck) = run.deck_id.and_then(|id| data.decks.get(&id)) {
        return Some(deck_view(deck));
    }
    // (deck, count) in first-played order.
    let mut counts: Vec<(i64, u32)> = Vec::new();
    for counted in &summary.matches {
        let deck_id = run
            .input
            .matches
            .iter()
            .find(|played| played.token == counted.token)
            .and_then(|played| played.deck_id);
        let Some(deck_id) = deck_id else {
            continue;
        };
        match counts.iter_mut().find(|(seen, _)| *seen == deck_id) {
            Some((_, count)) => *count += 1,
            None => counts.push((deck_id, 1)),
        }
    }
    let mut best: Option<(i64, u32)> = None;
    for (deck_id, count) in counts {
        if best.is_none_or(|(_, best_count)| count > best_count) {
            best = Some((deck_id, count));
        }
    }
    best.and_then(|(deck_id, _)| data.decks.get(&deck_id))
        .map(deck_view)
}

/// `vN` of the run's deck version (0.x `FormatLeagueRuns`).
fn run_version_label(run: &GatheredRun, versions: &[VersionDate]) -> Option<String> {
    let version = run.deck_version_id?;
    let found = versions.iter().find(|row| row.id == version)?;
    let mut dates = Vec::new();
    for row in versions.iter().filter(|row| row.deck_id == found.deck_id) {
        dates.push(row.modified_at);
    }
    Some(version_label(found.modified_at, &dates))
}

/// One run card (0.x `FormatLeagueRuns::formatRun`). `deck_id` counts only
/// that deck's matches (the deck pages).
pub(crate) fn card(
    run: &GatheredRun,
    deck_id: Option<i64>,
    data: &Gathered,
    tz: Tz,
) -> RunCardView {
    let summary = summarize_run(&run.input, deck_id, tz);
    let mut matches = Vec::new();
    for counted in &summary.matches {
        let stored = run.rows.iter().find(|row| row.token == counted.token);
        matches.push(RunMatchView {
            token: counted.token.clone(),
            result: mark_view(counted.result),
            opponent_name: stored.and_then(|row| row.opponent.clone()),
            opponent_archetype: counted.opponent_archetype.clone(),
            opponent_archetype_id: stored.and_then(|row| row.opponent_archetype_id),
            games: game_views(&counted.games),
            started_at_ms: counted.started_at,
            duration_seconds: counted.duration_seconds,
            notes: data.notes_by_match.get(&counted.token).cloned(),
            manual: stored.is_some_and(|row| row.source == "manual"),
            opponent_colors: None,
        });
    }
    let mut results = Vec::new();
    for result in &summary.results {
        results.push(result.map(mark_view));
    }
    RunCardView {
        key: run.key.clone(),
        source: run.source,
        name: run.name.clone(),
        format: display_format(run.input.format.as_deref().unwrap_or("")),
        format_code: run.input.format.clone(),
        state: state_view(run.input.state),
        user_state: run.user_state.map(|user| state_view(user.run_state())),
        auto_state: state_view(run.fold_state),
        manual: run.input.manual,
        started_at_ms: run.input.started_at,
        dropped_at_ms: run.dropped_at,
        deck: run_deck(run, &summary, data),
        version_label: run_version_label(run, &data.versions),
        results,
        matches,
        notes: data.notes.get(&run.key).cloned(),
        classification: classification_view(summary.classification),
        live_round: summary.live_round,
        avg_match_seconds: summary.avg_match_seconds,
        time_of_day: summary.time_of_day.map(time_of_day_view),
        game_wins: summary.game_wins,
        game_losses: summary.game_losses,
        on_play_record: win_loss_view(summary.on_play),
        on_draw_record: win_loss_view(summary.on_draw),
        tix_delta: summary.tix_delta,
        top_opponent_archetype: summary.top_opponent_archetype.clone(),
        top_matchups: summary.top_matchups.iter().map(top_matchup_view).collect(),
        limited: false,
        colors: None,
    }
}

/// Every constructed run of `data` as the home dashboard reads it
/// (dashboard redesign spec 4.2, 4.6): its run card's marks and deck.
pub fn dashboard_runs(data: &Gathered, tz: Tz) -> Vec<DashboardLeagueRun> {
    let rounds = LeagueKind::Constructed.round_count() as u32;
    let mut runs = Vec::new();
    for run in &data.runs {
        let shown = card(run, None, data, tz);
        let wins = shown
            .results
            .iter()
            .filter(|mark| **mark == Some(MarkView::W))
            .count() as u32;
        let losses = shown
            .results
            .iter()
            .filter(|mark| **mark == Some(MarkView::L))
            .count() as u32;
        let last_played_at = run
            .rows
            .iter()
            .map(|row| row.started_at)
            .max()
            .unwrap_or(run.input.started_at);
        let deck = shown.deck.as_ref();
        runs.push(DashboardLeagueRun {
            key: run.key.clone(),
            kind: LeagueKind::Constructed,
            rounds,
            wins,
            losses,
            state: run.fold_state,
            shown_state: run.input.state,
            name: run.name.clone(),
            subject: deck.map(|deck| deck.name.clone()).unwrap_or_default(),
            started_at: run.input.started_at,
            last_played_at,
            format: run.input.format.clone(),
            deck_id: deck.map(|deck| deck.id),
            set_code: None,
            set_name: None,
            colors: deck.map(|deck| deck.colors.clone()),
            colour_name: None,
            cover_art: deck.and_then(|deck| deck.cover_art.clone()),
            local_cover_art: deck.and_then(|deck| deck.local_cover_art.clone()),
            game_wins: shown.game_wins,
            game_losses: shown.game_losses,
            first_pick: None,
            match_tokens: run.rows.iter().map(|row| row.token.clone()).collect(),
        });
    }
    runs
}

fn kpis_view(runs: &[&GatheredRun]) -> LeagueKpisView {
    let mut inputs = Vec::new();
    for run in runs {
        inputs.push(run.input.clone());
    }
    let kpis = league_kpis(&inputs);
    LeagueKpisView {
        total: kpis.total,
        completed: kpis.completed,
        live: kpis.live,
        decks: kpis.decks,
        trophies: kpis.trophies,
        trophy_rate: kpis.trophy_rate,
        cash_rate: kpis.cash_rate,
        dropped: kpis.dropped,
        drop_rate: kpis.drop_rate,
        avg_finish: kpis.avg_finish,
        top_matchup: kpis.top_matchup.as_ref().map(top_matchup_view),
    }
}

// ---- The Leagues page ----------------------------------------------------------

/// Whether the run passes the page's filters (0.x `buildQuery`).
fn passes_filters(
    run: &GatheredRun,
    query: &LeagueListQuery,
    decks: &HashMap<i64, LeagueDeck>,
) -> bool {
    if let Some(format) = not_blank(query.format.as_deref()) {
        let own = run.input.format.as_deref() == Some(format.as_str());
        let played = run
            .rows
            .iter()
            .any(|row| row.state == "complete" && row.format.as_deref() == Some(format.as_str()));
        if !own && !played {
            return false;
        }
    }
    if !passes_state_filter(query.state.as_str(), &run.input) {
        return false;
    }
    if let Some(deck_id) = query.deck_id
        && run.deck_id != Some(i64::from(deck_id))
    {
        return false;
    }
    if let Some(archetype_id) = query.archetype_id {
        let deck_archetype = run
            .deck_id
            .and_then(|id| decks.get(&id))
            .and_then(|deck| deck.archetype_id);
        if deck_archetype != Some(i64::from(archetype_id)) {
            return false;
        }
    }
    if let Some(search) = not_blank(query.search.as_deref()) {
        let term = search.to_lowercase();
        let contains =
            |text: Option<&str>| text.is_some_and(|name| name.to_lowercase().contains(&term));
        let found = run.rows.iter().any(|row| {
            row.state == "complete"
                && (contains(row.opponent.as_deref())
                    || contains(row.opponent_archetype.as_deref()))
        });
        if !found {
            return false;
        }
    }
    true
}

/// 0.x's sorts; equal runs keep newest first, then the run key.
fn sort_runs(runs: &mut [&GatheredRun], sort: LeagueSort) {
    runs.sort_by(|a, b| {
        let newest = b.input.started_at.cmp(&a.input.started_at);
        let order = match sort {
            LeagueSort::Newest => newest,
            LeagueSort::Oldest => a.input.started_at.cmp(&b.input.started_at),
            LeagueSort::Best => complete_wins(&b.input)
                .cmp(&complete_wins(&a.input))
                .then(newest),
            LeagueSort::Worst => complete_wins(&a.input)
                .cmp(&complete_wins(&b.input))
                .then(newest),
        };
        order.then_with(|| a.key.cmp(&b.key))
    });
}

/// The formats of the runs' complete matches, by code (0.x `allFormats`).
fn format_options(runs: &[&GatheredRun]) -> Vec<LeagueFormatOption> {
    let mut codes: Vec<String> = Vec::new();
    for run in runs {
        for row in &run.rows {
            if row.state != "complete" {
                continue;
            }
            if let Some(code) = &row.format
                && !codes.contains(code)
            {
                codes.push(code.clone());
            }
        }
    }
    codes.sort();
    let mut options = Vec::new();
    for code in codes {
        options.push(LeagueFormatOption {
            label: display_format(&code),
            code,
        });
    }
    options
}

/// The runs' decks, by name (0.x `allDecks`).
fn deck_options(runs: &[&GatheredRun], decks: &HashMap<i64, LeagueDeck>) -> Vec<LeagueDeckOption> {
    let mut found: Vec<&LeagueDeck> = Vec::new();
    for run in runs {
        if let Some(deck) = run.deck_id.and_then(|id| decks.get(&id))
            && !found.iter().any(|seen| seen.id == deck.id)
        {
            found.push(deck);
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let mut options = Vec::new();
    for deck in found {
        options.push(LeagueDeckOption {
            id: deck.id,
            name: deck.name.clone(),
        });
    }
    options
}

/// The archetypes of the runs' decks, by name (0.x `deckArchetypes`).
fn deck_archetypes(
    runs: &[&GatheredRun],
    decks: &HashMap<i64, LeagueDeck>,
) -> Vec<LeagueArchetypeOption> {
    let mut found: Vec<LeagueArchetypeOption> = Vec::new();
    for run in runs {
        let Some(deck) = run.deck_id.and_then(|id| decks.get(&id)) else {
            continue;
        };
        if let (Some(id), Some(name)) = (deck.archetype_id, &deck.archetype_name)
            && !found.iter().any(|seen| seen.id == id)
        {
            found.push(LeagueArchetypeOption {
                id,
                name: name.clone(),
            });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    found
}

/// The account's decks a manual league can be created for, by name (0.x
/// `manualDeckOptions`; archived decks are refused, so they are left out).
fn manual_deck_options(
    account: Option<i64>,
    decks: &HashMap<i64, LeagueDeck>,
) -> Vec<ManualDeckOption> {
    let Some(account) = account else {
        return Vec::new();
    };
    let mut found: Vec<&LeagueDeck> = decks
        .values()
        .filter(|deck| deck.account_id == Some(account) && !deck.archived)
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let mut options = Vec::new();
    for deck in found {
        options.push(ManualDeckOption {
            id: deck.id,
            name: deck.name.clone(),
            format: display_format(&deck.format),
            format_code: deck.format.clone(),
        });
    }
    options
}

/// One Leagues page (0.x `Leagues\IndexController`).
pub fn list(
    conn: &Connection,
    query: &LeagueListQuery,
    tz: Tz,
) -> Result<LeagueListView, CommandError> {
    let account =
        store::accounts::current_account_id(conn, store::Platform::Mtgo).map_err(storage)?;
    let accounts = accounts_or_all(conn, store::Platform::Mtgo, account).map_err(storage)?;
    let data = gather(conn, &accounts).map_err(storage)?;

    let base: Vec<&GatheredRun> = data
        .runs
        .iter()
        .filter(|run| on_leagues_page(&run.input))
        .collect();
    let mut filtered: Vec<&GatheredRun> = base
        .iter()
        .copied()
        .filter(|run| passes_filters(run, query, &data.decks))
        .collect();
    sort_runs(&mut filtered, query.sort);

    let total = filtered.len() as u32;
    let last_page = total.div_ceil(LEAGUES_PER_PAGE).max(1);
    let page = query.page.clamp(1, last_page);
    let start = ((page - 1) * LEAGUES_PER_PAGE) as usize;
    let mut runs = Vec::new();
    for run in filtered.iter().skip(start).take(LEAGUES_PER_PAGE as usize) {
        runs.push(card(run, None, &data, tz));
    }

    Ok(LeagueListView {
        runs,
        total,
        page,
        per_page: LEAGUES_PER_PAGE,
        last_page,
        kpis: kpis_view(&filtered),
        format_options: format_options(&base),
        deck_options: deck_options(&base, &data.decks),
        deck_archetypes: deck_archetypes(&base, &data.decks),
        manual_deck_options: manual_deck_options(account, &data.decks),
    })
}

// ---- Deck pages ----------------------------------------------------------------

fn in_range(at_ms: i64, range: &Range) -> bool {
    at_ms >= range.from_ms && at_ms <= range.to_ms
}

/// A deck's Leagues tab (0.x `Decks\LeaguesController`): runs holding one of
/// the deck's complete matches (of `version` when given) that started in the
/// timeframe, newest first. Cards count only the deck's matches; the KPIs
/// count every match of those runs, as in 0.x.
pub fn deck_league_runs(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<DeckLeaguesView, CommandError> {
    let Some(accounts) = deck_accounts(conn, deck_id).map_err(storage)? else {
        return Ok(DeckLeaguesView {
            runs: Vec::new(),
            kpis: kpis_view(&[]),
        });
    };
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let data = gather(conn, &accounts).map_err(storage)?;
    let mut found: Vec<&GatheredRun> = data
        .runs
        .iter()
        .filter(|run| {
            has_complete_deck_match(run, deck_id, version) && in_range(run.input.started_at, &range)
        })
        .collect();
    sort_runs(&mut found, LeagueSort::Newest);
    let mut runs = Vec::new();
    for run in &found {
        runs.push(card(run, Some(deck_id), &data, tz));
    }
    Ok(DeckLeaguesView {
        runs,
        kpis: kpis_view(&found),
    })
}

/// A deck's Tournaments tab (0.x `Decks\TournamentsController`):
/// tournaments with a complete match of the deck (of `version` when given)
/// started in the timeframe, newest first. Each lists every match of the
/// deck in it, in progress ones included (the live dots).
pub fn deck_tournament_runs(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    timeframe: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Vec<TournamentView>, CommandError> {
    let Some(accounts) = deck_accounts(conn, deck_id).map_err(storage)? else {
        return Ok(Vec::new());
    };
    let (range, _previous) = timeframe_ranges(timeframe, now_ms, tz);
    let mut groups = Vec::new();
    for account in &accounts {
        for group in
            stored_tournaments(conn, *account, Some(deck_id), None, None).map_err(storage)?
        {
            let wanted = group.matches.iter().any(|row| {
                row.state == "complete"
                    && (version.is_none() || row.deck_version_id == version)
                    && in_range(row.started_at, &range)
            });
            if wanted {
                groups.push(group);
            }
        }
    }
    groups.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then(b.event_id.cmp(&a.event_id))
    });

    let mut versions = Vec::new();
    for row in version_dates(conn).map_err(storage)? {
        if row.deck_id == deck_id {
            versions.push(DeckVersionDate {
                id: row.id,
                modified_at: row.modified_at,
            });
        }
    }
    let notes = notes_by_match(conn).map_err(storage)?;

    let mut views = Vec::new();
    for group in &groups {
        let mut matches = Vec::new();
        for row in &group.matches {
            matches.push(row.tournament_match());
        }
        let input = TournamentInput {
            event_id: group.event_id,
            format: group.format.clone(),
            started_at: group.started_at,
            matches,
        };
        let run = tournament_run(&input, &versions, tz);
        views.push(tournament_view(run, &group.matches, &notes));
    }
    Ok(views)
}

fn tournament_view(
    run: TournamentRun,
    rows: &[LeagueMatchRow],
    notes: &HashMap<String, String>,
) -> TournamentView {
    let mut matches = Vec::new();
    for row in &run.matches {
        let stored = rows.iter().find(|stored| stored.token == row.token);
        matches.push(TournamentMatchView {
            token: row.token.clone(),
            state: store::names::state_str(row.state).to_string(),
            result: row.result.map(mark_view),
            opponent_name: stored.and_then(|stored| stored.opponent.clone()),
            opponent_archetype: stored.and_then(|stored| stored.opponent_archetype.clone()),
            games: game_views(&row.games),
            started_at_ms: row.started_at,
            duration_seconds: row.duration_seconds,
            round: row.round,
            notes: notes.get(&row.token).cloned(),
        });
    }
    let mut results = Vec::new();
    for mark in &run.results {
        results.push(mark_view(*mark));
    }
    TournamentView {
        event_id: run.event_id,
        name: run.name,
        format: run.format,
        started_at_ms: run.started_at,
        version_label: run.version_label,
        results,
        matches,
        avg_match_seconds: run.avg_match_seconds,
        game_wins: run.game_wins,
        game_losses: run.game_losses,
        on_play_record: win_loss_view(run.on_play),
        on_draw_record: win_loss_view(run.on_draw),
        matches_count: run.matches_count,
        in_progress_count: run.in_progress_count,
        wins: run.wins,
        losses: run.losses,
    }
}

/// The deck's matches with an outcome (of `version` when given) that belong
/// to a run, each counted once: the record its league odds read (0.x
/// `GetLeagueInProgress::winProbability` counts `outcome IS NOT NULL`, not
/// the match state).
fn league_record(runs: &[GatheredRun], deck_id: i64, version: Option<i64>) -> Record {
    let mut seen: HashSet<&str> = HashSet::new();
    let (mut wins, mut losses, mut total) = (0, 0, 0);
    for run in runs {
        for played in &run.input.matches {
            let wanted = played.outcome != Outcome::Unknown
                && played.deck_id == Some(deck_id)
                && (version.is_none() || played.deck_version_id == version);
            if !wanted || !seen.insert(played.token.as_str()) {
                continue;
            }
            total += 1;
            match played.outcome {
                Outcome::Win => wins += 1,
                Outcome::Loss => losses += 1,
                Outcome::Draw | Outcome::Unknown => {}
            }
        }
    }
    Record::from_total(wins, losses, total)
}

/// The deck's whole record (its matches with an outcome, of `version` when
/// given), as 0.x counts it.
fn overall_record(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
) -> Result<Record, StoreError> {
    store::deck_pages::deck_outcome_record(conn, deck_id, version)
}

fn odds_view(odds: FinishOdds) -> FinishOddsView {
    FinishOddsView {
        rounds_left: odds.rounds_left,
        prize_wins: odds.prize_wins,
        wins_for_prize: odds.wins_for_prize,
        chance_of_prize: odds.chance_of_prize,
        expected_tix: odds.expected_tix,
    }
}

fn in_progress_view(progress: InProgress) -> InProgressView {
    InProgressView {
        wins: progress.wins,
        losses: progress.losses,
        win_probability: progress.win_probability,
        odds_source: match progress.odds_source {
            OddsSource::League => OddsSourceView::League,
            OddsSource::Deck => OddsSourceView::Deck,
            OddsSource::None => OddsSourceView::None,
        },
        odds: odds_view(progress.odds),
    }
}

/// A deck dashboard's league panels, all time: "League Finishes" (0.x
/// `GetLeagueResultDistribution`) and the run in progress with its odds
/// (0.x `GetLeagueInProgress`), or else the latest Complete or Dropped run
/// (0.x `GetLatestLeague`). Run cards count every match of the run, as in
/// 0.x.
pub fn deck_panels(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
    tz: Tz,
) -> Result<DeckLeaguePanelsView, CommandError> {
    let accounts = deck_accounts(conn, deck_id)
        .map_err(storage)?
        .unwrap_or_default();
    let data = gather(conn, &accounts).map_err(storage)?;
    let mut inputs = Vec::new();
    let mut fold_inputs = Vec::new();
    for run in &data.runs {
        inputs.push(run.input.clone());
        fold_inputs.push(fold_input(run));
    }

    let distribution = result_distribution(&inputs, deck_id, version);
    let mut buckets = Vec::new();
    for (label, count) in distribution.buckets {
        buckets.push(FinishBucketView { label, count });
    }
    let finishes = FinishesView {
        buckets,
        dropped: distribution.dropped,
    };

    let index = in_progress_index(&fold_inputs, deck_id, version)
        .or_else(|| latest_finished_index(&fold_inputs, deck_id, version));
    let current = match index {
        Some(index) => {
            Some(panel_for_run(conn, &data, index, Some(deck_id), version, tz).map_err(storage)?)
        }
        None => None,
    };
    Ok(DeckLeaguePanelsView { finishes, current })
}

/// The run card of `data.runs[index]` as the deck dashboard and the home
/// dashboard show it: an Active run with a deck also gets its finish odds,
/// read from that deck's records (of `version` when given). `deck_id` is
/// the deck page's deck; `None` takes the run's own deck (the card's).
pub fn panel_for_run(
    conn: &Connection,
    data: &Gathered,
    index: usize,
    deck_id: Option<i64>,
    version: Option<i64>,
    tz: Tz,
) -> Result<PanelRunView, StoreError> {
    let run = &data.runs[index];
    let shown = card(run, None, data, tz);
    let odds_deck = deck_id.or_else(|| shown.deck.as_ref().map(|deck| deck.id));
    let in_progress = match odds_deck {
        Some(deck) if run.fold_state == RunState::Active => {
            let league = league_record(&data.runs, deck, version);
            let overall = overall_record(conn, deck, version)?;
            let progress = league_in_progress(&fold_input(run), &league, &overall);
            Some(in_progress_view(progress))
        }
        _ => None,
    };
    Ok(PanelRunView {
        run: shown,
        in_progress,
    })
}

/// The deck sidebar's trophies (0.x `GetDeckViewSharedProps`): Complete runs
/// in which the deck won every one of its complete matches, all time.
pub fn deck_trophies(conn: &Connection, deck_id: i64) -> Result<u32, CommandError> {
    let Some(accounts) = deck_accounts(conn, deck_id).map_err(storage)? else {
        return Ok(0);
    };
    let data = gather(conn, &accounts).map_err(storage)?;
    let mut inputs = Vec::new();
    for run in &data.runs {
        inputs.push(run.input.clone());
    }
    Ok(trophy_count(&inputs, deck_id))
}

/// The matches "Add matches..." offers a league (league self-management
/// spec 5.3, 0.x `AvailableMatchesController` for a manual league): the
/// matches `available_matches` allows (spec 4.1), newest first, at most
/// 100, each with the league it sits in now. None for a deleted league.
pub fn available(
    conn: &Connection,
    run_key: &str,
) -> Result<Vec<AvailableMatchView>, CommandError> {
    let mut views = Vec::new();
    for found in available_matches(conn, run_key).map_err(storage)? {
        let row = found.row;
        let won = row
            .games
            .iter()
            .filter(|game| game.result == GameResult::Win)
            .count();
        let lost = row
            .games
            .iter()
            .filter(|game| game.result == GameResult::Loss)
            .count();
        let result = MarkView::from_stored_outcome(&row.outcome);
        views.push(AvailableMatchView {
            token: row.token,
            started_at_ms: row.started_at,
            ended_at_ms: row.ended_at,
            result,
            opponent_name: row.opponent,
            opponent_archetype: row.opponent_archetype,
            game_record: format!("{won}-{lost}"),
            current_league: found.now_in.map(league_ref_view),
        });
    }
    Ok(views)
}

/// Where a match is and the leagues it may move to ("Move to league...").
pub fn targets(conn: &Connection, token: &str) -> Result<MatchLeagueTargetsView, CommandError> {
    let mut current = match_leagues(conn, &[token.to_string()]).map_err(storage)?;
    let mut views = Vec::new();
    for target in move_targets(conn, token).map_err(storage)? {
        views.push(target_view(target));
    }
    Ok(MatchLeagueTargetsView {
        current: current.remove(token).map(league_ref_view),
        targets: views,
    })
}

// ---- Edits -------------------------------------------------------------------

/// The local date and time formats a start is typed in.
const DATE_TIME_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M",
    "%Y-%m-%d %H:%M:%S",
];

/// A manual league's start, typed as a local date and time
/// (`YYYY-MM-DDTHH:MM`, seconds optional, or a date alone, as an HTML
/// `datetime-local` input sends it), read in `tz` like 0.x reads it in the
/// system zone, in UTC ms. A time the clocks skip is moved on by the gap.
pub fn parse_started_at(text: &str, tz: Tz) -> Result<i64, CommandError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CommandError::Refused(
            "The started at field is required.".to_string(),
        ));
    }
    let mut local: Option<NaiveDateTime> = None;
    for format in DATE_TIME_FORMATS {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, format) {
            local = Some(parsed);
            break;
        }
    }
    if local.is_none()
        && let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d")
    {
        local = day.and_hms_opt(0, 0, 0);
    }
    let invalid =
        || CommandError::Refused("The started at field must be a valid date.".to_string());
    let local = local.ok_or_else(invalid)?;
    mtgo_protocol::local_to_utc_ms(local, tz).ok_or_else(invalid)
}

fn league_refusal(outcome: LeagueEditOutcome) -> CommandError {
    CommandError::Refused(outcome.refusal().unwrap_or("Nothing changed.").to_string())
}

/// A done edit, or the store's refusal as a message for the user.
pub fn league_edit_result(outcome: LeagueEditOutcome) -> Result<(), CommandError> {
    match outcome {
        LeagueEditOutcome::Done => Ok(()),
        refusal => Err(league_refusal(refusal)),
    }
}

/// A league editor for the running tracker. The tracker lock is held only
/// to take it, never while waiting for an answer.
fn league_editor(state: &AppState) -> Result<tracker::LeagueEditor, CommandError> {
    state.tracker_part(tracker::TrackerHandle::league_editor)
}

fn send_edit(state: &AppState, edit: LeagueEdit) -> Result<(), CommandError> {
    let outcome = league_editor(state)?
        .edit(edit)
        .map_err(CommandError::Tracker)?;
    league_edit_result(outcome)
}

/// Why a manual league cannot be created before MyMTGO has seen the
/// player's account (it has no decks to offer then either).
pub const NO_ACCOUNT_YET: &str = "Play a match first so MyMTGO knows your account.";

/// 0.x `StoreController`: a manual league of one of the current account's
/// decks. The name is checked by the store (trimmed, 1 to 100 characters).
pub fn create_manual(
    state: &AppState,
    deck_id: i64,
    name: &str,
    started_at: &str,
    now_ms: i64,
) -> Result<(), CommandError> {
    let started_at = parse_started_at(started_at, state.timezone)?;
    let account = store::accounts::current_account_id(&lock(&state.reader), store::Platform::Mtgo)
        .map_err(storage)?;
    let Some(account_id) = account else {
        return Err(CommandError::Refused(NO_ACCOUNT_YET.to_string()));
    };
    send_edit(
        state,
        LeagueEdit::CreateManual {
            account_id,
            deck_id,
            name: name.to_string(),
            token: store::leagues::manual_league_token(&crate::api::random::random_bytes(64)),
            started_at,
            now_ms,
        },
    )
}

/// "Add matches..." (spec 5.3, 0.x `LinkMatchController`): any listed
/// league takes the matches spec 4.1 allows.
pub fn link(
    state: &AppState,
    run_key: &str,
    match_tokens: &[String],
    now_ms: i64,
) -> Result<(), CommandError> {
    send_edit(
        state,
        LeagueEdit::LinkMatches {
            run_key: run_key.to_string(),
            match_tokens: match_tokens.to_vec(),
            now_ms,
        },
    )
}

/// "Remove from league" (spec 4.2, 0.x `UnlinkMatchController` for a
/// manual league): the match leaves every league.
pub fn unlink(
    state: &AppState,
    run_key: &str,
    match_token: &str,
    now_ms: i64,
) -> Result<(), CommandError> {
    send_edit(
        state,
        LeagueEdit::UnlinkMatch {
            run_key: run_key.to_string(),
            match_token: match_token.to_string(),
            now_ms,
        },
    )
}

/// 0.x `UpdateNotesController`: trimmed; blank clears the note.
pub fn set_notes(
    state: &AppState,
    run_key: &str,
    notes: &str,
    now_ms: i64,
) -> Result<(), CommandError> {
    send_edit(
        state,
        LeagueEdit::SetNotes {
            run_key: run_key.to_string(),
            notes: notes.to_string(),
            now_ms,
        },
    )
}

/// A status the UI sends (`active`, `complete`, `partial`, `dropped`);
/// `None` is Automatic.
fn parse_status(status: Option<&str>) -> Result<Option<UserState>, CommandError> {
    match status {
        None => Ok(None),
        Some(text) => match UserState::parse(text) {
            Some(state) => Ok(Some(state)),
            None => Err(CommandError::Refused("Choose a status.".to_string())),
        },
    }
}

/// What a delete does with the league's matches: `keep` or `delete`.
fn parse_delete_matches(text: &str) -> Result<DeleteMatches, CommandError> {
    match text {
        "keep" => Ok(DeleteMatches::KeepAsNonLeague),
        "delete" => Ok(DeleteMatches::DeleteToo),
        _ => Err(CommandError::Refused(
            "Choose what happens to the matches.".to_string(),
        )),
    }
}

/// A time the UI sends back (Undo's `removed_at`). The TypeScript export
/// refuses `i64` arguments, so it arrives as an `f64`; a JavaScript number
/// holds whole milliseconds exactly below 2^53.
pub fn ms_from_ui(value: f64) -> Result<i64, CommandError> {
    const LIMIT: f64 = 9_007_199_254_740_992.0;
    if value.is_finite() && value.fract() == 0.0 && value.abs() < LIMIT {
        Ok(value as i64)
    } else {
        Err(CommandError::Refused("That time is not valid.".to_string()))
    }
}

/// The status Undo puts back: when the fold says Active but the player
/// said Complete or Partial, the delete records a drop (spec 4.4), and
/// Undo sets their status again.
pub fn status_before_delete(status: Option<LeagueStatus>) -> Option<String> {
    let status = status?;
    let user = status.user_state?;
    let dropped_by_delete = status.state == RunState::Active
        && (user == UserState::Complete || user == UserState::Partial);
    if dropped_by_delete {
        Some(user.as_str().to_string())
    } else {
        None
    }
}

/// "Set status" (spec 4.3): `None` is Automatic. Dropped stops the run
/// taking matches; it does not drop the player on MTGO.
pub fn set_status(
    state: &AppState,
    run_key: &str,
    status: Option<&str>,
    now_ms: i64,
) -> Result<(), CommandError> {
    let status = parse_status(status)?;
    send_edit(
        state,
        LeagueEdit::SetStatus {
            run_key: run_key.to_string(),
            status,
            now_ms,
        },
    )
}

/// "Delete league" (spec 4.4). Refused while the league reads Active.
pub fn delete_run(
    state: &AppState,
    run_key: &str,
    matches: &str,
    now_ms: i64,
) -> Result<DeleteLeagueView, CommandError> {
    let matches = parse_delete_matches(matches)?;
    let before = league_status(&lock(&state.reader), run_key).map_err(storage)?;
    send_edit(
        state,
        LeagueEdit::Delete {
            run_key: run_key.to_string(),
            matches,
            now_ms,
        },
    )?;
    // A done delete was of a listed league, so `before` holds its row.
    let Some(league_id) = before.map(|status| status.id) else {
        return Err(league_refusal(LeagueEditOutcome::NotFound));
    };
    Ok(DeleteLeagueView {
        league_id,
        removed_at: now_ms,
        previous_status: status_before_delete(before),
    })
}

/// The delete toast's Undo (spec 4.5): the league back, and the matches the
/// delete hid. It names the league by its row id, which a rekey since the
/// delete keeps (sync identity spec 2.5), and answers the run key the row
/// has now, which the UI puts a previous status back with (`set_status`).
pub fn undo_delete(
    state: &AppState,
    league_id: i64,
    removed_at: i64,
    now_ms: i64,
) -> Result<String, CommandError> {
    send_edit(
        state,
        LeagueEdit::UndoDelete {
            league_id,
            removed_at,
            now_ms,
        },
    )?;
    let run_key = store::read::league_run_key(&lock(&state.reader), league_id).map_err(storage)?;
    run_key.ok_or_else(|| league_refusal(LeagueEditOutcome::NotFound))
}

/// Restore a deleted league (Settings, Archives, Deleted) with the matches
/// deleted with it.
pub fn restore_run(state: &AppState, run_key: &str, now_ms: i64) -> Result<(), CommandError> {
    send_edit(
        state,
        LeagueEdit::Restore {
            run_key: run_key.to_string(),
            now_ms,
        },
    )
}

/// "Move to league..." (spec 4.1); `None` is No league (spec 4.2).
pub fn move_match(
    state: &AppState,
    match_token: &str,
    run_key: Option<&str>,
    now_ms: i64,
) -> Result<(), CommandError> {
    send_edit(
        state,
        LeagueEdit::MoveMatch {
            match_token: match_token.to_string(),
            to_run_key: run_key.map(str::to_string),
            now_ms,
        },
    )
}

// ---- Commands ----------------------------------------------------------------

#[tauri::command]
#[specta::specta]
pub fn list_leagues(
    query: LeagueListQuery,
    startup: State<'_, Startup>,
) -> Result<LeagueListView, CommandError> {
    let _timer = Timer::start("list_leagues");
    let state = ready(&startup)?;
    list(&lock(&state.reader), &query, state.timezone)
}

/// `timeframe`: the deck page's timeframe key.
#[tauri::command]
#[specta::specta]
pub fn deck_leagues(
    id: u32,
    version: Option<u32>,
    timeframe: String,
    startup: State<'_, Startup>,
) -> Result<DeckLeaguesView, CommandError> {
    let _timer = Timer::start("deck_leagues");
    let state = ready(&startup)?;
    deck_league_runs(
        &lock(&state.reader),
        i64::from(id),
        version.map(i64::from),
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

/// `timeframe`: the deck page's timeframe key.
#[tauri::command]
#[specta::specta]
pub fn deck_tournaments(
    id: u32,
    version: Option<u32>,
    timeframe: String,
    startup: State<'_, Startup>,
) -> Result<Vec<TournamentView>, CommandError> {
    let _timer = Timer::start("deck_tournaments");
    let state = ready(&startup)?;
    deck_tournament_runs(
        &lock(&state.reader),
        i64::from(id),
        version.map(i64::from),
        &timeframe,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

#[tauri::command]
#[specta::specta]
pub fn deck_league_panels(
    id: u32,
    version: Option<u32>,
    startup: State<'_, Startup>,
) -> Result<DeckLeaguePanelsView, CommandError> {
    let _timer = Timer::start("deck_league_panels");
    let state = ready(&startup)?;
    deck_panels(
        &lock(&state.reader),
        i64::from(id),
        version.map(i64::from),
        state.timezone,
    )
}

#[tauri::command]
#[specta::specta]
pub fn league_available_matches(
    run_key: String,
    startup: State<'_, Startup>,
) -> Result<Vec<AvailableMatchView>, CommandError> {
    let _timer = Timer::start("league_available_matches");
    available(&lock(&ready(&startup)?.reader), &run_key)
}

/// Where a match is now and the leagues "Move to league..." may move it
/// to (league self-management spec 4.1, 5.4).
#[tauri::command]
#[specta::specta]
pub fn match_league_targets(
    token: String,
    startup: State<'_, Startup>,
) -> Result<MatchLeagueTargetsView, CommandError> {
    let _timer = Timer::start("match_league_targets");
    targets(&lock(&ready(&startup)?.reader), &token)
}

/// `started_at`: a local date and time, `YYYY-MM-DDTHH:MM`. Async so the
/// wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn create_manual_league(
    app: AppHandle,
    deck_id: u32,
    name: String,
    started_at: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("create_manual_league");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        create_manual(
            ready(&startup)?,
            i64::from(deck_id),
            &name,
            &started_at,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Link complete matches to a manual league. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn link_league_match(
    app: AppHandle,
    run_key: String,
    match_tokens: Vec<String>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("link_league_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        link(
            ready(&startup)?,
            &run_key,
            &match_tokens,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn unlink_league_match(
    app: AppHandle,
    run_key: String,
    match_token: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("unlink_league_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        unlink(
            ready(&startup)?,
            &run_key,
            &match_token,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Blank notes clear the note. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn set_league_notes(
    app: AppHandle,
    run_key: String,
    notes: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_league_notes");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_notes(ready(&startup)?, &run_key, &notes, crate::wall_clock_ms())
    })
    .await
}

/// Set a league's status; `null` is Automatic. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn set_league_status(
    app: AppHandle,
    run_key: String,
    status: Option<String>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_league_status");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_status(
            ready(&startup)?,
            &run_key,
            status.as_deref(),
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Delete a league that is not Active. `matches`: `keep` (as non-league
/// matches) or `delete`. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn delete_league(
    app: AppHandle,
    run_key: String,
    matches: String,
) -> Result<DeleteLeagueView, CommandError> {
    let _timer = Timer::start("delete_league");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        delete_run(ready(&startup)?, &run_key, &matches, crate::wall_clock_ms())
    })
    .await
}

/// Undo a delete: `league_id` and `removed_at` are the `leagueId` and
/// `removedAt` the delete answered. Answers the league's run key now (a
/// rekey since the delete changes it). Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn undo_delete_league(
    app: AppHandle,
    league_id: u32,
    removed_at: f64,
) -> Result<String, CommandError> {
    let _timer = Timer::start("undo_delete_league");
    let removed_at = ms_from_ui(removed_at)?;
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        undo_delete(
            ready(&startup)?,
            i64::from(league_id),
            removed_at,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// Restore a deleted league. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn restore_league(app: AppHandle, run_key: String) -> Result<(), CommandError> {
    let _timer = Timer::start("restore_league");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        restore_run(ready(&startup)?, &run_key, crate::wall_clock_ms())
    })
    .await
}

/// Move a finished match into a league, or (`run_key` `null`) out of every
/// league. Async, as above.
#[tauri::command]
#[specta::specta]
pub async fn move_match_to_league(
    app: AppHandle,
    match_token: String,
    run_key: Option<String>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("move_match_to_league");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        move_match(
            ready(&startup)?,
            &match_token,
            run_key.as_deref(),
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// The league screenshot's data (0.x `Leagues/ScreenshotDataController`):
/// the run deck's cover art, read only when a screenshot is taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueScreenshotView {
    /// The cover's art crop as a `data:` URL, `None` without a cover.
    pub cover_art_base64: Option<String>,
}

/// The cover art sources of the run card's deck (`deck_id`, the deck the
/// card shows); no deck or no cover gives `None`.
pub fn league_cover(
    conn: &Connection,
    deck_id: Option<i64>,
) -> Result<Option<store::cards::ImageSources>, CommandError> {
    match deck_id {
        Some(deck_id) => crate::decks::screenshot::cover_sources(conn, deck_id),
        None => Ok(None),
    }
}

/// The file name of the deck's own cover image, when it has one (it shows
/// before the cover card).
pub fn league_custom_cover(
    conn: &Connection,
    deck_id: Option<i64>,
) -> Result<Option<String>, CommandError> {
    let Some(deck_id) = deck_id else {
        return Ok(None);
    };
    let deck = store::decks::deck_by_id(conn, deck_id).map_err(crate::commands::storage)?;
    Ok(deck.and_then(|deck| deck.cover_file))
}

/// The screenshot data, the cover read through `image`.
pub fn league_screenshot_view(
    cover: Option<store::cards::ImageSources>,
    image: &dyn Fn(&store::cards::ImageSources) -> Option<String>,
) -> LeagueScreenshotView {
    LeagueScreenshotView {
        cover_art_base64: cover.as_ref().and_then(image),
    }
}

/// "Copy screenshot of league": the cover as a `data:` URL. Off the IPC
/// thread (a remote image waits up to 5 s), and the reader is let go before
/// the image is read.
#[tauri::command]
#[specta::specta]
pub async fn league_screenshot_data(
    app: AppHandle,
    deck_id: Option<u32>,
) -> Result<LeagueScreenshotView, CommandError> {
    let _timer = Timer::start("league_screenshot_data");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let cover = league_cover(&lock(&state.reader), deck_id.map(i64::from))?;
        let custom = league_custom_cover(&lock(&state.reader), deck_id.map(i64::from))?;
        let covers = crate::decks::cover::folder(&state.data_dir);
        if let Some(url) = custom.and_then(|name| crate::decks::cover::data_url(&covers, &name)) {
            return Ok(LeagueScreenshotView {
                cover_art_base64: Some(url),
            });
        }
        let folder = crate::cards::data_url::current_folder(&app);
        let fetcher = crate::cards::images::Fetcher::standard();
        Ok(league_screenshot_view(cover, &|sources| {
            crate::cards::data_url::image_data_url(sources, folder.as_deref(), &fetcher)
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use std::time::{Duration, Instant};
    use store::league_sync::ImportedSeed;
    use store::params;
    use tracker::TrackerEvent;

    // 0.x Leagues/ScreenshotDataTest: "returns the league deck cover art as
    // a data url from local storage" and "returns a null cover when the
    // league deck has no cover art".
    #[test]
    fn the_league_screenshot_is_the_run_decks_cover_art() {
        let (_dir, conn, (deck_id, _, _)) = crate::decks::tests::database("leagues-screenshot");
        let read = |sources: &store::cards::ImageSources| {
            sources
                .local_image
                .as_ref()
                .map(|name| format!("data:image/jpeg;base64,{name}"))
        };
        let view =
            league_screenshot_view(league_cover(&conn, Some(deck_id)).expect("cover"), &read);
        assert_eq!(
            view.cover_art_base64.as_deref(),
            Some("data:image/jpeg;base64,1001_art.jpg")
        );
        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(
            json["coverArtBase64"],
            "data:image/jpeg;base64,1001_art.jpg"
        );

        assert_eq!(
            league_screenshot_view(league_cover(&conn, None).expect("cover"), &read)
                .cover_art_base64,
            None
        );
        conn.execute_batch("UPDATE decks SET cover_catalog_id = NULL")
            .expect("no cover");
        let view =
            league_screenshot_view(league_cover(&conn, Some(deck_id)).expect("cover"), &read);
        assert_eq!(view.cover_art_base64, None);
        assert_eq!(
            serde_json::to_value(&view).expect("json")["coverArtBase64"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn the_league_screenshot_finds_the_run_decks_own_cover_file() {
        let (_dir, conn, (deck_id, _, _)) = crate::decks::tests::database("leagues-custom-cover");
        assert_eq!(league_custom_cover(&conn, Some(deck_id)), Ok(None));
        conn.execute_batch("UPDATE decks SET cover_file = '1-5.png'")
            .expect("custom cover");
        assert_eq!(
            league_custom_cover(&conn, Some(deck_id)),
            Ok(Some("1-5.png".to_string()))
        );
        assert_eq!(league_custom_cover(&conn, None), Ok(None));
        assert_eq!(league_custom_cover(&conn, Some(999)), Ok(None));
    }

    /// 2026-07-01T10:00:00Z, a Wednesday morning.
    const T0: i64 = 1_782_900_000_000;
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const UTC: Tz = chrono_tz::UTC;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        (dir, conn)
    }

    /// The league sync the tracker runs after every change, for every owner.
    /// These tests store matches and league rows directly, so each read
    /// below syncs first; a read-only connection reads what the tracker
    /// stored.
    fn sync(conn: &Connection) {
        if !conn.is_readonly("main").expect("read-only") {
            crate::commands::tests::sync_leagues(conn);
        }
    }

    fn list(
        conn: &Connection,
        query: &LeagueListQuery,
        tz: Tz,
    ) -> Result<LeagueListView, CommandError> {
        sync(conn);
        super::list(conn, query, tz)
    }

    fn deck_league_runs(
        conn: &Connection,
        deck_id: i64,
        version: Option<i64>,
        timeframe: &str,
        now_ms: i64,
        tz: Tz,
    ) -> Result<DeckLeaguesView, CommandError> {
        sync(conn);
        super::deck_league_runs(conn, deck_id, version, timeframe, now_ms, tz)
    }

    fn deck_tournament_runs(
        conn: &Connection,
        deck_id: i64,
        version: Option<i64>,
        timeframe: &str,
        now_ms: i64,
        tz: Tz,
    ) -> Result<Vec<TournamentView>, CommandError> {
        sync(conn);
        super::deck_tournament_runs(conn, deck_id, version, timeframe, now_ms, tz)
    }

    fn deck_panels(
        conn: &Connection,
        deck_id: i64,
        version: Option<i64>,
        tz: Tz,
    ) -> Result<DeckLeaguePanelsView, CommandError> {
        sync(conn);
        super::deck_panels(conn, deck_id, version, tz)
    }

    fn deck_trophies(conn: &Connection, deck_id: i64) -> Result<u32, CommandError> {
        sync(conn);
        super::deck_trophies(conn, deck_id)
    }

    fn available(
        conn: &Connection,
        run_key: &str,
    ) -> Result<Vec<AvailableMatchView>, CommandError> {
        sync(conn);
        super::available(conn, run_key)
    }

    fn targets(conn: &Connection, token: &str) -> Result<MatchLeagueTargetsView, CommandError> {
        sync(conn);
        super::targets(conn, token)
    }

    /// A manual league row `manual:{id}`, as the league edit makes one: the
    /// deck's format and the version it had at `created_at`.
    fn manual_league(
        conn: &Connection,
        id: i64,
        account_id: Option<i64>,
        deck_id: Option<i64>,
        name: &str,
        started_at: i64,
        created_at: i64,
    ) {
        conn.execute(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, name, format,
                 deck_version_id, deck_id, started_at, created_at, token)
             VALUES (?1, 'manual:' || ?1, ?2, 'manual', 'constructed', 'complete', ?3,
                 (SELECT format FROM decks WHERE id = ?4),
                 COALESCE(
                     (SELECT id FROM deck_versions WHERE deck_id = ?4 AND modified_at <= ?6
                      ORDER BY modified_at DESC, id DESC LIMIT 1),
                     (SELECT id FROM deck_versions WHERE deck_id = ?4
                      ORDER BY modified_at, id LIMIT 1)),
                 ?4, ?5, ?6, 'manual_Q' || ?1)",
            params![id, account_id, name, deck_id, started_at, created_at],
        )
        .expect("manual league");
    }

    /// A 0.x run `0x:{id}` as the import stores it (row `1000 + id`).
    fn imported_run(conn: &Connection, id: i64, account_id: i64, seed: &ImportedSeed) {
        conn.execute(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, event_id,
                 name, format, deck_version_id, deck_id, started_at, joined_at, completed_at,
                 dropped_at, created_at, imported_from, imported_seed)
             VALUES (1000 + ?1, '0x:' || ?1, ?2, 'imported', ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                 (SELECT deck_id FROM deck_versions WHERE id = ?9), ?10, ?11, ?12, ?13, 7, ?1,
                 ?14)",
            params![
                id,
                account_id,
                seed.kind,
                seed.state,
                seed.token,
                seed.event_id,
                seed.name,
                seed.format,
                seed.deck_version_id,
                seed.started_at,
                seed.joined_at,
                seed.completed_at,
                seed.dropped_at,
                serde_json::to_string(seed).expect("seed")
            ],
        )
        .expect("imported run");
    }

    /// A constructed 0.x seed.
    fn seed_of(token: &str, name: &str, state: &str, started_at: i64) -> ImportedSeed {
        ImportedSeed {
            token: token.to_string(),
            event_id: Some(9),
            name: name.to_string(),
            format: Some("CMODERN".to_string()),
            state: state.to_string(),
            started_at,
            joined_at: None,
            completed_at: None,
            dropped_at: None,
            deck_version_id: Some(10),
            kind: "constructed".to_string(),
            set_code: None,
            mtgo_course_id: None,
            legacy_id: None,
        }
    }

    /// Link a match to league row `league_id` (a manual league's id; an
    /// imported run's is `1000 + its 0.x id`).
    fn member(conn: &Connection, league_id: i64, token: &str) {
        conn.execute(
            "UPDATE matches SET league_id = ?2, league_linked = 1 WHERE token = ?1",
            params![token, league_id],
        )
        .expect("member");
    }

    fn account(conn: &Connection, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0)
             ON CONFLICT (username) DO NOTHING",
            [name],
        )
        .expect("account");
        conn.query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [name],
            |row| row.get(0),
        )
        .expect("account id")
    }

    fn deck(conn: &Connection, id: i64, account_id: Option<i64>, name: &str, archived: bool) {
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, account_id, name, format, color_identity,
                 archived_at, created_at, updated_at, source)
             VALUES (?1, 'Q-deck-' || ?1, ?2, ?3, 'CMODERN', 'U,R', ?4, 1, 1, 'file')",
            params![id, account_id, name, archived.then_some(5_i64)],
        )
        .expect("deck");
    }

    fn version(conn: &Connection, id: i64, deck_id: i64, modified_at: i64) {
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (?1, ?2, 'Q-sig-' || ?1, ?3)",
            params![id, deck_id, modified_at],
        )
        .expect("version");
    }

    /// A stored match, built with `played` and changed field by field.
    #[derive(Clone)]
    struct Played {
        token: String,
        account: i64,
        source: &'static str,
        league: Option<String>,
        format: &'static str,
        tournament: Option<(i64, i64)>,
        state: &'static str,
        outcome: &'static str,
        started_at: i64,
        version: Option<i64>,
        opponent: &'static str,
    }

    /// A complete log match of `league` lasting 20 minutes, won or lost.
    fn played(account: i64, token: &str, league: Option<&str>, at: i64, outcome: &str) -> Played {
        Played {
            token: token.to_string(),
            account,
            source: "log",
            league: league.map(str::to_string),
            format: "CMODERN",
            tournament: None,
            state: "complete",
            outcome: if outcome == "win" { "win" } else { "loss" },
            started_at: at,
            version: None,
            opponent: "Qopponent",
        }
    }

    /// The match, its games (two won on the play then the draw for a win,
    /// two lost for a loss, one won for a match in progress) and its deck
    /// link.
    fn insert(conn: &Connection, m: &Played) {
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at) VALUES (?1, 0)
             ON CONFLICT (username) DO NOTHING",
            [m.opponent],
        )
        .expect("opponent");
        let complete = m.state == "complete";
        let ended_at = complete.then_some(m.started_at + 20 * MINUTE);
        let outcome = if complete { m.outcome } else { "unknown" };
        let id: i64 = conn
            .query_row(
                "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                     tournament_event_id, tournament_round, state, outcome, started_at, ended_at,
                     last_activity_at, revision, derived_version, league_token, game_structure)
                 VALUES (?1, ?2, (SELECT id FROM opponents WHERE username = ?3), ?4, ?5,
                     'Constructed', ?6, ?7, ?8, ?9, ?10, ?11, ?10, 1, 4, ?12, 'Modern')
                 RETURNING id",
                params![
                    m.token,
                    m.account,
                    m.opponent,
                    m.source,
                    m.format,
                    m.tournament.map(|(event, _)| event),
                    m.tournament.map(|(_, round)| round),
                    m.state,
                    outcome,
                    m.started_at,
                    ended_at,
                    m.league,
                ],
                |row| row.get(0),
            )
            .expect("match");
        let games: Vec<&str> = if !complete {
            vec!["win"]
        } else if m.outcome == "win" {
            vec!["win", "win"]
        } else {
            vec!["loss", "loss"]
        };
        for (index, result) in games.iter().enumerate() {
            let number = index as i64 + 1;
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                     started_at, ended_at)
                 VALUES (?1, ?2, ?3, 'win', ?4, ?5, ?6)",
                params![
                    id,
                    number,
                    result,
                    number == 1,
                    m.started_at + number * MINUTE,
                    m.started_at + number * 5 * MINUTE
                ],
            )
            .expect("game");
        }
        if let Some(version) = m.version {
            conn.execute(
                "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
                params![m.token, version],
            )
            .expect("link");
        }
    }

    /// A run of `outcomes` on `league`, one match an hour from `at`, linked
    /// to `version`. Returns the match tokens.
    fn run_of(
        conn: &Connection,
        account: i64,
        league: &str,
        at: i64,
        outcomes: &[&str],
        version: i64,
        format: &'static str,
    ) -> Vec<String> {
        let mut tokens = Vec::new();
        for (index, outcome) in outcomes.iter().enumerate() {
            let token = format!("{league}-m{index}");
            insert(
                conn,
                &Played {
                    version: Some(version),
                    format,
                    ..played(
                        account,
                        &token,
                        Some(league),
                        at + index as i64 * HOUR,
                        outcome,
                    )
                },
            );
            tokens.push(token);
        }
        tokens
    }

    struct Seeded {
        local: i64,
        /// Run A: 4-1 on deck 1 (v2), CASH.
        a: String,
        /// Run B: 5-0 Pioneer on deck 2, TROPHY.
        b: String,
        /// Run C: 1-1 on deck 1 and still going, LIVE.
        c: String,
    }

    /// One account with decks 1 (Qburn, versions 10 and 11), 2 (Qcontrol,
    /// version 20) and 3 (archived), runs A, B, C, an Active run D with only a
    /// match in progress, an empty manual league of deck 1, and an imported
    /// Active run of deck 1 (version 10) the player dropped by hand.
    fn seed(conn: &Connection) -> Seeded {
        let local = account(conn, "Qlocal");
        deck(conn, 1, Some(local), "Qburn", false);
        deck(conn, 2, Some(local), "Qcontrol", false);
        deck(conn, 3, Some(local), "Qarchived", true);
        version(conn, 10, 1, T0 - 30 * DAY);
        version(conn, 11, 1, T0 - 10 * DAY);
        version(conn, 20, 2, T0 - 10 * DAY);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, name, art_crop, local_art_crop)
                 VALUES (77, 'Qcard', 'https://q/77.jpg', '77_art.jpg');
             UPDATE decks SET cover_catalog_id = 77 WHERE id = 1;",
        )
        .expect("cover");

        let a = run_of(
            conn,
            local,
            "Q-league-a",
            T0,
            &["win", "win", "loss", "win", "win"],
            11,
            "CMODERN",
        );
        conn.execute(
            "UPDATE matches SET opponent_id = (SELECT id FROM opponents WHERE username = 'Qopponent')",
            [],
        )
        .expect("opponents");
        conn.execute_batch(&format!(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qzed', 0);
             UPDATE matches SET opponent_id = (SELECT id FROM opponents WHERE username = 'Qzed')
                 WHERE token = '{}';
             UPDATE matches SET notes = 'Q match note' WHERE token = '{}';",
            a[2], a[0]
        ))
        .expect("notes");
        let b = run_of(
            conn,
            local,
            "Q-league-b",
            T0 + DAY,
            &["win", "win", "win", "win", "win"],
            20,
            "CPIONEER",
        );
        let c = run_of(
            conn,
            local,
            "Q-league-c",
            T0 + 2 * DAY,
            &["win", "loss"],
            11,
            "CMODERN",
        );
        insert(
            conn,
            &Played {
                state: "in_progress",
                version: Some(11),
                ..played(
                    local,
                    "Q-league-d-m0",
                    Some("Q-league-d"),
                    T0 + 3 * DAY,
                    "win",
                )
            },
        );

        manual_league(conn, 1, Some(local), Some(1), "Qmanual", T0 - DAY, T0);
        imported_run(
            conn,
            42,
            local,
            &seed_of("Q-0x-token", "Qimported", "active", T0 - 2 * DAY),
        );
        insert(
            conn,
            &Played {
                source: "imported",
                version: Some(10),
                ..played(local, "Q-imported-m0", None, T0 - 2 * DAY, "win")
            },
        );
        member(conn, 1042, "Q-imported-m0");
        conn.execute(
            "UPDATE leagues SET user_dropped_at = ?1, user_state = 'dropped' WHERE run_key = '0x:42'",
            [T0 - DAY],
        )
        .expect("the imported run's drop");
        // The run note goes on run A's row, once the sync made it.
        sync(conn);
        conn.execute(
            "UPDATE leagues SET notes = 'Q run note' WHERE run_key = ?1",
            [format!("Q-league-a:{}", a[0])],
        )
        .expect("run note");

        Seeded {
            local,
            a: format!("Q-league-a:{}", a[0]),
            b: format!("Q-league-b:{}", b[0]),
            c: format!("Q-league-c:{}", c[0]),
        }
    }

    /// The Arena context with an Arena account that has no leagues.
    fn switch_to_arena(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id');
             INSERT INTO settings (key, value) VALUES ('platform_context', 'arena');",
        )
        .expect("arena context");
    }

    // MTGO-only machinery reads the MTGO account, whatever the context.
    #[test]
    fn machinery_reads_mtgo_in_the_arena_context() {
        let (_dir, conn) = db("leagues-machinery-arena");
        let seeded = seed(&conn);
        switch_to_arena(&conn);
        let scope = crate::platform::page_scope(&conn).expect("scope");
        assert_eq!(scope.platform, store::Platform::Arena);
        assert_ne!(scope.account, Some(seeded.local), "the page scope is Arena");

        let page = list(&conn, &query(), UTC).expect("leagues");
        assert!(
            keys(&page.runs).contains(&seeded.a),
            "the MTGO run is listed"
        );

        let card = crate::overlay::league::league_card(
            &conn,
            &crate::settings::Settings::default(),
            None,
            T0 + 3 * DAY + HOUR,
            None,
            &mut crate::overlay::game_net::GameNet::default(),
        )
        .expect("card");
        assert!(
            card.live_tokens.contains("Q-league-d-m0"),
            "the overlay card reads the MTGO run in progress: {:?}",
            card.live_tokens
        );
    }

    fn query() -> LeagueListQuery {
        LeagueListQuery {
            format: None,
            state: LeagueStateChip::All,
            deck_id: None,
            archetype_id: None,
            search: None,
            sort: LeagueSort::Newest,
            page: 1,
        }
    }

    fn keys(runs: &[RunCardView]) -> Vec<String> {
        runs.iter().map(|run| run.key.clone()).collect()
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|text| text.to_string()).collect()
    }

    fn refused(message: &str) -> CommandError {
        CommandError::Refused(message.to_string())
    }

    /// The row id of the listed league `run_key`.
    fn row_id(state: &AppState, run_key: &str) -> i64 {
        store::leagues::league_status(&lock(&state.reader), run_key)
            .expect("league status")
            .expect("a listed league")
            .id
    }

    // Mapping

    #[test]
    fn a_derived_run_card_has_0x_fields() {
        let (_dir, conn) = db("leagues-card");
        let seeded = seed(&conn);
        let page = list(&conn, &query(), UTC).expect("list");
        let card = page
            .runs
            .iter()
            .find(|run| run.key == seeded.a)
            .expect("run A");
        let deck = RunDeckView {
            id: 1,
            name: "Qburn".to_string(),
            colors: strings(&["U", "R"]),
            cover_art: Some("https://q/77.jpg".to_string()),
            local_cover_art: Some("77_art.jpg".to_string()),
        };
        assert_eq!(card.source, RunSource::Log);
        assert_eq!(card.name, "Modern League 01-07-2026 10:00am");
        assert_eq!(
            (card.format.as_str(), card.format_code.as_deref()),
            ("Modern", Some("CMODERN"))
        );
        assert_eq!(card.state, RunStateView::Complete);
        assert!(!card.manual);
        assert_eq!((card.started_at_ms, card.dropped_at_ms), (T0, None));
        assert_eq!(card.deck, Some(deck));
        assert_eq!(card.version_label.as_deref(), Some("v2"));
        let (w, l) = (Some(MarkView::W), Some(MarkView::L));
        assert_eq!(card.results, vec![w, w, l, w, w]);
        assert_eq!(card.matches.len(), 5);
        let first = &card.matches[0];
        assert_eq!(first.result, MarkView::W);
        assert_eq!(first.opponent_name.as_deref(), Some("Qopponent"));
        assert_eq!(first.notes.as_deref(), Some("Q match note"));
        assert_eq!(
            first.games,
            vec![
                GameMarkView {
                    result: MarkView::W,
                    on_play: Some(true)
                },
                GameMarkView {
                    result: MarkView::W,
                    on_play: Some(false)
                },
            ]
        );
        assert_eq!(
            (first.started_at_ms, first.duration_seconds),
            (T0, Some(1200))
        );
        assert_eq!(card.matches[2].opponent_name.as_deref(), Some("Qzed"));
        assert_eq!(card.matches[2].notes, None);
        assert_eq!(card.notes.as_deref(), Some("Q run note"));
        assert_eq!(card.classification, ClassificationView::Cash);
        assert_eq!(card.live_round, None);
        assert_eq!(card.avg_match_seconds, Some(1200));
        // Started at 10, 11, 12, 13 and 14 o'clock.
        assert_eq!(card.time_of_day, Some(TimeOfDayView::Afternoon));
        assert_eq!((card.game_wins, card.game_losses), (8, 2));
        assert_eq!(card.on_play_record, WinLossView { wins: 4, losses: 1 });
        assert_eq!(card.on_draw_record, WinLossView { wins: 4, losses: 1 });
        assert_eq!(card.tix_delta, Some(12.70));

        let live = page
            .runs
            .iter()
            .find(|run| run.key == seeded.c)
            .expect("run C");
        assert_eq!(live.classification, ClassificationView::Live);
        assert_eq!(live.live_round, Some(3));
        assert_eq!(live.results, vec![w, l, None, None, None]);
        assert_eq!(live.tix_delta, None);
        assert_eq!(live.notes, None);
    }

    #[test]
    fn manual_and_imported_runs_have_cards_and_a_manual_drop_drops_an_imported_run() {
        let (_dir, conn) = db("leagues-card-manual-imported");
        seed(&conn);
        let page = list(&conn, &query(), UTC).expect("list");
        let manual = page
            .runs
            .iter()
            .find(|run| run.key == "manual:1")
            .expect("manual");
        assert_eq!(manual.source, RunSource::Manual);
        assert!(manual.manual);
        assert_eq!(manual.name, "Qmanual");
        assert_eq!(manual.state, RunStateView::Complete);
        assert_eq!(manual.classification, ClassificationView::Empty);
        assert!(manual.results.is_empty());
        assert_eq!(manual.format_code.as_deref(), Some("CMODERN"));
        assert_eq!(manual.deck.as_ref().map(|deck| deck.id), Some(1));
        assert_eq!(
            manual.version_label.as_deref(),
            Some("v2"),
            "the deck's version when the league was created"
        );

        let imported = page
            .runs
            .iter()
            .find(|run| run.key == "0x:42")
            .expect("imported");
        assert_eq!(imported.source, RunSource::Imported);
        assert_eq!(imported.name, "Qimported");
        assert_eq!(imported.state, RunStateView::Dropped, "dropped by hand");
        assert_eq!(imported.dropped_at_ms, Some(T0 - DAY));
        assert_eq!(imported.classification, ClassificationView::Brick);
        assert_eq!(imported.version_label.as_deref(), Some("v1"));
        assert_eq!(imported.results, vec![Some(MarkView::W)]);
        assert_eq!(
            imported.tix_delta,
            Some(-10.0),
            "a dropped 1-0 is scored 1-4"
        );
    }

    /// A manual match is marked in a run's rows (0.x `ManualMatchBadge`),
    /// in a derived run and in a manual league alike.
    #[test]
    fn a_manual_match_is_marked_manual_in_its_runs_rows() {
        let (_dir, conn) = db("leagues-card-manual-match");
        let seeded = seed(&conn);
        insert(
            &conn,
            &Played {
                source: "manual",
                version: Some(11),
                ..played(
                    seeded.local,
                    "Q-manual-c",
                    Some("Q-league-c"),
                    T0 + 2 * DAY + 2 * HOUR,
                    "win",
                )
            },
        );
        insert(
            &conn,
            &Played {
                source: "manual",
                version: Some(11),
                ..played(seeded.local, "Q-manual-l", None, T0 - DAY + HOUR, "loss")
            },
        );
        member(&conn, 1, "Q-manual-l");
        let page = list(&conn, &query(), UTC).expect("list");
        let marks = |key: &str| -> Vec<(String, bool)> {
            page.runs
                .iter()
                .find(|run| run.key == key)
                .expect("run")
                .matches
                .iter()
                .map(|row| (row.token.clone(), row.manual))
                .collect()
        };
        assert_eq!(
            marks(&seeded.c),
            vec![
                ("Q-league-c-m0".to_string(), false),
                ("Q-league-c-m1".to_string(), false),
                ("Q-manual-c".to_string(), true),
            ]
        );
        assert_eq!(marks("manual:1"), vec![("Q-manual-l".to_string(), true)]);
    }

    /// A drop time the fold ignores (no `user_state`, as the migration
    /// leaves a Complete run holding one: spec 10) never drops the run. A
    /// player's Dropped status shows over the fold's (spec 4.3).
    #[test]
    fn a_manual_drop_never_drops_a_complete_imported_run() {
        let (_dir, conn) = db("leagues-imported-complete-drop");
        seed(&conn);
        conn.execute(
            "UPDATE leagues SET state = 'complete', user_state = NULL,
                 imported_seed = json_set(imported_seed, '$.state', 'complete')
             WHERE imported_from = 42",
            [],
        )
        .expect("complete");
        let page = list(&conn, &query(), UTC).expect("list");
        let imported = page
            .runs
            .iter()
            .find(|run| run.key == "0x:42")
            .expect("imported");
        assert_eq!(imported.state, RunStateView::Complete);
        assert_eq!(imported.dropped_at_ms, None);
    }

    // Runs that span 0.x and v1 (final review Important 1): the owner runs
    // both apps side by side, so a 0.x run's later matches are often live in
    // v1. It shows once, with 0.x's record, and continues in v1.

    const SPAN: &str = "Q-span";

    /// Qlocal with deck 1 (Qburn, version 10).
    fn span_db(label: &str) -> (TestDir, Connection, i64) {
        let (dir, conn) = db(label);
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), "Qburn", false);
        version(&conn, 10, 1, T0 - 30 * DAY);
        (dir, conn, local)
    }

    /// A 0.x run on `SPAN` with version 10, started at T0, as the import
    /// stores it.
    fn imported_span(conn: &Connection, id: i64, account: i64, state: &str) {
        imported_run(
            conn,
            id,
            account,
            &ImportedSeed {
                completed_at: (state == "complete").then_some(T0 + 5 * HOUR),
                ..seed_of(SPAN, "Qspan", state, T0)
            },
        );
    }

    /// Matches an hour apart from `at` on version 10: imported copies (no
    /// league token) or live ones on `SPAN`. Members of run `member_of`.
    fn span_matches(
        conn: &Connection,
        account: i64,
        prefix: &str,
        source: &'static str,
        at: i64,
        outcomes: &[&str],
        member_of: Option<i64>,
    ) {
        for (index, outcome) in outcomes.iter().enumerate() {
            let token = format!("{prefix}{index}");
            let league = (source == "log").then_some(SPAN);
            insert(
                conn,
                &Played {
                    source,
                    version: Some(10),
                    ..played(account, &token, league, at + index as i64 * HOUR, outcome)
                },
            );
            if let Some(run_id) = member_of {
                member(conn, 1000 + run_id, &token);
            }
        }
    }

    #[test]
    fn a_0x_run_finished_live_in_v1_is_one_complete_4_1_run_with_its_tix() {
        let (_dir, conn, local) = span_db("leagues-span-finished");
        imported_span(&conn, 31, local, "complete");
        let outcomes = ["win", "win", "loss"];
        span_matches(&conn, local, "Q-0x-", "imported", T0, &outcomes, Some(31));
        // The last two were tracked live, and the import claimed them by token.
        let live = ["win", "win"];
        span_matches(&conn, local, "Q-v1-", "log", T0 + 3 * HOUR, &live, Some(31));

        let page = list(&conn, &query(), UTC).expect("list");

        assert_eq!(keys(&page.runs), strings(&["0x:31"]), "one run, not two");
        let run = &page.runs[0];
        assert_eq!(run.source, RunSource::Imported);
        assert_eq!(run.name, "Qspan");
        assert_eq!(run.state, RunStateView::Complete);
        let (w, l) = (Some(MarkView::W), Some(MarkView::L));
        assert_eq!(run.results, vec![w, w, l, w, w]);
        assert_eq!(run.classification, ClassificationView::Cash);
        assert_eq!(run.tix_delta, Some(12.70));
        assert_eq!(page.kpis.live, 0);
    }

    #[test]
    fn a_0x_run_active_at_import_continues_in_v1_and_keeps_its_key() {
        let (_dir, conn, local) = span_db("leagues-span-active");
        imported_span(&conn, 32, local, "active");
        span_matches(
            &conn,
            local,
            "Q-0x-",
            "imported",
            T0,
            &["win", "win"],
            Some(32),
        );
        conn.execute(
            "UPDATE leagues SET notes = 'Q span' WHERE run_key = '0x:32'",
            [],
        )
        .expect("note");
        // Played in v1 after the import: not members, they join on the token.
        let live = ["win", "loss", "win"];
        span_matches(&conn, local, "Q-v1-", "log", T0 + 2 * HOUR, &live, None);

        let page = list(&conn, &query(), UTC).expect("list");

        assert_eq!(keys(&page.runs), strings(&["0x:32"]));
        let run = &page.runs[0];
        assert_eq!(run.state, RunStateView::Complete);
        assert_eq!(run.matches.len(), 5);
        assert_eq!(run.classification, ClassificationView::Cash);
        assert_eq!(run.tix_delta, Some(12.70));
        assert_eq!(run.notes.as_deref(), Some("Q span"));

        // Re-entry on the same token with the same deck: a new run, and no
        // Active run is left behind.
        let again = ["win", "win", "win", "win", "win"];
        span_matches(&conn, local, "Q-again-", "log", T0 + DAY, &again, None);
        let page = list(&conn, &query(), UTC).expect("list");
        assert_eq!(
            keys(&page.runs),
            vec![format!("{SPAN}:Q-again-0"), "0x:32".to_string()]
        );
        assert!(
            page.runs
                .iter()
                .all(|run| run.state == RunStateView::Complete),
            "no Active run is left behind"
        );
        assert_eq!(page.kpis.live, 0);
    }

    #[test]
    fn a_complete_0x_run_and_a_new_live_run_on_its_token_are_two_runs() {
        let (_dir, conn, local) = span_db("leagues-span-two");
        imported_span(&conn, 33, local, "complete");
        let outcomes = ["win", "win", "win", "loss", "loss"];
        span_matches(&conn, local, "Q-0x-", "imported", T0, &outcomes, Some(33));
        span_matches(&conn, local, "Q-v1-", "log", T0 + DAY, &["win"], None);

        let page = list(&conn, &query(), UTC).expect("list");

        assert_eq!(
            keys(&page.runs),
            vec![format!("{SPAN}:Q-v1-0"), "0x:33".to_string()]
        );
        assert_eq!(page.runs[0].state, RunStateView::Active);
        assert_eq!(page.runs[0].classification, ClassificationView::Live);
        assert_eq!(page.runs[1].state, RunStateView::Complete);
        assert_eq!(page.runs[1].classification, ClassificationView::Finish);
    }

    // The Leagues page

    #[test]
    fn the_page_lists_manual_runs_and_runs_with_a_complete_match_with_kpis() {
        let (_dir, conn) = db("leagues-base");
        let seeded = seed(&conn);
        let page = list(&conn, &query(), UTC).expect("list");
        assert_eq!(
            keys(&page.runs),
            vec![
                seeded.c.clone(),
                seeded.b.clone(),
                seeded.a.clone(),
                "manual:1".to_string(),
                "0x:42".to_string()
            ],
            "newest first; run D has no complete match"
        );
        assert_eq!(
            (page.total, page.page, page.per_page, page.last_page),
            (5, 1, 20, 1)
        );
        assert_eq!(
            page.kpis,
            LeagueKpisView {
                total: 5,
                completed: 4,
                live: 1,
                decks: 2,
                trophies: 1,
                trophy_rate: Some(25.0),
                cash_rate: Some(50.0),
                dropped: 1,
                drop_rate: Some(25.0),
                avg_finish: Some(2.5),
                top_matchup: None,
            }
        );
        assert_eq!(
            page.format_options,
            vec![
                LeagueFormatOption {
                    code: "CMODERN".to_string(),
                    label: "Modern".to_string()
                },
                LeagueFormatOption {
                    code: "CPIONEER".to_string(),
                    label: "Pioneer".to_string()
                },
            ]
        );
        assert_eq!(
            page.deck_options,
            vec![
                LeagueDeckOption {
                    id: 1,
                    name: "Qburn".to_string()
                },
                LeagueDeckOption {
                    id: 2,
                    name: "Qcontrol".to_string()
                },
            ]
        );
        let manual_decks: Vec<(i64, &str, &str, &str)> = page
            .manual_deck_options
            .iter()
            .map(|deck| {
                (
                    deck.id,
                    deck.name.as_str(),
                    deck.format.as_str(),
                    deck.format_code.as_str(),
                )
            })
            .collect();
        assert_eq!(
            manual_decks,
            vec![
                (1, "Qburn", "Modern", "CMODERN"),
                (2, "Qcontrol", "Modern", "CMODERN")
            ],
            "not the archived deck"
        );
        let _ = seeded.local;
    }

    #[test]
    fn filters_narrow_the_runs_and_the_kpis() {
        let (_dir, conn) = db("leagues-filters");
        let seeded = seed(&conn);
        let filtered = |changed: LeagueListQuery| -> (Vec<String>, u32) {
            let page = list(&conn, &changed, UTC).expect("list");
            (keys(&page.runs), page.kpis.total)
        };
        let (a, b, c) = (seeded.a.clone(), seeded.b.clone(), seeded.c.clone());
        let manual = "manual:1".to_string();
        let imported = "0x:42".to_string();

        for (chip, wanted) in [
            (LeagueStateChip::Live, vec![c.clone()]),
            (LeagueStateChip::Trophies, vec![b.clone()]),
            (LeagueStateChip::Cash, vec![b.clone(), a.clone()]),
            (LeagueStateChip::Finish, vec![manual.clone()]),
            (LeagueStateChip::Bricks, vec![imported.clone()]),
        ] {
            let count = wanted.len() as u32;
            assert_eq!(
                filtered(LeagueListQuery {
                    state: chip,
                    ..query()
                }),
                (wanted, count),
                "{chip:?}"
            );
        }
        assert_eq!(
            filtered(LeagueListQuery {
                format: Some("CPIONEER".to_string()),
                ..query()
            }),
            (vec![b.clone()], 1)
        );
        assert_eq!(
            filtered(LeagueListQuery {
                format: Some("CMODERN".to_string()),
                ..query()
            })
            .0,
            vec![c.clone(), a.clone(), manual.clone(), imported.clone()]
        );
        assert_eq!(
            filtered(LeagueListQuery {
                format: Some("  ".to_string()),
                search: Some(" ".to_string()),
                ..query()
            })
            .1,
            5,
            "blank text filters nothing"
        );
        assert_eq!(
            filtered(LeagueListQuery {
                deck_id: Some(2),
                ..query()
            }),
            (vec![b.clone()], 1)
        );
        assert_eq!(
            filtered(LeagueListQuery {
                deck_id: Some(1),
                ..query()
            })
            .0,
            vec![c.clone(), a.clone(), manual.clone(), imported.clone()]
        );
        assert_eq!(
            filtered(LeagueListQuery {
                search: Some(" ZED ".to_string()),
                ..query()
            }),
            (vec![a.clone()], 1),
            "an opponent's name, any case"
        );
    }

    /// 0.x `Leagues/IndexControllerTest` "filters by deck archetype",
    /// "exposes deckArchetypes for the filter dropdown", "filters by
    /// archetype search"; `FormatLeagueRunsTest` "computes top opponent
    /// archetype and top matchups list"; `GetLeagueKpisTest` "returns top
    /// matchup with record and play count".
    #[test]
    fn archetypes_filter_search_and_name_the_runs() {
        let (_dir, conn) = db("leagues-archetypes");
        let seeded = seed(&conn);
        conn.execute_batch(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
                 VALUES (50, 'q-yawg', 'Qyawgmoth', 'modern', 'downloaded', 0, 0),
                        (51, 'q-ctl', 'Qcontrol arch', 'pioneer', 'manual', 0, 0);
             UPDATE decks SET archetype_id = 51 WHERE id = 2;",
        )
        .expect("archetypes");
        let a_tokens: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT token FROM matches WHERE league_token = 'Q-league-a'
                     ORDER BY started_at",
                )
                .expect("prepare");
            stmt.query_map([], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        for token in &a_tokens[2..4] {
            conn.execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 SELECT id, 1, 50, 1.0 FROM matches WHERE token = ?1",
                [token],
            )
            .expect("seat");
        }
        // A player seat never names the opponent.
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             SELECT id, 0, 51, 1.0 FROM matches WHERE token = ?1",
            [&a_tokens[0]],
        )
        .expect("player seat");

        let filtered = |changed: LeagueListQuery| -> Vec<String> {
            keys(&list(&conn, &changed, UTC).expect("list").runs)
        };
        assert_eq!(
            filtered(LeagueListQuery {
                archetype_id: Some(51),
                ..query()
            }),
            vec![seeded.b.clone()]
        );
        assert_eq!(
            filtered(LeagueListQuery {
                search: Some("YAWG".to_string()),
                ..query()
            }),
            vec![seeded.a.clone()]
        );
        assert!(
            filtered(LeagueListQuery {
                search: Some("Qcontrol arch".to_string()),
                ..query()
            })
            .is_empty()
        );

        let page = list(&conn, &query(), UTC).expect("list");
        assert_eq!(
            page.deck_archetypes,
            vec![LeagueArchetypeOption {
                id: 51,
                name: "Qcontrol arch".to_string(),
            }]
        );
        let a = page
            .runs
            .iter()
            .find(|run| run.key == seeded.a)
            .expect("run a");
        assert_eq!(
            a.matches[2].opponent_archetype.as_deref(),
            Some("Qyawgmoth")
        );
        assert_eq!(a.matches[2].opponent_archetype_id, Some(50));
        assert_eq!(a.matches[0].opponent_archetype, None);
        assert_eq!(a.top_opponent_archetype.as_deref(), Some("Qyawgmoth"));
        assert_eq!(
            a.top_matchups,
            vec![TopMatchupView {
                archetype: "Qyawgmoth".to_string(),
                wins: 1,
                losses: 1,
                count: 2,
            }]
        );
        assert_eq!(page.kpis.top_matchup, Some(a.top_matchups[0].clone()));
    }

    #[test]
    fn sorts_order_by_start_or_complete_wins() {
        let (_dir, conn) = db("leagues-sorts");
        let seeded = seed(&conn);
        let sorted = |sort: LeagueSort| -> Vec<String> {
            keys(
                &list(&conn, &LeagueListQuery { sort, ..query() }, UTC)
                    .expect("list")
                    .runs,
            )
        };
        let (a, b, c) = (seeded.a.clone(), seeded.b.clone(), seeded.c.clone());
        let manual = "manual:1".to_string();
        let imported = "0x:42".to_string();
        assert_eq!(
            sorted(LeagueSort::Oldest),
            vec![
                imported.clone(),
                manual.clone(),
                a.clone(),
                b.clone(),
                c.clone()
            ]
        );
        assert_eq!(
            sorted(LeagueSort::Best),
            vec![
                b.clone(),
                a.clone(),
                c.clone(),
                imported.clone(),
                manual.clone()
            ],
            "ties newest first"
        );
        assert_eq!(sorted(LeagueSort::Worst), vec![manual, c, imported, a, b]);
    }

    #[test]
    fn pages_hold_20_runs_and_a_page_past_the_end_shows_the_last() {
        let (_dir, conn) = db("leagues-pages");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), "Qburn", false);
        for id in 1..=25_i64 {
            let at = T0 + id * MINUTE;
            manual_league(&conn, id, Some(local), Some(1), "Qmanual", at, at);
        }
        let page = |number: u32| {
            list(
                &conn,
                &LeagueListQuery {
                    page: number,
                    ..query()
                },
                UTC,
            )
        };
        let first = page(1).expect("page 1");
        assert_eq!(first.runs.len(), 20);
        assert_eq!(first.runs[0].key, "manual:25");
        assert_eq!((first.total, first.last_page), (25, 2));
        assert_eq!(first.kpis.total, 25, "every page's runs");
        let second = page(2).expect("page 2");
        assert_eq!(second.runs.len(), 5);
        assert_eq!(second.page, 2);
        let past = page(9).expect("page 9");
        assert_eq!(past.page, 2);
        assert_eq!(keys(&past.runs), keys(&second.runs));
        assert_eq!(page(0).expect("page 0").page, 1);

        let (_empty_dir, empty) = db("leagues-pages-empty");
        let none = list(&empty, &query(), UTC).expect("empty");
        assert_eq!(
            (none.runs.len(), none.total, none.page, none.last_page),
            (0, 0, 1, 1)
        );
        assert_eq!(none.kpis.trophy_rate, None);
    }

    #[test]
    fn the_page_shows_the_current_account_and_leagues_without_an_account() {
        let (_dir, conn) = db("leagues-accounts");
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        conn.execute(
            "INSERT INTO log_files (path, identity_hash, head_hash, head_len, day_anchor, timezone,
                 local_username, clock_date, first_seen_at, last_seen_at)
             VALUES ('Q/mtgo.log', 'aa', 'bb', 0, '2026-07-01', 'UTC', 'Qlocal', '2026-07-01', 1, 2)",
            [],
        )
        .expect("log file");
        // Qlocal logged in last (release spec 5: a Login makes it current).
        conn.execute(
            "UPDATE accounts SET last_login_at = 2 WHERE id = ?1",
            [local],
        )
        .expect("login");
        for (id, owner) in [(1, Some(local)), (2, Some(other)), (3, None)] {
            manual_league(&conn, id, owner, None, "Qmanual", id, id);
        }
        let page = list(&conn, &query(), UTC).expect("list");
        assert_eq!(
            keys(&page.runs),
            strings(&["manual:3", "manual:1"]),
            "Qlocal's and the account-less one"
        );
        assert_eq!(page.runs[1].deck, None);
    }

    // Deck pages

    #[test]
    fn a_decks_leagues_tab_has_its_runs_in_the_timeframe_counting_its_matches() {
        let (_dir, conn) = db("leagues-deck-tab");
        let seeded = seed(&conn);
        let now = T0 + 3 * DAY;
        let tab = deck_league_runs(&conn, 1, None, "alltime", now, UTC).expect("tab");
        assert_eq!(
            keys(&tab.runs),
            vec![seeded.c.clone(), seeded.a.clone(), "0x:42".to_string()],
            "runs holding a complete deck match, newest first"
        );
        assert_eq!(tab.kpis.total, 3);
        assert_eq!(tab.kpis.completed, 2);

        let version = deck_league_runs(&conn, 1, Some(10), "alltime", now, UTC).expect("tab");
        assert_eq!(keys(&version.runs), strings(&["0x:42"]));
        let days =
            deck_league_runs(&conn, 1, None, "2026-07-01..2026-07-02", now, UTC).expect("tab");
        assert_eq!(keys(&days.runs), vec![seeded.a.clone()]);

        // Only the deck's matches count on its cards.
        insert(
            &conn,
            &Played {
                version: Some(20),
                ..played(
                    seeded.local,
                    "Q-league-c-m9",
                    Some("Q-league-c"),
                    T0 + 2 * DAY + 3 * HOUR,
                    "win",
                )
            },
        );
        let mixed = deck_league_runs(&conn, 1, None, "alltime", now, UTC).expect("tab");
        let live = &mixed.runs[0];
        assert_eq!(live.key, seeded.c);
        assert_eq!(live.matches.len(), 2, "not deck 2's match");

        let missing = deck_league_runs(&conn, 404, None, "alltime", now, UTC).expect("tab");
        assert!(missing.runs.is_empty());
    }

    #[test]
    fn a_decks_tournaments_need_a_complete_deck_match_in_the_timeframe() {
        let (_dir, conn) = db("leagues-deck-tournaments");
        let seeded = seed(&conn);
        let local = seeded.local;
        let round =
            |token: &str, event: i64, number: i64, at: i64, state: &'static str, version: i64| {
                insert(
                    &conn,
                    &Played {
                        tournament: Some((event, number)),
                        state,
                        version: Some(version),
                        ..played(local, token, None, at, "win")
                    },
                );
            };
        round("Q-t500-r1", 500, 1, T0 + 5 * DAY, "complete", 11);
        round("Q-t500-r2", 500, 2, T0 + 5 * DAY + HOUR, "in_progress", 11);
        round("Q-t600-r1", 600, 1, T0 + 5 * DAY, "in_progress", 11);
        round("Q-t700-r1", 700, 1, T0 + 5 * DAY, "complete", 20);
        round("Q-t800-r1", 800, 1, T0 - 40 * DAY, "complete", 10);
        let now = T0 + 6 * DAY;

        let all = deck_tournament_runs(&conn, 1, None, "alltime", now, UTC).expect("tab");
        let events: Vec<i64> = all.iter().map(|run| run.event_id).collect();
        assert_eq!(
            events,
            vec![500, 800],
            "newest first; 600 has no complete match"
        );
        let live = &all[0];
        assert_eq!(live.name, "Tournament 500 (06-07-2026)");
        assert_eq!(live.format, "Modern");
        assert_eq!(live.started_at_ms, T0 + 5 * DAY);
        assert_eq!(live.version_label.as_deref(), Some("v2"));
        assert_eq!(live.results, vec![MarkView::W]);
        assert_eq!((live.matches_count, live.in_progress_count), (2, 1));
        assert_eq!((live.wins, live.losses), (1, 0));
        let rows: Vec<(&str, Option<MarkView>, Option<i64>)> = live
            .matches
            .iter()
            .map(|row| (row.state.as_str(), row.result, row.round))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("complete", Some(MarkView::W), Some(1)),
                ("in_progress", None, Some(2))
            ],
            "the live dot"
        );
        assert_eq!(live.matches[0].opponent_name.as_deref(), Some("Qopponent"));

        let version = deck_tournament_runs(&conn, 1, Some(10), "alltime", now, UTC).expect("tab");
        assert_eq!(version.len(), 1);
        assert_eq!(version[0].event_id, 800);
        let week = deck_tournament_runs(&conn, 1, None, "week", now, UTC).expect("tab");
        assert_eq!(week.len(), 1);
        assert_eq!(week[0].event_id, 500);
    }

    #[test]
    fn a_decks_panels_have_its_finishes_and_its_current_or_latest_run() {
        let (_dir, conn) = db("leagues-deck-panels");
        let seeded = seed(&conn);
        let panels = deck_panels(&conn, 1, None, UTC).expect("panels");
        let buckets: Vec<(&str, u32)> = panels
            .finishes
            .buckets
            .iter()
            .map(|bucket| (bucket.label.as_str(), bucket.count))
            .collect();
        assert_eq!(
            buckets,
            vec![
                ("5-0", 0),
                ("4-1", 1),
                ("3-2", 0),
                ("2-3", 0),
                ("1-4", 0),
                ("0-5", 0)
            ]
        );
        assert_eq!(
            panels.finishes.dropped, 1,
            "the imported run dropped by hand"
        );
        // The newest Active run holding a deck match in any state: run D.
        let current = panels.current.expect("a run in progress");
        assert_eq!(current.run.key, "Q-league-d:Q-league-d-m0");
        let odds = current.in_progress.expect("odds");
        assert_eq!((odds.wins, odds.losses), (0, 0));
        // 8 league matches are too few: the deck's whole record, 6-2.
        assert_eq!(odds.odds_source, OddsSourceView::Deck);
        assert_eq!(odds.win_probability, 75);
        assert_eq!(odds.odds.rounds_left, 5);
        let version = deck_panels(&conn, 1, Some(10), UTC).expect("panels");
        let latest = version
            .current
            .expect("the latest finished run of version 10");
        assert_eq!(latest.run.key, "0x:42");
        assert_eq!(latest.in_progress, None);
        let _ = &seeded.c;

        let other = deck_panels(&conn, 2, None, UTC).expect("panels");
        let latest = other.current.expect("the latest finished run");
        assert_eq!(latest.run.key, seeded.b);
        assert_eq!(latest.in_progress, None);
        assert_eq!(other.finishes.buckets[0].count, 1);

        let none = deck_panels(&conn, 3, None, UTC).expect("panels");
        assert_eq!(none.current, None);
        assert_eq!(none.finishes.dropped, 0);
    }

    #[test]
    fn a_decks_trophies_are_its_complete_runs_it_won_every_match_of() {
        let (_dir, conn) = db("leagues-deck-trophies");
        seed(&conn);
        assert_eq!(deck_trophies(&conn, 2), Ok(1));
        assert_eq!(deck_trophies(&conn, 1), Ok(0), "run A lost a match");
        assert_eq!(deck_trophies(&conn, 404), Ok(0));
        let header = crate::decks::header(&conn, 2, None, T0, UTC, None)
            .expect("header")
            .expect("deck");
        assert_eq!(header.trophies, Some(1));
    }

    // Edits

    #[test]
    fn a_start_is_a_local_date_and_time() {
        let london: Tz = "Europe/London".parse().expect("zone");
        assert_eq!(parse_started_at("2026-07-01T11:00", london), Ok(T0));
        assert_eq!(parse_started_at(" 2026-07-01 11:00:00 ", london), Ok(T0));
        assert_eq!(parse_started_at("2026-07-01T10:00", UTC), Ok(T0));
        assert_eq!(parse_started_at("2026-07-01", UTC), Ok(T0 - 10 * HOUR));
        // 01:30 does not exist in London that night: moved on by the gap.
        assert_eq!(
            parse_started_at("2026-03-29T01:30", london),
            parse_started_at("2026-03-29T01:30", UTC)
        );
        // Lord Howe's gap is half an hour: 02:15 (+10:30) is 02:45 (+11:00).
        let lord_howe: Tz = "Australia/Lord_Howe".parse().expect("zone");
        assert_eq!(
            parse_started_at("2026-10-04T02:15", lord_howe),
            parse_started_at("2026-10-03T15:45", UTC)
        );
        assert_eq!(
            parse_started_at("", UTC),
            Err(refused("The started at field is required."))
        );
        assert_eq!(
            parse_started_at("next tuesday", UTC),
            Err(refused("The started at field must be a valid date."))
        );
    }

    #[test]
    fn refused_league_edits_become_messages() {
        assert_eq!(league_edit_result(LeagueEditOutcome::Done), Ok(()));
        assert_eq!(
            league_edit_result(LeagueEditOutcome::NoMatches),
            Err(refused("No matches selected."))
        );
    }

    #[test]
    fn the_new_league_refusals_become_messages() {
        assert_eq!(
            league_edit_result(LeagueEditOutcome::StillActive),
            Err(refused("Drop it first"))
        );
        assert_eq!(
            league_edit_result(LeagueEditOutcome::LeagueRemoved),
            Err(refused("That league was deleted"))
        );
        assert_eq!(
            league_edit_result(LeagueEditOutcome::Tournament),
            Err(refused("Tournament matches stay in their tournament"))
        );
        assert_eq!(
            league_edit_result(LeagueEditOutcome::NotFinished),
            Err(refused("Only finished matches can be moved"))
        );
        assert_eq!(
            league_edit_result(LeagueEditOutcome::NothingToShow),
            Err(refused(
                "No matches are left in this league, so it has nothing to show."
            ))
        );
    }

    #[test]
    fn times_from_the_ui_are_whole_milliseconds() {
        assert_eq!(ms_from_ui(1_782_900_000_000.0), Ok(1_782_900_000_000));
        assert_eq!(ms_from_ui(1.5), Err(refused("That time is not valid.")));
        assert_eq!(
            ms_from_ui(f64::NAN),
            Err(refused("That time is not valid."))
        );
        assert_eq!(ms_from_ui(1e300), Err(refused("That time is not valid.")));
    }

    #[test]
    fn undo_puts_back_the_status_a_delete_replaced_with_a_drop() {
        let status = |state, user_state| {
            Some(LeagueStatus {
                id: 1,
                state,
                user_state,
            })
        };
        // The fold says Active, the player said Complete: the delete drops it.
        assert_eq!(
            status_before_delete(status(RunState::Active, Some(UserState::Complete))),
            Some("complete".to_string())
        );
        assert_eq!(
            status_before_delete(status(RunState::Active, Some(UserState::Partial))),
            Some("partial".to_string())
        );
        // No drop recorded: nothing to put back.
        assert_eq!(
            status_before_delete(status(RunState::Complete, Some(UserState::Partial))),
            None
        );
        assert_eq!(status_before_delete(status(RunState::Complete, None)), None);
        assert_eq!(status_before_delete(None), None);
    }

    #[test]
    fn league_self_management_goes_through_the_tracker() {
        let dir = TestDir::new("leagues-self-management");
        let (state, _receiver) = ready_state(&dir);
        {
            let conn = store::open_writer(&state.db_path).expect("open");
            let local = account(&conn, "Qlocal");
            deck(&conn, 1, Some(local), "Qburn", false);
            version(&conn, 10, 1, T0 - 2 * DAY);
            run_of(
                &conn,
                local,
                "Q-done",
                T0 - DAY,
                &["win", "win", "win", "win", "loss"],
                10,
                "CMODERN",
            );
            insert(
                &conn,
                &Played {
                    version: Some(10),
                    ..played(local, "Q-free", None, T0 + 2 * HOUR, "win")
                },
            );
            insert(
                &conn,
                &Played {
                    state: "in_progress",
                    version: Some(10),
                    ..played(local, "Q-live-m0", Some("Q-live"), T0 + HOUR, "win")
                },
            );
            insert(
                &conn,
                &Played {
                    version: Some(10),
                    ..played(local, "Q-live-m1", Some("Q-live"), T0, "win")
                },
            );
            sync(&conn);
        }
        let done = "Q-done:Q-done-m0".to_string();
        let live = "Q-live:Q-live-m1".to_string();
        let card = |state: &AppState, key: &str| {
            list(&lock(&state.reader), &query(), UTC)
                .expect("list")
                .runs
                .into_iter()
                .find(|run| run.key == key)
        };

        // Move to league: the targets, a move, its refusals, and No league.
        let offered = targets(&lock(&state.reader), "Q-free").expect("targets");
        assert_eq!(offered.current, None);
        let keys: Vec<&str> = offered
            .targets
            .iter()
            .map(|target| target.key.as_str())
            .collect();
        assert_eq!(keys, vec![live.as_str(), done.as_str()], "newest first");
        assert!(
            targets(&lock(&state.reader), "Q-live-m0")
                .expect("targets")
                .targets
                .is_empty(),
            "a match in progress cannot move"
        );
        assert_eq!(move_match(&state, "Q-free", Some(&done), T0), Ok(()));
        let moved = targets(&lock(&state.reader), "Q-free").expect("targets");
        assert_eq!(
            moved.current.as_ref().map(|league| league.key.as_str()),
            Some(done.as_str())
        );
        assert!(moved.targets.iter().all(|target| target.key != done));
        assert_eq!(
            move_match(&state, "Q-live-m0", Some(&done), T0),
            Err(refused("Only finished matches can be moved"))
        );
        assert_eq!(move_match(&state, "Q-free", None, T0), Ok(()));
        assert_eq!(
            targets(&lock(&state.reader), "Q-free")
                .expect("targets")
                .current,
            None
        );

        // Status: a player's status, then Automatic again.
        assert_eq!(set_status(&state, &done, Some("partial"), T0), Ok(()));
        let partial = card(&state, &done).expect("the done run");
        assert_eq!(partial.state, RunStateView::Partial);
        assert_eq!(partial.user_state, Some(RunStateView::Partial));
        assert_eq!(partial.auto_state, RunStateView::Complete);
        assert_eq!(
            set_status(&state, &done, Some("finished"), T0),
            Err(refused("Choose a status."))
        );
        assert_eq!(set_status(&state, &done, None, T0), Ok(()));
        let automatic = card(&state, &done).expect("the done run");
        assert_eq!(automatic.user_state, None);
        assert_eq!(automatic.state, RunStateView::Complete);

        // Delete: refused while Active; a run marked Complete records a drop.
        assert_eq!(
            delete_run(&state, &live, "keep", T0),
            Err(refused("Drop it first"))
        );
        assert_eq!(
            delete_run(&state, &live, "maybe", T0),
            Err(refused("Choose what happens to the matches."))
        );
        assert_eq!(set_status(&state, &live, Some("complete"), T0), Ok(()));
        let live_id = row_id(&state, &live);
        assert_eq!(
            delete_run(&state, &live, "keep", T0 + 3 * HOUR),
            Ok(DeleteLeagueView {
                league_id: live_id,
                removed_at: T0 + 3 * HOUR,
                previous_status: Some("complete".to_string()),
            })
        );
        assert_eq!(
            card(&state, &live),
            None,
            "a deleted league leaves the page"
        );
        let listed = crate::matches::deleted(&lock(&state.reader)).expect("deleted");
        assert_eq!(
            listed
                .leagues
                .iter()
                .map(|league| league.run_key.as_str())
                .collect::<Vec<_>>(),
            vec![live.as_str()]
        );
        assert!(listed.matches.is_empty(), "kept matches are not deleted");

        // Undo by the row id answers the run key it has now, for the
        // status the UI puts back.
        assert_eq!(
            undo_delete(&state, live_id, T0 + 3 * HOUR, T0 + 4 * HOUR),
            Ok(live.clone())
        );
        assert_eq!(
            undo_delete(&state, live_id + 1_000, T0 + 3 * HOUR, T0 + 4 * HOUR),
            Err(league_refusal(LeagueEditOutcome::NotFound)),
            "no such row"
        );
        assert_eq!(
            set_status(&state, &live, Some("complete"), T0 + 4 * HOUR),
            Ok(())
        );
        let undone = card(&state, &live).expect("back on the page");
        assert_eq!(undone.user_state, Some(RunStateView::Complete));

        // Delete with its matches: a match deleted with its league comes
        // back only with the league (issue #418), which comes back whole.
        assert_eq!(set_notes(&state, &done, "Q note", T0 + 4 * HOUR), Ok(()));
        assert_eq!(
            set_status(&state, &done, Some("partial"), T0 + 4 * HOUR),
            Ok(())
        );
        let done_id = row_id(&state, &done);
        assert_eq!(
            delete_run(&state, &done, "delete", T0 + 5 * HOUR),
            Ok(DeleteLeagueView {
                league_id: done_id,
                removed_at: T0 + 5 * HOUR,
                previous_status: None,
            })
        );
        let listed = crate::matches::deleted(&lock(&state.reader)).expect("deleted");
        assert!(listed.leagues.iter().any(|league| league.run_key == done));
        let mut hidden: Vec<&str> = listed
            .matches
            .iter()
            .map(|row| row.token.as_str())
            .collect();
        hidden.sort_unstable();
        assert_eq!(
            hidden,
            vec![
                "Q-done-m0",
                "Q-done-m1",
                "Q-done-m2",
                "Q-done-m3",
                "Q-done-m4"
            ]
        );
        for row in &listed.matches {
            let with = row.deleted_with_league.as_ref().expect("deleted with it");
            assert_eq!(with.run_key, done);
        }
        for token in ["Q-done-m0", "Q-done-m1"] {
            assert_eq!(
                crate::matches::restore(&state, token, T0 + 6 * HOUR),
                Err(refused("Restore its league to bring it back."))
            );
        }
        let listed = crate::matches::deleted(&lock(&state.reader)).expect("deleted");
        assert_eq!(listed.matches.len(), 5, "nothing restored");
        assert_eq!(restore_run(&state, &done, T0 + 7 * HOUR), Ok(()));
        let restored = card(&state, &done).expect("listed again, same run key");
        assert_eq!(restored.matches.len(), 5);
        assert_eq!(restored.notes.as_deref(), Some("Q note"));
        assert_eq!(restored.user_state, Some(RunStateView::Partial));
        let listed = crate::matches::deleted(&lock(&state.reader)).expect("deleted");
        assert!(listed.matches.is_empty());
        assert!(listed.leagues.iter().all(|league| league.run_key != done));

        crate::commands::stop_tracker(&state);
        assert_eq!(
            set_status(&state, &done, None, T0),
            Err(CommandError::Tracker("tracking is not running".to_string()))
        );
    }

    fn saw_leagues_changed(receiver: &std::sync::mpsc::Receiver<TrackerEvent>) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(TrackerEvent::LeaguesChanged) =
                receiver.recv_timeout(Duration::from_millis(100))
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn league_edits_go_through_the_tracker() {
        let dir = TestDir::new("leagues-edits");
        let (state, receiver) = ready_state(&dir);
        let local;
        {
            let conn = store::open_writer(&state.db_path).expect("open");
            local = account(&conn, "Qlocal");
            deck(&conn, 1, Some(local), "Qburn", false);
            deck(&conn, 2, Some(local), "Qcontrol", false);
            version(&conn, 10, 1, T0 - DAY);
            insert(
                &conn,
                &Played {
                    version: Some(10),
                    ..played(local, "Q-free", None, T0, "win")
                },
            );
            insert(
                &conn,
                &Played {
                    state: "in_progress",
                    version: Some(10),
                    ..played(local, "Q-live-m0", Some("Q-live"), T0 + HOUR, "win")
                },
            );
            insert(
                &conn,
                &Played {
                    version: Some(10),
                    ..played(local, "Q-live-m1", Some("Q-live"), T0, "win")
                },
            );
            // As the tracker's sync would after these matches.
            sync(&conn);
        }
        let read = |state: &AppState| list(&lock(&state.reader), &query(), UTC).expect("list");

        assert_eq!(
            create_manual(&state, 1, "  ", "2026-07-01T10:00", T0),
            Err(refused("The name field is required."))
        );
        assert_eq!(
            create_manual(&state, 1, "Qmanual", "", T0),
            Err(refused("The started at field is required."))
        );
        assert_eq!(
            create_manual(&state, 404, "Qmanual", "2026-07-01T10:00", T0),
            Err(refused("The selected deck id is invalid."))
        );
        assert_eq!(
            create_manual(&state, 1, " Qmanual ", "2026-07-01T09:00", T0),
            Ok(())
        );
        assert!(saw_leagues_changed(&receiver), "the edit is announced");
        let page = read(&state);
        let manual = page
            .runs
            .iter()
            .find(|run| run.manual)
            .expect("the manual league");
        let key = manual.key.clone();
        assert_eq!(
            (manual.name.as_str(), manual.started_at_ms),
            ("Qmanual", T0 - HOUR)
        );

        let offered: Vec<String> = available(&lock(&state.reader), &key)
            .expect("available")
            .iter()
            .map(|row| row.token.clone())
            .collect();
        assert!(offered.contains(&"Q-free".to_string()), "{offered:?}");
        assert!(!offered.contains(&"Q-live-m0".to_string()), "in progress");
        let row = available(&lock(&state.reader), &key)
            .expect("available")
            .into_iter()
            .find(|row| row.token == "Q-free")
            .expect("Q-free");
        assert_eq!(row.result, Some(MarkView::W));
        assert_eq!(row.game_record, "2-0");
        assert_eq!(row.opponent_name.as_deref(), Some("Qopponent"));
        let live_key = "Q-live:Q-live-m1".to_string();
        assert!(
            available(&lock(&state.reader), &live_key).is_ok(),
            "every league offers matches"
        );

        assert_eq!(
            link(&state, &key, &[], T0),
            Err(refused("No matches selected."))
        );
        assert_eq!(link(&state, &key, &strings(&["Q-free"]), T0), Ok(()));
        assert!(
            !available(&lock(&state.reader), &key)
                .expect("available")
                .iter()
                .any(|row| row.token == "Q-free"),
            "linked now"
        );
        let linked = read(&state);
        let manual = linked
            .runs
            .iter()
            .find(|run| run.key == key)
            .expect("manual");
        assert_eq!(manual.results, vec![Some(MarkView::W)]);
        assert_eq!(
            unlink(&state, &key, "Q-live-m1", T0),
            Err(refused("Match does not belong to this league."))
        );
        assert_eq!(unlink(&state, &key, "Q-free", T0), Ok(()));

        assert_eq!(set_notes(&state, &key, "  Q note ", T0), Ok(()));
        let noted = read(&state);
        let manual = noted
            .runs
            .iter()
            .find(|run| run.key == key)
            .expect("manual");
        assert_eq!(manual.notes.as_deref(), Some("Q note"));
        assert_eq!(
            set_notes(&state, &key, &"q".repeat(5001), T0),
            Err(refused(
                "The notes field must not be greater than 5000 characters."
            ))
        );

        assert_eq!(
            set_status(&state, &live_key, Some("dropped"), T0 + 2 * HOUR),
            Ok(())
        );
        let dropped = read(&state);
        let live = dropped
            .runs
            .iter()
            .find(|run| run.key == live_key)
            .expect("the live run");
        assert_eq!(live.state, RunStateView::Dropped);
        assert_eq!(live.classification, ClassificationView::Brick);
        assert_eq!(
            set_status(&state, "manual:404", Some("dropped"), T0),
            Err(refused("This league no longer exists."))
        );

        crate::commands::stop_tracker(&state);
        assert_eq!(
            set_status(&state, &live_key, Some("dropped"), T0),
            Err(CommandError::Tracker("tracking is not running".to_string()))
        );
    }

    #[test]
    fn creating_a_league_with_no_account_yet_asks_for_a_match_first() {
        let dir = TestDir::new("leagues-create-no-account");
        let (state, _receiver) = ready_state(&dir);
        assert_eq!(
            create_manual(&state, 1, "Qmanual", "2026-07-01T10:00", T0),
            Err(refused("Play a match first so MyMTGO knows your account."))
        );
        crate::commands::stop_tracker(&state);
    }

    // The stored runs

    #[test]
    fn dashboard_runs_count_each_runs_rounds_marks_and_deck() {
        let (_dir, conn) = db("leagues-dashboard-runs");
        let seeded = seed(&conn);
        sync(&conn);
        let data = super::gather(&conn, &[seeded.local]).expect("gather");
        let runs = dashboard_runs(&data, UTC);
        let a = runs.iter().find(|run| run.key == seeded.a).expect("run A");
        assert_eq!(
            (a.kind, a.rounds, a.wins, a.losses, a.state),
            (LeagueKind::Constructed, 5, 4, 1, RunState::Complete)
        );
        assert_eq!(a.subject, "Qburn");
        assert_eq!(a.deck_id, Some(1));
        assert_eq!(a.cover_art.as_deref(), Some("https://q/77.jpg"));
        assert_eq!(a.format.as_deref(), Some("CMODERN"));
        assert_eq!(a.match_tokens.len(), 5);
        assert_eq!(a.first_pick, None);
        let c = runs.iter().find(|run| run.key == seeded.c).expect("run C");
        assert_eq!(c.state, RunState::Active);
        assert_eq!((c.wins, c.losses), (1, 1));
    }

    /// #390: a run whose complete matches have no deck link (as 0.x imports
    /// can have) shows their record and rows on its card, and the card, the
    /// KPIs and the home dashboard agree.
    #[test]
    fn a_run_of_matches_with_no_deck_shows_its_record_everywhere() {
        let (_dir, conn) = db("leagues-no-deck-run");
        let local = account(&conn, "Qlocal");
        let outcomes = ["win", "win", "loss", "win", "win"];
        for (index, outcome) in outcomes.iter().enumerate() {
            insert(
                &conn,
                &played(
                    local,
                    &format!("Q-nodeck-m{index}"),
                    Some("Q-league-nodeck"),
                    T0 + index as i64 * HOUR,
                    outcome,
                ),
            );
        }
        sync(&conn);

        let page = super::list(&conn, &query(), UTC).expect("list");
        assert_eq!(page.runs.len(), 1);
        let card = &page.runs[0];
        assert_eq!(card.matches.len(), 5);
        assert_eq!(card.deck, None);
        let wins = card
            .results
            .iter()
            .filter(|mark| **mark == Some(MarkView::W))
            .count();
        assert_eq!(wins, 4);
        assert_eq!(card.classification, ClassificationView::Cash);
        assert_eq!(page.kpis.avg_finish, Some(4.0));
        assert_eq!(page.kpis.cash_rate, Some(100.0));

        let data = super::gather(&conn, &[local]).expect("gather");
        let runs = dashboard_runs(&data, UTC);
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].wins, runs[0].losses), (4, 1));
    }

    /// Counting matches with no deck differs from 0.x (#390), so the parity
    /// allowlist notes it (fixture `leagues`).
    #[test]
    fn counting_matches_with_no_deck_is_noted_in_the_allowlist() {
        let list: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../fixtures/parity/allowlist.json"))
                .expect("the allowlist is JSON");
        let note = list.iter().find(|entry| {
            entry["fixture"] == "leagues"
                && entry["field"] == "run card counts matches with no deck"
        });
        assert!(note.is_some_and(|entry| {
            entry["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("#390"))
        }));
    }

    /// The pages read the stored runs and tournaments, never the log: with
    /// the log's tables gone (the workspace's SQLite has no statement hook
    /// to count reads), the Leagues page, the deck pages and the home dashboard
    /// read as before.
    #[test]
    fn the_leagues_page_and_dashboard_read_no_log_events() {
        let (_dir, conn) = db("leagues-no-log-reads");
        seed(&conn);
        insert(
            &conn,
            &Played {
                tournament: Some((500, 1)),
                version: Some(11),
                ..played(1, "Q-round-1", None, T0 + 4 * DAY, "win")
            },
        );
        sync(&conn);
        let now = T0 + 10 * DAY;
        let before = super::list(&conn, &query(), UTC).expect("list");
        let tab_before = super::deck_league_runs(&conn, 1, None, "alltime", now, UTC).expect("tab");
        let home = |conn: &Connection| {
            (
                crate::dashboard::home_dashboard_view(conn, "alltime", "all", now, UTC)
                    .expect("home"),
                crate::dashboard::home_dashboard_view(conn, "alltime", "constructed", now, UTC)
                    .expect("home"),
            )
        };
        let home_before = home(&conn);
        assert!(home_before.0.league_results.is_some());

        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE log_events;
             DROP TABLE match_event_archives;",
        )
        .expect("the log's tables gone");

        assert_eq!(super::list(&conn, &query(), UTC).expect("list"), before);
        assert_eq!(
            super::deck_league_runs(&conn, 1, None, "alltime", now, UTC).expect("tab"),
            tab_before
        );
        let tournaments =
            super::deck_tournament_runs(&conn, 1, None, "alltime", now, UTC).expect("tournaments");
        assert_eq!(tournaments.len(), 1);
        super::deck_panels(&conn, 1, None, UTC).expect("panels");
        assert_eq!(super::deck_trophies(&conn, 2), Ok(1));
        assert_eq!(home(&conn), home_before);
    }

    // The UI's names

    #[test]
    fn the_ui_sends_and_receives_camel_case() {
        let parsed: LeagueListQuery = serde_json::from_value(serde_json::json!({
            "format": null, "state": "trophies", "deckId": 2, "search": "q",
            "sort": "best", "page": 3
        }))
        .expect("parses");
        assert_eq!(parsed.state, LeagueStateChip::Trophies);
        assert_eq!(
            (parsed.sort, parsed.deck_id, parsed.page),
            (LeagueSort::Best, Some(2), 3)
        );

        let (_dir, conn) = db("leagues-json");
        let seeded = seed(&conn);
        let page = list(&conn, &query(), UTC).expect("list");
        let json = serde_json::to_value(&page).expect("json");
        assert_eq!(json["perPage"], 20);
        assert_eq!(json["kpis"]["trophyRate"], 25.0);
        let live = &json["runs"][0];
        assert_eq!(live["key"], seeded.c.as_str());
        assert_eq!(live["source"], "log");
        assert_eq!(live["state"], "active");
        assert_eq!(live["classification"], "LIVE");
        assert_eq!(
            live["results"],
            serde_json::json!(["W", "L", null, null, null])
        );
        assert_eq!(live["startedAtMs"], T0 + 2 * DAY);
        assert_eq!(live["timeOfDay"], "morning");
        assert_eq!(live["onPlayRecord"]["wins"], 1);
        assert_eq!(live["matches"][0]["games"][0]["onPlay"], true);
        let panels =
            serde_json::to_value(deck_panels(&conn, 1, None, UTC).expect("panels")).expect("json");
        assert_eq!(panels["current"]["inProgress"]["oddsSource"], "deck");
        assert_eq!(panels["finishes"]["buckets"][1]["label"], "4-1");
    }

    // Speed

    #[test]
    fn the_leagues_page_stays_fast_with_2000_league_matches() {
        let (_dir, mut conn) = db("leagues-speed");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), "Qburn", false);
        deck(&conn, 2, Some(local), "Qcontrol", false);
        version(&conn, 10, 1, 0);
        version(&conn, 20, 2, 0);
        let tx = conn.transaction().expect("transaction");
        for run in 0..400_i64 {
            let league = format!("Q-league-{run}");
            let outcomes = if run % 3 == 0 {
                ["win", "win", "win", "win", "win"]
            } else {
                ["win", "loss", "win", "loss", "win"]
            };
            let version = if run % 2 == 0 { 10 } else { 20 };
            run_of(
                &tx,
                local,
                &league,
                T0 + run * DAY,
                &outcomes,
                version,
                "CMODERN",
            );
        }
        tx.commit().expect("commit");
        // The tracker's sync, before the page is read.
        sync(&conn);

        let started = Instant::now();
        let page = super::list(&conn, &query(), UTC).expect("list");
        let first = started.elapsed();
        let searched = Instant::now();
        let found = super::list(
            &conn,
            &LeagueListQuery {
                search: Some("qopp".to_string()),
                sort: LeagueSort::Best,
                page: 7,
                ..query()
            },
            UTC,
        )
        .expect("list");
        let second = searched.elapsed();
        eprintln!("list_leagues over 2000 matches: {first:?}, searched and sorted: {second:?}");
        assert_eq!(page.total, 400);
        assert_eq!(page.runs.len(), 20);
        assert_eq!(found.page, 7);
        assert!(
            first < Duration::from_secs(1) && second < Duration::from_secs(1),
            "too slow: {first:?}, {second:?}"
        );
    }
    fn record_match(token: &str, state: MatchState, outcome: Outcome) -> leagues::RunMatch {
        leagues::RunMatch {
            token: token.to_string(),
            state,
            outcome,
            started_at: T0,
            ended_at: None,
            deck_version_id: None,
            deck_id: Some(1),
            games: Vec::new(),
            opponent_archetype: None,
        }
    }

    /// 0.x `GetLeagueInProgress::winProbability` counts the deck's matches in
    /// a league that have an outcome, whatever their state.
    #[test]
    fn the_league_record_counts_every_match_with_an_outcome() {
        let run = GatheredRun {
            key: "k".to_string(),
            source: RunSource::Log,
            name: String::new(),
            dropped_at: None,
            deck_version_id: None,
            deck_id: Some(1),
            fold_state: RunState::Active,
            user_state: None,
            input: RunInput {
                course_record: None,
                state: RunState::Active,
                manual: false,
                uncapped: false,
                format: None,
                started_at: T0,
                matches: vec![
                    record_match("a", MatchState::Complete, Outcome::Win),
                    record_match("b", MatchState::Complete, Outcome::Loss),
                    record_match("c", MatchState::InProgress, Outcome::Unknown),
                    record_match("d", MatchState::Complete, Outcome::Draw),
                    record_match("e", MatchState::Abandoned, Outcome::Unknown),
                ],
            },
            rows: Vec::new(),
        };

        let record = league_record(&[run], 1, None);

        assert_eq!((record.wins, record.losses, record.total), (1, 1, 3));
    }
}
