//! The tournament sync in the tracker (event overlay spec 7.1): the stored
//! `tournament_standings` and `tournament_rounds` are written by the
//! writer's `sync_tournaments` from the stored log.
//!
//! When:
//! - debounced: a stored line of a joined tournament (a change to a joined
//!   state, or any tournament or `InPlayoffs` line of a token the tracker
//!   watches) or the close of a watched round's match marks the sync wanted,
//!   and the first mark sets a deadline `TOURNAMENT_SYNC_AFTER_MS` later. At
//!   the first caught-up tick past it, one sync runs. A backlog read over
//!   many ticks syncs once, at the end, and a burst of match closes in one
//!   tick syncs once. Lobby events (tokens never joined) never mark it;
//! - once at open, when caught up: the fold's version may have risen, and
//!   tournaments may have left the 48-hour window while the tracker was off.
//!
//! The watched tokens are read from the store after every sync (the
//! unfinished tournaments and their newest round's matches), plus the
//! tokens joined since.
//!
//! A failed sync is tried again after `TOURNAMENT_RETRY_FIRST_MS`, doubling
//! up to `TOURNAMENT_RETRY_MAX_MS`.
//!
//! `TournamentsChanged` is raised only when the sync changed a stored
//! standing. It never raises the league events.

use std::collections::HashSet;

use mtgo_protocol::tournament::{
    TournamentFact, decode_fact, is_joined_state, is_match_over_state,
};
use mtgo_protocol::{EventKind, PreparedEvent};
use store::StoreError;

use super::Engine;
use crate::{TrackerError, TrackerEvent};

/// How long after the first change the tournament sync runs (when caught up).
pub const TOURNAMENT_SYNC_AFTER_MS: i64 = 1_000;
/// After a failed tournament sync, the first retry waits this long.
pub const TOURNAMENT_RETRY_FIRST_MS: i64 = 5_000;
/// The longest wait between retries.
pub const TOURNAMENT_RETRY_MAX_MS: i64 = 60_000;

/// The tournament sync's bookkeeping in the engine.
#[derive(Debug, Default)]
pub(crate) struct TournamentSyncState {
    /// When the debounced sync is due (set by the first mark, and at open).
    pub(crate) due_ms: Option<i64>,
    /// Tokens of joined tournaments still running.
    watched_tournaments: HashSet<String>,
    /// Match tokens of the watched tournaments' newest rounds.
    watched_matches: HashSet<String>,
    /// Syncs run so far, failed ones too (tests count them).
    pub(crate) syncs: u32,
    /// The wait before the next retry after a failure; 0 after a success.
    retry_ms: i64,
}

impl Engine {
    /// At open: sync once caught up, and read what to watch.
    pub(super) fn plan_tournament_sync(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        self.tournaments.due_ms = Some(now_ms);
        self.read_watched_tournaments()?;
        Ok(())
    }

    fn read_watched_tournaments(&mut self) -> Result<(), TrackerError> {
        let watched = store::tournament_sync::watched(&self.reader)?;
        self.tournaments.watched_tournaments = watched.tournament_tokens;
        self.tournaments.watched_matches = watched.match_tokens;
        Ok(())
    }

    /// Mark the sync wanted when a stored event is one of its inputs.
    pub(super) fn note_tournament_event(&mut self, event: &PreparedEvent) {
        let state = &mut self.tournaments;
        let wanted = match event.kind {
            // The fold reads only a match's end (its close): other state
            // changes of the round's matches never mark it.
            EventKind::MatchStateChanged => {
                event
                    .subjects
                    .match_token
                    .as_ref()
                    .is_some_and(|token| state.watched_matches.contains(token))
                    && ends_a_match(event)
            }
            EventKind::TournamentStateChanged
            | EventKind::TournamentSync
            | EventKind::TournamentRoundInfo
            | EventKind::TournamentRoundResult
            | EventKind::TournamentPlayerEliminated
            | EventKind::TournamentEnded
            | EventKind::TournamentPlayerState => {
                let Some(token) = &event.subjects.tournament_token else {
                    return;
                };
                if !state.watched_tournaments.contains(token) && joins(event) {
                    state.watched_tournaments.insert(token.clone());
                }
                state.watched_tournaments.contains(token)
            }
            _ => false,
        };
        if wanted && state.due_ms.is_none() {
            state.due_ms = Some(self.batch.now_ms + TOURNAMENT_SYNC_AFTER_MS);
        }
    }

    /// Whether the tournament sync runs at this (caught-up) tick.
    pub(super) fn tournament_sync_due(&self, now_ms: i64) -> bool {
        self.tournaments.due_ms.is_some_and(|due| now_ms >= due)
    }

    /// Run the tournament sync now and raise `TournamentsChanged` when it
    /// changed a stored standing (committed when the writer answers).
    pub(super) fn sync_tournaments(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        self.tournaments.due_ms = None;
        self.tournaments.syncs += 1;
        match self.writer.sync_tournaments(now_ms) {
            Ok((changed, work)) => {
                tracing::debug!(
                    target: "mymtgo::timing",
                    rows = work.rows,
                    micros = work.micros,
                    "tournament_sync"
                );
                self.tournaments.retry_ms = 0;
                if changed {
                    self.pending_events.push(TrackerEvent::TournamentsChanged);
                }
                self.read_watched_tournaments()?;
            }
            Err(StoreError::WriterGone) => return Err(StoreError::WriterGone.into()),
            Err(error) => {
                let state = &mut self.tournaments;
                state.retry_ms = if state.retry_ms == 0 {
                    TOURNAMENT_RETRY_FIRST_MS
                } else {
                    (state.retry_ms * 2).min(TOURNAMENT_RETRY_MAX_MS)
                };
                tracing::warn!(%error, retry_in_ms = state.retry_ms, "the tournament sync failed");
                state.due_ms = Some(now_ms + state.retry_ms);
            }
        }
        Ok(())
    }
}

/// A match state change to an over state (a close, or the account's own
/// match completing).
fn ends_a_match(event: &PreparedEvent) -> bool {
    matches!(
        decode_fact(event.kind, &event.text, None),
        Some(TournamentFact::MatchState { to, .. }) if is_match_over_state(&to)
    )
}

/// A change of a tournament's state to a joined one (the account is in it).
fn joins(event: &PreparedEvent) -> bool {
    if event.kind != EventKind::TournamentStateChanged {
        return false;
    }
    matches!(
        decode_fact(event.kind, &event.text, None),
        Some(TournamentFact::StateChanged { to }) if is_joined_state(&to)
    )
}
