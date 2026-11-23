//! The league sync in the tracker (target schema sections 8 and 9): the
//! stored league runs, tournaments, drafts and registered decks are written
//! by the writer's `sync_leagues` from the stored inputs.
//!
//! When:
//! - debounced: a change to an owner's league inputs (a league or tournament
//!   match derived or removed, a league, draft, Login, registered deck or
//!   tournament round info event stored, a link, a hide, an edit or an import) marks the owner
//!   dirty, and the first mark sets a deadline `LEAGUE_SYNC_AFTER_MS` later.
//!   At the first caught-up tick past it (outside the match write batch, and
//!   not while the projection is being rebuilt), one sync folds every dirty
//!   owner. A backlog read over many ticks syncs once, at the end;
//! - when an open draft's `abandons_at` has passed (`now > abandons_at`),
//!   so it turns abandoned without a new event;
//! - in full (`sync_all`) when the stored fold is older than this build's or
//!   was folded in another timezone, and when the OS timezone changes;
//! - before a league edit or a manual match edit, so the edit sees current
//!   runs.
//!
//! A due sync does not run on a tick that pushes a match (`MatchUpdated`): it
//! waits for the next tick, so the push never waits on a fold. It waits once:
//! the tick after runs it even if that tick pushes a match too.
//!
//! A pending sync survives a stop and a crash: the stop runs it, and the
//! first mark also stores `LEAGUE_SYNC_PENDING` (true) with the tick's batch,
//! cleared once a sync commits with nothing left pending. An open that finds
//! it true syncs every owner once caught up (a crash is rare, and the cost
//! is one full fold, the same as a new build's backfill).
//!
//! A failed sync keeps its owners and retries after `LEAGUE_RETRY_FIRST_MS`,
//! doubling up to `LEAGUE_RETRY_MAX_MS`; the league events are not held
//! while it retries.
//!
//! `LeaguesChanged` and `LimitedChanged` wait while a sync is pending, and
//! the sync's outcome can raise them. `MatchUpdated` never waits.

use std::collections::HashMap;

use store::StoreError;
use store::league_sync::{LEAGUE_FOLD_TIMEZONE, LeagueOwner, league_fold_current};
use store::read;
use store::write::WriteBatch;

use super::Engine;
use crate::TrackerError;

/// How long after the first change the league sync runs (when caught up).
pub const LEAGUE_SYNC_AFTER_MS: i64 = 1_000;
/// The setting that says a league sync was pending.
pub const LEAGUE_SYNC_PENDING: &str = "league_sync_pending";
/// After a failed league sync, the first retry waits this long.
pub const LEAGUE_RETRY_FIRST_MS: i64 = 5_000;
/// The longest wait between retries.
pub const LEAGUE_RETRY_MAX_MS: i64 = 60_000;

/// The league sync's bookkeeping in the engine.
#[derive(Debug, Default)]
pub(crate) struct LeagueSyncState {
    /// Owners whose league inputs changed since the last sync.
    pub(crate) dirty_owners: std::collections::HashSet<LeagueOwner>,
    /// An input changed whose owner is not known (an import, a match of an
    /// account not stored yet): every owner is synced.
    pub(crate) every_owner_dirty: bool,
    /// Sync every owner at the next caught-up tick, without waiting for the
    /// deadline: the stored fold is from another build or timezone.
    pub(crate) sync_all_wanted: bool,
    /// When the debounced sync is due (set by the first mark).
    pub(crate) league_sync_due_ms: Option<i64>,
    /// The earliest `abandons_at` of the attached drafts.
    pub(crate) next_abandon_ms: Option<i64>,
    /// Account ids by username, as read.
    account_ids: HashMap<String, i64>,
    /// Syncs run so far, failed ones too (tests count them).
    pub(crate) syncs: u32,
    /// `LEAGUE_SYNC_PENDING` is stored as true.
    marker_written: bool,
    /// A due sync was left to the next tick because its tick pushed a match.
    /// The next tick runs it whatever it pushes, so a burst of ticks that
    /// each push a match delays it by one tick, not for good.
    pub(crate) postponed: bool,
    /// The last sync failed: the next one waits `retry_ms`.
    retrying: bool,
    retry_ms: i64,
}

impl Engine {
    /// At open: a full sync when the stored fold is not current in this
    /// zone (a zone change while closed also syncs the games again), every
    /// owner when a sync was pending when the tracker last went away, and
    /// the first abandonment deadline.
    pub(super) fn plan_league_sync(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        self.leagues.sync_all_wanted = !league_fold_current(&self.reader, self.timezone)?;
        let folded_in = self.stored_setting(LEAGUE_FOLD_TIMEZONE)?;
        let zone = format!("\"{}\"", self.timezone.name());
        if folded_in.is_some_and(|folded_in| folded_in != zone) {
            self.restart_game_backfill()?;
        }
        if self.stored_setting(LEAGUE_SYNC_PENDING)?.as_deref() == Some("true") {
            self.leagues.marker_written = true;
            self.leagues.every_owner_dirty = true;
            self.leagues.league_sync_due_ms = Some(now_ms + LEAGUE_SYNC_AFTER_MS);
        }
        self.leagues.next_abandon_ms = read::next_abandon_at(&self.reader)?;
        Ok(())
    }

    /// A stored setting's value.
    pub(super) fn stored_setting(&self, key: &str) -> Result<Option<String>, TrackerError> {
        Ok(read::settings(&self.reader)?
            .into_iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, value)| value))
    }

    /// Store that a league sync is pending, with the next batch (once).
    fn store_league_marker(&mut self) {
        if !self.leagues.marker_written {
            self.leagues.marker_written = true;
            self.batch
                .settings
                .push((LEAGUE_SYNC_PENDING.to_string(), "true".to_string()));
        }
    }

    /// The OS timezone changed: run names and pick times depend on it, so
    /// every owner is folded again.
    pub(super) fn refold_for_new_zone(&mut self) {
        self.leagues.sync_all_wanted = true;
    }

    /// Whether a sync is waiting.
    pub(super) fn league_sync_pending(&self) -> bool {
        self.leagues.sync_all_wanted
            || self.leagues.every_owner_dirty
            || !self.leagues.dirty_owners.is_empty()
    }

    /// Whether the league events wait: a sync is pending and not retrying
    /// after a failure.
    pub(super) fn league_events_held(&self) -> bool {
        self.league_sync_pending() && !self.leagues.retrying
    }

    /// The stop: run a pending sync of marked owners (a full sync for a new
    /// build or zone runs at the next start instead).
    pub(super) fn sync_leagues_before_stopping(&mut self) -> Result<(), TrackerError> {
        let marked = self.leagues.every_owner_dirty || !self.leagues.dirty_owners.is_empty();
        if marked && !self.leagues.sync_all_wanted {
            self.sync_leagues(self.batch.now_ms)?;
        }
        Ok(())
    }

    /// Mark one owner's league inputs changed.
    pub(super) fn mark_owner(&mut self, owner: LeagueOwner) {
        self.leagues.dirty_owners.insert(owner);
        self.start_league_deadline();
        self.store_league_marker();
    }

    /// Mark every owner's league inputs changed.
    pub(super) fn mark_every_owner(&mut self) {
        self.leagues.every_owner_dirty = true;
        self.start_league_deadline();
        self.store_league_marker();
    }

    /// Mark the owner with this username (the player of a match or of a log
    /// file). An account not stored yet, or no username, marks every owner.
    pub(super) fn mark_account(&mut self, username: Option<&str>) {
        let Some(username) = username else {
            self.mark_every_owner();
            return;
        };
        if let Some(id) = self.leagues.account_ids.get(username) {
            let owner = LeagueOwner::Account(*id);
            self.mark_owner(owner);
            return;
        }
        match read::account_id(&self.reader, username) {
            Ok(Some(id)) => {
                self.leagues.account_ids.insert(username.to_string(), id);
                self.mark_owner(LeagueOwner::Account(id));
            }
            Ok(None) => self.mark_every_owner(),
            Err(error) => {
                tracing::warn!(%error, "could not read an account; syncing every league owner");
                self.mark_every_owner();
            }
        }
    }

    /// Mark the owner of a stored match (every owner when it is not stored),
    /// and queue its Arena runs' sync when it is a limited Arena match.
    pub(super) fn mark_match_owner(&mut self, token: &str) {
        self.queue_arena_runs_of_match(token);
        match read::match_account(&self.reader, token) {
            Ok(Some(account)) => self.mark_owner(owner_of(account)),
            Ok(None) => self.mark_every_owner(),
            Err(error) => {
                tracing::warn!(%token, %error, "could not read a match's account; syncing every league owner");
                self.mark_every_owner();
            }
        }
    }

    /// Mark the owner of a stored league run (every owner when there is no
    /// such run).
    pub(super) fn mark_run_owner(&mut self, run_key: &str) {
        match read::league_run_account(&self.reader, run_key) {
            Ok(Some(account)) => self.mark_owner(owner_of(account)),
            Ok(None) => self.mark_every_owner(),
            Err(error) => {
                tracing::warn!(%run_key, %error, "could not read a run's account; syncing every league owner");
                self.mark_every_owner();
            }
        }
    }

    fn start_league_deadline(&mut self) {
        if self.leagues.league_sync_due_ms.is_none() {
            self.leagues.league_sync_due_ms = Some(self.batch.now_ms + LEAGUE_SYNC_AFTER_MS);
        }
    }

    /// Whether the league sync runs at this (caught-up) tick.
    pub(super) fn league_sync_due(&self, now_ms: i64) -> bool {
        let state = &self.leagues;
        let debounced =
            self.league_sync_pending() && state.league_sync_due_ms.is_some_and(|due| now_ms >= due);
        // A retry waits for its backoff, abandonment included.
        let abandoning = !state.retrying && state.next_abandon_ms.is_some_and(|at| now_ms > at);
        state.sync_all_wanted || debounced || abandoning
    }

    /// Run the sync of marked owners now (before a pick note edit), whatever
    /// its deadline. A full sync (new build or timezone) is left for its own
    /// tick: a note edit never waits for it.
    pub(super) fn sync_marked_leagues(&mut self) -> Result<(), TrackerError> {
        let marked = self.leagues.every_owner_dirty || !self.leagues.dirty_owners.is_empty();
        if !marked {
            return Ok(());
        }
        let full_wanted = std::mem::take(&mut self.leagues.sync_all_wanted);
        let result = self.sync_leagues(self.batch.now_ms);
        self.leagues.sync_all_wanted |= full_wanted;
        result
    }

    /// Run a pending league sync now (before an edit), whatever its
    /// deadline.
    pub(super) fn sync_pending_leagues(&mut self) -> Result<(), TrackerError> {
        if self.league_sync_pending() {
            self.sync_leagues(self.batch.now_ms)?;
        }
        Ok(())
    }

    /// Run the league sync now: every dirty owner, plus the owners of drafts
    /// abandoned by now (or every owner), and raise the league events from
    /// its outcome. Committed when it returns.
    pub(super) fn sync_leagues(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        let mut all = self.leagues.sync_all_wanted || self.leagues.every_owner_dirty;
        let mut owners: Vec<LeagueOwner> = self.leagues.dirty_owners.drain().collect();
        if !all && self.leagues.next_abandon_ms.is_some_and(|at| now_ms > at) {
            match read::abandoned_draft_accounts(&self.reader, now_ms) {
                Ok(accounts) => {
                    for account in accounts {
                        let owner = owner_of(account);
                        if !owners.contains(&owner) {
                            owners.push(owner);
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "could not read the abandoned drafts; syncing every league owner");
                    all = true;
                }
            }
        }
        owners.sort_by_key(|owner| match owner {
            LeagueOwner::Account(id) => (0, *id),
            LeagueOwner::Nobody => (1, 0),
        });
        self.leagues.sync_all_wanted = false;
        self.leagues.every_owner_dirty = false;
        self.leagues.league_sync_due_ms = None;
        self.leagues.postponed = false;
        self.leagues.syncs += 1;
        let synced = owners.len();
        match self
            .writer
            .sync_leagues(owners.clone(), all, self.timezone, now_ms)
        {
            Ok((outcome, work)) => {
                tracing::debug!(
                    target: "mymtgo::timing",
                    rows = work.rows,
                    micros = work.micros,
                    owners = synced,
                    all,
                    "league_sync"
                );
                for token in &outcome.skipped_snapshots {
                    tracing::warn!(%token, "a limited match's registered deck could not be read from its archive");
                }
                self.leagues_changed |= outcome.leagues_changed;
                self.limited_changed |= outcome.limited_changed;
                self.leagues.retrying = false;
                self.leagues.retry_ms = 0;
                if self.leagues.marker_written {
                    self.leagues.marker_written = false;
                    let unsent = self
                        .batch
                        .settings
                        .iter()
                        .position(|(key, _)| key == LEAGUE_SYNC_PENDING);
                    match unsent {
                        // The mark's "true" is still in the unsent batch (a
                        // sync run before an edit): it would land after a
                        // "false" written now, so it is dropped instead.
                        Some(index) => {
                            self.batch.settings.remove(index);
                        }
                        // After the sync, in the writer's order.
                        None => self.writer.write(WriteBatch {
                            settings: vec![(LEAGUE_SYNC_PENDING.to_string(), "false".to_string())],
                            ..WriteBatch::default()
                        })?,
                    }
                }
            }
            Err(StoreError::WriterGone) => return Err(StoreError::WriterGone.into()),
            Err(error) => {
                // Keep the owners and try again after a backoff; the stored
                // marker stays, so a start retries too.
                let state = &mut self.leagues;
                state.retry_ms = if state.retry_ms == 0 {
                    LEAGUE_RETRY_FIRST_MS
                } else {
                    (state.retry_ms * 2).min(LEAGUE_RETRY_MAX_MS)
                };
                tracing::warn!(%error, retry_in_ms = state.retry_ms, "the league sync failed");
                state.retrying = true;
                state.every_owner_dirty |= all;
                state.dirty_owners.extend(owners);
                state.league_sync_due_ms = Some(now_ms + state.retry_ms);
            }
        }
        let next = match read::next_abandon_at(&self.reader) {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(%error, "could not read the next draft deadline");
                None
            }
        };
        self.leagues.next_abandon_ms = match next {
            // The sync abandons every draft past its deadline; one still past
            // it would make every tick sync again.
            Some(at) if at < now_ms => {
                tracing::warn!(
                    abandons_at = at,
                    "an open draft past its deadline was not abandoned"
                );
                None
            }
            other => other,
        };
        Ok(())
    }
}

/// The owner of rows with this `account_id`.
fn owner_of(account: Option<i64>) -> LeagueOwner {
    match account {
        Some(id) => LeagueOwner::Account(id),
        None => LeagueOwner::Nobody,
    }
}
