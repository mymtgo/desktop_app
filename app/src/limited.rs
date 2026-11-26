//! Limited commands (limited spec section 8): the Limited index, the event
//! pages (sidebar, draft review, cross-draft card stats, deck evolution,
//! cards, matches, one match), pick notes (saved through the tracker, the
//! only writer) and the draft notes window's data (#8 shows it).
//!
//! Every read takes the stored limited runs
//! (`store::leagues::stored_limited_runs`: the draft and sealed runs the
//! league sync stored, with their drafts, listed matches and registered
//! decks; plus `stored_tournament_drafts`: the drafts played in a tournament,
//! a Swiss Draft, which the home dashboard leaves out; a page showing one run
//! reads it alone by its key) and the games'
//! stored cards, never a log event, and computes the pages with the
//! `limited` crate, as 0.x's `BuildLimitedIndex`, `GetLimitedEventSharedProps`,
//! `BuildDraftReview`, `ComputeCrossDraftCardStats`, `BuildDeckEvolution`,
//! `BuildLimitedCardRows`, `Limited\MatchesController` and
//! `BuildDraftNotes` did.
//!
//! Whose runs: the current account's (every account's when there is none),
//! plus the 0.x runs and drafts the import could not match to an account,
//! as the Leagues page. A run is named by its run key (`0x:{id}`,
//! `draft:{token}`, `tournament:{event id}` or a derived key); an unknown
//! key, or a constructed run's,
//! is not found (`None`), as 0.x answered 404.
//!
//! Card data may be missing (#6, #7): a card with no row shows as `#id`,
//! with no art and no colours, and every page still reads.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::DateTime;
use chrono_tz::Tz;
use decks::stats::Range;
use leagues::{
    LeagueKind, RunState, RunSummary, is_tournament_run_key, summarize_limited_run,
    tournament_draft_label,
};
use limited::{
    CardFacts, CardFactsMap, CardRowsInput, CastStats, CompletedMatch, CrossDraftStats, Draft,
    DraftState, EvolutionInput, OtherDraft, Pick, RegisteredCard, RunPool, cross_draft_card_stats,
    deck_evolution, draft_pick_stats, draft_signals, limited_card_rows, limited_index_kpis,
    pick_timing, run_pool, sealed_pack_count, sealed_pool, seats, seen_wheel, timing_summary,
    wheel_for_pick,
};
use serde::{Deserialize, Serialize};
use store::decks::{CardRow, card_infos};
use store::league_sync::LeagueOwner;
use store::leagues::{
    LeagueMatchRow, LimitedRun, RunFilter, notes_by_match, stored_limited_runs,
    stored_tournament_drafts,
};
use store::limited::{DraftNoteEdit, DraftNoteOutcome, PickNote, pick_note_rows};
use store::names::game_won;
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager, State};

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::dashboard::DashboardLeagueRun;
use crate::decks::{color_list, not_blank, timeframe_ranges};
use crate::leagues::{
    ClassificationView, MarkView, RunCardView, RunMatchView, RunStateView, TimeOfDayView,
    WinLossView, accounts_or_all, classification_view, game_views, mark_view, owners_of,
    run_source, state_view, time_of_day_view, top_matchup_view, win_loss_view,
};
use crate::logging::Timer;
use crate::matches::{GameBoard, MatchDetail, game_boards, opponent_colors};
use crate::summary::{MatchSummary, from_row};

/// The picks a draft has when 0.x had no draft to ask (`?? 42`).
const DEFAULT_PICKS_EXPECTED: u32 = 42;
/// How long a finished draft keeps the notes window (0.x `GRACE_SECONDS`).
const NOTES_GRACE_MS: i64 = 30_000;
/// The index's default timeframe (0.x `alltime`).
const ALL_TIME: &str = "alltime";

// ---- Types the UI sends ------------------------------------------------------

/// The Limited index filters (0.x `set`, `kind`, `timeframe`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedIndexQuery {
    /// A set code (`HOB`); `None` or blank for every set.
    pub set: Option<String>,
    /// `draft` or `sealed`; anything else is every kind.
    pub kind: Option<String>,
    /// A timeframe key (`alltime`, `monthly`, `week`); `None` is all time.
    pub timeframe: Option<String>,
}

// ---- Types the UI receives ---------------------------------------------------

/// The filters the index applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedFiltersView {
    pub set: Option<String>,
    pub kind: Option<String>,
    pub timeframe: String,
}

/// The index KPI strip (0.x `LimitedIndexKpisData`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedKpisView {
    pub events: u32,
    pub drafts: u32,
    pub unlinked: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub match_win_pct: Option<i64>,
    pub match_wins: u32,
    pub match_losses: u32,
    pub avg_wins: Option<f64>,
    pub avg_losses: Option<f64>,
    pub completed_runs: u32,
    pub most_drafted_set: Option<String>,
    pub most_drafted_count: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_pick_seconds: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub indecision_pct: Option<i64>,
}

/// One index row: a limited run, or a draft no run holds (0.x
/// `LimitedIndexRowData`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedIndexRowView {
    /// The run's key; `None` for an unlinked draft.
    pub run_key: Option<String>,
    pub draft_token: Option<String>,
    /// `HOB Draft`, `FRA Swiss Draft`, or `Draft (league unknown)`.
    pub title: String,
    pub set_code: Option<String>,
    /// `draft` or `sealed`.
    pub kind: String,
    /// The badge text (`Active`, `Complete`, `No matches`, `Dropped`,
    /// `Draft abandoned`, `Unlinked`).
    pub state: String,
    /// The badge variant (`default`, `success`, `warning`, `destructive`).
    pub state_variant: String,
    #[specta(type = Option<specta_typescript::Number>)]
    pub started_at_ms: Option<i64>,
    /// The shown record: tracked matches, raised to an Arena course's
    /// stated record.
    pub wins: u32,
    pub losses: u32,
    /// Matches counted in the record but played where MyMTGO never saw
    /// them (an Arena course's stated record above the tracked one).
    pub untracked_matches: u32,
    /// One mark per complete match (`null` for a draw), padded with `null`
    /// to the kind's round count.
    pub results: Vec<Option<MarkView>>,
    pub picks_made: u32,
    pub picks_expected: u32,
    /// Boosters a sealed pool was opened from; `None` for a draft.
    pub packs: Option<u32>,
    pub deck_registered: bool,
    pub version_count: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_pick_seconds: Option<i64>,
    pub opponents: Vec<String>,
    /// The run's counted matches, as the Leagues page's run card has them,
    /// without the opponents' colours (`limited_run_colours` reads those
    /// when the row is expanded).
    pub matches: Vec<RunMatchView>,
    pub game_wins: u32,
    pub game_losses: u32,
    pub on_play_record: WinLossView,
    pub on_draw_record: WinLossView,
    pub note: Option<String>,
    /// A run (true) or an unlinked draft.
    pub linked: bool,
    /// A league holds the run, so its "..." menu edits it. False for a
    /// draft played in a tournament (a Swiss Draft: no league row to edit)
    /// and for an unlinked draft.
    pub in_league: bool,
    /// The league run header's score ring (`EMPTY` for an unlinked draft).
    pub classification: ClassificationView,
    /// The round being played, for a LIVE run.
    pub live_round: Option<u32>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_match_seconds: Option<i64>,
    pub time_of_day: Option<TimeOfDayView>,
    /// The player's status for the run; `None` is Automatic (and for an
    /// unlinked draft).
    pub user_state: Option<RunStateView>,
    /// The fold's state, whatever the player set. Meaningless (always
    /// Active) while `linked` is false: there is no run to fold.
    pub auto_state: RunStateView,
    /// The run's note (distinct from the `note` info line).
    pub notes: Option<String>,
}

/// The Limited index.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedIndexView {
    /// Newest first.
    pub rows: Vec<LimitedIndexRowView>,
    pub kpis: LimitedKpisView,
    /// Every limited run's set code, sorted (the set filter's options).
    pub sets: Vec<PickSetOptionView>,
    pub filters: LimitedFiltersView,
}

/// The colours one of a run's matches' opponents showed (0.x
/// `opponentColors`), read only when an index row is expanded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedMatchColoursView {
    pub token: String,
    /// WUBRG letters, comma separated (`B,G`); `None` when nothing coloured
    /// was seen.
    pub opponent_colors: Option<String>,
}

/// An event page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LimitedPage {
    Draft,
    Deck,
    Matches,
    Cards,
}

/// One entry of the event's sidebar nav.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedNavItemView {
    pub page: LimitedPage,
    pub label: String,
}

/// The props every event page shares (0.x `LimitedEventData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedEventView {
    pub key: String,
    pub draft_token: Option<String>,
    /// `HOB Draft · 22 Aug`.
    pub title: String,
    /// `The Hobbit · League 11039`; `Unknown set` without card data.
    pub subtitle: String,
    pub set_code: Option<String>,
    /// What the set code reads as: `HOB`, or `Vintage Cube` for `C03`.
    pub set_label: Option<String>,
    pub set_name: Option<String>,
    /// `draft` or `sealed`.
    pub kind: String,
    pub state: String,
    pub state_variant: String,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    /// `22 Aug 2026 · 11:00` (OS timezone).
    pub started_at_human: Option<String>,
    /// The shown record: tracked matches, raised to an Arena course's
    /// stated record.
    pub wins: u32,
    pub losses: u32,
    /// Matches counted in the record but played where MyMTGO never saw
    /// them.
    pub untracked_matches: u32,
    pub picks_made: u32,
    pub picks_expected: u32,
    pub packs: Option<u32>,
    pub deck_registered: bool,
    /// The art crop of the first rare or mythic pick (or pool card) with
    /// art, else the first with art.
    pub cover_art: Option<String>,
    /// Its file in v1's images folder (`card` scheme), when downloaded.
    pub local_cover_art: Option<String>,
    pub seat_index: Option<u32>,
    pub seat_count: u32,
    pub booster_catalog_id: Option<u32>,
    /// `connecting`, `picking`, `finished`, `abandoned`, or `none`.
    pub draft_state: String,
    /// Draft (not for sealed), Deck, Matches, Cards.
    pub nav: Vec<LimitedNavItemView>,
    /// The player may change the run (status, removal, notes, matches); an
    /// Arena run takes only a status.
    pub editable: bool,
    /// The run has a linked draft with picks (an Arena draft has none).
    pub picks_recorded: bool,
}

/// One card as the Limited pages show it (0.x `LimitedCardData`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedCardView {
    pub catalog_id: u32,
    /// The card's name, or `#id` without card data.
    pub name: String,
    /// The card has a name.
    pub resolved: bool,
    #[serde(rename = "type")]
    pub card_type: Option<String>,
    pub sub_type: Option<String>,
    pub rarity: Option<String>,
    /// Colour letters as stored (`U,G`); empty without card data.
    pub colors: String,
    pub mana_cost: Option<String>,
    pub cmc: Option<f64>,
    pub image: Option<String>,
    pub art_crop: Option<String>,
    pub local_image: Option<String>,
    pub local_art_crop: Option<String>,
    pub oracle_id: Option<String>,
}

/// Picked cards of one colour (`C` for colourless).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorCountView {
    pub color: String,
    pub count: u32,
}

/// The draft review's header (0.x `BuildDraftReview` `header`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftHeaderView {
    pub seat_index: Option<u32>,
    pub seat_count: u32,
    pub booster_catalog_id: Option<u32>,
    pub pack_size: u32,
    pub picks_made: u32,
    pub picks_expected: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub started_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub ended_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub duration_minutes: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_pick_seconds: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_margin_seconds: Option<i64>,
    pub indecisive_count: u32,
    pub fastest_pack: Option<u32>,
    pub slowest_pack: Option<u32>,
    /// WUBRG then C, colours with no pick left out.
    pub colors_picked: Vec<ColorCountView>,
    pub state: String,
}

/// A card hovered before the commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ReservationView {
    pub catalog_id: u32,
    /// Whole seconds after the pack was shown; `None` without a shown time.
    #[specta(type = Option<specta_typescript::Number>)]
    pub at_seconds: Option<i64>,
}

/// One pick of the draft review (0.x `DraftPickData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftPickView {
    pub ordinal: u32,
    pub pack_number: u32,
    pub pick_number: u32,
    /// `P1p1`.
    pub label: String,
    #[specta(type = Option<specta_typescript::Number>)]
    pub pack_id: Option<u64>,
    /// 1 passes right, otherwise left.
    pub direction: Option<u32>,
    pub available: Vec<u32>,
    pub picked_catalog_id: Option<u32>,
    pub reservations: Vec<ReservationView>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub elapsed_seconds: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub margin_seconds: Option<i64>,
    pub indecisive: bool,
    #[specta(type = Option<specta_typescript::Number>)]
    pub shown_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub deadline_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub picked_at_ms: Option<i64>,
    pub note: Option<String>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub note_saved_at_ms: Option<i64>,
    pub wheel_return_ordinal: Option<u32>,
    /// Cards passed here that were still in the pack when it came back.
    pub wheeled_ids: Vec<u32>,
    /// Cards passed here that others took before it came back.
    pub taken_ids: Vec<u32>,
}

/// One colour's signal (0.x `ComputeDraftSignals`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorSignalView {
    pub color: String,
    pub wheeled: u32,
    pub seen_twice: u32,
    pub score: u32,
    pub share: f64,
}

/// What one card did across the draft's packs (0.x `ComputeSeenWheel`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SeenWheelView {
    pub seen_count: u32,
    pub first_seen_ordinal: u32,
    pub wheeled: bool,
    pub wheeled_to_me: bool,
    pub picked_count: u32,
    pub passed_count: u32,
}

/// The whole draft review, every pick at once (0.x `BuildDraftReview`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftReviewView {
    pub header: DraftHeaderView,
    pub picks: Vec<DraftPickView>,
    pub signals: Vec<ColorSignalView>,
    /// By catalog id.
    pub seen_wheel: BTreeMap<u32, SeenWheelView>,
    /// Every card the review names, by catalog id.
    pub cards: BTreeMap<u32, LimitedCardView>,
}

/// The Draft page (0.x `DraftController`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedDraftView {
    /// Sealed has no picks: show the Deck page instead.
    pub redirect_to_deck: bool,
    /// The pick asked for when the draft has it, else the first pick.
    pub selected_ordinal: Option<u32>,
    /// `None` when the run has no draft.
    pub review: Option<DraftReviewView>,
}

/// How a card fared in the user's other drafts of the set (0.x
/// `ComputeCrossDraftCardStats` row).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CrossDraftStatsView {
    pub oracle_id: String,
    pub drafts: u32,
    pub times_taken: u32,
    pub avg_ordinal: Option<f64>,
    pub times_passed: u32,
    pub times_wheeled: u32,
    pub made_deck: u32,
}

/// Every card of a draft's cross-draft panel, one read per draft.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CrossDraftsView {
    pub set_code: Option<String>,
    pub cards: Vec<CrossDraftView>,
}

/// One card's cross-draft panel.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CrossDraftView {
    pub set_code: Option<String>,
    pub catalog_id: u32,
    /// `None` without card data: nothing to compare by.
    pub oracle_id: Option<String>,
    /// `None` when the card was never seen in another draft of the set.
    pub stats: Option<CrossDraftStatsView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardQuantityView {
    pub catalog_id: u32,
    pub quantity: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ZoneDiffView {
    pub added: Vec<CardQuantityView>,
    pub removed: Vec<CardQuantityView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PoolCardView {
    pub catalog_id: u32,
    pub quantity: u32,
    pub added: u32,
    /// `main`, `side`, `pool` or `cut`.
    pub status: String,
    pub main_qty: u32,
    pub side_qty: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PoolGroupView {
    /// `W`, `U`, `B`, `R`, `G`, `M`, `C` or `L`.
    pub key: String,
    pub label: String,
    pub count: u32,
    pub cards: Vec<PoolCardView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PoolView {
    pub groups: Vec<PoolGroupView>,
}

/// One registered version (0.x `BuildDeckEvolution` `versions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredVersionView {
    pub index: u32,
    pub signature: String,
    #[specta(type = specta_typescript::Number)]
    pub captured_at_ms: i64,
    pub match_tokens: Vec<String>,
    /// `Match N`.
    pub match_labels: Vec<String>,
    pub main: u32,
    pub side: u32,
    pub colors: String,
    pub diff_main: ZoneDiffView,
    pub diff_side: ZoneDiffView,
    pub is_current: bool,
    pub pool: PoolView,
    pub main_cards: Vec<CardQuantityView>,
    pub side_cards: Vec<CardQuantityView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EvolutionSummaryView {
    pub drafted: u32,
    pub added: u32,
    pub main_spells: u32,
    pub basics: u32,
    pub sideboard: u32,
    pub version_count: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub first_registered_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_registered_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameChangeView {
    pub number: u32,
    pub added: Vec<CardQuantityView>,
    pub removed: Vec<CardQuantityView>,
    /// `registered deck`, `no board data`, `no changes`, or `None`.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchChangesView {
    pub match_token: String,
    /// `Match N`.
    pub label: String,
    pub opponent_name: Option<String>,
    /// `W`, `L`, or `None`.
    pub result: Option<String>,
    pub games: Vec<GameChangeView>,
}

/// The Deck page (0.x `BuildDeckEvolution`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedDeckView {
    pub summary: EvolutionSummaryView,
    pub versions: Vec<RegisteredVersionView>,
    pub pool: PoolView,
    pub games: Vec<MatchChangesView>,
    pub cards: BTreeMap<u32, LimitedCardView>,
}

/// One Cards page row (0.x `BuildLimitedCardRows`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedCardRowView {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    pub ordinals: Vec<u32>,
    pub labels: Vec<String>,
    /// `main`, `side`, `pool` or `cut`.
    pub status: String,
    pub added: bool,
    pub games_cast: u32,
    pub cast_won: u32,
    pub cast_lost: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub win_pct_cast: Option<i64>,
    pub seen_count: u32,
    pub wheeled: bool,
    pub prior_taken: u32,
    pub prior_avg_ordinal: Option<f64>,
    pub prior_wheeled: u32,
    pub prior_drafts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardRowsSummaryView {
    pub distinct: u32,
    pub games: u32,
    pub other_drafts: u32,
}

/// The Cards page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedCardsView {
    pub rows: Vec<LimitedCardRowView>,
    pub summary: CardRowsSummaryView,
    pub cards: BTreeMap<u32, LimitedCardView>,
}

/// The Matches page KPIs (0.x `Limited\MatchesController` `kpis`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedMatchesKpisView {
    pub wins: u32,
    pub losses: u32,
    pub game_wins: u32,
    pub game_losses: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub avg_match_seconds: Option<i64>,
    pub on_play: WinLossView,
    pub on_draw: WinLossView,
    /// The first match's start.
    #[specta(type = Option<specta_typescript::Number>)]
    pub queued_at_ms: Option<i64>,
    /// The last match's end, else its start.
    #[specta(type = Option<specta_typescript::Number>)]
    pub finished_at_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub total_minutes: Option<i64>,
}

/// The Matches page.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedMatchesView {
    /// The run's complete matches, oldest first, with opponent colours.
    pub matches: Vec<MatchSummary>,
    pub kpis: LimitedMatchesKpisView,
}

/// One pick in the draft notes window (0.x `DraftNotePickData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftNotePickView {
    pub ordinal: u32,
    /// `P1p3`.
    pub label: String,
    pub cards_in_pack: u32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub deadline_at_ms: Option<i64>,
    pub picked_catalog_id: Option<u32>,
    /// The picked card's name, `#id` without card data.
    pub picked_name: Option<String>,
    pub note: Option<String>,
    /// When the note was last saved, so the window's autosave never lets an
    /// older pushed note replace a newer one it saved.
    #[specta(type = Option<specta_typescript::Number>)]
    pub note_saved_at_ms: Option<i64>,
}

/// The live draft for the notes window (0.x `DraftNotesData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftNotesView {
    pub draft_token: String,
    /// The run holding the draft; `None` while unlinked.
    pub run_key: Option<String>,
    /// `connecting`, `picking` or `finished`.
    pub state: String,
    /// The newest pick; `None` before the first pack.
    pub current_ordinal: Option<u32>,
    /// Every pick, by ordinal.
    pub picks: Vec<DraftNotePickView>,
}

/// One Top limited picks row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedPickCountView {
    pub catalog_id: u32,
    /// `None` without card data (the card says "Card #id").
    pub name: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
    pub count: u32,
}

/// The home dashboard's Most picked card (0.x `LimitedPicksWidget`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedPicksView {
    pub set_code: Option<String>,
    pub set_name: Option<String>,
    pub picks: Vec<LimitedPickCountView>,
}

/// A set Most picked can switch to (dashboard redesign spec 4.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PickSetOptionView {
    pub code: String,
    /// The set's name, else its code.
    pub name: String,
}

// ---- Gathering ---------------------------------------------------------------

/// Every limited run and unlinked draft a page reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gathered {
    /// Newest first (by start).
    pub runs: Vec<LimitedRun>,
    /// Newest first (by start).
    pub unlinked: Vec<Draft>,
}

impl Gathered {
    /// The run with `key`; failing that, the run that took over an old
    /// link: one listing `key` among its former keys, or (stored runs keep
    /// no former keys) the run of the draft an old `draft:{token}` key named.
    fn run(&self, key: &str) -> Option<&LimitedRun> {
        if let Some(found) = self.runs.iter().find(|limited| limited.run.key == key) {
            return Some(found);
        }
        if let Some(found) = self
            .runs
            .iter()
            .find(|limited| limited.run.former_keys.iter().any(|former| former == key))
        {
            return Some(found);
        }
        let token = key.strip_prefix("draft:")?;
        self.runs
            .iter()
            .find(|limited| limited.run.draft_token.as_deref() == Some(token))
    }
}

/// Whose limited runs the pages read: the page platform's current account's
/// (every account's of that platform when there is none), plus on MTGO those
/// the 0.x import left without an account; only the run `run_key` when given.
fn limited_filter(conn: &Connection, run_key: Option<&str>) -> Result<RunFilter, StoreError> {
    let scope = crate::platform::page_scope(conn)?;
    let accounts = accounts_or_all(conn, scope.platform, scope.account)?;
    let owners = match scope.platform {
        store::Platform::Mtgo => owners_of(&accounts),
        store::Platform::Arena | store::Platform::Endstep => {
            accounts.into_iter().map(LeagueOwner::Account).collect()
        }
    };
    Ok(RunFilter {
        owners,
        kinds: Vec::new(),
        deck_id: None,
        run_key: run_key.map(str::to_string),
    })
}

/// Every stored limited run (league runs and tournament drafts) and
/// unlinked draft the pages read.
pub fn gather(conn: &Connection) -> Result<Gathered, StoreError> {
    let filter = limited_filter(conn, None)?;
    let found = stored_limited_runs(conn, &filter)?;
    let mut runs = found.runs;
    runs.extend(stored_tournament_drafts(conn, &filter)?);
    // Stable: equal starts keep league runs first.
    runs.sort_by_key(|limited| std::cmp::Reverse(limited.run.started_at));
    let mut gathered = Gathered {
        runs,
        unlinked: found.unlinked_drafts,
    };
    // Stable: equal starts keep the stored order.
    gathered
        .unlinked
        .sort_by_key(|draft| std::cmp::Reverse(draft_time(draft)));
    Ok(gathered)
}

/// The stored limited run `key` alone (a constructed run's key, or one of
/// another account's, is none).
fn run_by_key(conn: &Connection, key: &str) -> Result<Option<LimitedRun>, StoreError> {
    let filter = limited_filter(conn, Some(key))?;
    if is_tournament_run_key(key) {
        return Ok(stored_tournament_drafts(conn, &filter)?.into_iter().next());
    }
    let found = stored_limited_runs(conn, &filter)?;
    Ok(found.runs.into_iter().next())
}

/// When a draft started: its start, else its first pick's shown time (0.x
/// `started_at ?? created_at`).
fn draft_time(draft: &Draft) -> Option<i64> {
    draft
        .started_at_ms
        .or_else(|| draft.picks.iter().filter_map(|pick| pick.shown_at_ms).min())
}

// ---- Cards -------------------------------------------------------------------

/// Card rows and the facts the `limited` crate reads, for some catalog ids.
struct Cards {
    rows: HashMap<u32, CardRow>,
    facts: CardFactsMap,
}

impl Cards {
    fn read(conn: &Connection, ids: &BTreeSet<u32>) -> Result<Cards, StoreError> {
        let list: Vec<u32> = ids.iter().copied().collect();
        let rows = card_infos(conn, &list)?;
        let mut facts = CardFactsMap::new();
        for (catalog_id, row) in &rows {
            facts.insert(
                *catalog_id,
                CardFacts {
                    name: row.name.clone(),
                    card_type: row.card_type.clone(),
                    colors: row.colors.clone(),
                    color_identity: row.color_identity.clone(),
                    rarity: row.rarity.clone(),
                    set_code: row.set_code.clone(),
                    set_name: row.set_name.clone(),
                    oracle_id: row.oracle_id.clone(),
                    cmc: row.cmc,
                },
            );
        }
        Ok(Cards { rows, facts })
    }

    fn name(&self, catalog_id: u32) -> Option<String> {
        self.rows.get(&catalog_id).and_then(|row| row.name.clone())
    }

    fn view(&self, catalog_id: u32) -> LimitedCardView {
        let row = self.rows.get(&catalog_id);
        let name = row.and_then(|row| row.name.clone());
        LimitedCardView {
            catalog_id,
            resolved: name.is_some(),
            name: name.unwrap_or_else(|| format!("#{catalog_id}")),
            card_type: row.and_then(|row| row.card_type.clone()),
            sub_type: row.and_then(|row| row.sub_type.clone()),
            rarity: row.and_then(|row| row.rarity.clone()),
            colors: row.and_then(|row| row.colors.clone()).unwrap_or_default(),
            mana_cost: row.and_then(|row| row.mana_cost.clone()),
            cmc: row.and_then(|row| row.cmc),
            image: row.and_then(|row| row.image.clone()),
            art_crop: row.and_then(|row| row.art_crop.clone()),
            local_image: row.and_then(|row| row.local_image.clone()),
            local_art_crop: row.and_then(|row| row.local_art_crop.clone()),
            oracle_id: row.and_then(|row| row.oracle_id.clone()),
        }
    }

    fn views(&self, ids: &BTreeSet<u32>) -> BTreeMap<u32, LimitedCardView> {
        let mut views = BTreeMap::new();
        for &catalog_id in ids {
            views.insert(catalog_id, self.view(catalog_id));
        }
        views
    }
}

/// Every catalog id a run's registered decks list.
fn registered_ids(run: &LimitedRun, ids: &mut BTreeSet<u32>) {
    for deck in &run.registered {
        for card in &deck.cards {
            ids.insert(card.catalog_id);
        }
    }
}

/// Every catalog id of the pool Arena stated for a run.
fn stated_ids(run: &LimitedRun, ids: &mut BTreeSet<u32>) {
    if let Some(stated) = &run.stated_pool {
        ids.extend(stated.iter().copied());
    }
}

/// Every catalog id a draft names: packs, reservations and picks.
fn draft_ids(draft: &Draft, ids: &mut BTreeSet<u32>) {
    for pick in &draft.picks {
        ids.extend(pick.cards_available.iter().copied());
        ids.extend(
            pick.reservations
                .iter()
                .map(|reservation| reservation.catalog_id),
        );
        ids.extend(pick.picked_catalog_id);
    }
}

// ---- Shared pieces -----------------------------------------------------------

fn is_sealed(run: &LimitedRun) -> bool {
    run.run.kind == LeagueKind::Sealed
}

/// The run's complete matches, oldest first.
fn complete_matches(run: &LimitedRun) -> Vec<&LeagueMatchRow> {
    let mut rows: Vec<&LeagueMatchRow> = run
        .matches
        .iter()
        .filter(|row| row.state == "complete")
        .collect();
    rows.sort_by_key(|row| row.started_at);
    rows
}

/// Every match of the run, oldest first.
fn matches_by_start(run: &LimitedRun) -> Vec<&LeagueMatchRow> {
    let mut rows: Vec<&LeagueMatchRow> = run.matches.iter().collect();
    rows.sort_by_key(|row| row.started_at);
    rows
}

/// A match's mark: W, L, or `None` for any other outcome.
fn outcome_mark(row: &LeagueMatchRow) -> Option<MarkView> {
    MarkView::from_stored_outcome(&row.outcome)
}

/// The record a run shows (cross-device sync spec section 8): its complete
/// matches' wins and losses, each raised to its Arena course's stated count
/// (`leagues::shown_record`), so matches played elsewhere count.
fn shown_record(run: &LimitedRun, complete: &[&LeagueMatchRow]) -> (u32, u32) {
    leagues::shown_record(run.course_record, record(complete))
}

/// How many of a run's shown matches were played where MyMTGO never saw
/// them (another device's untracked play, the phone).
fn untracked_matches(run: &LimitedRun, complete: &[&LeagueMatchRow]) -> u32 {
    let (wins, losses) = shown_record(run, complete);
    let (tracked_wins, tracked_losses) = record(complete);
    (wins + losses).saturating_sub(tracked_wins + tracked_losses)
}

/// Won and lost matches.
fn record(rows: &[&LeagueMatchRow]) -> (u32, u32) {
    let (mut wins, mut losses) = (0, 0);
    for row in rows {
        match outcome_mark(row) {
            Some(MarkView::W) => wins += 1,
            Some(MarkView::L) => losses += 1,
            None => {}
        }
    }
    (wins, losses)
}

/// The state badge (0.x `LeagueStateBadge`): an abandoned draft outranks
/// the run's state.
fn state_badge(
    state: RunState,
    draft: Option<&Draft>,
    match_count: usize,
) -> (&'static str, &'static str) {
    if draft.is_some_and(|draft| draft.state == DraftState::Abandoned) {
        return ("Draft abandoned", "warning");
    }
    match state {
        RunState::Active => ("Active", "default"),
        RunState::Complete if match_count == 0 => ("No matches", "warning"),
        RunState::Complete => ("Complete", "success"),
        RunState::Partial | RunState::Dropped => ("Dropped", "destructive"),
    }
}

fn draft_state_name(state: DraftState) -> &'static str {
    match state {
        DraftState::Connecting => "connecting",
        DraftState::Picking => "picking",
        DraftState::Finished => "finished",
        DraftState::Abandoned => "abandoned",
    }
}

/// `Draft`, `Sealed` (0.x `ucfirst($kind)`).
fn kind_label(kind: LeagueKind) -> &'static str {
    match kind {
        LeagueKind::Constructed => "Constructed",
        LeagueKind::Draft => "Draft",
        LeagueKind::Sealed => "Sealed",
    }
}

/// Picks with a picked card.
fn picks_made(draft: Option<&Draft>) -> u32 {
    draft.map_or(0, |draft| {
        draft
            .picks
            .iter()
            .filter(|pick| pick.picked_catalog_id.is_some())
            .count() as u32
    })
}

fn picks_expected(draft: Option<&Draft>) -> u32 {
    draft.map_or(DEFAULT_PICKS_EXPECTED, |draft| draft.picks_expected)
}

/// The distinct registered deck signatures.
fn version_count(run: &LimitedRun) -> u32 {
    let signatures: BTreeSet<&str> = run
        .registered
        .iter()
        .map(|deck| deck.signature.as_str())
        .collect();
    signatures.len() as u32
}

/// Boosters a sealed pool was opened from (0.x `CountSealedPacks`); `None`
/// for a draft.
fn sealed_packs(run: &LimitedRun, cards: &CardFactsMap) -> Option<u32> {
    if !is_sealed(run) {
        return None;
    }
    // MTGO reads the packs off the registered decks even when a draft is
    // linked: a sealed draft's picks never show the added booster.
    let pool = match &run.stated_pool {
        Some(_) => pool_of(run, cards),
        None => sealed_pool(&run.registered, cards),
    };
    sealed_pack_count(run.run.format.as_deref().unwrap_or(""), &pool)
}

/// The run's pool: Arena's stated pool, else its picks, else (sealed) read
/// off its registered decks.
fn pool_of(run: &LimitedRun, cards: &CardFactsMap) -> RunPool {
    run_pool(
        is_sealed(run),
        run.draft.as_ref(),
        &run.registered,
        cards,
        run.stated_pool.as_deref(),
    )
}

/// The player has a linked draft with picks.
fn picks_recorded(run: &LimitedRun) -> bool {
    run.draft
        .as_ref()
        .is_some_and(|draft| !draft.picks.is_empty())
}

/// `trim(set_code.' '.Kind)`, or the format's name in place of the code
/// (`Vintage Cube Draft`). An Arena run has its own name, from its event
/// (`Arena Direct · EOE Sealed`); a tournament draft is named for its event
/// (`FRA Swiss Draft`).
fn run_title(run: &LimitedRun) -> String {
    if leagues::is_arena_run_key(&run.run.key) && !run.run.name.is_empty() {
        return run.run.name.clone();
    }
    let kind = if is_tournament_run_key(&run.run.key) {
        tournament_draft_label(Some(&run.run.name))
    } else {
        kind_label(run.run.kind)
    };
    let set = set_label(run).unwrap_or_default();
    format!("{set} {kind}").trim().to_string()
}

/// The name MTGO gives the run's format when it is more than one set
/// (`Vintage Cube` for `C03`), from its matches' `GameStructureCd`.
fn format_name(run: &LimitedRun) -> Option<String> {
    // An Arena match's game structure is `bo1` or `bo3`, never a format.
    if leagues::is_arena_run_key(&run.run.key) {
        return None;
    }
    run.matches.iter().find_map(|row| {
        limited::format_name(run.run.set_code.as_deref(), row.game_structure.as_deref())
    })
}

/// What the run's set reads as: its format's name, else its set code.
fn set_label(run: &LimitedRun) -> Option<String> {
    format_name(run).or_else(|| run.run.set_code.clone())
}

/// What `code` reads as: the format name of a run with that code (`Vintage
/// Cube` for `C03`), else the code.
fn code_label(runs: &[LimitedRun], code: &str) -> String {
    runs.iter()
        .filter(|limited| limited.run.set_code.as_deref() == Some(code))
        .find_map(format_name)
        .unwrap_or_else(|| code.to_string())
}

/// The name `code` shows: the format name of a run with that code, else
/// the set's name from card data.
fn code_name(
    conn: &Connection,
    runs: &[LimitedRun],
    code: &str,
) -> Result<Option<String>, StoreError> {
    let named = runs
        .iter()
        .filter(|limited| limited.run.set_code.as_deref() == Some(code))
        .find_map(format_name);
    match named {
        Some(name) => Ok(Some(name)),
        None => store::limited::set_name(conn, code),
    }
}

/// An instant in the OS timezone, formatted.
fn local_text(at_ms: i64, tz: Tz, pattern: &str) -> Option<String> {
    let at = DateTime::from_timestamp_millis(at_ms)?;
    Some(at.with_timezone(&tz).format(pattern).to_string())
}

/// Whole minutes between two instants, rounded (0.x
/// `round(diffInSeconds / 60)`).
fn minutes_between(from_ms: i64, to_ms: i64) -> i64 {
    ((to_ms - from_ms).abs() as f64 / 60_000.0).round() as i64
}

/// The run's numbers (0.x `FormatLeagueRuns`, limited rounds).
fn summary_of(run: &LimitedRun, tz: Tz) -> RunSummary {
    summarize_limited_run(&run.run_input(), run.run.round_count(), tz)
}

/// The run's counted matches as a run card lists them.
fn run_match_views(
    run: &LimitedRun,
    summary: &RunSummary,
    notes: &HashMap<String, String>,
    colors: &HashMap<String, String>,
) -> Vec<RunMatchView> {
    let mut views = Vec::new();
    for counted in &summary.matches {
        let stored = run.matches.iter().find(|row| row.token == counted.token);
        views.push(RunMatchView {
            token: counted.token.clone(),
            result: mark_view(counted.result),
            opponent_name: stored.and_then(|row| row.opponent.clone()),
            // A draft seat has no archetype.
            opponent_archetype: None,
            opponent_archetype_id: None,
            games: game_views(&counted.games),
            started_at_ms: counted.started_at,
            duration_seconds: counted.duration_seconds,
            notes: notes.get(&counted.token).cloned(),
            manual: stored.is_some_and(|row| row.source == "manual"),
            opponent_colors: colors.get(&counted.token).cloned(),
        });
    }
    views
}

// ---- Index -------------------------------------------------------------------

fn in_range(at_ms: Option<i64>, range: &Range) -> bool {
    match at_ms {
        Some(at) => at >= range.from_ms && at <= range.to_ms,
        // A draft with no time at all shows only for all time.
        None => range.from_ms == i64::MIN,
    }
}

fn run_row(
    run: &LimitedRun,
    cards: &CardFactsMap,
    notes_by_match: &HashMap<String, String>,
    tz: Tz,
) -> LimitedIndexRowView {
    let draft = run.draft.as_ref();
    let complete = complete_matches(run);
    let (wins, losses) = shown_record(run, &complete);
    let (state, variant) = state_badge(run.shown_state(), draft, complete.len());
    let summary = summary_of(run, tz);

    let mut results: Vec<Option<MarkView>> = complete.iter().map(|row| outcome_mark(row)).collect();
    // An Arena run has no known round count, so no empty slots.
    if !run.is_arena() {
        while results.len() < run.run.round_count() {
            results.push(None);
        }
    }
    let opponents: Vec<String> = complete
        .iter()
        .filter_map(|row| row.opponent.clone())
        .filter(|name| !name.is_empty())
        .collect();
    let note = (complete.is_empty() && run.shown_state() == RunState::Complete)
        .then(|| "Draft finished, league ended without play".to_string());

    LimitedIndexRowView {
        run_key: Some(run.run.key.clone()),
        draft_token: draft.map(|draft| draft.token.clone()),
        title: run_title(run),
        set_code: run.run.set_code.clone(),
        kind: run.run.kind.as_str().to_string(),
        state: state.to_string(),
        state_variant: variant.to_string(),
        started_at_ms: Some(run.run.started_at),
        wins,
        losses,
        untracked_matches: untracked_matches(run, &complete),
        results,
        picks_made: picks_made(draft),
        picks_expected: picks_expected(draft),
        packs: sealed_packs(run, cards),
        deck_registered: !run.registered.is_empty(),
        version_count: version_count(run),
        avg_pick_seconds: draft
            .and_then(draft_pick_stats)
            .and_then(|stats| stats.avg_seconds),
        opponents,
        // No colours: reading them decodes every match's events.
        matches: run_match_views(run, &summary, notes_by_match, &HashMap::new()),
        game_wins: summary.game_wins,
        game_losses: summary.game_losses,
        on_play_record: win_loss_view(summary.on_play),
        on_draw_record: win_loss_view(summary.on_draw),
        note,
        linked: true,
        in_league: !is_tournament_run_key(&run.run.key),
        classification: classification_view(summary.classification),
        live_round: summary.live_round,
        avg_match_seconds: summary.avg_match_seconds,
        time_of_day: summary.time_of_day.map(time_of_day_view),
        user_state: run.user_state.map(|user| state_view(user.run_state())),
        auto_state: state_view(run.run.state),
        notes: run.notes.clone(),
    }
}

fn unlinked_row(draft: &Draft) -> LimitedIndexRowView {
    let started = draft_time(draft);
    let state = if draft.state == DraftState::Abandoned {
        "Draft abandoned"
    } else {
        "Unlinked"
    };
    LimitedIndexRowView {
        run_key: None,
        draft_token: Some(draft.token.clone()),
        title: "Draft (league unknown)".to_string(),
        set_code: None,
        kind: LeagueKind::Draft.as_str().to_string(),
        state: state.to_string(),
        state_variant: "warning".to_string(),
        started_at_ms: started,
        wins: 0,
        losses: 0,
        untracked_matches: 0,
        results: vec![None, None, None],
        picks_made: picks_made(Some(draft)),
        picks_expected: draft.picks_expected,
        packs: None,
        deck_registered: false,
        version_count: 0,
        avg_pick_seconds: draft_pick_stats(draft).and_then(|stats| stats.avg_seconds),
        opponents: Vec::new(),
        matches: Vec::new(),
        game_wins: 0,
        game_losses: 0,
        on_play_record: WinLossView { wins: 0, losses: 0 },
        on_draw_record: WinLossView { wins: 0, losses: 0 },
        note: Some("Will link when a match or pool grant arrives".to_string()),
        linked: false,
        in_league: false,
        classification: ClassificationView::Empty,
        live_round: None,
        avg_match_seconds: None,
        time_of_day: None,
        user_state: None,
        // No run to fold: neutral, meaningless while `linked` is false.
        auto_state: state_view(RunState::Active),
        notes: None,
    }
}

/// The run `key` of `data` as the Leagues page's run card (the home
/// dashboard's League in progress card, dashboard redesign spec 4.6): named
/// as the Limited pages name it (`QNW Draft`), with its deck's colours and
/// no deck. Reads the card facts of its registered decks (the colours) and
/// the match notes. `None` for an unknown key.
pub fn run_card(
    conn: &Connection,
    data: &Gathered,
    key: &str,
    tz: Tz,
) -> Result<Option<RunCardView>, StoreError> {
    let Some(limited) = data.run(key) else {
        return Ok(None);
    };
    let mut ids = BTreeSet::new();
    registered_ids(limited, &mut ids);
    let cards = Cards::read(conn, &ids)?;
    let notes = notes_by_match(conn)?;
    Ok(Some(run_card_view(limited, &cards.facts, &notes, tz)))
}

fn run_card_view(
    limited: &LimitedRun,
    cards: &CardFactsMap,
    notes_by_match: &HashMap<String, String>,
    tz: Tz,
) -> RunCardView {
    let summary = summary_of(limited, tz);
    let mut results = Vec::new();
    for result in &summary.results {
        results.push(result.map(mark_view));
    }
    let colours = limited::run_deck_colours(&limited.registered, cards);
    RunCardView {
        key: limited.run.key.clone(),
        source: run_source(&limited.source),
        name: run_title(limited),
        format: kind_label(limited.run.kind).to_string(),
        format_code: limited.run.format.clone(),
        state: state_view(limited.shown_state()),
        user_state: limited.user_state.map(|user| state_view(user.run_state())),
        auto_state: state_view(limited.run.state),
        manual: false,
        started_at_ms: limited.run.started_at,
        dropped_at_ms: limited.run.dropped_at,
        deck: None,
        version_label: None,
        results,
        // No opponent colours: reading them decodes every match's events.
        matches: run_match_views(limited, &summary, notes_by_match, &HashMap::new()),
        notes: limited.notes.clone(),
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
        limited: true,
        colors: colours.as_deref().map(|text| color_list(Some(text))),
    }
}

/// The Limited index (0.x `BuildLimitedIndex`): limited runs that started
/// in the timeframe (of the set and kind asked), and the drafts no run
/// holds (not for sealed; the set filter does not apply to them, as 0.x),
/// newest first, with the KPI strip over the same rows.
pub fn index(
    conn: &Connection,
    query: &LimitedIndexQuery,
    now_ms: i64,
    tz: Tz,
) -> Result<LimitedIndexView, CommandError> {
    let data = gather(conn).map_err(storage)?;
    let set = not_blank(query.set.as_deref());
    let kind = not_blank(query.kind.as_deref()).filter(|kind| kind == "draft" || kind == "sealed");
    let timeframe = not_blank(query.timeframe.as_deref()).unwrap_or_else(|| ALL_TIME.to_string());
    let (range, _) = timeframe_ranges(&timeframe, now_ms, tz);

    let runs: Vec<&LimitedRun> = data
        .runs
        .iter()
        .filter(|limited| set.is_none() || limited.run.set_code == set)
        .filter(|limited| {
            kind.as_deref()
                .is_none_or(|kind| limited.run.kind.as_str() == kind)
        })
        .filter(|limited| in_range(Some(limited.run.started_at), &range))
        .collect();
    let unlinked: Vec<&Draft> = if kind.as_deref() == Some("sealed") {
        Vec::new()
    } else {
        data.unlinked
            .iter()
            .filter(|draft| in_range(draft_time(draft), &range))
            .collect()
    };

    // Card data for the sealed pools (basics are not counted).
    let mut ids = BTreeSet::new();
    for limited in runs.iter().filter(|limited| is_sealed(limited)) {
        registered_ids(limited, &mut ids);
        stated_ids(limited, &mut ids);
    }
    let cards = Cards::read(conn, &ids).map_err(storage)?;
    let notes = notes_by_match(conn).map_err(storage)?;

    let mut rows: Vec<LimitedIndexRowView> = Vec::new();
    for limited in &runs {
        rows.push(run_row(limited, &cards.facts, &notes, tz));
    }
    for draft in &unlinked {
        rows.push(unlinked_row(draft));
    }
    // Stable: newest first, rows without a time last.
    rows.sort_by_key(|row| std::cmp::Reverse(row.started_at_ms));

    let mut index_runs = Vec::new();
    let mut pick_stats = Vec::new();
    for limited in &runs {
        let complete = complete_matches(limited);
        let (wins, losses) = shown_record(limited, &complete);
        index_runs.push(limited::IndexRun {
            set_code: limited.run.set_code.clone(),
            complete: limited.shown_state() == RunState::Complete,
            wins,
            losses,
            matches: complete.len() as u32,
            has_draft: limited.draft.is_some(),
        });
        if let Some(stats) = limited.draft.as_ref().and_then(draft_pick_stats) {
            pick_stats.push(stats);
        }
    }
    for draft in &unlinked {
        if let Some(stats) = draft_pick_stats(draft) {
            pick_stats.push(stats);
        }
    }
    let kpis = limited_index_kpis(&index_runs, unlinked.len() as u32, &pick_stats);

    Ok(LimitedIndexView {
        rows,
        kpis: LimitedKpisView {
            events: kpis.events,
            drafts: kpis.drafts,
            unlinked: kpis.unlinked,
            match_win_pct: kpis.match_win_pct,
            match_wins: kpis.match_wins,
            match_losses: kpis.match_losses,
            avg_wins: kpis.avg_wins,
            avg_losses: kpis.avg_losses,
            completed_runs: kpis.completed_runs,
            most_drafted_set: kpis
                .most_drafted_set
                .map(|code| code_label(&data.runs, &code)),
            most_drafted_count: kpis.most_drafted_count,
            avg_pick_seconds: kpis.avg_pick_seconds,
            indecision_pct: kpis.indecision_pct,
        },
        sets: set_codes(&data),
        filters: LimitedFiltersView {
            set,
            kind,
            timeframe,
        },
    })
}

/// The opponents' colours of a run's complete matches, in the run's match
/// order, for an expanded index row. `None` for an unknown key.
pub fn run_colours(
    conn: &Connection,
    key: &str,
) -> Result<Option<Vec<LimitedMatchColoursView>>, CommandError> {
    let Some(run) = run_by_key(conn, key).map_err(storage)? else {
        return Ok(None);
    };
    let tokens: Vec<String> = complete_matches(&run)
        .iter()
        .map(|row| row.token.clone())
        .collect();
    let mut colors = opponent_colors(conn, &tokens)?;
    let mut views = Vec::new();
    for token in tokens {
        let opponent_colors = colors.remove(&token);
        views.push(LimitedMatchColoursView {
            token,
            opponent_colors,
        });
    }
    Ok(Some(views))
}

/// Every limited run's set code, sorted, with what it reads as.
fn set_codes(data: &Gathered) -> Vec<PickSetOptionView> {
    let codes: BTreeSet<String> = data
        .runs
        .iter()
        .filter_map(|limited| limited.run.set_code.clone())
        .collect();
    codes
        .into_iter()
        .map(|code| PickSetOptionView {
            name: code_label(&data.runs, &code),
            code,
        })
        .collect()
}

// ---- Event -------------------------------------------------------------------

/// The cards an event's art and set name come from: the picks in pick
/// order, distinct; a sealed run with no draft, or a run with a stated pool
/// and no draft, has its pool instead.
fn event_card_ids(run: &LimitedRun, cards_for_pool: &CardFactsMap) -> Vec<u32> {
    match &run.draft {
        Some(draft) => {
            let mut ids: Vec<u32> = Vec::new();
            for pick in &draft.picks {
                if let Some(id) = pick.picked_catalog_id
                    && !ids.contains(&id)
                {
                    ids.push(id);
                }
            }
            ids
        }
        None if is_sealed(run) || run.stated_pool.is_some() => {
            pool_of(run, cards_for_pool).pool.into_keys().collect()
        }
        None => Vec::new(),
    }
}

/// A cover: the art crop URL and its local file.
type Cover = (String, Option<String>);

/// The art crop (and its local file) of the first rare or mythic card with
/// art, else the first card with art.
fn cover_art(ids: &[u32], cards: &Cards) -> Option<Cover> {
    let art = |id: &u32| -> Option<(Option<&str>, Cover)> {
        let row = cards.rows.get(id)?;
        let crop = row.art_crop.clone().filter(|crop| !crop.is_empty())?;
        Some((row.rarity.as_deref(), (crop, row.local_art_crop.clone())))
    };
    let mut first = None;
    for id in ids {
        if let Some((rarity, crop)) = art(id) {
            if matches!(rarity, Some("rare") | Some("mythic")) {
                return Some(crop);
            }
            if first.is_none() {
                first = Some(crop);
            }
        }
    }
    first
}

fn event_view(conn: &Connection, run: &LimitedRun, tz: Tz) -> Result<LimitedEventView, StoreError> {
    let draft = run.draft.as_ref();
    let mut ids = BTreeSet::new();
    registered_ids(run, &mut ids);
    stated_ids(run, &mut ids);
    if let Some(draft) = draft {
        ids.extend(draft.picks.iter().filter_map(|pick| pick.picked_catalog_id));
    }
    let cards = Cards::read(conn, &ids)?;
    let card_ids = event_card_ids(run, &cards.facts);
    let cover = cover_art(&card_ids, &cards);
    // A cube's cards come from many sets: its format names it instead.
    let set_name = format_name(run).or_else(|| {
        card_ids
            .iter()
            .find_map(|id| cards.rows.get(id).and_then(|row| row.set_name.clone()))
    });
    let complete = complete_matches(run);
    let (wins, losses) = shown_record(run, &complete);
    let (state, variant) = state_badge(run.shown_state(), draft, complete.len());

    let date = local_text(run.run.started_at, tz, "%-d %b");
    let mut title = run_title(run);
    if let Some(date) = date {
        title = format!("{title} · {date}");
    }
    let mut subtitle = set_name
        .clone()
        .unwrap_or_else(|| "Unknown set".to_string());
    if let Some(event_id) = run.run.event_id {
        subtitle = format!("{subtitle} · League {event_id}");
    }

    let mut nav = Vec::new();
    for (page, label) in [
        (LimitedPage::Draft, "Draft"),
        (LimitedPage::Deck, "Deck"),
        (LimitedPage::Matches, "Matches"),
        (LimitedPage::Cards, "Cards"),
    ] {
        if page == LimitedPage::Draft && is_sealed(run) {
            continue;
        }
        nav.push(LimitedNavItemView {
            page,
            label: label.to_string(),
        });
    }

    Ok(LimitedEventView {
        key: run.run.key.clone(),
        draft_token: draft.map(|draft| draft.token.clone()),
        title,
        subtitle,
        set_code: run.run.set_code.clone(),
        set_label: set_label(run),
        set_name,
        kind: run.run.kind.as_str().to_string(),
        state: state.to_string(),
        state_variant: variant.to_string(),
        started_at_ms: run.run.started_at,
        started_at_human: local_text(run.run.started_at, tz, "%-d %b %Y · %H:%M"),
        wins,
        losses,
        untracked_matches: untracked_matches(run, &complete),
        picks_made: picks_made(draft),
        picks_expected: picks_expected(draft),
        packs: sealed_packs(run, &cards.facts),
        deck_registered: !run.registered.is_empty(),
        cover_art: cover.as_ref().map(|(crop, _)| crop.clone()),
        local_cover_art: cover.and_then(|(_, local)| local),
        seat_index: draft.and_then(|draft| draft.seat_index),
        seat_count: draft.map_or(8, |draft| seats(draft.seat_count)),
        booster_catalog_id: draft.and_then(|draft| draft.booster_catalog_id),
        draft_state: draft
            .map_or("none", |draft| draft_state_name(draft.state))
            .to_string(),
        nav,
        editable: !run.is_arena(),
        picks_recorded: picks_recorded(run),
    })
}

/// The props every event page shares (0.x `GetLimitedEventSharedProps`).
/// `None` for an unknown key.
pub fn event(
    conn: &Connection,
    key: &str,
    tz: Tz,
) -> Result<Option<LimitedEventView>, CommandError> {
    let Some(run) = run_by_key(conn, key).map_err(storage)? else {
        return Ok(None);
    };
    Ok(Some(event_view(conn, &run, tz).map_err(storage)?))
}

// ---- Draft review ------------------------------------------------------------

/// Picked cards per colour letter (0.x `colorsPicked`): a multicolour card
/// counts once per colour, a card with no colour (or no card data) as `C`.
fn colors_picked(picks: &[Pick], cards: &Cards) -> Vec<ColorCountView> {
    let letters = ['W', 'U', 'B', 'R', 'G', 'C'];
    let mut tally = [0u32; 6];
    for pick in picks {
        let Some(catalog_id) = pick.picked_catalog_id else {
            continue;
        };
        let colors = cards
            .rows
            .get(&catalog_id)
            .and_then(|row| row.colors.as_deref())
            .unwrap_or("");
        let mut any = false;
        for letter in colors.chars() {
            if let Some(index) = letters[..5].iter().position(|known| *known == letter) {
                tally[index] += 1;
                any = true;
            }
        }
        if !any {
            tally[5] += 1;
        }
    }
    let mut counts = Vec::new();
    for (index, count) in tally.iter().enumerate() {
        if *count > 0 {
            counts.push(ColorCountView {
                color: letters[index].to_string(),
                count: *count,
            });
        }
    }
    counts
}

fn pick_view(
    pick: &Pick,
    picks: &[Pick],
    seat_count: u32,
    notes: &BTreeMap<u32, PickNote>,
) -> DraftPickView {
    let timing = pick_timing(pick);
    let wheel = wheel_for_pick(picks, pick, seat_count);
    let mut reservations = Vec::new();
    for reservation in &pick.reservations {
        reservations.push(ReservationView {
            catalog_id: reservation.catalog_id,
            at_seconds: pick
                .shown_at_ms
                .map(|shown| (reservation.at_ms - shown).abs() / 1000),
        });
    }
    let note = notes.get(&pick.ordinal);
    DraftPickView {
        ordinal: pick.ordinal,
        pack_number: pick.pack_number,
        pick_number: pick.pick_number,
        label: format!("P{}p{}", pick.pack_number, pick.pick_number),
        pack_id: pick.pack_id,
        direction: pick.direction,
        available: pick.cards_available.clone(),
        picked_catalog_id: pick.picked_catalog_id,
        reservations,
        elapsed_seconds: timing.elapsed_seconds,
        margin_seconds: timing.margin_seconds,
        indecisive: timing.indecisive,
        shown_at_ms: pick.shown_at_ms,
        deadline_at_ms: pick.deadline_at_ms,
        picked_at_ms: pick.picked_at_ms,
        note: note.map(|note| note.note.clone()),
        note_saved_at_ms: note.map(|note| note.updated_at_ms),
        wheel_return_ordinal: wheel.as_ref().map(|wheel| wheel.return_ordinal),
        wheeled_ids: wheel
            .as_ref()
            .map(|wheel| wheel.survived.clone())
            .unwrap_or_default(),
        taken_ids: wheel.map(|wheel| wheel.taken).unwrap_or_default(),
    }
}

/// The whole draft review (0.x `BuildDraftReview`).
fn review(conn: &Connection, draft: &Draft) -> Result<DraftReviewView, StoreError> {
    let seat_count = seats(draft.seat_count);
    let mut ids = BTreeSet::new();
    draft_ids(draft, &mut ids);
    let cards = Cards::read(conn, &ids)?;
    let notes = pick_note_rows(conn, &draft.token)?;
    let seen = seen_wheel(&draft.picks, seat_count);
    let timings = timing_summary(&draft.picks);

    let mut picks = Vec::new();
    for pick in &draft.picks {
        picks.push(pick_view(pick, &draft.picks, seat_count, &notes));
    }
    let mut signals = Vec::new();
    for signal in draft_signals(&seen, &cards.facts) {
        signals.push(ColorSignalView {
            color: signal.color.to_string(),
            wheeled: signal.wheeled,
            seen_twice: signal.seen_twice,
            score: signal.score,
            share: signal.share,
        });
    }
    let mut seen_views = BTreeMap::new();
    for (catalog_id, facts) in &seen {
        seen_views.insert(
            *catalog_id,
            SeenWheelView {
                seen_count: facts.seen_count,
                first_seen_ordinal: facts.first_seen_ordinal,
                wheeled: facts.wheeled,
                wheeled_to_me: facts.wheeled_to_me,
                picked_count: facts.picked_count,
                passed_count: facts.passed_count,
            },
        );
    }
    let duration_minutes = match (draft.started_at_ms, draft.ended_at_ms) {
        (Some(started), Some(ended)) => Some(minutes_between(started, ended)),
        _ => None,
    };

    Ok(DraftReviewView {
        header: DraftHeaderView {
            seat_index: draft.seat_index,
            seat_count,
            booster_catalog_id: draft.booster_catalog_id,
            pack_size: draft.pack_size,
            picks_made: picks_made(Some(draft)),
            picks_expected: draft.picks_expected,
            started_at_ms: draft.started_at_ms,
            ended_at_ms: draft.ended_at_ms,
            duration_minutes,
            avg_pick_seconds: timings.avg_seconds,
            avg_margin_seconds: timings.avg_margin_seconds,
            indecisive_count: timings.indecisive_count as u32,
            fastest_pack: timings.fastest_pack,
            slowest_pack: timings.slowest_pack,
            colors_picked: colors_picked(&draft.picks, &cards),
            state: draft_state_name(draft.state).to_string(),
        },
        picks,
        signals,
        seen_wheel: seen_views,
        cards: cards.views(&ids),
    })
}

/// The Draft page (0.x `DraftController`): sealed goes to the Deck page;
/// `pick` selects an ordinal the draft has, else its first pick.
pub fn draft_page(
    conn: &Connection,
    key: &str,
    pick: Option<u32>,
) -> Result<Option<LimitedDraftView>, CommandError> {
    let Some(run) = run_by_key(conn, key).map_err(storage)? else {
        return Ok(None);
    };
    if is_sealed(&run) {
        return Ok(Some(LimitedDraftView {
            redirect_to_deck: true,
            selected_ordinal: None,
            review: None,
        }));
    }
    let Some(draft) = run.draft.as_ref() else {
        return Ok(Some(LimitedDraftView {
            redirect_to_deck: false,
            selected_ordinal: None,
            review: None,
        }));
    };
    let ordinals: Vec<u32> = draft.picks.iter().map(|pick| pick.ordinal).collect();
    let selected = match pick {
        Some(ordinal) if ordinals.contains(&ordinal) => Some(ordinal),
        _ => ordinals.first().copied(),
    };
    Ok(Some(LimitedDraftView {
        redirect_to_deck: false,
        selected_ordinal: selected,
        review: Some(review(conn, draft).map_err(storage)?),
    }))
}

// ---- Cross-draft card stats --------------------------------------------------

/// The cross-draft stats of `run` (0.x `ComputeCrossDraftCardStats`): the
/// drafts of the other runs of its set (drafts no run holds are left out),
/// with card data for every card they and `extra` name.
fn cross_stats(
    conn: &Connection,
    data: &Gathered,
    run: &LimitedRun,
    extra: &BTreeSet<u32>,
) -> Result<(BTreeMap<String, CrossDraftStats>, Cards), StoreError> {
    let set_code = run.run.set_code.as_deref();
    let others: Vec<&LimitedRun> = data
        .runs
        .iter()
        .filter(|other| other.run.key != run.run.key)
        .filter(|other| set_code.is_some() && other.run.set_code.as_deref() == set_code)
        .filter(|other| other.draft.is_some())
        .collect();
    let mut ids = extra.clone();
    for other in &others {
        if let Some(draft) = &other.draft {
            draft_ids(draft, &mut ids);
        }
        if let Some(last) = other.registered.last() {
            ids.extend(last.cards.iter().map(|card| card.catalog_id));
        }
    }
    let cards = Cards::read(conn, &ids)?;
    let mut other_drafts = Vec::new();
    for other in &others {
        if let Some(draft) = &other.draft {
            other_drafts.push(OtherDraft {
                set_code: other.run.set_code.as_deref(),
                draft,
                last_registered: other.registered.last().map(|deck| deck.cards.as_slice()),
            });
        }
    }
    let stats = cross_draft_card_stats(set_code, &other_drafts, &cards.facts);
    Ok((stats, cards))
}

fn cross_stats_view(stats: &CrossDraftStats) -> CrossDraftStatsView {
    CrossDraftStatsView {
        oracle_id: stats.oracle_id.clone(),
        drafts: stats.drafts,
        times_taken: stats.times_taken,
        avg_ordinal: stats.avg_ordinal,
        times_passed: stats.times_passed,
        times_wheeled: stats.times_wheeled,
        made_deck: stats.made_deck,
    }
}

/// The cross-draft panel of every card the run's draft and registered decks
/// name (0.x's deferred `crossDraft` prop, looked up by the card's oracle
/// id), from one fold of the other drafts instead of one per card. `None`
/// for an unknown key.
pub fn cross_drafts(conn: &Connection, key: &str) -> Result<Option<CrossDraftsView>, CommandError> {
    let data = gather(conn).map_err(storage)?;
    let Some(run) = data.run(key) else {
        return Ok(None);
    };
    let mut ids = BTreeSet::new();
    registered_ids(run, &mut ids);
    stated_ids(run, &mut ids);
    if let Some(draft) = &run.draft {
        draft_ids(draft, &mut ids);
    }
    let (stats, cards) = cross_stats(conn, &data, run, &ids).map_err(storage)?;
    let mut views = Vec::new();
    for catalog_id in ids {
        let oracle_id = cards
            .rows
            .get(&catalog_id)
            .and_then(|row| row.oracle_id.clone())
            .filter(|oracle| !oracle.is_empty());
        let found = oracle_id
            .as_ref()
            .and_then(|oracle| stats.get(oracle))
            .map(cross_stats_view);
        views.push(CrossDraftView {
            set_code: run.run.set_code.clone(),
            catalog_id,
            oracle_id,
            stats: found,
        });
    }
    Ok(Some(CrossDraftsView {
        set_code: run.run.set_code.clone(),
        cards: views,
    }))
}

// ---- Deck --------------------------------------------------------------------

fn quantity_views(list: &[limited::CardQuantity]) -> Vec<CardQuantityView> {
    list.iter()
        .map(|card| CardQuantityView {
            catalog_id: card.catalog_id,
            quantity: card.quantity,
        })
        .collect()
}

fn diff_view(diff: &limited::ZoneDiff) -> ZoneDiffView {
    ZoneDiffView {
        added: quantity_views(&diff.added),
        removed: quantity_views(&diff.removed),
    }
}

fn pool_view(groups: &[limited::PoolGroup]) -> PoolView {
    let mut views = Vec::new();
    for group in groups {
        views.push(PoolGroupView {
            key: group.key.clone(),
            label: group.label.clone(),
            count: group.count,
            cards: group
                .cards
                .iter()
                .map(|card| PoolCardView {
                    catalog_id: card.catalog_id,
                    quantity: card.quantity,
                    added: card.added,
                    status: card.status.as_str().to_string(),
                    main_qty: card.main_qty,
                    side_qty: card.side_qty,
                })
                .collect(),
        });
    }
    PoolView { groups: views }
}

/// A game board's local deck as registered card rows; `None` without
/// board data.
fn game_deck(board: &GameBoard) -> Option<Vec<RegisteredCard>> {
    if board.local_deck.is_empty() {
        return None;
    }
    Some(
        board
            .local_deck
            .iter()
            .map(|card| RegisteredCard {
                catalog_id: card.catalog_id,
                quantity: card.quantity,
                sideboard: card.sideboard,
            })
            .collect(),
    )
}

/// The Deck page (0.x `BuildDeckEvolution`). `None` for an unknown key.
pub fn deck_page(conn: &Connection, key: &str) -> Result<Option<LimitedDeckView>, CommandError> {
    let Some(run) = run_by_key(conn, key).map_err(storage)? else {
        return Ok(None);
    };
    let run = &run;

    let mut completed = Vec::new();
    let mut ids = BTreeSet::new();
    for row in complete_matches(run) {
        let boards = game_boards(conn, &row.token, false)?;
        let mut game_decks = Vec::new();
        for board in &boards {
            ids.extend(board.local_deck.iter().map(|card| card.catalog_id));
            game_decks.push(game_deck(board));
        }
        completed.push(CompletedMatch {
            token: row.token.clone(),
            won: outcome_mark(row).map(|mark| mark == MarkView::W),
            opponent_name: row.opponent.clone(),
            game_decks,
        });
    }
    registered_ids(run, &mut ids);
    stated_ids(run, &mut ids);
    if let Some(draft) = &run.draft {
        ids.extend(draft.picks.iter().filter_map(|pick| pick.picked_catalog_id));
    }
    let cards = Cards::read(conn, &ids).map_err(storage)?;
    let pool = pool_of(run, &cards.facts);
    let match_order: Vec<String> = matches_by_start(run)
        .iter()
        .map(|row| row.token.clone())
        .collect();
    let evolution = deck_evolution(&EvolutionInput {
        pool: &pool,
        decks: &run.registered,
        match_order: &match_order,
        completed: &completed,
        cards: &cards.facts,
    });

    // The cards the page names: the pool and every registered or played
    // card.
    let mut named: BTreeSet<u32> = pool.pool.keys().copied().collect();
    named.extend(ids.iter().copied());

    let summary = &evolution.summary;
    let mut versions = Vec::new();
    for version in &evolution.versions {
        versions.push(RegisteredVersionView {
            index: version.index,
            signature: version.signature.clone(),
            captured_at_ms: version.captured_at_ms,
            match_tokens: version.match_tokens.clone(),
            match_labels: version.match_labels.clone(),
            main: version.main,
            side: version.side,
            colors: version.colors.clone(),
            diff_main: diff_view(&version.diff_main),
            diff_side: diff_view(&version.diff_side),
            is_current: version.is_current,
            pool: pool_view(&version.pool_groups),
            main_cards: quantity_views(&version.main_cards),
            side_cards: quantity_views(&version.side_cards),
        });
    }
    let mut games = Vec::new();
    for changes in &evolution.games {
        games.push(MatchChangesView {
            match_token: changes.match_token.clone(),
            label: changes.label.clone(),
            opponent_name: changes.opponent_name.clone(),
            result: changes.result.clone(),
            games: changes
                .games
                .iter()
                .map(|game| GameChangeView {
                    number: game.number,
                    added: quantity_views(&game.added),
                    removed: quantity_views(&game.removed),
                    note: game.note.clone(),
                })
                .collect(),
        });
    }

    Ok(Some(LimitedDeckView {
        summary: EvolutionSummaryView {
            drafted: summary.drafted,
            added: summary.added,
            main_spells: summary.main_spells,
            basics: summary.basics,
            sideboard: summary.sideboard,
            version_count: summary.version_count as u32,
            first_registered_at_ms: summary.first_registered_at_ms,
            last_registered_at_ms: summary.last_registered_at_ms,
        },
        versions,
        pool: pool_view(&evolution.pool_groups),
        games,
        cards: cards.views(&named),
    }))
}

// ---- Cards page --------------------------------------------------------------

/// Per oracle id, the games a card was cast in and how those games went
/// (0.x `GetCardGameStats` `castGames`, `castWon`, `castLost`): every game
/// of the run's complete matches whose game log shows the player casting
/// it. A game not won counts as lost, as 0.x.
fn cast_stats(
    boards: &[(String, Vec<GameBoard>)],
    cards: &CardFactsMap,
) -> BTreeMap<String, CastStats> {
    let mut stats: BTreeMap<String, CastStats> = BTreeMap::new();
    for (_, games) in boards {
        for game in games {
            let mut oracles: BTreeSet<String> = BTreeSet::new();
            for catalog_id in &game.cast {
                if let Some(oracle) = cards
                    .get(catalog_id)
                    .and_then(|card| card.oracle_id.clone())
                    .filter(|oracle| !oracle.is_empty())
                {
                    oracles.insert(oracle);
                }
            }
            for oracle in oracles {
                let entry = stats.entry(oracle).or_default();
                entry.games_cast += 1;
                if game_won(&game.result) == Some(true) {
                    entry.cast_won += 1;
                } else {
                    entry.cast_lost += 1;
                }
            }
        }
    }
    stats
}

/// The Cards page (0.x `BuildLimitedCardRows`). `None` for an unknown key.
pub fn cards_page(conn: &Connection, key: &str) -> Result<Option<LimitedCardsView>, CommandError> {
    // The other drafts of the set are read too (cross-draft stats).
    let data = gather(conn).map_err(storage)?;
    let Some(run) = data.run(key) else {
        return Ok(None);
    };

    let mut boards: Vec<(String, Vec<GameBoard>)> = Vec::new();
    let mut ids = BTreeSet::new();
    for row in complete_matches(run) {
        let games = game_boards(conn, &row.token, true)?;
        for game in &games {
            ids.extend(game.cast.iter().copied());
        }
        boards.push((row.token.clone(), games));
    }
    registered_ids(run, &mut ids);
    if let Some(draft) = &run.draft {
        draft_ids(draft, &mut ids);
    }
    let (cross, cards) = if run.draft.is_some() {
        cross_stats(conn, &data, run, &ids).map_err(storage)?
    } else {
        (BTreeMap::new(), Cards::read(conn, &ids).map_err(storage)?)
    };
    let casts = cast_stats(&boards, &cards.facts);
    let games_played: usize = run.matches.iter().map(|row| row.games.len()).sum();

    let built = limited_card_rows(&CardRowsInput {
        is_sealed: is_sealed(run),
        draft: run.draft.as_ref(),
        decks: &run.registered,
        stated_pool: run.stated_pool.as_deref(),
        cross_draft: &cross,
        cast_stats: &casts,
        games_played: games_played as u32,
        cards: &cards.facts,
    });

    let mut rows = Vec::new();
    let mut named = BTreeSet::new();
    for row in &built.rows {
        named.insert(row.catalog_id);
        rows.push(LimitedCardRowView {
            catalog_id: row.catalog_id,
            oracle_id: row.oracle_id.clone(),
            ordinals: row.ordinals.clone(),
            labels: row.labels.clone(),
            status: row.status.as_str().to_string(),
            added: row.added,
            games_cast: row.games_cast,
            cast_won: row.cast_won,
            cast_lost: row.cast_lost,
            win_pct_cast: row.win_pct_cast,
            seen_count: row.seen_count,
            wheeled: row.wheeled,
            prior_taken: row.prior_taken,
            prior_avg_ordinal: row.prior_avg_ordinal,
            prior_wheeled: row.prior_wheeled,
            prior_drafts: row.prior_drafts,
        });
    }
    Ok(Some(LimitedCardsView {
        rows,
        summary: CardRowsSummaryView {
            distinct: built.summary.distinct as u32,
            games: built.summary.games,
            other_drafts: built.summary.other_drafts,
        },
        cards: cards.views(&named),
    }))
}

// ---- Matches -----------------------------------------------------------------

/// The list row of a match, `None` for a hidden one or one the list cannot
/// show.
fn match_summary(conn: &Connection, token: &str) -> Result<Option<MatchSummary>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "{}\nWHERE m.token = ?1",
        store::read::MATCH_ROW_SELECT
    ))?;
    let mut rows = stmt.query_map([token], store::read::match_row)?;
    match rows.next() {
        Some(row) => Ok(from_row(&row?)),
        None => Ok(None),
    }
}

/// The Matches page (0.x `Limited\MatchesController`). `None` for an
/// unknown key.
pub fn matches_page(
    conn: &Connection,
    key: &str,
    tz: Tz,
) -> Result<Option<LimitedMatchesView>, CommandError> {
    let Some(run) = run_by_key(conn, key).map_err(storage)? else {
        return Ok(None);
    };
    let run = &run;
    let complete = complete_matches(run);
    let tokens: Vec<String> = complete.iter().map(|row| row.token.clone()).collect();
    let colors = opponent_colors(conn, &tokens)?;
    let mut matches = Vec::new();
    for token in &tokens {
        if let Some(mut summary) = match_summary(conn, token).map_err(storage)? {
            summary.opponent_colors = colors.get(token).cloned();
            matches.push(summary);
        }
    }

    let (wins, losses) = shown_record(run, &complete);
    let summary = summary_of(run, tz);
    let first = complete.first();
    let last = complete.last();
    let queued_at_ms = first.map(|row| row.started_at);
    let finished_at_ms = last.map(|row| row.ended_at.unwrap_or(row.started_at));
    let total_minutes = match (queued_at_ms, finished_at_ms) {
        (Some(from), Some(to)) => Some(minutes_between(from, to)),
        _ => None,
    };
    Ok(Some(LimitedMatchesView {
        matches,
        kpis: LimitedMatchesKpisView {
            wins,
            losses,
            game_wins: summary.game_wins,
            game_losses: summary.game_losses,
            avg_match_seconds: summary.avg_match_seconds,
            on_play: win_loss_view(summary.on_play),
            on_draw: win_loss_view(summary.on_draw),
            queued_at_ms,
            finished_at_ms,
            total_minutes,
        },
    }))
}

/// One match of the run (0.x `Limited\MatchController`): the match detail
/// page, `None` when the key is unknown or the match is not one of the
/// run's. `dat` is the match's `.dat` game log entries.
pub fn match_page(
    conn: &Connection,
    key: &str,
    token: &str,
    dat: &[game_detail::GameLogEntry],
) -> Result<Option<MatchDetail>, CommandError> {
    if !run_holds(conn, key, token)? {
        return Ok(None);
    }
    crate::matches::detail(conn, token, dat)
}

/// Whether the limited run `key` holds the match `token`.
fn run_holds(conn: &Connection, key: &str, token: &str) -> Result<bool, CommandError> {
    let run = run_by_key(conn, key).map_err(storage)?;
    Ok(run.is_some_and(|run| run.run.match_tokens.iter().any(|member| member == token)))
}

// ---- Pick notes --------------------------------------------------------------

/// Save a pick note through the tracker (0.x `UpdatePickNoteController`):
/// trimmed, at most 2000 characters, empty deletes it; the pick must exist.
pub fn set_note(
    state: &AppState,
    draft_token: &str,
    ordinal: u32,
    note: &str,
    now_ms: i64,
) -> Result<(), CommandError> {
    let editor = state.tracker_part(tracker::TrackerHandle::draft_note_editor)?;
    let outcome = editor
        .edit(DraftNoteEdit::Set {
            draft_token: draft_token.to_string(),
            ordinal,
            note: note.to_string(),
            now_ms,
        })
        .map_err(CommandError::Tracker)?;
    match outcome {
        DraftNoteOutcome::Done => Ok(()),
        refused => Err(CommandError::Refused(
            refused.refusal().unwrap_or("Nothing changed.").to_string(),
        )),
    }
}

// ---- Draft notes window ------------------------------------------------------

/// The live draft (0.x `SyncDraftNotesWindowVisibility::liveDraft`): the
/// newest connecting or picking draft, else the draft that finished most
/// recently, within the last 30 seconds.
pub(crate) fn live_draft(data: &Gathered, now_ms: i64) -> Option<(&Draft, Option<&str>)> {
    let mut drafts: Vec<(&Draft, Option<&str>)> = Vec::new();
    for limited in &data.runs {
        if let Some(draft) = &limited.draft {
            drafts.push((draft, Some(limited.run.key.as_str())));
        }
    }
    for draft in &data.unlinked {
        drafts.push((draft, None));
    }
    let live = drafts
        .iter()
        .filter(|(draft, _)| matches!(draft.state, DraftState::Connecting | DraftState::Picking))
        .max_by_key(|(draft, _)| draft_time(draft));
    if let Some(found) = live {
        return Some(*found);
    }
    drafts
        .iter()
        .filter(|(draft, _)| draft.state == DraftState::Finished)
        .filter(|(draft, _)| {
            draft
                .ended_at_ms
                .is_some_and(|ended| ended >= now_ms - NOTES_GRACE_MS)
        })
        .max_by_key(|(draft, _)| draft.ended_at_ms)
        .copied()
}

/// The draft notes window's data (0.x `BuildDraftNotes`), `None` when no
/// draft is live.
pub fn notes_window(
    conn: &Connection,
    now_ms: i64,
) -> Result<Option<DraftNotesView>, CommandError> {
    Ok(live_notes(conn, now_ms)?.map(|live| live.view))
}

/// The live draft's notes window data, and for a finished draft the first
/// moment it is no longer live, 30 seconds after it ended (the overlay
/// worker's timer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveNotes {
    pub view: DraftNotesView,
    pub grace_ends_at_ms: Option<i64>,
}

/// `notes_window` with the grace end. `None` when no draft is live.
pub(crate) fn live_notes(
    conn: &Connection,
    now_ms: i64,
) -> Result<Option<LiveNotes>, CommandError> {
    let data = gather(conn).map_err(storage)?;
    let Some((draft, run_key)) = live_draft(&data, now_ms) else {
        return Ok(None);
    };
    let ids: BTreeSet<u32> = draft
        .picks
        .iter()
        .filter_map(|pick| pick.picked_catalog_id)
        .collect();
    let cards = Cards::read(conn, &ids).map_err(storage)?;
    let notes = pick_note_rows(conn, &draft.token).map_err(storage)?;
    let mut picks = Vec::new();
    for pick in &draft.picks {
        picks.push(DraftNotePickView {
            ordinal: pick.ordinal,
            label: format!("P{}p{}", pick.pack_number, pick.pick_number),
            cards_in_pack: pick.cards_available.len() as u32,
            deadline_at_ms: pick.deadline_at_ms,
            picked_catalog_id: pick.picked_catalog_id,
            picked_name: pick
                .picked_catalog_id
                .map(|id| cards.name(id).unwrap_or_else(|| format!("#{id}"))),
            note: notes.get(&pick.ordinal).map(|note| note.note.clone()),
            note_saved_at_ms: notes.get(&pick.ordinal).map(|note| note.updated_at_ms),
        });
    }
    picks.sort_by_key(|pick| pick.ordinal);
    let grace_ends_at_ms = match draft.state {
        // Live through the grace's last millisecond (`live_draft`).
        DraftState::Finished => draft.ended_at_ms.map(|ended| ended + NOTES_GRACE_MS + 1),
        _ => None,
    };
    Ok(Some(LiveNotes {
        view: DraftNotesView {
            draft_token: draft.token.clone(),
            run_key: run_key.map(str::to_string),
            state: draft_state_name(draft.state).to_string(),
            current_ordinal: picks.last().map(|pick| pick.ordinal),
            picks,
        },
        grace_ends_at_ms,
    }))
}

// ---- Home dashboard ----------------------------------------------------------

/// Every limited run of `data` as the home dashboard reads it (dashboard
/// redesign spec 4.2 to 4.7), newest first. The card facts are read once:
/// every run's registered cards (its colours), and the picks of the runs a
/// card shows with art (`shown_runs`).
pub fn dashboard_runs(
    conn: &Connection,
    data: &Gathered,
    tz: Tz,
) -> Result<Vec<DashboardLeagueRun>, StoreError> {
    // A tournament draft shows on the Limited pages only.
    let league_runs: Vec<&LimitedRun> = data
        .runs
        .iter()
        .filter(|limited| !is_tournament_run_key(&limited.run.key))
        .collect();
    let shown = shown_runs(&league_runs);
    let mut ids = BTreeSet::new();
    for (index, limited) in league_runs.iter().enumerate() {
        registered_ids(limited, &mut ids);
        if shown.contains(&index)
            && let Some(draft) = &limited.draft
        {
            ids.extend(draft.picks.iter().filter_map(|pick| pick.picked_catalog_id));
        }
    }
    let cards = Cards::read(conn, &ids)?;
    let mut set_names: HashMap<String, Option<String>> = HashMap::new();
    let mut runs = Vec::new();
    for (index, limited) in league_runs.iter().copied().enumerate() {
        let set_name = match (&limited.run.set_code, format_name(limited)) {
            (_, Some(name)) => Some(name),
            (Some(code), None) => {
                if !set_names.contains_key(code) {
                    set_names.insert(code.clone(), store::limited::set_name(conn, code)?);
                }
                set_names.get(code).cloned().flatten()
            }
            (None, None) => None,
        };
        runs.push(dashboard_run(
            limited,
            &cards,
            set_name,
            shown.contains(&index),
            tz,
        ));
    }
    Ok(runs)
}

/// The runs whose art and first pick the dashboard shows: the two newest
/// (Recent drafts), the newest Active one and the newest other one (League
/// in progress). `runs` is newest first.
fn shown_runs(runs: &[&LimitedRun]) -> BTreeSet<usize> {
    let mut shown: BTreeSet<usize> = (0..runs.len().min(2)).collect();
    if let Some(index) = runs
        .iter()
        .position(|limited| limited.run.state == RunState::Active)
    {
        shown.insert(index);
    }
    if let Some(index) = runs
        .iter()
        .position(|limited| limited.run.state != RunState::Active)
    {
        shown.insert(index);
    }
    shown
}

fn dashboard_run(
    limited: &LimitedRun,
    cards: &Cards,
    set_name: Option<String>,
    with_art: bool,
    tz: Tz,
) -> DashboardLeagueRun {
    let complete = complete_matches(limited);
    let (wins, losses) = shown_record(limited, &complete);
    let summary = summary_of(limited, tz);
    let rounds = limited.run.round_count();
    let colours = limited::run_deck_colours(&limited.registered, &cards.facts);
    let colour_name = colours.as_deref().map(limited::colour_combination_name);
    let set_label = set_name
        .clone()
        .or_else(|| limited.run.set_code.clone())
        .unwrap_or_else(|| "Unknown set".to_string());
    let subject = match &colour_name {
        Some(name) => format!("{set_label} · {name}"),
        None => set_label,
    };
    let (cover, first_pick) = if with_art {
        (
            cover_art(&event_card_ids(limited, &cards.facts), cards),
            first_pick(limited, cards),
        )
    } else {
        (None, None)
    };
    let last_played_at = limited
        .matches
        .iter()
        .map(|row| row.started_at)
        .max()
        .unwrap_or(limited.run.started_at);
    DashboardLeagueRun {
        key: limited.run.key.clone(),
        kind: limited.run.kind,
        rounds: rounds as u32,
        wins,
        losses,
        state: limited.run.state,
        // What the card shows: the player's status, else the fold's.
        shown_state: limited.shown_state(),
        name: limited.run.name.clone(),
        subject,
        started_at: limited.run.started_at,
        last_played_at,
        format: limited.run.format.clone(),
        deck_id: None,
        set_code: limited.run.set_code.clone(),
        set_name,
        colors: colours.as_deref().map(|text| color_list(Some(text))),
        colour_name,
        cover_art: cover.as_ref().map(|(crop, _)| crop.clone()),
        local_cover_art: cover.and_then(|(_, local)| local),
        game_wins: summary.game_wins,
        game_losses: summary.game_losses,
        first_pick,
        match_tokens: limited
            .matches
            .iter()
            .map(|row| row.token.clone())
            .collect(),
    }
}

/// The draft's first pick by name (P1p1); `Sealed` for a sealed run; `None`
/// for a draft with no pick yet.
fn first_pick(limited: &LimitedRun, cards: &Cards) -> Option<String> {
    if is_sealed(limited) {
        return Some("Sealed".to_string());
    }
    let draft = limited.draft.as_ref()?;
    let catalog_id = draft.picks.iter().find_map(|pick| pick.picked_catalog_id)?;
    Some(limited::card_name(&cards.facts, catalog_id))
}

/// The cards Most picked shows (spec 4.8: a 3 x 2 grid).
pub const MOST_PICKED: usize = 6;

/// Most picked (spec 4.8): the six most picked cards over the drafts of
/// `set_code`'s runs (every run's for `None`), most first, then by catalog
/// id. The timeframe is ignored.
pub fn most_picked(
    conn: &Connection,
    data: &Gathered,
    set_code: Option<&str>,
) -> Result<LimitedPicksView, StoreError> {
    let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
    for limited in &data.runs {
        if set_code.is_some_and(|code| limited.run.set_code.as_deref() != Some(code)) {
            continue;
        }
        for pick in limited.draft.iter().flat_map(|draft| draft.picks.iter()) {
            if let Some(catalog_id) = pick.picked_catalog_id {
                *counts.entry(catalog_id).or_insert(0) += 1;
            }
        }
    }
    let mut ranked: Vec<(u32, u32)> = counts.into_iter().collect();
    // Most picked first; the map's order (catalog id) breaks ties.
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    ranked.truncate(MOST_PICKED);
    let ids: BTreeSet<u32> = ranked.iter().map(|(id, _)| *id).collect();
    let cards = Cards::read(conn, &ids)?;
    let mut picks = Vec::new();
    for (catalog_id, count) in ranked {
        let row = cards.rows.get(&catalog_id);
        picks.push(LimitedPickCountView {
            catalog_id,
            name: row.and_then(|row| row.name.clone()),
            image: row.and_then(|row| row.image.clone()),
            local_image: row.and_then(|row| row.local_image.clone()),
            count,
        });
    }
    let set_name = match set_code {
        Some(code) => code_name(conn, &data.runs, code)?,
        None => None,
    };
    Ok(LimitedPicksView {
        set_code: set_code.map(str::to_string),
        set_name,
        picks,
    })
}

/// The sets Most picked can switch to: those of runs with a pick, newest
/// run first, each once.
pub fn picked_sets(
    conn: &Connection,
    data: &Gathered,
) -> Result<Vec<PickSetOptionView>, StoreError> {
    let mut sets: Vec<PickSetOptionView> = Vec::new();
    for limited in &data.runs {
        let Some(code) = &limited.run.set_code else {
            continue;
        };
        let picked = limited.draft.as_ref().is_some_and(|draft| {
            draft
                .picks
                .iter()
                .any(|pick| pick.picked_catalog_id.is_some())
        });
        if !picked || sets.iter().any(|set| &set.code == code) {
            continue;
        }
        let name = code_name(conn, &data.runs, code)?.unwrap_or_else(|| code.clone());
        sets.push(PickSetOptionView {
            code: code.clone(),
            name,
        });
    }
    Ok(sets)
}

// ---- Commands ----------------------------------------------------------------

/// Async: it reads every limited run, which must not block the window.
#[tauri::command]
#[specta::specta]
pub async fn limited_index(
    app: AppHandle,
    query: LimitedIndexQuery,
) -> Result<LimitedIndexView, CommandError> {
    let _timer = Timer::start("limited_index");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        index(
            &lock(&state.reader),
            &query,
            crate::wall_clock_ms(),
            state.timezone,
        )
    })
    .await
}

/// The opponents' colours of a run's matches, for an expanded index row;
/// `None` for an unknown run. Async: it reads every match's games, which
/// must not block the window.
#[tauri::command]
#[specta::specta]
pub async fn limited_run_colours(
    app: AppHandle,
    key: String,
) -> Result<Option<Vec<LimitedMatchColoursView>>, CommandError> {
    let _timer = Timer::start("limited_run_colours");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        run_colours(&lock(&state.reader), &key)
    })
    .await
}

/// `key`: a run key. `None` for an unknown run.
#[tauri::command]
#[specta::specta]
pub fn limited_event(
    key: String,
    startup: State<'_, Startup>,
) -> Result<Option<LimitedEventView>, CommandError> {
    let _timer = Timer::start("limited_event");
    let state = ready(&startup)?;
    event(&lock(&state.reader), &key, state.timezone)
}

/// `pick`: the ordinal to select.
#[tauri::command]
#[specta::specta]
pub fn limited_draft(
    key: String,
    pick: Option<u32>,
    startup: State<'_, Startup>,
) -> Result<Option<LimitedDraftView>, CommandError> {
    let _timer = Timer::start("limited_draft");
    let state = ready(&startup)?;
    draft_page(&lock(&state.reader), &key, pick)
}

#[tauri::command]
#[specta::specta]
pub fn limited_cross_drafts(
    key: String,
    startup: State<'_, Startup>,
) -> Result<Option<CrossDraftsView>, CommandError> {
    let _timer = Timer::start("limited_cross_drafts");
    let state = ready(&startup)?;
    cross_drafts(&lock(&state.reader), &key)
}

#[tauri::command]
#[specta::specta]
pub fn limited_deck(
    key: String,
    startup: State<'_, Startup>,
) -> Result<Option<LimitedDeckView>, CommandError> {
    let _timer = Timer::start("limited_deck");
    let state = ready(&startup)?;
    deck_page(&lock(&state.reader), &key)
}

#[tauri::command]
#[specta::specta]
pub fn limited_cards(
    key: String,
    startup: State<'_, Startup>,
) -> Result<Option<LimitedCardsView>, CommandError> {
    let _timer = Timer::start("limited_cards");
    let state = ready(&startup)?;
    cards_page(&lock(&state.reader), &key)
}

#[tauri::command]
#[specta::specta]
pub fn limited_matches(
    key: String,
    startup: State<'_, Startup>,
) -> Result<Option<LimitedMatchesView>, CommandError> {
    let _timer = Timer::start("limited_matches");
    let state = ready(&startup)?;
    matches_page(&lock(&state.reader), &key, state.timezone)
}

/// The match detail of one of the run's matches. Async: it searches MTGO's
/// data folder for the game log, which must not block the window.
#[tauri::command]
#[specta::specta]
pub async fn limited_match(
    app: AppHandle,
    key: String,
    token: String,
) -> Result<Option<MatchDetail>, CommandError> {
    let _timer = Timer::start("limited_match");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let member = run_holds(&lock(&state.reader), &key, &token)?;
        if !member {
            return Ok(None);
        }
        let dat = crate::matches::match_dat_entries(state, &token)?;
        crate::matches::detail(&lock(&state.reader), &token, &dat)
    })
    .await
}

/// Save a pick's note (blank deletes it). Async so the wait for the
/// tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn set_pick_note(
    app: AppHandle,
    draft_token: String,
    ordinal: u32,
    note: String,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_pick_note");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_note(
            ready(&startup)?,
            &draft_token,
            ordinal,
            &note,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// The live draft for the notes window (#8 shows it); `None` when no draft
/// is live.
#[tauri::command]
#[specta::specta]
pub fn draft_notes(startup: State<'_, Startup>) -> Result<Option<DraftNotesView>, CommandError> {
    let _timer = Timer::start("draft_notes");
    let state = ready(&startup)?;
    notes_window(&lock(&state.reader), crate::wall_clock_ms())
}

#[cfg(test)]
mod tests;
