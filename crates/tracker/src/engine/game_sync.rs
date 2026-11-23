//! The game sync (target schema section 6, "Writers"): a log match's
//! per-game card rows (`game_cards`, `game_card_casts`,
//! `game_mulligan_cards`, the hand sizes) are written by the writer from the
//! stored events, as separate requests after the tick's batch commits, so the
//! match push never waits on them.
//!
//! When:
//! - a game settles (its result is no longer `unknown`) and when the match
//!   settles (no longer in progress);
//! - once a match is complete (live or while catching up), its `.dat` game
//!   log is looked for, then every `GAME_LOG_RETRY_MS` up to
//!   `GAME_LOG_TRIES` times. One walk of MTGO's data folder
//!   (`list_game_logs`) resolves every due match by token, never one walk
//!   per match. A file found is read and the match synced with it (decoded
//!   into `game_logs`); it is looked at again on the next round and read
//!   again while its size or modified time changes (the file can still be
//!   being written), within the same tries;
//! - after a population run changed card data (a name or an oracle id),
//!   every log match with a game row for a changed card, one match per
//!   slice (its own transaction), after the `.dat` files found and before
//!   the backfill;
//! - at startup, when the stored rows were built with an older
//!   `GAME_CARDS_VERSION` (or never), and when the OS timezone changes,
//!   every log match is synced in the background, then the version is
//!   recorded. Each backfilled match's `.dat` file is looked for once, with
//!   one walk when the backfill ends: an old match's file does not appear
//!   later.
//!
//! The data folder is walked at most once per `GAME_LOG_RETRY_MS`, plus
//! once when a match completes (its first look, so its file is read at
//! once) and once when a backfill ends. Each walk is a directory listing on
//! the tick thread; these bounds keep it rare.
//!
//! Every sync goes to the writer in slices of at most `GAME_SYNCS_PER_TICK`
//! matches, each its own transaction, and only one slice is in flight: the
//! settled matches first, then the `.dat` files found, then the backfill.
//! While slices remain the tick reports `syncing`, and the supervisor ticks
//! again as soon as the slice in flight is answered, so a backfill or a
//! rebuild's queue takes about its work time while each slice holds the
//! writer under 100 ms and the match batches go between them. The syncs are
//! sent at caught-up ticks outside a projection rebuild, after the tick's
//! batch, so a backlog is synced once at the end and the match push never
//! waits on them. A sync that wrote rows for a match the player sees pushes
//! that match again (`MatchUpdated`): its first push came before the rows,
//! and an open match page refetches on it.
//!
//! A match whose events cannot be read (a corrupt archive) is skipped by the
//! writer and logged; the rest of its slice is synced. The backfill still
//! records the version: the archive is a fact that never changes, so another
//! try would fail the same way, and not recording would sync every match
//! again at every start. A skipped live match is synced again on its next
//! change, and every skipped match at the next `GAME_CARDS_VERSION`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use mtgo_match::{Derived, GameResult, MatchState, MatchView};
use store::StoreError;
use store::game_sync::{GAME_CARDS_VERSION, game_cards_version, log_match_tokens};
use store::writer::GameSyncReply;

use super::{Engine, FileStamp, file_stamp};
use crate::TrackerError;

/// How long after a miss the `.dat` file is looked for again.
pub const GAME_LOG_RETRY_MS: i64 = 60_000;
/// How many times a complete match's `.dat` file is looked for.
pub const GAME_LOG_TRIES: u32 = 10;
/// Log matches per game sync slice, so a sync never stalls live tracking:
/// one slice is one writer transaction, and 10 matches of 300 frames take
/// about 60 ms in release (20 took about 120 ms, over the 100 ms a
/// background step may hold the writer; Plan R Task 8). Also the most `.dat`
/// files read per slice.
pub const GAME_SYNCS_PER_TICK: usize = 10;

/// The setting that says live game syncs were pending.
pub const GAME_SYNC_PENDING: &str = "game_sync_pending";

/// A match whose `.dat` file is looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DatLookup {
    /// Looks so far.
    tries: u32,
    /// When to look next.
    next_at_ms: i64,
    /// The file's stamp when it was last read, `None` before it is found.
    read: Option<FileStamp>,
    /// The file the last walk found, waiting for a slice to read it.
    found: Option<PathBuf>,
}

/// A sync request sent and not answered yet.
#[derive(Debug)]
struct SentSync {
    /// The matches to push again when the sync writes rows (none for the
    /// backfill: nobody waits on it).
    refresh: Vec<String>,
    /// The latest view and revision of each queued match, pushed again when
    /// the projector no longer holds the match.
    views: HashMap<String, (Box<MatchView>, u64)>,
    reply: Receiver<GameSyncReply>,
    /// A card resync (`queue_card_resync`): its answer may end the queue.
    card_resync: bool,
}

/// The game sync's bookkeeping in the engine.
#[derive(Debug, Default)]
pub(crate) struct GameSyncState {
    /// Per match at its last sync request: the settled games, whether the
    /// match had settled, and whether it was complete.
    marks: HashMap<String, (usize, bool, bool)>,
    /// Matches to sync at the next caught-up tick, after its batch.
    pending: Vec<String>,
    /// The latest view and revision of each pending match.
    pending_views: HashMap<String, (Box<MatchView>, u64)>,
    /// Matches whose `.dat` file is still looked for (or watched while it
    /// may still be written).
    dat_lookups: HashMap<String, DatLookup>,
    /// Log matches to sync again because card data they read changed.
    resync: Vec<String>,
    /// Card resyncs sent so far (tests count them).
    #[cfg(test)]
    resyncs_sent: Vec<String>,
    /// Log matches the startup backfill still has to sync.
    backfill: Vec<String>,
    backfilling: bool,
    /// The backfill's matches, whose `.dat` files are looked for once when
    /// it ends.
    backfill_looks: Vec<String>,
    /// When the data folder was last walked.
    last_walk_ms: Option<i64>,
    /// Walk at the next look whatever the time (a backfill ended).
    walk_now: bool,
    /// Requests sent and not answered yet.
    replies: Vec<SentSync>,
    version_reply: Option<Receiver<Result<(), StoreError>>>,
    /// `GAME_SYNC_PENDING` is stored as true.
    marker_written: bool,
    /// A sync failed after it was sent: the marker stays true until the next
    /// start, whose backfill syncs every match again.
    sync_failed: bool,
    /// Walks of the data folder so far (tests count them).
    #[cfg(test)]
    walks: u32,
    /// The slowest slice's writer time and the sum of all slices' (tests).
    #[cfg(test)]
    pub(crate) slowest_micros: u64,
    #[cfg(test)]
    pub(crate) work_micros: u64,
}

impl GameSyncState {
    /// Whether the startup backfill is still running.
    #[cfg(test)]
    pub(crate) fn backfilling_now(&self) -> bool {
        self.backfilling
    }

    /// The log matches the backfill still has to sync.
    #[cfg(test)]
    pub(crate) fn backfill_queue(&self) -> &[String] {
        &self.backfill
    }

    /// Whether this token's `.dat` file is still looked for.
    #[cfg(test)]
    pub(crate) fn looks_for_dat(&self, token: &str) -> bool {
        self.dat_lookups.contains_key(token)
    }

    /// Queue syncs as settling matches do.
    #[cfg(test)]
    pub(crate) fn queue_for_test(&mut self, tokens: Vec<String>) {
        self.pending.extend(tokens);
    }

    /// Matches queued and not sent yet.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.pending.len()
    }

    /// Requests sent and not answered yet.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.replies.len()
    }

    /// Card resyncs queued or not answered yet.
    #[cfg(test)]
    pub(crate) fn card_resync_outstanding(&self) -> bool {
        !self.resync.is_empty() || self.replies.iter().any(|sent| sent.card_resync)
    }

    /// The card resyncs sent so far, in order.
    #[cfg(test)]
    pub(crate) fn resyncs_sent(&self) -> &[String] {
        &self.resyncs_sent
    }

    /// Walks of the data folder so far.
    #[cfg(test)]
    pub(crate) fn walks(&self) -> u32 {
        self.walks
    }

    /// Matches whose `.dat` file is still looked for.
    #[cfg(test)]
    pub(crate) fn dat_lookups(&self) -> usize {
        self.dat_lookups.len()
    }
}

impl Engine {
    /// Plan the startup backfill when the stored game rows are older than
    /// this build's (or there are none yet).
    pub(super) fn plan_game_backfill(&mut self) -> Result<(), TrackerError> {
        // Syncs pending when the tracker last went away were lost: sync
        // every log match again (a crash is rare; the backfill's budget).
        if self.stored_setting(GAME_SYNC_PENDING)?.as_deref() == Some("true") {
            self.game_sync.marker_written = true;
            return self.restart_game_backfill();
        }
        let stored = game_cards_version(&self.reader)?;
        if stored.is_some_and(|version| version >= GAME_CARDS_VERSION) {
            return Ok(());
        }
        self.restart_game_backfill()
    }

    /// Sync every log match again in the background (a new build's rows, or
    /// the OS timezone changed).
    pub(super) fn restart_game_backfill(&mut self) -> Result<(), TrackerError> {
        self.game_sync.backfill = log_match_tokens(&self.reader)?;
        self.game_sync.backfill_looks = self.game_sync.backfill.clone();
        self.game_sync.backfilling = true;
        Ok(())
    }

    /// Queue syncs as settling matches do (tests).
    #[cfg(test)]
    pub(crate) fn queue_game_syncs_for_test(&mut self, tokens: Vec<String>) {
        self.game_sync.queue_for_test(tokens);
    }

    /// The slowest slice's writer time and the sum of all slices', in
    /// microseconds (tests).
    #[cfg(test)]
    pub(crate) fn game_sync_work(&self) -> (u64, u64) {
        (self.game_sync.slowest_micros, self.game_sync.work_micros)
    }

    /// Whether the backfill still has matches to send (tests).
    #[cfg(test)]
    pub(crate) fn game_backfill_running(&self) -> bool {
        self.game_sync.backfilling
    }

    /// Whether game syncs are queued or in flight.
    fn game_syncs_queued(&self) -> bool {
        let state = &self.game_sync;
        !state.pending.is_empty()
            || !state.resync.is_empty()
            || state.backfilling
            || !state.replies.is_empty()
            || state
                .dat_lookups
                .values()
                .any(|lookup| lookup.found.is_some())
    }

    /// Take the answers that came back; true once no slice is in flight, so
    /// the supervisor ticks again to send the next one.
    pub(crate) fn game_slice_answered(&mut self) -> bool {
        self.take_game_sync_replies();
        let arena_waiting = self.take_arena_sync_replies();
        self.game_sync.replies.is_empty() && !arena_waiting
    }

    /// A visible match's new result: queue a sync when a game or the match
    /// settled since the last request, and look for its `.dat` file once the
    /// match is complete. `revision` is the view's, for the push after the
    /// sync.
    pub(super) fn note_game_sync(&mut self, view: &MatchView, revision: u64) {
        let settled = view
            .games
            .iter()
            .filter(|game| game.result != GameResult::Unknown)
            .count();
        let match_settled = view.state != MatchState::InProgress;
        let complete = view.state == MatchState::Complete;
        let mark = (settled, match_settled, complete);
        if self.game_sync.pending.contains(&view.token) {
            self.game_sync
                .pending_views
                .insert(view.token.clone(), (Box::new(view.clone()), revision));
        }
        let before = self.game_sync.marks.get(&view.token).copied();
        if before == Some(mark) {
            return;
        }
        self.game_sync.marks.insert(view.token.clone(), mark);
        if settled == 0 && !match_settled {
            return;
        }
        if !self.game_sync.pending.contains(&view.token) {
            self.game_sync.pending.push(view.token.clone());
        }
        if !self.game_sync.marker_written {
            // With the batch that stores the settled result.
            self.game_sync.marker_written = true;
            self.batch
                .settings
                .push((GAME_SYNC_PENDING.to_string(), "true".to_string()));
        }
        self.game_sync
            .pending_views
            .insert(view.token.clone(), (Box::new(view.clone()), revision));
        let was_complete = before.is_some_and(|(_, _, complete)| complete);
        if complete && !was_complete {
            self.look_for_dat(&view.token);
        }
    }

    /// Sync these log matches again because card data their rows read
    /// changed (`Engine::resolve_cards`). The pending marker is stored with
    /// the next batch, so a resync lost to a crash becomes a backfill.
    pub(super) fn queue_card_resync(&mut self, tokens: Vec<String>) {
        if tokens.is_empty() {
            return;
        }
        for token in tokens {
            if !self.game_sync.resync.contains(&token) {
                self.game_sync.resync.push(token);
            }
        }
        if !self.game_sync.marker_written {
            self.game_sync.marker_written = true;
            self.batch
                .settings
                .push((GAME_SYNC_PENDING.to_string(), "true".to_string()));
        }
    }

    /// Look for this match's `.dat` file from the next caught-up tick.
    fn look_for_dat(&mut self, token: &str) {
        let now_ms = self.batch.now_ms;
        self.game_sync
            .dat_lookups
            .entry(token.to_string())
            .or_insert(DatLookup {
                tries: 0,
                next_at_ms: now_ms,
                read: None,
                found: None,
            });
    }

    /// Look for the backfilled matches' `.dat` files once, with one walk
    /// at the next look. A match already looked for keeps its tries.
    fn look_for_backfilled_dats(&mut self, now_ms: i64) {
        for token in std::mem::take(&mut self.game_sync.backfill_looks) {
            self.game_sync
                .dat_lookups
                .entry(token)
                .or_insert(DatLookup {
                    tries: GAME_LOG_TRIES - 1,
                    next_at_ms: now_ms,
                    read: None,
                    found: None,
                });
        }
        self.game_sync.walk_now = true;
    }

    /// Forget a match that is no longer visible.
    pub(super) fn forget_game_sync(&mut self, token: &str) {
        self.game_sync.marks.remove(token);
        self.game_sync.pending_views.remove(token);
        self.game_sync.dat_lookups.remove(token);
    }

    /// After the tick's batch: take the answers that came back; when caught
    /// up (and not rebuilding), look for the due `.dat` files and, unless a
    /// slice is still in flight, send the next one. Returns whether game
    /// syncs remain (the tick's `syncing`).
    pub(super) fn send_game_syncs(
        &mut self,
        now_ms: i64,
        caught_up: bool,
    ) -> Result<bool, TrackerError> {
        self.take_game_sync_replies();
        if caught_up {
            self.look_for_game_logs(now_ms)?;
            if self.game_sync.replies.is_empty() {
                self.send_next_slice(now_ms)?;
            }
            self.clear_game_marker()?;
        }
        if let Some(reply) = &self.game_sync.version_reply {
            match reply.try_recv() {
                Ok(Ok(())) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.game_sync.version_reply = None;
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, "the game card rows' version could not be recorded");
                    self.game_sync.version_reply = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        Ok(caught_up && self.game_syncs_queued())
    }

    /// Send one slice: the settled matches first (the player waits on
    /// them), then the `.dat` files found, then the backfill (the
    /// projection rebuild re-derives matches first; the backfill follows).
    fn send_next_slice(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        if !self.game_sync.pending.is_empty() {
            return self.send_pending_slice();
        }
        if self.send_dat_slice()? {
            return Ok(());
        }
        if !self.game_sync.resync.is_empty() {
            // One match per slice: its own transaction.
            let token = self.game_sync.resync.remove(0);
            #[cfg(test)]
            self.game_sync.resyncs_sent.push(token.clone());
            let reply = self
                .writer
                .sync_games(vec![token.clone()], Vec::new(), self.timezone)?;
            self.game_sync.replies.push(SentSync {
                refresh: vec![token],
                views: HashMap::new(),
                reply,
                card_resync: true,
            });
            return Ok(());
        }
        if self.game_sync.backfilling {
            let count = self.game_sync.backfill.len().min(GAME_SYNCS_PER_TICK);
            let tokens: Vec<String> = self.game_sync.backfill.drain(..count).collect();
            if !tokens.is_empty() {
                let reply = self.writer.sync_games(tokens, Vec::new(), self.timezone)?;
                self.game_sync.replies.push(SentSync {
                    refresh: Vec::new(),
                    views: HashMap::new(),
                    reply,
                    card_resync: false,
                });
            }
            if self.game_sync.backfill.is_empty() {
                // After the last slice, in the writer's order.
                self.game_sync.backfilling = false;
                self.game_sync.version_reply = Some(self.writer.record_game_cards_version()?);
                self.look_for_backfilled_dats(now_ms);
            }
        }
        Ok(())
    }

    /// Send the next `GAME_SYNCS_PER_TICK` queued matches as one slice.
    fn send_pending_slice(&mut self) -> Result<(), TrackerError> {
        let count = self.game_sync.pending.len().min(GAME_SYNCS_PER_TICK);
        let tokens: Vec<String> = self.game_sync.pending.drain(..count).collect();
        let mut views = HashMap::new();
        for token in &tokens {
            if let Some(view) = self.game_sync.pending_views.remove(token) {
                views.insert(token.clone(), view);
            }
        }
        let reply = self
            .writer
            .sync_games(tokens.clone(), Vec::new(), self.timezone)?;
        self.game_sync.replies.push(SentSync {
            refresh: tokens,
            views,
            reply,
            card_resync: false,
        });
        Ok(())
    }

    /// Send every queued match while stopping, still one slice per
    /// transaction.
    pub(super) fn send_pending_game_syncs(&mut self) -> Result<(), TrackerError> {
        while !self.game_sync.pending.is_empty() {
            self.send_pending_slice()?;
        }
        self.clear_game_marker()
    }

    /// Store that no game sync is pending, after the requests sent (in the
    /// writer's order), once nothing is queued and no backfill runs.
    fn clear_game_marker(&mut self) -> Result<(), TrackerError> {
        let done = self.game_sync.pending.is_empty()
            && self.game_sync.resync.is_empty()
            && !self.game_sync.backfilling;
        if self.game_sync.marker_written && done && !self.game_sync.sync_failed {
            self.game_sync.marker_written = false;
            self.writer.write(store::write::WriteBatch {
                settings: vec![(GAME_SYNC_PENDING.to_string(), "false".to_string())],
                ..store::write::WriteBatch::default()
            })?;
        }
        Ok(())
    }

    /// A sync failed after it was sent: its marker may already be cleared, so
    /// store it as pending again and keep it until the next start (whose
    /// backfill syncs every match again).
    fn keep_game_marker(&mut self) {
        self.game_sync.sync_failed = true;
        if self.game_sync.marker_written {
            return;
        }
        self.game_sync.marker_written = true;
        let written = self.writer.write(store::write::WriteBatch {
            settings: vec![(GAME_SYNC_PENDING.to_string(), "true".to_string())],
            ..store::write::WriteBatch::default()
        });
        if let Err(error) = written {
            tracing::warn!(%error, "the game sync's pending marker could not be stored");
        }
    }

    /// Log the answers that came back, and push again each match a sync
    /// wrote rows for. When the last card resync is answered, `CardsChanged`
    /// is told again: pages that refetched on the first one (sent with the
    /// names, before the resyncs) read the resynced rows now. `push_again`
    /// reaches only the live visible matches.
    fn take_game_sync_replies(&mut self) {
        let mut waiting = Vec::new();
        let mut had_card_resync = false;
        for sent in std::mem::take(&mut self.game_sync.replies) {
            if sent.card_resync {
                had_card_resync = true;
            }
            match sent.reply.try_recv() {
                Ok(Ok((outcome, work))) => {
                    tracing::debug!(
                        target: "mymtgo::timing",
                        rows = work.rows,
                        micros = work.micros,
                        matches = sent.refresh.len(),
                        "game_sync"
                    );
                    #[cfg(test)]
                    {
                        self.game_sync.slowest_micros =
                            self.game_sync.slowest_micros.max(work.micros);
                        self.game_sync.work_micros += work.micros;
                    }
                    for token in &outcome.skipped {
                        tracing::warn!(%token, "a match's games could not be synced: its events could not be read");
                    }
                    if outcome.changed {
                        // Only the matches whose rows changed: the others
                        // have nothing new to show.
                        let changed: Vec<String> = sent
                            .refresh
                            .into_iter()
                            .filter(|token| outcome.changed_tokens.contains(token))
                            .collect();
                        self.push_again(changed, sent.views);
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, "a game sync failed");
                    self.keep_game_marker();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => waiting.push(sent),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if had_card_resync
            && self.game_sync.resync.is_empty()
            && !waiting.iter().any(|sent| sent.card_resync)
        {
            self.cards_changed = true;
        }
        self.game_sync.replies = waiting;
    }

    /// Push these matches again (the projector's view, or the one queued
    /// with the sync), unless the player cannot see them.
    fn push_again(
        &mut self,
        tokens: Vec<String>,
        mut views: HashMap<String, (Box<MatchView>, u64)>,
    ) {
        for token in tokens {
            if !self.visible.contains(&token) || self.hidden.contains(&token) {
                continue;
            }
            let current = match self.projector.derived(&token) {
                Some(Derived::Visible(view)) => {
                    Some((view.clone(), self.projector.revision(&token)))
                }
                // A newer revision stored since was pushed already.
                _ => views.remove(&token).filter(|(_, revision)| {
                    store::read::match_revision(&self.reader, &token)
                        .ok()
                        .flatten()
                        .is_none_or(|stored| stored <= *revision)
                }),
            };
            if let Some((view, revision)) = current {
                self.push_projected(*view, revision);
            }
        }
    }

    /// Look for the `.dat` files that are due, with one walk of the data
    /// folder for all of them, at most once per `GAME_LOG_RETRY_MS` unless a
    /// match looks for the first time or a backfill ended. A file found (or
    /// changed since it was read) waits for a slice to read it; one
    /// unchanged since its read is done. Every look counts a try; after
    /// `GAME_LOG_TRIES` a match with no file waiting is dropped.
    fn look_for_game_logs(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        let mut due: Vec<String> = self
            .game_sync
            .dat_lookups
            .iter()
            .filter(|(_, lookup)| lookup.next_at_ms <= now_ms && lookup.found.is_none())
            .map(|(token, _)| token.clone())
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        let first_look = due
            .iter()
            .any(|token| self.game_sync.dat_lookups[token].tries == 0);
        let walk_due = self.game_sync.walk_now
            || first_look
            || self
                .game_sync
                .last_walk_ms
                .is_none_or(|at| now_ms - at >= GAME_LOG_RETRY_MS);
        if !walk_due {
            return Ok(());
        }
        due.sort();
        self.game_sync.walk_now = false;
        self.game_sync.last_walk_ms = Some(now_ms);
        let files = self.game_log_files();
        #[cfg(test)]
        {
            self.game_sync.walks += 1;
        }
        for token in due {
            let Some(mut lookup) = self.game_sync.dat_lookups.remove(&token) else {
                continue;
            };
            if let Some(path) = files.get(&token) {
                match file_stamp(path) {
                    // Read already and unchanged since: settled.
                    Some(stamp) if lookup.read == Some(stamp) => continue,
                    Some(_) => lookup.found = Some(path.clone()),
                    None => {}
                }
            }
            lookup.tries += 1;
            lookup.next_at_ms = now_ms + GAME_LOG_RETRY_MS;
            if lookup.tries < GAME_LOG_TRIES || lookup.found.is_some() {
                self.game_sync.dat_lookups.insert(token, lookup);
            }
        }
        Ok(())
    }

    /// Read up to `GAME_SYNCS_PER_TICK` of the files found and sync their
    /// matches with them as one slice. Returns whether a slice was sent.
    fn send_dat_slice(&mut self) -> Result<bool, TrackerError> {
        let mut waiting: Vec<String> = self
            .game_sync
            .dat_lookups
            .iter()
            .filter(|(_, lookup)| lookup.found.is_some())
            .map(|(token, _)| token.clone())
            .collect();
        waiting.sort();
        let mut tokens = Vec::new();
        let mut dat = Vec::new();
        for token in waiting.into_iter().take(GAME_SYNCS_PER_TICK) {
            let Some(mut lookup) = self.game_sync.dat_lookups.remove(&token) else {
                continue;
            };
            let Some(path) = lookup.found.take() else {
                continue;
            };
            // A file gone or unreadable is looked for again at its next try.
            if let Some(stamp) = file_stamp(&path)
                && let Ok(bytes) = std::fs::read(&path)
            {
                lookup.read = Some(stamp);
                tokens.push(token.clone());
                dat.push((token.clone(), bytes));
            }
            if lookup.tries < GAME_LOG_TRIES {
                self.game_sync.dat_lookups.insert(token, lookup);
            }
        }
        if tokens.is_empty() {
            return Ok(false);
        }
        let reply = self.writer.sync_games(tokens.clone(), dat, self.timezone)?;
        self.game_sync.replies.push(SentSync {
            refresh: tokens,
            views: HashMap::new(),
            reply,
            card_resync: false,
        });
        Ok(true)
    }

    /// Every game log file under the data folder, by match token (the
    /// lexically smallest path when a token has several).
    fn game_log_files(&self) -> HashMap<String, PathBuf> {
        let mut files = HashMap::new();
        for path in mtgo_log::list_game_logs(&self.env, self.config.mtgo_data_folder.as_deref()) {
            if let Some(token) = mtgo_log::game_log_token(&path) {
                files.entry(token).or_insert(path);
            }
        }
        files
    }
}
