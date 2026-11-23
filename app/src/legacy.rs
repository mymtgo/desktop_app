//! Importing a MyMTGO 0.x database from the app (importer spec section 7). A
//! background thread reads 0.x and sends batches to the tracker's writer, the
//! only writer, while live tracking keeps running.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use importer::Skipped;
use mtgo_match::panic_message;
use serde::{Deserialize, Serialize};
use store::import::{AccountImportCounts, ImportCounts, ImportedAccount, ImportedMatch};
use store::import_api::ApiImportCounts;
use store::import_archetypes::ArchetypeImportCounts;
use store::import_decks::{DeckImportCounts, ImportedDecks};
use store::import_leagues::LeagueImportCounts;
use store::import_limited::LimitedImportCounts;
use store::import_overlay::OverlayImportCounts;
use store::imported_detail::{DetailChange, DetailImportCounts};

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, storage};

/// The user's chosen 0.x database (JSON string or null).
pub const LEGACY_DB_PATH: &str = "legacy_db_path";
/// The 0.x preferences kept from settings.json (JSON object).
pub const LEGACY_SETTINGS: &str = "legacy_0x_settings";
/// The last import's `ImportSummary` (JSON).
pub const LEGACY_LAST_IMPORT: &str = "legacy_0x_last_import";
/// Matches sent to the tracker at a time (spec section 7).
pub const BATCH_SIZE: usize = 200;
/// When an unfinished 0.x import began (UTC unix ms; `null` once an
/// import's archetype step finished). Kept across restarts, so detection
/// that marks an imported match after it is never read as a v1 decision,
/// and the archetypes worker leaves imported matches alone while it is
/// set (archetypes spec section 15).
pub const LEGACY_IMPORT_STARTED: &str = "legacy_0x_import_started_at";

/// The start an import goes by: the earliest unfinished one.
pub fn import_start(stored: Option<i64>, now_ms: i64) -> i64 {
    match stored {
        Some(earlier) => earlier.min(now_ms),
        None => now_ms,
    }
}

/// A stored `LEGACY_IMPORT_STARTED` value as a time (`null` is none).
fn stored_start(value: Option<&str>) -> Option<i64> {
    value?.trim().parse().ok()
}

fn started_row(at_ms: i64) -> (String, String) {
    (LEGACY_IMPORT_STARTED.to_string(), at_ms.to_string())
}

/// When the unfinished 0.x import began, if one did not finish.
pub fn unfinished_import_start(conn: &store::Connection) -> Result<Option<i64>, String> {
    let rows = store::read::settings(conn).map_err(|error| error.to_string())?;
    let value = rows
        .iter()
        .find(|(key, _)| key == LEGACY_IMPORT_STARTED)
        .map(|(_, value)| value.as_str());
    Ok(stored_start(value))
}

/// Whether the archetypes backfill leaves imported matches alone: an
/// import runs now, or one did not finish (the app closed part way).
pub fn import_gate_closed(state: &AppState, conn: &store::Connection) -> bool {
    if state.import_running.load(Ordering::SeqCst) {
        return true;
    }
    match unfinished_import_start(conn) {
        Ok(start) => start.is_some(),
        Err(error) => {
            tracing::warn!(%error, "could not read the 0.x import start");
            false
        }
    }
}
/// The OS "launch at login" switch, as the autostart plugin sees it. The
/// app passes the plugin; tests pass a fake.
pub trait LaunchAtLogin {
    fn is_enabled(&self) -> Result<bool, String>;
    fn enable(&self) -> Result<(), String>;
}

/// Whether the import turns launch at login on (release spec section 13
/// item 4, ruling 16): 0.x had it on, v1's is off, and the question was never
/// settled (no earlier import applied it, the user never changed v1's).
pub fn carry_launch_at_login(legacy_on: bool, v1_on: bool, settled: bool) -> bool {
    legacy_on && !v1_on && !settled
}

/// Apply 0.x's launch at login if `carry_launch_at_login` says so. Returns
/// the row that records it, or `None` when nothing was changed (a refused
/// change is not recorded, so a later import tries again).
fn carried_launch_at_login(
    stored: &[(String, String)],
    legacy_on: bool,
    launch: &dyn LaunchAtLogin,
) -> Option<(String, String)> {
    let settled = crate::settings::autostart_from_0x_applied(stored);
    if !legacy_on || settled {
        return None;
    }
    let v1_on = match launch.is_enabled() {
        Ok(on) => on,
        Err(error) => {
            tracing::warn!(%error, "could not read launch at login");
            return None;
        }
    };
    if !carry_launch_at_login(legacy_on, v1_on, settled) {
        return None;
    }
    match launch.enable() {
        Ok(()) => {
            tracing::info!("launch at login turned on, as in 0.x");
            Some(crate::settings::autostart_from_0x_applied_row())
        }
        Err(error) => {
            tracing::warn!(%error, "could not turn on launch at login from 0.x");
            None
        }
    }
}

/// The user changed v1's launch at login switch: 0.x's value never replaces
/// their choice (ruling 16).
pub fn launch_at_login_changed(state: &AppState) -> Result<(), CommandError> {
    state
        .save_setting_rows(vec![crate::settings::autostart_from_0x_applied_row()])
        .map_err(CommandError::Tracker)
}

/// Match details stored between two `Details` progress reports. Each match's
/// detail is read and written on its own (0.x keeps far more detail than
/// matches), so reporting every one would flood the windows.
pub const DETAIL_PROGRESS_EVERY: u32 = 25;

/// What the Settings card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LegacyImportInfo {
    /// The 0.x database found in its usual place, if any.
    pub detected_path: Option<String>,
    /// The file the user chose; it wins over the detected one.
    pub chosen_path: Option<String>,
    pub last_import: Option<ImportSummary>,
    /// An import is running now.
    pub running: bool,
}

/// The page's message when the import panics (the panic text goes to the log).
pub const IMPORT_PANICKED: &str = "The import stopped unexpectedly. See the log for details.";

/// What one import did (spec section 7), stored in `legacy_0x_last_import`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// When the import finished, UTC unix ms.
    #[specta(type = specta_typescript::Number)]
    pub finished_at_ms: i64,
    pub path: String,
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// The match is already tracked from the logs, which always win.
    pub skipped_live: u32,
    pub skipped_in_progress: u32,
    /// No local seat in any game, no deck account, and not exactly one 0.x
    /// account (Plan O imports the rest, gameless ones with their score).
    pub skipped_no_local_player: u32,
    /// The local player is not one of 0.x's accounts (issue #337). Missing
    /// from a summary stored before it was counted.
    #[serde(default)]
    pub skipped_not_account: u32,
    pub skipped_duplicate_token: u32,
    pub skipped_bad_time: u32,
    /// A blank 0.x token. Missing from a summary stored before it was counted.
    #[serde(default)]
    pub skipped_blank_token: u32,
    /// Games (not matches) dropped for repeating a Game ID within their
    /// match. Missing from a summary stored before it was counted.
    #[serde(default)]
    pub skipped_duplicate_games: u32,
    /// Matches whose game detail failed to read or store (an unreadable
    /// detail, or tracking not running); a match with nothing to import is
    /// not counted. An import run again retries them. Missing from a summary
    /// stored before it was counted.
    #[serde(default)]
    pub skipped_details: u32,
    /// 0.x shared replay links brought over with their matches (replays
    /// spec section 10). Missing from a summary stored before they were
    /// counted.
    #[serde(default)]
    pub replay_shares_added: u32,
    /// 0.x shared replay links missing a part (uuid, url or time): left out.
    #[serde(default)]
    pub replay_shares_incomplete: u32,
    /// 0.x shared replay links with no v1 match to hold them (the match was
    /// left out, v1 tracked it under another token, or it holds another
    /// link): they stay on mymtgo.com, so the player switches them off there.
    #[serde(default)]
    pub replay_shares_unplaced: u32,
    /// Pre-release per-game replay links 0.x holds: never imported, so the
    /// player switches them off on mymtgo.com (issue #307 F1).
    #[serde(default)]
    pub legacy_game_links: u32,
    /// Cards, decks, versions and match links (decks spec section 9). Missing
    /// from a summary stored before decks were imported.
    #[serde(default)]
    pub decks: Option<DeckSummary>,
    /// Constructed leagues (leagues spec section 9). Missing from a summary
    /// stored before leagues were imported.
    #[serde(default)]
    pub leagues: Option<LeagueSummary>,
    /// The 0.x accounts and their login ids (Plan O). Missing from a summary
    /// stored before accounts were imported.
    #[serde(default)]
    pub accounts: Option<AccountSummary>,
    /// Drafts, picks, pick notes and limited registered decks (Plan N).
    /// Missing from a summary stored before Limited was imported.
    #[serde(default)]
    pub limited: Option<LimitedSummary>,
    /// What 0.x sent and synced with the MyMTGO API (Plan Q). Missing from a
    /// summary stored before the API state was imported.
    #[serde(default)]
    pub api: Option<ApiSummary>,
    /// 0.x's archetypes, match and deck archetypes, sideboard guides and
    /// matchup notes (Plan T). Missing from a summary stored before they
    /// were imported.
    #[serde(default)]
    pub archetypes: Option<ArchetypeSummary>,
    /// 0.x's overlay deck labels and custom background (Plan U). Missing
    /// from a summary stored before they were imported.
    #[serde(default)]
    pub overlay: Option<OverlaySummary>,
}

/// What the import did with 0.x's custom league card background (overlay
/// spec section 12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundImport {
    /// 0.x had no custom background.
    #[default]
    NotSet,
    /// Copied into v1's overlay folder.
    Copied,
    /// v1 already has its own background: it stays.
    Kept,
    /// 0.x's file is not there (or could not be read).
    Missing,
    /// 0.x's file is there but could not be copied.
    Failed,
    /// 0.x's file is not an image the card takes, or is over 5 MiB.
    Refused,
}

/// What one import brought over of 0.x's overlay data (overlay spec section
/// 12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySummary {
    pub labels_added: u32,
    /// The deck already had a label in v1: v1's stayed.
    pub labels_kept: u32,
    /// Labels of a 0.x deck id no imported deck has.
    pub labels_unknown: u32,
    pub background: BackgroundImport,
}

/// The overlay part of a summary: the writer's label counts, the labels
/// the reader found no 0.x deck for, and what happened to the background.
pub fn overlay_summary(
    counts: OverlayImportCounts,
    labels_without_deck: u32,
    background: BackgroundImport,
) -> OverlaySummary {
    OverlaySummary {
        labels_added: counts.labels_added,
        labels_kept: counts.labels_kept,
        labels_unknown: counts.labels_unknown + labels_without_deck,
        background,
    }
}

/// What one import brought over of 0.x's archetype data (archetypes spec
/// sections 15 and 21.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeSummary {
    pub archetypes_added: u32,
    pub archetypes_updated: u32,
    pub archetypes_unchanged: u32,
    pub match_rows_added: u32,
    /// v1's own row (or the player's "none") stayed.
    pub match_rows_kept: u32,
    /// Of matches not imported, without a seat, or of archetypes not
    /// imported.
    pub match_rows_skipped: u32,
    pub matches_detected: u32,
    pub decks_linked: u32,
    pub guides_added: u32,
    pub guides_kept: u32,
    pub guide_cards_added: u32,
    pub notes_added: u32,
    pub notes_unchanged: u32,
    pub notes_skipped: u32,
}

/// The archetype part of a summary, from the writer's counts.
pub fn archetype_summary(counts: ArchetypeImportCounts) -> ArchetypeSummary {
    ArchetypeSummary {
        archetypes_added: counts.archetypes_added,
        archetypes_updated: counts.archetypes_updated,
        archetypes_unchanged: counts.archetypes_unchanged,
        match_rows_added: counts.match_rows_added,
        match_rows_kept: counts.match_rows_kept,
        match_rows_skipped: counts.match_rows_skipped,
        matches_detected: counts.matches_detected,
        decks_linked: counts.decks_linked,
        guides_added: counts.guides_added,
        guides_kept: counts.guides_kept,
        guide_cards_added: counts.guide_cards_added,
        notes_added: counts.notes_added,
        notes_unchanged: counts.notes_unchanged,
        notes_skipped: counts.notes_skipped,
    }
}

/// What one import brought over of 0.x's MyMTGO API state (07 spec section
/// 11): what it reported, its card stats queue and its sync state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ApiSummary {
    /// Matches 0.x reported, recorded so v1 never reports them again.
    pub reports_sent: u32,
    pub card_stats_added: u32,
    /// Card stats of a game v1 has no match for.
    pub card_stats_skipped: u32,
    pub synced_rows: u32,
    pub deck_flags: u32,
    pub sync_states: u32,
    pub rejections: u32,
    pub tournaments_named: u32,
    pub tournament_tokens: u32,
}

/// The API part of a summary, from the writer's counts.
pub fn api_summary(counts: ApiImportCounts) -> ApiSummary {
    ApiSummary {
        reports_sent: counts.reports_sent,
        card_stats_added: counts.card_stats_added,
        card_stats_skipped: counts.card_stats_skipped,
        synced_rows: counts.synced_rows,
        deck_flags: counts.deck_flags,
        sync_states: counts.sync_states,
        rejections: counts.rejections,
        tournaments_named: counts.tournaments_named,
        tournament_tokens: counts.tournament_tokens,
    }
}

/// What one import did with the 0.x Limited data (draft and sealed leagues
/// are counted with the leagues).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedSummary {
    /// False for an old 0.x database without Limited tables.
    pub tables_found: bool,
    pub drafts_added: u32,
    pub drafts_updated: u32,
    pub drafts_unchanged: u32,
    /// Stored with no account (no league run with one, and more than one
    /// account).
    pub drafts_without_account: u32,
    /// A blank draft token.
    pub drafts_skipped: u32,
    /// Drafts of a league whose matches are all of a player that is not one
    /// of 0.x's accounts (issue #337). Missing from a summary stored before
    /// it was counted.
    #[serde(default)]
    pub drafts_not_account: u32,
    /// Pick notes added; a note made in v1 is kept.
    pub notes_added: u32,
    /// Pick notes of a log draft's picks v1 has not read yet (a later
    /// import adds them). Missing from summaries stored before it existed.
    #[serde(default)]
    pub notes_skipped: u32,
    pub decks_added: u32,
    pub decks_updated: u32,
    pub decks_unchanged: u32,
    /// Registered decks of a league that was not imported.
    pub decks_without_run: u32,
    /// Registered decks without a match, cards or capture time.
    pub decks_skipped: u32,
}

/// The Limited part of a summary, from the writer's counts and the reader's
/// skips.
pub fn limited_summary(
    counts: LimitedImportCounts,
    drafts_skipped: u32,
    drafts_not_account: u32,
    decks_skipped: u32,
) -> LimitedSummary {
    LimitedSummary {
        tables_found: true,
        drafts_added: counts.drafts_added,
        drafts_updated: counts.drafts_updated,
        // A draft v1 has from the log is left as it is.
        drafts_unchanged: counts.drafts_unchanged + counts.drafts_from_log,
        drafts_without_account: counts.drafts_without_account,
        drafts_skipped,
        drafts_not_account,
        notes_added: counts.notes_added,
        notes_skipped: counts.notes_without_pick,
        decks_added: counts.decks_added,
        decks_updated: counts.decks_updated,
        decks_unchanged: counts.decks_unchanged,
        decks_without_run: counts.decks_without_run,
        // A deck whose match v1 does not have is left out like an unreadable one.
        decks_skipped: decks_skipped + counts.decks_without_match,
    }
}

/// The Limited part of a summary for an old 0.x database without Limited
/// tables.
pub fn no_limited_tables() -> LimitedSummary {
    LimitedSummary::default()
}

/// What one import did with the 0.x accounts (deleted ones are left out).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    /// Registered in v1 now, with or without matches.
    pub added: u32,
    /// Already in v1.
    pub unchanged: u32,
    /// A v1 account without a login id took 0.x's.
    pub login_ids_filled: u32,
    /// 0.x's login id already belongs to another v1 account, which keeps it.
    pub login_id_clashes: u32,
    /// Untracked now, as in 0.x (issue #310). 0 in a summary stored before.
    #[serde(default)]
    pub untracked: u32,
    /// 0.x's active account became the current account (v1 had no switch).
    #[serde(default)]
    pub active_seeded: bool,
}

/// The account part of a summary, from the writer's counts.
pub fn account_summary(counts: AccountImportCounts) -> AccountSummary {
    AccountSummary {
        added: counts.added,
        unchanged: counts.unchanged,
        login_ids_filled: counts.login_ids_filled,
        login_id_clashes: counts.login_id_clashes,
        untracked: counts.untracked,
        active_seeded: counts.active_seeded,
    }
}

/// What one import did with the 0.x deck data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckSummary {
    /// False for an old 0.x database without deck tables: only matches were
    /// imported.
    pub tables_found: bool,
    pub cards_added: u32,
    pub cards_updated: u32,
    pub cards_unchanged: u32,
    pub decks_added: u32,
    pub decks_updated: u32,
    pub decks_unchanged: u32,
    /// Limited decks are not imported as decks: their versions become the
    /// registered decks of their matches (`LimitedSummary`).
    pub decks_skipped_limited: u32,
    /// Decks whose MTGO id an earlier 0.x deck has: the first wins, these
    /// are left out with their versions and links (issue #122 M13). Zero in
    /// a summary stored before they were counted.
    #[serde(default)]
    pub decks_skipped_duplicate: u32,
    pub versions_added: u32,
    pub versions_unchanged: u32,
    /// Versions v1 cannot read: a bad signature, or a legacy one naming a
    /// card the 0.x database does not have.
    pub versions_skipped: u32,
    pub links_added: u32,
    /// The match already had a deck (from tracking or an earlier import).
    pub links_unchanged: u32,
}

/// What one import did with the 0.x leagues: manual leagues and imported runs
/// (constructed, draft and sealed) together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueSummary {
    /// False for an old 0.x database without a leagues table.
    pub tables_found: bool,
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// Stored with no account (no matches, no deck, and more than one
    /// account): not listed under any account yet.
    pub without_account: u32,
    /// Deleted in 0.x.
    pub skipped_deleted: u32,
    /// A start time, state or kind v1 cannot read.
    pub skipped_unreadable: u32,
    /// Every match of the league is of a player that is not one of 0.x's
    /// accounts (issue #337). Missing from a summary stored before it was
    /// counted.
    #[serde(default)]
    pub skipped_not_account: u32,
    /// Draft and sealed leagues left out: only a summary stored before
    /// Limited was imported holds a count (issue #194); 0 since.
    #[serde(default)]
    pub skipped_limited: u32,
}

/// How an import is going, pushed to every window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum ImportProgress {
    Reading,
    Writing {
        done: u32,
        total: u32,
    },
    /// The matches are in; each one's game detail (Plan O) is read and
    /// stored, one match at a time.
    Details {
        done: u32,
        total: u32,
    },
    /// The match details are in; the decks, versions, cards and links follow.
    Decks,
    /// The decks are in; the leagues follow.
    Leagues,
    /// The leagues are in; the drafts, picks, notes and limited registered
    /// decks follow.
    Limited,
    /// The rest is in; the archetypes, sideboard guides and matchup notes
    /// follow.
    Archetypes,
    Finished {
        // Boxed: the summary is far larger than the other steps.
        summary: Box<ImportSummary>,
    },
    Failed {
        message: String,
    },
}

/// The parts of a summary after the matches: each `None` in a summary
/// stored before that part was imported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SummaryParts {
    pub skipped_details: u32,
    pub decks: Option<DeckSummary>,
    pub leagues: Option<LeagueSummary>,
    pub accounts: Option<AccountSummary>,
    pub limited: Option<LimitedSummary>,
    pub api: Option<ApiSummary>,
    pub archetypes: Option<ArchetypeSummary>,
    pub overlay: Option<OverlaySummary>,
}

/// The summary of an import that finished at `finished_at_ms`.
pub fn import_summary(
    finished_at_ms: i64,
    path: &str,
    counts: ImportCounts,
    skipped: Skipped,
    parts: SummaryParts,
) -> ImportSummary {
    ImportSummary {
        finished_at_ms,
        path: path.to_string(),
        added: counts.added,
        updated: counts.updated,
        unchanged: counts.unchanged,
        skipped_live: counts.skipped_live,
        skipped_in_progress: skipped.in_progress,
        skipped_no_local_player: skipped.no_local_player,
        skipped_not_account: skipped.not_account,
        skipped_duplicate_token: skipped.duplicate_token,
        skipped_bad_time: skipped.bad_time,
        skipped_blank_token: skipped.blank_token,
        skipped_duplicate_games: skipped.duplicate_games,
        skipped_details: parts.skipped_details,
        replay_shares_added: counts.replay_shares_added,
        replay_shares_incomplete: skipped.replay_shares_incomplete,
        replay_shares_unplaced: counts.replay_shares_unplaced + skipped.replay_shares_unplaced,
        legacy_game_links: skipped.legacy_game_links,
        decks: parts.decks,
        leagues: parts.leagues,
        accounts: parts.accounts,
        limited: parts.limited,
        api: parts.api,
        archetypes: parts.archetypes,
        overlay: parts.overlay,
    }
}

/// The deck part of a summary, from the writer's counts and the reader's skips.
pub fn deck_summary(
    counts: DeckImportCounts,
    skipped_limited: u32,
    skipped_versions: u32,
    skipped_duplicates: u32,
) -> DeckSummary {
    DeckSummary {
        tables_found: true,
        cards_added: counts.cards_added,
        cards_updated: counts.cards_updated,
        cards_unchanged: counts.cards_unchanged,
        decks_added: counts.decks_added,
        decks_updated: counts.decks_updated,
        decks_unchanged: counts.decks_unchanged,
        decks_skipped_limited: skipped_limited,
        decks_skipped_duplicate: skipped_duplicates,
        versions_added: counts.versions_added,
        versions_unchanged: counts.versions_unchanged,
        versions_skipped: skipped_versions,
        links_added: counts.links_added,
        links_unchanged: counts.links_unchanged,
    }
}

/// The deck part of a summary for an old 0.x database without deck tables.
pub fn no_deck_tables() -> DeckSummary {
    DeckSummary::default()
}

/// The league part of a summary, from the writer's counts and the reader's
/// skips.
pub fn league_summary(
    counts: LeagueImportCounts,
    skipped_deleted: u32,
    skipped_unreadable: u32,
    skipped_not_account: u32,
) -> LeagueSummary {
    LeagueSummary {
        tables_found: true,
        added: counts.added,
        updated: counts.updated,
        unchanged: counts.unchanged,
        without_account: counts.without_account,
        skipped_deleted,
        skipped_unreadable,
        skipped_not_account,
        skipped_limited: 0,
    }
}

/// The league part of a summary for an old 0.x database without a leagues
/// table.
pub fn no_league_table() -> LeagueSummary {
    LeagueSummary::default()
}

/// The database to import: the chosen file, else the detected one.
pub fn import_path(
    chosen: Option<String>,
    detected: Option<String>,
) -> Result<String, CommandError> {
    match chosen.or(detected) {
        Some(path) => Ok(path),
        None => Err(CommandError::Storage(
            "no MyMTGO 0.x database was found; choose it on the Import page".to_string(),
        )),
    }
}

/// The card's state from the stored settings rows. A row that does not parse
/// counts as missing.
pub fn info_from_rows(
    rows: &[(String, String)],
    detected_path: Option<String>,
    running: bool,
) -> LegacyImportInfo {
    let mut chosen_path = None;
    let mut last_import = None;
    for (key, value) in rows {
        match key.as_str() {
            LEGACY_DB_PATH => {
                chosen_path = serde_json::from_str::<Option<String>>(value).ok().flatten();
            }
            LEGACY_LAST_IMPORT => {
                last_import = serde_json::from_str::<ImportSummary>(value).ok();
            }
            _ => {}
        }
    }
    LegacyImportInfo {
        detected_path,
        chosen_path,
        last_import,
        running,
    }
}

/// The 0.x database in its usual place on this machine, if any.
pub fn detected_path() -> Option<String> {
    importer::detect().map(|path| path.to_string_lossy().into_owned())
}

/// The card's state now. `detected` is what `detected_path` found.
pub fn info(state: &AppState, detected: Option<String>) -> Result<LegacyImportInfo, CommandError> {
    let rows = store::read::settings(&lock(&state.reader)).map_err(storage)?;
    let running = state.import_running.load(Ordering::SeqCst);
    Ok(info_from_rows(&rows, detected, running))
}

/// Store the user's chosen 0.x database; a blank or missing path forgets it,
/// so the detected one is used again.
pub fn choose(
    state: &AppState,
    path: Option<String>,
    detected: Option<String>,
) -> Result<LegacyImportInfo, CommandError> {
    let cleaned = path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());
    if let Some(path) = &cleaned {
        crate::paths::refuse_remote_path(path)?;
    }
    let value = serde_json::to_string(&cleaned).unwrap_or_else(|_| "null".to_string());
    state
        .save_setting_rows(vec![(LEGACY_DB_PATH.to_string(), value)])
        .map_err(CommandError::Tracker)?;
    info(state, detected)
}

/// Claim the one import slot and pick the database to read. The caller must
/// then call `run_import`, which frees the slot when it ends.
pub fn begin(state: &AppState, detected: Option<String>) -> Result<String, CommandError> {
    let chosen = info(state, None)?.chosen_path;
    let path = import_path(chosen, detected)?;
    let claimed = state
        .import_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if !claimed {
        // Refused: nothing failed, the request conflicts with the running import.
        return Err(CommandError::Refused(
            "an import is already running".to_string(),
        ));
    }
    Ok(path)
}

/// Frees the import slot when dropped, even if the import panics.
struct Running<'a>(&'a AtomicBool);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Import the 0.x database at `path`, reporting every phase to `progress`
/// and ending with `Finished` or `Failed`, a panic included. The import slot
/// is free again before the last report, so a window that reacts to it sees
/// the import as no longer running.
///
/// Returns 0.x's card image folder when the import finished and 0.x had
/// local images on: the caller hands it to the cards worker, which copies
/// the files before downloading (cards spec section 13).
pub fn run_import(
    state: &AppState,
    path: &str,
    launch: &dyn LaunchAtLogin,
    progress: &mut dyn FnMut(ImportProgress),
) -> Option<PathBuf> {
    let outcome = {
        let _running = Running(&state.import_running);
        catch_unwind(AssertUnwindSafe(|| {
            import(state, path, launch, &mut *progress)
        }))
    };
    let mut images = None;
    let last = match outcome {
        Ok(Ok((summary, legacy_images))) => {
            images = legacy_images;
            tracing::info!(
                added = summary.added,
                updated = summary.updated,
                unchanged = summary.unchanged,
                "the 0.x import finished"
            );
            ImportProgress::Finished {
                summary: Box::new(summary),
            }
        }
        Ok(Err(message)) => {
            tracing::warn!(%message, "the 0.x import failed");
            ImportProgress::Failed { message }
        }
        Err(payload) => {
            // A warning: the panic hook already reported the panic itself.
            // The page gets a plain message; the detail is only logged.
            tracing::warn!(detail = %panic_message(payload.as_ref()), "the 0.x import panicked");
            ImportProgress::Failed {
                message: IMPORT_PANICKED.to_string(),
            }
        }
    };
    progress(last);
    images
}

fn import(
    state: &AppState,
    path: &str,
    launch: &dyn LaunchAtLogin,
    progress: &mut dyn FnMut(ImportProgress),
) -> Result<(ImportSummary, Option<PathBuf>), String> {
    // Detection that marks an imported match after this moment ran during
    // the import, before the 0.x archetype rows landed (archetypes spec
    // section 15): the archetype step does not read it as a v1 decision.
    // An earlier import that did not finish keeps its start (fix round 2,
    // N1): its matches may have been detected after it.
    let earlier = unfinished_import_start(&lock(&state.reader))?;
    let started_at_ms = import_start(earlier, crate::wall_clock_ms());
    state.save_setting_rows(vec![started_row(started_at_ms)])?;
    progress(ImportProgress::Reading);
    let read = importer::read_database(Path::new(path)).map_err(|error| error.to_string())?;
    let detail_targets = detail_targets(&read.matches, &read.match_ids);
    let mut names = player_names(&read.matches);
    if let Some(deck_read) = &read.decks {
        names.extend(deck_accounts(&deck_read.data));
    }
    names.extend(read.accounts.iter().map(|account| account.username.clone()));
    crate::scrub::remember(names);

    // Before the matches, so an account registers with its 0.x creation
    // time rather than the import's.
    let accounts = account_summary(import_accounts(state, read.accounts)?);

    let total = read.matches.len() as u32;
    let mut done: u32 = 0;
    let mut counts = ImportCounts::default();
    progress(ImportProgress::Writing { done, total });
    let mut remaining = read.matches;
    while !remaining.is_empty() {
        let rest = remaining.split_off(remaining.len().min(BATCH_SIZE));
        let batch = std::mem::replace(&mut remaining, rest);
        let sent = batch.len() as u32;
        let batch_counts = importer(state)?.import(batch)?;
        add_counts(&mut counts, batch_counts);
        done += sent;
        progress(ImportProgress::Writing { done, total });
    }

    // Shared replay links (replays spec section 10): imported with their
    // matches; a link missing a part and the pre-release per-game links are
    // counted only.
    tracing::info!(
        added = counts.replay_shares_added,
        incomplete = read.skipped.replay_shares_incomplete,
        "the 0.x shared replay links were imported"
    );
    if read.skipped.legacy_game_links > 0 {
        tracing::warn!(
            links = read.skipped.legacy_game_links,
            "0.x holds pre-release per-game replay links; they are not imported (switch them off on mymtgo.com)"
        );
    }

    // After the matches, so each detail finds its row.
    let (details, details_failed) = import_details(state, path, &detail_targets, progress);
    tracing::info!(
        added = details.added,
        updated = details.updated,
        unchanged = details.unchanged,
        removed = details.removed,
        skipped = details.skipped,
        card_stats_skipped = details.card_stats_skipped,
        "the 0.x match details were imported"
    );

    // After the matches, so the links find their rows.
    let decks = match read.decks {
        Some(deck_read) => {
            progress(ImportProgress::Decks);
            let deck_counts = importer(state)?.import_decks(deck_read.data)?;
            deck_summary(
                deck_counts,
                deck_read.skipped_limited,
                deck_read.skipped_versions,
                deck_read.skipped_duplicates,
            )
        }
        None => no_deck_tables(),
    };

    // After the decks, so members and deck versions are found.
    let leagues = match read.leagues {
        Some(league_read) => {
            progress(ImportProgress::Leagues);
            let league_counts = importer(state)?.import_leagues(league_read.leagues)?;
            league_summary(
                league_counts,
                league_read.skipped_deleted,
                league_read.skipped_unreadable,
                league_read.skipped_not_account,
            )
        }
        None => no_league_table(),
    };

    // After the leagues, so drafts and registered decks find their runs.
    let limited = match read.limited {
        Some(limited_read) => {
            progress(ImportProgress::Limited);
            let limited_counts = importer(state)?.import_limited(limited_read.data)?;
            limited_summary(
                limited_counts,
                limited_read.drafts_skipped,
                limited_read.drafts_not_account,
                limited_read.decks_skipped,
            )
        }
        None => no_limited_tables(),
    };

    // Last, so the matches, decks and leagues it names are in.
    let mut api_state = importer::api_read::read_api_state_at(Path::new(path))
        .map_err(|error| error.to_string())?;
    // A tournament only players who are not 0.x's accounts played is left
    // out with their matches (issue #337).
    api_state
        .tournaments
        .retain(|tournament| !read.not_account.holds_tournament(tournament.mtgo_event_id));
    let api = api_summary(importer(state)?.import_api(api_state)?);

    // After the matches and decks, so the match rows, deck links, guides and
    // notes find their rows.
    progress(ImportProgress::Archetypes);
    let mut archetype_data = importer::archetype_read::read_archetypes_at(Path::new(path))
        .map_err(|error| error.to_string())?;
    archetype_data.import_started_at_ms = Some(started_at_ms);
    let archetypes = archetype_summary(importer(state)?.import_archetypes(archetype_data)?);

    // After the decks, so each label finds its deck (overlay spec section
    // 12). The background is copied with the settings rows below.
    let legacy_json = legacy_settings_json(path);
    let kept_settings = legacy_json.as_deref().and_then(kept_legacy_settings);
    let legacy_launch_at_login = legacy_json
        .as_deref()
        .is_some_and(importer::settings::launch_at_login);
    let overlay =
        importer::overlay_read::read_overlay_at(Path::new(path), kept_settings.as_deref())
            .map_err(|error| error.to_string())?;
    let label_counts = if overlay.labels.is_empty() {
        OverlayImportCounts::default()
    } else {
        importer(state)?.import_overlay_labels(overlay.labels)?
    };

    // A settings save cannot interleave with this read and write.
    let _one_save = lock(&state.settings_lock);
    let stored_rows = store::read::settings(&lock(&state.reader))
        .map_err(|error| format!("the matches were imported, but {error}"))?;
    let overlay_folder = crate::overlay::background::folder(&state.data_dir);
    let (background, copied) = crate::overlay::background::legacy_background(
        &overlay_folder,
        crate::settings::overlay_background_file(&stored_rows).as_deref(),
        &overlay.background,
        crate::wall_clock_ms(),
    );
    let summary = import_summary(
        crate::wall_clock_ms(),
        path,
        counts,
        read.skipped,
        SummaryParts {
            skipped_details: details_failed,
            decks: Some(decks),
            leagues: Some(leagues),
            accounts: Some(accounts),
            limited: Some(limited),
            api: Some(api),
            archetypes: Some(archetypes),
            overlay: Some(overlay_summary(
                label_counts,
                overlay.labels_unknown,
                background,
            )),
        },
    );
    let mut rows = Vec::new();
    if let Some(name) = &copied {
        rows.push(crate::settings::overlay_background_file_row(Some(name)));
    }
    let mut images = None;
    if let Some(kept) = kept_settings {
        images = crate::cards::legacy_images::legacy_images_source(&kept, Path::new(path));
        // Seeded settings (the deck settings, card stats trust)
        // v1 left at their defaults take 0.x's values now: once v1 saved its
        // settings, seeding on read no longer reaches them.
        rows.extend(crate::settings::legacy_seeded_rows(&stored_rows, &kept));
        // An offline 0.x user stays offline; a v1 row is never replaced.
        rows.extend(mymtgo_api::seed_offline_rows(&stored_rows, &kept));
        rows.push((LEGACY_SETTINGS.to_string(), kept));
    }
    // 0.x's launch at login, once, while v1's was never changed.
    rows.extend(carried_launch_at_login(
        &stored_rows,
        legacy_launch_at_login,
        launch,
    ));
    let stored = serde_json::to_string(&summary)
        .map_err(|error| format!("the import summary could not be written: {error}"))?;
    rows.push((LEGACY_LAST_IMPORT.to_string(), stored));
    // The archetype step finished: no unfinished import is left.
    rows.push((LEGACY_IMPORT_STARTED.to_string(), "null".to_string()));
    if let Err(error) = state.save_setting_rows(rows) {
        // The copied background has no row to name it: it goes again.
        crate::overlay::background::delete_background(&overlay_folder, copied.as_deref());
        return Err(format!("the matches were imported, but {error}"));
    }
    // The scan waits on the tracker (up to 30 s): a Settings save must not.
    drop(_one_save);
    scan_card_stubs(state);
    Ok((summary, images))
}

/// What the overlay worker is told after an import (overlay spec section
/// 12): the background may have been copied in, and the overlay settings
/// may have been seeded from 0.x, so the windows follow them at once as
/// after a save. New deck labels arrive through the tracker's
/// `DecksChanged`.
pub fn overlay_messages_after_import(state: &AppState) -> Vec<crate::overlay::worker::OverlayMsg> {
    use crate::overlay::worker::OverlayMsg;
    let mut messages = vec![OverlayMsg::BackgroundChanged];
    match store::read::settings(&lock(&state.reader)) {
        Ok(rows) => messages.push(OverlayMsg::SettingsChanged {
            settings: Box::new(crate::settings::Settings::from_rows(&rows)),
        }),
        Err(error) => {
            tracing::warn!(%error, "the imported settings were not read for the overlays")
        }
    }
    messages
}

/// The full card stub scan after an import (cards spec section 13): ids 0.x
/// never had card rows for get stubs, and the tracker's `CardStubsAdded`
/// wakes the cards worker. A failed scan does not fail the import: the next
/// population run or stub scan finds the ids.
fn scan_card_stubs(state: &AppState) {
    let Ok(scanner) = state.tracker_part(tracker::TrackerHandle::card_stub_scanner) else {
        return;
    };
    match scanner.scan() {
        Ok(added) => tracing::info!(added, "card stubs scanned after the 0.x import"),
        Err(error) => tracing::warn!(%error, "the card stub scan after the 0.x import failed"),
    }
}

/// One match whose game detail is imported.
struct DetailTarget {
    token: String,
    /// The 0.x `matches.id`.
    match_id: i64,
    /// The local player, to find the local seat when 0.x marked none.
    local_username: String,
}

/// The matches whose game detail is imported, in read order. A 0.x manual
/// match keeps its recorded detail as a manual match (Plan L), so it has
/// none here.
fn detail_targets(
    matches: &[ImportedMatch],
    match_ids: &HashMap<String, i64>,
) -> Vec<DetailTarget> {
    let mut targets = Vec::new();
    for imported in matches {
        if imported.manual.is_some() {
            continue;
        }
        if let Some(match_id) = match_ids.get(&imported.token) {
            targets.push(DetailTarget {
                token: imported.token.clone(),
                match_id: *match_id,
                local_username: imported.local_username.clone(),
            });
        }
    }
    targets
}

/// Read and store each match's detail, one match at a time, so the frames
/// of only one match are in memory at once. The detail is extra: a match
/// whose detail cannot be read or stored is counted as skipped with a
/// warning, so the decks, leagues and summary still follow. Also returns how
/// many details failed (a match with nothing to import, or one that is no
/// longer a 0.x row, is skipped but did not fail).
fn import_details(
    state: &AppState,
    path: &str,
    targets: &[DetailTarget],
    progress: &mut dyn FnMut(ImportProgress),
) -> (DetailImportCounts, u32) {
    let total = targets.len() as u32;
    let mut done: u32 = 0;
    let mut failed: u32 = 0;
    let mut counts = DetailImportCounts::default();
    progress(ImportProgress::Details { done, total });
    if targets.is_empty() {
        return (counts, failed);
    }
    let mut reader = match importer::DetailReader::open(Path::new(path)) {
        Ok(reader) => Some(reader),
        Err(error) => {
            tracing::warn!(%error, "the 0.x match details could not be read");
            None
        }
    };
    for target in targets {
        let import = match reader.as_mut() {
            Some(reader) => reader
                .read(target.match_id, &target.token, &target.local_username)
                .map_err(|error| error.to_string())
                .and_then(|detail| importer(state)?.import_detail(target.token.clone(), detail)),
            None => Err("the 0.x match details could not be read".to_string()),
        };
        match import {
            Ok(import) => counts.add_import(import),
            Err(message) => {
                tracing::warn!(%message, "a 0.x match detail was skipped");
                counts.add(DetailChange::Skipped);
                failed += 1;
            }
        }
        done += 1;
        if done.is_multiple_of(DETAIL_PROGRESS_EVERY) || done == total {
            progress(ImportProgress::Details { done, total });
        }
    }
    (counts, failed)
}

/// Every local and opponent name, for the report scrubber.
fn player_names(matches: &[ImportedMatch]) -> Vec<String> {
    let mut names = Vec::new();
    for imported in matches {
        names.push(imported.local_username.clone());
        if let Some(opponent) = &imported.opponent {
            names.push(opponent.clone());
        }
    }
    names
}

/// The 0.x account names of the decks, for the report scrubber.
fn deck_accounts(data: &ImportedDecks) -> Vec<String> {
    let mut names = Vec::new();
    for deck in &data.decks {
        if let Some(account) = &deck.account {
            names.push(account.clone());
        }
    }
    names
}

fn add_counts(total: &mut ImportCounts, more: ImportCounts) {
    total.added += more.added;
    total.updated += more.updated;
    total.unchanged += more.unchanged;
    total.skipped_live += more.skipped_live;
    total.replay_shares_added += more.replay_shares_added;
    total.replay_shares_unplaced += more.replay_shares_unplaced;
}

/// The running tracker's `Importer`. The tracker lock is held only to take
/// it, never while an import waits: stopping tracking (exit, an update
/// install) or a settings save must not wait for an import.
fn importer(state: &AppState) -> Result<tracker::Importer, String> {
    state
        .tracker_part(tracker::TrackerHandle::importer)
        .map_err(|error| error.message().to_string())
}

/// Send the 0.x accounts to the tracker and wait for its counts. The import
/// may seed the `active_account` setting, so it holds the settings lock: a
/// save in progress cannot write its stale switch back over the seed.
fn import_accounts(
    state: &AppState,
    accounts: Vec<ImportedAccount>,
) -> Result<AccountImportCounts, String> {
    let importer = importer(state)?;
    let _one_save = lock(&state.settings_lock);
    importer.import_accounts(accounts)
}

/// 0.x's settings.json beside the database, if it exists and reads. Read
/// only: the file is never changed.
fn legacy_settings_json(db_path: &str) -> Option<String> {
    let settings = importer::settings_path(Path::new(db_path))?;
    std::fs::read_to_string(&settings).ok()
}

/// The kept 0.x preferences, when settings.json parses (importer spec
/// section 6).
fn kept_legacy_settings(json: &str) -> Option<String> {
    match importer::filter_settings(json) {
        Ok(kept) => Some(kept),
        Err(error) => {
            tracing::warn!(%error, "the 0.x settings were not kept");
            None
        }
    }
}

#[cfg(test)]
mod import_scale;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use std::path::PathBuf;

    const SCHEMA: &str = include_str!("../../fixtures/0x/schema.sql");

    pub(crate) fn summary(path: &str) -> ImportSummary {
        import_summary(
            1_000,
            path,
            ImportCounts {
                added: 1,
                updated: 2,
                unchanged: 3,
                skipped_live: 4,
                replay_shares_added: 23,
                replay_shares_unplaced: 26,
            },
            Skipped {
                in_progress: 5,
                no_local_player: 6,
                not_account: 75,
                duplicate_token: 7,
                bad_time: 8,
                blank_token: 21,
                duplicate_games: 22,
                replay_shares_incomplete: 24,
                replay_shares_unplaced: 27,
                legacy_game_links: 25,
            },
            SummaryParts {
                skipped_details: 73,
                decks: Some(deck_summary(
                    DeckImportCounts {
                        cards_added: 9,
                        cards_updated: 10,
                        cards_unchanged: 11,
                        decks_added: 12,
                        decks_updated: 13,
                        decks_unchanged: 14,
                        versions_added: 15,
                        versions_unchanged: 16,
                        links_added: 17,
                        links_unchanged: 18,
                        rules_failed: 0,
                    },
                    19,
                    20,
                    74,
                )),
                leagues: Some(league_summary(
                    LeagueImportCounts {
                        added: 23,
                        updated: 24,
                        unchanged: 25,
                        without_account: 26,
                    },
                    28,
                    29,
                    76,
                )),
                accounts: Some(account_summary(AccountImportCounts {
                    added: 30,
                    unchanged: 31,
                    login_ids_filled: 32,
                    login_id_clashes: 33,
                    ..AccountImportCounts::default()
                })),
                limited: Some(limited_summary(
                    LimitedImportCounts {
                        drafts_added: 34,
                        drafts_updated: 35,
                        drafts_unchanged: 36,
                        drafts_without_account: 37,
                        notes_added: 38,
                        decks_added: 39,
                        decks_updated: 40,
                        decks_unchanged: 41,
                        decks_without_run: 42,
                        drafts_from_log: 0,
                        decks_without_match: 0,
                        notes_without_pick: 45,
                    },
                    43,
                    77,
                    44,
                )),
                api: Some(api_summary(ApiImportCounts {
                    reports_sent: 46,
                    card_stats_added: 47,
                    card_stats_skipped: 48,
                    synced_rows: 49,
                    deck_flags: 50,
                    sync_states: 51,
                    rejections: 52,
                    tournaments_named: 53,
                    tournament_tokens: 54,
                })),
                archetypes: Some(archetype_summary(ArchetypeImportCounts {
                    archetypes_added: 55,
                    archetypes_updated: 56,
                    archetypes_unchanged: 57,
                    match_rows_added: 58,
                    match_rows_kept: 59,
                    match_rows_skipped: 60,
                    matches_detected: 61,
                    decks_linked: 62,
                    guides_added: 63,
                    guides_kept: 64,
                    guide_cards_added: 65,
                    notes_added: 66,
                    notes_unchanged: 67,
                    notes_skipped: 68,
                })),
                overlay: Some(overlay_summary(
                    OverlayImportCounts {
                        labels_added: 69,
                        labels_kept: 70,
                        labels_unknown: 71,
                    },
                    72,
                    BackgroundImport::Copied,
                )),
            },
        )
    }

    #[test]
    fn a_summary_holds_the_counts_and_every_skip_reason() {
        assert_eq!(
            summary("C:/q/nativephp.sqlite"),
            ImportSummary {
                finished_at_ms: 1_000,
                path: "C:/q/nativephp.sqlite".to_string(),
                added: 1,
                updated: 2,
                unchanged: 3,
                skipped_live: 4,
                skipped_in_progress: 5,
                skipped_no_local_player: 6,
                skipped_not_account: 75,
                skipped_duplicate_token: 7,
                skipped_bad_time: 8,
                skipped_blank_token: 21,
                skipped_duplicate_games: 22,
                skipped_details: 73,
                replay_shares_added: 23,
                replay_shares_incomplete: 24,
                replay_shares_unplaced: 53,
                legacy_game_links: 25,
                decks: Some(DeckSummary {
                    tables_found: true,
                    cards_added: 9,
                    cards_updated: 10,
                    cards_unchanged: 11,
                    decks_added: 12,
                    decks_updated: 13,
                    decks_unchanged: 14,
                    decks_skipped_limited: 19,
                    decks_skipped_duplicate: 74,
                    versions_added: 15,
                    versions_unchanged: 16,
                    versions_skipped: 20,
                    links_added: 17,
                    links_unchanged: 18,
                }),
                leagues: Some(LeagueSummary {
                    tables_found: true,
                    added: 23,
                    updated: 24,
                    unchanged: 25,
                    without_account: 26,
                    skipped_deleted: 28,
                    skipped_unreadable: 29,
                    skipped_not_account: 76,
                    skipped_limited: 0,
                }),
                accounts: Some(AccountSummary {
                    added: 30,
                    unchanged: 31,
                    login_ids_filled: 32,
                    login_id_clashes: 33,
                    untracked: 0,
                    active_seeded: false,
                }),
                limited: Some(LimitedSummary {
                    tables_found: true,
                    drafts_added: 34,
                    drafts_updated: 35,
                    drafts_unchanged: 36,
                    drafts_without_account: 37,
                    drafts_skipped: 43,
                    drafts_not_account: 77,
                    notes_added: 38,
                    notes_skipped: 45,
                    decks_added: 39,
                    decks_updated: 40,
                    decks_unchanged: 41,
                    decks_without_run: 42,
                    decks_skipped: 44,
                }),
                api: Some(ApiSummary {
                    reports_sent: 46,
                    card_stats_added: 47,
                    card_stats_skipped: 48,
                    synced_rows: 49,
                    deck_flags: 50,
                    sync_states: 51,
                    rejections: 52,
                    tournaments_named: 53,
                    tournament_tokens: 54,
                }),
                archetypes: Some(ArchetypeSummary {
                    archetypes_added: 55,
                    archetypes_updated: 56,
                    archetypes_unchanged: 57,
                    match_rows_added: 58,
                    match_rows_kept: 59,
                    match_rows_skipped: 60,
                    matches_detected: 61,
                    decks_linked: 62,
                    guides_added: 63,
                    guides_kept: 64,
                    guide_cards_added: 65,
                    notes_added: 66,
                    notes_unchanged: 67,
                    notes_skipped: 68,
                }),
                overlay: Some(OverlaySummary {
                    labels_added: 69,
                    labels_kept: 70,
                    labels_unknown: 143,
                    background: BackgroundImport::Copied,
                }),
            }
        );
    }

    #[test]
    fn a_summary_stored_before_replay_links_were_counted_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        let object = stored.as_object_mut().expect("an object");
        for key in [
            "replaySharesAdded",
            "replaySharesIncomplete",
            "replaySharesUnplaced",
            "legacyGameLinks",
        ] {
            object.remove(key);
        }
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(
            (
                read.replay_shares_added,
                read.replay_shares_incomplete,
                read.replay_shares_unplaced,
                read.legacy_game_links
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn a_summary_stored_before_the_overlay_was_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored.as_object_mut().expect("an object").remove("overlay");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.overlay, None);
    }

    #[test]
    fn a_summary_stored_before_archetypes_were_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored
            .as_object_mut()
            .expect("an object")
            .remove("archetypes");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.archetypes, None);
    }

    #[test]
    fn a_summary_stored_before_the_api_state_was_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored.as_object_mut().expect("an object").remove("api");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.api, None);
    }

    #[test]
    fn a_summary_stored_before_limited_was_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        let object = stored.as_object_mut().expect("an object");
        object.remove("limited");
        // A summary from before Plan N counted Limited leagues as skipped.
        object["leagues"]
            .as_object_mut()
            .expect("leagues")
            .insert("skippedLimited".to_string(), serde_json::json!(2));
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.limited, None);
        assert_eq!(read.leagues.expect("leagues").skipped_deleted, 28);
    }

    #[test]
    fn an_old_0x_without_limited_tables_says_so() {
        assert_eq!(
            no_limited_tables(),
            LimitedSummary {
                tables_found: false,
                ..LimitedSummary::default()
            }
        );
    }

    #[test]
    fn a_summary_stored_before_accounts_were_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored
            .as_object_mut()
            .expect("an object")
            .remove("accounts");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.accounts, None);
    }

    #[test]
    fn an_old_0x_without_deck_tables_says_matches_only() {
        assert_eq!(
            no_deck_tables(),
            DeckSummary {
                tables_found: false,
                ..DeckSummary::default()
            }
        );
    }

    #[test]
    fn a_summary_stored_before_not_account_skips_were_counted_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        let object = stored.as_object_mut().expect("an object");
        object.remove("skippedNotAccount");
        object["leagues"]
            .as_object_mut()
            .expect("leagues")
            .remove("skippedNotAccount");
        object["limited"]
            .as_object_mut()
            .expect("limited")
            .remove("draftsNotAccount");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.skipped_not_account, 0);
        assert_eq!(read.leagues.expect("leagues").skipped_not_account, 0);
        assert_eq!(read.limited.expect("limited").drafts_not_account, 0);
    }

    #[test]
    fn a_summary_stored_before_blank_tokens_and_repeated_games_were_counted_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        let object = stored.as_object_mut().expect("an object");
        object.remove("skippedBlankToken");
        object.remove("skippedDuplicateGames");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(
            (read.skipped_blank_token, read.skipped_duplicate_games),
            (0, 0)
        );
    }

    #[test]
    fn an_old_0x_without_a_leagues_table_says_so() {
        assert_eq!(
            no_league_table(),
            LeagueSummary {
                tables_found: false,
                ..LeagueSummary::default()
            }
        );
    }

    // Issue #194: a summary stored before Limited was imported counted the
    // draft and sealed leagues it left out; that count still reads.
    #[test]
    fn a_summary_stored_before_limited_keeps_its_skipped_limited_leagues() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored["leagues"]["skippedLimited"] = serde_json::json!(4);
        stored.as_object_mut().expect("an object").remove("limited");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.leagues.expect("leagues").skipped_limited, 4);
    }

    #[test]
    fn a_summary_stored_before_leagues_were_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored.as_object_mut().expect("an object").remove("leagues");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.leagues, None);
    }

    #[test]
    fn a_summary_stored_before_decks_were_imported_still_reads() {
        let mut stored = serde_json::to_value(summary("p")).expect("json");
        stored.as_object_mut().expect("an object").remove("decks");
        let read: ImportSummary = serde_json::from_value(stored).expect("parses");
        assert_eq!(read.decks, None);
    }

    #[test]
    fn progress_reaches_the_ui_tagged_by_phase() {
        let writing = serde_json::to_value(ImportProgress::Writing {
            done: 200,
            total: 450,
        })
        .expect("json");
        assert_eq!(
            writing,
            serde_json::json!({ "phase": "writing", "done": 200, "total": 450 })
        );
        let finished = serde_json::to_value(ImportProgress::Finished {
            summary: Box::new(summary("p")),
        })
        .expect("json");
        assert_eq!(finished["phase"], "finished");
        assert_eq!(finished["summary"]["skippedNoLocalPlayer"], 6);
        assert_eq!(finished["summary"]["decks"]["linksAdded"], 17);
        assert_eq!(finished["summary"]["leagues"]["skippedDeleted"], 28);
        assert_eq!(
            serde_json::to_value(ImportProgress::Details {
                done: 40,
                total: 120
            })
            .expect("json"),
            serde_json::json!({ "phase": "details", "done": 40, "total": 120 })
        );
        assert_eq!(
            serde_json::to_value(ImportProgress::Decks).expect("json"),
            serde_json::json!({ "phase": "decks" })
        );
        assert_eq!(
            serde_json::to_value(ImportProgress::Leagues).expect("json"),
            serde_json::json!({ "phase": "leagues" })
        );
        assert_eq!(
            serde_json::to_value(ImportProgress::Limited).expect("json"),
            serde_json::json!({ "phase": "limited" })
        );
        assert_eq!(
            serde_json::to_value(ImportProgress::Archetypes).expect("json"),
            serde_json::json!({ "phase": "archetypes" })
        );
        assert_eq!(finished["summary"]["archetypes"]["guidesAdded"], 63);
        assert_eq!(finished["summary"]["limited"]["notesAdded"], 38);
    }

    #[test]
    fn the_chosen_path_beats_the_detected_one() {
        assert_eq!(
            import_path(Some("chosen".to_string()), Some("detected".to_string())),
            Ok("chosen".to_string())
        );
        assert_eq!(
            import_path(None, Some("detected".to_string())),
            Ok("detected".to_string())
        );
        assert_eq!(
            import_path(None, None),
            Err(CommandError::Storage(
                "no MyMTGO 0.x database was found; choose it on the Import page".to_string()
            ))
        );
    }

    #[test]
    fn stored_rows_give_the_chosen_path_and_the_last_import() {
        let rows = vec![
            (LEGACY_DB_PATH.to_string(), "\"D:/q.sqlite\"".to_string()),
            (
                LEGACY_LAST_IMPORT.to_string(),
                serde_json::to_string(&summary("D:/q.sqlite")).expect("json"),
            ),
            ("close_to_tray".to_string(), "true".to_string()),
        ];
        let info = info_from_rows(&rows, Some("E:/found.sqlite".to_string()), true);
        assert_eq!(
            info,
            LegacyImportInfo {
                detected_path: Some("E:/found.sqlite".to_string()),
                chosen_path: Some("D:/q.sqlite".to_string()),
                last_import: Some(summary("D:/q.sqlite")),
                running: true,
            }
        );
        let unreadable = vec![
            (LEGACY_DB_PATH.to_string(), "null".to_string()),
            (LEGACY_LAST_IMPORT.to_string(), "{".to_string()),
        ];
        let empty = info_from_rows(&unreadable, None, false);
        assert_eq!((empty.chosen_path, empty.last_import), (None, None));
    }

    #[test]
    fn only_one_import_runs_at_a_time() {
        let dir = TestDir::new("legacy-one-at-a-time");
        let (state, _events) = ready_state(&dir);
        assert!(
            matches!(begin(&state, None), Err(CommandError::Storage(_))),
            "nothing found and nothing chosen"
        );
        assert!(!state.import_running.load(Ordering::SeqCst));
        choose(&state, Some("Q:/0x.sqlite".to_string()), None).expect("chosen");
        let detected = Some("Q:/detected.sqlite".to_string());
        assert_eq!(
            begin(&state, detected.clone()),
            Ok("Q:/0x.sqlite".to_string())
        );
        assert!(state.import_running.load(Ordering::SeqCst));
        assert_eq!(
            begin(&state, detected),
            Err(CommandError::Refused(
                "an import is already running".to_string()
            )),
            "a state conflict, not a storage failure"
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn choosing_a_file_stores_it_and_clearing_it_forgets_it() {
        let dir = TestDir::new("legacy-choose");
        let (state, _events) = ready_state(&dir);
        let chosen = choose(&state, Some("  Q:/0x.sqlite ".to_string()), None).expect("chosen");
        assert_eq!(chosen.chosen_path, Some("Q:/0x.sqlite".to_string()));
        assert_eq!(
            info(&state, None).expect("info").chosen_path,
            Some("Q:/0x.sqlite".to_string())
        );
        let cleared = choose(&state, Some("   ".to_string()), None).expect("cleared");
        assert_eq!(cleared.chosen_path, None);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn choosing_a_network_file_is_refused() {
        let dir = TestDir::new("legacy-choose-remote");
        let (state, _events) = ready_state(&dir);
        let error =
            choose(&state, Some(r"\\host\share\0x.sqlite".to_string()), None).expect_err("refused");
        assert_eq!(
            error,
            CommandError::Refused(crate::paths::REMOTE_PATH_REFUSED.to_string())
        );
        assert_eq!(info(&state, None).expect("info").chosen_path, None);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// A 0.x install under `dir`: `<root>/database/nativephp.sqlite` with one
    /// complete match between Qlocal and Qopponent, and its settings.json.
    pub(crate) fn legacy_install(dir: &TestDir) -> PathBuf {
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"deck_card_size":"large","sync_key":"secret"}"#,
        );
        dir.write("legacy/database/.keep", b"");
        let path = dir.join("legacy/database/nativephp.sqlite");
        let conn = store::Connection::open(&path).expect("open 0.x test database");
        conn.execute_batch("PRAGMA foreign_keys = OFF;")
            .expect("no foreign keys");
        conn.execute_batch(SCHEMA).expect("0.x schema");
        conn.execute_batch(
            r#"INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');
             INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won,
                 games_lost, started_at, ended_at, state)
             VALUES (1, 'q-token-1', '4242', 'CMODERN', 'Constructed', 1, 0,
                 '2026-01-02 03:04:05', '2026-01-02 03:34:05', 'complete'),
                 (2, 'q-token-2', '4243', 'CMODERN', 'Constructed', 0, 0,
                 '2026-01-02 04:04:05', NULL, 'in_progress');
             INSERT INTO games (id, match_id, mtgo_id, won, started_at)
             VALUES (10, 1, '10', 1, '2026-01-02 03:04:05');
             INSERT INTO game_player (game_id, player_id, instance_id, is_local, deck_json)
             VALUES (10, 1, 1, 1, '[{"mtgo_id":12,"quantity":4,"sideboard":false}]'),
                    (10, 2, 1, 0, NULL);
             INSERT INTO game_timelines (game_id, timestamp, content)
             VALUES (10, '2026-01-02 03:04:06', '{"Cards":[],"Players":[]}'),
                    (10, '2026-01-02 03:04:07', '{"Cards":[],"Players":[]}');
             INSERT INTO game_logs (match_token, file_path, decoded_entries)
             VALUES ('q-token-1', 'Q.dat',
                 '[{"timestamp":"2026-01-02T03:04:05+00:00","message":"Q line"}]');
             INSERT INTO cards (id, mtgo_id, name) VALUES (1, '12', 'Q Bolt');
             INSERT INTO accounts (id, username, login_id, created_at, deleted_at)
             VALUES (1, 'Qdeckowner', 4321, NULL, NULL),
                    (2, 'Qquiet', NULL, '2025-12-01 00:00:00', NULL),
                    (3, 'Qgone', 99, NULL, '2025-12-02 00:00:00'),
                    (4, 'Qlocal', NULL, NULL, NULL);
             INSERT INTO decks (id, mtgo_id, name, format, account_id, cover_id)
             VALUES (1, '1001', 'Q Burn', 'CMODERN', 1, 1);
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (1, 1, 'MTI6NDpmYWxzZQ==', '2026-01-01 00:00:00');
             UPDATE matches SET deck_version_id = 1 WHERE id = 1;"#,
        )
        .expect("0.x rows");
        conn.execute_batch(
            "INSERT INTO leagues (id, token, name, format, started_at, state, deck_version_id, kind)
             VALUES (1, 'q-league', 'Q League', 'CMODERN', '2026-01-02 03:00:00', 'complete', 1,
                     'constructed'),
                    (2, 'q-draft', 'Q Draft', 'Limited', '2026-01-02 03:00:00', 'complete', NULL,
                     'draft');
             UPDATE matches SET league_id = 1 WHERE id = 1;",
        )
        .expect("0.x leagues");
        conn.execute_batch(
            "INSERT INTO drafts (id, league_id, draft_token, state, started_at)
             VALUES (1, 2, 'q-draft-token', 'finished', '2026-01-02 03:00:00');
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, picked_catalog_id, reservations, note)
             VALUES (1, 1, 1, 1, '[12]', 12, '[]', 'Q note');",
        )
        .expect("0.x draft");
        path
    }

    fn stored(state: &crate::commands::AppState, key: &str) -> Option<String> {
        store::read::settings(&lock(&state.reader))
            .expect("settings")
            .into_iter()
            .find(|(stored_key, _)| stored_key == key)
            .map(|(_, value)| value)
    }

    /// Import a 0.x install whose settings.json is `settings`, with these v1
    /// rows already stored, and return v1's `offline_mode` row afterwards.
    fn offline_row_after_import(
        name: &str,
        settings: &[u8],
        v1_rows: &[(&str, &str)],
    ) -> Option<String> {
        let dir = TestDir::new(name);
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        dir.write("legacy/storage/app/private/settings.json", settings);
        let own: Vec<(String, String)> = v1_rows
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        if !own.is_empty() {
            state.save_setting_rows(own).expect("v1 rows");
        }
        state.import_running.store(true, Ordering::SeqCst);
        let mut seen = Vec::new();
        run_import(
            &state,
            &path.to_string_lossy(),
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );
        assert!(
            matches!(seen.last(), Some(ImportProgress::Finished { .. })),
            "{seen:?}"
        );
        let row = stored(&state, mymtgo_api::OFFLINE_MODE);
        lock(&state.tracker).take().expect("tracker").stop();
        row
    }

    // 0.x RenameShareStatsToOfflineModeTest, as the import's seed.
    #[test]
    fn the_import_keeps_a_0x_offline_user_offline() {
        assert_eq!(
            offline_row_after_import("legacy-offline", br#"{"offline_mode":true}"#, &[]),
            Some("true".to_string())
        );
    }

    #[test]
    fn the_import_leaves_a_missing_0x_value_online() {
        assert_eq!(
            offline_row_after_import("legacy-online", br#"{"deck_card_size":"large"}"#, &[]),
            None
        );
    }

    #[test]
    fn the_import_never_replaces_an_existing_v1_offline_row() {
        assert_eq!(
            offline_row_after_import(
                "legacy-own-offline",
                br#"{"offline_mode":true}"#,
                &[(mymtgo_api::OFFLINE_MODE, "false")]
            ),
            Some("false".to_string())
        );
    }

    // Cards spec section 13: after an import the full stub scan runs, so an
    // id stored anywhere without a card row gets one (here a deck cover
    // written without its stub), and the cards worker is woken by the event.
    #[test]
    fn an_import_scans_for_stubs() {
        let dir = TestDir::new("legacy-card-stubs");
        let (state, events) = ready_state(&dir);
        let path = legacy_install(&dir);
        store::Connection::open(&state.db_path)
            .expect("v1")
            .execute_batch(
                "INSERT INTO decks (mtgo_id, name, format, cover_catalog_id, created_at,
                     updated_at, source)
                 VALUES ('q-older', 'Q Older', 'CMODERN', 777, 0, 0, 'file');",
            )
            .expect("a cover without a card row");
        state.import_running.store(true, Ordering::SeqCst);
        let mut seen = Vec::new();
        run_import(
            &state,
            &path.to_string_lossy(),
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );
        assert!(
            matches!(seen.last(), Some(ImportProgress::Finished { .. })),
            "{seen:?}"
        );
        let stubs: Vec<i64> = {
            let reader = lock(&state.reader);
            let mut stmt = reader
                .prepare("SELECT catalog_id FROM cards WHERE catalog_id = 777")
                .expect("prepare");
            stmt.query_map([], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        assert_eq!(stubs, vec![777]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut woken = false;
        while !woken && std::time::Instant::now() < deadline {
            if let Ok(event) = events.recv_timeout(std::time::Duration::from_millis(100)) {
                woken = crate::bridge::wakes_the_cards_worker(&event);
            }
        }
        assert!(woken, "CardStubsAdded wakes the cards worker");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn the_0x_image_folder_is_handed_over_and_the_flag_is_not_carried() {
        let dir = TestDir::new("legacy-local-images");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"local_images":true,"card_stats_trust":80,"card_images_path":"D:/Q/mymtgo-card-images"}"#,
        );
        // v1 saved its settings before (every field has a row).
        state
            .save_setting_rows(crate::settings::Settings::default().to_rows())
            .expect("v1 rows");
        state.import_running.store(true, Ordering::SeqCst);

        let source = run_import(
            &state,
            &path.to_string_lossy(),
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |_| {},
        );

        assert_eq!(
            source,
            Some(PathBuf::from("D:/Q/mymtgo-card-images")),
            "0.x's folder is handed to the cards worker"
        );
        let rows = store::read::settings(&lock(&state.reader)).expect("settings");
        assert!(
            rows.iter().all(|(key, _)| key != "local_images"),
            "no local_images row is written"
        );
        let settings = crate::settings::Settings::from_rows(&rows);
        assert_eq!(settings.card_stats_trust, 80);
        assert_eq!(settings.card_images_path, None, "v1 keeps its own folder");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRq-legacy-png";

    /// Run an import to its end and return its summary.
    fn finished_import(state: &crate::commands::AppState, path: &Path) -> ImportSummary {
        state.import_running.store(true, Ordering::SeqCst);
        let mut seen = Vec::new();
        run_import(
            state,
            &path.to_string_lossy(),
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );
        match seen.pop() {
            Some(ImportProgress::Finished { summary }) => *summary,
            other => panic!("not finished: {other:?}"),
        }
    }

    fn overlay_files(state: &crate::commands::AppState) -> Vec<String> {
        let folder = crate::overlay::background::folder(&state.data_dir);
        let mut names: Vec<String> = match std::fs::read_dir(folder) {
            Ok(entries) => entries
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    // Overlay spec section 12: 0.x's deck labels land on their decks by
    // mtgo_id (an id of no deck is counted), the background is copied into
    // v1's overlay folder, the internal publish keys are not kept, and a
    // rerun keeps what v1 has.
    #[test]
    fn the_import_brings_the_deck_labels_and_the_custom_background() {
        let dir = TestDir::new("legacy-overlay");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"overlay_deck_labels":{"1":" Q stream burn ","7":"Q gone"},
                "overlay_background_path":"background-1767225600.png",
                "overlay_background_remote":{"url":"https://example.invalid/q.png","sha1":"q"},
                "overlay_last_published_at":5,"overlay_publish_error":"rejected"}"#,
        );
        dir.write("legacy/storage/app/overlay/background-1767225600.png", PNG);

        let first = finished_import(&state, &path);
        assert_eq!(
            first.overlay,
            Some(OverlaySummary {
                labels_added: 1,
                labels_kept: 0,
                labels_unknown: 1,
                background: BackgroundImport::Copied,
            })
        );
        let label: Option<String> = lock(&state.reader)
            .query_row(
                "SELECT overlay_label FROM decks WHERE mtgo_id = '1001'",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        assert_eq!(label.as_deref(), Some("Q stream burn"));
        let files = overlay_files(&state);
        assert_eq!(files.len(), 1, "{files:?}");
        let rows = store::read::settings(&lock(&state.reader)).expect("rows");
        let name = crate::settings::overlay_background_file(&rows).expect("the row");
        assert_eq!(files[0], name);
        assert!(crate::overlay::background::is_background_name(&name));
        let copied = crate::overlay::background::folder(&state.data_dir).join(&name);
        assert_eq!(std::fs::read(copied).expect("copy"), PNG);
        assert!(
            dir.join("legacy/storage/app/overlay/background-1767225600.png")
                .is_file(),
            "0.x's file is only read"
        );
        let kept = stored(&state, LEGACY_SETTINGS).expect("kept settings");
        for internal in [
            "overlay_background_remote",
            "overlay_last_published_at",
            "overlay_publish_error",
        ] {
            assert!(!kept.contains(internal), "{internal}: {kept}");
        }
        assert_eq!(crate::settings::overlay_background_remote(&rows), None);

        // The streamer relabels the deck in v1; a rerun keeps the label and
        // the background.
        state
            .tracker_part(tracker::TrackerHandle::deck_editor)
            .expect("tracker")
            .edit(store::decks::DeckEdit::SetOverlayLabel {
                deck_id: lock(&state.reader)
                    .query_row("SELECT id FROM decks WHERE mtgo_id = '1001'", [], |row| {
                        row.get(0)
                    })
                    .expect("deck id"),
                label: Some("Q renamed".to_string()),
            })
            .expect("relabel");
        let again = finished_import(&state, &path);
        assert_eq!(
            again.overlay,
            Some(OverlaySummary {
                labels_added: 0,
                labels_kept: 1,
                labels_unknown: 1,
                background: BackgroundImport::Kept,
            })
        );
        let label: Option<String> = lock(&state.reader)
            .query_row(
                "SELECT overlay_label FROM decks WHERE mtgo_id = '1001'",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        assert_eq!(label.as_deref(), Some("Q renamed"));
        assert_eq!(overlay_files(&state), files, "nothing copied again");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    // U12 review I3: the overlay worker hears the imported overlay settings
    // at once (as after a save), and the background check.
    #[test]
    fn the_overlay_worker_is_told_the_imported_settings() {
        use crate::overlay::worker::OverlayMsg;
        let dir = TestDir::new("legacy-overlay-settings");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"game_overlay":true,"overlay_size":"compact","overlay_show_reveals":false}"#,
        );
        finished_import(&state, &path);
        let messages = overlay_messages_after_import(&state);
        assert!(matches!(messages[0], OverlayMsg::BackgroundChanged));
        let Some(OverlayMsg::SettingsChanged { settings }) = messages.get(1) else {
            panic!("no settings message");
        };
        assert!(settings.game_overlay);
        assert_eq!(settings.overlay_size, overlay::OverlaySize::Compact);
        assert!(!settings.overlay_show_reveals);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_missing_or_refused_0x_background_is_skipped() {
        let dir = TestDir::new("legacy-overlay-missing");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"overlay_background_path":"background-1767225600.png"}"#,
        );
        let missing = finished_import(&state, &path);
        assert_eq!(
            missing.overlay.map(|overlay| overlay.background),
            Some(BackgroundImport::Missing)
        );
        // Not an image, whatever its name says.
        dir.write(
            "legacy/storage/app/overlay/background-1767225600.png",
            b"Q not an image",
        );
        let refused = finished_import(&state, &path);
        assert_eq!(
            refused.overlay.map(|overlay| overlay.background),
            Some(BackgroundImport::Refused)
        );
        let rows = store::read::settings(&lock(&state.reader)).expect("rows");
        assert_eq!(crate::settings::overlay_background_file(&rows), None);
        assert!(overlay_files(&state).is_empty());

        // Without a background setting there is nothing to copy.
        dir.write("legacy/storage/app/private/settings.json", b"{}");
        let none = finished_import(&state, &path);
        assert_eq!(
            none.overlay,
            Some(OverlaySummary {
                background: BackgroundImport::NotSet,
                ..OverlaySummary::default()
            })
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn the_api_crate_names_the_kept_settings_row_as_the_import_does() {
        assert_eq!(mymtgo_api::KEPT_0X_SETTINGS, LEGACY_SETTINGS);
    }

    #[test]
    fn an_import_runs_through_the_tracker_and_stores_its_summary() {
        let dir = TestDir::new("legacy-run");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        let path_text = path.to_string_lossy().into_owned();
        state.import_running.store(true, Ordering::SeqCst);

        let mut seen = Vec::new();
        run_import(
            &state,
            &path_text,
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );

        assert_eq!(seen[0], ImportProgress::Reading);
        assert_eq!(seen[1], ImportProgress::Writing { done: 0, total: 1 });
        assert_eq!(seen[2], ImportProgress::Writing { done: 1, total: 1 });
        assert_eq!(seen[3], ImportProgress::Details { done: 0, total: 1 });
        assert_eq!(seen[4], ImportProgress::Details { done: 1, total: 1 });
        assert_eq!(seen[5], ImportProgress::Decks);
        assert_eq!(seen[6], ImportProgress::Leagues);
        assert_eq!(seen[7], ImportProgress::Limited);
        assert_eq!(seen[8], ImportProgress::Archetypes);
        let ImportProgress::Finished { summary } = &seen[9] else {
            panic!("finished: {seen:?}");
        };
        assert_eq!(seen.len(), 10);
        assert_eq!(
            summary.archetypes,
            Some(ArchetypeSummary::default()),
            "a 0.x without archetype rows imports none"
        );
        assert_eq!(summary.path, path_text);
        assert_eq!((summary.added, summary.skipped_in_progress), (1, 1));
        let decks = summary.decks.expect("deck counts");
        assert_eq!(
            (
                decks.tables_found,
                decks.cards_added,
                decks.decks_added,
                decks.versions_added,
                decks.links_added
            ),
            (true, 1, 1, 1, 1)
        );
        let leagues = summary.leagues.expect("league counts");
        assert_eq!(
            (leagues.tables_found, leagues.added, leagues.without_account),
            (true, 2, 1),
            "the draft league is a limited seed now; with no matches or deck and two \
             accounts, it has none"
        );
        let limited = summary.limited.expect("limited counts");
        assert_eq!(
            (
                limited.tables_found,
                limited.drafts_added,
                limited.notes_added
            ),
            (true, 1, 1)
        );
        assert_eq!(
            store::limited::pick_notes(&lock(&state.reader), "q-draft-token")
                .expect("notes")
                .get(&1)
                .map(String::as_str),
            Some("Q note")
        );
        assert!(!state.import_running.load(Ordering::SeqCst), "free again");

        let count: i64 = lock(&state.reader)
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE source = 'imported'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
        let members: i64 = lock(&state.reader)
            .query_row(
                "SELECT COUNT(*) FROM matches
                 WHERE token = 'q-token-1' AND league_linked = 1 AND league_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("members");
        assert_eq!(members, 1);
        let reader = lock(&state.reader);
        let number = |sql: &str| -> i64 { reader.query_row(sql, [], |row| row.get(0)).expect(sql) };
        assert_eq!(
            number(
                "SELECT COUNT(*) FROM game_cards c JOIN games g ON g.id = c.game_id
                 JOIN matches m ON m.id = g.match_id
                 WHERE m.token = 'q-token-1' AND c.side = 'local' AND c.main_qty > 0"
            ),
            1,
            "the game's deck list is stored as its rows"
        );
        assert_eq!(
            number("SELECT frame_count FROM game_timelines"),
            2,
            "a repeat is dropped, but a game keeps its last frame (replays ruling 4)"
        );
        assert_eq!(number("SELECT entry_count FROM game_logs"), 1);
        drop(reader);
        let last = stored(&state, LEGACY_LAST_IMPORT).expect("summary stored");
        assert_eq!(
            serde_json::from_str::<ImportSummary>(&last).expect("parses"),
            **summary
        );
        assert_eq!(
            stored(&state, LEGACY_SETTINGS),
            Some(r#"{"deck_card_size":"large"}"#.to_string())
        );
        let known = crate::scrub::known().expect("names");
        assert!(known.contains(&"Qopponent".to_string()));
        assert!(known.contains(&"Qdeckowner".to_string()));
        assert!(
            known.contains(&"Qquiet".to_string()),
            "an account without matches or decks"
        );
        assert_eq!(
            summary.accounts,
            Some(AccountSummary {
                added: 3,
                unchanged: 0,
                login_ids_filled: 1,
                login_id_clashes: 0,
                untracked: 0,
                active_seeded: false,
            }),
            "the deleted account is left out"
        );
        let accounts: String = lock(&state.reader)
            .query_row(
                "SELECT group_concat(row, ';') FROM (SELECT concat_ws(',', username,
                     IFNULL(login_id, ''), first_seen_at) AS row FROM accounts ORDER BY username)",
                [],
                |row| row.get(0),
            )
            .expect("accounts");
        assert!(
            accounts.starts_with("Qdeckowner,4321,")
                && accounts.contains(";Qquiet,,1764547200000")
                && !accounts.contains("Qgone"),
            "{accounts}"
        );
        assert_eq!(
            info(&state, None).expect("info").last_import,
            Some((**summary).clone())
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_match_without_a_local_seat_or_games_is_imported_under_the_deck_s_account() {
        let dir = TestDir::new("legacy-gameless");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        store::Connection::open(&path)
            .expect("open 0.x test database")
            .execute_batch(
                "INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won,
                     games_lost, started_at, ended_at, state, deck_version_id)
                 VALUES (3, 'q-token-3', '4244', 'CMODERN', 'Constructed', 0, 2,
                     '2026-01-03 03:04:05', NULL, 'complete', 1);",
            )
            .expect("a gameless match");
        let path_text = path.to_string_lossy().into_owned();
        state.import_running.store(true, Ordering::SeqCst);

        let mut seen = Vec::new();
        run_import(
            &state,
            &path_text,
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );

        let Some(ImportProgress::Finished { summary }) = seen.last() else {
            panic!("finished: {seen:?}");
        };
        assert_eq!(
            (summary.added, summary.skipped_no_local_player),
            (2, 0),
            "the gameless match is imported, not skipped"
        );
        let row: String = lock(&state.reader)
            .query_row(
                "SELECT concat_ws(',', a.username, m.games_won_imported, m.games_lost_imported)
                 FROM matches m JOIN accounts a ON a.id = m.account_id
                 WHERE m.token = 'q-token-3'",
                [],
                |row| row.get(0),
            )
            .expect("the gameless match");
        assert_eq!(row, "Qdeckowner,0,2");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// Issue #337: a 0.x match whose local player is not one of 0.x's
    /// accounts is skipped and counted, registers no account, and takes the
    /// real name of a tournament only it played with it. A rerun is the same.
    #[test]
    fn a_match_of_a_player_who_is_not_a_0x_account_is_skipped() {
        let dir = TestDir::new("legacy-not-account");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        store::Connection::open(&path)
            .expect("open 0.x test database")
            .execute_batch(
                "INSERT INTO players (id, username) VALUES (3, 'Qstranger');
                 INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won,
                     games_lost, started_at, ended_at, state, deck_version_id,
                     tournament_event_id)
                 VALUES (3, 'q-token-3', '4244', 'CMODERN', 'Constructed', 2, 0,
                     '2026-01-03 03:04:05', NULL, 'complete', NULL, 88);
                 INSERT INTO games (id, match_id, mtgo_id, won, started_at)
                 VALUES (30, 3, '30', 1, '2026-01-03 03:04:05');
                 INSERT INTO game_player (game_id, player_id, instance_id, is_local)
                 VALUES (30, 3, 1, 1), (30, 2, 1, 0);
                 INSERT INTO tournaments (mtgo_event_id, token, name, format, started_at,
                     name_synthesized)
                 VALUES (88, 'q-stranger-cup', 'Q Stranger Cup', 'CMODERN',
                     '2026-01-03 03:00:00', 0);",
            )
            .expect("another player's match");
        let path_text = path.to_string_lossy().into_owned();
        for run in 1..=2 {
            state.import_running.store(true, Ordering::SeqCst);
            let mut seen = Vec::new();
            run_import(
                &state,
                &path_text,
                &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
                &mut |progress| seen.push(progress),
            );
            let Some(ImportProgress::Finished { summary }) = seen.last() else {
                panic!("finished: {seen:?}");
            };
            assert_eq!(summary.skipped_not_account, 1, "run {run}");
            let reader = lock(&state.reader);
            let number =
                |sql: &str| -> i64 { reader.query_row(sql, [], |row| row.get(0)).expect("number") };
            assert_eq!(
                number("SELECT COUNT(*) FROM matches WHERE token = 'q-token-3'"),
                0
            );
            assert_eq!(
                number("SELECT COUNT(*) FROM accounts WHERE username = 'Qstranger'"),
                0,
                "run {run}"
            );
            assert_eq!(
                number("SELECT COUNT(*) FROM tournaments WHERE mtgo_event_id = 88"),
                0
            );
        }
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_match_whose_detail_cannot_be_read_is_skipped_and_the_import_goes_on() {
        let dir = TestDir::new("legacy-detail-error");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        // Text that is not UTF-8 makes the detail read of the match fail.
        store::Connection::open(&path)
            .expect("open 0.x test database")
            .execute_batch(
                "INSERT INTO game_timelines (game_id, timestamp, content)
                 VALUES (10, '2026-01-02 03:04:08', CAST(X'FF' AS TEXT));",
            )
            .expect("a broken frame");
        let path_text = path.to_string_lossy().into_owned();
        state.import_running.store(true, Ordering::SeqCst);

        let mut seen = Vec::new();
        run_import(
            &state,
            &path_text,
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );

        assert_eq!(seen[3], ImportProgress::Details { done: 0, total: 1 });
        assert_eq!(
            seen[4],
            ImportProgress::Details { done: 1, total: 1 },
            "the skipped match still counts as done"
        );
        assert_eq!(seen[5], ImportProgress::Decks);
        assert_eq!(seen[6], ImportProgress::Leagues);
        assert_eq!(seen[7], ImportProgress::Limited);
        assert_eq!(seen[8], ImportProgress::Archetypes);
        let ImportProgress::Finished { summary } = &seen[9] else {
            panic!("finished: {seen:?}");
        };
        assert_eq!(summary.added, 1);
        assert_eq!(summary.skipped_details, 1, "the skipped detail is counted");
        assert_eq!(summary.decks.expect("deck counts").decks_added, 1);
        assert_eq!(summary.leagues.expect("league counts").added, 2);
        assert!(stored(&state, LEGACY_LAST_IMPORT).is_some());
        let detail_rows: i64 = lock(&state.reader)
            .query_row(
                "SELECT (SELECT COUNT(*) FROM game_cards) + (SELECT COUNT(*) FROM game_logs)
                     + (SELECT COUNT(*) FROM game_timelines)",
                [],
                |row| row.get(0),
            )
            .expect("detail rows");
        assert_eq!(detail_rows, 0);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_match_with_nothing_to_import_or_a_gone_token_is_not_a_failed_detail() {
        let dir = TestDir::new("legacy-detail-empty");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        store::Connection::open(&path)
            .expect("open 0.x test database")
            .execute_batch(
                "DELETE FROM game_timelines; DELETE FROM game_logs;
                 DELETE FROM game_player; DELETE FROM games;",
            )
            .expect("a match with no detail");
        let path_text = path.to_string_lossy().into_owned();
        state.import_running.store(true, Ordering::SeqCst);

        let mut seen = Vec::new();
        run_import(
            &state,
            &path_text,
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );
        let Some(ImportProgress::Finished { summary }) = seen.last() else {
            panic!("finished: {seen:?}");
        };
        assert_eq!(
            summary.skipped_details, 0,
            "nothing to import is no failure"
        );

        // A token that is no longer a 0.x row in v1: skipped, not failed.
        let gone = vec![DetailTarget {
            token: "q-gone-token".to_string(),
            match_id: 1,
            local_username: "Qlocal".to_string(),
        }];
        let (counts, failed) = import_details(&state, &path_text, &gone, &mut |_| {});
        assert_eq!(counts.skipped, 1);
        assert_eq!(failed, 0);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_panicking_import_fails_and_frees_the_import() {
        let dir = TestDir::new("legacy-panic");
        let (state, _events) = ready_state(&dir);
        state.import_running.store(true, Ordering::SeqCst);

        let mut seen = Vec::new();
        run_import(
            &state,
            "Q:/never-read.sqlite",
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| {
                if progress == ImportProgress::Reading {
                    panic!("Q test panic");
                }
                seen.push(progress);
            },
        );

        assert_eq!(
            seen,
            vec![ImportProgress::Failed {
                message: IMPORT_PANICKED.to_string()
            }]
        );
        assert!(!state.import_running.load(Ordering::SeqCst));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn an_unreadable_database_fails_and_frees_the_import() {
        let dir = TestDir::new("legacy-fail");
        let (state, _events) = ready_state(&dir);
        dir.write("not-0x.sqlite", b"not a database");
        let path = dir.join("not-0x.sqlite").to_string_lossy().into_owned();
        state.import_running.store(true, Ordering::SeqCst);

        let mut seen = Vec::new();
        run_import(
            &state,
            &path,
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );

        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0], ImportProgress::Reading);
        assert!(matches!(seen[1], ImportProgress::Failed { .. }));
        assert!(!state.import_running.load(Ordering::SeqCst));
        assert_eq!(stored(&state, LEGACY_LAST_IMPORT), None);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    // Review M4 (issue #310): the summary says what the import did to
    // tracking and the current account.
    #[test]
    fn the_account_summary_carries_untracked_and_the_seeded_switch() {
        let summary = account_summary(AccountImportCounts {
            untracked: 2,
            active_seeded: true,
            ..AccountImportCounts::default()
        });
        assert_eq!((summary.untracked, summary.active_seeded), (2, true));
        // A summary stored before these counts reads them as none.
        let old: AccountSummary = serde_json::from_str(
            r#"{"added":1,"unchanged":0,"loginIdsFilled":0,"loginIdClashes":0}"#,
        )
        .expect("an old summary");
        assert_eq!((old.untracked, old.active_seeded), (0, false));
    }

    // Review M2 (issue #310): the account import may seed `active_account`,
    // so it waits for a settings save in progress, which would otherwise
    // write the switch it read back over the seed.
    #[test]
    fn the_account_import_waits_for_a_settings_save() {
        let dir = TestDir::new("legacy-accounts-lock");
        let (state, _events) = ready_state(&dir);
        let accounts = vec![ImportedAccount {
            username: "Qactive".to_string(),
            login_id: None,
            created_at_ms: None,
            tracked: true,
            active_since_ms: Some(1_000),
        }];
        let (done_sender, done) = std::sync::mpsc::channel();
        let held = lock(&state.settings_lock);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let result = import_accounts(&state, accounts);
                done_sender.send(result.is_ok()).expect("send");
            });
            // Generous wait: an import that ignores the lock is far quicker.
            let waited = done
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_err();
            // Release before asserting, so the scope always ends.
            drop(held);
            assert!(waited, "the account import must wait for the save");
            assert!(
                done.recv_timeout(std::time::Duration::from_secs(30))
                    .expect("finished")
            );
        });
        lock(&state.tracker).take().expect("tracker").stop();
    }
}

#[cfg(test)]
mod import_start_tests {
    use super::tests::legacy_install;
    use super::*;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use store::archetype_edit::{ArchetypeEdit, MatchDetection};

    #[test]
    fn the_earliest_unfinished_start_wins() {
        assert_eq!(import_start(None, 5_000), 5_000);
        assert_eq!(import_start(Some(1_000), 5_000), 1_000);
        assert_eq!(import_start(Some(9_000), 5_000), 5_000);
        assert_eq!(stored_start(None), None);
        assert_eq!(stored_start(Some("null")), None);
        assert_eq!(stored_start(Some("1234")), Some(1_234));
    }

    fn stored(state: &AppState) -> Option<i64> {
        unfinished_import_start(&lock(&state.reader)).expect("read")
    }

    /// Archetype rows for the `legacy_install` 0.x database: match 1's
    /// opponent was detected as Q Tron, and match 3 (a second complete
    /// match) had its opponent set by hand to Q Manual.
    fn add_archetypes(path: &Path) {
        let conn = store::Connection::open(path).expect("open 0.x");
        conn.execute_batch(
            "INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won,
                 games_lost, started_at, ended_at, state)
             VALUES (3, 'q-token-3', '4244', 'CMODERN', 'Constructed', 1, 0,
                 '2026-01-02 05:04:05', '2026-01-02 05:34:05', 'complete');
             INSERT INTO games (id, match_id, mtgo_id, won, started_at)
             VALUES (30, 3, '30', 1, '2026-01-02 05:04:05');
             INSERT INTO game_player (game_id, player_id, instance_id, is_local)
             VALUES (30, 1, 1, 1), (30, 2, 1, 0);
             INSERT INTO archetypes (id, uuid, name, format, manual, is_fallback, created_at,
                 updated_at)
             VALUES (10, 'q-uuid-tron', 'Q Tron', 'modern', 0, 0, '2026-01-01 00:00:00',
                     '2026-01-01 00:00:00'),
                    (20, 'qdev0001-manual', 'Q Manual', 'modern', 1, 0,
                     '2026-01-01 00:00:00', '2026-01-01 00:00:00');
             INSERT INTO match_archetypes (id, archetype_id, mtgo_match_id, player_id,
                 confidence, manual)
             VALUES (1, 10, 1, 2, 0.8, 0), (2, 20, 3, 2, 1, 1);",
        )
        .expect("0.x archetypes");
    }

    fn opponent_rows(state: &AppState) -> Vec<String> {
        let reader = lock(&state.reader);
        let mut stmt = reader
            .prepare(
                "SELECT m.token || '|' || a.uuid || '|' || x.manual FROM match_archetypes x
                 JOIN matches m ON m.id = x.match_id JOIN archetypes a ON a.id = x.archetype_id
                 WHERE x.is_opponent = 1 ORDER BY m.token",
            )
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    #[test]
    fn a_finished_import_clears_its_start() {
        let dir = TestDir::new("legacy-start-cleared");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        state.import_running.store(true, Ordering::SeqCst);
        run_import(
            &state,
            &path.to_string_lossy(),
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |_| {},
        );
        assert_eq!(stored(&state), None);
        assert!(!import_gate_closed(&state, &lock(&state.reader)));
    }

    /// Fix round 2 (N1): the app closed after the match batches of a first
    /// import; on the next start the stored start keeps imported matches
    /// out of the backfill, and a detection that marked them anyway (by
    /// hand) is no v1 decision on the rerun: 0.x's auto and hand-set
    /// opponent rows land, and the finished rerun clears the start.
    #[test]
    fn an_interrupted_import_keeps_its_start_so_a_rerun_lands_0x_opponent_rows() {
        let dir = TestDir::new("legacy-interrupted");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        add_archetypes(&path);
        let path_text = path.to_string_lossy().into_owned();

        // The first run, a minute ago: its start is stored, then the
        // matches, then the app closes.
        let first_start = crate::wall_clock_ms() - 60_000;
        state
            .save_setting_rows(vec![started_row(first_start)])
            .expect("start stored");
        let read = importer::read_database(&path).expect("read");
        importer(&state)
            .and_then(|importer| importer.import(read.matches))
            .expect("matches");
        assert_eq!(stored(&state), Some(first_start));
        assert!(
            import_gate_closed(&state, &lock(&state.reader)),
            "the backfill leaves imported matches alone after the restart"
        );
        // Detection ran on them anyway (the match page "Detect"), after the
        // first start and well before the rerun's.
        let ids: Vec<i64> = {
            let reader = lock(&state.reader);
            let mut stmt = reader
                .prepare("SELECT id FROM matches WHERE token IN ('q-token-1', 'q-token-3')")
                .expect("prepare");
            stmt.query_map([], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        assert_eq!(ids.len(), 2);
        let editor = lock(&state.tracker)
            .as_ref()
            .expect("tracker")
            .archetype_editor();
        editor
            .edit(ArchetypeEdit::SetDetections {
                results: ids
                    .iter()
                    .map(|id| MatchDetection {
                        match_id: *id,
                        deck_archetype_id: None,
                        deck_archetype: None,
                        player: None,
                        opponent: None,
                    })
                    .collect(),
                now_ms: first_start + 1,
            })
            .expect("detected with nothing");
        let marked: i64 = lock(&state.reader)
            .query_row(
                "SELECT count(*) FROM matches WHERE archetypes_detected_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("marked");
        assert_eq!(marked, 2, "both matches are marked detected");

        // The rerun, after the restart.
        state.import_running.store(true, Ordering::SeqCst);
        let mut seen = Vec::new();
        run_import(
            &state,
            &path_text,
            &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
            &mut |progress| seen.push(progress),
        );
        assert!(
            matches!(seen.last(), Some(ImportProgress::Finished { .. })),
            "{seen:?}"
        );

        assert_eq!(
            opponent_rows(&state),
            vec![
                "q-token-1|q-uuid-tron|0".to_string(),
                "q-token-3|qdev0001-manual|1".to_string()
            ]
        );
        assert_eq!(stored(&state), None, "the finished rerun clears the start");
    }
}

/// Launch at login as the autostart plugin would report it, for tests.
#[cfg(test)]
pub(crate) mod fake_launch {
    use std::cell::Cell;

    use super::LaunchAtLogin;

    #[derive(Default)]
    pub(crate) struct FakeLaunchAtLogin {
        pub(crate) on: Cell<bool>,
        pub(crate) enables: Cell<u32>,
        /// The OS refuses every change.
        pub(crate) refuse: bool,
    }

    impl LaunchAtLogin for FakeLaunchAtLogin {
        fn is_enabled(&self) -> Result<bool, String> {
            Ok(self.on.get())
        }

        fn enable(&self) -> Result<(), String> {
            if self.refuse {
                return Err("denied".to_string());
            }
            self.on.set(true);
            self.enables.set(self.enables.get() + 1);
            Ok(())
        }
    }
}

// Release spec section 13 item 4 and ruling 16: 0.x's launch at login is
// applied once, by the import, while v1's was never changed.
#[cfg(test)]
mod launch_at_login_tests {
    use super::fake_launch::FakeLaunchAtLogin;
    use super::tests::legacy_install;
    use super::*;
    use crate::commands::tests::ready_state;
    use crate::settings::AUTOSTART_FROM_0X_APPLIED;
    use fixture_tools::TestDir;

    fn applied(state: &AppState) -> Option<String> {
        store::read::settings(&lock(&state.reader))
            .expect("settings")
            .into_iter()
            .find(|(key, _)| key == AUTOSTART_FROM_0X_APPLIED)
            .map(|(_, value)| value)
    }

    fn import_with(state: &AppState, path: &Path, launch: &FakeLaunchAtLogin) {
        state.import_running.store(true, Ordering::SeqCst);
        let mut seen = Vec::new();
        run_import(state, &path.to_string_lossy(), launch, &mut |progress| {
            seen.push(progress)
        });
        assert!(
            matches!(seen.last(), Some(ImportProgress::Finished { .. })),
            "{seen:?}"
        );
    }

    /// A 0.x install whose settings.json has launch at login on.
    fn install_with_launch_at_login(dir: &TestDir) -> PathBuf {
        let path = legacy_install(dir);
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"deck_card_size":"large","autostart_enabled":true}"#,
        );
        path
    }

    #[test]
    fn the_0x_value_is_carried_only_while_v1_never_chose() {
        assert!(carry_launch_at_login(true, false, false));
        assert!(
            !carry_launch_at_login(false, false, false),
            "0.x had it off"
        );
        assert!(
            !carry_launch_at_login(true, true, false),
            "v1's is on already"
        );
        assert!(!carry_launch_at_login(true, false, true), "settled before");
    }

    #[test]
    fn launch_at_login_is_applied_once_from_0x() {
        let dir = TestDir::new("legacy-launch-once");
        let (state, _events) = ready_state(&dir);
        let path = install_with_launch_at_login(&dir);
        let launch = FakeLaunchAtLogin::default();

        import_with(&state, &path, &launch);
        assert!(launch.on.get(), "0.x's launch at login is on in v1");
        assert_eq!(launch.enables.get(), 1);
        assert_eq!(applied(&state), Some("true".to_string()));

        // The user turns it off again; a second import leaves it off.
        launch.on.set(false);
        import_with(&state, &path, &launch);
        assert!(!launch.on.get(), "not twice");
        assert_eq!(launch.enables.get(), 1);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn launch_at_login_changed_in_v1_is_never_replaced() {
        let dir = TestDir::new("legacy-launch-changed");
        let (state, _events) = ready_state(&dir);
        let path = install_with_launch_at_login(&dir);
        let launch = FakeLaunchAtLogin::default();
        assert_eq!(applied(&state), None);

        // The user switched launch at login on and off again in v1.
        launch_at_login_changed(&state).expect("recorded");
        assert_eq!(applied(&state), Some("true".to_string()));

        import_with(&state, &path, &launch);
        assert!(!launch.on.get());
        assert_eq!(launch.enables.get(), 0);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn launch_at_login_off_in_0x_or_refused_is_not_recorded() {
        let dir = TestDir::new("legacy-launch-off");
        let (state, _events) = ready_state(&dir);
        let path = legacy_install(&dir);
        let launch = FakeLaunchAtLogin::default();

        // 0.x had it off (settings.json has no `autostart_enabled`).
        import_with(&state, &path, &launch);
        assert_eq!(launch.enables.get(), 0);
        assert_eq!(applied(&state), None);

        // The OS refuses: nothing is recorded, so a later import tries again.
        dir.write(
            "legacy/storage/app/private/settings.json",
            br#"{"autostart_enabled":true}"#,
        );
        let refusing = FakeLaunchAtLogin {
            refuse: true,
            ..FakeLaunchAtLogin::default()
        };
        import_with(&state, &path, &refusing);
        assert_eq!(applied(&state), None);
        import_with(&state, &path, &launch);
        assert_eq!(launch.enables.get(), 1);
        assert_eq!(applied(&state), Some("true".to_string()));
        lock(&state.tracker).take().expect("tracker").stop();
    }
}
