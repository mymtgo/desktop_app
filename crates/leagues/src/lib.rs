//! League rules ported from the 0.x app (leagues spec sections 4, 6 and 7).
//!
//! `fold` replays 0.x's league assignment (`AssignLeague`,
//! `ResolveLeagueRunFromLog`, `MintLeague`, `CompleteLeague`,
//! `ProcessLeagueEvents`, `AttributeDropFromLog`) over one account's league
//! matches and league events, and returns the runs. Runs are derived, never
//! stored: the same inputs always give the same runs, whatever order they
//! arrived in.
//!
//! `numbers`, `ev`, `odds` and `tournament` turn runs and tournaments into
//! the numbers the pages show (0.x `FormatLeagueRuns`, `GetLeagueKpis`,
//! `LeagueEvTable`, `LeagueFinishOdds`, `GetLeagueResultDistribution`,
//! `GetLatestLeague`, `GetLeagueInProgress`, `FormatTournamentRuns` and the
//! sidebar trophy rule of `GetDeckViewSharedProps`). They return plain owned
//! structs; the app serializes them.
//!
//! `limited_runs` adds draft and sealed runs to the same fold
//! (`fold_league_runs`): kinds and round counts, drafts as inputs, pool
//! guards, placeholder healing and late links (limited spec section 5).
//!
//! `arena_runs` folds one account's Arena course snapshots and limited Arena
//! matches into runs (`fold_arena_runs`): a run per course, orphans split
//! into entries by their pool, and
//! completion from Arena's own course state (Arena limited runs spec
//! section 3).
//!
//! `standing` folds one tournament's log facts into the account's standing
//! for the event card (event overlay spec 5.4 and 7.1).
//!
//! Pure: no filesystem, clock, network or database. Callers pass in the
//! inputs read from the database and the timezone.

/// The version of the league fold's rules (`fold_league_runs`, `fold_drafts`).
/// The store's league sync records it in `derivations ('league_fold')` and
/// refolds every owner when the stored version is lower: bump it whenever a
/// rule change can change a stored run or draft.
pub const FOLD_VERSION: i64 = 3;

pub mod arena_runs;
pub mod bars;
pub mod ev;
pub mod fold;
pub mod limited_runs;
pub mod numbers;
pub mod odds;
pub mod standing;
pub mod tournament;

pub use arena_runs::{
    ArenaRun, ArenaRunMatch, CourseRecord, CourseSnapshot, CourseState, MatchDeck, PulledCourseRun,
    PulledRunMatch, arena_course_id, arena_run_name, fold_arena_runs, fold_arena_runs_merged,
    is_arena_run_key, join_records, newer_record, same_pool,
};
pub use bars::{BarRun, BarTone, DROPPED_BAR_HEIGHT, RunBar, RunsSummary, run_bar, runs_summary};
pub use ev::{break_even_wins, entry_cost, ev_table, net_tix, net_tix_for_run};
pub use fold::{
    CONSTRUCTED_ROUNDS, DRAFT_ROUNDS, ENTRY_PANEL_WINDOW_MS, GameResult, ImportedRunSeed,
    LeagueInput, LeagueMatch, OrderedInput, Run, RunState, SEALED_ROUNDS, fold_league_runs,
    fold_runs, is_limited_format_code, panel_round_count,
};
pub use limited_runs::{
    DraftInput, LINK_WINDOW_MS, LeagueFold, LeagueKind, LimitedFacts, LimitedSeed,
    MIN_PICKS_FOR_LINKAGE, PLACEHOLDER_PREFIX, PanelFormat,
};
pub use numbers::{
    Classification, GameMark, InProgress, LeagueKpis, Mark, ResultDistribution, RunInput, RunMatch,
    RunMatchRow, RunSummary, TimeOfDay, TopMatchup, WinLoss, complete_wins, in_progress_index,
    latest_finished_index, league_in_progress, league_kpis, on_leagues_page, passes_state_filter,
    result_distribution, shown_record, summarize_limited_run, summarize_run, trophy_count,
    version_label,
};
pub use odds::{FinishOdds, MINIMUM_LEAGUE_MATCHES, OddsSource, finish_odds, win_probability};
pub use tournament::{
    DeckVersionDate, TournamentInput, TournamentMatch, TournamentMatchRow, TournamentRun,
    tournament_name, tournament_run,
};
