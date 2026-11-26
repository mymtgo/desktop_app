//! The cross-device sync run (survey section 12, 07 spec section 10, 0.x
//! `SyncRunner`, `SyncController`, `SetDeckCloudSync`): manifest, push and
//! pull per type in the fixed order deck, league, match, then attestation
//! and the toast. v1 pushes the rows it owns (file decks, log and manual
//! matches and leagues; ruling 5) and pulls everything through the 0.x
//! import paths.
//!
//! Runs on the `api` worker thread (`Job::Sync`) or inside
//! `spawn_blocking`. Reads use a read-only connection; every write goes
//! through the tracker (`SyncEditor`). One run at a time: a trigger during
//! a run queues one follow-up (0.x `RunSyncJob`, unique until processing).
//!
//! `NotLinked` and a held gate (offline mode) end the
//! run quietly. A failure to reach MyMTGO (no connection or a timeout, as
//! at launch or wake) is only logged while the worker's retries
//! (`TransientRetry`) are left; any other failure is stored for the
//! Account page (`sync_last_error`). Every step is safe to repeat, so the
//! next run simply diffs again.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use importer::bundle_read::{pulled_league, read_match_bundle};
use mymtgo_api::{
    ApiError, ApiRequest, ApiResponse, BLOBS_PER_REQUEST, BlobUpload, COMPRESSED_PER_BLOB,
    COMPRESSED_PER_REQUEST, DECK_SLOT_PASSTHROUGH, DirtyEntry, FetchedBlob, MANIFEST_CHUNK,
    RequestClass, SlotError, Slots, SyncFailure, SyncType, deck_slot_request, fetch_request, gate,
    gzip_blob, manifest_request, parse_blobs, parse_deck_slot, parse_fetch, parse_manifest,
    parse_slots, slots_request, sync_answer, sync_status_request,
};
use serde::Serialize;
use serde_json::Value;
use store::Connection;
use store::import_decks::{ImportedDeck, ImportedVersion};
use store::import_limited::LegacyLimitedDeck;
use store::sync::{BuiltRow, DeckCandidate, Rejection, SyncEdit, SyncedRow};
use sync_bundles::league::{
    arena_run_bundle, arena_run_changed_at, is_arena_run_bundle, league_bundle, league_sidecar,
    read_arena_run_bundle, read_league_bundle,
};
use sync_bundles::match_bundle::{match_bundle_owned, match_sidecar};
use sync_bundles::{
    BundlePlatform, CANONICAL_VERSION, Carry, DECK_CARRY, LEAGUE_CARRY, MATCH_CARRY,
    bundle_datetime, bundle_hash, canonical_json, deck_bundle, deck_client_id, deck_sidecar,
    parse_bundle_datetime, read_deck_bundle,
};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tracker::SyncEditor;

use crate::api::attest::attest_known_accounts_with;
use crate::api::device::ensure_device_id;
use crate::api::random::random_bytes;
use crate::api::session::ApiState;
use crate::api::worker::{WorkerSignal, signal};
use crate::bridge::lock;
use crate::cards::worker::CardsSignal;
use crate::commands::{CommandError, Startup, off_async_workers, ready, storage};
use crate::events::{AppNotification, SyncFinished};

/// The order every run takes the types in.
pub const SYNC_TYPES: [SyncType; 3] = [SyncType::Deck, SyncType::League, SyncType::Match];
/// The activity feed keeps the last lines of the current run.
pub const ACTIVITY_LINES: usize = 40;
/// The local rejection for a blob too large to send.
pub const TOO_LARGE_LOCAL: &str = "too_large_local";
/// 0.x's refusal of a deck toggle while signed out.
pub const NOT_LINKED_MESSAGE: &str = "Sign in from Settings before syncing decks.";
/// 0.x's refusal of a limited deck on the free tier.
pub const SUPPORTER_MESSAGE: &str = "Syncing draft and sealed decks is a supporter feature.";

/// The run's limits (0.x `sync_client.limits`), the survey's values by
/// default; tests make them small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub manifest_chunk: usize,
    pub blobs_per_request: usize,
    pub compressed_per_request: u64,
    pub compressed_per_blob: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            manifest_chunk: MANIFEST_CHUNK,
            blobs_per_request: BLOBS_PER_REQUEST,
            compressed_per_request: COMPRESSED_PER_REQUEST,
            compressed_per_blob: COMPRESSED_PER_BLOB,
        }
    }
}

/// Runs that ask for a carry the server withholds before the row settles
/// as having none (the API: "ask again later").
const MAX_CARRY_ASKS: u32 = 5;
/// ... and at least this long after the first unanswered ask (runs come in
/// bursts, so a run count alone is not time).
const MIN_CARRY_ASK_AGE_MS: i64 = 24 * 3_600_000;

/// The run's shared state, kept in `ApiState`: whether a run is going, the
/// follow-up a trigger during it asked for, and the activity feed.
#[derive(Default)]
pub struct SyncRunState {
    running: AtomicBool,
    /// A trigger during a run: run again after it (`true`: in full).
    follow_up: Mutex<Option<bool>>,
    /// "Sync now" was pressed and the run has not started yet.
    asked: AtomicBool,
    /// `(ms, line)`, the current run only.
    activity: Mutex<VecDeque<(i64, String)>>,
    /// The platforms the server's last manifest answer confirmed (its
    /// `platforms` echo; none from an API that sends no echo). Kept for
    /// the session, so a deck slot change knows it outside a run.
    confirmed: Mutex<Vec<String>>,
}

impl SyncRunState {
    /// Start a run, or, while one is going, queue one follow-up (full when
    /// any trigger asked for full) and say no.
    pub fn try_start(&self, full: bool) -> bool {
        let mut follow_up = lock(&self.follow_up);
        if self.running.load(Ordering::SeqCst) {
            *follow_up = Some(follow_up.unwrap_or(false) || full);
            return false;
        }
        self.running.store(true, Ordering::SeqCst);
        self.asked.store(false, Ordering::SeqCst);
        true
    }

    /// A run ended: the follow-up to run now (still running), or `None`
    /// (stopped).
    pub fn finish(&self) -> Option<bool> {
        let mut follow_up = lock(&self.follow_up);
        if let Some(full) = follow_up.take() {
            return Some(full);
        }
        self.running.store(false, Ordering::SeqCst);
        None
    }

    /// Withdraw a "Sync now" whose run did not start (the gate refused, or
    /// tracking is restarting), so the page stops reading as syncing. A run
    /// that is going is not touched.
    pub fn clear_asked(&self) {
        self.asked.store(false, Ordering::SeqCst);
    }

    /// A run is going or "Sync now" is waiting for one.
    pub fn syncing(&self) -> bool {
        self.running.load(Ordering::SeqCst) || self.asked.load(Ordering::SeqCst)
    }

    /// "Sync now" (0.x `SyncActivity::markQueued`): the feed reads as
    /// active from the press. A run already going keeps its feed.
    pub fn ask(&self, now_ms: i64) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }
        self.asked.store(true, Ordering::SeqCst);
        self.reset();
        self.log(now_ms, "Sync queued.");
    }

    /// The feed's lines, oldest first.
    pub fn activity(&self) -> Vec<(i64, String)> {
        lock(&self.activity).iter().cloned().collect()
    }

    fn reset(&self) {
        lock(&self.activity).clear();
    }

    /// Keep what a manifest answer confirmed (`None`: no echo, nothing).
    fn set_confirmed(&self, platforms: Option<&[String]>) {
        *lock(&self.confirmed) = platforms.map(<[String]>::to_vec).unwrap_or_default();
    }

    /// Whether the server's last answer confirmed `platform`.
    fn confirms(&self, platform: &str) -> bool {
        lock(&self.confirmed).iter().any(|item| item == platform)
    }

    fn log(&self, now_ms: i64, line: &str) {
        let mut activity = lock(&self.activity);
        activity.push_back((now_ms, line.to_string()));
        while activity.len() > ACTIVITY_LINES {
            activity.pop_front();
        }
    }
}

/// What a run needs from the app.
pub struct SyncContext<'a> {
    pub api: &'a ApiState,
    pub editor: &'a SyncEditor,
    /// A fresh read-only connection, or `None` when it cannot be opened.
    pub reader: &'a dyn Fn() -> Option<Connection>,
    pub now: &'a dyn Fn() -> i64,
    /// Raise the toast `(title, message)`.
    pub notify: &'a dyn Fn(&str, &str),
    /// The pause between attestation attempts.
    pub sleep: &'a dyn Fn(u64),
    pub limits: Limits,
    /// The OS timezone: an imported game's frames, which keep only a time
    /// of day, are dated in it (replays spec section 10).
    pub timezone: Tz,
    /// Called once at the end of a run whose pull imported anything (the
    /// cards worker scans for stubs and fills them; 0.x `SyncRunner` ran
    /// `CreateMissingCardsFromLocalData` there).
    pub pulled: &'a dyn Fn(),
    /// Called once at the end of a run whose deck pull brought sideboard
    /// guides or matchup notes, with their `(deck MTGO id, archetype uuid)`
    /// pairs (the app pushes `GuidesChanged`, #300 F22).
    pub guides_pulled: &'a dyn Fn(&[GuidePair]),
    /// Store a failure to reach MyMTGO for the Account page: the worker's
    /// retries were used. Otherwise it is only logged.
    pub report_unreachable: bool,
}

/// A pulled guide's or note's `(deck MTGO id, archetype uuid)`.
pub type GuidePair = (String, String);

/// How `run_sync` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    Ran,
    /// The run (the last, when a follow-up ran) could not reach MyMTGO.
    Unreachable,
    /// Another run was going: this trigger runs after it.
    Queued,
}

/// Why a run stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// Signed out: quiet.
    NotLinked,
    /// Offline mode: quiet.
    Held,
    /// MyMTGO could not be reached (`ApiError::is_transient`): the worker
    /// tries again soon.
    Unreachable(String),
    /// Anything else, as the Account page shows it.
    Failed(String),
}

impl From<ApiError> for SyncError {
    fn from(error: ApiError) -> SyncError {
        match error {
            ApiError::NotLinked => SyncError::NotLinked,
            ApiError::Offline => SyncError::Held,
            other if other.is_transient() => SyncError::Unreachable(other.to_string()),
            other => SyncError::Failed(other.to_string()),
        }
    }
}

impl From<SyncFailure> for SyncError {
    fn from(failure: SyncFailure) -> SyncError {
        SyncError::Failed(failure.message)
    }
}

impl From<store::StoreError> for SyncError {
    fn from(error: store::StoreError) -> SyncError {
        SyncError::Failed(error.to_string())
    }
}

/// Run a sync now (one at a time; a trigger during a run queues one
/// follow-up, which this call then runs too).
pub fn run_sync(ctx: &SyncContext, full: bool) -> RunEnd {
    let state = &ctx.api.sync;
    if !state.try_start(full) {
        return RunEnd::Queued;
    }
    let mut full = full;
    loop {
        let unreachable = run_once(ctx, full);
        match state.finish() {
            Some(next) => full = next,
            None if unreachable => return RunEnd::Unreachable,
            None => return RunEnd::Ran,
        }
    }
}

/// What one run moved, for the toast.
#[derive(Default)]
struct Moved {
    pushed_matches: u32,
    pulled_matches: u32,
    /// Any blob of any type was imported.
    pulled_any: bool,
    /// Pulled leagues' registered decks, placed again after the match pull.
    snapshots: Vec<LegacyLimitedDeck>,
    /// Pulled Arena runs that recorded nothing (on a new device, no Arena
    /// account yet: it comes with the matches), applied again after the
    /// match pull so they land in this run, not the next.
    arena_runs: Vec<SyncEdit>,
    /// `(deck MTGO id, archetype uuid)` of pulled guides and notes, once
    /// each (#300 F22).
    guide_pairs: Vec<(String, String)>,
    slots_released: bool,
    /// The run turned on its Supporter's limited deck slots (#825).
    limited_slots_checked: bool,
    /// The platforms the server confirmed it takes (the manifest's
    /// `platforms` echo); `None` until an answer names them. An API that
    /// predates them would store an Arena or Endstep row as MTGO and serve
    /// it to old clients, so those rows wait while this is `None`.
    platforms: Option<Vec<String>>,
    /// The run logged that rows wait for the server's confirmation.
    held_logged: bool,
}

impl Moved {
    /// Whether this run may send a row of `platform`: MTGO always, any other
    /// only once the server confirmed it.
    fn sends(&self, platform: BundlePlatform) -> bool {
        match platform.key() {
            None => true,
            Some(key) => self
                .platforms
                .as_ref()
                .is_some_and(|confirmed| confirmed.iter().any(|item| item == key)),
        }
    }
}

/// One run; true when it could not reach MyMTGO.
fn run_once(ctx: &SyncContext, full: bool) -> bool {
    let state = &ctx.api.sync;
    state.reset();
    log(
        ctx,
        if full {
            "Sync started (full reconcile)."
        } else {
            "Sync started."
        },
    );
    let mut moved = Moved::default();
    let result = run_types(ctx, full, &mut moved);
    if moved.pulled_any {
        (ctx.pulled)();
    }
    if !moved.guide_pairs.is_empty() {
        (ctx.guides_pulled)(&moved.guide_pairs);
    }
    match result {
        Ok(()) => {
            attest(ctx);
            if let Err(error) = ctx.editor.edit(SyncEdit::LastError { error: None }) {
                tracing::warn!(%error, "could not clear the last sync error");
            }
            log(ctx, "Sync complete.");
            if let Some(message) = completion_message(moved.pushed_matches, moved.pulled_matches) {
                (ctx.notify)("Synced with MyMTGO", &message);
            }
        }
        Err(SyncError::NotLinked) => {
            log(ctx, "Aborted: this device is not linked.");
            tracing::info!("Sync run aborted: this device is not linked.");
        }
        Err(SyncError::Held) => {
            log(ctx, "Aborted: offline mode is enabled.");
            tracing::info!("Sync run aborted: offline mode is enabled.");
        }
        Err(SyncError::Unreachable(message)) => {
            if ctx.report_unreachable {
                store_last_error(ctx, &message);
            }
            log(ctx, &format!("Aborted: {message}"));
            tracing::warn!(error = %message, "Sync run could not reach MyMTGO.");
            return true;
        }
        Err(SyncError::Failed(message)) => {
            store_last_error(ctx, &message);
            log(ctx, &format!("Aborted: {message}"));
            tracing::error!(error = %message, "Sync run aborted due to an unexpected error.");
        }
    }
    false
}

/// Store the run's error for the Account page.
fn store_last_error(ctx: &SyncContext, message: &str) {
    let edit = SyncEdit::LastError {
        error: Some(message.to_string()),
    };
    if let Err(error) = ctx.editor.edit(edit) {
        tracing::warn!(%error, "could not store the last sync error");
    }
}

fn run_types(ctx: &SyncContext, full: bool, moved: &mut Moved) -> Result<(), SyncError> {
    if !ctx.api.session().linked() {
        return Err(SyncError::NotLinked);
    }
    if let Some(refused) = gate(RequestClass::Sync, &ctx.api.gate_now()).refusal() {
        return Err(SyncError::from(refused));
    }
    for sync_type in SYNC_TYPES {
        let held = run_type(ctx, sync_type, full, moved)?;
        // The run's first answer confirms the platforms only after its own
        // dirty list went out: what waited for it goes in a second pass.
        if held.iter().any(|platform| moved.sends(*platform)) {
            run_type(ctx, sync_type, full, moved)?;
        }
    }
    Ok(())
}

fn log(ctx: &SyncContext, line: &str) {
    ctx.api.sync.log((ctx.now)(), line);
}

/// The sync log line for a pulled row the store did not take; it is left
/// for the next run.
fn pull_failed_line(sync_type: SyncType, client_id: &str, error: &dyn std::fmt::Display) -> String {
    format!(
        "{}: {client_id} could not be imported ({error}); left for the next run.",
        sync_type.as_str()
    )
}

/// 0.x `notifyCompletion`'s message: `None` for a run that moved no match.
pub fn completion_message(pushed: u32, pulled: u32) -> Option<String> {
    let mut parts = Vec::new();
    let matches = |count: u32| if count == 1 { "match" } else { "matches" };
    if pushed > 0 {
        parts.push(format!("{pushed} {} uploaded", matches(pushed)));
    }
    if pulled > 0 {
        parts.push(format!("{pulled} {} downloaded", matches(pulled)));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// 0.x `attestKnownAccounts`: never fatal; the counts become feed lines.
fn attest(ctx: &SyncContext) {
    let counts = attest_known_accounts_with(ctx.api, (ctx.now)(), ctx.sleep);
    let plural = |count: u32| if count == 1 { "" } else { "s" };
    if counts.confirmed > 0 {
        log(
            ctx,
            &format!(
                "Confirmed {} MTGO account{}.",
                counts.confirmed,
                plural(counts.confirmed)
            ),
        );
    }
    if counts.owed > 0 {
        log(
            ctx,
            &format!(
                "Could not confirm {} MTGO account{}, retrying next run.",
                counts.owed,
                plural(counts.owed)
            ),
        );
    }
}

fn read(ctx: &SyncContext) -> Result<Connection, SyncError> {
    (ctx.reader)()
        .ok_or_else(|| SyncError::Failed("could not open the database to read".to_string()))
}

fn edit(ctx: &SyncContext, edit: SyncEdit) -> Result<u32, SyncError> {
    ctx.editor.edit(edit).map_err(SyncError::Failed)
}

/// Send a sync request through 0.x `SyncApi::send`'s 401 loop
/// (`tokens::send_with_refresh`); `passthrough` statuses are the caller's
/// to read; any other failed status fails with 0.x's message.
pub fn sync_send(
    api: &ApiState,
    request: &ApiRequest,
    passthrough: &[u16],
    now_ms: i64,
) -> Result<ApiResponse, SyncError> {
    let response = crate::api::tokens::send_with_refresh(api, request, now_ms)?;
    sync_answer(&response, passthrough)?;
    Ok(response)
}

/// A multipart boundary from the random source.
pub(crate) fn boundary() -> String {
    let hex: String = random_bytes(16)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("mymtgo{hex}")
}

// ---- One type ---------------------------------------------------------------

/// The row a candidate is built from.
#[derive(Debug, Clone)]
enum RowRef {
    Deck(DeckCandidate),
    League(i64),
    Match(i64),
}

/// A candidate of any type: its client id, whether v1 owns it (ruling 5),
/// its start (the match push order), its own time and its touch key.
#[derive(Debug, Clone)]
struct Candidate {
    client_id: String,
    owned: bool,
    started_at: i64,
    own_time: i64,
    touch_key: String,
    /// The player edited it: its first build takes the run's time.
    edited: bool,
    /// The game of its bundle: an Arena or Endstep row is sent only to a
    /// server that confirmed the platform this run (`Moved::platforms`).
    platform: BundlePlatform,
    row: RowRef,
}

/// The candidates of a type at `now_ms` (store order: client id).
fn candidates(
    conn: &Connection,
    sync_type: SyncType,
    tier: mymtgo_api::Tier,
    now_ms: i64,
) -> Result<Vec<Candidate>, SyncError> {
    let found = match sync_type {
        SyncType::Deck => store::sync::deck_candidates(conn, tier)?
            .into_iter()
            .map(|candidate| Candidate {
                client_id: candidate.client_id.clone(),
                owned: candidate.owned,
                started_at: 0,
                own_time: candidate.updated_at,
                touch_key: candidate.touch_key.clone(),
                edited: false,
                platform: candidate.platform,
                row: RowRef::Deck(candidate),
            })
            .collect(),
        SyncType::League | SyncType::Match => {
            let rows = if sync_type == SyncType::League {
                store::sync::league_candidates(conn, tier)?
            } else {
                store::sync::match_candidates(conn, tier, now_ms)?
            };
            rows.into_iter()
                .map(|candidate| Candidate {
                    client_id: candidate.client_id,
                    owned: candidate.owned,
                    started_at: candidate.started_at,
                    own_time: candidate.changed_at,
                    edited: candidate.edited,
                    touch_key: candidate.touch_key,
                    platform: candidate.platform,
                    row: if sync_type == SyncType::League {
                        RowRef::League(candidate.row_id)
                    } else {
                        RowRef::Match(candidate.row_id)
                    },
                })
                .collect()
        }
    };
    Ok(found)
}

/// The bundle parts a type carries (`synced_rows.carry`).
fn carry_pointers(sync_type: SyncType) -> &'static [&'static str] {
    match sync_type {
        SyncType::Deck => DECK_CARRY,
        SyncType::League => LEAGUE_CARRY,
        SyncType::Match => MATCH_CARRY,
    }
}

/// A row the run may send, with what its last build gave.
struct Work {
    candidate: Candidate,
    hash: String,
    changed_at: i64,
}

/// The manifest phase's findings for one type.
#[derive(Default)]
struct Plan {
    /// Every client id this device holds, and every parked pull still
    /// waiting (#830), so the server offers it again only once its copy
    /// changes (the final call's `known`). A parked pull whose hold is gone
    /// is left out, so the server offers it whenever it still holds and
    /// serves it.
    known: Vec<String>,
    dirty: Vec<DirtyEntry>,
    /// The rows v1 may push, in push order.
    work: Vec<Work>,
    /// Their carries, by client id.
    carries: HashMap<String, Carry>,
}

/// One type's manifest, push and pull. Returns the platforms of the rows it
/// held back because the server had not confirmed them.
fn run_type(
    ctx: &SyncContext,
    sync_type: SyncType,
    full: bool,
    moved: &mut Moved,
) -> Result<Vec<BundlePlatform>, SyncError> {
    let conn = read(ctx)?;
    let state = store::sync::sync_state(&conn, sync_type)?;
    let version = i64::from(CANONICAL_VERSION);
    let effective_full = full
        || state
            .as_ref()
            .is_some_and(|state| state.canonical_version != version);
    let since_ms = if effective_full {
        None
    } else {
        state.as_ref().and_then(|state| state.last_synced_at)
    };
    let since = since_ms.map(bundle_datetime);
    // The server's download cursor follows the same rule as `since`: none
    // on a full run, so the server lists everything (spec section 3.1).
    let download_cursor = if effective_full {
        None
    } else {
        state
            .as_ref()
            .and_then(|state| state.download_cursor.clone())
    };
    let cursor = if effective_full {
        state.and_then(|state| state.full_cursor)
    } else {
        None
    };
    // Before the manifest: an edit landing mid-run is still older, so the
    // next run checks it again.
    let started_at = (ctx.now)();
    let tier = store::sync::tier(&conn)?;
    let mut plan = type_plan(
        ctx,
        &conn,
        sync_type,
        tier,
        effective_full,
        cursor.as_deref(),
    )?;
    drop(conn);
    let held = hold_unconfirmed(&mut plan, moved);
    if !held.is_empty() && !moved.held_logged {
        moved.held_logged = true;
        log(
            ctx,
            "MyMTGO has not confirmed it takes MTG Arena and Endstep data: they wait.",
        );
    }

    let ManifestOutcome {
        upload,
        download,
        slots,
        tombstones,
        cursor: next_cursor,
        platforms,
    } = manifest_phase(
        ctx,
        sync_type,
        since.as_deref(),
        download_cursor.as_deref(),
        &plan.known,
        &plan.dirty,
    )?;
    ctx.api.sync.set_confirmed(platforms.as_deref());
    if platforms.is_some() {
        moved.platforms = platforms;
    }
    if let Some(slots) = slots.filter(|slots| slots.decks.is_some()) {
        edit(
            ctx,
            SyncEdit::ApplySlots {
                slots: slots.clone(),
            },
        )?;
        release_archived_slots(ctx, &slots, moved)?;
        enable_limited_slots(ctx, &slots, moved)?;
    }
    log(
        ctx,
        &format!(
            "{}: {} to upload, {} to download.",
            sync_type.as_str(),
            upload.len(),
            download.len()
        ),
    );
    push_phase(ctx, sync_type, &upload, &plan, effective_full, moved)?;
    pull_phase(ctx, sync_type, &download, moved)?;
    if sync_type == SyncType::Match && !moved.snapshots.is_empty() {
        // A pulled league comes before its matches: its registered decks
        // are placed again once they are here.
        let decks = std::mem::take(&mut moved.snapshots);
        if let Err(error) = ctx.editor.edit(SyncEdit::PulledSnapshots {
            decks,
            now_ms: (ctx.now)(),
        }) {
            tracing::warn!(%error, "Sync pull: the registered decks were not placed.");
        }
    }
    if sync_type == SyncType::Match {
        // A pulled Arena run that found no Arena account is applied again
        // now its matches (and with them the account) are here. One that
        // was only unchanged records nothing again.
        for run in std::mem::take(&mut moved.arena_runs) {
            if let Err(error) = ctx.editor.edit(run) {
                tracing::warn!(%error, "Sync pull: a pulled Arena run was not applied again.");
            }
        }
    }
    if sync_type == SyncType::Match && !download.is_empty() {
        // Pulled Arena limited matches join their course's run: the runs
        // are folded once for the whole pull, never once per match.
        if let Err(error) = ctx.editor.edit(SyncEdit::ArenaRunsAfterPull {
            now_ms: (ctx.now)(),
        }) {
            tracing::warn!(%error, "Sync pull: the Arena runs were not folded.");
        }
    }
    if sync_type == SyncType::Match && !tombstones.is_empty() {
        let hidden = edit(
            ctx,
            SyncEdit::Tombstones {
                client_ids: tombstones,
                since_ms,
                now_ms: (ctx.now)(),
            },
        )?;
        if hidden > 0 {
            log(ctx, &format!("match: {hidden} deleted on the website."));
        }
    }
    edit(
        ctx,
        SyncEdit::Finished {
            sync_type,
            last_synced_at: started_at,
            canonical_version: version,
            download_cursor: next_cursor,
        },
    )?;
    Ok(held)
}

/// Take the rows of platforms the server has not confirmed out of the
/// plan, so neither their dirty entries nor their uploads are sent; their
/// client ids stay known. Returns their platforms, once each.
fn hold_unconfirmed(plan: &mut Plan, moved: &Moved) -> Vec<BundlePlatform> {
    let mut held_ids: BTreeSet<String> = BTreeSet::new();
    let mut platforms: Vec<BundlePlatform> = Vec::new();
    for work in &plan.work {
        let platform = work.candidate.platform;
        if moved.sends(platform) {
            continue;
        }
        held_ids.insert(work.candidate.client_id.clone());
        if !platforms.contains(&platform) {
            platforms.push(platform);
        }
    }
    if held_ids.is_empty() {
        return platforms;
    }
    plan.dirty
        .retain(|entry| !held_ids.contains(&entry.client_id));
    plan.work
        .retain(|work| !held_ids.contains(&work.candidate.client_id));
    platforms
}

/// The manifest scan of a type: fetch the carry of every owned row the
/// server holds but this device has none for (a row 0.x synced), then
/// rebuild each owned row whose touch key moved (every one in full mode,
/// past the cursor), and list it as dirty when its hash is not the
/// server's. Rows are pushed oldest first (matches, by start) in
/// incremental mode, in client id order in full mode (the cursor's).
fn type_plan(
    ctx: &SyncContext,
    conn: &Connection,
    sync_type: SyncType,
    tier: mymtgo_api::Tier,
    full: bool,
    cursor: Option<&str>,
) -> Result<Plan, SyncError> {
    let candidates = candidates(conn, sync_type, tier, (ctx.now)())?;
    let mut rows = store::sync::synced_rows(conn, sync_type)?;
    let mut known: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.client_id.clone())
        .collect();
    let parked = store::sync::parked_pulls(conn, sync_type)?;
    for client_id in parked.waiting {
        if !known.contains(&client_id) {
            known.push(client_id);
        }
    }
    let mut owned: Vec<Candidate> = candidates
        .into_iter()
        .filter(|candidate| candidate.owned)
        .filter(|candidate| cursor.is_none_or(|cursor| candidate.client_id.as_str() > cursor))
        .collect();
    if !full && sync_type == SyncType::Match {
        owned.sort_by(|a, b| (a.started_at, &a.client_id).cmp(&(b.started_at, &b.client_id)));
    }

    // A type whose bundles are all rows (decks and matches since #9) has
    // nothing to carry, so nothing is fetched or waited for.
    let carries = !carry_pointers(sync_type).is_empty();
    let missing: Vec<String> = owned
        .iter()
        .filter(|candidate| {
            carries
                && rows
                    .get(&candidate.client_id)
                    .is_some_and(|row| row.hash.is_some() && row.carry.is_none())
        })
        .map(|candidate| candidate.client_id.clone())
        .collect();
    if !missing.is_empty() {
        let (fetched, withheld) = fetch_carries(ctx, sync_type, &missing)?;
        for (client_id, carry) in &fetched {
            if let Some(row) = rows.get_mut(client_id) {
                // A new carry changes the bundle: rebuild it.
                row.carry = Some(carry.clone());
                row.built_hash = None;
            }
        }
        edit(
            ctx,
            SyncEdit::Carried {
                sync_type,
                rows: fetched,
            },
        )?;
        if !withheld.is_empty() {
            edit(
                ctx,
                SyncEdit::CarryWithheld {
                    sync_type,
                    client_ids: withheld,
                    max_asks: MAX_CARRY_ASKS,
                    min_age_ms: MIN_CARRY_ASK_AGE_MS,
                    now_ms: (ctx.now)(),
                },
            )?;
        }
    }

    let now = (ctx.now)();
    let mut plan = Plan {
        known,
        ..Plan::default()
    };
    let mut built = Vec::new();
    for candidate in owned {
        let row = rows.get(&candidate.client_id);
        if carries && row.is_some_and(|row| row.hash.is_some() && row.carry.is_none()) {
            // Its carry is still to come: built without it, the bundle would
            // drop the server's parts (spec section 10).
            continue;
        }
        let carry = row.and_then(|row| stored_carry(row, sync_type));
        let (hash, changed_at) = match row {
            Some(SyncedRow {
                built_hash: Some(hash),
                touch_key: Some(key),
                changed_at,
                ..
            }) if !full && *key == candidate.touch_key => {
                (hash.clone(), changed_at.unwrap_or(candidate.own_time))
            }
            _ => {
                let Some(bundle) = build_row(conn, &candidate.row, carry.as_ref(), ctx.timezone)?
                else {
                    continue;
                };
                let hash = bundle_hash(&canonical_json(&bundle));
                // An Arena course run is every device's merge of the same
                // facts: its own newest time orders it (last write wins on
                // the server), never the clock, so a stale build made late
                // never wins over a newer one (spec section 8). Only league
                // bundles can be runs: Arena match and deck bundles carry
                // the same platform key.
                let run_time = match sync_type {
                    SyncType::League => arena_run_changed_at(&bundle),
                    _ => None,
                };
                let changed_at = match run_time {
                    Some(own) => arena_run_send_time(own, row, &hash),
                    None => match row.and_then(|row| row.built_hash.as_ref()) {
                        None => first_build_time(
                            row,
                            &candidate.touch_key,
                            candidate.own_time,
                            candidate.edited,
                            now,
                        ),
                        Some(previous) if *previous != hash => now,
                        Some(_) => row
                            .and_then(|row| row.changed_at)
                            .unwrap_or(candidate.own_time),
                    },
                };
                built.push(BuiltRow {
                    client_id: candidate.client_id.clone(),
                    built_hash: hash.clone(),
                    touch_key: candidate.touch_key.clone(),
                    changed_at,
                });
                (hash, changed_at)
            }
        };
        if full || row.and_then(|row| row.hash.as_ref()) != Some(&hash) {
            plan.dirty.push(DirtyEntry {
                client_id: candidate.client_id.clone(),
                hash: hash.clone(),
                updated_at: bundle_datetime(changed_at),
            });
        }
        if let Some(carry) = carry {
            plan.carries.insert(candidate.client_id.clone(), carry);
        }
        plan.work.push(Work {
            candidate,
            hash,
            changed_at,
        });
    }
    if !built.is_empty() {
        edit(
            ctx,
            SyncEdit::Built {
                sync_type,
                rows: built,
            },
        )?;
    }
    Ok(plan)
}

/// The `updated_at` an Arena course run is sent with: its bundle's own time
/// (`arena_run_changed_at`), and when its bytes are not the server's, at
/// least a second past the server copy's time this device knows (the
/// `changed_at` a pull or its own upload left). A merge can take an older
/// part (notes, a player edit) into a copy whose own time is the server
/// copy's, and the server keeps a copy that is not strictly older: the
/// second makes the merge win. Once both devices hold the same bytes,
/// nothing is sent, so the seconds never climb on their own.
fn arena_run_send_time(own: i64, row: Option<&SyncedRow>, hash: &str) -> i64 {
    let Some(row) = row else {
        return own;
    };
    if row.hash.as_deref() == Some(hash) {
        // Same bytes: nothing is sent, but the known server time stays.
        return row.changed_at.map_or(own, |known| known.max(own));
    }
    match row.changed_at {
        Some(known) => own.max(known + 1_000),
        None => own,
    }
}

/// `changed_at` of a row built with no earlier build hash. A row that
/// already has a `changed_at` was built before and only lost its build hash
/// to a carry: its time stands while its touch key is the one of that build,
/// and takes the clock when the row moved since (an edit, which must win
/// over the server's copy; #314 row 6). Otherwise it is a first build: a
/// pulled (or 0.x-synced) manual row, edited here, takes the clock (the
/// server's copy is at least as new as the row's own time, so only the
/// clock makes the edit win there); any other first build is never newer
/// than the cloud. So does a row the player edited (league self-management
/// spec 6.2): an imported copy's own time is older than the server's copy,
/// so only the clock makes the edit win there.
fn first_build_time(
    row: Option<&SyncedRow>,
    touch_key: &str,
    own_time: i64,
    edited: bool,
    now: i64,
) -> i64 {
    if let Some(row) = row
        && let Some(changed_at) = row.changed_at
    {
        if row.touch_key.as_deref() == Some(touch_key) {
            return changed_at;
        }
        return now;
    }
    if edited || row.is_some_and(|row| row.pulled_key.is_some()) {
        now
    } else {
        own_time
    }
}

/// A row's stored carry, without the parts its type no longer carries (a
/// carry stored before #9 gave decks and matches their rows).
fn stored_carry(row: &SyncedRow, sync_type: SyncType) -> Option<Carry> {
    let text = row.carry.as_deref()?;
    let value: Value = serde_json::from_str(text).ok()?;
    Carry::from_json(&value, carry_pointers(sync_type))
}

/// A row's bundle from its stored rows and its carry; `None` when the row
/// is gone.
fn build_row(
    conn: &Connection,
    row: &RowRef,
    carry: Option<&Carry>,
    timezone: Tz,
) -> Result<Option<Value>, SyncError> {
    match row {
        RowRef::Deck(candidate) => Ok(Some(build_deck(conn, candidate, carry)?)),
        RowRef::League(league_id) => {
            // An Arena course run has its own bundle (cross-device sync spec
            // section 8); every other league the 0.x one.
            if store::sync::is_arena_league(conn, *league_id)? {
                return Ok(store::sync::arena_run_bundle_input(conn, *league_id)?
                    .map(|run| arena_run_bundle(&run, carry)));
            }
            Ok(store::sync::league_bundle_input(conn, *league_id)?
                .map(|input| league_bundle(&input, carry)))
        }
        RowRef::Match(match_id) => match store::sync::match_bundle_input(conn, *match_id, timezone)
        {
            Ok(input) => Ok(input.map(|input| match_bundle_owned(input, carry))),
            // The database itself failed: the run stops.
            Err(store::StoreError::Sqlite(error)) => Err(store::StoreError::Sqlite(error).into()),
            // One match's stored data does not read (a corrupt pulled
            // source or Arena archive): it is left out of this run, as a
            // row that is gone, so the other rows still sync.
            Err(error) => {
                tracing::warn!(match_id, %error, "a match bundle could not be built; skipped");
                Ok(None)
            }
        },
    }
}

/// The sidecar sent beside a row's blob.
fn sidecar_of(row: &RowRef, bundle: &Value) -> Value {
    match row {
        RowRef::Deck(_) => deck_sidecar(bundle),
        RowRef::League(_) => league_sidecar(bundle),
        RowRef::Match(_) => match_sidecar(bundle),
    }
}

/// A deck's bundle from its stored rows (its versions, archetype, guides
/// and notes) and its carry.
fn build_deck(
    conn: &Connection,
    candidate: &DeckCandidate,
    carry: Option<&Carry>,
) -> Result<Value, SyncError> {
    let input = store::sync::deck_bundle_input(conn, candidate)?;
    Ok(deck_bundle(&input, carry))
}

/// What a type's manifest calls asked for.
struct ManifestOutcome {
    /// Every call's uploads.
    upload: BTreeSet<String>,
    /// The final call's downloads and slot ledger.
    download: Vec<String>,
    slots: Option<Slots>,
    /// The final call's tombstones (matches deleted on the website).
    tombstones: Vec<String>,
    /// The final call's download cursor, for the next run (`None` from an
    /// older server).
    cursor: Option<String>,
    /// The platforms the server confirmed (`None`: it named none).
    platforms: Option<Vec<String>>,
}

/// The manifest calls: the dirty entries in chunks with `known: []` and
/// `last: false`, then one final call with every known id, `dirty: []` and
/// `last: true`. Every call sends `since`, the stored download cursor and
/// this device's id. Returns the upload set (every call's), and the final
/// call's downloads, slots and cursor.
fn manifest_phase(
    ctx: &SyncContext,
    sync_type: SyncType,
    since: Option<&str>,
    cursor: Option<&str>,
    known: &[String],
    dirty: &[DirtyEntry],
) -> Result<ManifestOutcome, SyncError> {
    let device = ensure_device_id(ctx.api);
    let device = Some(device.as_str());
    let mut upload = BTreeSet::new();
    for chunk in dirty.chunks(ctx.limits.manifest_chunk.max(1)) {
        let request = manifest_request(sync_type, since, cursor, device, &[], chunk, false);
        let answer = parse_manifest(&sync_send(ctx.api, &request, &[], (ctx.now)())?)?;
        upload.extend(answer.upload);
    }
    let request = manifest_request(sync_type, since, cursor, device, known, &[], true);
    let answer = parse_manifest(&sync_send(ctx.api, &request, &[], (ctx.now)())?)?;
    upload.extend(answer.upload);
    Ok(ManifestOutcome {
        upload,
        download: answer.download,
        slots: answer.slots,
        tombstones: answer.tombstones,
        cursor: answer.cursor,
        platforms: answer.platforms,
    })
}

/// 0.x `releaseTrashedDeckSlots`, once per run: an archived deck the ledger
/// still lists as enabled gets its slot back. A failure is logged and left
/// for the next run.
fn release_archived_slots(
    ctx: &SyncContext,
    slots: &Slots,
    moved: &mut Moved,
) -> Result<(), SyncError> {
    if moved.slots_released {
        return Ok(());
    }
    moved.slots_released = true;
    let enabled: BTreeSet<&str> = slots
        .decks
        .iter()
        .flatten()
        .filter(|deck| deck.disabled_at.is_none())
        .map(|deck| deck.client_id.as_str())
        .collect();
    if enabled.is_empty() {
        return Ok(());
    }
    let archived = store::sync::archived_deck_client_ids(&read(ctx)?)?;
    for client_id in archived
        .iter()
        .filter(|client_id| enabled.contains(client_id.as_str()))
    {
        let request = deck_slot_request(client_id, false);
        let answer = sync_send(ctx.api, &request, &DECK_SLOT_PASSTHROUGH, (ctx.now)())
            .map_err(|error| format!("{error:?}"))
            .and_then(|response| parse_deck_slot(&response).map_err(|error| format!("{error:?}")));
        match answer {
            Ok(slots) if slots.decks.is_some() => {
                if let Err(error) = ctx.editor.edit(SyncEdit::ApplySlots { slots }) {
                    tracing::info!(%error, "could not store the released slot");
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::info!(%error, "Could not release the cloud sync slot of a deleted deck.");
            }
        }
    }
    Ok(())
}

/// The most limited deck slots one run asks for; the rest go next run.
const LIMITED_SLOTS_PER_RUN: usize = 50;

/// Once per run, for a Supporter (#825: limited decks go up with no
/// switch): turn on the slot of each unarchived limited deck whose slot is
/// off, so its matches and runs are taken. Only those decks are asked, at
/// most `LIMITED_SLOTS_PER_RUN` of them, so a run after the first sends
/// nothing. An Arena or Endstep deck waits until the server confirms its
/// platform (as `set_deck_sync`). The free tier never does this. No
/// connection, offline mode, signing out or a rate limit stops the requests
/// (the rest of the run still goes); any other failure is logged and the
/// deck is asked again next run.
fn enable_limited_slots(
    ctx: &SyncContext,
    slots: &Slots,
    moved: &mut Moved,
) -> Result<(), SyncError> {
    if moved.limited_slots_checked || slots.tier != mymtgo_api::Tier::Supporter {
        return Ok(());
    }
    moved.limited_slots_checked = true;
    let decks = store::sync::limited_decks_without_slot(&read(ctx)?)?;
    let mut asked = 0;
    for (client_id, platform) in decks {
        if asked == LIMITED_SLOTS_PER_RUN {
            break;
        }
        if !moved.sends(platform) {
            continue;
        }
        asked += 1;
        let request = deck_slot_request(&client_id, true);
        let response = match crate::api::tokens::send_with_refresh(ctx.api, &request, (ctx.now)()) {
            Ok(response) if response.status == 429 => {
                tracing::info!("Limited deck slots wait: MyMTGO is rate limiting.");
                break;
            }
            Ok(response) => response,
            Err(error) if stops_slot_requests(&error) => {
                tracing::info!(%error, "Limited deck slots wait for the next run.");
                break;
            }
            Err(error) => {
                tracing::info!(%error, "Could not turn on the cloud sync slot of a limited deck.");
                continue;
            }
        };
        match parse_deck_slot(&response) {
            Ok(slots) if slots.decks.is_some() => {
                if let Err(error) = ctx.editor.edit(SyncEdit::ApplySlots { slots }) {
                    tracing::info!(%error, "could not store the limited deck's slot");
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::info!(
                    ?error,
                    "Could not turn on the cloud sync slot of a limited deck."
                );
            }
        }
    }
    Ok(())
}

/// A failure after which no more slot requests go this run: no connection,
/// offline mode, signed out, or a path paused by a rate limit.
fn stops_slot_requests(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::NotLinked | ApiError::Offline | ApiError::RateLimited(_)
    ) || error.is_transient()
}

/// The push: every owned row the server asked for, unless a rejection holds
/// the same hash; too large alone becomes `too_large_local`; the rest go in
/// requests of at most 20 blobs and 8 MiB. Full mode moves the cursor past
/// each batch sent.
fn push_phase(
    ctx: &SyncContext,
    sync_type: SyncType,
    upload: &BTreeSet<String>,
    plan: &Plan,
    full: bool,
    moved: &mut Moved,
) -> Result<(), SyncError> {
    if upload.is_empty() {
        return Ok(());
    }
    let conn = read(ctx)?;
    let rejections = store::sync::rejections(&conn, sync_type)?;
    let mut batch: Vec<BlobUpload> = Vec::new();
    let mut batch_bytes = 0u64;
    let mut too_large = Vec::new();
    for work in &plan.work {
        let client_id = &work.candidate.client_id;
        if !upload.contains(client_id) {
            continue;
        }
        let Some(bundle) = build_row(
            &conn,
            &work.candidate.row,
            plan.carries.get(client_id),
            ctx.timezone,
        )?
        else {
            continue;
        };
        let canonical = canonical_json(&bundle);
        let hash = bundle_hash(&canonical);
        if hash != work.hash {
            tracing::info!(
                client_id,
                "a row changed since the manifest; sent as it is now"
            );
        }
        if rejections
            .get(client_id)
            .is_some_and(|(_, rejected)| *rejected == hash)
        {
            continue;
        }
        let gz = gzip_blob(&canonical);
        let bytes = gz.len() as u64;
        if bytes > ctx.limits.compressed_per_blob {
            too_large.push(Rejection {
                client_id: client_id.clone(),
                reason: TOO_LARGE_LOCAL.to_string(),
                hash,
            });
            continue;
        }
        if !batch.is_empty()
            && (batch.len() + 1 > ctx.limits.blobs_per_request
                || batch_bytes + bytes > ctx.limits.compressed_per_request)
        {
            flush(ctx, sync_type, &batch, full, moved)?;
            batch.clear();
            batch_bytes = 0;
        }
        batch.push(BlobUpload {
            client_id: client_id.clone(),
            hash,
            updated_at: bundle_datetime(work.changed_at),
            gz,
            sidecar: sidecar_of(&work.candidate.row, &bundle),
        });
        batch_bytes += bytes;
    }
    if !too_large.is_empty() {
        edit(
            ctx,
            SyncEdit::Rejected {
                sync_type,
                rows: too_large,
            },
        )?;
    }
    flush(ctx, sync_type, &batch, full, moved)
}

/// Send one batch and record what the server stored and refused.
fn flush(
    ctx: &SyncContext,
    sync_type: SyncType,
    batch: &[BlobUpload],
    full: bool,
    moved: &mut Moved,
) -> Result<(), SyncError> {
    if batch.is_empty() {
        return Ok(());
    }
    let device = ensure_device_id(ctx.api);
    let request = mymtgo_api::blobs_request(sync_type, Some(&device), batch, &boundary());
    let answer = parse_blobs(&sync_send(ctx.api, &request, &[], (ctx.now)())?)?;
    let hashes: HashMap<&str, &str> = batch
        .iter()
        .map(|blob| (blob.client_id.as_str(), blob.hash.as_str()))
        .collect();
    let stored: Vec<(String, String)> = answer
        .stored
        .iter()
        .filter_map(|id| {
            hashes
                .get(id.as_str())
                .map(|hash| (id.clone(), hash.to_string()))
        })
        .collect();
    let rejected: Vec<Rejection> = answer
        .rejected
        .iter()
        .filter_map(|(id, reason)| {
            hashes.get(id.as_str()).map(|hash| Rejection {
                client_id: id.clone(),
                reason: reason.clone(),
                hash: hash.to_string(),
            })
        })
        .collect();
    let rejected_text = if rejected.is_empty() {
        String::new()
    } else {
        format!(", {} rejected", rejected.len())
    };
    log(
        ctx,
        &format!(
            "{}: uploaded {}{rejected_text}.",
            sync_type.as_str(),
            answer.stored.len()
        ),
    );
    for rejection in &rejected {
        if rejection.reason == "hash_mismatch" {
            tracing::warn!(
                sync_type = sync_type.as_str(),
                client_id = rejection.client_id,
                "Sync push: blob rejected for a hash mismatch."
            );
        }
    }
    if sync_type == SyncType::Match {
        moved.pushed_matches += stored.len() as u32;
    }
    if !stored.is_empty() {
        edit(
            ctx,
            SyncEdit::Stored {
                sync_type,
                rows: stored,
                now_ms: (ctx.now)(),
            },
        )?;
    }
    if !rejected.is_empty() {
        edit(
            ctx,
            SyncEdit::Rejected {
                sync_type,
                rows: rejected,
            },
        )?;
    }
    if full && let Some(last) = batch.iter().map(|blob| blob.client_id.as_str()).max() {
        edit(
            ctx,
            SyncEdit::Cursor {
                sync_type,
                full_cursor: last.to_string(),
                canonical_version: i64::from(CANONICAL_VERSION),
            },
        )?;
    }
    Ok(())
}

/// Fetch `client_ids` in slices, looping on the ids still missing until
/// none are or a request makes no progress (the server may answer fewer
/// than asked). Every verified blob goes to `each`. Returns the ids the
/// server returned only with a blob that did not verify (a wrong hash).
fn fetch_all(
    ctx: &SyncContext,
    sync_type: SyncType,
    client_ids: &[String],
    each: &mut dyn FnMut(FetchedBlob) -> Result<(), SyncError>,
) -> Result<BTreeSet<String>, SyncError> {
    let mut unverified: BTreeSet<String> = BTreeSet::new();
    for slice in client_ids.chunks(ctx.limits.blobs_per_request.max(1)) {
        let mut remaining: Vec<String> = slice.to_vec();
        while !remaining.is_empty() {
            let request = fetch_request(sync_type, &remaining);
            let answer = parse_fetch(&sync_send(ctx.api, &request, &[], (ctx.now)())?, &remaining)?;
            let received: BTreeSet<String> = answer
                .blobs
                .iter()
                .map(|blob| blob.client_id.clone())
                .collect();
            // Asked again in this loop like a missing id; remembered in case
            // no good blob ever comes.
            for id in answer.unverified {
                if remaining.contains(&id) && !received.contains(&id) {
                    unverified.insert(id);
                }
            }
            for blob in answer.blobs {
                if remaining.contains(&blob.client_id) {
                    unverified.remove(&blob.client_id);
                    each(blob)?;
                }
            }
            let next: Vec<String> = remaining
                .iter()
                .filter(|id| !received.contains(*id))
                .cloned()
                .collect();
            if next.len() == remaining.len() {
                tracing::warn!(
                    sync_type = sync_type.as_str(),
                    remaining = remaining.len(),
                    "Sync pull: a request made no progress on the requested ids."
                );
                break;
            }
            remaining = next;
        }
    }
    Ok(unverified)
}

/// The carries fetched `(client id, carry JSON)`, and the ids withheld.
type FetchedCarries = (Vec<(String, String)>, Vec<String>);

/// The carries of rows the server holds and this device has none for, and
/// the ids the server did not give back at all (withheld: a deck switched
/// off there, a blob missing on its disk; the API says ask again later).
/// A withheld id gets no carry yet: it stays unset and is asked again at
/// the next run, up to `MAX_CARRY_ASKS` times, then settles as `{}`. An id
/// returned with a blob that does not verify or read gets no carry either:
/// it stays unset, so the next run asks again (and the row is not built or
/// pushed until then).
fn fetch_carries(
    ctx: &SyncContext,
    sync_type: SyncType,
    client_ids: &[String],
) -> Result<FetchedCarries, SyncError> {
    let mut found: HashMap<String, String> = HashMap::new();
    let mut unreadable: BTreeSet<String> = BTreeSet::new();
    let unverified = fetch_all(ctx, sync_type, client_ids, &mut |blob| {
        match serde_json::from_str::<Value>(&blob.canonical) {
            Ok(bundle) => {
                let carry = Carry::from_bundle(&bundle, carry_pointers(sync_type));
                found.insert(blob.client_id, carry.to_json().to_string());
            }
            Err(_) => {
                unreadable.insert(blob.client_id);
            }
        }
        Ok(())
    })?;
    let mut carries = Vec::new();
    let mut withheld = Vec::new();
    for id in client_ids {
        if let Some(carry) = found.remove(id) {
            carries.push((id.clone(), carry));
        } else if unverified.contains(id) || unreadable.contains(id) {
            tracing::warn!(
                sync_type = sync_type.as_str(),
                client_id = id,
                "Sync carry: the blob did not verify, asked again at the next run."
            );
        } else {
            withheld.push(id.clone());
        }
    }
    Ok((carries, withheld))
}

/// The pull: each verified blob imported through its type's import path
/// (decks, then 0.x's league and match paths) with its hash and carry. A
/// blob that does not read is skipped (logged); one the tracker cannot take
/// now is left for the next run. A pulled league's registered decks are
/// kept for after the match pull.
fn pull_phase(
    ctx: &SyncContext,
    sync_type: SyncType,
    download: &[String],
    moved: &mut Moved,
) -> Result<(), SyncError> {
    if download.is_empty() {
        return Ok(());
    }
    let total = download.len();
    let mut imported = 0usize;
    // A blob that does not verify is skipped; the next manifest offers it again.
    fetch_all(ctx, sync_type, download, &mut |blob| {
        let client_id = blob.client_id.clone();
        let mut pairs: Vec<(String, String)> = Vec::new();
        let read = match sync_type {
            SyncType::Deck => pulled_deck(blob, (ctx.now)()).map(|(edit, deck_pairs)| {
                pairs = deck_pairs;
                edit
            }),
            SyncType::League => {
                pulled_league_edit(blob, ctx.timezone, (ctx.now)()).map(|(edit, decks)| {
                    moved.snapshots.extend(decks);
                    edit
                })
            }
            SyncType::Match => pulled_match(blob, ctx.timezone, (ctx.now)()),
        };
        let Some(pulled) = read else {
            tracing::warn!(
                client_id,
                sync_type = sync_type.as_str(),
                "Sync pull: a blob did not read as a bundle of its type, skipping."
            );
            return Ok(());
        };
        let arena_run = matches!(pulled, SyncEdit::PulledArenaRun { .. }).then(|| pulled.clone());
        match ctx.editor.edit(pulled) {
            Ok(changed) => {
                if changed == 0
                    && let Some(run) = arena_run
                {
                    moved.arena_runs.push(run);
                }
                imported += 1;
                moved.pulled_any = true;
                for pair in pairs {
                    if !moved.guide_pairs.contains(&pair) {
                        moved.guide_pairs.push(pair);
                    }
                }
                log(
                    ctx,
                    &format!("{}: downloaded {imported} of {total}.", sync_type.as_str()),
                );
                if sync_type == SyncType::Match {
                    moved.pulled_matches += 1;
                }
            }
            Err(error) => {
                tracing::warn!(%error, client_id, "Sync pull: the import was not taken, skipping.");
                log(ctx, &pull_failed_line(sync_type, &client_id, &error));
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// A verified deck blob as the import's deck, with its archetype, guides
/// and notes, its hash and carry; and the `(deck MTGO id, archetype uuid)`
/// pairs of its guides and notes.
fn pulled_deck(blob: FetchedBlob, now_ms: i64) -> Option<(SyncEdit, Vec<(String, String)>)> {
    let bundle: Value = serde_json::from_str(&blob.canonical).ok()?;
    let (input, keys, carry) = read_deck_bundle(&bundle).ok()?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    let uuids = input
        .guides
        .iter()
        .map(|guide| &guide.archetype_uuid)
        .chain(input.notes.iter().map(|note| &note.archetype_uuid));
    for uuid in uuids {
        let pair = (input.mtgo_id.clone(), uuid.clone());
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    let parts = store::sync::PulledDeckParts {
        keys,
        platform: input.platform,
        endstep_cover: input.endstep_cover,
        archetype_uuid: input.archetype_uuid,
        guides: input.guides,
        notes: input.notes,
    };
    let deck = ImportedDeck {
        mtgo_id: input.mtgo_id,
        account: None,
        name: input.name,
        original_name: input.original_name,
        format: input.format.unwrap_or_default(),
        color_identity: input.color_identity,
        cover_catalog_id: input.cover_catalog_id.map(i64::from),
        archived_at_ms: None,
        created_at_ms: None,
        updated_at_ms: None,
        versions: input
            .versions
            .into_iter()
            .map(|(signature, modified_at_ms)| ImportedVersion {
                signature,
                modified_at_ms,
            })
            .collect(),
    };
    let edit = SyncEdit::PulledDeck {
        client_id: blob.client_id,
        deck: Box::new(deck),
        parts: Box::new(parts),
        hash: blob.hash,
        carry: carry.to_json().to_string(),
        now_ms,
    };
    Some((edit, pairs))
}

/// A verified league blob as the 0.x league and limited imports take it,
/// with its hash and carry, and its registered decks.
fn pulled_league_edit(
    blob: FetchedBlob,
    timezone: Tz,
    now_ms: i64,
) -> Option<(SyncEdit, Vec<LegacyLimitedDeck>)> {
    let bundle: Value = serde_json::from_str(&blob.canonical).ok()?;
    if is_arena_run_bundle(&bundle) {
        // An Arena course run is a run fold input, never a league import.
        let (run, carry) = read_arena_run_bundle(&bundle).ok()?;
        let edit = SyncEdit::PulledArenaRun {
            server_updated_at_ms: blob.updated_at.as_deref().and_then(parse_bundle_datetime),
            client_id: blob.client_id,
            run: Box::new(run),
            hash: blob.hash,
            carry: carry.to_json().to_string(),
            timezone,
            now_ms,
        };
        return Some((edit, Vec::new()));
    }
    let (input, carry) = read_league_bundle(&bundle).ok()?;
    let pulled = pulled_league(&input, &blob.client_id);
    let decks = pulled.decks.clone();
    let edit = SyncEdit::PulledLeague {
        client_id: blob.client_id,
        pulled: Box::new(pulled),
        hash: blob.hash,
        carry: carry.to_json().to_string(),
        timezone,
        now_ms,
    };
    Some((edit, decks))
}

/// A verified match blob as the 0.x match import takes it, with its hash
/// and carry.
fn pulled_match(blob: FetchedBlob, timezone: Tz, now_ms: i64) -> Option<SyncEdit> {
    let bundle: Value = serde_json::from_str(&blob.canonical).ok()?;
    let (pulled, carry) = read_match_bundle(&bundle).ok()?;
    Some(SyncEdit::PulledMatch {
        client_id: blob.client_id,
        pulled: Box::new(pulled),
        hash: blob.hash,
        carry: carry.to_json().to_string(),
        timezone,
        now_ms,
    })
}

// ---- The Account page -------------------------------------------------------

/// Per type counts (0.x `{match, deck, league}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct TypeCounts {
    #[serde(rename = "match")]
    pub matches: u32,
    #[serde(rename = "deck")]
    pub decks: u32,
    #[serde(rename = "league")]
    pub leagues: u32,
}

/// The deck slots in use (`limit` null: no limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SlotCounts {
    pub limit: Option<u32>,
    pub used: u32,
}

impl SlotCounts {
    fn of(slots: &Slots) -> SlotCounts {
        SlotCounts {
            limit: slots.limit,
            used: slots.used,
        }
    }
}

/// The Account page's sync status (0.x `SyncController::status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub linked: bool,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_synced_at_ms: Option<i64>,
    pub pending: u32,
    /// Everything a sync will send: `pending` plus the matches still in
    /// their send hold (`store::api_reads::SEND_HOLD_MS`), which the next
    /// run skips. The status bar's "queued".
    pub queued: u32,
    pub pending_by_type: TypeCounts,
    /// The current run's feed, `HH:MM:SS line`, newest last.
    pub activity: Vec<String>,
    pub syncing: bool,
    /// Archived decks: never offered again.
    pub not_synced: u32,
    pub slots: Option<SlotCounts>,
    pub rejections: u32,
    pub last_error: Option<String>,
}

/// The status from the stored rows and the run's state at `now_ms`.
pub fn sync_status_now(
    api: &ApiState,
    conn: &Connection,
    timezone: Tz,
    now_ms: i64,
) -> Result<SyncStatus, CommandError> {
    // A press waits for a run; while the gate refuses (or no one is signed
    // in) no run will start, so the press is withdrawn.
    if !api.session().linked()
        || gate(RequestClass::Sync, &api.gate_now())
            .refusal()
            .is_some()
    {
        api.sync.clear_asked();
    }
    let pending_by_type = TypeCounts {
        matches: store::sync::pending_count(conn, SyncType::Match, now_ms).map_err(storage)?,
        decks: store::sync::pending_count(conn, SyncType::Deck, now_ms).map_err(storage)?,
        leagues: store::sync::pending_count(conn, SyncType::League, now_ms).map_err(storage)?,
    };
    let activity = api
        .sync
        .activity()
        .into_iter()
        .map(|(at, line)| {
            let clock = timezone
                .timestamp_millis_opt(at)
                .single()
                .map(|time| time.format("%H:%M:%S").to_string())
                .unwrap_or_default();
            format!("{clock} {line}")
        })
        .collect();
    let pending = pending_by_type.matches + pending_by_type.decks + pending_by_type.leagues;
    // Counted as if every hold were over: a held match is queued too.
    let after_every_hold = now_ms.saturating_add(store::api_reads::SEND_HOLD_MS);
    let held_matches = store::sync::pending_count(conn, SyncType::Match, after_every_hold)
        .map_err(storage)?
        .saturating_sub(pending_by_type.matches);
    Ok(SyncStatus {
        linked: api.session().linked(),
        last_synced_at_ms: store::sync::last_synced_at(conn).map_err(storage)?,
        pending,
        queued: pending + held_matches,
        pending_by_type,
        activity,
        syncing: api.sync.syncing(),
        not_synced: store::sync::not_synced_count(conn).map_err(storage)?,
        slots: store::sync::stored_slots(conn)
            .map_err(storage)?
            .map(|slots| SlotCounts::of(&slots)),
        rejections: store::sync::rejection_count(conn).map_err(storage)?,
        last_error: store::sync::last_error(conn).map_err(storage)?,
    })
}

/// What waits in the cloud (0.x `SyncController::cloud`): the server's
/// counts minus this device's known ids, and the live slot ledger (applied,
/// as the authority). Both null when signed out or when a call fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudCounts {
    pub download: Option<TypeCounts>,
    pub slots: Option<SlotCounts>,
}

pub fn cloud_counts(
    api: &ApiState,
    editor: &SyncEditor,
    conn: &Connection,
    now_ms: i64,
) -> CloudCounts {
    if !api.session().linked() {
        return CloudCounts::default();
    }
    match read_cloud(api, editor, conn, now_ms) {
        Ok(counts) => counts,
        Err(error) => {
            tracing::info!(?error, "Sync cloud counts unavailable.");
            CloudCounts::default()
        }
    }
}

fn read_cloud(
    api: &ApiState,
    editor: &SyncEditor,
    conn: &Connection,
    now_ms: i64,
) -> Result<CloudCounts, SyncError> {
    let status = sync_send(api, &sync_status_request(), &[], now_ms)?;
    let server: Value = status.json()?;
    let slots = parse_slots(&sync_send(api, &slots_request(), &[], now_ms)?)?;
    if slots.decks.is_some() {
        editor
            .edit(SyncEdit::ApplySlots {
                slots: slots.clone(),
            })
            .map_err(SyncError::Failed)?;
    }
    let tier = store::sync::tier(conn)?;
    let waiting = |sync_type: SyncType| -> Result<u32, SyncError> {
        let held = server["counts"][sync_type.as_str()].as_u64().unwrap_or(0);
        let known = store::sync::held_ids(conn, sync_type, tier, now_ms)?.len() as u64;
        Ok(u32::try_from(held.saturating_sub(known)).unwrap_or(u32::MAX))
    };
    Ok(CloudCounts {
        download: Some(TypeCounts {
            matches: waiting(SyncType::Match)?,
            decks: waiting(SyncType::Deck)?,
            leagues: waiting(SyncType::League)?,
        }),
        slots: Some(SlotCounts::of(&slots)),
    })
}

/// Turn a deck's cloud sync on or off through the API, then mirror the
/// server's ledger (0.x `SetDeckCloudSync` and `UpdateCloudSyncController`).
/// The caller queues a sync run after enabling.
pub fn set_deck_sync(
    api: &ApiState,
    editor: &SyncEditor,
    conn: &Connection,
    deck_id: i64,
    enabled: bool,
    timezone: Tz,
    now_ms: i64,
) -> Result<SlotCounts, CommandError> {
    let refused = |message: &str| CommandError::Refused(message.to_string());
    let (mtgo_id, archived) = store::sync::deck_identity(conn, deck_id)
        .map_err(storage)?
        .ok_or_else(|| refused("That deck no longer exists."))?;
    if archived {
        return Err(refused(store::decks::ARCHIVED_DECK));
    }
    if !api.session().linked() {
        return Err(refused(NOT_LINKED_MESSAGE));
    }
    // A slot is a server resource: an Arena or Endstep deck takes one only
    // on an API that keeps its platform, so an older API's free slot is
    // never held by a deck that would never upload. Turning sync off is
    // always allowed.
    let platform = store::platform::deck_platform(conn, deck_id).map_err(storage)?;
    if enabled && platform != store::Platform::Mtgo {
        match platform_confirmed(api, platform.as_str(), now_ms) {
            Ok(true) => {}
            Ok(false) => return Err(refused(PLATFORM_NOT_CONFIRMED_MESSAGE)),
            Err(SyncError::NotLinked) => return Err(refused(NOT_LINKED_MESSAGE)),
            Err(SyncError::Held) => {
                return Err(refused(
                    "Offline mode is on. Turn it off in Data & Privacy to sync.",
                ));
            }
            Err(SyncError::Unreachable(message) | SyncError::Failed(message)) => {
                return Err(CommandError::Refused(message));
            }
        }
    }
    let request = deck_slot_request(&deck_client_id(&mtgo_id), enabled);
    let response = match sync_send(api, &request, &DECK_SLOT_PASSTHROUGH, now_ms) {
        Ok(response) => response,
        Err(SyncError::NotLinked) => return Err(refused(NOT_LINKED_MESSAGE)),
        Err(SyncError::Held) => {
            return Err(refused(
                "Offline mode is on. Turn it off in Data & Privacy to sync.",
            ));
        }
        Err(SyncError::Unreachable(message) | SyncError::Failed(message)) => {
            return Err(CommandError::Refused(message));
        }
    };
    match parse_deck_slot(&response) {
        Ok(slots) => {
            if slots.decks.is_some() {
                editor
                    .edit(SyncEdit::ApplySlots {
                        slots: slots.clone(),
                    })
                    .map_err(CommandError::Tracker)?;
            }
            Ok(SlotCounts::of(&slots))
        }
        Err(SlotError::SlotLimit { frees_at: None, .. }) => {
            match store::sync::held_deck_name(conn).map_err(storage)? {
                Some(held) => Err(CommandError::Refused(format!(
                    "Your free deck slot is in use by {held}. Turn off sync on that deck first."
                ))),
                None => Err(refused(
                    "Your free deck slot is in use. Turn off sync on another deck first.",
                )),
            }
        }
        Err(SlotError::SlotLimit {
            frees_at: Some(frees_at),
            ..
        }) => Err(CommandError::Refused(format!(
            "Your free deck slot is cooling down. It frees on {}.",
            local_day(&frees_at, timezone)
        ))),
        Err(SlotError::LimitedRequiresSupporter) => Err(refused(SUPPORTER_MESSAGE)),
        Err(SlotError::Failed(message)) => Err(CommandError::Refused(message)),
    }
}

/// Why an Arena or Endstep deck's sync cannot be turned on yet.
const PLATFORM_NOT_CONFIRMED_MESSAGE: &str =
    "MyMTGO cannot sync MTG Arena and Endstep decks yet, so this deck stays off. Try again later.";

/// Whether the server takes `platform`: the session's last manifest answer
/// says so, else one manifest call with nothing in it asks (cheap: no
/// dirty rows, no downloads listed).
fn platform_confirmed(api: &ApiState, platform: &str, now_ms: i64) -> Result<bool, SyncError> {
    if api.sync.confirms(platform) {
        return Ok(true);
    }
    let device = ensure_device_id(api);
    let request = manifest_request(SyncType::Deck, None, None, Some(&device), &[], &[], false);
    let answer = parse_manifest(&sync_send(api, &request, &[], now_ms)?)?;
    api.sync.set_confirmed(answer.platforms.as_deref());
    Ok(api.sync.confirms(platform))
}

/// The server's date as 0.x shows it (`toLocal()->format('j M Y')`); the raw
/// text when it does not read.
fn local_day(text: &str, timezone: Tz) -> String {
    let utc: Option<DateTime<Utc>> = DateTime::parse_from_rfc3339(text)
        .map(|time| time.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|time| time.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|day| day.and_hms_opt(0, 0, 0))
                .map(|time| time.and_utc())
        });
    match utc {
        Some(time) => time
            .with_timezone(&timezone)
            .format("%-d %b %Y")
            .to_string(),
        None => text.to_string(),
    }
}

// ---- The app ----------------------------------------------------------------

/// The worker's sync job: a run with the app's tracker, database and toast.
/// `report_unreachable` stores a failure to reach MyMTGO for the Account
/// page. Returns true when the run could not reach MyMTGO.
pub fn run_app_sync(app: &AppHandle, full: bool, report_unreachable: bool) -> bool {
    let api = app.state::<ApiState>();
    // Every way out below that does not run withdraws a "Sync now" press.
    let Some(startup) = app.try_state::<Startup>() else {
        api.sync.clear_asked();
        return false;
    };
    let Ok(state) = ready(&startup) else {
        api.sync.clear_asked();
        return false;
    };
    let Ok(editor) = state.tracker_part(tracker::TrackerHandle::sync_editor) else {
        tracing::debug!("tracking is restarting; the sync waits for the next run");
        api.sync.clear_asked();
        api.sync.log(
            crate::wall_clock_ms(),
            "Tracking is restarting; press Sync now again in a moment.",
        );
        return false;
    };
    let db_path = state.db_path.clone();
    let reader = move || store::open_reader(&db_path).ok();
    let notify = |title: &str, message: &str| {
        let toast = AppNotification {
            kind: "sync".to_string(),
            title: title.to_string(),
            message: message.to_string(),
            route: Some("/settings/account".to_string()),
        };
        if let Err(error) = toast.emit(app) {
            tracing::warn!(%error, "could not raise the sync toast");
        }
    };
    let sleep = |ms: u64| std::thread::sleep(std::time::Duration::from_millis(ms));
    let ctx = SyncContext {
        api: &api,
        editor: &editor,
        reader: &reader,
        now: &crate::wall_clock_ms,
        notify: &notify,
        sleep: &sleep,
        limits: Limits::default(),
        timezone: state.timezone,
        pulled: &|| crate::cards::worker::signal(app, CardsSignal::SyncPulled),
        guides_pulled: &|pairs| announce_pulled_guides(app, &state.db_path, pairs),
        report_unreachable,
    };
    let end = run_sync(&ctx, full);
    // The status bar's "Last sync" line reads again after every run.
    if let Err(error) = (SyncFinished {}).emit(app) {
        tracing::warn!(%error, "could not announce the finished sync");
    }
    end == RunEnd::Unreachable
}

/// Push `GuidesChanged` for each pulled `(deck MTGO id, archetype uuid)`
/// pair this device holds.
fn announce_pulled_guides(app: &AppHandle, db_path: &std::path::Path, pairs: &[(String, String)]) {
    let reader = match store::open_reader(db_path) {
        Ok(reader) => reader,
        Err(error) => {
            tracing::warn!(%error, "could not read the pulled guides to announce them");
            return;
        }
    };
    for changed in pulled_guides_changed(&reader, pairs) {
        crate::guides::announce(app, changed);
    }
}

/// The `GuidesChanged` for each pulled `(deck MTGO id, archetype uuid)`
/// pair whose deck and archetype this device holds; other pairs are left
/// out.
fn pulled_guides_changed(
    reader: &store::Connection,
    pairs: &[(String, String)],
) -> Vec<crate::events::GuidesChanged> {
    let mut changed = Vec::new();
    for (mtgo_id, uuid) in pairs {
        let deck = store::decks::deck_by_mtgo_id(reader, mtgo_id)
            .ok()
            .flatten();
        let archetype = store::archetypes::held_uuid(reader, uuid).ok().flatten();
        if let (Some(deck), Some(archetype)) = (deck, archetype) {
            changed.push(crate::events::GuidesChanged {
                deck_id: deck.id,
                archetype_id: archetype.id,
            });
        }
    }
    changed
}

/// The Account page's sync status.
#[tauri::command]
#[specta::specta]
pub async fn sync_status(app: AppHandle) -> Result<SyncStatus, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        sync_status_now(
            &app.state::<ApiState>(),
            &lock(&state.reader),
            state.timezone,
            crate::wall_clock_ms(),
        )
    })
    .await
}

/// "Sync now": the worker runs it at its next tick (a run already going
/// takes it as its follow-up).
#[tauri::command]
#[specta::specta]
pub async fn sync_now(app: AppHandle) -> Result<SyncStatus, CommandError> {
    app.state::<ApiState>().sync.ask(crate::wall_clock_ms());
    signal(&app, WorkerSignal::SyncNow);
    sync_status(app).await
}

/// What waits in the cloud; asked on the page's mount, refresh and after a
/// run, never on its poll.
#[tauri::command]
#[specta::specta]
pub async fn sync_cloud_counts(app: AppHandle) -> Result<CloudCounts, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let editor = state.tracker_part(tracker::TrackerHandle::sync_editor)?;
        let conn = store::open_reader(&state.db_path).map_err(storage)?;
        Ok(cloud_counts(
            &app.state::<ApiState>(),
            &editor,
            &conn,
            crate::wall_clock_ms(),
        ))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::secrets::{ACCESS_TOKEN, REFRESH_TOKEN};
    use crate::api::session::testing::{TestApi, test_api};
    use crate::api::stub::{Recorded, Reply, Stub};
    use crate::bridge::lock;
    use crate::commands::AppState;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;
    use mymtgo_api::KnownAccount;
    use serde_json::{Value, json};
    use std::io::Read;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};
    use tracker::TrackerEvent;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 86_400_000;
    const MANIFEST: &str = "/api/sync/manifest";
    const BLOBS: &str = "/api/sync/blobs";
    const FETCH: &str = "/api/sync/blobs/fetch";

    fn linked(base_url: &str) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let token_valid = (NOW + 30 * DAY).to_string();
        let rows = [
            ("api_device_id", r#""qdevice""#),
            ("api_key_expires_at", key_valid.as_str()),
            ("api_token_expires_at", token_valid.as_str()),
        ];
        test_api(
            base_url,
            &rows,
            &[
                ("api_key", "qdevice-key"),
                (ACCESS_TOKEN, "qaccess"),
                (REFRESH_TOKEN, "qrefresh"),
            ],
        )
    }

    struct Device {
        _dir: TestDir,
        state: AppState,
        _events: Receiver<TrackerEvent>,
    }

    fn device(label: &str) -> Device {
        let dir = TestDir::new(label);
        let (state, events) = ready_state(&dir);
        Device {
            _dir: dir,
            state,
            _events: events,
        }
    }

    fn write(device: &Device, sql: &str) {
        store::open_writer(&device.state.db_path)
            .expect("writer")
            .execute_batch(sql)
            .expect("sql");
    }

    fn reader(device: &Device) -> store::Connection {
        store::open_reader(&device.state.db_path).expect("reader")
    }

    /// An Arena run (an orphan, or a course run with no record yet) builds
    /// no bundle at all: never the 0.x league bundle.
    #[test]
    fn an_arena_run_without_its_bundle_builds_nothing() {
        let device = device("sync-arena-run-no-bundle");
        write(
            &device,
            "INSERT INTO leagues (id, run_key, source, kind, state, token, name, started_at,
                 created_at)
             VALUES (71, 'arena:q-course', 'log', 'draft', 'active', 'arena:q-course',
                     'Qdraft', 1, 1),
                    (72, 'arena:QuickDraft_FDN_20261101:q-m1', 'log', 'draft', 'active',
                     'arena:QuickDraft_FDN_20261101:q-m1', 'Qorphan', 1, 1);",
        );
        let conn = reader(&device);
        for league_id in [71, 72] {
            let built =
                build_row(&conn, &RowRef::League(league_id), None, chrono_tz::UTC).expect("build");
            assert_eq!(built, None, "{league_id}");
        }
    }

    /// A deck read from its file, with one version, a day old.
    fn file_deck(device: &Device, mtgo_id: &str, enabled: bool) -> i64 {
        let conn = store::open_writer(&device.state.db_path).expect("writer");
        let id: i64 = conn
            .query_row(
                "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
                     cloud_sync_enabled)
                 VALUES (?1, 'Qdeck ' || ?1, 'CMODERN', ?2, ?2, 'file', ?3) RETURNING id",
                store::params![mtgo_id, NOW - DAY, enabled],
                |row| row.get(0),
            )
            .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)",
            store::params![id, format!("Qsig{mtgo_id}"), NOW - DAY],
        )
        .expect("version");
        id
    }

    fn editor(device: &Device) -> SyncEditor {
        lock(&device.state.tracker)
            .as_ref()
            .expect("tracker")
            .sync_editor()
    }

    #[test]
    fn pulled_guide_pairs_resolve_to_the_decks_and_archetypes_held() {
        let device = device("sync-pulled-guides-changed");
        let deck_id = file_deck(&device, "7001", false);
        write(
            &device,
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('bbbbbbbb-2222-4222-8222-222222222222', 'Qheld', 'CMODERN',
                     'downloaded', 0, 0);",
        );
        let conn = reader(&device);
        let archetype_id: i64 = conn
            .query_row(
                "SELECT id FROM archetypes WHERE uuid = 'bbbbbbbb-2222-4222-8222-222222222222'",
                [],
                |row| row.get(0),
            )
            .expect("archetype");
        let pair = |deck: &str, uuid: &str| (deck.to_string(), uuid.to_string());
        let held = "bbbbbbbb-2222-4222-8222-222222222222";
        let pairs = [
            pair("7001", held),
            pair("9999", held),
            pair("7001", "cccccccc-3333-4333-8333-333333333333"),
        ];
        let changed = pulled_guides_changed(&conn, &pairs);
        assert_eq!(changed.len(), 1, "only the pair held on both sides");
        assert_eq!(changed[0].deck_id, deck_id);
        assert_eq!(changed[0].archetype_id, archetype_id);
        assert!(pulled_guides_changed(&conn, &[]).is_empty());
    }

    type Toasts = Vec<(String, String)>;

    fn run_with(api: &TestApi, device: &Device, full: bool, limits: Limits) -> Toasts {
        let editor = editor(device);
        let db = device.state.db_path.clone();
        let reader = move || store::open_reader(&db).ok();
        let toasts: Mutex<Toasts> = Mutex::new(Vec::new());
        let pulls: Mutex<u32> = Mutex::new(0);
        let guide_pairs: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        let notify = |title: &str, message: &str| {
            lock(&toasts).push((title.to_string(), message.to_string()));
        };
        let ctx = SyncContext {
            api: &api.state,
            editor: &editor,
            reader: &reader,
            now: &|| NOW,
            notify: &notify,
            sleep: &|_| {},
            limits,
            timezone: Tz::UTC,
            pulled: &|| *lock(&pulls) += 1,
            guides_pulled: &|pairs| lock(&guide_pairs).extend(pairs.iter().cloned()),
            report_unreachable: false,
        };
        run_sync(&ctx, full);
        PULLS.with(|count| count.set(*lock(&pulls)));
        GUIDE_PAIRS.with(|seen| *seen.borrow_mut() = lock(&guide_pairs).clone());
        toasts.into_inner().expect("toasts")
    }

    thread_local! {
        /// How many times the last `run_with` on this thread called `pulled`.
        static PULLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        /// The guide pairs the last `run_with` on this thread announced.
        static GUIDE_PAIRS: std::cell::RefCell<Vec<(String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn last_pulls() -> u32 {
        PULLS.with(std::cell::Cell::get)
    }

    fn last_guide_pairs() -> Vec<(String, String)> {
        GUIDE_PAIRS.with(|seen| seen.borrow().clone())
    }

    /// #300 F22: a pull that brought guides or notes announces their pairs
    /// once, at the end of the run; one without announces nothing.
    #[test]
    fn a_pulled_deck_announces_its_guides_and_notes() {
        let stub = pulling("deck", "777", deck_bundle_of("777", None));
        let first = device("sync-run-pull-guides");
        let api = linked(&stub.base_url);
        run(&api, &first);
        assert_eq!(
            last_guide_pairs(),
            [("777".to_string(), "q-foe".to_string())]
        );
        let mut bare = deck_bundle_of("778", None);
        bare["guides"] = json!([]);
        bare["notes"] = json!([]);
        let stub = pulling("deck", "778", bare);
        let other = device("sync-run-pull-no-guides");
        let api = linked(&stub.base_url);
        run(&api, &other);
        assert!(last_guide_pairs().is_empty());
    }

    fn run(api: &TestApi, device: &Device) -> Toasts {
        run_with(api, device, false, Limits::default())
    }

    fn run_full(api: &TestApi, device: &Device) -> Toasts {
        run_with(api, device, true, Limits::default())
    }

    fn body(request: &Recorded) -> Value {
        serde_json::from_slice(&request.body).unwrap_or(Value::Null)
    }

    fn empty_manifest() -> Reply {
        Reply::json(200, r#"{"upload":[],"download":[],"tombstones":[]}"#)
    }

    /// The server with nothing to say: empty manifests, nothing stored or
    /// fetched, every other call a 200.
    fn quiet(request: &Recorded) -> Reply {
        match request.target.as_str() {
            MANIFEST => empty_manifest(),
            BLOBS => Reply::json(200, r#"{"stored":[],"rejected":[]}"#),
            FETCH => Reply::json(200, r#"{"blobs":[]}"#),
            _ => Reply::json(200, "{}"),
        }
    }

    /// A manifest answer for `sync_type`'s final call; the other calls are
    /// quiet.
    fn answering(
        sync_type: &'static str,
        answer: Value,
    ) -> impl Fn(&Recorded) -> Option<Reply> + Send + Sync + 'static {
        move |request: &Recorded| {
            let found = body(request);
            (request.target == MANIFEST && found["type"] == sync_type && found["last"] == true)
                .then(|| Reply::json(200, &answer.to_string()))
        }
    }

    fn manifests(stub: &Stub, sync_type: &str) -> Vec<Value> {
        stub.requests()
            .iter()
            .filter(|request| request.target == MANIFEST)
            .map(body)
            .filter(|found| found["type"] == sync_type)
            .collect()
    }

    fn dirty_ids(stub: &Stub, sync_type: &str) -> Vec<String> {
        manifests(stub, sync_type)
            .iter()
            .flat_map(|found| found["dirty"].as_array().cloned().unwrap_or_default())
            .map(|entry| entry["client_id"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    // Spec 6.4: a match deleted on the website is hidden here, without an
    // edit, and no longer offered as dirty.
    #[test]
    fn a_tombstoned_match_is_hidden_and_left_out_of_dirty() {
        let manifest = answering(
            "match",
            json!({"upload": [], "download": [], "tombstones": ["q-match"]}),
        );
        let stub = Stub::start(move |request| manifest(request).unwrap_or_else(|| quiet(request)));
        let device = device("sync-run-tombstone");
        file_deck(&device, "111", true);
        log_match(&device, "q-match", NOW - DAY, "111");
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(dirty_ids(&stub, "match"), vec!["q-match".to_string()]);
        let columns: (Option<i64>, Option<i64>) = reader(&device)
            .query_row(
                "SELECT hidden_at, edited_at FROM matches WHERE token = 'q-match'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("match");
        assert_eq!(columns, (Some(NOW), None));
        let before = stub.requests().len();
        run(&api, &device);
        let dirty: Vec<Value> = stub.requests()[before..]
            .iter()
            .filter(|request| request.target == MANIFEST)
            .map(body)
            .filter(|found| found["type"] == "match")
            .flat_map(|found| found["dirty"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(dirty.is_empty(), "a tombstoned match is not offered again");
    }

    fn targets(stub: &Stub) -> Vec<String> {
        stub.requests()
            .into_iter()
            .map(|request| format!("{} {}", request.method, request.target))
            .collect()
    }

    /// The blobs a multipart upload carried: client id to the unzipped
    /// bundle.
    fn uploaded(stub: &Stub) -> Vec<(String, Value)> {
        let mut found = Vec::new();
        for request in stub.requests() {
            if request.target != BLOBS {
                continue;
            }
            let boundary = request
                .header("content-type")
                .and_then(|value| value.split("boundary=").nth(1))
                .expect("boundary")
                .to_string();
            let end = format!("\r\n--{boundary}");
            let bytes = &request.body;
            let marker = b"Content-Type: application/gzip\r\n\r\n";
            let mut at = 0;
            while let Some(offset) = find(&bytes[at..], marker) {
                let start = at + offset + marker.len();
                let stop = start + find(&bytes[start..], end.as_bytes()).expect("part end");
                let head = String::from_utf8_lossy(&bytes[at..start]).to_string();
                let name_at = head.rfind("filename=\"").expect("filename") + 10;
                let client_id = head[name_at..]
                    .split(".json.gz")
                    .next()
                    .expect("name")
                    .to_string();
                let mut text = String::new();
                flate2::read::GzDecoder::new(&bytes[start..stop])
                    .read_to_string(&mut text)
                    .expect("gzip");
                found.push((client_id, serde_json::from_str(&text).expect("bundle")));
                at = stop;
            }
        }
        found
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A fetch answer's blob for this bundle (or a wrong hash).
    fn blob(client_id: &str, bundle: &Value, good_hash: bool) -> Value {
        let canonical = canonical_json(bundle);
        let hash = if good_hash {
            bundle_hash(&canonical)
        } else {
            "0".repeat(64)
        };
        json!({
            "client_id": client_id,
            "hash": hash,
            "data": base64(&gzip_blob(&canonical)),
        })
    }

    fn deck_bundle_of(mtgo_id: &str, cover: Option<&str>) -> Value {
        json!({
            "deck": {
                "mtgo_id": mtgo_id,
                "name": "Qcloud deck",
                "original_name": null,
                "format": "CMODERN",
                "color_identity": "UR",
                "cover_mtgo_id": cover,
                "archetype_uuid": "q-archetype",
            },
            "versions": [{"signature": "QsigCloud", "modified_at": "2027-01-15T08:00:00Z"}],
            "guides": [{"archetype_uuid": "q-foe", "cards": [
                {"oracle_id": "q-oracle", "direction": "in", "quantity": 2}
            ]}],
            "notes": [{"archetype_uuid": "q-foe", "body": "Qnote",
                "created_at": "2027-01-15T08:00:00Z"}],
        })
    }

    fn synced(device: &Device, client_id: &str) -> Option<store::sync::SyncedRow> {
        store::sync::synced_rows(&reader(device), SyncType::Deck)
            .expect("rows")
            .remove(client_id)
    }

    fn last_error(device: &Device) -> Option<String> {
        store::sync::last_error(&reader(device)).expect("error")
    }

    fn deck_state(device: &Device) -> Option<store::sync::SyncStateRow> {
        store::sync::sync_state(&reader(device), SyncType::Deck).expect("state")
    }

    fn activity(api: &TestApi) -> String {
        api.state
            .sync
            .activity()
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn flag(device: &Device, deck_id: i64) -> bool {
        reader(device)
            .query_row(
                "SELECT cloud_sync_enabled FROM decks WHERE id = ?1",
                [deck_id],
                |row| row.get(0),
            )
            .expect("flag")
    }

    // ---- SyncRunnerTest ---------------------------------------------------

    #[test]
    fn chunks_the_manifest_with_a_separate_always_final_known_and_last_call() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-chunks");
        file_deck(&device, "111", false);
        file_deck(&device, "222", false);
        let api = linked(&stub.base_url);
        let limits = Limits {
            manifest_chunk: 1,
            ..Limits::default()
        };
        run_with(&api, &device, false, limits);
        let calls = manifests(&stub, "deck");
        assert_eq!(calls.len(), 3);
        for call in &calls[..2] {
            assert_eq!(call["known"], json!([]));
            assert_eq!(call["last"], json!(false));
            assert_eq!(call["dirty"].as_array().expect("dirty").len(), 1);
        }
        assert_eq!(calls[2]["dirty"], json!([]));
        assert_eq!(calls[2]["last"], json!(true));
        assert_eq!(calls[2]["known"], json!(["111", "222"]));
    }

    #[test]
    fn sends_each_dirty_entry_as_an_object_matching_the_server_contract() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-entry");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let entry = manifests(&stub, "deck")[0]["dirty"][0].clone();
        let keys: Vec<&String> = entry.as_object().expect("entry").keys().collect();
        assert_eq!(keys, vec!["client_id", "hash", "updated_at"]);
        let hash = entry["hash"].as_str().expect("hash");
        assert!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(entry["updated_at"], json!(bundle_datetime(NOW - DAY)));
    }

    #[test]
    fn fetches_a_download_id_hash_verifies_it_and_imports_it() {
        let bundle = deck_bundle_of("777", Some("424242"));
        let answer = blob("777", &bundle, true);
        let manifest = answering("deck", json!({"upload": [], "download": ["777"]}));
        let stub = Stub::start(move |request| {
            if request.target == FETCH {
                return Reply::json(200, &json!({"blobs": [answer.clone()]}).to_string());
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-pull");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let fetch = stub
            .requests()
            .into_iter()
            .find(|request| request.target == FETCH)
            .expect("fetched");
        assert_eq!(
            body(&fetch),
            json!({"type": "deck", "client_ids": ["777"], "platforms": ["arena", "endstep"]})
        );
        let (name, cover): (String, Option<i64>) = reader(&device)
            .query_row(
                "SELECT name, cover_catalog_id FROM decks WHERE mtgo_id = '777'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("imported");
        assert_eq!(name, "Qcloud deck");
        assert_eq!(cover, Some(424_242));
        let row = synced(&device, "777").expect("row");
        assert_eq!(row.hash, Some(bundle_hash(&canonical_json(&bundle))));
        assert_eq!(last_error(&device), None);
    }

    #[test]
    fn parks_a_rejected_blob_skips_it_unchanged_and_retries_it_once_its_hash_changes() {
        let store_it = Arc::new(AtomicUsize::new(0));
        let flag_store = Arc::clone(&store_it);
        let manifest = answering("deck", json!({"upload": ["111"], "download": []}));
        let stub = Stub::start(move |request| {
            if request.target == BLOBS {
                let answer = if flag_store.load(Ordering::SeqCst) == 1 {
                    r#"{"stored":["111"],"rejected":[]}"#
                } else {
                    r#"{"stored":[],"rejected":[{"client_id":"111","reason":"hash_mismatch"}]}"#
                };
                return Reply::json(200, answer);
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-rejected");
        let id = file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let rejections =
            store::sync::rejections(&reader(&device), SyncType::Deck).expect("rejections");
        let (reason, first_hash) = rejections["111"].clone();
        assert_eq!(reason, "hash_mismatch");
        assert_eq!(synced(&device, "111").and_then(|row| row.hash), None);

        run(&api, &device);
        let uploads = || {
            stub.requests()
                .iter()
                .filter(|request| request.target == BLOBS)
                .count()
        };
        assert_eq!(uploads(), 1, "unchanged: not sent again");

        write(
            &device,
            &format!(
                "UPDATE decks SET name = 'Qrenamed', updated_at = updated_at + 1 WHERE id = {id}"
            ),
        );
        store_it.store(1, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(uploads(), 2, "a new hash is retried");
        let row = synced(&device, "111").expect("row");
        assert!(row.hash.is_some_and(|hash| hash != first_hash));
        assert!(
            store::sync::rejections(&reader(&device), SyncType::Deck)
                .expect("rejections")
                .is_empty()
        );
    }

    #[test]
    fn runs_the_deck_league_then_match_manifest_calls_in_that_fixed_order() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-order");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let types: Vec<Value> = stub
            .requests()
            .iter()
            .filter(|request| request.target == MANIFEST)
            .map(|request| body(request)["type"].clone())
            .collect();
        assert_eq!(types, vec![json!("deck"), json!("league"), json!("match")]);
    }

    #[test]
    fn gives_every_type_a_manifest_call_even_when_only_one_type_has_work() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-every-type");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        for sync_type in ["deck", "league", "match"] {
            assert!(!manifests(&stub, sync_type).is_empty(), "{sync_type}");
        }
    }

    #[test]
    fn continues_to_the_next_download_slice_after_one_slice_makes_no_progress() {
        let bundle = deck_bundle_of("777", None);
        let answer = blob("777", &bundle, true);
        let manifest = answering(
            "deck",
            json!({"upload": [], "download": ["q-ghost", "777"]}),
        );
        let stub = Stub::start(move |request| {
            if request.target == FETCH {
                let asked = body(request)["client_ids"].clone();
                let blobs = if asked == json!(["777"]) {
                    json!([answer.clone()])
                } else {
                    json!([])
                };
                return Reply::json(200, &json!({ "blobs": blobs }).to_string());
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-slices");
        let api = linked(&stub.base_url);
        let limits = Limits {
            blobs_per_request: 1,
            ..Limits::default()
        };
        run_with(&api, &device, false, limits);
        assert!(
            synced(&device, "777").is_some(),
            "the second slice was tried"
        );
    }

    #[test]
    fn records_the_aborting_error_for_the_settings_card_and_clears_it_on_the_next_clean_run() {
        let failing = Arc::new(AtomicUsize::new(1));
        let fail = Arc::clone(&failing);
        let stub = Stub::start(move |request| {
            if fail.load(Ordering::SeqCst) == 1 && request.target.starts_with("/api/sync/") {
                return Reply::json(
                    422,
                    r#"{"message":"The dirty.7.client_id field format is invalid."}"#,
                );
            }
            quiet(request)
        });
        let device = device("sync-run-error");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let error = last_error(&device).expect("recorded");
        assert!(
            error.contains("422") && error.contains("dirty.7.client_id"),
            "{error}"
        );
        assert!(activity(&api).contains("Aborted: "));
        failing.store(0, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(last_error(&device), None);
    }

    // #307 F2: a 401 whose refresh fails (the token endpoint answers 500,
    // or cannot be reached) ends the run with that error, still signed in.
    #[test]
    fn a_401_whose_refresh_fails_ends_the_run_with_the_refresh_error() {
        let stub = Stub::start(|request| {
            if request.target == "/oauth/token" {
                return Reply::json(500, r#"{"message":"Server Error"}"#);
            }
            if request.target.starts_with("/api/sync/") {
                return Reply::json(401, r#"{"message":"Unauthenticated."}"#);
            }
            quiet(request)
        });
        let device = device("sync-run-refresh-500");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let error = last_error(&device).expect("recorded");
        assert!(error.contains("500"), "{error}");
        assert!(activity(&api).contains("Aborted: "));
        assert!(api.state.session().linked(), "credentials kept");
        let refreshes = stub
            .requests()
            .iter()
            .filter(|request| request.target == "/oauth/token")
            .count();
        assert_eq!(refreshes, 1, "the run stops at the first failed refresh");
    }

    #[test]
    fn sends_fetch_client_ids_as_strings_even_when_the_manifest_returned_a_number() {
        let manifest = answering("deck", json!({"upload": [], "download": [108_886_838]}));
        let stub = Stub::start(move |request| manifest(request).unwrap_or_else(|| quiet(request)));
        let device = device("sync-run-numeric");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let fetch = stub
            .requests()
            .into_iter()
            .find(|request| request.target == FETCH)
            .expect("fetched");
        assert_eq!(body(&fetch)["client_ids"], json!(["108886838"]));
    }

    #[test]
    fn writes_a_fresh_activity_feed_for_each_run_ending_in_sync_complete() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-activity");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let lines = activity(&api);
        assert!(lines.contains("Sync started."), "{lines}");
        assert!(
            lines.contains("deck: 0 to upload, 0 to download."),
            "{lines}"
        );
        assert!(lines.ends_with("Sync complete."), "{lines}");
        run(&api, &device);
        assert_eq!(activity(&api).matches("Sync started.").count(), 1);
        assert!(!api.state.sync.syncing());
    }

    #[test]
    fn raises_one_toast_for_a_run_that_moved_matches_and_stays_quiet_otherwise() {
        assert_eq!(
            completion_message(1, 0).as_deref(),
            Some("1 match uploaded")
        );
        assert_eq!(
            completion_message(2, 1).as_deref(),
            Some("2 matches uploaded, 1 match downloaded")
        );
        assert_eq!(
            completion_message(0, 3).as_deref(),
            Some("3 matches downloaded")
        );
        assert_eq!(completion_message(0, 0), None);
        let stub = Stub::start(quiet);
        let device = device("sync-run-toast");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        assert!(run(&api, &device).is_empty(), "decks alone never toast");
    }

    #[test]
    fn applies_the_manifest_slots_to_local_deck_flags() {
        let slots = json!({"limit": 1, "used": 1, "decks": [
            {"client_id": "111", "enabled_at": "x", "disabled_at": null, "frees_at": null}
        ]});
        let manifest = answering(
            "deck",
            json!({"upload": [], "download": [], "slots": slots}),
        );
        let stub = Stub::start(move |request| manifest(request).unwrap_or_else(|| quiet(request)));
        let device = device("sync-run-slots");
        let id = file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(flag(&device, id));
        let stored = store::sync::stored_slots(&reader(&device))
            .expect("slots")
            .expect("stored");
        assert_eq!(stored.used, 1);
    }

    #[test]
    fn ignores_a_slots_payload_with_no_decks_key() {
        let manifest = answering(
            "deck",
            json!({"upload": [], "download": [], "slots": {"limit": 1, "used": 1}}),
        );
        let stub = Stub::start(move |request| manifest(request).unwrap_or_else(|| quiet(request)));
        let device = device("sync-run-slots-malformed");
        let id = file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(flag(&device, id));
        assert_eq!(
            store::sync::stored_slots(&reader(&device)).expect("slots"),
            None
        );
    }

    #[test]
    fn applies_an_empty_decks_ledger_which_turns_every_deck_off() {
        let manifest = answering(
            "deck",
            json!({"upload": [], "download": [], "slots": {"limit": 1, "used": 0, "decks": []}}),
        );
        let stub = Stub::start(move |request| manifest(request).unwrap_or_else(|| quiet(request)));
        let device = device("sync-run-slots-empty");
        let id = file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(!flag(&device, id));
    }

    fn listed_enabled(client_ids: &[&str]) -> Value {
        let decks: Vec<Value> = client_ids
            .iter()
            .map(|id| json!({"client_id": id, "enabled_at": "x", "disabled_at": null, "frees_at": null}))
            .collect();
        json!({"upload": [], "download": [], "slots": {"limit": 2, "used": 2, "decks": decks}})
    }

    fn every_manifest(answer: Value) -> impl Fn(&Recorded) -> Option<Reply> + Send + Sync {
        move |request: &Recorded| {
            (request.target == MANIFEST && body(request)["last"] == true)
                .then(|| Reply::json(200, &answer.to_string()))
        }
    }

    #[test]
    fn hands_back_the_slot_of_an_archived_deck_the_server_still_lists_once_per_run() {
        let manifest = every_manifest(listed_enabled(&["111"]));
        let stub = Stub::start(move |request| {
            if request.method == "PUT" {
                return Reply::json(200, r#"{"limit":1,"used":0,"decks":[]}"#);
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-release");
        let id = file_deck(&device, "111", true);
        write(
            &device,
            &format!("UPDATE decks SET archived_at = 1 WHERE id = {id}"),
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        let puts: Vec<Recorded> = stub
            .requests()
            .into_iter()
            .filter(|request| request.method == "PUT")
            .collect();
        assert_eq!(
            puts.len(),
            1,
            "once per run, though every manifest lists it"
        );
        assert_eq!(puts[0].target, "/api/sync/decks/111");
        assert_eq!(body(&puts[0])["enabled"], json!(false));
        assert!(!flag(&device, id));
        let stored = store::sync::stored_slots(&reader(&device))
            .expect("slots")
            .expect("stored");
        assert_eq!(
            stored.used, 2,
            "the league manifest's ledger is the last one applied"
        );
    }

    #[test]
    fn releases_an_archived_limited_decks_slot_by_its_client_ids_kind() {
        let manifest = every_manifest(listed_enabled(&["limited_abc"]));
        let stub = Stub::start(move |request| {
            if request.method == "PUT" {
                return Reply::json(200, r#"{"limit":null,"used":0,"decks":[]}"#);
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-release-limited");
        let id = file_deck(&device, "limited:abc", true);
        write(
            &device,
            &format!("UPDATE decks SET archived_at = 1 WHERE id = {id}"),
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        let put = stub
            .requests()
            .into_iter()
            .find(|request| request.method == "PUT")
            .expect("released");
        assert_eq!(put.target, "/api/sync/decks/limited_abc");
        assert_eq!(body(&put), json!({"enabled": false, "kind": "limited"}));
        assert!(!flag(&device, id));
    }

    /// A ledger of the given tier listing `client_ids` as enabled.
    fn tier_ledger(tier: &str, client_ids: &[&str]) -> Value {
        let decks: Vec<Value> = client_ids
            .iter()
            .map(|id| json!({"client_id": id, "enabled_at": "x", "disabled_at": null, "frees_at": null}))
            .collect();
        json!({"tier": tier, "limit": null, "used": client_ids.len(), "decks": decks})
    }

    fn puts(stub: &Stub) -> Vec<Recorded> {
        stub.requests()
            .into_iter()
            .filter(|request| request.method == "PUT")
            .collect()
    }

    /// A server whose ledger lists `before` until a PUT, then `after` (its
    /// answer to the PUT too); `platforms` confirms Arena and Endstep.
    fn slot_server(
        tier: &'static str,
        before: &'static [&'static str],
        after: &'static [&'static str],
        platforms: bool,
    ) -> Stub {
        let turned = Arc::new(AtomicUsize::new(0));
        Stub::start(move |request| {
            if request.method == "PUT" {
                turned.fetch_add(1, Ordering::SeqCst);
                return Reply::json(200, &tier_ledger(tier, after).to_string());
            }
            if request.target == MANIFEST && body(request)["last"] == true {
                let listed = if turned.load(Ordering::SeqCst) > 0 {
                    after
                } else {
                    before
                };
                let mut answer = json!({
                    "upload": [], "download": [], "slots": tier_ledger(tier, listed),
                });
                if platforms {
                    answer["platforms"] = json!(["arena", "endstep"]);
                }
                return Reply::json(200, &answer.to_string());
            }
            quiet(request)
        })
    }

    // #825: a Supporter's limited decks go up with no switch. The run turns
    // on the slot of each unarchived limited deck whose slot is off, once,
    // and leaves the ones already on alone.
    #[test]
    fn turns_on_the_slot_of_each_limited_deck_for_a_supporter() {
        let both: &'static [&'static str] = &["limited_on", "limited_off"];
        let stub = slot_server("supporter", &["limited_on"], both, false);
        let device = device("sync-run-limited-auto");
        let on = file_deck(&device, "limited:on", true);
        let off = file_deck(&device, "limited:off", false);
        let local_only = file_deck(&device, "limited:league-7", false);
        let archived = file_deck(&device, "limited:gone", false);
        write(
            &device,
            &format!("UPDATE decks SET archived_at = 1 WHERE id = {archived}"),
        );
        let constructed = file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let sent = puts(&stub);
        assert_eq!(sent.len(), 1, "one PUT, for the deck whose slot is off");
        assert_eq!(sent[0].target, "/api/sync/decks/limited_off");
        assert_eq!(body(&sent[0]), json!({"enabled": true, "kind": "limited"}));
        assert!(flag(&device, on));
        assert!(flag(&device, off), "the answer's ledger is applied");
        assert!(!flag(&device, local_only));
        assert!(!flag(&device, archived));
        assert!(
            !flag(&device, constructed),
            "a constructed deck keeps its switch"
        );
        // The next run finds every slot on: no request at all.
        let stub = slot_server("supporter", both, both, false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(puts(&stub).is_empty());
        assert!(flag(&device, off));
    }

    #[test]
    fn never_turns_on_a_limited_slot_on_the_free_tier() {
        let stub = slot_server("free", &[], &["limited_off"], false);
        let device = device("sync-run-limited-free");
        let off = file_deck(&device, "limited:off", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(puts(&stub).is_empty());
        assert!(!flag(&device, off));
    }

    // Review fix: one run asks for at most `LIMITED_SLOTS_PER_RUN` slots;
    // the rest go next run.
    #[test]
    fn turns_on_at_most_fifty_limited_slots_per_run() {
        let stub = slot_server("supporter", &[], &[], false);
        let device = device("sync-run-limited-cap");
        for number in 0..55 {
            file_deck(&device, &format!("limited:q{number:02}"), false);
        }
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(puts(&stub).len(), LIMITED_SLOTS_PER_RUN);
        assert_eq!(LIMITED_SLOTS_PER_RUN, 50);
    }

    // Review fix: a 429 stops the slot requests (the league and match
    // phases still run); so would no connection, offline mode or signing
    // out.
    #[test]
    fn stops_turning_on_limited_slots_at_a_rate_limit() {
        let manifest = every_manifest(json!({
            "upload": [], "download": [], "slots": tier_ledger("supporter", &[]),
        }));
        let stub = Stub::start(move |request| {
            if request.method == "PUT" {
                return Reply::json(429, r#"{"message":"Too Many Attempts."}"#);
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-limited-429");
        for mtgo_id in ["limited:qa", "limited:qb", "limited:qc"] {
            file_deck(&device, mtgo_id, false);
        }
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(puts(&stub).len(), 1);
        assert!(!manifests(&stub, "match").is_empty(), "the run went on");
    }

    // Review fix: a limited deck turned on by the run sends its match in
    // the same run.
    #[test]
    fn a_newly_enabled_limited_decks_match_uploads_in_the_same_run() {
        let ledger = |listed: &[&str]| tier_ledger("supporter", listed);
        let turned = Arc::new(AtomicUsize::new(0));
        let stub = Stub::start(move |request| {
            if request.method == "PUT" {
                turned.fetch_add(1, Ordering::SeqCst);
                return Reply::json(200, &ledger(&["limited_q"]).to_string());
            }
            let found = body(request);
            if request.target == MANIFEST && found["last"] == true {
                let listed: &[&str] = if turned.load(Ordering::SeqCst) > 0 {
                    &["limited_q"]
                } else {
                    &[]
                };
                let upload = if found["type"] == "match" {
                    json!(["q-limited"])
                } else {
                    json!([])
                };
                let answer = json!({"upload": upload, "download": [], "slots": ledger(listed)});
                return Reply::json(200, &answer.to_string());
            }
            if request.target == BLOBS {
                return Reply::json(200, r#"{"stored":["q-limited"],"rejected":[]}"#);
            }
            quiet(request)
        });
        let device = device("sync-run-limited-same-run");
        file_deck(&device, "limited:q", false);
        log_match(&device, "q-limited", NOW - DAY, "limited:q");
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(dirty_ids(&stub, "match"), vec!["q-limited".to_string()]);
        let sent: Vec<String> = uploaded(&stub).into_iter().map(|(id, _)| id).collect();
        assert_eq!(sent, vec!["q-limited".to_string()]);
    }

    // An Arena limited deck takes a slot only on a server that confirmed
    // Arena (as `set_deck_sync`).
    #[test]
    fn holds_an_arena_limited_slot_until_the_server_confirms_arena() {
        let after: &'static [&'static str] = &["limited_arena_q-course"];
        let stub = slot_server("supporter", &[], after, false);
        let device = device("sync-run-limited-arena");
        let arena = file_deck(&device, "limited:arena_q-course", false);
        write(
            &device,
            &format!("UPDATE decks SET pulled_platform = 'arena' WHERE id = {arena}"),
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(puts(&stub).is_empty(), "Arena is not confirmed");
        let stub = slot_server("supporter", &[], after, true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let sent = puts(&stub);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, "/api/sync/decks/limited_arena_q-course");
        assert!(flag(&device, arena));
    }

    #[test]
    fn carries_on_with_the_run_when_the_slot_release_fails() {
        let manifest = every_manifest(listed_enabled(&["111", "222"]));
        let stub = Stub::start(move |request| {
            if request.method == "PUT" {
                return Reply::json(500, r#"{"message":"Server Error"}"#);
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-release-fails");
        let archived = file_deck(&device, "111", true);
        write(
            &device,
            &format!("UPDATE decks SET archived_at = 1 WHERE id = {archived}"),
        );
        file_deck(&device, "222", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(last_error(&device), None);
        assert!(activity(&api).contains("Sync complete."));
        assert!(!flag(&device, archived), "lowered locally all the same");
        assert!(
            !manifests(&stub, "match").is_empty(),
            "the rest of the run happened"
        );
        assert_eq!(dirty_ids(&stub, "deck"), vec!["222".to_string()]);
    }

    #[test]
    fn confirms_every_account_with_a_login_id_at_the_end_of_a_run() {
        let stub = Stub::start(|request| {
            if request.target == "/api/account/players" {
                return Reply::json(
                    201,
                    r#"{"id":1,"username":"Qkeyed","login_id":6031972,"public":false}"#,
                );
            }
            quiet(request)
        });
        let device = device("sync-run-attest");
        let api = linked(&stub.base_url);
        *lock(&api.accounts) = vec![
            KnownAccount {
                username: "Qkeyed".to_string(),
                login_id: Some(6_031_972),
            },
            KnownAccount {
                username: "Qunkeyed".to_string(),
                login_id: None,
            },
        ];
        run(&api, &device);
        let posts = stub
            .requests()
            .into_iter()
            .filter(|request| request.target == "/api/account/players")
            .count();
        assert_eq!(posts, 1);
        assert!(activity(&api).contains("Confirmed 1 MTGO account."));
        let last = targets(&stub).pop().expect("requests");
        assert_eq!(last, "POST /api/account/players", "after every type");
    }

    #[test]
    fn does_not_re_post_an_account_already_confirmed() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-attested");
        let api = linked(&stub.base_url);
        *lock(&api.accounts) = vec![KnownAccount {
            username: "Qkeyed".to_string(),
            login_id: Some(6_031_972),
        }];
        mymtgo_api::record_attested(&mut api.state.session().attested, 6_031_972, "Qkeyed");
        run(&api, &device);
        assert!(
            !targets(&stub)
                .iter()
                .any(|target| target.ends_with("/api/account/players"))
        );
        assert!(!activity(&api).contains("Confirmed"));
    }

    #[test]
    fn completes_the_run_and_leaves_the_account_owed_when_confirming_fails() {
        let stub = Stub::start(|request| {
            if request.target == "/api/account/players" {
                return Reply::json(500, r#"{"message":"Server Error"}"#);
            }
            quiet(request)
        });
        let device = device("sync-run-attest-fails");
        let api = linked(&stub.base_url);
        *lock(&api.accounts) = vec![KnownAccount {
            username: "Qkeyed".to_string(),
            login_id: Some(6_031_972),
        }];
        run(&api, &device);
        assert_eq!(last_error(&device), None);
        let lines = activity(&api);
        assert!(lines.contains("Sync complete."), "{lines}");
        assert!(
            lines.contains("Could not confirm 1 MTGO account, retrying next run."),
            "{lines}"
        );
        assert!(api.state.session().attested.is_empty());
    }

    // ---- DirtinessTest and the carry --------------------------------------

    #[test]
    fn an_incremental_run_touching_nothing_makes_only_the_manifest_calls() {
        let manifest = answering("deck", json!({"upload": ["111"], "download": []}));
        let uploaded_once = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&uploaded_once);
        let stub = Stub::start(move |request| {
            if request.target == BLOBS {
                seen.fetch_add(1, Ordering::SeqCst);
                return Reply::json(200, r#"{"stored":["111"],"rejected":[]}"#);
            }
            if seen.load(Ordering::SeqCst) > 0 && request.target == MANIFEST {
                return empty_manifest();
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-quiet");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(
            last_pulls(),
            0,
            "a push alone does not wake the cards worker"
        );
        let before = stub.requests().len();
        run(&api, &device);
        let second: Vec<String> = targets(&stub)[before..].to_vec();
        assert_eq!(
            second,
            vec![
                format!("POST {MANIFEST}"),
                format!("POST {MANIFEST}"),
                format!("POST {MANIFEST}"),
            ]
        );
        assert!(
            dirty_ids(&stub, "deck").len() == 1,
            "only the first run sent it"
        );
    }

    #[test]
    fn changed_at_is_the_decks_time_on_the_first_build_and_the_clock_on_a_new_hash() {
        let stub = Stub::start(|request| {
            if request.target == BLOBS {
                return Reply::json(200, r#"{"stored":["111"],"rejected":[]}"#);
            }
            if request.target == MANIFEST && body(request)["last"] == true {
                return Reply::json(200, r#"{"upload":["111"],"download":[]}"#);
            }
            quiet(request)
        });
        let device = device("sync-run-changed-at");
        let id = file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(
            synced(&device, "111").and_then(|row| row.changed_at),
            Some(NOW - DAY)
        );
        write(
            &device,
            &format!("UPDATE decks SET updated_at = updated_at + 5 WHERE id = {id}"),
        );
        run(&api, &device);
        assert_eq!(
            synced(&device, "111").and_then(|row| row.changed_at),
            Some(NOW - DAY),
            "touched, same bytes: the time stays"
        );
        write(
            &device,
            &format!(
                "UPDATE decks SET name = 'Qrenamed', updated_at = updated_at + 5 WHERE id = {id}"
            ),
        );
        run(&api, &device);
        assert_eq!(
            synced(&device, "111").and_then(|row| row.changed_at),
            Some(NOW)
        );
        let last = manifests(&stub, "deck")
            .into_iter()
            .rev()
            .find(|call| call["last"] == false)
            .expect("dirty call");
        assert_eq!(last["dirty"][0]["updated_at"], json!(bundle_datetime(NOW)));
    }

    // Archetypes spec section 14: a deck 0.x synced (a server hash, no
    // carry) is no longer fetched first, since its archetype, guides and
    // notes are rows here (the 0.x import brought them); it is pushed from
    // them.
    #[test]
    fn a_0x_synced_deck_is_pushed_from_its_rows_without_a_carry_fetch() {
        let stub = Stub::start(move |request| match request.target.as_str() {
            BLOBS => Reply::json(200, r#"{"stored":["111"],"rejected":[]}"#),
            MANIFEST if body(request)["last"] == true => {
                Reply::json(200, r#"{"upload":["111"],"download":[]}"#)
            }
            _ => quiet(request),
        });
        let device = device("sync-run-no-carry-fetch");
        let id = file_deck(&device, "111", false);
        write(
            &device,
            &format!(
                "INSERT INTO synced_rows (type, client_id, hash, synced_at)
                     VALUES ('deck', '111', 'q0x-hash', 1);
                 INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                     VALUES ('q-archetype', 'Qarchetype', 'modern', 'downloaded', 1, 1),
                         ('q-foe', 'Qfoe', 'modern', 'downloaded', 1, 1);
                 UPDATE decks SET archetype_id = (SELECT id FROM archetypes
                     WHERE uuid = 'q-archetype') WHERE id = {id};
                 INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
                     VALUES ({id}, (SELECT id FROM archetypes WHERE uuid = 'q-foe'), 1, 1);
                 INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
                     VALUES (last_insert_rowid(), 'q-oracle', 'in', 2);
                 INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
                     VALUES ({id}, (SELECT id FROM archetypes WHERE uuid = 'q-foe'), 'Qnote',
                         1787216400000);"
            ),
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(
            targets(&stub).iter().all(|target| !target.contains(FETCH)),
            "{:?}",
            targets(&stub)
        );
        let pushed = uploaded(&stub);
        assert_eq!(pushed.len(), 1);
        let (_, bundle) = &pushed[0];
        assert_eq!(
            bundle["guides"],
            json!([{"archetype_uuid": "q-foe", "cards": [
                {"oracle_id": "q-oracle", "direction": "in", "quantity": 2}
            ]}])
        );
        assert_eq!(
            bundle["notes"],
            json!([{"archetype_uuid": "q-foe", "body": "Qnote",
                    "created_at": "2026-08-20T09:00:00Z"}])
        );
        assert_eq!(bundle["deck"]["archetype_uuid"], json!("q-archetype"));
        let row = synced(&device, "111").expect("row");
        assert_eq!(
            row.carry.as_deref(),
            Some("{}"),
            "nothing is carried for a deck"
        );
    }

    // ---- BundleRoundTripTest (deck cases) ---------------------------------

    #[test]
    fn restores_cover_and_archetype_from_a_deck_bundle_and_rebuilds_the_same_bytes() {
        let bundle = deck_bundle_of("777", Some("424242"));
        let answer = blob("777", &bundle, true);
        let manifest = answering("deck", json!({"upload": [], "download": ["777"]}));
        let pulled = Arc::new(AtomicUsize::new(0));
        let once = Arc::clone(&pulled);
        let stub = Stub::start(move |request| {
            if request.target == FETCH {
                once.fetch_add(1, Ordering::SeqCst);
                return Reply::json(200, &json!({"blobs": [answer.clone()]}).to_string());
            }
            if once.load(Ordering::SeqCst) > 0 && request.target == MANIFEST {
                return empty_manifest();
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-round-trip");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let row = synced(&device, "777").expect("row");
        assert_eq!(row.carry.as_deref(), Some("{}"));
        // The archetype, guide and note are rows now (unknown uuids stubbed).
        let conn = reader(&device);
        let (archetype, guides, notes): (String, i64, i64) = conn
            .query_row(
                "SELECT a.uuid, (SELECT COUNT(*) FROM sideboard_guide_cards),
                     (SELECT COUNT(*) FROM deck_archetype_notes)
                 FROM decks d JOIN archetypes a ON a.id = d.archetype_id
                 WHERE d.mtgo_id = '777'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("parts");
        assert_eq!((archetype.as_str(), guides, notes), ("q-archetype", 1, 1));
        // The pulled deck is not v1's (ruling 5), so it is never offered.
        write(
            &device,
            "UPDATE decks SET source = 'file' WHERE mtgo_id = '777'",
        );
        run(&api, &device);
        assert!(
            dirty_ids(&stub, "deck").is_empty(),
            "rebuilt from v1's rows, the bytes equal the cloud's"
        );
    }

    // ---- FullReconcileTest ------------------------------------------------

    fn clean_deck(api: &TestApi, device: &Device, stub: &Stub) {
        run(api, device);
        assert!(
            synced(device, "111").and_then(|row| row.hash).is_some(),
            "{:?}",
            targets(stub)
        );
    }

    fn storing() -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
        |request: &Recorded| match request.target.as_str() {
            BLOBS => {
                let text = String::from_utf8_lossy(&request.body).to_string();
                let ids: Vec<String> = text
                    .split("[client_id]\"\r\n\r\n")
                    .skip(1)
                    .map(|rest| rest.split("\r\n").next().unwrap_or_default().to_string())
                    .collect();
                Reply::json(200, &json!({"stored": ids, "rejected": []}).to_string())
            }
            MANIFEST if body(request)["last"] == false => {
                let ids: Vec<Value> = body(request)["dirty"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|entry| entry["client_id"].clone())
                    .collect();
                Reply::json(200, &json!({"upload": ids, "download": []}).to_string())
            }
            _ => quiet(request),
        }
    }

    #[test]
    fn bypasses_the_clean_check_in_full_mode() {
        let stub = Stub::start(storing());
        let device = device("sync-full-bypass");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        clean_deck(&api, &device, &stub);
        let before = manifests(&stub, "deck").len();
        run_full(&api, &device);
        let calls = manifests(&stub, "deck")[before..].to_vec();
        let row = synced(&device, "111").expect("row");
        assert_eq!(
            calls[0]["dirty"],
            json!([{"client_id": "111", "hash": row.hash, "updated_at": bundle_datetime(NOW - DAY)}])
        );
        assert_eq!(calls[0]["since"], Value::Null);
    }

    #[test]
    fn excludes_a_free_accounts_limited_decks_in_full_mode_too() {
        let stub = Stub::start(quiet);
        let device = device("sync-full-limited");
        file_deck(&device, "limited:abc", true);
        let api = linked(&stub.base_url);
        run_full(&api, &device);
        assert!(dirty_ids(&stub, "deck").is_empty());
    }

    #[test]
    fn leaves_an_archived_deck_out_of_a_full_reconcile_too() {
        let stub = Stub::start(quiet);
        let device = device("sync-full-archived");
        let id = file_deck(&device, "111", false);
        write(
            &device,
            &format!("UPDATE decks SET archived_at = 1 WHERE id = {id}"),
        );
        let api = linked(&stub.base_url);
        run_full(&api, &device);
        assert!(dirty_ids(&stub, "deck").is_empty());
    }

    #[test]
    fn persists_the_cursor_past_a_pushed_batch_even_when_the_run_is_interrupted() {
        let storer = storing();
        let stub = Stub::start(move |request| match request.target.as_str() {
            FETCH => Reply::json(500, r#"{"error":"boom"}"#),
            MANIFEST if body(request)["last"] == true => {
                Reply::json(200, r#"{"upload":[],"download":["q-ghost"]}"#)
            }
            _ => storer(request),
        });
        let device = device("sync-full-cursor");
        file_deck(&device, "111", false);
        file_deck(&device, "222", false);
        let api = linked(&stub.base_url);
        run_full(&api, &device);
        let state = deck_state(&device).expect("state");
        assert_eq!(state.full_cursor.as_deref(), Some("222"));
        assert_eq!(state.last_synced_at, None);
        assert!(last_error(&device).is_some_and(|error| error.contains("500")));
    }

    #[test]
    fn resumes_the_full_scan_past_a_set_cursor() {
        let stub = Stub::start(quiet);
        let device = device("sync-full-resume");
        file_deck(&device, "111", false);
        file_deck(&device, "222", false);
        write(
            &device,
            "INSERT INTO sync_state (type, canonical_version, full_cursor) VALUES ('deck', 4, '111')",
        );
        let api = linked(&stub.base_url);
        run_full(&api, &device);
        assert_eq!(dirty_ids(&stub, "deck"), vec!["222".to_string()]);
    }

    #[test]
    fn forces_full_mode_for_a_type_on_its_own_when_canonical_version_mismatches() {
        let stub = Stub::start(storing());
        let device = device("sync-full-version");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        clean_deck(&api, &device, &stub);
        write(
            &device,
            "UPDATE sync_state SET canonical_version = 0 WHERE type = 'deck'",
        );
        let before = manifests(&stub, "deck").len();
        run(&api, &device);
        let calls = manifests(&stub, "deck")[before..].to_vec();
        assert_eq!(calls[0]["dirty"][0]["client_id"], json!("111"));
        assert_eq!(calls[0]["since"], Value::Null);
        let state = deck_state(&device).expect("state");
        assert_eq!(state.canonical_version, 4);
        assert_eq!(state.full_cursor, None);
    }

    #[test]
    fn clears_full_cursor_and_stamps_canonical_version_once_a_full_run_completes() {
        let stub = Stub::start(quiet);
        let device = device("sync-full-finish");
        file_deck(&device, "111", false);
        write(
            &device,
            "INSERT INTO sync_state (type, canonical_version, full_cursor) VALUES ('deck', 4, 'zzz')",
        );
        let api = linked(&stub.base_url);
        run_full(&api, &device);
        let state = deck_state(&device).expect("state");
        assert_eq!(state.full_cursor, None);
        assert_eq!(state.canonical_version, 4);
        assert_eq!(state.last_synced_at, Some(NOW));
    }

    // ---- Download cursor (sync identity spec section 3) -------------------

    /// A server whose every manifest answer carries `cursor`.
    fn cursoring(cursor: &'static str) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
        move |request: &Recorded| match request.target.as_str() {
            MANIFEST => Reply::json(
                200,
                &json!({"upload": [], "download": [], "tombstones": [], "cursor": cursor})
                    .to_string(),
            ),
            _ => quiet(request),
        }
    }

    fn download_cursor(device: &Device, sync_type: SyncType) -> Option<String> {
        store::sync::sync_state(&reader(device), sync_type)
            .expect("state")
            .and_then(|state| state.download_cursor)
    }

    fn all_manifests(stub: &Stub, from: usize) -> Vec<Value> {
        stub.requests()[from..]
            .iter()
            .filter(|request| request.target == MANIFEST)
            .map(body)
            .collect()
    }

    #[test]
    fn a_second_run_sends_back_the_first_runs_cursor_and_every_call_names_the_device() {
        let stub = Stub::start(cursoring("2026-10-16T09:58:00.123456Z"));
        let device = device("sync-cursor-second-run");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let first = all_manifests(&stub, 0);
        assert!(first.len() >= 3);
        for call in &first {
            assert_eq!(call["cursor"], Value::Null, "nothing stored yet: {call}");
            assert_eq!(call["device"], json!("qdevice"));
        }
        for sync_type in SYNC_TYPES {
            assert_eq!(
                download_cursor(&device, sync_type).as_deref(),
                Some("2026-10-16T09:58:00.123456Z"),
                "{}",
                sync_type.as_str()
            );
        }
        let before = stub.requests().len();
        run(&api, &device);
        let second = all_manifests(&stub, before);
        assert!(second.len() >= 3);
        for call in &second {
            assert_eq!(call["cursor"], json!("2026-10-16T09:58:00.123456Z"));
            assert_eq!(call["device"], json!("qdevice"));
            assert_ne!(call["since"], Value::Null, "since is still sent");
        }
    }

    #[test]
    fn a_full_run_sends_no_cursor_and_stores_the_new_one() {
        let stub = Stub::start(cursoring("2026-10-16T10:00:00.000000Z"));
        let device = device("sync-cursor-full");
        write(
            &device,
            "INSERT INTO sync_state (type, last_synced_at, canonical_version, download_cursor)
             VALUES ('deck', 5, 4, '2026-10-01T00:00:00.000000Z')",
        );
        let api = linked(&stub.base_url);
        run_full(&api, &device);
        for call in all_manifests(&stub, 0) {
            assert_eq!(call["cursor"], Value::Null);
            assert_eq!(call["since"], Value::Null);
            assert_eq!(call["device"], json!("qdevice"));
        }
        assert_eq!(
            download_cursor(&device, SyncType::Deck).as_deref(),
            Some("2026-10-16T10:00:00.000000Z")
        );
    }

    #[test]
    fn a_canonical_version_change_sends_no_cursor_for_that_type_alone() {
        let stub = Stub::start(cursoring("2026-10-16T10:00:00.000000Z"));
        let device = device("sync-cursor-version");
        write(
            &device,
            "INSERT INTO sync_state (type, last_synced_at, canonical_version, download_cursor)
             VALUES ('deck', 5, 0, 'qdeck-cursor'), ('league', 5, 4, 'qleague-cursor')",
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        let deck = manifests(&stub, "deck");
        assert!(deck.iter().all(|call| call["cursor"] == Value::Null));
        let league = manifests(&stub, "league");
        assert!(
            league
                .iter()
                .all(|call| call["cursor"] == json!("qleague-cursor"))
        );
    }

    // Review Focus 5: an older server sends no cursor; the stored one stays
    // and the `since` flow carries on.
    #[test]
    fn an_answer_with_no_cursor_keeps_the_stored_one() {
        let stub = Stub::start(quiet);
        let device = device("sync-cursor-older-server");
        write(
            &device,
            "INSERT INTO sync_state (type, last_synced_at, canonical_version, download_cursor)
             VALUES ('deck', 5, 4, 'qkept-cursor')",
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        let deck = manifests(&stub, "deck");
        assert_eq!(deck[0]["cursor"], json!("qkept-cursor"));
        assert_eq!(deck[0]["since"], json!(bundle_datetime(5)));
        let state = deck_state(&device).expect("state");
        assert_eq!(state.download_cursor.as_deref(), Some("qkept-cursor"));
        assert_eq!(state.last_synced_at, Some(NOW));
    }

    #[test]
    fn a_run_that_fails_before_it_finishes_keeps_the_old_cursor() {
        let stub = Stub::start(|request| match request.target.as_str() {
            FETCH => Reply::json(500, r#"{"error":"boom"}"#),
            MANIFEST => Reply::json(
                200,
                r#"{"upload":[],"download":["q-ghost"],"tombstones":[],"cursor":"qnew-cursor"}"#,
            ),
            _ => quiet(request),
        });
        let device = device("sync-cursor-failed");
        write(
            &device,
            "INSERT INTO sync_state (type, last_synced_at, canonical_version, download_cursor)
             VALUES ('deck', 5, 4, 'qold-cursor')",
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert!(last_error(&device).is_some_and(|error| error.contains("500")));
        let state = deck_state(&device).expect("state");
        assert_eq!(state.download_cursor.as_deref(), Some("qold-cursor"));
        assert_eq!(state.last_synced_at, Some(5));
    }

    #[test]
    fn an_upload_names_the_device() {
        let stub = Stub::start(storing());
        let device = device("sync-cursor-upload-device");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let uploads: Vec<String> = stub
            .requests()
            .iter()
            .filter(|request| request.target == BLOBS)
            .map(|request| String::from_utf8_lossy(&request.body).to_string())
            .collect();
        assert_eq!(uploads.len(), 1);
        assert!(
            uploads[0].contains("name=\"device\"\r\n\r\nqdevice\r\n"),
            "{}",
            uploads[0]
        );
    }

    // Sync identity spec 3.2: the cursor lags, so a run can be offered a
    // row it pulled last time. Pulling the same copy again changes nothing
    // and offers nothing back.
    #[test]
    fn a_re_offered_download_matching_the_synced_hash_changes_nothing() {
        let stub = pulling("match", "q-cloud", cloud_match("q-cloud"));
        let device = device("sync-cursor-re-offered");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let first = match_row(&device, "q-cloud").expect("pulled");
        let count = |device: &Device| -> i64 {
            reader(device)
                .query_row("SELECT COUNT(*) FROM matches", [], |row| row.get(0))
                .expect("count")
        };
        assert_eq!(count(&device), 1);
        run(&api, &device);
        assert_eq!(match_row(&device, "q-cloud"), Some(first));
        assert_eq!(count(&device), 1);
        let before = stub.requests().len();
        run(&api, &device);
        let dirty: Vec<Value> = stub.requests()[before..]
            .iter()
            .filter(|request| request.target == MANIFEST)
            .map(body)
            .flat_map(|found| found["dirty"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(dirty.is_empty(), "nothing is offered back: {dirty:?}");
        assert!(
            !stub.requests()[before..]
                .iter()
                .any(|request| request.target == BLOBS)
        );
    }

    // ---- Review Focus 5 ---------------------------------------------------

    #[test]
    fn a_bad_pull_ends_the_run_with_the_error_and_no_partial_state() {
        let good = blob("qa", &deck_bundle_of("qa", None), true);
        let tampered = blob("qb", &deck_bundle_of("qb", None), false);
        let fetches = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&fetches);
        let manifest = answering(
            "deck",
            json!({"upload": [], "download": ["qa", "qb", "qc"]}),
        );
        let stub = Stub::start(move |request| {
            if request.target == FETCH {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    // Fewer blobs than asked, one with a wrong hash.
                    let blobs = json!({"blobs": [good.clone(), tampered.clone()]});
                    return Reply::json(200, &blobs.to_string());
                }
                return Reply::json(429, r#"{"message":"Too Many Attempts."}"#);
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        });
        let device = device("sync-run-bad-pull");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let asked: Vec<Value> = stub
            .requests()
            .iter()
            .filter(|request| request.target == FETCH)
            .map(|request| body(request)["client_ids"].clone())
            .collect();
        assert_eq!(asked, vec![json!(["qa", "qb", "qc"]), json!(["qb", "qc"])]);
        let error = last_error(&device).expect("the run's error");
        assert!(
            error.starts_with("Sync API request failed with status 429"),
            "{error}"
        );
        assert_eq!(deck_state(&device), None, "the type never finished");
        assert!(synced(&device, "qa").is_some(), "a verified blob is whole");
        for missing in ["qb", "qc"] {
            assert!(synced(&device, missing).is_none(), "{missing}");
            let decks: i64 = reader(&device)
                .query_row(
                    "SELECT COUNT(*) FROM decks WHERE mtgo_id = ?1",
                    [missing],
                    |row| row.get(0),
                )
                .expect("count");
            assert_eq!(decks, 0, "{missing}");
        }
        assert!(manifests(&stub, "league").is_empty(), "the run ended");
    }

    // ---- Gate (Review Focus 1) --------------------------------------------

    #[test]
    fn a_signed_out_device_aborts_quietly_without_traffic() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-signed-out");
        let api = test_api(&stub.base_url, &[], &[]);
        run(&api, &device);
        assert_eq!(stub.connections(), 0);
        assert_eq!(last_error(&device), None);
        assert!(activity(&api).contains("Aborted: this device is not linked."));
    }

    #[test]
    fn no_sync_traffic_while_offline_or_signed_out() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-gated");
        file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        api.state.set_offline_mode(true, None);
        run(&api, &device);
        assert!(activity(&api).contains("Aborted: offline mode is enabled."));
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(counts, CloudCounts::default());
        let refused = set_deck_sync(
            &api.state,
            &editor(&device),
            &reader(&device),
            1,
            true,
            Tz::UTC,
            NOW,
        );
        assert!(refused.is_err());
        api.state.set_offline_mode(false, None);
        crate::api::tokens::sign_out_tokens(&api.state);
        run(&api, &device);
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(counts, CloudCounts::default());
        assert!(
            set_deck_sync(
                &api.state,
                &editor(&device),
                &reader(&device),
                1,
                true,
                Tz::UTC,
                NOW
            )
            .is_err()
        );
        assert_eq!(stub.connections(), 0);
        assert_eq!(last_error(&device), None, "a held run is not a failure");
    }

    // ---- RunSyncJobTest ---------------------------------------------------

    #[test]
    fn one_run_at_a_time_and_a_trigger_during_a_run_queues_one_follow_up() {
        let state = SyncRunState::default();
        assert!(state.try_start(false));
        assert!(state.syncing());
        assert!(!state.try_start(false), "a second incremental waits");
        assert!(!state.try_start(false), "and folds into the same follow-up");
        assert_eq!(state.finish(), Some(false), "one follow-up runs");
        assert_eq!(state.finish(), None);
        assert!(!state.syncing());
        assert!(state.try_start(true));
        assert!(!state.try_start(false));
        assert!(
            !state.try_start(true),
            "a full trigger makes the follow-up full"
        );
        assert_eq!(state.finish(), Some(true));
        assert_eq!(state.finish(), None);
    }

    #[test]
    fn a_trigger_during_a_run_runs_after_it_on_the_same_thread() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-follow-up");
        let api = linked(&stub.base_url);
        assert!(api.state.sync.try_start(false), "a run is going");
        assert_eq!(
            run_sync_now(&api, &device),
            RunEnd::Queued,
            "a trigger during it waits"
        );
        assert_eq!(stub.connections(), 0);
        assert_eq!(api.state.sync.finish(), Some(false));
        api.state.sync.finish();
        assert_eq!(run_sync_now(&api, &device), RunEnd::Ran);
        assert_eq!(manifests(&stub, "deck").len(), 1);
    }

    fn run_sync_now(api: &TestApi, device: &Device) -> RunEnd {
        run_reporting(api, device, false)
    }

    fn run_reporting(api: &TestApi, device: &Device, report_unreachable: bool) -> RunEnd {
        let editor = editor(device);
        let db = device.state.db_path.clone();
        let reader = move || store::open_reader(&db).ok();
        let ctx = SyncContext {
            api: &api.state,
            editor: &editor,
            reader: &reader,
            now: &|| NOW,
            notify: &|_, _| {},
            sleep: &|_| {},
            limits: Limits::default(),
            timezone: Tz::UTC,
            pulled: &|| {},
            guides_pulled: &|_| {},
            report_unreachable,
        };
        run_sync(&ctx, false)
    }

    /// A base URL nothing listens on.
    fn dead_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        format!("http://127.0.0.1:{port}")
    }

    // At launch or wake the network is often not up yet: a run that cannot
    // reach MyMTGO is not shown as failed while the worker's retries are
    // left, and is once they are used.
    #[test]
    fn a_run_that_cannot_reach_mymtgo_is_shown_only_once_the_retries_are_used() {
        let device = device("sync-run-unreachable");
        let api = linked(&dead_url());
        assert_eq!(run_reporting(&api, &device, false), RunEnd::Unreachable);
        assert_eq!(
            last_error(&device),
            None,
            "not shown while retries are left"
        );
        assert!(activity(&api).contains("Aborted: connection failed"));
        assert_eq!(run_reporting(&api, &device, true), RunEnd::Unreachable);
        let error = last_error(&device).expect("shown once the retries are used");
        assert!(error.starts_with("connection failed"), "{error}");
    }

    // A failure the server answered is still shown at once.
    #[test]
    fn an_http_failure_is_shown_at_once() {
        let stub = Stub::start(|request| {
            if request.target.starts_with("/api/sync/") {
                return Reply::json(500, r#"{"message":"Server Error"}"#);
            }
            quiet(request)
        });
        let device = device("sync-run-http-failure");
        let api = linked(&stub.base_url);
        assert_eq!(run_reporting(&api, &device, false), RunEnd::Ran);
        assert!(last_error(&device).is_some_and(|error| error.contains("500")));
    }

    // ---- SyncSettingsTest -------------------------------------------------

    #[test]
    fn shows_an_unlinked_empty_status() {
        let device = device("sync-status-empty");
        let api = test_api("http://127.0.0.1:1", &[], &[]);
        let status = sync_status_now(&api.state, &reader(&device), Tz::UTC, NOW).expect("status");
        assert_eq!(
            status,
            SyncStatus {
                linked: false,
                last_synced_at_ms: None,
                pending: 0,
                queued: 0,
                pending_by_type: TypeCounts::default(),
                activity: Vec::new(),
                syncing: false,
                not_synced: 0,
                slots: None,
                rejections: 0,
                last_error: None,
            }
        );
    }

    #[test]
    fn shows_linked_status_with_pending_not_synced_and_rejections_from_stored_rows() {
        let device = device("sync-status-linked");
        file_deck(&device, "111", false);
        let archived = file_deck(&device, "222", false);
        write(
            &device,
            &format!(
                "UPDATE decks SET archived_at = 1 WHERE id = {archived};
                 INSERT INTO sync_state (type, last_synced_at, canonical_version)
                     VALUES ('match', 1000, 4), ('deck', 2000, 4);
                 INSERT INTO sync_rejections (type, client_id, reason, hash)
                     VALUES ('match', 'q-token', 'hash_mismatch', 'h');
                 INSERT INTO settings (key, value)
                     VALUES ('sync_slots', '{{\"limit\":1,\"used\":1,\"decks\":[]}}');"
            ),
        );
        let api = linked("http://127.0.0.1:1");
        let status = sync_status_now(&api.state, &reader(&device), Tz::UTC, NOW).expect("status");
        assert!(status.linked);
        assert_eq!(status.last_synced_at_ms, Some(2000));
        assert_eq!(status.pending, 1);
        assert_eq!(
            status.pending_by_type,
            TypeCounts {
                matches: 0,
                decks: 1,
                leagues: 0
            }
        );
        assert_eq!(status.not_synced, 1);
        assert_eq!(
            status.slots,
            Some(SlotCounts {
                limit: Some(1),
                used: 1
            })
        );
        assert_eq!(status.rejections, 1);
        let json = serde_json::to_value(&status).expect("json");
        assert_eq!(
            json["pendingByType"],
            json!({"match": 0, "deck": 1, "league": 0})
        );
    }

    /// The status bar's "queued" counts a match still in its send hold too,
    /// so a match that just ended never reads as synced; `pending` stays
    /// what the next run sends.
    #[test]
    fn queued_counts_matches_still_in_their_send_hold() {
        let device = device("sync-status-queued");
        file_deck(&device, "111", true);
        log_match(&device, "q-old", NOW - DAY, "111");
        log_match(&device, "q-held", NOW - 2 * 60_000, "111");
        let api = linked("http://127.0.0.1:1");
        let status = sync_status_now(&api.state, &reader(&device), Tz::UTC, NOW).expect("status");
        assert_eq!(status.pending_by_type.matches, 1, "the held match waits");
        assert_eq!(status.pending, 2);
        assert_eq!(status.queued, 3);
    }

    // Batch 5 review Important 1: a press whose run never starts (the gate
    // refuses, tracking restarts) must not leave the button on "Syncing".
    #[test]
    fn a_press_whose_run_never_starts_stops_reporting_syncing() {
        let state = SyncRunState::default();
        state.ask(NOW);
        assert!(state.syncing());
        state.clear_asked();
        assert!(!state.syncing(), "the ask is withdrawn");
        assert!(state.try_start(false), "a later run still starts");
        state.ask(NOW);
        assert!(state.syncing());
        state.clear_asked();
        assert!(
            state.syncing(),
            "a run that is going still reads as syncing"
        );
    }

    #[test]
    fn the_status_does_not_wait_on_a_press_the_gate_refuses() {
        let device = device("sync-status-gate-refused");
        let stub = Stub::start(quiet);
        let api = linked(&stub.base_url);
        api.state.sync.ask(NOW);
        let status = sync_status_now(&api.state, &reader(&device), Tz::UTC, NOW).expect("status");
        assert!(status.syncing, "the gate is open: the run is coming");
        api.state.set_offline_mode(true, None);
        let status = sync_status_now(&api.state, &reader(&device), Tz::UTC, NOW).expect("status");
        assert!(!status.syncing, "offline: no run will start");
        api.state.set_offline_mode(false, None);
        let status = sync_status_now(&api.state, &reader(&device), Tz::UTC, NOW).expect("status");
        assert!(!status.syncing, "the press was withdrawn, not parked");
    }

    #[test]
    fn reports_syncing_from_the_press_until_the_run_ends() {
        let state = SyncRunState::default();
        state.ask(NOW);
        assert!(state.syncing());
        assert_eq!(state.activity()[0].1, "Sync queued.");
        assert!(state.try_start(false));
        state.finish();
        assert!(!state.syncing());
    }

    #[test]
    fn reports_null_cloud_counts_when_the_device_is_not_linked() {
        let stub = Stub::start(quiet);
        let device = device("sync-cloud-unlinked");
        let api = test_api(&stub.base_url, &[], &[]);
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(
            counts,
            CloudCounts {
                download: None,
                slots: None
            }
        );
        assert_eq!(stub.connections(), 0);
    }

    fn cloud_stub(slots: Value) -> Stub {
        // The status call carries its `platforms[]` query.
        Stub::start(
            move |request| match request.target.split('?').next().unwrap_or_default() {
                "/api/sync/status" => {
                    Reply::json(200, r#"{"counts":{"match":441,"deck":8,"league":3}}"#)
                }
                "/api/sync/decks/slots" => Reply::json(200, &slots.to_string()),
                _ => quiet(request),
            },
        )
    }

    #[test]
    fn reports_what_is_waiting_in_the_cloud_as_server_counts_minus_local_rows() {
        let stub = cloud_stub(json!({"limit": 1, "used": 1, "decks": []}));
        let device = device("sync-cloud-counts");
        for index in 0..3 {
            file_deck(&device, &format!("{index}"), false);
        }
        let api = linked(&stub.base_url);
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(
            counts.download,
            Some(TypeCounts {
                matches: 441,
                decks: 5,
                leagues: 3
            })
        );
    }

    // A deleted deck the server still holds is this device's, not one
    // waiting in the cloud.
    #[test]
    fn deleted_decks_the_server_holds_are_not_waiting_in_the_cloud() {
        let stub = cloud_stub(json!({"limit": 1, "used": 1, "decks": []}));
        let device = device("sync-cloud-deleted");
        for index in 0..4 {
            file_deck(&device, &format!("{index}"), false);
        }
        write(
            &device,
            "INSERT INTO synced_rows (type, client_id, hash) VALUES ('deck', '2', 'h2');
             UPDATE decks SET archived_at = 1 WHERE mtgo_id IN ('2', '3');",
        );
        let api = linked(&stub.base_url);
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(counts.download.map(|counts| counts.decks), Some(5));
    }

    #[test]
    fn returns_the_server_slot_ledger_and_writes_it_down() {
        let stub = cloud_stub(json!({"limit": 1, "used": 1, "decks": [
            {"client_id": "111", "enabled_at": "2026-09-01 09:30:00", "disabled_at": null, "frees_at": null}
        ]}));
        let device = device("sync-cloud-slots");
        let id = file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(
            counts.slots,
            Some(SlotCounts {
                limit: Some(1),
                used: 1
            })
        );
        assert!(flag(&device, id));
    }

    #[test]
    fn reports_null_counts_and_null_slots_when_the_slot_ledger_call_fails() {
        let stub =
            Stub::start(
                |request| match request.target.split('?').next().unwrap_or_default() {
                    "/api/sync/status" => {
                        Reply::json(200, r#"{"counts":{"match":1,"deck":1,"league":1}}"#)
                    }
                    _ => Reply::json(500, "[]"),
                },
            );
        let device = device("sync-cloud-fails");
        let api = linked(&stub.base_url);
        let counts = cloud_counts(&api.state, &editor(&device), &reader(&device), NOW);
        assert_eq!(counts, CloudCounts::default());
    }

    // ---- SetDeckCloudSyncTest, UpdateCloudSyncTest ------------------------

    fn toggle(
        api: &TestApi,
        device: &Device,
        deck_id: i64,
        enabled: bool,
    ) -> Result<SlotCounts, CommandError> {
        set_deck_sync(
            &api.state,
            &editor(device),
            &reader(device),
            deck_id,
            enabled,
            Tz::UTC,
            NOW,
        )
    }

    /// A file deck whose account is on `platform`.
    fn platform_deck(device: &Device, platform: &str) -> i64 {
        let id = file_deck(device, "111", false);
        write(
            device,
            &format!(
                "INSERT INTO accounts (id, username, first_seen_at, platform)
                 VALUES (90, '{platform}:u1', 0, '{platform}');
                 UPDATE decks SET account_id = 90 WHERE id = {id};"
            ),
        );
        id
    }

    /// A deck of an `platform` account takes a slot like an MTGO deck once
    /// the server confirmed the platform (one empty manifest asks): the
    /// request goes out under its client id and the answer is mirrored.
    fn takes_a_slot_for(platform: &str) {
        let stub = Stub::start(|request| {
            if request.target == MANIFEST {
                return Reply::json(
                    200,
                    r#"{"upload":[],"download":[],"tombstones":[],"platforms":["arena","endstep"]}"#,
                );
            }
            Reply::json(
                201,
                r#"{"limit":1,"used":1,"decks":[{"client_id":"111","enabled_at":"x","disabled_at":null,"frees_at":null}]}"#,
            )
        });
        let device = device(&format!("sync-toggle-{platform}"));
        let id = platform_deck(&device, platform);
        let api = linked(&stub.base_url);
        assert!(toggle(&api, &device, id, true).is_ok());
        assert!(flag(&device, id));
        let targets: Vec<String> = stub
            .requests()
            .iter()
            .map(|request| request.target.clone())
            .collect();
        assert_eq!(
            targets,
            vec![MANIFEST.to_string(), "/api/sync/decks/111".to_string()]
        );
        // Confirmed for the session: the next change asks no more.
        assert!(toggle(&api, &device, id, true).is_ok());
        assert_eq!(stub.requests().len(), 3);
    }

    #[test]
    fn deck_slot_sends_arena() {
        takes_a_slot_for("arena");
    }

    #[test]
    fn deck_slot_sends_endstep() {
        takes_a_slot_for("endstep");
    }

    #[test]
    fn an_mtgo_account_deck_still_takes_a_slot() {
        let stub = Stub::start(|_| {
            Reply::json(
                201,
                r#"{"limit":1,"used":1,"decks":[{"client_id":"111","enabled_at":"x","disabled_at":null,"frees_at":null}]}"#,
            )
        });
        let device = device("sync-toggle-mtgo-account");
        let id = platform_deck(&device, "mtgo");
        let api = linked(&stub.base_url);
        assert!(toggle(&api, &device, id, true).is_ok());
        assert!(flag(&device, id));
    }

    #[test]
    fn enables_cloud_sync_for_a_deck_and_stores_the_slots() {
        let stub = Stub::start(|_| {
            Reply::json(
                201,
                r#"{"limit":1,"used":1,"decks":[{"client_id":"111","enabled_at":"x","disabled_at":null,"frees_at":null}]}"#,
            )
        });
        let device = device("sync-toggle-on");
        let id = file_deck(&device, "111", false);
        let api = linked(&stub.base_url);
        assert_eq!(
            toggle(&api, &device, id, true),
            Ok(SlotCounts {
                limit: Some(1),
                used: 1
            })
        );
        assert!(flag(&device, id));
        let put = stub.requests().remove(0);
        assert_eq!(put.method, "PUT");
        assert_eq!(put.target, "/api/sync/decks/111");
        assert_eq!(body(&put), json!({"enabled": true, "kind": "constructed"}));
        assert_eq!(put.header("authorization"), Some("Bearer qaccess"));
    }

    #[test]
    fn disables_cloud_sync() {
        let stub = Stub::start(|_| {
            Reply::json(
                200,
                r#"{"limit":1,"used":1,"decks":[{"client_id":"111","enabled_at":"x","disabled_at":"y","frees_at":"z"}]}"#,
            )
        });
        let device = device("sync-toggle-off");
        let id = file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        assert!(toggle(&api, &device, id, false).is_ok());
        assert!(!flag(&device, id));
    }

    fn refused(message: &str) -> Result<SlotCounts, CommandError> {
        Err(CommandError::Refused(message.to_string()))
    }

    #[test]
    fn reports_slot_limit_with_the_deck_holding_the_slot_and_leaves_the_flag_off() {
        let stub = Stub::start(|_| {
            Reply::json(
                422,
                r#"{"error":"slot_limit","limit":1,"used":1,"frees_at":null}"#,
            )
        });
        let device = device("sync-toggle-limit");
        let id = file_deck(&device, "222", false);
        let api = linked(&stub.base_url);
        assert_eq!(
            toggle(&api, &device, id, true),
            refused("Your free deck slot is in use. Turn off sync on another deck first.")
        );
        file_deck(&device, "111", true);
        assert_eq!(
            toggle(&api, &device, id, true),
            refused(
                "Your free deck slot is in use by Qdeck 111. Turn off sync on that deck first."
            )
        );
        assert!(!flag(&device, id));
    }

    #[test]
    fn reports_a_cooling_slot_with_the_day_it_frees() {
        let stub = Stub::start(|_| {
            Reply::json(
                422,
                r#"{"error":"slot_limit","limit":1,"used":1,"frees_at":"2026-11-05T09:30:00+00:00"}"#,
            )
        });
        let device = device("sync-toggle-cooling");
        let id = file_deck(&device, "222", false);
        let api = linked(&stub.base_url);
        assert_eq!(
            toggle(&api, &device, id, true),
            refused("Your free deck slot is cooling down. It frees on 5 Nov 2026.")
        );
    }

    #[test]
    fn refuses_when_the_device_is_not_linked() {
        let stub = Stub::start(quiet);
        let device = device("sync-toggle-unlinked");
        let id = file_deck(&device, "111", false);
        let api = test_api(&stub.base_url, &[], &[]);
        assert_eq!(
            toggle(&api, &device, id, true),
            refused("Sign in from Settings before syncing decks.")
        );
        assert_eq!(stub.connections(), 0);
    }

    #[test]
    fn uses_the_sanitised_client_id_and_kind_for_a_limited_deck() {
        let stub = Stub::start(|_| {
            Reply::json(
                201,
                r#"{"tier":"supporter","limit":null,"used":1,"decks":[{"client_id":"limited_abc","enabled_at":"x","disabled_at":null,"frees_at":null}]}"#,
            )
        });
        let device = device("sync-toggle-limited");
        let id = file_deck(&device, "limited:abc", false);
        let api = linked(&stub.base_url);
        assert!(toggle(&api, &device, id, true).is_ok());
        let put = stub.requests().remove(0);
        assert_eq!(put.target, "/api/sync/decks/limited_abc");
        assert_eq!(body(&put)["kind"], json!("limited"));
    }

    #[test]
    fn turns_the_server_tier_refusal_into_its_message() {
        let stub = Stub::start(|_| Reply::json(422, r#"{"error":"limited_requires_supporter"}"#));
        let device = device("sync-toggle-tier");
        let id = file_deck(&device, "limited:abc", false);
        let api = linked(&stub.base_url);
        assert_eq!(
            toggle(&api, &device, id, true),
            refused("Syncing draft and sealed decks is a supporter feature.")
        );
    }

    #[test]
    fn refuses_an_archived_deck() {
        let stub = Stub::start(quiet);
        let device = device("sync-toggle-archived");
        let id = file_deck(&device, "111", false);
        write(
            &device,
            &format!("UPDATE decks SET archived_at = 1 WHERE id = {id}"),
        );
        let api = linked(&stub.base_url);
        assert_eq!(
            toggle(&api, &device, id, true),
            refused("This deck has been deleted on MTGO and is read-only.")
        );
        assert_eq!(stub.connections(), 0);
    }

    #[test]
    fn clears_deck_flags_and_stored_slots_on_sign_out() {
        let device = device("sync-sign-out");
        let id = file_deck(&device, "111", true);
        write(
            &device,
            "INSERT INTO settings (key, value) VALUES ('sync_slots', '{\"limit\":1,\"used\":1,\"decks\":[]}')",
        );
        let api = linked("http://127.0.0.1:1");
        crate::api::tokens::sign_out_and_forget_slots(&api.state, Some(&editor(&device)));
        assert!(!api.state.session().linked());
        assert!(!flag(&device, id));
        assert_eq!(
            store::sync::stored_slots(&reader(&device)).expect("slots"),
            None
        );
    }

    // ---- Matches and leagues (Plan Q task 11) -----------------------------

    /// A complete log match `token` started at `started_at`, played with the
    /// deck `mtgo_id`'s version: Qplayer against Qfoe, one game won.
    fn log_match(device: &Device, token: &str, started_at: i64, mtgo_id: &str) {
        write(
            device,
            &format!(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)
                     ON CONFLICT (username) DO NOTHING;
                 INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0)
                     ON CONFLICT (username) DO NOTHING;
                 INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source,
                     format, match_type, state, outcome, started_at, ended_at, last_activity_at,
                     revision, derived_version, deck_version_id)
                 VALUES ('{token}', NULL, (SELECT id FROM accounts WHERE username = 'Qplayer'),
                     (SELECT id FROM opponents WHERE username = 'Qfoe'), 'log', 'CMODERN',
                     'Constructed', 'complete', 'win', {started_at}, {started_at} + 60000,
                     {started_at} + 60000, 1, 1,
                     (SELECT v.id FROM deck_versions v JOIN decks d ON d.id = v.deck_id
                      WHERE d.mtgo_id = '{mtgo_id}'));
                 INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                     started_at, ended_at)
                 SELECT id, 1, 'win', 'win', 1, {started_at}, {started_at} + 60000
                 FROM matches WHERE token = '{token}';"
            ),
        );
    }

    fn match_row(device: &Device, token: &str) -> Option<store::sync::SyncedRow> {
        store::sync::synced_rows(&reader(device), SyncType::Match)
            .expect("rows")
            .remove(token)
    }

    /// A server that stores every uploaded blob and asks for the match
    /// type's dirty entries among `wanted` (what it does not hold yet).
    fn storing_matches(wanted: Vec<&'static str>) -> Stub {
        let manifest = move |request: &Recorded| {
            let found = body(request);
            (request.target == MANIFEST && found["type"] == "match").then(|| {
                let upload: Vec<Value> = found["dirty"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|entry| entry["client_id"].clone())
                    .filter(|id| wanted.iter().any(|want| id == want))
                    .collect();
                Reply::json(200, &json!({"upload": upload, "download": []}).to_string())
            })
        };
        Stub::start(move |request| {
            if request.target == BLOBS {
                let text = String::from_utf8_lossy(&request.body).to_string();
                let mut stored = Vec::new();
                for part in text.split("name=\"meta[").skip(1) {
                    if let Some(rest) = part.split("][client_id]\"\r\n\r\n").nth(1) {
                        stored.push(rest.split("\r\n").next().unwrap_or_default().to_string());
                    }
                }
                return Reply::json(200, &json!({"stored": stored, "rejected": []}).to_string());
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        })
    }

    // XS3 review: one match whose bundle cannot be built (a corrupt pulled
    // source) is skipped with a warning; the others still push.
    #[test]
    fn a_match_whose_bundle_cannot_be_built_does_not_block_the_others() {
        let stub = storing_matches(vec!["q-good", "q-bad"]);
        let device = device("sync-run-broken-match");
        file_deck(&device, "111", true);
        log_match(&device, "q-good", NOW - DAY, "111");
        log_match(&device, "q-bad", NOW - DAY, "111");
        write(
            &device,
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id');
             UPDATE matches SET source = 'imported', edited_at = 1,
                 account_id = (SELECT id FROM accounts WHERE platform = 'arena')
                 WHERE token = 'q-bad';
             INSERT INTO synced_sources (match_id, platform, payload)
                 SELECT id, 'arena', X'00FF' FROM matches WHERE token = 'q-bad';",
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(dirty_ids(&stub, "match"), vec!["q-good".to_string()]);
        let sent: Vec<String> = uploaded(&stub).into_iter().map(|(id, _)| id).collect();
        assert_eq!(sent, vec!["q-good".to_string()]);
        assert_eq!(last_error(&device), None);
        assert!(match_row(&device, "q-good").is_some_and(|row| row.hash.is_some()));
    }

    // 0.x "pushes a dirty match, carrying the right hash and sidecar, and
    // sets synced_hash once stored".
    #[test]
    fn pushes_a_dirty_match_with_its_hash_and_sidecar_and_records_it_once_stored() {
        let stub = storing_matches(vec!["q-match"]);
        let device = device("sync-run-match-push");
        file_deck(&device, "111", true);
        log_match(&device, "q-match", NOW - DAY, "111");
        let api = linked(&stub.base_url);
        let toasts = run(&api, &device);
        let entry = dirty_ids(&stub, "match");
        assert_eq!(entry, vec!["q-match".to_string()]);
        let dirty_hash = manifests(&stub, "match")[0]["dirty"][0]["hash"].clone();
        let sent = uploaded(&stub);
        let (client_id, bundle) = sent
            .iter()
            .find(|(client_id, _)| client_id == "q-match")
            .expect("uploaded");
        assert_eq!(client_id, "q-match");
        assert_eq!(json!(bundle_hash(&canonical_json(bundle))), dirty_hash);
        assert_eq!(bundle["match"]["deck_mtgo_id"], json!("111"));
        let body = String::from_utf8_lossy(
            &stub
                .requests()
                .into_iter()
                .find(|request| request.target == BLOBS)
                .expect("blobs")
                .body,
        )
        .to_string();
        assert!(body.contains("name=\"meta[0][sidecar][deck_client_id]\"\r\n\r\n111\r\n"));
        assert!(body.contains("name=\"meta[0][sidecar][outcome]\"\r\n\r\nwin\r\n"));
        let row = match_row(&device, "q-match").expect("row");
        assert_eq!(
            row.hash.as_ref(),
            dirty_hash.as_str().map(str::to_string).as_ref()
        );
        assert_eq!(
            toasts,
            vec![(
                "Synced with MyMTGO".to_string(),
                "1 match uploaded".to_string()
            )]
        );
        let state = store::sync::sync_state(&reader(&device), SyncType::Match)
            .expect("state")
            .expect("saved");
        assert_eq!(state.last_synced_at, Some(NOW));
    }

    // 0.x "self-heals a touched-but-unchanged match: no upload, but a fresh
    // synced_at".
    #[test]
    fn self_heals_a_touched_but_unchanged_match() {
        let stub = storing_matches(vec!["q-match"]);
        let device = device("sync-run-match-heal");
        file_deck(&device, "111", true);
        log_match(&device, "q-match", NOW - DAY, "111");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let uploads = || {
            stub.requests()
                .iter()
                .filter(|request| request.target == BLOBS)
                .count()
        };
        assert_eq!(uploads(), 1);
        write(&device, "UPDATE matches SET revision = revision + 1");
        let before = stub.requests().len();
        run(&api, &device);
        assert_eq!(uploads(), 1, "the same bytes are not sent again");
        let dirty: Vec<Value> = stub.requests()[before..]
            .iter()
            .filter(|request| request.target == MANIFEST)
            .map(body)
            .filter(|found| found["type"] == "match")
            .flat_map(|found| found["dirty"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(dirty.is_empty(), "rebuilt, found equal, not dirty");
    }

    // 0.x "pushes 30+ dirty matches oldest-first by started_at, each exactly
    // once, across more than one candidate-fetch chunk".
    #[test]
    fn pushes_30_plus_dirty_matches_oldest_first_each_exactly_once() {
        let device = device("sync-run-match-order");
        file_deck(&device, "111", true);
        let mut wanted: Vec<&'static str> = Vec::new();
        for index in 0..31i64 {
            // Tokens sort the other way round from their start times.
            let token: &'static str = Box::leak(format!("q-{:02}", 30 - index).into_boxed_str());
            log_match(&device, token, NOW - DAY + index * 1_000, "111");
            wanted.push(token);
        }
        let stub = storing_matches(wanted.clone());
        let api = linked(&stub.base_url);
        run(&api, &device);
        let order: Vec<String> = uploaded(&stub)
            .into_iter()
            .map(|(client_id, _)| client_id)
            .filter(|client_id| client_id.starts_with("q-"))
            .collect();
        let expected: Vec<String> = (0..31)
            .map(|index| format!("q-{:02}", 30 - index))
            .collect();
        assert_eq!(order, expected);
        let requests = stub
            .requests()
            .iter()
            .filter(|request| request.target == BLOBS)
            .count();
        assert_eq!(requests, 2, "20 blobs per request");
    }

    // 0.x "only offers matches and leagues of enabled decks as dirty or
    // known, decks regardless".
    #[test]
    fn only_offers_matches_of_enabled_decks_and_pushes_only_owned_ones() {
        let stub = storing_matches(vec!["q-on", "q-off", "q-imported"]);
        let device = device("sync-run-match-enabled");
        file_deck(&device, "111", true);
        file_deck(&device, "222", false);
        log_match(&device, "q-on", NOW - DAY, "111");
        log_match(&device, "q-off", NOW - DAY, "222");
        log_match(&device, "q-imported", NOW - DAY, "111");
        write(
            &device,
            "UPDATE matches SET source = 'imported' WHERE token = 'q-imported'",
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        let last = manifests(&stub, "match")
            .into_iter()
            .find(|found| found["last"] == true)
            .expect("final call");
        assert_eq!(last["known"], json!(["q-imported", "q-on"]));
        assert_eq!(dirty_ids(&stub, "match"), vec!["q-on".to_string()]);
        let sent: Vec<String> = uploaded(&stub)
            .into_iter()
            .map(|(client_id, _)| client_id)
            .collect();
        assert_eq!(
            sent,
            vec!["q-on".to_string()],
            "v1 pushes only rows it owns"
        );
    }

    /// A pulled match's bundle, built as another device built it.
    fn cloud_match(token: &str) -> Value {
        json!({
            "match": {
                "token": token, "mtgo_id": "285746550", "format": "CMODERN",
                "match_type": "Constructed", "result": "1-0", "outcome": "win",
                "games_won": 1, "games_lost": 0, "started_at": "2027-01-14T08:00:00Z",
                "ended_at": "2027-01-14T08:20:00Z", "state": "complete", "notes": null,
                "imported": false, "manual": false, "deck_mtgo_id": null,
                "deck_version_signature": null, "league_client_id": null,
                "tournament_mtgo_event_id": null, "tournament_round": null,
            },
            "games": [{"mtgo_id": "945500844", "won": true,
                "started_at": "2027-01-14T08:00:00Z", "ended_at": "2027-01-14T08:20:00Z",
                "turn_count": 7}],
            "players": [
                {"game": "945500844", "username": "Qfoe", "is_local": false, "on_play": false,
                 "starting_hand_size": 7, "mulligan_count": 0, "dice_roll": 3,
                 "deck_json": [], "opening_hand_json": null},
                {"game": "945500844", "username": "Qplayer", "is_local": true, "on_play": true,
                 "starting_hand_size": 7, "mulligan_count": 0, "dice_roll": 5,
                 "deck_json": [], "opening_hand_json": null},
            ],
            "timelines": [], "card_stats": [],
            "archetypes": [{"uuid": "q-uuid", "confidence": "1.00", "manual": false,
                "player_username": "Qfoe", "player_is_player": false}],
        })
    }

    fn pulling(sync_type: &'static str, client_id: &'static str, bundle: Value) -> Stub {
        let answer = blob(client_id, &bundle, true);
        let manifest = answering(sync_type, json!({"upload": [], "download": [client_id]}));
        Stub::start(move |request| {
            if request.target == FETCH && body(request)["type"] == sync_type {
                return Reply::json(200, &json!({"blobs": [answer.clone()]}).to_string());
            }
            manifest(request).unwrap_or_else(|| quiet(request))
        })
    }

    // Review Focus 3: a match another device played and reported is never
    // reported again from this one (0.x marks it submitted).
    #[test]
    fn a_pulled_complete_match_is_recorded_as_reported() {
        let stub = pulling("match", "q-cloud", cloud_match("q-cloud"));
        let device = device("sync-run-match-pull");
        let api = linked(&stub.base_url);
        let toasts = run(&api, &device);
        assert_eq!(
            last_pulls(),
            1,
            "a pull that imported wakes the cards worker"
        );
        let conn = reader(&device);
        let source: String = conn
            .query_row(
                "SELECT source FROM matches WHERE token = 'q-cloud'",
                [],
                |row| row.get(0),
            )
            .expect("pulled");
        assert_eq!(source, "imported");
        let status: String = conn
            .query_row(
                "SELECT status FROM outbox WHERE kind = 'match_report' AND subject = 'q-cloud'",
                [],
                |row| row.get(0),
            )
            .expect("a report row");
        assert_eq!(status, "sent");
        // Even with a player archetype stored, it is not reported again.
        write(
            &device,
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('aaaaaaaa-1111-4111-8111-111111111111', 'Qarchetype', 'CMODERN',
                     'downloaded', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES ((SELECT id FROM matches WHERE token = 'q-cloud'), 0,
                     last_insert_rowid(), 1.0);",
        );
        let reportable = store::api_reads::pending_report_count(&conn, NOW).expect("count");
        assert_eq!(reportable, 0, "nothing to report");
        let row = match_row(&device, "q-cloud").expect("row");
        assert_eq!(
            row.carry.as_deref(),
            Some("{}"),
            "nothing rides in the carry"
        );
        // Archetypes spec section 14: the opponent seat lands as a row, its
        // unknown uuid as a stub the next check fills.
        let opponent: String = conn
            .query_row(
                "SELECT a.uuid FROM match_archetypes x JOIN archetypes a ON a.id = x.archetype_id
                 JOIN matches m ON m.id = x.match_id
                 WHERE m.token = 'q-cloud' AND x.is_opponent = 1",
                [],
                |row| row.get(0),
            )
            .expect("the opponent seat");
        assert_eq!(opponent, "q-uuid");
        assert_eq!(
            toasts,
            vec![(
                "Synced with MyMTGO".to_string(),
                "1 match downloaded".to_string()
            )]
        );
    }

    fn cloud_league() -> Value {
        json!({
            "league": {
                "token": "q-cloud-run", "name": "Qcloud league", "format": "DDSK",
                "started_at": "2027-01-14T07:00:00Z", "joined_at": null, "dropped_at": null,
                "completed_at": null, "state": "active", "notes": "Qnote", "manual": false,
                "deck_change_detected": true, "kind": "draft", "set_code": "DSK",
                "mtgo_course_id": 7, "deck_mtgo_id": null, "deck_version_signature": null,
            },
            "drafts": [{"draft_token": "q-cloud-draft", "mtgo_draft_id": 1, "pod_token": null,
                "seat_count": 8, "seat_index": 2, "booster_catalog_id": null,
                "state": "finished", "pack_size": 14, "picks_expected": 42,
                "started_at": "2027-01-14T07:01:00Z", "ended_at": "2027-01-14T07:30:00Z",
                "picks": [{"ordinal": 1, "pack_number": 1, "pick_number": 1, "pack_id": null,
                    "direction": null, "cards_available": [5, 6], "picked_catalog_id": 5,
                    "picked_card_id": null, "reservations": [], "shown_at": null,
                    "deadline_at": null, "picked_at": null, "note": "Qpick"}]}],
            "snapshots": [{"source": "registered",
                "cards": [{"catalog_id": 5, "quantity": 1, "sideboard": false}],
                "signature": "q-sig", "captured_at": "2027-01-14T08:00:00Z",
                "match_token": "q-cloud"}],
        })
    }

    #[test]
    fn a_run_pulls_a_league_then_its_match_and_places_the_registered_deck() {
        let league = cloud_league();
        let mut game = cloud_match("q-cloud");
        game["match"]["league_client_id"] = json!("q-cloud-run_20270114T070000Z");
        let league_blob = blob("q-cloud-run_20270114T070000Z", &league, true);
        let match_blob = blob("q-cloud", &game, true);
        let stub = Stub::start(move |request| {
            let found = body(request);
            if request.target == FETCH {
                let blob = if found["type"] == "league" {
                    league_blob.clone()
                } else {
                    match_blob.clone()
                };
                return Reply::json(200, &json!({"blobs": [blob]}).to_string());
            }
            if request.target == MANIFEST && found["last"] == true {
                let download = match found["type"].as_str() {
                    Some("league") => json!(["q-cloud-run_20270114T070000Z"]),
                    Some("match") => json!(["q-cloud"]),
                    _ => json!([]),
                };
                return Reply::json(
                    200,
                    &json!({"upload": [], "download": download}).to_string(),
                );
            }
            quiet(request)
        });
        let device = device("sync-run-league-pull");
        let api = linked(&stub.base_url);
        run(&api, &device);
        let conn = reader(&device);
        let (source, linked_matches, picks): (String, i64, i64) = conn
            .query_row(
                "SELECT l.source,
                     (SELECT COUNT(*) FROM matches m WHERE m.league_id = l.id),
                     (SELECT COUNT(*) FROM draft_picks)
                 FROM leagues l",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("league");
        assert_eq!((source.as_str(), linked_matches, picks), ("imported", 1, 1));
        let snapshot: String = conn
            .query_row(
                "SELECT m.token FROM limited_deck_snapshots s JOIN matches m ON m.id = s.match_id",
                [],
                |row| row.get(0),
            )
            .expect("the registered deck found its match");
        assert_eq!(snapshot, "q-cloud");
        let carry = store::sync::synced_rows(&conn, SyncType::League).expect("rows")
            ["q-cloud-run_20270114T070000Z"]
            .carry
            .clone()
            .expect("carry");
        assert!(carry.contains("deck_change_detected"));
    }

    // ---- Pulled manual rows (Plan Q task 11 review fixes) -----------------

    /// The server's copy of one row: the bundle it hands out, whether it
    /// offers it for download, its `updated_at` (a bundle datetime) and the
    /// client ids of the uploads it kept.
    #[derive(Clone)]
    struct CloudCopy {
        bundle: Arc<Mutex<Value>>,
        offer: Arc<AtomicBool>,
        updated_at: Arc<Mutex<String>>,
        landed: Arc<Mutex<Vec<String>>>,
    }

    fn cloud_copy(bundle: Value, updated_at: &str) -> CloudCopy {
        CloudCopy {
            bundle: Arc::new(Mutex::new(bundle)),
            offer: Arc::new(AtomicBool::new(true)),
            updated_at: Arc::new(Mutex::new(updated_at.to_string())),
            landed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The `(client_id, updated_at)` of every blob of a multipart upload.
    fn upload_meta(request: &Recorded) -> Vec<(String, String)> {
        let text = String::from_utf8_lossy(&request.body).to_string();
        let mut found: Vec<(String, String, String)> = Vec::new();
        for part in text.split("name=\"meta[").skip(1) {
            let Some((index, rest)) = part.split_once("][") else {
                continue;
            };
            let Some((field, rest)) = rest.split_once("]\"\r\n\r\n") else {
                continue;
            };
            let value = rest.split("\r\n").next().unwrap_or_default().to_string();
            found.push((index.to_string(), field.to_string(), value));
        }
        let mut blobs = Vec::new();
        for (index, field, client_id) in &found {
            if field != "client_id" {
                continue;
            }
            let updated_at = found
                .iter()
                .find(|(other, name, _)| other == index && name == "updated_at")
                .map(|(_, _, value)| value.clone())
                .unwrap_or_default();
            blobs.push((client_id.clone(), updated_at));
        }
        blobs
    }

    /// A server holding `copy` as one row of `sync_type`, deciding by
    /// `updated_at` as the API does: the manifest asks for a dirty entry only
    /// when it is newer than the server's copy (`ManifestDiff`), and an
    /// older upload is dropped yet still reported stored
    /// (`UploadBlobsController`).
    fn cloud_row(sync_type: &'static str, client_id: &'static str, copy: &CloudCopy) -> Stub {
        let copy = copy.clone();
        Stub::start(move |request| {
            let found = body(request);
            match request.target.as_str() {
                FETCH if found["type"] == sync_type => {
                    let answer = blob(client_id, &lock(&copy.bundle), true);
                    Reply::json(200, &json!({"blobs": [answer]}).to_string())
                }
                MANIFEST if found["type"] == sync_type => {
                    let server_time = lock(&copy.updated_at).clone();
                    let upload: Vec<Value> = found["dirty"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|entry| {
                            entry["client_id"] != client_id
                                || entry["updated_at"].as_str().unwrap_or_default()
                                    > server_time.as_str()
                        })
                        .map(|entry| entry["client_id"].clone())
                        .collect();
                    let download = if found["last"] == true && copy.offer.load(Ordering::SeqCst) {
                        json!([client_id])
                    } else {
                        json!([])
                    };
                    Reply::json(
                        200,
                        &json!({"upload": upload, "download": download}).to_string(),
                    )
                }
                BLOBS => {
                    let mut stored = Vec::new();
                    for (id, updated_at) in upload_meta(request) {
                        let mut server_time = lock(&copy.updated_at);
                        if id == client_id && updated_at > *server_time {
                            *server_time = updated_at;
                            lock(&copy.landed).push(id.clone());
                        }
                        stored.push(id);
                    }
                    Reply::json(200, &json!({"stored": stored, "rejected": []}).to_string())
                }
                _ => quiet(request),
            }
        })
    }

    fn uploads_of(stub: &Stub, client_id: &str) -> usize {
        uploaded(stub)
            .into_iter()
            .filter(|(id, _)| id == client_id)
            .count()
    }

    /// A manual match as 0.x built it on another device: 0.x's own random
    /// `manual_` id and card stats v1 does not keep, so v1's rebuild of it
    /// never equals the server's copy.
    fn cloud_manual_match(token: &str, note: &str, won: bool) -> Value {
        let mut bundle = cloud_match(token);
        bundle["match"]["manual"] = json!(true);
        bundle["match"]["mtgo_id"] = json!("manual_Q0xRandomId");
        bundle["match"]["deck_mtgo_id"] = json!("111");
        bundle["match"]["deck_version_signature"] = json!("Qsig111");
        bundle["match"]["notes"] = json!(note);
        bundle["match"]["outcome"] = json!(if won { "win" } else { "loss" });
        bundle["games"][0]["mtgo_id"] = json!("q-manual-1");
        bundle["games"][0]["won"] = json!(won);
        for player in bundle["players"].as_array_mut().expect("players") {
            player["game"] = json!("q-manual-1");
        }
        bundle["card_stats"] = json!([{"game": "q-manual-1", "oracle_id": "q-oracle",
            "opponent": false, "quantity": 4, "kept": 1, "seen": 2}]);
        bundle
    }

    fn match_note_and_result(device: &Device, token: &str) -> (Option<String>, String) {
        reader(device)
            .query_row(
                "SELECT m.notes, g.result FROM matches m JOIN games g ON g.match_id = m.id
                 WHERE m.token = ?1",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("match")
    }

    // Review fix A (1) and (2): a pulled manual match is the server's copy
    // until it is edited here.
    #[test]
    fn a_pulled_manual_match_is_pushed_only_after_a_local_edit() {
        // The other device stamped its first build with the match's end.
        let copy = cloud_copy(
            cloud_manual_match("q-hand", "Qcloud", true),
            "2027-01-14T08:20:00Z",
        );
        let stub = cloud_row("match", "q-hand", &copy);
        let device = device("sync-run-manual-match-edit");
        file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let source: String = reader(&device)
            .query_row(
                "SELECT source FROM matches WHERE token = 'q-hand'",
                [],
                |row| row.get(0),
            )
            .expect("pulled");
        assert_eq!(source, "manual");
        copy.offer.store(false, Ordering::SeqCst);

        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-hand"), 0, "never pushed back unedited");
        assert!(!dirty_ids(&stub, "match").contains(&"q-hand".to_string()));
        assert_eq!(
            store::sync::pending_count(&reader(&device), SyncType::Match, NOW).expect("pending"),
            0
        );

        write(
            &device,
            "UPDATE matches SET notes = 'Qedited here', revision = revision + 1
             WHERE token = 'q-hand'",
        );
        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-hand"), 1, "a local edit is pushed");
        assert_eq!(
            *lock(&copy.landed),
            vec!["q-hand".to_string()],
            "and the server keeps it (newer than its copy)"
        );
    }

    // Review fix A (3): a re-pull replaces an unedited pulled manual match
    // with the server's copy; an edited one keeps this device's.
    #[test]
    fn a_re_pull_replaces_an_unedited_pulled_manual_match_with_the_server_copy() {
        // The other device stamped its first build with the match's end.
        let copy = cloud_copy(
            cloud_manual_match("q-hand", "Qcloud", true),
            "2027-01-14T08:20:00Z",
        );
        let stub = cloud_row("match", "q-hand", &copy);
        let device = device("sync-run-manual-match-repull");
        file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(
            match_note_and_result(&device, "q-hand"),
            (Some("Qcloud".to_string()), "win".to_string())
        );

        *lock(&copy.bundle) = cloud_manual_match("q-hand", "Qother device", false);
        *lock(&copy.updated_at) = bundle_datetime(NOW - DAY / 2);
        run(&api, &device);
        assert_eq!(
            match_note_and_result(&device, "q-hand"),
            (Some("Qother device".to_string()), "loss".to_string()),
            "the other device's edit lands"
        );
        let matches: i64 = reader(&device)
            .query_row("SELECT COUNT(*) FROM matches", [], |row| row.get(0))
            .expect("count");
        assert_eq!(matches, 1);
        copy.offer.store(false, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-hand"), 0, "still the server's copy");

        // Edited here: this device's copy wins a later pull (as decks do).
        write(
            &device,
            "UPDATE matches SET notes = 'Qedited here', revision = revision + 1
             WHERE token = 'q-hand'",
        );
        *lock(&copy.bundle) = cloud_manual_match("q-hand", "Qthird copy", true);
        copy.offer.store(true, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(
            match_note_and_result(&device, "q-hand").0.as_deref(),
            Some("Qedited here")
        );
    }

    const MANUAL_LEAGUE_ID: &str = "manual_QqQqQqQqQqQqQqQqQqQqQqQq_20270114T070000Z";

    /// A manual league as 0.x built it on another device; its `completed_at`
    /// is a column a v1 manual league does not keep, so v1's rebuild of it
    /// never equals the server's copy.
    fn cloud_manual_league(name: &str, note: &str) -> Value {
        json!({
            "league": {
                "token": "manual_QqQqQqQqQqQqQqQqQqQqQqQq", "name": name, "format": "CMODERN",
                "started_at": "2027-01-14T07:00:00Z", "joined_at": null, "dropped_at": null,
                "completed_at": "2027-01-14T09:00:00Z", "state": "complete", "notes": note,
                "manual": true, "deck_change_detected": false, "kind": "constructed",
                "set_code": null, "mtgo_course_id": null, "deck_mtgo_id": "111",
                "deck_version_signature": "Qsig111",
            },
            "drafts": [],
            "snapshots": [],
        })
    }

    fn league_name_and_note(device: &Device) -> (String, Option<String>) {
        reader(device)
            .query_row("SELECT name, notes FROM leagues", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("league")
    }

    // Review fix A (4): the same for a pulled manual league.
    #[test]
    fn a_pulled_manual_league_is_pushed_only_after_a_local_edit_and_replaced_until_then() {
        // The other device stamped its first build with the league's end.
        let copy = cloud_copy(
            cloud_manual_league("Qcloud league", "Qcloud"),
            "2027-01-14T09:00:00Z",
        );
        let stub = cloud_row("league", MANUAL_LEAGUE_ID, &copy);
        let device = device("sync-run-manual-league");
        file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        let source: String = reader(&device)
            .query_row("SELECT source FROM leagues", [], |row| row.get(0))
            .expect("pulled");
        assert_eq!(source, "manual");
        copy.offer.store(false, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, MANUAL_LEAGUE_ID),
            0,
            "never pushed unedited"
        );
        assert_eq!(
            store::sync::pending_count(&reader(&device), SyncType::League, NOW).expect("pending"),
            0
        );

        *lock(&copy.bundle) = cloud_manual_league("Qrenamed elsewhere", "Qother device");
        *lock(&copy.updated_at) = bundle_datetime(NOW - DAY / 2);
        copy.offer.store(true, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(
            league_name_and_note(&device),
            (
                "Qrenamed elsewhere".to_string(),
                Some("Qother device".to_string())
            ),
            "the other device's edit lands"
        );
        copy.offer.store(false, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(uploads_of(&stub, MANUAL_LEAGUE_ID), 0);

        write(&device, "UPDATE leagues SET notes = 'Qedited here'");
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, MANUAL_LEAGUE_ID),
            1,
            "a local edit is pushed"
        );
        assert_eq!(
            *lock(&copy.landed),
            vec![MANUAL_LEAGUE_ID.to_string()],
            "and the server keeps it (newer than its copy)"
        );
        *lock(&copy.bundle) = cloud_manual_league("Qthird", "Qthird copy");
        copy.offer.store(true, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(
            league_name_and_note(&device).1.as_deref(),
            Some("Qedited here"),
            "an edited league keeps this device's note"
        );
    }

    // Fix round 2: editing a member match of a pulled manual league never
    // pushes the league (its bundle holds no match data); editing the league
    // does.
    #[test]
    fn editing_a_member_match_never_pushes_a_pulled_manual_league() {
        // The other device stamped its first build with the league's end.
        let copy = cloud_copy(
            cloud_manual_league("Qcloud league", "Qcloud"),
            "2027-01-14T09:00:00Z",
        );
        let stub = cloud_row("league", MANUAL_LEAGUE_ID, &copy);
        let device = device("sync-run-manual-league-member");
        file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        copy.offer.store(false, Ordering::SeqCst);
        log_match(&device, "q-member", NOW - DAY, "111");
        write(
            &device,
            "UPDATE matches SET league_id = (SELECT id FROM leagues), league_linked = 1
             WHERE token = 'q-member'",
        );
        run(&api, &device);
        write(
            &device,
            "UPDATE matches SET notes = 'Qedited here', revision = revision + 1
             WHERE token = 'q-member'",
        );
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, MANUAL_LEAGUE_ID),
            0,
            "a member's edit is not the league's"
        );
        write(&device, "UPDATE leagues SET notes = 'Qedited here'");
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, MANUAL_LEAGUE_ID),
            1,
            "the league's own edit"
        );
        assert_eq!(*lock(&copy.landed), vec![MANUAL_LEAGUE_ID.to_string()]);
    }

    // Final review I1: a manual league 0.x synced is the server's copy (the
    // 0.x import marks it so); edited here, it is pushed with this device's
    // clock, so the server keeps it.
    #[test]
    fn an_edited_manual_league_0x_synced_lands_on_the_server() {
        let copy = cloud_copy(
            cloud_manual_league("Qmine", "Qnote"),
            "2027-01-14T09:00:00Z",
        );
        copy.offer.store(false, Ordering::SeqCst);
        let stub = cloud_row("league", MANUAL_LEAGUE_ID, &copy);
        let device = device("sync-run-0x-manual-league-edit");
        file_deck(&device, "111", true);
        write(
            &device,
            &format!(
                "INSERT INTO leagues (run_key, source, kind, state, token, name, format, deck_id,
                     started_at, created_at)
                 VALUES ('manual:1', 'manual', 'constructed', 'complete',
                     'manual_QqQqQqQqQqQqQqQqQqQqQqQq', 'Qmine', 'CMODERN',
                     (SELECT id FROM decks WHERE mtgo_id = '111'), {started}, 1);",
                started = 1_799_910_000_000i64,
            ),
        );
        let state = store::import_api::ImportedApiState {
            synced: vec![store::import_api::LegacySyncedRow {
                sync_type: SyncType::League,
                client_id: MANUAL_LEAGUE_ID.to_string(),
                hash: "q0x-hash".to_string(),
                synced_at_ms: Some(1),
            }],
            ..Default::default()
        };
        let mut conn = store::open_writer(&device.state.db_path).expect("writer");
        store::import_api::import_api_state(&mut conn, &state, NOW).expect("import");
        drop(conn);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, MANUAL_LEAGUE_ID),
            0,
            "never pushed unedited"
        );

        write(&device, "UPDATE leagues SET notes = 'Qedited here'");
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, MANUAL_LEAGUE_ID),
            1,
            "a local edit is pushed"
        );
        assert_eq!(*lock(&copy.landed), vec![MANUAL_LEAGUE_ID.to_string()]);
    }

    // Final review Minor 1: a pull the store refused is worded neutrally,
    // with its error.
    #[test]
    fn a_refused_pull_is_logged_with_its_error() {
        assert_eq!(
            pull_failed_line(SyncType::Match, "q-match", &"no such deck"),
            "match: q-match could not be imported (no such deck); left for the next run."
        );
    }

    // Final re-review: the `Err` arm of `pull_phase` itself. A pull the
    // store refuses is logged with its error, not counted as imported, does
    // not fail the run, and is offered again at the next run.
    #[test]
    fn a_pull_the_store_refuses_is_logged_and_left_for_the_next_run() {
        let match_blob = blob("q-refused", &cloud_match("q-refused"), true);
        let stub = Stub::start(move |request| {
            let found = body(request);
            if request.target == FETCH && found["type"] == "match" {
                return Reply::json(200, &json!({"blobs": [match_blob.clone()]}).to_string());
            }
            if request.target == MANIFEST && found["type"] == "match" && found["last"] == true {
                return Reply::json(
                    200,
                    &json!({"upload": [], "download": ["q-refused"]}).to_string(),
                );
            }
            quiet(request)
        });
        let device = device("sync-run-refused-pull");
        write(
            &device,
            "CREATE TRIGGER q_refuse BEFORE INSERT ON matches
             BEGIN SELECT RAISE(ABORT, 'q refused'); END;",
        );
        let api = linked(&stub.base_url);
        let toasts = run(&api, &device);
        let lines = activity(&api);
        assert!(
            lines.contains("match: q-refused could not be imported ("),
            "{lines}"
        );
        assert!(lines.contains("q refused"), "the store's error: {lines}");
        assert!(!lines.contains("downloaded 1 of 1"), "{lines}");
        assert!(lines.ends_with("Sync complete."), "{lines}");
        assert!(toasts.is_empty(), "nothing was imported: {toasts:?}");
        assert_eq!(
            last_pulls(),
            0,
            "a pull that imported nothing does not wake the cards worker"
        );
        assert_eq!(last_error(&device), None, "the run did not fail");
        assert!(synced(&device, "q-refused").is_none(), "offered again");
    }

    // ---- Carries (review fix B) -------------------------------------------

    /// A server holding `client_id` of `sync_type` with a carry blob whose
    /// hash is wrong; it asks for every dirty entry.
    fn bad_carry_server(sync_type: &'static str, client_id: &'static str, bundle: Value) -> Stub {
        let answer = blob(client_id, &bundle, false);
        Stub::start(move |request| {
            let found = body(request);
            match request.target.as_str() {
                FETCH if found["type"] == sync_type => {
                    Reply::json(200, &json!({"blobs": [answer.clone()]}).to_string())
                }
                MANIFEST if found["type"] == sync_type => Reply::json(
                    200,
                    &json!({"upload": [client_id], "download": []}).to_string(),
                ),
                _ => quiet(request),
            }
        })
    }

    fn assert_carry_left_for_the_next_run(
        sync_type: SyncType,
        client_id: &str,
        stub: &Stub,
        device: &Device,
    ) {
        let row = store::sync::synced_rows(&reader(device), sync_type)
            .expect("rows")
            .remove(client_id)
            .expect("row");
        assert_eq!(row.carry, None, "a wrong-hash blob leaves the carry unset");
        assert_eq!(uploads_of(stub, client_id), 0, "never built or pushed");
        assert!(!dirty_ids(stub, sync_type.as_str()).contains(&client_id.to_string()));
    }

    // Archetypes spec section 14: decks and matches carry nothing now, so a
    // row 0.x synced (a hash, no carry) is neither fetched first nor held
    // back for a carry: it is built from its rows and offered at once.
    #[test]
    fn decks_and_matches_never_wait_for_a_carry() {
        for (sync_type, client_id) in [(SyncType::Deck, "111"), (SyncType::Match, "q-match")] {
            let bundle = if sync_type == SyncType::Deck {
                deck_bundle_of("111", None)
            } else {
                cloud_match("q-match")
            };
            let stub = bad_carry_server(sync_type.as_str(), client_id, bundle);
            let device = device(&format!("sync-run-no-carry-{}", sync_type.as_str()));
            file_deck(&device, "111", true);
            if sync_type == SyncType::Match {
                log_match(&device, "q-match", NOW - DAY, "111");
            }
            write(
                &device,
                &format!(
                    "INSERT INTO synced_rows (type, client_id, hash, synced_at)
                     VALUES ('{}', '{client_id}', 'q0x-hash', 1)",
                    sync_type.as_str()
                ),
            );
            let api = linked(&stub.base_url);
            run(&api, &device);
            let fetched = stub
                .requests()
                .iter()
                .filter(|request| {
                    request.target == FETCH && body(request)["type"] == sync_type.as_str()
                })
                .count();
            assert_eq!(fetched, 0, "{sync_type:?}: no carry fetch");
            assert!(
                dirty_ids(&stub, sync_type.as_str()).contains(&client_id.to_string()),
                "{sync_type:?}: built and offered"
            );
        }
    }

    fn synced_row(changed_at: Option<i64>, pulled_key: Option<&str>) -> SyncedRow {
        SyncedRow {
            client_id: "ql".to_string(),
            hash: Some("h".to_string()),
            synced_at: None,
            built_hash: None,
            touch_key: None,
            changed_at,
            carry: None,
            pulled_key: pulled_key.map(str::to_string),
        }
    }

    // An edited pulled row takes the clock at its first build only; losing
    // its build hash to a carry fetch does not move the time again, unless
    // the row itself moved since that build (#314 row 6).
    #[test]
    fn a_carry_fetch_does_not_move_an_edited_pulled_rows_time_again() {
        let pulled = synced_row(None, Some("k"));
        assert_eq!(
            first_build_time(Some(&pulled), "k2", 10, false, 99),
            99,
            "first build"
        );
        let mut built_before = synced_row(Some(50), Some("k"));
        built_before.touch_key = Some("k2".to_string());
        assert_eq!(
            first_build_time(Some(&built_before), "k2", 10, false, 99),
            50
        );
        assert_eq!(
            first_build_time(Some(&built_before), "k3", 10, false, 99),
            99,
            "edited since that build"
        );
        let synced_by_0x = synced_row(None, None);
        assert_eq!(
            first_build_time(Some(&synced_by_0x), "k2", 10, false, 99),
            10
        );
        assert_eq!(first_build_time(None, "k2", 10, false, 99), 10);
        // League self-management spec 6.2: a player edit's first build takes
        // the run's time, so the server's older copy loses.
        assert_eq!(first_build_time(None, "k2", 10, true, 99), 99);
        assert_eq!(
            first_build_time(Some(&synced_by_0x), "k2", 10, true, 99),
            99
        );
    }

    // League self-management spec 6.2: an imported copy is never pushed
    // until the player edits it; then it is, with the run's time.
    #[test]
    fn an_edited_imported_match_is_pushed_with_the_runs_time() {
        let mut bundle = cloud_match("q-copy");
        bundle["match"]["deck_mtgo_id"] = json!("111");
        bundle["match"]["deck_version_signature"] = json!("Qsig111");
        let copy = cloud_copy(bundle, "2027-01-14T08:20:00Z");
        let stub = cloud_row("match", "q-copy", &copy);
        let device = device("sync-run-edited-copy");
        file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        copy.offer.store(false, Ordering::SeqCst);
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, "q-copy"),
            0,
            "an unedited copy is not sent"
        );

        write(
            &device,
            &format!(
                "UPDATE matches SET hidden_at = {NOW}, edited_at = {NOW} WHERE token = 'q-copy'"
            ),
        );
        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-copy"), 1);
        assert_eq!(lock(&copy.landed).as_slice(), ["q-copy".to_string()]);
        let (_, sent) = uploaded(&stub)
            .into_iter()
            .find(|(id, _)| id == "q-copy")
            .expect("sent");
        assert_eq!(sent["match"]["hidden_at"], json!(NOW));
        assert_eq!(sent["match"]["edited_at"], json!(NOW));
    }

    // #314 row 6: a league built here, then marked synced (the 0.x import)
    // and edited before its carry arrives. The edit's build takes the clock,
    // so the server keeps it over its own older copy.
    #[test]
    fn an_edit_before_a_carry_fetch_takes_the_clock_and_lands() {
        let copy = cloud_copy(
            cloud_manual_league("Qmine", "Qnote"),
            "2027-01-14T09:00:00Z",
        );
        copy.offer.store(false, Ordering::SeqCst);
        let stub = cloud_row("league", MANUAL_LEAGUE_ID, &copy);
        let device = device("sync-run-edit-before-carry");
        file_deck(&device, "111", true);
        write(
            &device,
            &format!(
                "INSERT INTO leagues (run_key, source, kind, state, token, name, format, deck_id,
                     started_at, created_at)
                 VALUES ('manual:1', 'manual', 'constructed', 'complete',
                     'manual_QqQqQqQqQqQqQqQqQqQqQqQq', 'Qmine', 'CMODERN',
                     (SELECT id FROM decks WHERE mtgo_id = '111'), {started}, 1);
                 INSERT INTO synced_rows (type, client_id, hash, synced_at, built_hash,
                     touch_key, changed_at, pulled_key)
                 VALUES ('league', '{MANUAL_LEAGUE_ID}', 'q0x-hash', 1, 'qbuilt', 'qkey-then',
                     {started}, 'qkey-pulled');
                 UPDATE leagues SET notes = 'Qedited here';",
                started = 1_799_910_000_000i64,
            ),
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        let row = store::sync::synced_rows(&reader(&device), SyncType::League)
            .expect("rows")
            .remove(MANUAL_LEAGUE_ID)
            .expect("row");
        assert_eq!(row.changed_at, Some(NOW), "the edit's time");
        assert_eq!(
            *lock(&copy.landed),
            vec![MANUAL_LEAGUE_ID.to_string()],
            "newer than the server's copy"
        );
    }

    // the API: a carry the server does not give back (a deck switched off
    // there, a blob missing on its disk) is "ask again later": the row's
    // carry stays unset for a bounded number of runs, then settles as `{}`.
    #[test]
    fn a_withheld_league_carry_is_asked_again_for_a_bounded_number_of_runs() {
        let stub = Stub::start(quiet);
        let device = device("sync-run-withheld-carry");
        file_deck(&device, "111", true);
        write(
            &device,
            &format!(
                "INSERT INTO leagues (run_key, source, kind, state, token, name, format, deck_id,
                     started_at, created_at)
                 VALUES ('manual:1', 'manual', 'constructed', 'complete',
                     'manual_QqQqQqQqQqQqQqQqQqQqQqQq', 'Qmine', 'CMODERN',
                     (SELECT id FROM decks WHERE mtgo_id = '111'), {started}, 1);
                 INSERT INTO synced_rows (type, client_id, hash, synced_at)
                 VALUES ('league', '{MANUAL_LEAGUE_ID}', 'q0x-hash', 1);",
                started = 1_799_910_000_000i64,
            ),
        );
        let api = linked(&stub.base_url);
        let carry = || {
            store::sync::synced_rows(&reader(&device), SyncType::League)
                .expect("rows")
                .remove(MANUAL_LEAGUE_ID)
                .expect("row")
                .carry
        };
        // Many quick runs (all at NOW) do not settle it.
        for run_number in 1..=MAX_CARRY_ASKS + 2 {
            run(&api, &device);
            assert_eq!(carry(), None, "run {run_number}: asked again later");
        }
        // The first ask is more than a day old, and it has been asked enough.
        write(
            &device,
            &format!(
                "UPDATE synced_rows SET carry_first_ask_at = {}",
                NOW - MIN_CARRY_ASK_AGE_MS
            ),
        );
        run(&api, &device);
        assert_eq!(carry().as_deref(), Some("{}"), "given up after a day");
    }

    #[test]
    fn a_wrong_hash_league_carry_is_retried_and_the_league_is_not_pushed() {
        let stub = bad_carry_server(
            "league",
            MANUAL_LEAGUE_ID,
            cloud_manual_league("Qmine", "Qnote"),
        );
        let device = device("sync-run-bad-league-carry");
        file_deck(&device, "111", true);
        write(
            &device,
            &format!(
                "INSERT INTO leagues (run_key, source, kind, state, token, name, format, deck_id,
                     started_at, created_at)
                 VALUES ('manual:1', 'manual', 'constructed', 'complete',
                     'manual_QqQqQqQqQqQqQqQqQqQqQqQq', 'Qmine', 'CMODERN',
                     (SELECT id FROM decks WHERE mtgo_id = '111'), {started}, 1);
                 INSERT INTO synced_rows (type, client_id, hash, synced_at)
                 VALUES ('league', '{MANUAL_LEAGUE_ID}', 'q0x-hash', 1);",
                started = 1_799_910_000_000i64,
            ),
        );
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_carry_left_for_the_next_run(SyncType::League, MANUAL_LEAGUE_ID, &stub, &device);
    }

    // League self-management spec 6.3: another device's newer edit lands
    // on the log match this device played, and is not sent back.
    #[test]
    fn a_pulled_edit_lands_on_a_log_match_and_is_not_echoed() {
        let device = device("sync-run-pulled-edit");
        file_deck(&device, "111", true);
        log_match(&device, "q-match", NOW - DAY, "111");
        let copy = cloud_copy(json!({}), "2027-01-01T00:00:00Z");
        copy.offer.store(false, Ordering::SeqCst);
        let stub = cloud_row("match", "q-match", &copy);
        let api = linked(&stub.base_url);
        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-match"), 1);
        let (_, sent) = uploaded(&stub)
            .into_iter()
            .find(|(id, _)| id == "q-match")
            .expect("sent");
        let mut edited = sent;
        edited["match"]["hidden_at"] = json!(NOW + 1_000);
        edited["match"]["edited_at"] = json!(NOW + 1_000);
        *lock(&copy.bundle) = edited;
        *lock(&copy.updated_at) = "2027-01-15T09:00:00Z".to_string();
        copy.offer.store(true, Ordering::SeqCst);
        run(&api, &device);
        copy.offer.store(false, Ordering::SeqCst);
        let hidden: Option<i64> = reader(&device)
            .query_row(
                "SELECT hidden_at FROM matches WHERE token = 'q-match'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        assert_eq!(hidden, Some(NOW + 1_000));
        run(&api, &device);
        assert_eq!(
            uploads_of(&stub, "q-match"),
            1,
            "the pulled edit is not sent back"
        );
    }

    // Spec 6.3: an older copy pulled over a newer local edit (0.x sent its
    // copy again) leaves the local fields and sends them again.
    #[test]
    fn a_newer_local_edit_is_sent_again_after_an_older_pull() {
        let mut bundle = cloud_match("q-copy");
        bundle["match"]["deck_mtgo_id"] = json!("111");
        bundle["match"]["deck_version_signature"] = json!("Qsig111");
        let original = bundle.clone();
        let copy = cloud_copy(bundle, "2027-01-14T08:20:00Z");
        let stub = cloud_row("match", "q-copy", &copy);
        let device = device("sync-run-local-edit-wins");
        file_deck(&device, "111", true);
        let api = linked(&stub.base_url);
        run(&api, &device);
        copy.offer.store(false, Ordering::SeqCst);
        let at = NOW - 1_000;
        write(
            &device,
            &format!(
                "UPDATE matches SET hidden_at = {at}, edited_at = {at} WHERE token = 'q-copy'"
            ),
        );
        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-copy"), 1, "the edit is sent");

        *lock(&copy.bundle) = original;
        *lock(&copy.updated_at) = "2027-01-15T07:00:00Z".to_string();
        copy.offer.store(true, Ordering::SeqCst);
        run(&api, &device);
        copy.offer.store(false, Ordering::SeqCst);
        let hidden: Option<i64> = reader(&device)
            .query_row(
                "SELECT hidden_at FROM matches WHERE token = 'q-copy'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        assert_eq!(hidden, Some(at), "the newer local edit stays");
        run(&api, &device);
        assert_eq!(uploads_of(&stub, "q-copy"), 2, "sent again");
        assert_eq!(lock(&copy.landed).len(), 2);
    }

    // ---- Every platform through one cloud (cross-device sync, XS9) --------

    use crate::api::fake_cloud::{FakeCloud, StoredAs, multipart_fields};

    /// `linked` with this device id, so two devices tell the server apart.
    fn linked_as(base_url: &str, device_id: &str) -> TestApi {
        let key_valid = (NOW + DAY).to_string();
        let token_valid = (NOW + 30 * DAY).to_string();
        let device_row = format!("\"{device_id}\"");
        let rows = [
            ("api_device_id", device_row.as_str()),
            ("api_key_expires_at", key_valid.as_str()),
            ("api_token_expires_at", token_valid.as_str()),
        ];
        test_api(
            base_url,
            &rows,
            &[
                ("api_key", "qdevice-key"),
                (ACCESS_TOKEN, "qaccess"),
                (REFRESH_TOKEN, "qrefresh"),
            ],
        )
    }

    /// The tracker tests' clock for the fixtures (their season and gates).
    const SEED_NOW: i64 = 1_793_720_000_000;

    /// The Endstep ranked fixture's capture files, as the inbox import reads
    /// them.
    fn endstep_fixture_captures() -> Vec<store::endstep_captures::NewCapture> {
        use sha2::{Digest, Sha256};
        let folder = fixture_tools::fixtures_dir().join("endstep/ranked-modern-bo3");
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&folder)
            .expect("the Endstep fixture folder")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        paths.sort();
        let mut captures = Vec::new();
        for path in paths {
            let bytes = std::fs::read(&path).expect("a capture");
            let text = String::from_utf8(bytes.clone()).expect("UTF-8");
            let parsed = endstep_capture::parse_capture(&text).expect("a capture file");
            let file_hash: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            captures.push(store::endstep_captures::NewCapture {
                header: parsed.header,
                frame_count: parsed.frame_count,
                text,
                file_hash,
            });
        }
        captures
    }

    /// Arena's sealed fixture (one course run of six limited matches, with
    /// the course deck) and the Endstep ranked fixture (a three game match
    /// and its deck), read into the device's database by a tracker of their
    /// own, as `arena-seed` and `endstep-seed` do. The device's tracker has
    /// Arena and Endstep off, so it leaves them alone.
    fn seed_arena_and_endstep(device: &Device, label: &str) -> TestDir {
        let folder = TestDir::new(label);
        folder.write(
            "arena/Player.log",
            &fixture_tools::arena_fixture("sealed-direct-fast-login").bytes,
        );
        let config = tracker::TrackerConfig {
            db_path: device.state.db_path.clone(),
            timezone: Tz::UTC,
            os_timezone: None,
            log_overrides: vec![folder.join("logs")],
            arena_log_overrides: vec![folder.join("arena/Player.log")],
            mtgo_data_folder: Some(folder.join("mtgo-data")),
            replay_day_anchor: None,
            watching: true,
            arena_enabled: true,
            endstep_enabled: true,
        };
        let mut engine = tracker::Engine::open(
            config,
            mtgo_log::Env::current(),
            Arc::default(),
            Vec::new(),
            SEED_NOW,
        )
        .expect("open a seeding tracker");
        engine
            .store_endstep_captures(endstep_fixture_captures(), SEED_NOW)
            .expect("store the captures");
        let mut now = SEED_NOW;
        for _ in 0..10_000 {
            let tick = engine.tick(now).expect("tick");
            now += 1;
            if !tick.more {
                break;
            }
        }
        for _ in 0..3 {
            now += tracker::engine::RESCAN_EVERY_MS;
            engine.tick(now).expect("tick");
        }
        engine.finish().expect("finish");
        folder
    }

    fn ids(device: &Device, sql: &str) -> Vec<i64> {
        let conn = reader(device);
        let mut statement = conn.prepare(sql).expect("sql");
        statement
            .query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn lines(device: &Device, sql: &str) -> Vec<String> {
        let conn = reader(device);
        let mut statement = conn.prepare(sql).expect("sql");
        statement
            .query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn pending(device: &Device) -> [u32; 3] {
        let conn = reader(device);
        [SyncType::Deck, SyncType::League, SyncType::Match]
            .map(|sync_type| store::sync::pending_count(&conn, sync_type, NOW).expect("pending"))
    }

    /// Name rows for the Arena cards a replay lacks, on both devices (the
    /// card catalog comes from the API, which no test reaches).
    fn add_arena_cards(devices: [&Device; 2], grp_ids: &[u32]) {
        for device in devices {
            let conn = store::open_writer(&device.state.db_path).expect("writer");
            for grp_id in grp_ids {
                conn.execute(
                    "INSERT OR IGNORE INTO cards (catalog_id, name, image, local_image)
                     VALUES (?1, ?2, ?3, ?4)",
                    store::params![
                        cards::ARENA_ID_OFFSET + grp_id,
                        format!("Qcard {grp_id}"),
                        format!("https://cards.example/{grp_id}.jpg"),
                        format!("{grp_id}.jpg"),
                    ],
                )
                .expect("card");
            }
        }
    }

    /// Device A (`here`) seeded with Arena's sealed fixture and the Endstep
    /// fixture, both its decks' slots on, and device B (`there`) fresh,
    /// both linked to one fake cloud.
    struct TwoDevices {
        cloud: FakeCloud,
        here: Device,
        there: Device,
        here_api: TestApi,
        there_api: TestApi,
        _seed: TestDir,
    }

    fn two_devices(label: &str) -> TwoDevices {
        let cloud = FakeCloud::start();
        let here = device(&format!("{label}-here"));
        let there = device(&format!("{label}-there"));
        let seed = seed_arena_and_endstep(&here, &format!("{label}-seed"));
        assert_eq!(
            lines(&here, PLATFORM_COUNTS),
            vec!["arena 6".to_string(), "endstep 1".to_string()],
            "the seeded matches"
        );
        let here_api = linked_as(&cloud.stub.base_url, "qdevice-here");
        let there_api = linked_as(&cloud.stub.base_url, "qdevice-there");
        let decks = ids(&here, "SELECT id FROM decks ORDER BY id");
        assert_eq!(decks.len(), 2, "the course deck and the Endstep deck");
        for deck_id in decks {
            toggle(&here_api, &here, deck_id, true).expect("a slot");
        }
        TwoDevices {
            cloud,
            here,
            there,
            here_api,
            there_api,
            _seed: seed,
        }
    }

    const PLATFORM_COUNTS: &str = "SELECT a.platform || ' ' || COUNT(*) FROM matches m
                                   JOIN accounts a ON a.id = m.account_id
                                   GROUP BY a.platform ORDER BY a.platform";

    fn run_summaries(device: &Device) -> Vec<String> {
        store::arena_run_sync::arena_run_summaries(&reader(device))
            .expect("runs")
            .into_iter()
            .map(|run| format!("{} {}-{} {}", run.name, run.wins, run.losses, run.state))
            .collect()
    }

    const SEALED_RUN: &str = "Arena Direct · EOE Sealed 5-0 active";

    /// What a re-import must leave alone: the runs, the synced hashes, each
    /// match's revision and edit, and the derived and source rows.
    fn fingerprint(device: &Device) -> Vec<String> {
        let mut all = run_summaries(device);
        for sql in [
            "SELECT type || ' ' || client_id || ' ' || COALESCE(hash, '') FROM synced_rows
             ORDER BY type, client_id",
            "SELECT token || ' ' || revision || ' ' || COALESCE(edited_at, '') FROM matches
             ORDER BY token",
            "SELECT 'game_cards ' || COUNT(*) FROM game_cards",
            "SELECT 'synced_sources ' || COUNT(*) FROM synced_sources",
            "SELECT 'decks ' || COUNT(*) FROM decks",
        ] {
            all.extend(lines(device, sql));
        }
        all
    }

    /// A pulled Arena run lands in the same sync as its matches, though on a
    /// new device the run is pulled before the matches that bring its Arena
    /// account.
    #[test]
    fn a_pulled_arena_run_lands_in_the_same_sync_as_its_matches() {
        let devices = two_devices("sync-run-same-sync");
        run(&devices.here_api, &devices.here);
        run(&devices.there_api, &devices.there);
        assert_eq!(run_summaries(&devices.there), vec![SEALED_RUN.to_string()]);
        let synced = lines(
            &devices.there,
            "SELECT client_id FROM synced_rows WHERE type = 'league' AND hash IS NOT NULL",
        );
        assert_eq!(synced.len(), 1, "the run is recorded as synced: {synced:?}");
    }

    // ---- Parked pulls (#830) -------------------------------------------------

    /// The client ids each fetch of `sync_type` asked for since request
    /// `from`.
    fn fetched_ids(cloud: &FakeCloud, sync_type: &str, from: usize) -> Vec<String> {
        cloud.stub.requests()[from..]
            .iter()
            .filter(|request| request.target == FETCH)
            .map(body)
            .filter(|body| body["type"] == json!(sync_type))
            .flat_map(|body| {
                body["client_ids"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
            })
            .collect()
    }

    /// A synced row's parked reason and hash, `-` for none.
    fn parked_of(device: &Device, sync_type: &str, client_id: &str) -> Vec<String> {
        lines(
            device,
            &format!(
                "SELECT COALESCE(parked_reason, '-') || ' ' || COALESCE(parked_hash, '-')
                     || ' ' || COALESCE(hash, '-')
                 FROM synced_rows WHERE type = '{sync_type}' AND client_id = '{client_id}'"
            ),
        )
    }

    fn cloud_hash(cloud: &FakeCloud, sync_type: &str, client_id: &str) -> String {
        let held = cloud.state.lock().expect("cloud");
        held.rows
            .get(&(sync_type.to_string(), client_id.to_string()))
            .expect("the cloud holds the row")
            .hash
            .clone()
    }

    /// Put a row first in the cloud's store order, so the fake cursor's
    /// one-row lag never offers it again on its own.
    fn store_first(cloud: &FakeCloud, sync_type: &str, client_id: &str) {
        let mut held = cloud.state.lock().expect("cloud");
        held.rows
            .get_mut(&(sync_type.to_string(), client_id.to_string()))
            .expect("the cloud holds the row")
            .stored = 0;
    }

    /// #830: an Arena run pulled on a device with no Arena account is parked
    /// with the server's hash, not fetched again while nothing changed,
    /// fetched again (and parked again) when its copy changes, and applied
    /// once an Arena account exists here.
    #[test]
    fn a_run_with_no_arena_account_is_parked_until_something_changes() {
        let devices = two_devices("sync-run-parked");
        run(&devices.here_api, &devices.here);
        // Only the run reaches device B: no match brings it an Arena account.
        lock(&devices.cloud.state)
            .rows
            .retain(|(kind, _), _| kind != "match");
        let (client_id, _) = sealed_run_upload(&devices.here);
        run(&devices.there_api, &devices.there);
        assert!(run_summaries(&devices.there).is_empty());
        let first_hash = cloud_hash(&devices.cloud, "league", &client_id);
        assert_eq!(
            parked_of(&devices.there, "league", &client_id),
            vec![format!("no_arena_account {first_hash} -")]
        );

        // Nothing changed: not fetched again.
        let from = devices.cloud.stub.requests().len();
        run(&devices.there_api, &devices.there);
        assert_eq!(
            fetched_ids(&devices.cloud, "league", from),
            Vec::<String>::new()
        );

        // A new copy on the server: fetched again, parked with its hash.
        course_read_up_to(&devices.here, 4);
        let (_, row) = sealed_run_upload(&devices.here);
        devices
            .cloud
            .store_as("qdevice-here", "league", &client_id, row);
        let second_hash = cloud_hash(&devices.cloud, "league", &client_id);
        assert_ne!(second_hash, first_hash);
        let from = devices.cloud.stub.requests().len();
        run(&devices.there_api, &devices.there);
        assert_eq!(
            fetched_ids(&devices.cloud, "league", from),
            vec![client_id.clone()]
        );
        assert_eq!(
            parked_of(&devices.there, "league", &client_id),
            vec![format!("no_arena_account {second_hash} -")]
        );

        // An Arena account here now: the parked run is fetched and applied.
        store_first(&devices.cloud, "league", &client_id);
        write(
            &devices.there,
            "INSERT INTO accounts (username, first_seen_at, platform, tracked)
             VALUES ('Qarena#00001', 0, 'arena', 1);",
        );
        let from = devices.cloud.stub.requests().len();
        run(&devices.there_api, &devices.there);
        assert_eq!(
            fetched_ids(&devices.cloud, "league", from),
            vec![client_id.clone()]
        );
        // The run, with the pulled course record (its matches are not here).
        assert_eq!(
            run_summaries(&devices.there),
            vec!["Arena Direct · EOE Sealed 0-0 active".to_string()]
        );
        assert_eq!(
            lines(
                &devices.there,
                &format!(
                    "SELECT course_wins || '-' || course_losses FROM leagues
                     WHERE sync_client_id = '{client_id}'"
                )
            ),
            vec!["4-0".to_string()]
        );
        assert_eq!(
            parked_of(&devices.there, "league", &client_id),
            vec![format!("- - {second_hash}")]
        );
    }

    /// #830 review: a parked run whose hold is gone is only left out of
    /// `known`, never forced into the download, so a run the server no
    /// longer holds is not asked for again at every sync.
    #[test]
    fn a_ready_parked_run_the_server_no_longer_holds_is_not_fetched() {
        let devices = two_devices("sync-run-parked-gone");
        run(&devices.here_api, &devices.here);
        lock(&devices.cloud.state)
            .rows
            .retain(|(kind, _), _| kind != "match");
        let (client_id, _) = sealed_run_upload(&devices.here);
        run(&devices.there_api, &devices.there);
        assert_eq!(
            parked_of(&devices.there, "league", &client_id),
            vec![format!(
                "no_arena_account {} -",
                cloud_hash(&devices.cloud, "league", &client_id)
            )]
        );
        // The run is gone from the server, and an Arena account is here now.
        lock(&devices.cloud.state)
            .rows
            .remove(&("league".to_string(), client_id.clone()));
        write(
            &devices.there,
            "INSERT INTO accounts (username, first_seen_at, platform, tracked)
             VALUES ('Qarena#00001', 0, 'arena', 1);",
        );
        let from = devices.cloud.stub.requests().len();
        run(&devices.there_api, &devices.there);
        run(&devices.there_api, &devices.there);
        assert_eq!(
            fetched_ids(&devices.cloud, "league", from),
            Vec::<String>::new()
        );
    }

    /// #830: an Arena match whose copy came without its source is parked
    /// with the server's hash, not fetched again while nothing changed, and
    /// fetched and stored once the server's copy changes.
    #[test]
    fn a_match_without_its_source_is_parked_until_its_copy_changes() {
        use sha2::{Digest, Sha256};
        let devices = two_devices("sync-match-parked");
        run(&devices.here_api, &devices.here);
        // The server holds one Arena match's copy without its source.
        let (client_id, whole) = {
            let held = lock(&devices.cloud.state);
            let ((_, client_id), row) = held
                .rows
                .iter()
                .find(|((kind, _), row)| {
                    kind == "match" && row.platform.as_deref() == Some("arena")
                })
                .expect("an Arena match");
            (client_id.clone(), row.clone())
        };
        let mut bundle: Value = serde_json::from_str(&whole.canonical).expect("JSON");
        assert!(
            bundle
                .as_object_mut()
                .expect("object")
                .remove("sources")
                .is_some()
        );
        let broken = canonical_json(&bundle);
        {
            let mut held = lock(&devices.cloud.state);
            let row = held
                .rows
                .get_mut(&("match".to_string(), client_id.clone()))
                .expect("the match");
            row.hash = Sha256::digest(broken.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            row.canonical = broken;
            row.stored = 0;
        }
        let broken_hash = cloud_hash(&devices.cloud, "match", &client_id);
        run(&devices.there_api, &devices.there);
        let stored = format!("SELECT COUNT(*) FROM matches WHERE token = '{client_id}'");
        assert_eq!(ids(&devices.there, &stored), vec![0]);
        assert_eq!(
            parked_of(&devices.there, "match", &client_id),
            vec![format!("no_sources {broken_hash} -")]
        );

        // Nothing changed: not fetched again.
        let from = devices.cloud.stub.requests().len();
        run(&devices.there_api, &devices.there);
        assert!(
            !fetched_ids(&devices.cloud, "match", from).contains(&client_id),
            "a parked match is not fetched again"
        );

        // The device that played it sends the whole copy: fetched and stored.
        devices.cloud.store_as(
            "qdevice-here",
            "match",
            &client_id,
            StoredAs {
                canonical: whole.canonical.clone(),
                updated_at: whole.updated_at.clone(),
                platform: whole.platform.clone(),
                deck_client_id: whole.deck_client_id.clone(),
            },
        );
        let from = devices.cloud.stub.requests().len();
        run(&devices.there_api, &devices.there);
        assert!(fetched_ids(&devices.cloud, "match", from).contains(&client_id));
        assert_eq!(ids(&devices.there, &stored), vec![1]);
        assert_eq!(
            parked_of(&devices.there, "match", &client_id),
            vec![format!("- - {}", whole.hash)]
        );
    }

    // ---- Arena run ordering (spec section 8) and the platforms gate --------

    /// Device `device` read the sealed course only up to its last `wins`-win
    /// listing: every later observation is set aside (any set aside before
    /// comes back first), and the runs are folded again.
    fn course_read_up_to(device: &Device, wins: u32) {
        let mut conn = store::open_writer(&device.state.db_path).expect("writer");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS set_aside AS SELECT * FROM arena_course_observations
                 WHERE 0;
             INSERT INTO arena_course_observations SELECT * FROM set_aside;
             DELETE FROM set_aside;",
        )
        .expect("restore");
        let cutoff: i64 = conn
            .query_row(
                "SELECT MAX(observed_at) FROM arena_course_observations
                 WHERE event_name LIKE '%Sealed%' AND wins = ?1",
                [wins],
                |row| row.get(0),
            )
            .expect("a listing at that record");
        conn.execute(
            "INSERT INTO set_aside SELECT * FROM arena_course_observations WHERE observed_at > ?1",
            [cutoff],
        )
        .expect("set aside");
        conn.execute(
            "DELETE FROM arena_course_observations WHERE observed_at > ?1",
            [cutoff],
        )
        .expect("delete");
        let tx = conn.transaction().expect("tx");
        store::arena_run_sync::sync_all_arena_runs(&tx, NOW).expect("fold");
        tx.commit().expect("commit");
    }

    /// The sealed run's client id and the row another device would upload
    /// for the run as `device` holds it now.
    fn sealed_run_upload(device: &Device) -> (String, StoredAs) {
        let conn = reader(device);
        let (league_id, client_id): (i64, String) = conn
            .query_row(
                "SELECT id, sync_client_id FROM leagues
                 WHERE run_key LIKE 'arena:%' AND name LIKE '%Sealed%'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the sealed run");
        let input = store::sync::arena_run_bundle_input(&conn, league_id)
            .expect("input")
            .expect("a course run");
        let bundle = arena_run_bundle(&input, None);
        let sidecar = league_sidecar(&bundle);
        let row = StoredAs {
            canonical: canonical_json(&bundle),
            updated_at: bundle_datetime(arena_run_changed_at(&bundle).expect("an Arena run")),
            platform: Some("arena".to_string()),
            deck_client_id: sidecar["deck_client_id"].as_str().map(str::to_string),
        };
        (client_id, row)
    }

    /// The sealed run's record as the cloud holds it.
    fn cloud_record(cloud: &FakeCloud, client_id: &str) -> (u64, u64) {
        let held = cloud.state.lock().expect("cloud");
        let row = held
            .rows
            .get(&("league".to_string(), client_id.to_string()))
            .expect("the cloud holds the run");
        let bundle: Value = serde_json::from_str(&row.canonical).expect("JSON");
        let snapshot = &bundle["league"]["snapshot"];
        (
            snapshot["wins"].as_u64().expect("wins"),
            snapshot["losses"].as_u64().expect("losses"),
        )
    }

    fn cloud_canonical(cloud: &FakeCloud, client_id: &str) -> String {
        let held = cloud.state.lock().expect("cloud");
        held.rows
            .get(&("league".to_string(), client_id.to_string()))
            .expect("the cloud holds the run")
            .canonical
            .clone()
    }

    fn league_uploads(cloud: &FakeCloud, from: usize) -> usize {
        cloud.stub.requests()[from..]
            .iter()
            .filter(|request| request.target == BLOBS)
            .filter(|request| {
                multipart_fields(request).get("type").map(Vec::as_slice)
                    == Some(b"league".as_slice())
            })
            .count()
    }

    /// Review I4 (A): device B pushed the run at 5-0; device A, behind on
    /// the course, builds a 4-0 late (its clock is past B's). The server
    /// keeps 5-0, A takes it, and both devices settle on it.
    #[test]
    fn a_stale_arena_run_built_late_never_replaces_a_newer_one() {
        let TwoDevices {
            cloud,
            here,
            there,
            here_api,
            there_api,
            _seed,
        } = two_devices("sync-run-stale");
        let (client_id, five) = sealed_run_upload(&here);
        course_read_up_to(&here, 3);
        run(&here_api, &here);
        assert_eq!(cloud_record(&cloud, &client_id), (3, 0));
        cloud.store_as("qdevice-there", "league", &client_id, five);

        course_read_up_to(&here, 4);
        run(&here_api, &here);
        assert_eq!(
            cloud_record(&cloud, &client_id),
            (5, 0),
            "the late 4-0 never replaces 5-0"
        );
        assert_eq!(run_summaries(&here)[1], SEALED_RUN, "A takes 5-0");

        run(&there_api, &there);
        let before = cloud.stub.requests().len();
        run(&here_api, &here);
        run(&there_api, &there);
        assert_eq!(league_uploads(&cloud, before), 0, "settled: nothing sent");
        assert_eq!(cloud_record(&cloud, &client_id), (5, 0));
        assert_eq!(run_summaries(&there), vec![SEALED_RUN.to_string()]);
        let held = cloud_canonical(&cloud, &client_id);
        assert_eq!(
            sealed_run_upload(&here).1.canonical,
            held,
            "A builds the server's bytes"
        );
        assert_eq!(
            sealed_run_upload(&there).1.canonical,
            held,
            "B builds the server's bytes"
        );
        // A's pending figure is left out: the held back cursor offers it the
        // run again, and a pulled run reads as waiting until its next build
        // (the known cosmetic gap after a re-pull), which then sends nothing.
        assert_eq!(pending(&there)[1], 0);
    }

    /// Review I4 (B): a device pulls the run, then plays on; its newer
    /// record uploads at its next sync and reaches the other device.
    #[test]
    fn a_newer_arena_record_after_a_pull_uploads_at_the_next_sync() {
        let TwoDevices {
            cloud,
            here,
            there,
            here_api,
            there_api,
            _seed,
        } = two_devices("sync-run-newer");
        course_read_up_to(&here, 4);
        let (client_id, four) = sealed_run_upload(&here);
        course_read_up_to(&here, 3);
        cloud.store_as("qdevice-there", "league", &client_id, four);
        run(&here_api, &here);
        assert_eq!(cloud_record(&cloud, &client_id), (4, 0), "the pull wins");

        course_read_up_to(&here, 5);
        let before = cloud.stub.requests().len();
        run(&here_api, &here);
        assert_eq!(
            league_uploads(&cloud, before),
            1,
            "the newer record is sent"
        );
        assert_eq!(cloud_record(&cloud, &client_id), (5, 0));
        run(&there_api, &there);
        assert_eq!(run_summaries(&there), vec![SEALED_RUN.to_string()]);
    }

    /// The sealed run's notes as the cloud holds them.
    fn cloud_notes(cloud: &FakeCloud, client_id: &str) -> Value {
        let bundle: Value = serde_json::from_str(&cloud_canonical(cloud, client_id)).expect("JSON");
        bundle["league"]["notes"].clone()
    }

    /// The sealed run's newest listing, seen again `later_ms` later: the
    /// same record, a newer observed time.
    fn course_listed_again(device: &Device, later_ms: i64) {
        let mut conn = store::open_writer(&device.state.db_path).expect("writer");
        conn.execute(
            "UPDATE arena_course_observations SET observed_at = observed_at + ?1
             WHERE observed_at = (SELECT MAX(observed_at) FROM arena_course_observations)",
            [later_ms],
        )
        .expect("listed again");
        let tx = conn.transaction().expect("tx");
        store::arena_run_sync::sync_all_arena_runs(&tx, NOW).expect("fold");
        tx.commit().expect("commit");
    }

    /// Re-review N1: B's notes are older than A's newest listing of the same
    /// record. A's copy replaces B's on the server; B's merge of the two
    /// (A's listing, B's notes) keeps A's time by its own formula, so it is
    /// sent a second past the copy it pulled, and every device ends with
    /// the notes. Exchanging again sends nothing.
    #[test]
    fn a_merge_with_older_notes_replaces_the_copy_it_pulled() {
        let TwoDevices {
            cloud,
            here,
            there,
            here_api,
            there_api,
            _seed,
        } = two_devices("sync-run-notes");
        course_read_up_to(&here, 3);
        run(&here_api, &here);
        run(&there_api, &there);
        let (client_id, _) = sealed_run_upload(&here);
        let sealed = "run_key LIKE 'arena:%' AND name LIKE '%Sealed%'";
        let observed = ids(
            &there,
            &format!("SELECT course_observed_at FROM leagues WHERE {sealed}"),
        )[0];
        store::open_writer(&there.state.db_path)
            .expect("writer")
            .execute(
                &format!(
                    "UPDATE leagues SET notes = 'Qnotes', notes_edited_at = ?1 WHERE {sealed}"
                ),
                [observed + 10_000],
            )
            .expect("B's notes");
        run(&there_api, &there);
        assert_eq!(cloud_notes(&cloud, &client_id), json!("Qnotes"));

        course_listed_again(&here, 60_000);
        run(&here_api, &here);
        assert_eq!(cloud_notes(&cloud, &client_id), Value::Null, "A's copy won");

        run(&there_api, &there);
        run(&there_api, &there);
        assert_eq!(
            cloud_notes(&cloud, &client_id),
            json!("Qnotes"),
            "B's merge won"
        );
        run(&here_api, &here);
        let notes = format!("SELECT COALESCE(notes, '') FROM leagues WHERE {sealed}");
        assert_eq!(lines(&here, &notes), vec!["Qnotes".to_string()]);

        let before = cloud.stub.requests().len();
        run(&here_api, &here);
        run(&there_api, &there);
        assert_eq!(
            league_uploads(&cloud, before),
            0,
            "a repeat exchange sends nothing"
        );
        let held = cloud_canonical(&cloud, &client_id);
        assert_eq!(sealed_run_upload(&here).1.canonical, held);
        assert_eq!(sealed_run_upload(&there).1.canonical, held);
        assert_eq!(pending(&here)[1], 0, "nothing waits on A");
        assert_eq!(pending(&there)[1], 0, "nothing waits on B");
    }

    /// Re-review N2: a deck slot is a server resource. An Arena or Endstep
    /// deck's slot is changed only on an API that confirmed its platform,
    /// so an old API's single free slot is never taken by a deck that would
    /// never upload. MTGO decks are not gated.
    #[test]
    fn an_arena_deck_slot_waits_for_the_platforms_confirmation() {
        let cloud = FakeCloud::start();
        cloud.state.lock().expect("cloud").old_api = true;
        let here = device("sync-slot-gate");
        let _seed = seed_arena_and_endstep(&here, "sync-slot-gate-seed");
        let api = linked_as(&cloud.stub.base_url, "qdevice-here");
        let decks = ids(&here, "SELECT id FROM decks ORDER BY id");
        let refused = toggle(&api, &here, decks[0], true).expect_err("refused");
        assert!(
            matches!(&refused, CommandError::Refused(message) if message == PLATFORM_NOT_CONFIRMED_MESSAGE),
            "{refused:?}"
        );
        let puts = |stub: &Stub| {
            stub.requests()
                .iter()
                .filter(|request| request.method == "PUT")
                .count()
        };
        assert_eq!(puts(&cloud.stub), 0, "no slot request");
        assert!(cloud.state.lock().expect("cloud").enabled.is_empty());

        cloud.state.lock().expect("cloud").old_api = false;
        toggle(&api, &here, decks[0], true).expect("a slot once confirmed");
        assert_eq!(puts(&cloud.stub), 1);
        assert_eq!(cloud.state.lock().expect("cloud").enabled.len(), 1);
    }

    /// Review I3: an API from before Arena and Endstep sync drops a row's
    /// `platform` and would serve it to old clients as MTGO. Its manifest
    /// confirms no platform, so no Arena or Endstep row reaches it, not
    /// even as a dirty entry; the same device sends them all to an API that
    /// confirms them.
    #[test]
    fn an_api_that_confirms_no_platform_gets_no_arena_or_endstep_rows() {
        let TwoDevices {
            cloud,
            here,
            here_api,
            ..
        } = two_devices("sync-old-api");
        cloud.state.lock().expect("cloud").old_api = true;
        for _ in 0..2 {
            run(&here_api, &here);
            let waits = activity(&here_api).matches("has not confirmed").count();
            assert_eq!(waits, 1, "logged once a run: {}", activity(&here_api));
        }
        assert!(
            cloud.state.lock().expect("cloud").rows.is_empty(),
            "nothing stored"
        );
        for sync_type in ["deck", "league", "match"] {
            assert_eq!(
                dirty_ids(&cloud.stub, sync_type),
                Vec::<String>::new(),
                "{sync_type}: no dirty entry"
            );
        }
        let uploads = cloud
            .stub
            .requests()
            .iter()
            .filter(|request| request.target == BLOBS)
            .count();
        assert_eq!(uploads, 0);
        assert_eq!(pending(&here), [2, 1, 6], "all of it waits");

        cloud.state.lock().expect("cloud").old_api = false;
        run(&here_api, &here);
        assert_eq!(pending(&here), [0, 0, 0], "sent once confirmed");
        let held = cloud.state.lock().expect("cloud");
        assert!(
            held.rows.values().all(|row| row.platform.is_some()),
            "every row is stored with its platform"
        );
        assert_eq!(held.rows.len(), 2 + 1 + 6);
    }

    /// Spec sections 7 to 10, end to end: an Arena course run with its
    /// limited matches and an Endstep match, each with its deck, recorded on
    /// device A, reach device B through the cloud in one exchange; B's
    /// replays read the same as A's and the run and its record match. A
    /// second exchange sends nothing, the bundles the cloud offers again
    /// import as no change, and nothing is pending on either device.
    #[test]
    fn arena_and_endstep_sync_between_two_devices() {
        let TwoDevices {
            cloud,
            here,
            there,
            here_api,
            there_api,
            _seed,
        } = two_devices("sync-two-devices");
        run(&here_api, &here);
        run(&there_api, &there);

        {
            let held = cloud.state.lock().expect("cloud");
            let mut kinds: Vec<String> = held
                .rows
                .iter()
                .map(|((kind, _), row)| {
                    format!("{kind} {}", row.platform.as_deref().unwrap_or("mtgo"))
                })
                .collect();
            kinds.sort();
            kinds.dedup();
            assert_eq!(
                kinds,
                vec![
                    "deck arena".to_string(),
                    "deck endstep".to_string(),
                    "league arena".to_string(),
                    "match arena".to_string(),
                    "match endstep".to_string(),
                ],
                "every platform's rows reached the cloud with their platform"
            );
        }
        for device in [&here, &there] {
            assert_eq!(
                store::sync::rejection_count(&reader(device)).expect("rejections"),
                0,
                "the cloud took every upload"
            );
        }
        // The sync sends finished matches; the fixture's last Arena match
        // is still in progress when the log ends.
        let finished = "SELECT a.platform || ' ' || COUNT(*) FROM matches m
                        JOIN accounts a ON a.id = m.account_id
                        WHERE m.state IN ('complete', 'abandoned')
                        GROUP BY a.platform ORDER BY a.platform";
        assert_eq!(
            lines(&here, finished),
            vec!["arena 5".to_string(), "endstep 1".to_string()]
        );
        assert_eq!(lines(&there, finished), lines(&here, finished));
        assert_eq!(lines(&there, PLATFORM_COUNTS), lines(&here, finished));
        // Each match keeps its kind, which the deck pages filter on.
        let kinds = "SELECT token || ' ' || quote(game_structure) FROM matches
                     WHERE state IN ('complete', 'abandoned') ORDER BY token";
        assert!(
            lines(&here, kinds)
                .iter()
                .all(|row| row.ends_with("'bo1'") || row.ends_with("'bo3'")),
            "every seeded match has a kind: {:?}",
            lines(&here, kinds)
        );
        assert_eq!(lines(&there, kinds), lines(&here, kinds));
        assert_eq!(
            ids(
                &there,
                "SELECT COUNT(*) FROM decks WHERE account_id IS NULL"
            ),
            vec![0],
            "every pulled deck is filed under its platform's account"
        );

        // The sealed run syncs with its course deck. The Premier Draft
        // course has no registered deck yet, so no course deck whose slot
        // could carry it (ruling R5): it stays on A.
        assert_eq!(
            run_summaries(&here),
            vec![
                "Premier Draft · FRA 0-0 active".to_string(),
                SEALED_RUN.to_string()
            ]
        );
        assert_eq!(run_summaries(&there), vec![SEALED_RUN.to_string()]);
        let linked_matches = "SELECT COUNT(*) FROM matches m JOIN leagues l ON l.id = m.league_id
                              WHERE l.run_key LIKE 'arena:%'
                                AND m.state IN ('complete', 'abandoned')";
        assert_eq!(ids(&there, linked_matches), ids(&here, linked_matches));

        let sent = lines(
            &here,
            "SELECT token FROM matches WHERE state IN ('complete', 'abandoned')",
        );
        let tokens: Vec<String> = store::arena_replay::arena_tokens_with_messages(&reader(&here))
            .expect("Arena tokens")
            .into_iter()
            .filter(|token| sent.contains(token))
            .collect();
        assert_eq!(tokens.len(), 5);
        for token in &tokens {
            let mut recorded =
                store::arena_replay::arena_match_text(&reader(&here), token).expect("read");
            if let store::arena_replay::ArenaMatchRead::MissingCardData(grp_ids) = &recorded {
                add_arena_cards([&here, &there], grp_ids);
                recorded =
                    store::arena_replay::arena_match_text(&reader(&here), token).expect("read");
            }
            assert!(
                matches!(recorded, store::arena_replay::ArenaMatchRead::Ready(_)),
                "{token}: {recorded:?}"
            );
            let pulled =
                store::arena_replay::arena_match_text(&reader(&there), token).expect("read");
            assert_eq!(pulled, recorded, "{token}: B's replay reads as A's");
        }
        let endstep = store::endstep_replay::endstep_tokens(&reader(&here)).expect("tokens");
        assert_eq!(endstep.len(), 1);
        for token in &endstep {
            let recorded =
                store::endstep_replay::endstep_match_text(&reader(&here), token).expect("read");
            assert!(recorded.is_some(), "{token}");
            let pulled =
                store::endstep_replay::endstep_match_text(&reader(&there), token).expect("read");
            assert_eq!(pulled, recorded, "{token}: B's replay reads as A's");
        }

        // A second exchange settles it. B builds the run it pulled (a
        // pulled row has no touch key until its first build) and finds the
        // server's bytes, so nothing is sent. The cloud's cursor is held
        // back, so B is offered A's newest row again: it is fetched and
        // imports as no change.
        let here_before = fingerprint(&here);
        let there_before = fingerprint(&there);
        let before = cloud.stub.requests().len();
        run(&here_api, &here);
        run(&there_api, &there);
        let requests = cloud.stub.requests()[before..].to_vec();
        let uploads = requests
            .iter()
            .filter(|request| request.target == BLOBS)
            .count();
        let fetches = requests
            .iter()
            .filter(|request| request.target == FETCH)
            .count();
        assert_eq!(uploads, 0, "nothing is sent again");
        assert!(fetches > 0, "the held back cursor offers a row again");
        assert_eq!(fingerprint(&here), here_before);
        assert_eq!(
            fingerprint(&there),
            there_before,
            "the re-import changes nothing"
        );
        assert_eq!(pending(&here), [0, 0, 0], "nothing pending on A");
        assert_eq!(pending(&there), [0, 0, 0], "nothing pending on B");
    }

    /// Whether `value` has the key `key` at any depth.
    fn has_key(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(map) => {
                map.contains_key(key) || map.values().any(|item| has_key(item, key))
            }
            Value::Array(items) => items.iter().any(|item| has_key(item, key)),
            _ => false,
        }
    }

    /// An MTGO-only device sends no `platform` anywhere: not in a bundle, a
    /// sidecar or a request body (only the `platforms` declaration). Its
    /// bundles are the bytes they always were.
    #[test]
    fn an_mtgo_only_run_sends_no_platform_anywhere() {
        let cloud = FakeCloud::start();
        let device = device("sync-mtgo-only-no-platform");
        let deck_id = file_deck(&device, "111", false);
        log_match(&device, "q-mtgo-only", NOW - DAY, "111");
        let api = linked_as(&cloud.stub.base_url, "qdevice-here");
        toggle(&api, &device, deck_id, true).expect("a slot");
        run(&api, &device);

        let requests = cloud.stub.requests();
        let mut bundles = 0;
        for request in &requests {
            if request.target == BLOBS {
                for (name, bytes) in multipart_fields(request) {
                    assert!(!name.ends_with("[platform]"), "{name}");
                    if name.starts_with("blobs[") {
                        let mut text = String::new();
                        flate2::read::GzDecoder::new(bytes.as_slice())
                            .read_to_string(&mut text)
                            .expect("gzip");
                        let bundle: Value = serde_json::from_str(&text).expect("bundle");
                        assert!(!has_key(&bundle, "platform"), "{text}");
                        bundles += 1;
                    }
                }
            } else {
                assert!(!has_key(&body(request), "platform"), "{}", request.target);
            }
        }
        assert_eq!(bundles, 2, "the deck and its match");
        let held = cloud.state.lock().expect("cloud");
        assert_eq!(held.rows.len(), 2);
        assert!(held.rows.values().all(|row| row.platform.is_none()));
    }
}
