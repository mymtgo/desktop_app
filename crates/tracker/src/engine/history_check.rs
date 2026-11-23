//! Checking match outcomes against MTGO's game history (#454).
//!
//! The live log cannot always say who won: a player who leaves at a level
//! score logs only "Match Tied" (`docs/known_mtgo_lies_and_traps.md`). MTGO's
//! `mtgo_game_history` files name the winner of each match by its id, so
//! once caught up, every `HISTORY_CHECK_EVERY_MS`, the engine looks at them
//! and, when one changed size or modified time, reads them all and stores
//! MTGO's verdict for every stored log match it names the local player in
//! (`store::history_check`). A verdict that differs from the derived outcome
//! replaces it, is logged (it points at a log shape the rules do not know),
//! pushes the match again when the projector holds it, and refolds its
//! leagues. Manual and corrected matches are never checked.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use mtgo_history::{HistoryRecord, Verdict, local_verdict, merge_records, parse_history};
use mtgo_log::history_files::find_history_files;
use mtgo_match::{Derived, MatchView, Outcome};
use store::Platform;
use store::history_check::{HistoryOutcome, history_check_rows};
use store::names::outcome_str;

use super::{Engine, FileStamp, file_stamp};
use crate::{TrackerError, TrackerEvent};

/// How often the history files are looked at.
pub const HISTORY_CHECK_EVERY_MS: i64 = 300_000;

/// A history file's size and modified time when it was last read.
type Stamp = (PathBuf, Option<FileStamp>);

#[derive(Debug, Default)]
pub(super) struct HistoryCheck {
    /// When the files were last looked at. `None` until the first look,
    /// which runs once the tracker has caught up after opening.
    last_check_ms: Option<i64>,
    /// The files as last read: a look that finds the same stamps reads
    /// nothing.
    stamps: Vec<Stamp>,
    /// MTGO's verdicts by match token, so a match pushed again shows it.
    pub(super) verdicts: HashMap<String, Outcome>,
}

impl Engine {
    /// Is a history check due at `now_ms`: never looked yet, or the last
    /// look is `HISTORY_CHECK_EVERY_MS` old.
    pub(super) fn history_check_due(&self, now_ms: i64) -> bool {
        self.history_check
            .last_check_ms
            .is_none_or(|at| now_ms - at >= HISTORY_CHECK_EVERY_MS)
    }

    /// Look at MTGO's history files now and queue a verdict write for each
    /// stored log match whose verdict is new. Returns how many matches got
    /// a new verdict. The writes go in the pending batch; `flush` commits
    /// them.
    pub fn check_history(&mut self, now_ms: i64) -> Result<u32, TrackerError> {
        self.history_check.last_check_ms = Some(now_ms);
        let paths = find_history_files(&self.env, self.config.mtgo_data_folder.as_deref());
        let stamps: Vec<Stamp> = paths
            .iter()
            .map(|path| (path.clone(), file_stamp(path)))
            .collect();
        if stamps == self.history_check.stamps {
            return Ok(0);
        }
        self.history_check.stamps = stamps;
        let records = self.read_history(&paths);
        if records.is_empty() {
            return Ok(0);
        }
        let by_id: HashMap<u64, &HistoryRecord> =
            records.iter().map(|record| (record.id, record)).collect();
        // The stored rows must include every write sent so far.
        self.flush()?;
        let mut written = 0;
        for row in history_check_rows(&self.reader)? {
            let Some(record) = by_id.get(&row.mtgo_match_id) else {
                continue;
            };
            let outcome = match local_verdict(record, &row.username) {
                Some(Verdict::Win) => Outcome::Win,
                Some(Verdict::Loss) => Outcome::Loss,
                None => continue,
            };
            let text = outcome_str(outcome);
            self.history_check
                .verdicts
                .insert(row.token.clone(), outcome);
            if row.history_outcome.as_deref() == Some(text) {
                continue;
            }
            let revision = row.revision.max(self.projector.revision(&row.token)) + 1;
            self.projector.seed_revision(&row.token, revision);
            self.batch.history_outcomes.push(HistoryOutcome {
                token: row.token.clone(),
                outcome,
                revision,
            });
            written += 1;
            if row.outcome != text {
                tracing::warn!(
                    token = %row.token,
                    derived = %row.outcome,
                    mtgo = %text,
                    "MTGO's game history disagrees with the derived outcome"
                );
                self.leagues_changed = true;
                self.mark_account(Some(&row.username));
                self.push_with_verdict(&row.token, revision);
            }
        }
        Ok(written)
    }

    /// `outcome` with MTGO's verdict on `token` over it, if there is one.
    pub(super) fn history_verdict(&self, token: &str, outcome: Outcome) -> Outcome {
        self.history_check
            .verdicts
            .get(token)
            .copied()
            .unwrap_or(outcome)
    }

    /// Every history file's records, merged by id (the last file wins). A
    /// file that cannot be read is logged and left out.
    fn read_history(&self, paths: &[PathBuf]) -> Vec<HistoryRecord> {
        let mut files = Vec::new();
        for path in paths {
            let parsed = fs::read(path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    parse_history(&bytes, self.timezone).map_err(|error| error.to_string())
                });
            match parsed {
                Ok(records) => files.push(records),
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "a game history file could not be read");
                }
            }
        }
        merge_records(files)
    }

    /// Push a match the projector holds again, with its new outcome, unless
    /// the player cannot see it. A match it no longer holds shows the stored
    /// outcome when its page is read.
    fn push_with_verdict(&mut self, token: &str, revision: u64) {
        if !self.visible.contains(token) || self.hidden.contains(token) {
            return;
        }
        if let Some(Derived::Visible(view)) = self.projector.derived(token) {
            self.push_projected((**view).clone(), revision);
        }
    }

    /// Push a projected match the player can see again (no new cause), with
    /// MTGO's verdict over its derived outcome. Every push of a projected
    /// match outside a derivation goes through here.
    pub(super) fn push_projected(&mut self, mut view: MatchView, revision: u64) {
        view.outcome = self.history_verdict(&view.token, view.outcome);
        self.pending_events.push(TrackerEvent::MatchUpdated {
            view,
            revision,
            cause: None,
            manual: false,
            platform: Platform::Mtgo,
            opponent_name: None,
        });
    }
}
